//! Слоты предметов: топливо в контроллере двигателя, ID-карта в консоли, батарея в фонаре.
//! Образец — `ItemSlotsComponent`, `ItemSlot`, `ItemSlotsSystem` (`Insert`, `Eject`,
//! `Interactions`, `Verbs`, `Lock`) SS14 @ 2f4a2aad.
//!
//! Слот — контейнер-ячейка владельца с именем слота. Слоты задаёт компонент `ItemSlots` в данных
//! или другая система при инициализации (`add_item_slot`, как контроллер AME свой `fuelSlot`).

use std::collections::BTreeMap;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{ComponentInit, EventWorldExt, MapInit, RegistryWorldExt};
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::{ContainerKind, ContainerWorldExt, EntityCoordinates, SpawnWorldExt};
use astrocore_math::Vec2;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::examine::loc;
use crate::hands::HandsWorldExt;
use crate::interaction::{InteractHand, InteractUsing, UseInHand};
use crate::popups::{PopupType, PopupWorldExt};
use crate::verbs::{GetVerbs, Verb, VerbKind};
use crate::whitelist::EntityWhitelist;

fn yes() -> bool {
    true
}

fn insert_sound() -> Option<SoundSpecifier> {
    Some(SoundSpecifier::path(
        "/Audio/Weapons/Guns/MagIn/revolver_magin.ogg",
    ))
}

fn eject_sound() -> Option<SoundSpecifier> {
    Some(SoundSpecifier::path(
        "/Audio/Weapons/Guns/MagOut/revolver_magout.ogg",
    ))
}

/// Слот (`ItemSlot`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemSlot {
    #[serde(default)]
    pub whitelist: Option<EntityWhitelist>,
    #[serde(default)]
    pub blacklist: Option<EntityWhitelist>,
    #[serde(default = "insert_sound")]
    pub insert_sound: Option<SoundSpecifier>,
    #[serde(default = "eject_sound")]
    pub eject_sound: Option<SoundSpecifier>,
    /// Имя для глаголов; пусто — имя предмета.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub starting_item: Option<String>,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub disable_eject: bool,
    #[serde(default = "yes")]
    pub insert_on_interact: bool,
    #[serde(default)]
    pub eject_on_interact: bool,
    #[serde(default)]
    pub eject_on_use: bool,
    #[serde(default)]
    pub insert_verb_text: Option<String>,
    #[serde(default)]
    pub eject_verb_text: Option<String>,
    #[serde(default = "yes")]
    pub eject_on_deconstruct: bool,
    #[serde(default)]
    pub eject_on_break: bool,
    #[serde(default)]
    pub whitelist_fail_popup: Option<String>,
    #[serde(default)]
    pub locked_fail_popup: Option<String>,
    #[serde(default)]
    pub insert_success_popup: Option<String>,
    #[serde(default = "yes")]
    pub swap: bool,
    #[serde(default)]
    pub priority: i32,
}

impl Default for ItemSlot {
    /// Значения по умолчанию из данных (пустая запись).
    fn default() -> Self {
        astrocore_data::from_node(&astrocore_data::DataNode::map(
            astrocore_data::DataMap::new(),
        ))
        .expect("пустой слот разбирается")
    }
}

/// Слоты сущности (`ItemSlotsComponent`): имя слота (оно же имя контейнера) → слот.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ItemSlots {
    pub slots: BTreeMap<String, ItemSlot>,
    pub allow_smart_equip: bool,
}

/// Предмет в слоте.
pub fn slot_item(world: &World, owner: Entity, slot: &str) -> Option<Entity> {
    world
        .container(owner, slot)
        .and_then(|container| container.entities().first().copied())
}

fn slot(world: &World, owner: Entity, id: &str) -> Option<ItemSlot> {
    world.get::<ItemSlots>(owner)?.slots.get(id).cloned()
}

/// Добавляет слот (`AddItemSlot`): контейнер-ячейка с именем слота.
pub fn add_item_slot(world: &mut World, owner: Entity, id: &str, slot: ItemSlot) {
    world.ensure_container(owner, id, ContainerKind::Slot);
    let mut slots = world.get::<ItemSlots>(owner).cloned().unwrap_or_default();
    slots.slots.insert(id.to_owned(), slot);
    world.entity_mut(owner).insert(slots);
}

/// Запирает или отпирает слот (`SetLock`).
pub fn set_lock(world: &mut World, owner: Entity, id: &str, locked: bool) {
    if let Some(mut slots) = world.get_mut::<ItemSlots>(owner)
        && let Some(slot) = slots.slots.get_mut(id)
    {
        slot.locked = locked;
    }
}

/// Подходит ли предмет по белому и чёрному спискам (`CanInsertWhitelist`).
fn whitelist_allows(world: &World, slot: &ItemSlot, item: Entity) -> bool {
    slot.whitelist
        .as_ref()
        .is_none_or(|w| w.is_valid(world, item))
        && slot
            .blacklist
            .as_ref()
            .is_none_or(|b| !b.is_valid(world, item))
}

/// Можно ли вставить (`CanInsert`): слот не заперт, предмет подходит, место есть или его
/// можно поменять.
pub fn can_insert(world: &World, owner: Entity, id: &str, item: Entity, swap: bool) -> bool {
    let Some(slot) = slot(world, owner, id) else {
        return false;
    };
    if slot.locked || item == owner || !whitelist_allows(world, &slot, item) {
        return false;
    }
    slot_item(world, owner, id).is_none() || swap
}

/// Вставляет предмет (`Insert`): без проверок.
pub fn insert(world: &mut World, owner: Entity, id: &str, item: Entity) -> bool {
    if world.containing_container(item).is_some()
        && world.remove_from_container(item, None).is_err()
    {
        return false;
    }
    if world.insert_into_container(item, owner, id).is_err() {
        return false;
    }
    if let Some(sound) = slot(world, owner, id).and_then(|s| s.insert_sound) {
        world.play_entity(&sound, owner, None);
    }
    true
}

/// Можно ли вынуть (`CanEject`).
pub fn can_eject(world: &World, owner: Entity, id: &str) -> bool {
    slot(world, owner, id).is_some_and(|s| !s.locked && !s.disable_eject)
        && slot_item(world, owner, id).is_some()
}

/// Вынимает предмет в руки действующего или рядом (`TryEjectToHands`).
pub fn try_eject_to_hands(
    world: &mut World,
    owner: Entity,
    id: &str,
    user: Option<Entity>,
) -> bool {
    if !can_eject(world, owner, id) {
        return false;
    }
    let Some(item) = slot_item(world, owner, id) else {
        return false;
    };
    if world.remove_from_container(item, None).is_err() {
        return false;
    }
    if let Some(sound) = slot(world, owner, id).and_then(|s| s.eject_sound) {
        world.play_entity(&sound, owner, None);
    }
    world.pickup_or_drop(user, item);
    true
}

/// Вставляет предмет из активной руки в слот (`TryInsertFromHand`).
pub fn try_insert_from_hand(world: &mut World, owner: Entity, id: &str, user: Entity) -> bool {
    let Some(held) = world.active_item(user) else {
        return false;
    };
    if !can_insert(world, owner, id, held, false) {
        return false;
    }
    let Some(hand) = world.hand_holding(user, held) else {
        return false;
    };
    if !world.can_drop_held(user, &hand, true) {
        return false;
    }
    insert(world, owner, id, held)
}

fn on_interact_using(world: &mut World, owner: Entity, event: &mut InteractUsing) {
    if event.handled {
        return;
    }
    let Some(slots) = world.get::<ItemSlots>(owner).cloned() else {
        return;
    };
    let mut fitting: Vec<(String, ItemSlot)> = Vec::new();
    let mut whitelist_fail = None;
    let mut locked_fail = None;
    for (id, slot) in &slots.slots {
        if !slot.insert_on_interact {
            continue;
        }
        if can_insert(world, owner, id, event.used, slot.swap) {
            fitting.push((id.clone(), slot.clone()));
        } else {
            let allowed = whitelist_allows(world, slot, event.used);
            if locked_fail.is_none() && allowed && slot.locked {
                locked_fail.clone_from(&slot.locked_fail_popup);
            }
            if whitelist_fail.is_none() && !allowed {
                whitelist_fail.clone_from(&slot.whitelist_fail_popup);
            }
        }
    }
    if fitting.is_empty() {
        if let Some(message) = locked_fail.or(whitelist_fail) {
            let text = loc(world, &message, &[]);
            world.popup_entity_for(&text, owner, event.user, PopupType::Small);
        }
        return;
    }
    let Some(hand) = world.hand_holding(event.user, event.used) else {
        return;
    };
    if !world.can_drop_held(event.user, &hand, true) {
        return;
    }
    // Сначала пустые слоты, затем по приоритету (`SortEmpty`).
    fitting.sort_by_key(|(id, slot)| {
        (
            slot_item(world, owner, id).is_some(),
            std::cmp::Reverse(slot.priority),
        )
    });
    let (id, slot) = fitting.remove(0);
    let previous = slot_item(world, owner, &id);
    if let Some(previous) = previous
        && world.remove_from_container(previous, None).is_ok()
    {
        world.try_pickup_any_hand(event.user, previous, true);
    }
    if insert(world, owner, &id, event.used) {
        if let Some(message) = &slot.insert_success_popup {
            let text = loc(world, message, &[]);
            world.popup_entity_for(&text, owner, event.user, PopupType::Small);
        }
        event.handled = true;
    }
}

fn first_with(world: &World, owner: Entity, test: impl Fn(&ItemSlot) -> bool) -> Option<String> {
    let slots = world.get::<ItemSlots>(owner)?;
    slots
        .slots
        .iter()
        .find(|(id, slot)| test(slot) && can_eject(world, owner, id))
        .map(|(id, _)| id.clone())
}

fn slot_label(world: &World, owner: Entity, id: &str, slot: &ItemSlot) -> String {
    if !slot.name.is_empty() {
        return loc(world, &slot.name, &[]);
    }
    slot_item(world, owner, id).map_or_else(String::new, |item| world.entity_name(item))
}

/// Глаголы «вынуть» и «вставить» (`AddAlternativeVerbs`, `AddInteractionVerbs`).
fn on_verbs(world: &mut World, owner: Entity, event: &mut GetVerbs) {
    if !event.can_access || !event.can_interact {
        return;
    }
    let Some(slots) = world.get::<ItemSlots>(owner).cloned() else {
        return;
    };
    let user = event.user;
    match event.kind {
        VerbKind::Alternative => {
            for (id, slot) in &slots.slots {
                if slot.eject_on_interact || !can_eject(world, owner, id) {
                    continue;
                }
                let label = slot_label(world, owner, id, slot);
                let text = slot
                    .eject_verb_text
                    .as_deref()
                    .map_or(label, |key| loc(world, key, &[]));
                let id = id.clone();
                let mut verb = Verb::new(VerbKind::Alternative, text, move |world| {
                    try_eject_to_hands(world, owner, &id, Some(user));
                });
                verb.category = Some("verb-categories-eject".into());
                verb.priority = slot.priority;
                event.verbs.push(verb);
            }
        }
        VerbKind::Interaction => {
            let Some(using) = event.using else {
                return;
            };
            for (id, slot) in &slots.slots {
                if !can_insert(world, owner, id, using, false) {
                    continue;
                }
                let text = slot
                    .insert_verb_text
                    .as_deref()
                    .map_or_else(|| world.entity_name(using), |key| loc(world, key, &[]));
                let id = id.clone();
                let mut verb = Verb::new(VerbKind::Interaction, text, move |world| {
                    try_insert_from_hand(world, owner, &id, user);
                });
                verb.category = Some("verb-categories-insert".into());
                verb.priority = slot.priority;
                event.verbs.push(verb);
            }
        }
        _ => {}
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<ItemSlots>();
    world.subscribe::<ItemSlots, ComponentInit>(|world, owner, _| {
        let ids: Vec<String> = world
            .get::<ItemSlots>(owner)
            .map(|s| s.slots.keys().cloned().collect())
            .unwrap_or_default();
        for id in ids {
            world.ensure_container(owner, &id, ContainerKind::Slot);
        }
    });
    if !side.is_server() {
        return;
    }
    // Стартовые предметы (`OnMapInit`).
    world.subscribe::<ItemSlots, MapInit>(|world, owner, _| {
        let starting: Vec<(String, String)> = world
            .get::<ItemSlots>(owner)
            .map(|s| {
                s.slots
                    .iter()
                    .filter_map(|(id, slot)| Some((id.clone(), slot.starting_item.clone()?)))
                    .collect()
            })
            .unwrap_or_default();
        for (id, prototype) in starting {
            if slot_item(world, owner, &id).is_some() {
                continue;
            }
            if let Ok(item) =
                world.spawn_attached(&prototype, EntityCoordinates::new(owner, Vec2::ZERO))
            {
                let _ = world.force_insert_into_container(item, owner, &id);
            }
        }
    });
    world.subscribe::<ItemSlots, InteractUsing>(on_interact_using);
    world.subscribe::<ItemSlots, InteractHand>(|world, owner, event| {
        if event.handled {
            return;
        }
        if let Some(id) = first_with(world, owner, |s| s.eject_on_interact) {
            event.handled = try_eject_to_hands(world, owner, &id, Some(event.user));
        }
    });
    world.subscribe::<ItemSlots, UseInHand>(|world, owner, event| {
        if event.handled {
            return;
        }
        if let Some(id) = first_with(world, owner, |s| s.eject_on_use) {
            event.handled = try_eject_to_hands(world, owner, &id, Some(event.user));
        }
    });
    world.subscribe::<ItemSlots, GetVerbs>(on_verbs);
}

#[cfg(test)]
mod tests;
