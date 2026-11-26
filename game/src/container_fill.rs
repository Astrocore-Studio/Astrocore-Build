//! Начальное содержимое контейнеров на инициализированной карте: шкафы, ящики, рюкзаки,
//! ящики с частями двигателя. Образец — `StorageFillComponent`, `StorageSystem.Fill`,
//! `ContainerFillComponent`, `EntityTableContainerFillComponent`, `ContainerFillSystem`,
//! `EntitySpawnEntry`, `EntitySpawnCollection`, `EntityTableSelector` и его виды (`EntSelector`,
//! `AllSelector`, `GroupSelector`, `NestedSelector`, `NoneSelector`), `NumberSelector`
//! (`Constant`, `Range`, `Binomial`) SS14 @ 2f4a2aad.
//!
//! Таблицы хранятся как записи данных и разбираются при заполнении: так годятся и таблицы
//! прототипов `entityTable`, на которые ссылается `NestedSelector`. Условия таблиц
//! (`conditions`) пока не проверяются: условия раундов появятся в фазе 6.

use std::collections::BTreeMap;

use astrocore_data::DataNode;
use astrocore_ecs::{EventWorldExt, MapInit, RegistryWorldExt};
use astrocore_map::{ContainerWorldExt, EntityCoordinates, SpawnWorldExt};
use astrocore_math::Vec2;
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::random::{GameRandom, random};

/// Запись начинки (`EntitySpawnEntry`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitySpawnEntry {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default = "one_f", rename = "prob")]
    pub prob: f32,
    #[serde(default, rename = "orGroup")]
    pub or_group: Option<String>,
    #[serde(default = "one_i")]
    pub amount: i32,
    #[serde(default = "one_i")]
    pub max_amount: i32,
}

fn one_f() -> f32 {
    1.0
}

fn one_i() -> i32 {
    1
}

/// Начинка хранилища (`StorageFillComponent`): в `Storage` или в `EntityStorage`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageFill {
    pub contents: Vec<EntitySpawnEntry>,
}

/// Начинка контейнеров списком прототипов (`ContainerFillComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContainerFill {
    pub containers: BTreeMap<String, Vec<String>>,
    pub ignore_construction_spawn: Option<bool>,
}

/// Начинка контейнеров по таблицам (`EntityTableContainerFillComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EntityTableContainerFill {
    pub containers: BTreeMap<String, DataNode>,
    pub sort: bool,
    pub context_containers: bool,
}

/// Что выпало по записям начинки (`EntitySpawnCollection.GetSpawns`).
pub fn spawn_entries(entries: &[EntitySpawnEntry], rng: &mut GameRandom) -> Vec<String> {
    let amount = |entry: &EntitySpawnEntry, rng: &mut GameRandom| {
        if entry.max_amount <= entry.amount {
            entry.amount
        } else {
            rng.range(entry.amount, entry.max_amount)
        }
    };
    let mut spawned = Vec::new();
    let mut groups: BTreeMap<&str, Vec<&EntitySpawnEntry>> = BTreeMap::new();
    let mut group_order: Vec<&str> = Vec::new();
    for entry in entries {
        match entry.or_group.as_deref() {
            Some(group) => {
                if !groups.contains_key(group) {
                    group_order.push(group);
                }
                groups.entry(group).or_default().push(entry);
            }
            None => {
                if entry.prob != 1.0 && !rng.prob(entry.prob) {
                    continue;
                }
                let Some(id) = &entry.id else {
                    continue;
                };
                for _ in 0..amount(entry, rng) {
                    spawned.push(id.clone());
                }
            }
        }
    }
    for group in group_order {
        let members = &groups[group];
        let total: f64 = members.iter().map(|e| f64::from(e.prob)).sum();
        let roll = rng.next_f64() * total;
        let mut cumulative = 0.0;
        for entry in members {
            cumulative += f64::from(entry.prob);
            if roll > cumulative {
                continue;
            }
            if let Some(id) = &entry.id {
                for _ in 0..amount(entry, rng) {
                    spawned.push(id.clone());
                }
            }
            break;
        }
    }
    spawned
}

/// Число по `NumberSelector`: `3`, `1,4` или запись с тегом.
fn number(node: Option<&DataNode>, rng: &mut GameRandom) -> i32 {
    let Some(node) = node else {
        return 1;
    };
    match node.type_tag() {
        Some("ConstantNumberSelector") => node
            .get("value")
            .and_then(DataNode::as_str)
            .and_then(|v| v.parse().ok())
            .unwrap_or(1),
        Some("RangeNumberSelector") => range(node.get("range"), rng).unwrap_or(1),
        Some("BinomialNumberSelector") => {
            let trials: i32 = node
                .get("trials")
                .and_then(DataNode::as_str)
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let chance: f32 = node
                .get("chance")
                .and_then(DataNode::as_str)
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.5);
            (0..trials).filter(|_| rng.prob(chance)).count() as i32
        }
        _ => {
            let text = node.as_str().unwrap_or("1");
            text.parse()
                .ok()
                .or_else(|| range(Some(node), rng))
                .unwrap_or(1)
        }
    }
}

/// `x,y` включительно.
fn range(node: Option<&DataNode>, rng: &mut GameRandom) -> Option<i32> {
    let (min, max) = node?.as_str()?.split_once(',')?;
    let (min, max): (i32, i32) = (min.trim().parse().ok()?, max.trim().parse().ok()?);
    Some(rng.range(min, max + 1))
}

fn float(node: &DataNode, key: &str, default: f32) -> f32 {
    node.get(key)
        .and_then(DataNode::as_str)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Глубина вложенных таблиц, после которой таблица считается зацикленной.
const MAX_TABLE_DEPTH: u32 = 32;

/// Что выпало по таблице (`EntityTableSelector.GetSpawns`).
pub fn table_spawns(node: &DataNode, prototypes: &Prototypes, rng: &mut GameRandom) -> Vec<String> {
    let mut out = Vec::new();
    collect(node, prototypes, rng, &mut out, 0);
    out
}

fn collect(
    node: &DataNode,
    prototypes: &Prototypes,
    rng: &mut GameRandom,
    out: &mut Vec<String>,
    depth: u32,
) {
    if depth > MAX_TABLE_DEPTH {
        return;
    }
    let rolls = number(node.get("rolls"), rng);
    let prob = float(node, "prob", 1.0);
    let children = || {
        node.get("children")
            .and_then(DataNode::as_seq)
            .unwrap_or_default()
    };
    for _ in 0..rolls {
        if prob < 1.0 && !rng.prob(prob) {
            continue;
        }
        // Запись с `id` без тега — `EntSelector` (`EntityTableTypeSerializer`).
        let kind = node
            .type_tag()
            .or_else(|| node.get("id").map(|_| "EntSelector"));
        match kind {
            Some("EntSelector") => {
                let Some(id) = node.get("id").and_then(DataNode::as_str) else {
                    continue;
                };
                for _ in 0..number(node.get("amount"), rng) {
                    out.push(id.to_owned());
                }
            }
            Some("AllSelector") => {
                for child in children() {
                    collect(child, prototypes, rng, out, depth + 1);
                }
            }
            Some("GroupSelector") => {
                let options = children();
                let weights: Vec<f32> = options.iter().map(|c| float(c, "weight", 1.0)).collect();
                if let Some(index) = rng.pick_weighted(&weights) {
                    collect(&options[index], prototypes, rng, out, depth + 1);
                }
            }
            Some("NestedSelector") => {
                let table = node
                    .get("tableId")
                    .and_then(DataNode::as_str)
                    .and_then(|id| prototypes.resolved_node("entityTable", id))
                    .and_then(|proto| proto.get("table").cloned());
                if let Some(table) = table {
                    collect(&table, prototypes, rng, out, depth + 1);
                }
            }
            _ => {}
        }
    }
}

/// Создаёт сущность прототипа внутри владельца (в его координатах).
fn spawn_inside(world: &mut World, owner: Entity, prototype: &str) -> Option<Entity> {
    world
        .spawn_attached(prototype, EntityCoordinates::new(owner, Vec2::ZERO))
        .ok()
}

fn fill_container(world: &mut World, owner: Entity, container: &str, prototypes: Vec<String>) {
    if world.container(owner, container).is_none() {
        return;
    }
    for prototype in prototypes {
        let Some(spawned) = spawn_inside(world, owner, &prototype) else {
            continue;
        };
        if world
            .force_insert_into_container(spawned, owner, container)
            .is_err()
        {
            astrocore_ecs::LifecycleWorldExt::delete_entity(world, spawned);
            break;
        }
    }
}

fn storage_fill(world: &mut World, owner: Entity) {
    let Some(fill) = world.get::<StorageFill>(owner).cloned() else {
        return;
    };
    let spawns = spawn_entries(&fill.contents, &mut random(world));
    if world.get::<crate::storage::Storage>(owner).is_some() {
        for prototype in spawns {
            let Some(item) = spawn_inside(world, owner, &prototype) else {
                continue;
            };
            if !crate::storage::StorageWorldExt::storage_insert(world, owner, item, None, false) {
                astrocore_ecs::LifecycleWorldExt::delete_entity(world, item);
            }
        }
    } else if world
        .get::<crate::entity_storage::EntityStorage>(owner)
        .is_some()
    {
        fill_container(
            world,
            owner,
            crate::entity_storage::ENTITY_STORAGE_CONTAINER,
            spawns,
        );
    }
}

fn container_fill(world: &mut World, owner: Entity) {
    let Some(fill) = world.get::<ContainerFill>(owner).cloned() else {
        return;
    };
    for (container, prototypes) in fill.containers {
        fill_container(world, owner, &container, prototypes);
    }
}

fn table_fill(world: &mut World, owner: Entity) {
    let Some(fill) = world.get::<EntityTableContainerFill>(owner).cloned() else {
        return;
    };
    for (container, table) in fill.containers {
        let spawns = world.resource_scope(|world, prototypes: bevy_ecs::world::Mut<Prototypes>| {
            table_spawns(&table, &prototypes, &mut random(world))
        });
        fill_container(world, owner, &container, spawns);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<StorageFill>();
    world.register_data_component::<ContainerFill>();
    world.register_data_component::<EntityTableContainerFill>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<StorageFill, MapInit>(|world, entity, _| storage_fill(world, entity));
    world.subscribe::<ContainerFill, MapInit>(|world, entity, _| container_fill(world, entity));
    world.subscribe::<EntityTableContainerFill, MapInit>(|world, entity, _| {
        table_fill(world, entity)
    });
}

#[cfg(test)]
mod tests;
