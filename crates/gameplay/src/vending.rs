//! Торговые автоматы (`SharedVendingMachineSystem`, `VendingMachineSystem`, снимок SS14 `910154a7`).
//!
//! Товары автомата — `startingInventory` его набора; предметы без прототипа в Build пропускаются
//! (`ProtoMan.HasIndex` в `AddInventoryFromPrototype`). Начальный запас зависит от
//! `initialStockQuality`: с шансом `1 − качество` позиция укомплектована частично. Заказ
//! (`TryEjectVendorItem`): нужен ток, целая машина, свободный выдатчик и товар в наличии; иначе
//! автомат отказывает 2 с. Товар появляется после `ejectDelay` (1,2 с по умолчанию).
//!
//! Не перенесено: доступ по ID-картам (`AccessReader`), контрабандный и взломанный запас,
//! перезарядка, поломка и вылет товара при ударе, звук.

use astrocore_core::PrototypeId;
use serde::{Deserialize, Serialize};

use crate::disposal::Rng;

/// Состояние окна автомата у игрока; пустое значение — окно закрыто.
pub const VENDING_COMPONENT: &str = "build.vending";
pub const VENDING_MAX_BYTES: usize = 8192;

/// `VendingMachineEjectComponent.DenyDelay`.
pub const DENY_DELAY_S: f32 = 2.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub proto: PrototypeId,
    pub name: String,
    pub amount: u32,
}

/// Автомат в мире.
#[derive(Debug, Clone)]
pub struct Machine {
    pub entries: Vec<Entry>,
    pub eject_delay: f32,
    eject_end: Option<f32>,
    deny_end: Option<f32>,
    next: Option<PrototypeId>,
}

/// Почему автомат не выдал товар.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Идёт выдача или нет тока: молча игнорируется.
    Busy,
    /// Нет такого товара или он кончился: автомат отказывает со звуком.
    Denied,
}

impl Machine {
    /// Запас `stock` (имя, прототип, количество) с учётом `quality` (`RestockInventoryFromPrototype`).
    pub fn new(
        stock: impl IntoIterator<Item = (String, PrototypeId, u32)>,
        quality: f32,
        eject_delay: f32,
        rng: &mut Rng,
    ) -> Self {
        let missing = 1.0 - quality;
        let entries = stock
            .into_iter()
            .map(|(name, proto, amount)| {
                let roll = rng.next_f32();
                let restock = if roll < missing {
                    (amount as f32 * roll / missing).floor() as u32
                } else {
                    amount
                };
                Entry {
                    proto,
                    name,
                    amount: restock,
                }
            })
            .collect();
        Self {
            entries,
            eject_delay,
            eject_end: None,
            deny_end: None,
            next: None,
        }
    }

    pub fn ejecting(&self) -> bool {
        self.eject_end.is_some()
    }

    pub fn denying(&self) -> bool {
        self.deny_end.is_some()
    }

    fn deny(&mut self, now: f32) {
        if self.deny_end.is_none() {
            self.deny_end = Some(now + DENY_DELAY_S);
        }
    }

    /// `TryEjectVendorItem`.
    pub fn order(&mut self, now: f32, name: &str, powered: bool) -> Result<(), Refusal> {
        if self.ejecting() || !powered {
            return Err(Refusal::Busy);
        }
        let Some(entry) = self.entries.iter_mut().find(|e| e.name == name) else {
            self.deny(now);
            return Err(Refusal::Denied);
        };
        if entry.amount == 0 {
            self.deny(now);
            return Err(Refusal::Denied);
        }
        entry.amount -= 1;
        self.next = Some(entry.proto.clone());
        self.eject_end = Some(now + self.eject_delay);
        Ok(())
    }

    /// Ход времени: по окончании выдачи возвращает выданный товар.
    pub fn update(&mut self, now: f32) -> Option<PrototypeId> {
        if self.deny_end.is_some_and(|d| now > d) {
            self.deny_end = None;
        }
        if self.eject_end.is_some_and(|e| now > e) {
            self.eject_end = None;
            return self.next.take();
        }
        None
    }
}

/// Для клиента: окно автомата.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VendingView {
    pub machine: astrocore_core::ObjectId,
    pub name: String,
    pub items: Vec<VendingItem>,
    pub ejecting: bool,
    pub denying: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VendingItem {
    /// Имя прототипа SS14 (то, что присылается в команде заказа).
    pub name: String,
    /// Идентификатор прототипа движка (для значка).
    pub proto: String,
    pub amount: u32,
}

pub fn encode_vending(v: Option<&VendingView>) -> Vec<u8> {
    v.map(|v| serde_json::to_vec(v).unwrap_or_default())
        .unwrap_or_default()
}

pub fn decode_vending(bytes: &[u8]) -> Option<VendingView> {
    if bytes.is_empty() || bytes.len() > VENDING_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> PrototypeId {
        PrototypeId::new(s).unwrap()
    }

    fn machine(quality: f32) -> Machine {
        Machine::new(
            [
                ("Crowbar".to_string(), id("ss14:crowbar"), 3),
                ("Wrench".to_string(), id("ss14:wrench"), 1),
            ],
            quality,
            1.2,
            &mut Rng(5),
        )
    }

    #[test]
    fn a_full_machine_dispenses_after_the_eject_delay_and_counts_down() {
        let mut m = machine(1.0);
        assert!(m.order(10.0, "Crowbar", true).is_ok());
        assert!(m.ejecting());
        assert_eq!(m.update(10.5), None);
        // Пока выдаёт, новый заказ молча отклонён.
        assert_eq!(m.order(10.6, "Wrench", true), Err(Refusal::Busy));
        assert_eq!(m.update(11.3), Some(id("ss14:crowbar")));
        assert_eq!(m.entries[0].amount, 2);
        assert!(!m.ejecting());
    }

    #[test]
    fn an_empty_slot_or_unknown_item_is_denied_for_two_seconds() {
        let mut m = machine(1.0);
        m.order(0.0, "Wrench", true).unwrap();
        m.update(2.0);
        assert_eq!(m.order(3.0, "Wrench", true), Err(Refusal::Denied));
        assert!(m.denying());
        assert_eq!(m.order(3.1, "Nothing", true), Err(Refusal::Denied));
        m.update(5.5);
        assert!(!m.denying());
    }

    #[test]
    fn a_machine_without_power_does_nothing() {
        let mut m = machine(1.0);
        assert_eq!(m.order(0.0, "Crowbar", false), Err(Refusal::Busy));
        assert_eq!(m.entries[0].amount, 3);
    }

    #[test]
    fn low_stock_quality_leaves_some_slots_short() {
        let stock: Vec<_> = (0..40)
            .map(|i| (format!("I{i}"), id("ss14:crowbar"), 10))
            .collect();
        let m = Machine::new(stock, 0.33, 1.2, &mut Rng(9));
        let total: u32 = m.entries.iter().map(|e| e.amount).sum();
        assert!(total < 400 && total > 100, "{total}");
        assert!(m.entries.iter().all(|e| e.amount <= 10));
        assert!(m.entries.iter().any(|e| e.amount == 10));
    }

    #[test]
    fn the_wire_form_round_trips() {
        let v = VendingView {
            machine: astrocore_core::ObjectId::from_u128(7),
            name: "YouTool".into(),
            items: vec![VendingItem {
                name: "Crowbar".into(),
                proto: "ss14:crowbar".into(),
                amount: 4,
            }],
            ejecting: false,
            denying: true,
        };
        assert_eq!(decode_vending(&encode_vending(Some(&v))), Some(v));
        assert_eq!(decode_vending(&encode_vending(None)), None);
    }
}
