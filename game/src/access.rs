//! Доступ: уровни доступа на ID-картах, считыватели на дверях и машинах, КПК с картой. Образец —
//! `AccessComponent`, `AccessReaderComponent`, `AccessReaderSystem` (`IsAllowed`,
//! `AreAccessTagsAllowed`, `FindPotentialAccessItems`, `FindAccessTags`), `SharedAccessSystem`,
//! `IdCardComponent`, `PdaComponent` и `SharedPdaSystem` SS14 @ 2f4a2aad.
//!
//! Источники доступа действующего: он сам, предметы в руках, предмет в слоте `id`; у КПК —
//! вставленная карта. Считыватель пускает, если хотя бы один набор его уровней целиком есть у
//! действующего, а запрещённых уровней нет. Ключи записей станции — с записями (фаза 6).

use std::collections::{BTreeSet, HashSet};

use astrocore_ecs::{ComponentInit, EventWorldExt, MapInit, Paused, RegistryWorldExt};
use astrocore_map::ContainerWorldExt;
use astrocore_net::NetRegistryWorldExt;
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::hands::HandsWorldExt;
use crate::inventory::InventoryWorldExt;
use crate::item_slots::{ItemSlot, add_item_slot, slot_item};

fn yes() -> bool {
    true
}

/// Уровни доступа предмета или существа (`AccessComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Access {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub tags: BTreeSet<String>,
    /// Группы уровней (`accessGroup`): на старте разворачиваются в уровни.
    #[serde(default)]
    pub groups: BTreeSet<String>,
}

/// Считыватель (`AccessReaderComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessReader {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub deny_tags: BTreeSet<String>,
    /// Наборы уровней: хватает любого целиком (`AccessLists`).
    #[serde(default, rename = "access")]
    pub access_lists: Vec<BTreeSet<String>>,
    /// Контейнер, считыватель внутри которого решает за сущность (электроника двери).
    #[serde(default)]
    pub container_access_provider: Option<String>,
    #[serde(default = "yes")]
    pub break_on_access_breaker: bool,
    /// Журнал доступа отключён (`LoggingDisabled`); сам журнал — с записями станции.
    #[serde(default)]
    pub logging_disabled: bool,
}

/// ID-карта (`IdCardComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IdCard {
    pub full_name: Option<String>,
    pub job_title: Option<String>,
    pub job_icon: Option<String>,
    pub job_prototype: Option<String>,
    pub job_departments: Vec<String>,
    pub bypass_logging: bool,
}

/// Слот карты в КПК (`PdaComponent.PdaIdSlotId`).
pub const PDA_ID_SLOT: &str = "PDA-id";
/// Слот ручки.
pub const PDA_PEN_SLOT: &str = "PDA-pen";

/// КПК (`PdaComponent`): карта, ручка, стартовая карта.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Pda {
    pub id_slot: ItemSlot,
    pub pen_slot: ItemSlot,
    /// Стартовая карта.
    #[serde(rename = "id")]
    pub id_card: Option<String>,
}

/// Уровни группы доступа (`AccessGroupPrototype.Tags`).
fn group_tags(prototypes: &Prototypes, group: &str) -> Vec<String> {
    prototypes
        .resolved_node("accessGroup", group)
        .and_then(|node| node.get("tags").cloned())
        .and_then(|tags| astrocore_data::from_node::<Vec<String>>(&tags).ok())
        .unwrap_or_default()
}

/// Где искать доступ (`FindPotentialAccessItems`): действующий, руки, слот `id` и вставленная
/// в КПК карта.
pub fn potential_access_items(world: &World, user: Entity) -> HashSet<Entity> {
    let mut items = HashSet::new();
    let mut stack = vec![user];
    while let Some(entity) = stack.pop() {
        if !items.insert(entity) {
            continue;
        }
        stack.extend(world.held_items(entity));
        if let Some(id) = world.slot_entity(entity, "id") {
            stack.push(id);
        }
        if world.get::<Pda>(entity).is_some()
            && let Some(card) = slot_item(world, entity, PDA_ID_SLOT)
        {
            stack.push(card);
        }
    }
    items
}

/// Карта из сущности или вставленная в КПК (`TryGetIdCard`).
pub fn get_id_card(world: &World, entity: Entity) -> Option<Entity> {
    if world.get::<IdCard>(entity).is_some() {
        return Some(entity);
    }
    if world.get::<Pda>(entity).is_some() {
        return slot_item(world, entity, PDA_ID_SLOT)
            .filter(|card| world.get::<IdCard>(*card).is_some());
    }
    None
}

/// ID-карта действующего (`TryFindIdCard`): в активной руке, он сам, слот `id`.
pub fn find_id_card(world: &World, user: Entity) -> Option<Entity> {
    world
        .active_item(user)
        .and_then(|item| get_id_card(world, item))
        .or_else(|| get_id_card(world, user))
        .or_else(|| {
            world
                .slot_entity(user, "id")
                .and_then(|id| get_id_card(world, id))
        })
}

/// «Имя (Должность)» по ID-карте (`GetIdentityShortInfo`, `ExtractFullTitle`).
pub fn identity_short_info(world: &World, user: Entity) -> Option<String> {
    let card = world.get::<IdCard>(find_id_card(world, user)?)?;
    let job = card.job_title.clone().unwrap_or_default();
    let job: String = job
        .split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ");
    Some(
        format!("{} ({job})", card.full_name.clone().unwrap_or_default())
            .trim()
            .to_owned(),
    )
}

/// Уровни доступа действующего (`FindAccessTags`).
pub fn access_tags(world: &World, user: Entity) -> BTreeSet<String> {
    potential_access_items(world, user)
        .into_iter()
        .filter_map(|item| world.get::<Access>(item))
        .filter(|access| access.enabled)
        .flat_map(|access| access.tags.iter().cloned())
        .collect()
}

/// Пускают ли такие уровни (`AreAccessTagsAllowed`).
pub fn tags_allowed(tags: &BTreeSet<String>, reader: &AccessReader) -> bool {
    if !reader.enabled {
        return true;
    }
    if reader.deny_tags.iter().any(|tag| tags.contains(tag)) {
        return false;
    }
    reader.access_lists.is_empty() || reader.access_lists.iter().any(|set| set.is_subset(tags))
}

fn reader_allows(
    world: &World,
    tags: &BTreeSet<String>,
    target: Entity,
    reader: &AccessReader,
) -> bool {
    if !reader.enabled {
        return true;
    }
    let Some(provider) = &reader.container_access_provider else {
        return tags_allowed(tags, reader);
    };
    if world.get::<Paused>(target).is_some() {
        return true;
    }
    world
        .container(target, provider)
        .map(|container| container.entities().to_vec())
        .unwrap_or_default()
        .into_iter()
        .any(|inner| {
            world
                .get::<AccessReader>(inner)
                .is_some_and(|inner_reader| reader_allows(world, tags, inner, inner_reader))
        })
}

/// Пускает ли сущность действующего (`IsAllowed`). Без считывателя — пускает.
pub fn is_allowed(world: &World, user: Entity, target: Entity) -> bool {
    let Some(reader) = world.get::<AccessReader>(target) else {
        return true;
    };
    if !reader.enabled {
        return true;
    }
    let tags = access_tags(world, user);
    reader_allows(world, &tags, target, reader)
}

/// Все уровни доступа из прототипов `accessLevel`.
pub fn access_levels(prototypes: &Prototypes) -> Vec<String> {
    let mut ids: Vec<String> = prototypes.ids("accessLevel").map(str::to_owned).collect();
    ids.sort();
    ids
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Access>();
    world.register_data_component::<AccessReader>();
    world.register_data_component::<IdCard>();
    world.register_data_component::<Pda>();
    world.register_net_serde::<IdCard>();
    // Слоты КПК (`OnComponentInit`: `AddItemSlot`).
    world.subscribe::<Pda, ComponentInit>(|world, pda, _| {
        let Some(component) = world.get::<Pda>(pda).cloned() else {
            return;
        };
        let mut id_slot = component.id_slot;
        if id_slot.starting_item.is_none() {
            id_slot.starting_item = component.id_card;
        }
        add_item_slot(world, pda, PDA_ID_SLOT, id_slot);
        add_item_slot(world, pda, PDA_PEN_SLOT, component.pen_slot);
    });
    if !side.is_server() {
        return;
    }
    // Группы разворачиваются в уровни (`OnAccessInit`).
    world.subscribe::<Access, MapInit>(|world, entity, _| {
        let Some(groups) = world.get::<Access>(entity).map(|a| a.groups.clone()) else {
            return;
        };
        let tags: Vec<String> = world
            .get_resource::<Prototypes>()
            .map(|p| groups.iter().flat_map(|g| group_tags(p, g)).collect())
            .unwrap_or_default();
        if let Some(mut access) = world.get_mut::<Access>(entity) {
            access.tags.extend(tags);
        }
    });
}

#[cfg(test)]
pub(crate) mod tests;
