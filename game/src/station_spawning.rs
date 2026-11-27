//! Появление игрока на станции: точка появления по должности и стадии раунда, тело вида из
//! профиля, снаряжение роли (группы снаряжения со значениями по умолчанию), стартовое
//! снаряжение должности, ID-карта с именем, должностью и доступом. Образец —
//! `SpawnPointComponent`, `SpawnPointSystem`, `StationSpawningSystem` (`SpawnPlayerMob`,
//! `EquipStartingGear`, `EquipRoleLoadout`, `EquipRoleName`, `SetPdaAndIdCardData`),
//! `RoleLoadout` (`SetDefault`, `IsValid`), эффекты снаряжения `GroupLoadoutEffect`,
//! `SpeciesLoadoutEffect`, `JobRequirementLoadoutEffect`, `SharedIdCardSystem`
//! (`TryChangeFullName`, `TryChangeJobTitle`, `UpdateEntityName`) и
//! `SharedAccessSystem.SetAccessToJob` SS14 @ 2f4a2aad.
//!
//! Нет пока: особенностей должностей (`special`: импланты, компоненты), значков должностей
//! на карте, очков снаряжения, требований снаряжения по наигранному времени (времени игры
//! ещё нет — такие снаряжения доступны, как в SS14 без счётчика), описания внешности.

use std::collections::BTreeMap;

use astrocore_data::DataNode;
use astrocore_ecs::{MetaData, RegistryWorldExt};
use astrocore_loc::LocArg;
use astrocore_map::{EntityCoordinates, SpawnWorldExt, TransformWorldExt};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::access::{Access, IdCard, PDA_ID_SLOT};
use crate::hands::HandsWorldExt;
use crate::inventory::{EquipOptions, Inventory, InventoryWorldExt};
use crate::profiles::{HumanoidCharacterProfile, RoleLoadout};
use crate::stations::{StationJobs, owning_station};
use crate::storage::StorageWorldExt;

// Точки появления.

/// Вид точки появления (`SpawnPointType`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpawnPointType {
    #[default]
    Unset,
    LateJoin,
    Job,
    Observer,
}

/// Точка появления (`SpawnPointComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpawnPoint {
    #[serde(rename = "job_id")]
    pub job: Option<String>,
    #[serde(rename = "spawn_type")]
    pub spawn_type: SpawnPointType,
}

/// Места появления (`SpawnPointSystem.OnPlayerSpawning`): в раунде — точки позднего входа,
/// до раунда — точки должности (или без должности). Станция ограничивает выбор её гридами.
pub fn spawn_positions(
    world: &mut World,
    station: Option<Entity>,
    job: Option<&str>,
    in_round: bool,
) -> Vec<EntityCoordinates> {
    let points: Vec<(Entity, SpawnPoint)> = world
        .query::<(Entity, &SpawnPoint)>()
        .iter(world)
        .map(|(e, p)| (e, p.clone()))
        .collect();
    let mut positions = Vec::new();
    for (entity, point) in &points {
        if station.is_some() && owning_station(world, *entity) != station {
            continue;
        }
        let fits = if in_round {
            point.spawn_type == SpawnPointType::LateJoin
        } else {
            point.spawn_type == SpawnPointType::Job
                && (job.is_none() || point.job.is_none() || point.job.as_deref() == job)
        };
        if fits && let Some(at) = world.coordinates(*entity) {
            positions.push(at);
        }
    }
    if positions.is_empty() {
        // Запасной вариант SS14: любая точка появления.
        let mut sorted = points;
        sorted.sort_by_key(|(e, _)| *e);
        if let Some(at) = sorted.first().and_then(|(e, _)| world.coordinates(*e)) {
            eprintln!(
                "нет подходящей точки появления (станция {station:?}, должность {job:?}): взята любая"
            );
            positions.push(at);
        }
    }
    positions
}

// Стартовое снаряжение.

/// Снаряжение (`IEquipmentLoadout`): вещи по слотам, в руки и в хранилища слотов.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gear {
    pub equipment: BTreeMap<String, String>,
    pub inhand: Vec<String>,
    pub storage: BTreeMap<String, Vec<String>>,
}

fn strings(node: Option<&DataNode>) -> Vec<String> {
    node.and_then(DataNode::as_seq)
        .map(|s| {
            s.iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Снаряжение из узла `startingGear` или `loadout`.
pub fn gear_from_node(node: &DataNode) -> Gear {
    Gear {
        equipment: node
            .get("equipment")
            .and_then(DataNode::as_map)
            .map(|m| {
                m.iter()
                    .filter_map(|(slot, item)| Some((slot.to_owned(), item.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default(),
        inhand: strings(node.get("inhand")),
        storage: node
            .get("storage")
            .and_then(DataNode::as_map)
            .map(|m| {
                m.iter()
                    .map(|(slot, items)| (slot.to_owned(), strings(Some(items))))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Стартовое снаряжение по прототипу `startingGear`.
pub fn starting_gear(prototypes: &Prototypes, id: &str) -> Option<Gear> {
    prototypes
        .resolved_node("startingGear", id)
        .map(gear_from_node)
}

/// Надеть снаряжение (`EquipStartingGear`): по слотам инвентаря, в свободные руки, в
/// хранилища надетых вещей.
pub fn equip_gear(world: &mut World, entity: Entity, gear: &Gear) {
    let Some(at) = world.coordinates(entity) else {
        return;
    };
    let slots: Vec<String> = world
        .get::<Inventory>(entity)
        .map(|i| i.slots.iter().map(|s| s.name.clone()).collect())
        .unwrap_or_default();
    let options = EquipOptions {
        silent: true,
        force: true,
        delay: false,
    };
    for slot in slots {
        let Some(prototype) = gear.equipment.get(&slot).filter(|p| !p.is_empty()) else {
            continue;
        };
        match world.spawn_attached(prototype, at) {
            Ok(item) => {
                world.try_equip(entity, entity, item, &slot, options);
            }
            Err(error) => eprintln!("снаряжение {prototype}: {error}"),
        }
    }
    for prototype in &gear.inhand {
        if let Ok(item) = world.spawn_attached(prototype, at)
            && let Some(hand) = world.empty_hand(entity)
        {
            world.try_pickup(entity, item, Some(&hand), false);
        }
    }
    for (slot, items) in &gear.storage {
        let Some(container) = world.slot_entity(entity, slot) else {
            continue;
        };
        for prototype in items {
            if let Ok(item) = world.spawn_attached(prototype, at) {
                world.storage_insert(container, item, None, false);
            }
        }
    }
}

// Снаряжение роли.

/// Снаряжение роли (`RoleLoadoutPrototype`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoleLoadoutProto {
    pub id: String,
    pub groups: Vec<String>,
    pub can_customize_name: bool,
    pub name_dataset: Option<String>,
}

/// Группа снаряжения (`LoadoutGroupPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct LoadoutGroup {
    pub loadouts: Vec<String>,
    pub min_limit: i32,
    pub max_limit: i32,
    pub default_selected: i32,
}

fn int(node: &DataNode, key: &str, default: i32) -> i32 {
    node.get(key)
        .and_then(DataNode::as_str)
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(default)
}

/// Прототип снаряжения роли; должности — `Job` + идентификатор (`GetJobPrototype`).
pub fn role_loadout(prototypes: &Prototypes, id: &str) -> Option<RoleLoadoutProto> {
    let node = prototypes.resolved_node("roleLoadout", id)?;
    Some(RoleLoadoutProto {
        id: id.to_owned(),
        groups: strings(node.get("groups")),
        can_customize_name: node
            .get("canCustomizeName")
            .and_then(DataNode::as_str)
            .is_some_and(|t| t == "true"),
        name_dataset: node
            .get("nameDataset")
            .and_then(DataNode::as_str)
            .map(str::to_owned),
    })
}

/// Группа снаряжения по прототипу `loadoutGroup`.
pub fn loadout_group(prototypes: &Prototypes, id: &str) -> Option<LoadoutGroup> {
    let node = prototypes.resolved_node("loadoutGroup", id)?;
    Some(LoadoutGroup {
        loadouts: strings(node.get("loadouts")),
        min_limit: int(node, "minLimit", 1),
        max_limit: int(node, "maxLimit", 1),
        default_selected: int(node, "defaultSelected", 0),
    })
}

/// Проходят ли эффекты (`LoadoutEffect.Validate`): вид — по списку, группа — по своим
/// эффектам, требование времени игры — без счётчика времени проходит.
fn effects_valid(
    prototypes: &Prototypes,
    effects: &[DataNode],
    profile: &HumanoidCharacterProfile,
) -> bool {
    effects.iter().all(|effect| {
        let kind = effect.type_name().unwrap_or_default();
        match kind {
            "SpeciesLoadoutEffect" => strings(effect.get("species")).contains(&profile.species),
            "GroupLoadoutEffect" => effect
                .get("proto")
                .and_then(DataNode::as_str)
                .and_then(|id| prototypes.resolved_node("loadoutEffectGroup", id))
                .and_then(|group| group.get("effects").and_then(DataNode::as_seq))
                .is_none_or(|inner| effects_valid(prototypes, inner, profile)),
            _ => true,
        }
    })
}

/// Подходит ли снаряжение персонажу (`RoleLoadout.IsValid`).
pub fn loadout_valid(
    prototypes: &Prototypes,
    loadout: &str,
    profile: &HumanoidCharacterProfile,
) -> bool {
    let Some(node) = prototypes.resolved_node("loadout", loadout) else {
        return false;
    };
    let effects = node
        .get("effects")
        .and_then(DataNode::as_seq)
        .unwrap_or_default();
    effects_valid(prototypes, effects, profile)
}

/// Снаряжение по умолчанию (`SetDefault`): в каждой группе без выбора — первые подходящие,
/// не меньше минимума и выбранного по умолчанию.
pub fn set_default_loadout(
    prototypes: &Prototypes,
    loadout: &mut RoleLoadout,
    profile: &HumanoidCharacterProfile,
) {
    let Some(role) = role_loadout(prototypes, &loadout.role) else {
        return;
    };
    for group_id in role.groups.iter().rev() {
        let Some(group) = loadout_group(prototypes, group_id) else {
            continue;
        };
        if loadout.selected.contains_key(group_id) {
            continue;
        }
        let mut chosen = Vec::new();
        let wanted = group.min_limit.max(group.default_selected);
        if group.min_limit > 0 || group.default_selected > 0 {
            for id in &group.loadouts {
                if chosen.len() as i32 >= wanted {
                    break;
                }
                if prototypes.resolved_node("loadout", id).is_none()
                    || !loadout_valid(prototypes, id, profile)
                {
                    continue;
                }
                chosen.push(id.clone());
            }
        }
        loadout.selected.insert(group_id.clone(), chosen);
    }
}

/// Надеть снаряжение роли (`EquipRoleLoadout`): группы по порядку роли, у снаряжения —
/// его стартовое снаряжение и свои вещи.
pub fn equip_role_loadout(
    world: &mut World,
    entity: Entity,
    loadout: &RoleLoadout,
    role: &RoleLoadoutProto,
) {
    let mut groups: Vec<(&String, &Vec<String>)> = loadout.selected.iter().collect();
    groups.sort_by_key(|(group, _)| {
        role.groups
            .iter()
            .position(|g| g == *group)
            .map_or(-1, |i| i as i64)
    });
    let mut gears = Vec::new();
    if let Some(prototypes) = world.get_resource::<Prototypes>() {
        for (_, items) in groups {
            for id in items {
                let Some(node) = prototypes.resolved_node("loadout", id) else {
                    eprintln!("нет снаряжения {id}");
                    continue;
                };
                if let Some(gear) = node
                    .get("startingGear")
                    .and_then(DataNode::as_str)
                    .and_then(|g| starting_gear(prototypes, g))
                {
                    gears.push(gear);
                }
                gears.push(gear_from_node(node));
            }
        }
    }
    for gear in gears {
        equip_gear(world, entity, &gear);
    }
    equip_role_name(world, entity, loadout, role);
}

/// Имя роли (`EquipRoleName`): своё, если роль позволяет, иначе из набора имён.
pub fn equip_role_name(
    world: &mut World,
    entity: Entity,
    loadout: &RoleLoadout,
    role: &RoleLoadoutProto,
) {
    let mut name = role
        .can_customize_name
        .then(|| loadout.entity_name.clone())
        .flatten()
        .filter(|n| !n.is_empty());
    if name.is_none()
        && let Some(dataset) = &role.name_dataset
    {
        name = random_dataset_entry(world, dataset);
    }
    if let Some(name) = name
        && let Some(mut meta) = world.get_mut::<MetaData>(entity)
    {
        meta.name = Some(name);
    }
}

/// Случайная строка набора `dataset` или `localizedDataset`.
fn random_dataset_entry(world: &mut World, id: &str) -> Option<String> {
    let prototypes = world.get_resource::<Prototypes>()?;
    if let Some(values) = prototypes
        .resolved_node("dataset", id)
        .map(|n| strings(n.get("values")))
        .filter(|v| !v.is_empty())
    {
        let index = crate::random::random(world).range(0, values.len() as i32) as usize;
        return Some(values[index].clone());
    }
    let values = prototypes
        .resolved_node("localizedDataset", id)?
        .get("values")?;
    let prefix = values.get("prefix")?.as_str()?.to_owned();
    let count = int(values, "count", 0);
    if count <= 0 {
        return None;
    }
    let index = crate::random::random(world).range(1, count + 1);
    Some(crate::examine::loc(world, &format!("{prefix}{index}"), &[]))
}

// ID-карта.

/// Карта в слоте `id` носящего: сама карта или карта в КПК.
pub fn worn_id_card(world: &World, entity: Entity) -> Option<Entity> {
    let item = world.slot_entity(entity, "id")?;
    if world.get::<IdCard>(item).is_some() {
        return Some(item);
    }
    crate::item_slots::slot_item(world, item, PDA_ID_SLOT)
        .filter(|c| world.get::<IdCard>(*c).is_some())
}

/// Имя карты по владельцу и должности (`UpdateEntityName`).
pub fn update_id_card_name(world: &mut World, card: Entity) {
    let Some(id) = world.get::<IdCard>(card).cloned() else {
        return;
    };
    let suffix = id
        .job_title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .map(|t| format!(" ({t})"))
        .unwrap_or_default();
    let name = match id.full_name.as_deref().filter(|n| !n.trim().is_empty()) {
        Some(full) => crate::examine::loc(
            world,
            "access-id-card-component-owner-full-name-job-title-text",
            &[
                ("fullName", LocArg::from(full)),
                ("jobSuffix", LocArg::from(suffix.as_str())),
            ],
        ),
        None => crate::examine::loc(
            world,
            "access-id-card-component-owner-name-job-title-text",
            &[("jobSuffix", LocArg::from(suffix.as_str()))],
        ),
    };
    if let Some(mut meta) = world.get_mut::<MetaData>(card) {
        meta.name = Some(name);
    }
}

fn job_strings(prototypes: &Prototypes, job: &str, key: &str) -> Vec<String> {
    prototypes
        .resolved_node("job", job)
        .map(|n| strings(n.get(key)))
        .unwrap_or_default()
}

/// Доступ должности на карту (`SetAccessToJob`): уровни и группы должности, при расширенном
/// доступе — и расширенные.
pub fn set_access_to_job(world: &mut World, card: Entity, job: &str, extended: bool) {
    let Some(prototypes) = world.get_resource::<Prototypes>() else {
        return;
    };
    let mut tags = job_strings(prototypes, job, "access");
    let mut groups = job_strings(prototypes, job, "accessGroups");
    if extended {
        tags.extend(job_strings(prototypes, job, "extendedAccess"));
        groups.extend(job_strings(prototypes, job, "extendedAccessGroups"));
    }
    for group in &groups {
        if let Some(node) = prototypes.resolved_node("accessGroup", group) {
            tags.extend(strings(node.get("tags")));
        }
    }
    let Some(mut access) = world.get_mut::<Access>(card) else {
        return;
    };
    access.tags = tags.into_iter().collect();
}

/// Данные карты носящего (`SetPdaAndIdCardData`): имя, должность, значок, доступ.
pub fn set_id_card_data(
    world: &mut World,
    entity: Entity,
    name: &str,
    job: &str,
    station: Option<Entity>,
) {
    let Some(card) = worn_id_card(world, entity) else {
        return;
    };
    let (title, icon) = {
        let prototypes = world.get_resource::<Prototypes>();
        let node = prototypes.and_then(|p| p.resolved_node("job", job));
        let title = node
            .and_then(|n| n.get("name"))
            .and_then(DataNode::as_str)
            .map(str::to_owned);
        let icon = node
            .and_then(|n| n.get("icon"))
            .and_then(DataNode::as_str)
            .map(str::to_owned);
        (title, icon)
    };
    let title = title.map(|key| crate::examine::loc(world, &key, &[]));
    if let Some(mut id) = world.get_mut::<IdCard>(card) {
        id.full_name = Some(name.chars().take(30).collect());
        id.job_title = title.map(|t| t.chars().take(30).collect());
        if icon.is_some() {
            id.job_icon = icon;
        }
        id.job_prototype = Some(job.to_owned());
    }
    update_id_card_name(world, card);
    let extended = station
        .and_then(|s| world.get::<StationJobs>(s))
        .is_some_and(|j| j.extended_access);
    set_access_to_job(world, card, job, extended);
}

// Тело игрока.

/// Создать тело игрока (`SpawnPlayerMob`): сущность должности (борг, ИИ) или тело вида с
/// профилем, снаряжение роли, стартовое снаряжение, ID-карта.
pub fn spawn_player_mob(
    world: &mut World,
    at: EntityCoordinates,
    job: Option<&str>,
    profile: Option<&HumanoidCharacterProfile>,
    station: Option<Entity>,
) -> Option<Entity> {
    let (job_node_entity, gear_id, role) = {
        let prototypes = world.get_resource::<Prototypes>()?;
        let node = job.and_then(|j| prototypes.resolved_node("job", j));
        let job_entity = node
            .and_then(|n| n.get("jobEntity"))
            .and_then(DataNode::as_str)
            .map(str::to_owned);
        let gear = node
            .and_then(|n| n.get("startingGear"))
            .and_then(DataNode::as_str)
            .map(str::to_owned);
        let role = job.and_then(|j| role_loadout(prototypes, &format!("Job{j}")));
        (job_entity, gear, role)
    };
    // Снаряжение роли: из профиля или по умолчанию.
    let loadout = role.as_ref().map(|role| {
        let mut loadout = profile
            .and_then(|p| p.loadouts.get(&role.id).cloned())
            .unwrap_or_else(|| RoleLoadout {
                role: role.id.clone(),
                ..RoleLoadout::default()
            });
        if let (Some(prototypes), Some(profile)) = (world.get_resource::<Prototypes>(), profile) {
            set_default_loadout(prototypes, &mut loadout, profile);
        }
        loadout
    });

    if let Some(prototype) = job_node_entity {
        let entity = world.spawn_attached(&prototype, at).ok()?;
        if let (Some(loadout), Some(role)) = (&loadout, &role) {
            equip_role_name(world, entity, loadout, role);
        }
        return Some(entity);
    }

    let species = profile.map_or(crate::profiles::DEFAULT_SPECIES, |p| p.species.as_str());
    let prototype = world
        .get_resource::<Prototypes>()?
        .resolved_node("species", species)?
        .get("prototype")?
        .as_str()?
        .to_owned();
    let entity = match world.spawn_attached(&prototype, at) {
        Ok(entity) => entity,
        Err(error) => {
            eprintln!("тело {prototype}: {error}");
            return None;
        }
    };
    if let Some(profile) = profile {
        crate::body::apply_character_profile(world, entity, profile);
    }
    if let (Some(loadout), Some(role)) = (&loadout, &role) {
        equip_role_loadout(world, entity, loadout, role);
    }
    if let Some(gear) = gear_id.and_then(|g| starting_gear(world.get_resource::<Prototypes>()?, &g))
    {
        equip_gear(world, entity, &gear);
    }
    if let Some(job) = job {
        let name = world
            .get::<MetaData>(entity)
            .and_then(|m| m.name.clone())
            .unwrap_or_default();
        set_id_card_data(world, entity, &name, job, station);
    }
    Some(entity)
}

/// Игрок появляется на станции (`SpawnPlayerCharacterOnStation` и `PlayerSpawningEvent`):
/// случайное место из подходящих точек.
pub fn spawn_player_character_on_station(
    world: &mut World,
    station: Option<Entity>,
    job: Option<&str>,
    profile: Option<&HumanoidCharacterProfile>,
    in_round: bool,
) -> Option<Entity> {
    let positions = spawn_positions(world, station, job, in_round);
    if positions.is_empty() {
        eprintln!("нет ни одной точки появления");
        return None;
    }
    let index = crate::random::random(world).range(0, positions.len() as i32) as usize;
    spawn_player_mob(world, positions[index], job, profile, station)
}

pub fn install(world: &mut World) {
    world.register_data_component::<SpawnPoint>();
}

#[cfg(test)]
pub(crate) mod tests;
