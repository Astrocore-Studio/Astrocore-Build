//! Машины из нескольких частей: главная часть ищет остальные по смещениям от себя
//! (с учётом поворота), с нужным компонентом, поворотом и готовым узлом строительства.
//! Образец — `MultipartMachineSystem` и `SharedMultipartMachineSystem` SS14 @ 2f4a2aad.
//!
//! Нет пока: призраков недостающих частей на клиенте.

use std::collections::BTreeMap;

use astrocore_ecs::{EventWorldExt, MapInit, RegistryWorldExt};
use astrocore_map::{AnchorStateChanged, MapGrid, TileCoord, Transform, TransformWorldExt};
use astrocore_math::{Angle, IVec2};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::construction::{Construction, ConstructionNodeChanged};

/// Наибольшее расстояние от части до машины при пересмотре (`MaximumRange`).
const MAXIMUM_RANGE: f32 = 30.0;

/// Часть машины (`MachinePart`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinePart {
    /// Компонент, который должен быть у части.
    pub component: String,
    /// Смещение клетки от машины, `"x, y"`.
    pub offset: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub rotation: Angle,
    #[serde(default)]
    pub graph: String,
    #[serde(default)]
    pub expected_node: String,
    /// Призрак недостающей части на клиенте (пока не рисуется).
    #[serde(default)]
    pub ghost_proto: Option<String>,
    #[serde(skip)]
    pub entity: Option<Entity>,
}

impl MachinePart {
    fn offset(&self) -> IVec2 {
        let mut parts = self
            .offset
            .split(',')
            .map(|v| v.trim().parse::<i32>().unwrap_or(0));
        IVec2::new(parts.next().unwrap_or(0), parts.next().unwrap_or(0))
    }
}

/// Машина из частей (`MultipartMachineComponent`). Ключ части — как в данных
/// (`enum.AcceleratorParts.EndCap`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultipartMachine {
    #[serde(default)]
    pub parts: BTreeMap<String, MachinePart>,
    #[serde(default)]
    pub is_assembled: bool,
}

/// Сущность может быть частью машины (`MultipartMachinePartComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MultipartMachinePart {
    #[serde(skip)]
    pub master: Option<Entity>,
}

/// Сборка машины изменилась (`MultipartMachineAssemblyStateChanged`). На машине.
#[derive(Debug, Clone)]
pub struct AssemblyStateChanged {
    pub is_assembled: bool,
    pub user: Option<Entity>,
    pub parts_added: BTreeMap<String, Entity>,
    pub parts_removed: BTreeMap<String, Entity>,
}

/// Сущность части по ключу (`GetPartEntity`).
pub fn part_entity(world: &World, machine: Entity, key: &str) -> Option<Entity> {
    world
        .get::<MultipartMachine>(machine)?
        .parts
        .get(key)?
        .entity
}

/// Собрана ли машина (`IsAssembled`).
pub fn is_assembled(world: &World, machine: Entity) -> bool {
    world
        .get::<MultipartMachine>(machine)
        .is_some_and(|m| m.is_assembled)
}

fn cardinal(angle: Angle) -> Angle {
    angle.cardinal_direction().angle()
}

fn same_angle(a: Angle, b: Angle) -> bool {
    let diff = (a.0 - b.0).rem_euclid(std::f32::consts::TAU);
    diff < 0.01 || diff > std::f32::consts::TAU - 0.01
}

/// Поиск части в ожидаемой клетке (`ScanPart`).
fn scan_part(
    world: &World,
    grid: Entity,
    origin: TileCoord,
    rotation: Angle,
    part: &MachinePart,
) -> Option<Entity> {
    let offset = part.offset();
    let turned = rotation.rotate(astrocore_math::Vec2::new(offset.x as f32, offset.y as f32));
    let cell = origin.offset(IVec2::new(turned.x.round() as i32, turned.y.round() as i32));
    let expected = part.rotation + rotation;
    world
        .get::<MapGrid>(grid)?
        .anchored_at(cell)
        .iter()
        .copied()
        .find(|entity| {
            crate::whitelist::has_component_named(world, *entity, &part.component)
                && world
                    .get::<Transform>(*entity)
                    .is_some_and(|t| same_angle(t.local_rotation(), expected))
                && world
                    .get::<Construction>(*entity)
                    .is_some_and(|c| c.graph == part.graph && c.node == part.expected_node)
        })
}

/// Пересмотреть части (`Rescan`). `true` — что-то изменилось.
pub fn rescan(world: &mut World, machine: Entity, user: Option<Entity>) -> bool {
    let Some(transform) = world.get::<Transform>(machine).cloned() else {
        return false;
    };
    let Some(grid) = transform.grid() else {
        return false;
    };
    if !transform.anchored() || world.parent(machine) != Some(grid) {
        return false;
    }
    let Some(mut component) = world.get::<MultipartMachine>(machine).cloned() else {
        return false;
    };
    let origin = TileCoord::containing(transform.local_position(), transform.level());
    let rotation = cardinal(transform.local_rotation());
    let mut added = BTreeMap::new();
    let mut removed = BTreeMap::new();
    let mut missing = false;
    for (key, part) in component.parts.iter_mut() {
        let original = part.entity;
        part.entity = scan_part(world, grid, origin, rotation, part);
        if part.entity.is_none() && !part.optional {
            missing = true;
        }
        if part.entity == original {
            continue;
        }
        match part.entity {
            Some(entity) => {
                added.insert(key.clone(), entity);
            }
            None => {
                if let Some(old) = original {
                    removed.insert(key.clone(), old);
                }
            }
        }
    }
    component.is_assembled = !missing;
    let changed = !added.is_empty() || !removed.is_empty();
    world.entity_mut(machine).insert(component.clone());
    for entity in added.values() {
        world.entity_mut(*entity).insert(MultipartMachinePart {
            master: Some(machine),
        });
    }
    for entity in removed.values() {
        if let Some(mut part) = world.get_mut::<MultipartMachinePart>(*entity) {
            part.master = None;
        }
    }
    if changed {
        world.raise(
            machine,
            &mut AssemblyStateChanged {
                is_assembled: component.is_assembled,
                user,
                parts_added: added,
                parts_removed: removed,
            },
        );
    }
    changed
}

/// Отсоединить все части (`ClearAllParts`).
fn clear_all_parts(world: &mut World, machine: Entity) {
    let Some(mut component) = world.get::<MultipartMachine>(machine).cloned() else {
        return;
    };
    let mut cleared = BTreeMap::new();
    for (key, part) in component.parts.iter_mut() {
        if let Some(entity) = part.entity.take() {
            cleared.insert(key.clone(), entity);
        }
    }
    component.is_assembled = false;
    world.entity_mut(machine).insert(component);
    for entity in cleared.values() {
        if let Some(mut part) = world.get_mut::<MultipartMachinePart>(*entity) {
            part.master = None;
        }
    }
    if !cleared.is_empty() {
        world.raise(
            machine,
            &mut AssemblyStateChanged {
                is_assembled: false,
                user: None,
                parts_added: BTreeMap::new(),
                parts_removed: cleared,
            },
        );
    }
}

/// Машины не дальше `MAXIMUM_RANGE` от сущности.
fn machines_near(world: &mut World, entity: Entity) -> Vec<Entity> {
    let Some(at) = world.map_coordinates(entity) else {
        return Vec::new();
    };
    let machines: Vec<Entity> = world
        .query::<(Entity, &MultipartMachine)>()
        .iter(world)
        .map(|(e, _)| e)
        .collect();
    machines
        .into_iter()
        .filter(|m| {
            world.map_coordinates(*m).is_some_and(|p| {
                p.map == at.map && (p.position - at.position).length() <= MAXIMUM_RANGE
            })
        })
        .collect()
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<MultipartMachine>();
    world.register_data_component::<MultipartMachinePart>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<MultipartMachine, MapInit>(|world, machine, _| {
        rescan(world, machine, None);
    });
    world.subscribe::<MultipartMachine, AnchorStateChanged>(|world, machine, event| {
        if event.anchored {
            rescan(world, machine, None);
        } else {
            clear_all_parts(world, machine);
        }
    });
    world.subscribe::<MultipartMachinePart, AnchorStateChanged>(|world, part, event| {
        if !event.anchored {
            if let Some(master) = world
                .get::<MultipartMachinePart>(part)
                .and_then(|p| p.master)
            {
                rescan(world, master, None);
            }
            return;
        }
        for machine in machines_near(world, part) {
            if rescan(world, machine, None)
                && world
                    .get::<MultipartMachinePart>(part)
                    .is_some_and(|p| p.master == Some(machine))
            {
                return;
            }
        }
    });
    world.subscribe::<MultipartMachinePart, ConstructionNodeChanged>(|world, part, event| {
        for machine in machines_near(world, part) {
            let relevant = world.get::<MultipartMachine>(machine).is_some_and(|m| {
                m.parts.values().any(|p| {
                    p.graph == event.graph
                        && (p.expected_node == event.previous || p.expected_node == event.current)
                })
            });
            if relevant {
                rescan(world, machine, None);
            }
        }
    });
}
