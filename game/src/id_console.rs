//! Консоль ID-карт: карта с правами в одном слоте, изменяемая — в другом; уровни доступа
//! меняются в пределах прав первой карты. Образец — `IdCardConsoleComponent`,
//! `IdCardConsoleSystem` (`UpdateUserInterface`, `TryWriteToTargetId`,
//! `PrivilegedIdIsAuthorized`), `IdCardConsoleBoundUserInterfaceState` SS14 @ 2f4a2aad.
//!
//! Имя и должность пока не редактируются (поле ввода окна — вместе с профилями, 6.2):
//! консоль переписывает их теми же, что прислал клиент.

use std::collections::BTreeSet;

use astrocore_ecs::{ComponentInit, EventWorldExt, RegistryWorldExt};
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::{EntInsertedIntoContainer, EntRemovedFromContainer};
use astrocore_prototypes::Prototypes;
use astrocore_ui::{BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::access::{Access, AccessReader, IdCard, access_levels, access_tags, tags_allowed};
use crate::item_slots::{ItemSlot, add_item_slot, slot_item};

/// Слот карты с правами.
pub const PRIVILEGED_SLOT: &str = "IdCardConsole-privilegedId";
/// Слот изменяемой карты.
pub const TARGET_SLOT: &str = "IdCardConsole-targetId";
/// Ключ окна.
pub const ID_CONSOLE_UI_KEY: &str = "enum.IdCardConsoleUiKey.Key";
/// Наибольшая длина имени и должности (`MaxNameLength`, `MaxIdJobLength`).
pub const MAX_NAME_LENGTH: usize = 32;
pub const MAX_JOB_LENGTH: usize = 30;

/// Консоль (`IdCardConsoleComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IdCardConsole {
    pub privileged_id_slot: ItemSlot,
    pub target_id_slot: ItemSlot,
    /// Уровни, которые консоль может менять; пусто — все уровни из прототипов.
    pub access_levels: Vec<String>,
}

/// Состояние окна (`IdCardConsoleBoundUserInterfaceState`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdConsoleState {
    pub privileged_present: bool,
    pub privileged_authorized: bool,
    pub target_present: bool,
    pub target_full_name: Option<String>,
    pub target_job_title: Option<String>,
    pub target_access: Vec<String>,
    /// Уровни, которые даёт карта с правами (`AllowedModifyAccessList`).
    pub allowed_access: Vec<String>,
    /// Уровни консоли.
    pub console_access: Vec<String>,
    pub privileged_name: String,
    pub target_name: String,
}

/// Записать на изменяемую карту (`WriteToTargetIdMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteToTargetId {
    pub full_name: String,
    pub job_title: String,
    pub access: Vec<String>,
}

fn console_levels(world: &World, console: Entity) -> Vec<String> {
    let own = world
        .get::<IdCardConsole>(console)
        .map(|c| c.access_levels.clone())
        .unwrap_or_default();
    if !own.is_empty() {
        return own;
    }
    world
        .get_resource::<Prototypes>()
        .map(access_levels)
        .unwrap_or_default()
}

/// Карта с правами есть и проходит считыватель консоли (`PrivilegedIdIsAuthorized`).
fn privileged_authorized(world: &World, console: Entity) -> Option<Entity> {
    let card = slot_item(world, console, PRIVILEGED_SLOT)?;
    let Some(reader) = world.get::<AccessReader>(console) else {
        return Some(card);
    };
    tags_allowed(&access_tags(world, card), reader).then_some(card)
}

/// Состояние окна (`UpdateUserInterface`).
pub fn console_state(world: &World, console: Entity) -> IdConsoleState {
    let privileged = slot_item(world, console, PRIVILEGED_SLOT);
    let target = slot_item(world, console, TARGET_SLOT);
    let card = target.and_then(|t| world.get::<IdCard>(t));
    IdConsoleState {
        privileged_present: privileged.is_some(),
        privileged_authorized: privileged_authorized(world, console).is_some(),
        target_present: target.is_some(),
        target_full_name: card.and_then(|c| c.full_name.clone()),
        target_job_title: card.and_then(|c| c.job_title.clone()),
        target_access: target
            .and_then(|t| world.get::<Access>(t))
            .map(|a| a.tags.iter().cloned().collect())
            .unwrap_or_default(),
        allowed_access: privileged
            .map(|p| access_tags(world, p).into_iter().collect())
            .unwrap_or_default(),
        console_access: console_levels(world, console),
        privileged_name: privileged.map(|p| world.entity_name(p)).unwrap_or_default(),
        target_name: target.map(|t| world.entity_name(t)).unwrap_or_default(),
    }
}

fn update_ui(world: &mut World, console: Entity) {
    if world.get::<IdCardConsole>(console).is_none() {
        return;
    }
    let state = console_state(world, console);
    world.set_ui_state(console, ID_CONSOLE_UI_KEY, &state);
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Записывает изменения на карту (`TryWriteToTargetId`). `false` — отказано.
pub fn write_to_target(world: &mut World, console: Entity, message: &WriteToTargetId) -> bool {
    let Some(target) = slot_item(world, console, TARGET_SLOT) else {
        return false;
    };
    let Some(privileged) = privileged_authorized(world, console) else {
        return false;
    };
    if let Some(mut card) = world.get_mut::<IdCard>(target) {
        card.full_name = Some(truncate(&message.full_name, MAX_NAME_LENGTH));
        card.job_title = Some(truncate(&message.job_title, MAX_JOB_LENGTH));
    }
    let levels: BTreeSet<String> = console_levels(world, console).into_iter().collect();
    let wanted: BTreeSet<String> = message.access.iter().cloned().collect();
    if !wanted.is_subset(&levels) {
        return false;
    }
    let old: BTreeSet<String> = world
        .get::<Access>(target)
        .map(|a| a.tags.clone())
        .unwrap_or_default();
    // Менять можно только видимые консоли уровни, которые есть у карты с правами.
    let changed: BTreeSet<&String> = wanted.symmetric_difference(&old).collect();
    let privileged_tags = access_tags(world, privileged);
    if !changed
        .iter()
        .filter(|tag| levels.contains(**tag))
        .all(|tag| privileged_tags.contains(*tag))
    {
        return false;
    }
    let hidden: BTreeSet<String> = old.difference(&levels).cloned().collect();
    let new_tags: BTreeSet<String> = wanted.union(&hidden).cloned().collect();
    match world.get_mut::<Access>(target) {
        Some(mut access) => access.tags = new_tags,
        None => {
            world.entity_mut(target).insert(Access {
                enabled: true,
                tags: new_tags,
                groups: BTreeSet::new(),
            });
        }
    }
    true
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<IdCardConsole>();
    world.register_ui_message::<WriteToTargetId>();
    world.subscribe::<IdCardConsole, ComponentInit>(|world, console, _| {
        let Some(component) = world.get::<IdCardConsole>(console).cloned() else {
            return;
        };
        add_item_slot(
            world,
            console,
            PRIVILEGED_SLOT,
            component.privileged_id_slot,
        );
        add_item_slot(world, console, TARGET_SLOT, component.target_id_slot);
    });
    if !side.is_server() {
        return;
    }
    world.subscribe::<IdCardConsole, EntInsertedIntoContainer>(|world, console, _| {
        update_ui(world, console);
    });
    world.subscribe::<IdCardConsole, EntRemovedFromContainer>(|world, console, _| {
        update_ui(world, console);
    });
    world.subscribe::<IdCardConsole, BoundUiMessage<WriteToTargetId>>(|world, console, event| {
        let message = event.message.clone();
        write_to_target(world, console, &message);
        update_ui(world, console);
    });
}

#[cfg(test)]
mod tests;
