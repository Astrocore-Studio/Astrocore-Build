use serde::{Deserialize, Serialize};

use crate::hands::HeldItem;
use crate::items::{ItemInfo, ItemSize};

/// Самый большой предмет, который помещается в карман (`PocketableItemSize` в SS14).
pub const POCKETABLE_SIZE: ItemSize = ItemSize::Small;

/// Слот инвентаря: имя, флаг (какую одежду принимает) и слот, без которого он не работает.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotDef {
    pub name: String,
    /// Флаг слота в нижнем регистре (`feet`, `pocket`, …); сравнивается с `Clothing.slots` предмета.
    pub flag: String,
    pub depends_on: Option<String>,
}

fn slot(name: &str, flag: &str, depends_on: Option<&str>) -> SlotDef {
    SlotDef {
        name: name.into(),
        flag: flag.into(),
        depends_on: depends_on.map(str::to_string),
    }
}

/// Шаблон инвентаря гуманоида (`human_inventory_template.yml`): карманы и удостоверение зависят от
/// формы, слот на костюме — от верхней одежды.
pub fn human_template() -> Vec<SlotDef> {
    vec![
        slot("shoes", "feet", None),
        slot("jumpsuit", "innerclothing", None),
        slot("outerClothing", "outerclothing", None),
        slot("gloves", "gloves", None),
        slot("neck", "neck", None),
        slot("mask", "mask", None),
        slot("eyes", "eyes", None),
        slot("ears", "ears", None),
        slot("head", "head", None),
        slot("pocket1", "pocket", Some("jumpsuit")),
        slot("pocket2", "pocket", Some("jumpsuit")),
        slot("suitstorage", "suitstorage", Some("outerClothing")),
        slot("id", "idcard", Some("jumpsuit")),
        slot("belt", "belt", None),
        slot("back", "back", None),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InventoryError {
    #[error("такого слота нет")]
    NoSuchSlot,
    #[error("слот занят")]
    Occupied,
    #[error("слот пуст")]
    Empty,
    #[error("слот недоступен: не надет {0}")]
    MissingDependency(String),
    #[error("предмет не подходит к этому слоту")]
    DoesNotFit,
}

/// Одежда и экипировка: слоты шаблона и надетые предметы.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    slots: Vec<(SlotDef, Option<HeldItem>)>,
}

impl Inventory {
    pub fn new(template: Vec<SlotDef>) -> Self {
        Self {
            slots: template.into_iter().map(|d| (d, None)).collect(),
        }
    }

    pub fn human() -> Self {
        Self::new(human_template())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SlotDef, Option<&HeldItem>)> {
        self.slots.iter().map(|(d, i)| (d, i.as_ref()))
    }

    pub fn get(&self, slot: &str) -> Option<&HeldItem> {
        self.slots
            .iter()
            .find(|(d, _)| d.name == slot)
            .and_then(|(_, i)| i.as_ref())
    }

    pub fn worn(&self) -> impl Iterator<Item = &HeldItem> {
        self.slots.iter().filter_map(|(_, i)| i.as_ref())
    }

    /// Правила `CanEquip`: слот существует и пуст, слот-зависимость занят, а предмет либо
    /// одежда для этого флага, либо (в карман) не больше `Small`.
    pub fn can_equip(&self, slot: &str, item: &ItemInfo) -> Result<(), InventoryError> {
        let (def, current) = self
            .slots
            .iter()
            .find(|(d, _)| d.name == slot)
            .ok_or(InventoryError::NoSuchSlot)?;
        if current.is_some() {
            return Err(InventoryError::Occupied);
        }
        if let Some(dep) = &def.depends_on
            && self.get(dep).is_none()
        {
            return Err(InventoryError::MissingDependency(dep.clone()));
        }
        let fits_pocket = def.flag == "pocket" && item.size <= POCKETABLE_SIZE;
        if !fits_pocket && !item.slots.contains(&def.flag) {
            return Err(InventoryError::DoesNotFit);
        }
        Ok(())
    }

    pub fn equip(
        &mut self,
        slot: &str,
        item: HeldItem,
        info: &ItemInfo,
    ) -> Result<(), InventoryError> {
        self.can_equip(slot, info)?;
        if let Some((_, s)) = self.slots.iter_mut().find(|(d, _)| d.name == slot) {
            *s = Some(item);
        }
        Ok(())
    }

    /// Снять предмет. Возвращает снятое и всё, что выпало из зависимых слотов (в SS14 снятая
    /// форма роняет карманы и удостоверение; рекурсивно).
    pub fn unequip(&mut self, slot: &str) -> Result<(HeldItem, Vec<HeldItem>), InventoryError> {
        let idx = self
            .slots
            .iter()
            .position(|(d, _)| d.name == slot)
            .ok_or(InventoryError::NoSuchSlot)?;
        let item = self.slots[idx].1.take().ok_or(InventoryError::Empty)?;
        let mut dropped = Vec::new();
        let mut pending = vec![slot.to_string()];
        while let Some(parent) = pending.pop() {
            for (d, s) in &mut self.slots {
                if d.depends_on.as_deref() == Some(parent.as_str())
                    && let Some(i) = s.take()
                {
                    dropped.push(i);
                    pending.push(d.name.clone());
                }
            }
        }
        Ok((item, dropped))
    }

    /// Снять всё (игрок вышел).
    pub fn take_all(&mut self) -> Vec<HeldItem> {
        self.slots
            .iter_mut()
            .filter_map(|(_, i)| i.take())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use astrocore_core::{ObjectId, PrototypeId};

    use super::*;

    fn held(n: u128) -> HeldItem {
        HeldItem {
            id: ObjectId::from_u128(n),
            prototype: PrototypeId::new("ss14:thing").unwrap(),
        }
    }

    fn clothing(size: ItemSize, slots: &[&str]) -> ItemInfo {
        ItemInfo {
            size,
            slots: slots.iter().map(|s| (*s).to_string()).collect(),
            storage: None,
            qualities: BTreeSet::new(),
            tags: BTreeSet::new(),
            components: BTreeSet::new(),
        }
    }

    #[test]
    fn clothing_fits_only_its_slot_and_a_slot_holds_one_item() {
        let mut inv = Inventory::human();
        let boots = clothing(ItemSize::Normal, &["feet"]);
        assert_eq!(
            inv.can_equip("head", &boots),
            Err(InventoryError::DoesNotFit)
        );
        assert_eq!(
            inv.can_equip("nowhere", &boots),
            Err(InventoryError::NoSuchSlot)
        );
        inv.equip("shoes", held(1), &boots).unwrap();
        assert_eq!(
            inv.can_equip("shoes", &boots),
            Err(InventoryError::Occupied)
        );
        assert_eq!(inv.get("shoes").unwrap().id, ObjectId::from_u128(1));
    }

    #[test]
    fn pockets_need_a_jumpsuit_and_take_small_items_only() {
        let mut inv = Inventory::human();
        let pen = clothing(ItemSize::Tiny, &[]);
        let crate_ = clothing(ItemSize::Large, &[]);
        let suit = clothing(ItemSize::Normal, &["innerclothing"]);
        assert_eq!(
            inv.can_equip("pocket1", &pen),
            Err(InventoryError::MissingDependency("jumpsuit".into())),
            "без формы карманов нет"
        );
        inv.equip("jumpsuit", held(1), &suit).unwrap();
        assert!(inv.equip("pocket1", held(2), &pen).is_ok());
        assert_eq!(
            inv.can_equip("pocket2", &crate_),
            Err(InventoryError::DoesNotFit)
        );
        // Small ещё помещается, Normal — уже нет.
        assert!(
            inv.can_equip("pocket2", &clothing(ItemSize::Small, &[]))
                .is_ok()
        );
        assert_eq!(
            inv.can_equip("pocket2", &clothing(ItemSize::Normal, &[])),
            Err(InventoryError::DoesNotFit)
        );
    }

    #[test]
    fn removing_a_suit_drops_everything_that_depends_on_it() {
        let mut inv = Inventory::human();
        let suit = clothing(ItemSize::Normal, &["innerclothing"]);
        let armor = clothing(ItemSize::Normal, &["outerclothing"]);
        let tank = clothing(ItemSize::Normal, &["suitstorage"]);
        let pen = clothing(ItemSize::Tiny, &[]);
        let idc = clothing(ItemSize::Tiny, &["idcard"]);
        inv.equip("jumpsuit", held(1), &suit).unwrap();
        inv.equip("outerClothing", held(2), &armor).unwrap();
        inv.equip("suitstorage", held(3), &tank).unwrap();
        inv.equip("pocket1", held(4), &pen).unwrap();
        inv.equip("id", held(5), &idc).unwrap();
        // Снимаем верхнюю одежду: выпадает только то, что от неё зависит.
        let (item, dropped) = inv.unequip("outerClothing").unwrap();
        assert_eq!(item.id, ObjectId::from_u128(2));
        assert_eq!(
            dropped.iter().map(|i| i.id).collect::<Vec<_>>(),
            vec![ObjectId::from_u128(3)]
        );
        assert!(inv.get("pocket1").is_some());
        // Снимаем форму: карманы и удостоверение выпадают.
        let (_, mut dropped) = inv.unequip("jumpsuit").unwrap();
        dropped.sort_by_key(|i| i.id);
        assert_eq!(
            dropped.iter().map(|i| i.id).collect::<Vec<_>>(),
            vec![ObjectId::from_u128(4), ObjectId::from_u128(5)]
        );
        assert_eq!(inv.worn().count(), 0);
        assert_eq!(inv.unequip("jumpsuit"), Err(InventoryError::Empty));
        assert_eq!(inv.unequip("zzz"), Err(InventoryError::NoSuchSlot));
    }

    #[test]
    fn the_whole_inventory_round_trips_through_json_and_empties_on_demand() {
        let mut inv = Inventory::human();
        inv.equip("belt", held(1), &clothing(ItemSize::Small, &["belt"]))
            .unwrap();
        let json = serde_json::to_string(&inv).unwrap();
        assert_eq!(serde_json::from_str::<Inventory>(&json).unwrap(), inv);
        assert_eq!(inv.take_all().len(), 1);
    }
}
