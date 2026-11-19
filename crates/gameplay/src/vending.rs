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

    /// Отказ без заказа (нет доступа).
    pub fn refuse(&mut self, now: f32) {
        self.deny(now);
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
    #[serde(default)]
    pub broken: bool,
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

// ----- Система автоматов -----

use astrocore_api::HookContext;
use astrocore_core::{NetworkEntityId, ObjectId};
use astrocore_math::glam::Vec3;
use astrocore_world::AuthoredKind;

use crate::hands::HeldItem;
use crate::module::State;

#[derive(Debug, Clone)]
pub(crate) struct VendSpec {
    pub(crate) title: String,
    pub(crate) stock: Vec<(String, PrototypeId, u32)>,
    pub(crate) quality: f32,
    pub(crate) eject_delay: f32,
    pub(crate) access: Vec<Vec<String>>,
}

/// Система торговых автоматов: запас и выдача, окна у игроков, отказ без доступа или тока, вылет
/// части запаса при поломке (`GameEvent::EjectVendorItems`).
pub struct VendingSystem {
    specs: std::collections::BTreeMap<PrototypeId, VendSpec>,
    machines: std::collections::BTreeMap<ObjectId, Machine>,
    open: std::collections::BTreeMap<NetworkEntityId, ObjectId>,
    sent: std::collections::BTreeMap<NetworkEntityId, Vec<u8>>,
    serial: u128,
    rng: Rng,
}

impl VendingSystem {
    /// Автоматы — прототипы с компонентом `VendingMachine`; товар без прототипа в Build пропускается.
    pub(crate) fn from_registry(
        registry: &prototypes::PrototypeRegistry,
    ) -> Result<Self, prototypes::ProtoError> {
        let names = registry.engine_index(crate::module::NAMESPACE)?;
        let by_name: std::collections::BTreeMap<String, PrototypeId> = names
            .iter()
            .map(|(id, n)| (n.clone(), id.clone()))
            .collect();
        let mut specs = std::collections::BTreeMap::new();
        for (id, name) in &names {
            let Ok(r) = registry.spawnable(name) else {
                continue;
            };
            if let Some(c) = r.component("VendingMachine") {
                let stock = c
                    .get("stock")
                    .and_then(prototypes::Data::as_map)
                    .map(|s| {
                        s.iter()
                            .filter_map(|(k, v)| {
                                // Нет такого прототипа в Build — позиция пропускается (`HasIndex`).
                                registry.spawnable(k).ok()?;
                                Some((k.clone(), by_name.get(k)?.clone(), v.as_f64()? as u32))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                specs.insert(
                    id.clone(),
                    VendSpec {
                        title: name.clone(),
                        stock,
                        quality: c
                            .get("initialStockQuality")
                            .and_then(prototypes::Data::as_f64)
                            .unwrap_or(1.0) as f32,
                        access: r
                            .component("AccessReader")
                            .map(crate::module::access_lists)
                            .unwrap_or_default(),
                        eject_delay: c
                            .get("ejectDelay")
                            .and_then(prototypes::Data::as_f64)
                            .unwrap_or(1.2) as f32,
                    },
                );
            }
        }
        Ok(Self {
            specs,
            machines: std::collections::BTreeMap::new(),
            open: std::collections::BTreeMap::new(),
            sent: std::collections::BTreeMap::new(),
            serial: 0,
            rng: Rng(0x2545_f491_4f6c_dd1d),
        })
    }
}

impl crate::systems::GameSystem for VendingSystem {
    fn id(&self) -> &'static str {
        "vending"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[crate::wire::CMD_VEND, crate::wire::CMD_VEND_CLOSE]
    }

    fn command(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        kind: &str,
        data: &[u8],
    ) {
        if kind == crate::wire::CMD_VEND_CLOSE {
            self.open.remove(&actor);
        } else if st.can_act_now(actor)
            && let Ok(item) = std::str::from_utf8(data)
        {
            self.vend(st, ctx, actor, item);
        }
    }

    fn interact_hand(
        &mut self,
        st: &mut State,
        _ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &crate::interact::Target,
    ) -> bool {
        let crate::interact::Target::Item { id, .. } = target else {
            return false;
        };
        self.open_vending(st, user, *id)
    }

    fn on_event(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        event: &crate::systems::GameEvent,
    ) {
        let crate::systems::GameEvent::EjectVendorItems {
            object,
            percent,
            max,
            at,
        } = event;
        self.eject_damaged(st, ctx, *object, *percent, *max, *at);
    }

    fn tick(&mut self, st: &mut State, ctx: &mut HookContext<'_>, _dt: f32) {
        self.tick_vending(st, ctx);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl VendingSystem {
    /// Автоматы из карты получают начальный запас при первом появлении (`MapInit`).
    fn sync_vending(&mut self, ctx: &HookContext<'_>) {
        let mut seen = std::collections::BTreeSet::new();
        for o in ctx.world.authored() {
            let AuthoredKind::Placement { prototype } = &o.kind else {
                continue;
            };
            let Some(spec) = self.specs.get(prototype) else {
                continue;
            };
            seen.insert(o.id);
            if !self.machines.contains_key(&o.id) {
                let machine = crate::vending::Machine::new(
                    spec.stock.iter().cloned(),
                    spec.quality,
                    spec.eject_delay,
                    &mut self.rng,
                );
                self.machines.insert(o.id, machine);
            }
        }
        self.machines.retain(|id, _| seen.contains(id));
    }

    /// Окно автомата открывается пустой рукой, если есть ток (`ActivatableUIRequiresPower`).
    pub(crate) fn open_vending(
        &mut self,
        st: &mut State,
        user: NetworkEntityId,
        id: ObjectId,
    ) -> bool {
        if !self.machines.contains_key(&id) {
            return false;
        }
        if st.power_required && !st.powered_map().get(&id).copied().unwrap_or(false) {
            return true;
        }
        self.open.insert(user, id);
        true
    }

    /// Заказ товара из открытого автомата.
    pub(crate) fn vend(
        &mut self,
        st: &mut State,
        ctx: &HookContext<'_>,
        user: NetworkEntityId,
        item: &str,
    ) {
        let Some(id) = self.open.get(&user).copied() else {
            return;
        };
        let now = st.clock;
        let powered = !st.power_required || st.powered_map().get(&id).copied().unwrap_or(false);
        let powered = powered && !st.repairable_broken(ctx, id);
        // Доступ к автомату (`IsAuthorized`): без нужного уровня — отказ.
        let allowed = ctx
            .world
            .authored_object(id)
            .and_then(|o| match &o.kind {
                AuthoredKind::Placement { prototype } => self.specs.get(prototype),
                _ => None,
            })
            .is_none_or(|s| State::access_ok(&s.access, &st.user_access(user)));
        let at = State::center_of(ctx, id);
        let mut say: Option<(&str, &str)> = None;
        if let Some(machine) = self.machines.get_mut(&id) {
            if allowed {
                say = match machine.order(now, item, powered) {
                    Ok(()) => Some(("Machines/machine_vend.ogg", "")),
                    Err(crate::vending::Refusal::Denied) => Some((
                        "Machines/buzz-sigh.ogg",
                        "Нет такого товара или он кончился",
                    )),
                    Err(_) => None,
                };
            } else {
                machine.refuse(now);
                say = Some(("Machines/buzz-sigh.ogg", "Доступ запрещён"));
            }
        }
        if let (Some((path, text)), Some(at)) = (say, at) {
            st.sound(path, at);
            if !text.is_empty() {
                st.popup(user, text);
            }
        }
    }

    /// Автоматы каждый кадр: выдача готовых товаров, окна у игроков.
    fn tick_vending(&mut self, st: &mut State, ctx: &mut HookContext<'_>) {
        if self.specs.is_empty() {
            return;
        }
        self.sync_vending(ctx);
        let now = st.clock;
        let ids: Vec<ObjectId> = self.machines.keys().copied().collect();
        for id in ids {
            let Some(done) = self.machines.get_mut(&id).and_then(|m| m.update(now)) else {
                continue;
            };
            let Some(o) = ctx.world.authored_object(id) else {
                continue;
            };
            // Товар ложится у автомата, на полу перед ним.
            let c = (o.min() + o.max()) * 0.5;
            self.serial += 1;
            let item = HeldItem {
                id: ObjectId::from_u128(0xD3AD_0000_0000 + self.serial),
                prototype: done,
            };
            st.place(ctx, &item, Vec3::new(c.x, o.min().y, c.z));
        }
        // Окна: закрыть, если игрок ушёл от автомата; послать изменения.
        let players: Vec<NetworkEntityId> = ctx.players().collect();
        let mut views: Vec<(NetworkEntityId, Vec<u8>)> = Vec::new();
        for p in players {
            let open = self.open.get(&p).copied();
            let view = open.and_then(|id| {
                let o = ctx.world.authored_object(id)?;
                let feet = ctx.position_of(p)?;
                let c = (o.min() + o.max()) * 0.5;
                if crate::module::reach_distance(feet, c) > st.range_m + 1.0 {
                    return None;
                }
                let machine = self.machines.get(&id)?;
                let AuthoredKind::Placement { prototype } = &o.kind else {
                    return None;
                };
                Some(crate::vending::VendingView {
                    machine: id,
                    name: self
                        .specs
                        .get(prototype)
                        .map(|s| s.title.clone())
                        .unwrap_or_default(),
                    items: machine
                        .entries
                        .iter()
                        .map(|e| crate::vending::VendingItem {
                            name: e.name.clone(),
                            proto: e.proto.to_string(),
                            amount: e.amount,
                        })
                        .collect(),
                    ejecting: machine.ejecting(),
                    denying: machine.denying(),
                    broken: st.repairable_broken(ctx, id),
                })
            });
            if view.is_none() {
                self.open.remove(&p);
            }
            let bytes = crate::vending::encode_vending(view.as_ref());
            if self.sent.get(&p) != Some(&bytes) {
                views.push((p, bytes));
            }
        }
        for (p, bytes) in views {
            ctx.set_component(p, crate::vending::VENDING_COMPONENT, bytes.clone());
            self.sent.insert(p, bytes);
        }
    }

    fn eject_damaged(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        id: ObjectId,
        percent: f32,
        max: f32,
        at: Vec3,
    ) {
        self.sync_vending(ctx);
        let available = self.machines.get(&id).map_or(0, |vendor| {
            vendor
                .entries
                .iter()
                .filter(|entry| entry.amount > 0)
                .count()
        });
        let count = ((available as f32 * percent).min(max)).ceil() as usize;
        for _ in 0..count {
            let choices: Vec<_> = self
                .machines
                .get(&id)
                .into_iter()
                .flat_map(|vendor| vendor.entries.iter().enumerate())
                .filter(|(_, entry)| entry.amount > 0)
                .map(|(index, _)| index)
                .collect();
            if choices.is_empty() {
                break;
            }
            let index = choices[(st.spawn_rng.next_f32() * choices.len() as f32) as usize];
            let entry = &mut self.machines.get_mut(&id).unwrap().entries[index];
            entry.amount -= 1;
            let prototype = entry.proto.clone();
            if let Some(name) = st.ss14_names.get(&prototype).cloned()
                && let Some(item) = st.create_item(&name)
            {
                st.place(ctx, &item, at);
            }
        }
    }
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
            broken: false,
        };
        assert_eq!(decode_vending(&encode_vending(Some(&v))), Some(v));
        assert_eq!(decode_vending(&encode_vending(None)), None);
    }
}
