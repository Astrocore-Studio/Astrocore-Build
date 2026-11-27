//! Инвентарь: слоты одежды и карманов по шаблону (`inventoryTemplate`), в каждом слоте —
//! контейнер на один предмет. Надевание и снятие с проверками, задержкой и выпадением
//! зависимых слотов; клик по слоту. Образец — `Content.Shared/Inventory` SS14 @ 2f4a2aad
//! (`InventorySystem.Equip`, `InventorySystem.Slots`, `SlotFlags`) и
//! `Content.Shared/Clothing` (`ClothingComponent`, быстрое надевание).

use std::fmt;

use astrocore_ecs::{ComponentInit, EventWorldExt, LifecycleWorldExt, RegistryWorldExt};
use astrocore_map::{
    ContainerKind, ContainerWorldExt, EntGotInsertedIntoContainer, EntGotRemovedFromContainer,
    TransformWorldExt,
};
use astrocore_net::{NetEvent, NetEventWorldExt, NetRegistryWorldExt, Session};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use astrocore_audio::{AudioWorldExt, SoundSpecifier};

use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterWorldExt};
use crate::examine::loc;
use crate::hands::{HandDeselected, HandsWorldExt};
use crate::interaction::{INTERACTION_RANGE, InteractionWorldExt, UseInHand};
use crate::item::{Item, ItemLayer, size_weight};
use crate::popups::{PopupType, PopupWorldExt};
use crate::whitelist::EntityWhitelist;

/// Наибольший размер предмета, который влезает в карман (`PocketableItemSize`).
pub const POCKETABLE_ITEM_SIZE: &str = "Small";

/// Флаги слотов (`SlotFlags`): в какие слоты можно надеть вещь.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SlotFlags(pub u32);

impl SlotFlags {
    pub const NONE: SlotFlags = SlotFlags(0);
    pub const PREVENTEQUIP: SlotFlags = SlotFlags(1 << 0);
    pub const POCKET: SlotFlags = SlotFlags(1 << 12);
    pub const HEAD: SlotFlags = SlotFlags(1 << 1);
    pub const MASK: SlotFlags = SlotFlags(1 << 4);
    pub const GLOVES: SlotFlags = SlotFlags(1 << 10);

    const NAMES: [(&'static str, u32); 18] = [
        ("NONE", 0),
        ("PREVENTEQUIP", 1 << 0),
        ("HEAD", 1 << 1),
        ("EYES", 1 << 2),
        ("EARS", 1 << 3),
        ("MASK", 1 << 4),
        ("OUTERCLOTHING", 1 << 5),
        ("INNERCLOTHING", 1 << 6),
        ("NECK", 1 << 7),
        ("BACK", 1 << 8),
        ("BELT", 1 << 9),
        ("GLOVES", 1 << 10),
        ("IDCARD", 1 << 11),
        ("POCKET", 1 << 12),
        ("LEGS", 1 << 13),
        ("FEET", 1 << 14),
        ("SUITSTORAGE", 1 << 15),
        ("ALL", u32::MAX),
    ];

    fn named(name: &str) -> Option<u32> {
        let name = name.trim();
        if name.eq_ignore_ascii_case("WITHOUT_POCKET") {
            return Some(!Self::POCKET.0);
        }
        Self::NAMES
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, bits)| *bits)
            .or_else(|| name.parse().ok())
    }

    /// Все флаги `other` есть в `self` (`HasFlag`).
    pub fn has(self, other: SlotFlags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: SlotFlags) -> bool {
        self.0 & other.0 != 0
    }
}

impl Serialize for SlotFlags {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}

impl<'de> Deserialize<'de> for SlotFlags {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FlagsVisitor;

        impl<'de> Visitor<'de> for FlagsVisitor {
            type Value = SlotFlags;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("флаги слотов: имя, имена через запятую или список")
            }

            fn visit_u64<E: de::Error>(self, bits: u64) -> Result<SlotFlags, E> {
                Ok(SlotFlags(bits as u32))
            }

            fn visit_i64<E: de::Error>(self, bits: i64) -> Result<SlotFlags, E> {
                Ok(SlotFlags(bits as u32))
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<SlotFlags, E> {
                text.split(',').try_fold(SlotFlags::NONE, |flags, name| {
                    SlotFlags::named(name)
                        .map(|bits| SlotFlags(flags.0 | bits))
                        .ok_or_else(|| E::custom(format!("неизвестный флаг слота `{name}`")))
                })
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<SlotFlags, A::Error> {
                let mut flags = 0;
                while let Some(SlotFlags(bits)) = seq.next_element()? {
                    flags |= bits;
                }
                Ok(SlotFlags(flags))
            }
        }

        if !deserializer.is_human_readable() {
            return u32::deserialize(deserializer).map(SlotFlags);
        }
        deserializer.deserialize_any(FlagsVisitor)
    }
}

fn pocket_texture() -> String {
    "pocket".to_owned()
}

fn prevent_equip() -> SlotFlags {
    SlotFlags::PREVENTEQUIP
}

fn yes() -> bool {
    true
}

fn strip_time() -> f64 {
    4.0
}

/// Слот шаблона (`SlotDefinition`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotDefinition {
    pub name: String,
    #[serde(default = "pocket_texture", rename = "slotTexture")]
    pub texture_name: String,
    #[serde(default = "prevent_equip")]
    pub slot_flags: SlotFlags,
    #[serde(default = "yes")]
    pub show_in_window: bool,
    #[serde(default)]
    pub slot_group: Option<String>,
    #[serde(default = "strip_time")]
    pub strip_time: f64,
    /// Место в окне инвентаря: столбец и строка снизу.
    #[serde(default)]
    pub ui_window_pos: astrocore_math::IVec2,
    /// Слот, без вещи в котором этот недоступен (карманы — без комбинезона).
    #[serde(default)]
    pub depends_on: Option<String>,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub whitelist: Option<EntityWhitelist>,
    #[serde(default)]
    pub blacklist: Option<EntityWhitelist>,
}

fn human() -> String {
    "human".to_owned()
}

/// Инвентарь сущности. В данных — компонент `Inventory` с шаблоном `templateId`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    #[serde(default = "human")]
    pub template_id: String,
    #[serde(default)]
    pub species_id: Option<String>,
    /// Слоты шаблона: заполняются из прототипа при создании.
    #[serde(skip)]
    pub slots: Vec<SlotDefinition>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            template_id: human(),
            species_id: None,
            slots: Vec::new(),
        }
    }
}

/// Одежда. В данных — компонент `Clothing`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clothing {
    /// Слои на носящем по слотам вместо состояний по умолчанию.
    #[serde(default)]
    pub clothing_visuals: std::collections::BTreeMap<String, Vec<ItemLayer>>,
    /// Надевается использованием в руке.
    #[serde(default = "yes")]
    pub quick_equip: bool,
    /// Слоты, в которые надевается.
    #[serde(default)]
    pub slots: SlotFlags,
    #[serde(default)]
    pub equipped_prefix: Option<String>,
    #[serde(default)]
    pub equipped_state: Option<String>,
    /// Набор RSI на носящем; без него — набор спрайта вещи.
    #[serde(default)]
    pub sprite: Option<String>,
    /// Задержка надевания и снятия, секунды.
    #[serde(default)]
    pub equip_delay: f64,
    #[serde(default)]
    pub unequip_delay: f64,
    #[serde(default)]
    pub equip_while_moving: bool,
    /// Слот, в котором вещь сейчас.
    #[serde(default)]
    pub in_slot: Option<String>,
    #[serde(default)]
    pub equip_sound: Option<SoundSpecifier>,
    #[serde(default)]
    pub unequip_sound: Option<SoundSpecifier>,
}

impl Default for Clothing {
    fn default() -> Self {
        Self {
            clothing_visuals: Default::default(),
            quick_equip: true,
            slots: SlotFlags::NONE,
            equipped_prefix: None,
            equipped_state: None,
            sprite: None,
            equip_delay: 0.0,
            unequip_delay: 0.0,
            equip_while_moving: false,
            in_slot: None,
            equip_sound: None,
            unequip_sound: None,
        }
    }
}

/// Попытка надеть (`IsEquippingAttemptEvent` и соседи). Поднимается на действующем, на
/// цели и на вещи.
#[derive(Debug, Clone)]
pub struct EquipAttempt {
    pub user: Entity,
    pub target: Entity,
    pub item: Entity,
    pub slot: String,
    pub cancelled: bool,
    /// Ключ сообщения с причиной отказа.
    pub reason: Option<String>,
}

/// Попытка снять (`IsUnequippingAttemptEvent` и соседи).
#[derive(Debug, Clone)]
pub struct UnequipAttempt {
    pub user: Entity,
    pub target: Entity,
    pub item: Entity,
    pub slot: String,
    pub cancelled: bool,
    pub reason: Option<String>,
}

/// Вещь надета (`DidEquipEvent` на носящем, `GotEquippedEvent` на вещи).
#[derive(Debug, Clone)]
pub struct Equipped {
    pub wearer: Entity,
    pub item: Entity,
    pub slot: String,
    pub slot_flags: SlotFlags,
}

/// Вещь снята (`DidUnequipEvent` на носящем, `GotUnequippedEvent` на вещи).
#[derive(Debug, Clone)]
pub struct Unequipped {
    pub wearer: Entity,
    pub item: Entity,
    pub slot: String,
    pub slot_flags: SlotFlags,
}

/// Итог надевания с задержкой (`ClothingEquipDoAfterEvent`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClothingEquipDoAfter {
    pub slot: String,
}

/// Итог снятия с задержкой (`ClothingUnequipDoAfterEvent`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClothingUnequipDoAfter {
    pub slot: String,
}

/// Клик по слоту инвентаря (`UseSlotNetworkMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UseSlot {
    pub slot: String,
}

/// Слоты шаблона из прототипов.
fn template_slots(world: &World, template: &str) -> Vec<SlotDefinition> {
    world
        .get_resource::<Prototypes>()
        .and_then(|prototypes| prototypes.resolved_node("inventoryTemplate", template))
        .and_then(|node| node.get("slots"))
        .and_then(|slots| astrocore_data::from_node::<Vec<SlotDefinition>>(slots).ok())
        .unwrap_or_default()
}

/// Как надевать или снимать.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EquipOptions {
    /// Без всплывающих сообщений об отказе.
    pub silent: bool,
    /// Без проверок `CanEquip`/`CanUnequip`.
    pub force: bool,
    /// С задержкой вещи (`checkDoafter`).
    pub delay: bool,
}

/// Чем кончилась попытка надеть или снять.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipResult {
    Done,
    /// Началось действие с задержкой.
    Delayed,
    Refused,
}

/// Инвентарь на `World`.
pub trait InventoryWorldExt {
    fn slot_definition(&self, wearer: Entity, slot: &str) -> Option<SlotDefinition>;
    fn slot_entity(&self, wearer: Entity, slot: &str) -> Option<Entity>;
    /// Можно ли надеть (`CanEquip`). `Err` — ключ сообщения о причине.
    fn can_equip(
        &mut self,
        user: Entity,
        target: Entity,
        item: Entity,
        slot: &str,
        assume_empty: bool,
    ) -> Result<(), String>;
    /// Надеть (`TryEquip`). `delay` — с задержкой вещи, если она есть.
    fn try_equip(
        &mut self,
        user: Entity,
        target: Entity,
        item: Entity,
        slot: &str,
        options: EquipOptions,
    ) -> EquipResult;
    fn can_unequip(&mut self, user: Entity, target: Entity, slot: &str) -> Result<Entity, String>;
    /// Снять (`TryUnequip`): вещь кладётся рядом; зависимые слоты выпадают.
    fn try_unequip(
        &mut self,
        user: Entity,
        target: Entity,
        slot: &str,
        options: EquipOptions,
    ) -> (EquipResult, Option<Entity>);
}

impl InventoryWorldExt for World {
    fn slot_definition(&self, wearer: Entity, slot: &str) -> Option<SlotDefinition> {
        self.get::<Inventory>(wearer)?
            .slots
            .iter()
            .find(|definition| definition.name == slot)
            .cloned()
    }

    fn slot_entity(&self, wearer: Entity, slot: &str) -> Option<Entity> {
        self.slot_definition(wearer, slot)?;
        self.container(wearer, slot)?.entities().first().copied()
    }

    fn can_equip(
        &mut self,
        user: Entity,
        target: Entity,
        item: Entity,
        slot: &str,
        assume_empty: bool,
    ) -> Result<(), String> {
        let cannot = || "inventory-component-can-equip-cannot".to_owned();
        let definition = self.slot_definition(target, slot).ok_or_else(cannot)?;
        if !assume_empty && self.slot_entity(target, slot).is_some() {
            return Err(cannot());
        }
        if let Err(error) = self.can_insert(item, target, slot)
            && !(assume_empty && matches!(error, astrocore_map::ContainerError::SlotFull))
        {
            return Err(cannot());
        }
        if let Some(depends) = &definition.depends_on
            && self.slot_entity(target, depends).is_none()
        {
            return Err(cannot());
        }
        let clothing = self.get::<Clothing>(item).cloned();
        let fitting_in_pocket = definition.slot_flags.has(SlotFlags::POCKET)
            && self.get::<Item>(item).is_some_and(|item| {
                size_weight(self, &item.size) <= size_weight(self, POCKETABLE_ITEM_SIZE)
            });
        let fits = clothing
            .as_ref()
            .is_some_and(|clothing| clothing.slots.has(definition.slot_flags));
        if !fits && !fitting_in_pocket {
            return Err("inventory-component-can-equip-does-not-fit".into());
        }
        if !can_access(self, user, target, item) {
            return Err("interaction-system-user-interaction-cannot-reach".into());
        }
        let whitelisted = definition
            .whitelist
            .as_ref()
            .is_none_or(|list| list.is_valid(self, item));
        let blacklisted = definition
            .blacklist
            .as_ref()
            .is_some_and(|list| list.is_valid(self, item));
        if !whitelisted || blacklisted {
            return Err("inventory-component-can-equip-does-not-fit".into());
        }
        let mut attempt = EquipAttempt {
            user,
            target,
            item,
            slot: slot.to_owned(),
            cancelled: false,
            reason: None,
        };
        for on in [user, target, item] {
            self.raise_and_broadcast(on, &mut attempt);
            if attempt.cancelled {
                return Err(attempt.reason.unwrap_or_else(cannot));
            }
        }
        Ok(())
    }

    fn try_equip(
        &mut self,
        user: Entity,
        target: Entity,
        item: Entity,
        slot: &str,
        options: EquipOptions,
    ) -> EquipResult {
        let EquipOptions {
            silent,
            force,
            delay,
        } = options;
        let Some(definition) = self.slot_definition(target, slot) else {
            if !silent {
                refuse(self, user, "inventory-component-can-equip-cannot");
            }
            return EquipResult::Refused;
        };
        if !force && let Err(reason) = self.can_equip(user, target, item, slot, false) {
            if !silent {
                refuse(self, user, &reason);
            }
            return EquipResult::Refused;
        }
        if delay
            && let Some(clothing) = self.get::<Clothing>(item).cloned()
            && clothing.equip_delay > 0.0
            && clothing.slots.intersects(definition.slot_flags)
        {
            let mut args = DoAfterArgs::new(
                user,
                clothing.equip_delay,
                Some(target),
                Some(item),
                Some(item),
            );
            args.break_on_move = !clothing.equip_while_moving;
            args.need_hand = true;
            return if self
                .try_start_do_after(
                    args,
                    ClothingEquipDoAfter {
                        slot: slot.to_owned(),
                    },
                )
                .ok()
            {
                EquipResult::Delayed
            } else {
                EquipResult::Refused
            };
        }
        // Вещь из руки: сначала из руки, потом в слот.
        if self.containing_container(item).is_some()
            && self.remove_from_container(item, None).is_err()
        {
            return EquipResult::Refused;
        }
        if self
            .force_insert_into_container(item, target, slot)
            .is_err()
        {
            if !silent {
                refuse(self, user, "inventory-component-can-unequip-cannot");
            }
            return EquipResult::Refused;
        }
        if !silent
            && let Some(sound) = self
                .get::<Clothing>(item)
                .and_then(|c| c.equip_sound.clone())
        {
            self.play_entity(&sound, target, None);
        }
        EquipResult::Done
    }

    fn can_unequip(&mut self, user: Entity, target: Entity, slot: &str) -> Result<Entity, String> {
        let cannot = || "inventory-component-can-unequip-cannot".to_owned();
        self.slot_definition(target, slot).ok_or_else(cannot)?;
        let item = self.slot_entity(target, slot).ok_or_else(cannot)?;
        if !can_access(self, user, target, item) {
            return Err("interaction-system-user-interaction-cannot-reach".into());
        }
        let mut attempt = UnequipAttempt {
            user,
            target,
            item,
            slot: slot.to_owned(),
            cancelled: false,
            reason: None,
        };
        for on in [user, target, item] {
            self.raise_and_broadcast(on, &mut attempt);
            if attempt.cancelled {
                return Err(attempt.reason.unwrap_or_else(cannot));
            }
        }
        Ok(item)
    }

    fn try_unequip(
        &mut self,
        user: Entity,
        target: Entity,
        slot: &str,
        options: EquipOptions,
    ) -> (EquipResult, Option<Entity>) {
        let EquipOptions {
            silent,
            force,
            delay,
        } = options;
        let Some(definition) = self.slot_definition(target, slot) else {
            return (EquipResult::Refused, None);
        };
        let Some(item) = self.slot_entity(target, slot) else {
            return (EquipResult::Refused, None);
        };
        if !force && let Err(reason) = self.can_unequip(user, target, slot) {
            if !silent {
                refuse(self, user, &reason);
            }
            return (EquipResult::Refused, None);
        }
        if delay
            && let Some(clothing) = self.get::<Clothing>(item).cloned()
            && clothing.unequip_delay > 0.0
            && clothing.slots.intersects(definition.slot_flags)
        {
            let mut args = DoAfterArgs::new(
                user,
                clothing.unequip_delay,
                Some(target),
                Some(item),
                Some(item),
            );
            args.break_on_move = !clothing.equip_while_moving;
            args.need_hand = true;
            let started = self
                .try_start_do_after(
                    args,
                    ClothingUnequipDoAfter {
                        slot: slot.to_owned(),
                    },
                )
                .ok();
            return (
                if started {
                    EquipResult::Delayed
                } else {
                    EquipResult::Refused
                },
                None,
            );
        }
        if self.remove_from_container(item, None).is_err() {
            return (EquipResult::Refused, None);
        }
        // Зависимые слоты (карманы без комбинезона) выпадают.
        let dependents: Vec<String> = self
            .get::<Inventory>(target)
            .map(|inventory| {
                inventory
                    .slots
                    .iter()
                    .filter(|other| other.name != slot && other.depends_on.as_deref() == Some(slot))
                    .map(|other| other.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut dropped = 0;
        for dependent in dependents {
            if self
                .try_unequip(
                    user,
                    target,
                    &dependent,
                    EquipOptions {
                        silent: true,
                        force: true,
                        delay: false,
                    },
                )
                .1
                .is_some()
            {
                dropped += 1;
            }
        }
        if !silent
            && let Some(sound) = self
                .get::<Clothing>(item)
                .and_then(|c| c.unequip_sound.clone())
        {
            self.play_entity(&sound, target, None);
        }
        if !silent && dropped > 0 {
            let text = loc(
                self,
                "inventory-component-dropped-from-unequip",
                &[("items", f64::from(dropped).into())],
            );
            self.popup_entity_for(&text, target, target, PopupType::Small);
        }
        (EquipResult::Done, Some(item))
    }
}

fn refuse(world: &mut World, user: Entity, reason: &str) {
    let text = loc(world, reason, &[]);
    world.popup_cursor(&text, user, PopupType::Small);
}

/// Дотягивается ли действующий до вещи на цели (`InventorySystem.CanAccess`).
fn can_access(world: &mut World, user: Entity, target: Entity, item: Entity) -> bool {
    if user != target && !world.in_range_unobstructed(user, target, INTERACTION_RANGE) {
        return false;
    }
    world.is_accessible(user, item) && world.in_range_unobstructed(user, item, INTERACTION_RANGE)
}

/// Клик по слоту (`OnUseSlot`).
fn use_slot(world: &mut World, user: Entity, slot: &str) {
    if world.get::<Inventory>(user).is_none() || world.get::<crate::hands::Hands>(user).is_none() {
        return;
    }
    let held = world.active_item(user);
    let worn = world.slot_entity(user, slot);
    match (held, worn) {
        (Some(held), Some(worn)) => {
            if let Some(at) = world.map_coordinates(worn) {
                world.interact_using(user, held, worn, at);
            }
        }
        (None, Some(_)) => {
            if let (EquipResult::Done, Some(item)) = world.try_unequip(
                user,
                user,
                slot,
                EquipOptions {
                    silent: false,
                    force: false,
                    delay: true,
                },
            ) {
                world.pickup_or_drop(Some(user), item);
            }
        }
        (Some(held), None) => {
            if let Err(reason) = world.can_equip(user, user, held, slot, false) {
                refuse(world, user, &reason);
                return;
            }
            let Some(hand) = world.active_hand(user) else {
                return;
            };
            if !world.can_drop_held(user, &hand, false) {
                return;
            }
            world.raise(held, &mut HandDeselected { user });
            world.try_equip(
                user,
                user,
                held,
                slot,
                EquipOptions {
                    silent: false,
                    force: true,
                    delay: true,
                },
            );
        }
        (None, None) => {}
    }
}

/// Быстрое надевание из руки (`ClothingSystem.QuickEquip`).
fn quick_equip(world: &mut World, user: Entity, item: Entity) {
    let slots: Vec<String> = world
        .get::<Inventory>(user)
        .map(|inventory| {
            inventory
                .slots
                .iter()
                .map(|slot| slot.name.clone())
                .collect()
        })
        .unwrap_or_default();
    for slot in &slots {
        if world.can_equip(user, user, item, slot, true).is_err()
            || world.slot_entity(user, slot).is_some()
        {
            continue;
        }
        world.try_equip(
            user,
            user,
            item,
            slot,
            EquipOptions {
                silent: false,
                force: false,
                delay: true,
            },
        );
        return;
    }
    for slot in &slots {
        if world.can_equip(user, user, item, slot, true).is_err() {
            continue;
        }
        let Some(worn) = world.slot_entity(user, slot) else {
            continue;
        };
        if world
            .get::<Clothing>(worn)
            .is_some_and(|clothing| !clothing.quick_equip)
        {
            continue;
        }
        match world.try_unequip(
            user,
            user,
            slot,
            EquipOptions {
                silent: true,
                force: false,
                delay: true,
            },
        ) {
            (EquipResult::Done, Some(worn)) => {
                world.try_equip(
                    user,
                    user,
                    item,
                    slot,
                    EquipOptions {
                        silent: false,
                        force: false,
                        delay: true,
                    },
                );
                world.pickup_or_drop(Some(user), worn);
                return;
            }
            (EquipResult::Delayed, _) => return,
            _ => continue,
        }
    }
}

fn setup_slots(world: &mut World, wearer: Entity) {
    let Some(template) = world
        .get::<Inventory>(wearer)
        .map(|inventory| inventory.template_id.clone())
    else {
        return;
    };
    let slots = template_slots(world, &template);
    for slot in &slots {
        world.ensure_container(wearer, &slot.name, ContainerKind::Slot);
    }
    if let Some(mut inventory) = world.get_mut::<Inventory>(wearer) {
        inventory.slots = slots;
    }
}

fn handle_use_slot(world: &mut World, event: &mut NetEvent<UseSlot>) {
    let Some(user) = event
        .session
        .and_then(|session| world.get::<Session>(session)?.attached)
        .filter(|user| world.is_alive(*user))
    else {
        return;
    };
    let slot = event.event.slot.clone();
    use_slot(world, user, &slot);
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<Inventory>();
    world.register_data_component::<Clothing>();
    world.register_net_serde::<Inventory>();
    world.register_net_serde::<Clothing>();
    world.register_net_event::<UseSlot>();
    // Слоты из шаблона заполняются на обеих сторонах: клиенту они нужны для окна.
    world.subscribe::<Inventory, ComponentInit>(|world, wearer, _| setup_slots(world, wearer));
    if !side.is_server() {
        return;
    }
    world.subscribe::<Item, EntGotInsertedIntoContainer>(|world, item, event| {
        let Some(definition) = world.slot_definition(event.owner, &event.container) else {
            return;
        };
        let wearer = event.owner;
        if let Some(mut clothing) = world.get_mut::<Clothing>(item) {
            clothing.in_slot = Some(definition.name.clone());
        }
        let mut equipped = Equipped {
            wearer,
            item,
            slot: definition.name.clone(),
            slot_flags: definition.slot_flags,
        };
        world.raise_and_broadcast(wearer, &mut equipped);
        world.raise_and_broadcast(item, &mut equipped);
    });
    world.subscribe::<Item, EntGotRemovedFromContainer>(|world, item, event| {
        let Some(definition) = world.slot_definition(event.owner, &event.container) else {
            return;
        };
        let wearer = event.owner;
        if let Some(mut clothing) = world.get_mut::<Clothing>(item) {
            clothing.in_slot = None;
        }
        let mut unequipped = Unequipped {
            wearer,
            item,
            slot: definition.name.clone(),
            slot_flags: definition.slot_flags,
        };
        world.raise_and_broadcast(wearer, &mut unequipped);
        world.raise_and_broadcast(item, &mut unequipped);
    });
    world.subscribe::<Clothing, UseInHand>(|world, item, event| {
        if event.handled
            || !world
                .get::<Clothing>(item)
                .is_some_and(|clothing| clothing.quick_equip)
        {
            return;
        }
        if world.get::<Inventory>(event.user).is_none()
            || world.get::<crate::hands::Hands>(event.user).is_none()
        {
            return;
        }
        quick_equip(world, event.user, item);
        event.handled = true;
    });
    world.subscribe::<Clothing, DoAfterDone<ClothingEquipDoAfter>>(|world, item, event| {
        if event.handled || event.cancelled {
            return;
        }
        let Some(target) = event.target else {
            return;
        };
        event.handled = world.try_equip(
            event.user,
            target,
            item,
            &event.event.slot,
            EquipOptions {
                silent: false,
                force: false,
                delay: false,
            },
        ) == EquipResult::Done;
    });
    world.subscribe::<Clothing, DoAfterDone<ClothingUnequipDoAfter>>(|world, item, event| {
        if event.handled || event.cancelled {
            return;
        }
        let Some(target) = event.target else {
            return;
        };
        let (result, _) = world.try_unequip(
            event.user,
            target,
            &event.event.slot,
            EquipOptions {
                silent: false,
                force: false,
                delay: false,
            },
        );
        event.handled = result == EquipResult::Done;
        if event.handled {
            world.try_pickup(event.user, item, None, true);
        }
    });
    world.subscribe_net_event::<UseSlot>(handle_use_slot);
}

#[cfg(test)]
mod tests;
