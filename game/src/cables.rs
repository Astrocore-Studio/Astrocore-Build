//! Кабели: укладка из мотка, резка кусачками, вид по соседям, скрытие под полом. Образец —
//! `CableComponent`, `CableSystem` (`Placer`), `CablePlacerComponent`, `CableVisSystem`,
//! `CableVisualizerSystem`, `SubFloorHideComponent` и `SharedSubFloorHideSystem` SS14 @ 2f4a2aad.
//!
//! Под полом (`isSubfloor: false` у тайла) спрайты кабелей и труб не видны; кабель кладут
//! только на обшивку или голую клетку (`overTile` разрешает поверх пола).

use std::collections::{HashMap, HashSet};

use astrocore_ecs::{EventWorldExt, GameWorldExt, LifecycleWorldExt, RegistryWorldExt, Update};
use astrocore_map::{
    AnchorStateChanged, EntityCoordinates, MapGrid, MapWorldExt, SpawnWorldExt, TileCoord,
    TileDefinitions, Transform, TransformWorldExt,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::do_after::DoAfterDone;
use crate::interaction::{AfterInteract, InteractUsing};
use crate::node_container::{NodeGroups, node_info};
use crate::stack::StackWorldExt;
use crate::tools::{ToolWorldExt, register_tool_event};
use crate::whitelist::EntityWhitelist;

/// Ключ внешнего вида: маска соседей (`WireVisVisuals.ConnectedMask`).
pub const CONNECTED_MASK: &str = "enum.WireVisVisuals.ConnectedMask";

/// Свойства тайлов из прототипов: подпол и прочность (`IsSubFloor`, `Sturdy`). Ресурс.
#[derive(Resource, Debug, Clone, Default)]
pub struct TileProperties {
    subfloor: HashSet<String>,
    flimsy: HashSet<String>,
}

impl TileProperties {
    pub fn set(&mut self, tile: &str, subfloor: bool, sturdy: bool) {
        if subfloor {
            self.subfloor.insert(tile.to_owned());
        } else {
            self.subfloor.remove(tile);
        }
        if sturdy {
            self.flimsy.remove(tile);
        } else {
            self.flimsy.insert(tile.to_owned());
        }
    }

    pub fn is_subfloor(&self, tile: &str) -> bool {
        self.subfloor.contains(tile)
    }

    pub fn is_sturdy(&self, tile: &str) -> bool {
        !self.flimsy.contains(tile)
    }
}

fn cutting() -> Option<String> {
    Some("Cutting".into())
}

fn half() -> f64 {
    0.5
}

/// Кабель (`CableComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cable {
    #[serde(default)]
    pub cable_dropped_on_cut_prototype: Option<String>,
    #[serde(default = "cutting")]
    pub cutting_quality: Option<String>,
    #[serde(default = "half")]
    pub cutting_delay: f64,
    #[serde(default)]
    pub cable_type: Option<String>,
}

/// Моток кабеля (`CablePlacerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CablePlacer {
    #[serde(rename = "cablePrototypeID")]
    pub cable_prototype_id: String,
    #[serde(default)]
    pub blocking_wire_type: Option<String>,
    #[serde(default)]
    pub over_tile: bool,
    #[serde(default)]
    pub blacklist: Option<EntityWhitelist>,
}

/// Вид кабеля по соседям (`CableVisComponent`): какой узел смотреть.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CableVis {
    pub node: Option<String>,
}

/// Клиент: состояние слоя по маске соседей (`CableVisualizerComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CableVisualizer {
    pub state_prefix: Option<String>,
    pub extra_layer_prefix: Option<String>,
}

/// Скрывается под полом (`SubFloorHideComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubFloorHide {
    #[serde(default = "yes")]
    pub block_interactions: bool,
    #[serde(default = "yes")]
    pub block_ambience: bool,
    /// Слои, видимые и под полом (для рентгена).
    #[serde(default)]
    pub visible_layers: Vec<String>,
}

fn yes() -> bool {
    true
}

/// Окончание резки кабеля (`CableCuttingFinishedEvent`).
#[derive(Debug, Clone, Copy)]
pub struct CableCuttingFinished;

/// Имя тайла в точке грида.
fn tile_name(world: &World, grid: Entity, cell: TileCoord) -> Option<String> {
    let tile = world.get::<MapGrid>(grid)?.tile(cell);
    world
        .resource::<TileDefinitions>()
        .get(tile.type_id)
        .map(|d| d.id.clone())
}

/// Клетка под полом: тайл — подпол или пустота.
pub fn is_subfloor(world: &World, grid: Entity, cell: TileCoord) -> bool {
    tile_name(world, grid, cell).is_none_or(|name| {
        world
            .get_resource::<TileProperties>()
            .is_none_or(|p| p.is_subfloor(&name))
    })
}

/// Маски соседей кабелей (`CableVisSystem.UpdateAppearance`): кабели того же узла
/// соседних клеток в одной группе.
fn update_masks(world: &mut World, mut seen: bevy_ecs::system::Local<u64>) {
    let Some(generation) = world.get_resource::<NodeGroups>().map(|g| g.generation) else {
        return;
    };
    if *seen == generation {
        return;
    }
    *seen = generation;
    let cables: Vec<(Entity, Option<String>)> = world
        .query::<(Entity, &CableVis)>()
        .iter(world)
        .map(|(e, v)| (e, v.node.clone()))
        .collect();
    let mut masks: Vec<(Entity, i64)> = Vec::with_capacity(cables.len());
    {
        let groups = world.resource::<NodeGroups>();
        // Кабельные узлы по клетке и группе.
        let mut by_cell: HashMap<(Entity, TileCoord, usize), ()> = HashMap::new();
        let mut cell_of: HashMap<Entity, (Entity, TileCoord, usize)> = HashMap::new();
        for (entity, node) in &cables {
            let Some(transform) = world.get::<Transform>(*entity) else {
                continue;
            };
            let (Some(grid), true) = (transform.grid(), transform.anchored()) else {
                continue;
            };
            let name = node.clone().or_else(|| {
                world
                    .get::<crate::node_container::NodeContainer>(*entity)?
                    .nodes
                    .iter()
                    .find(|(_, n)| node_info(n).is_some())
                    .map(|(name, _)| name.clone())
            });
            let Some(group) = name.and_then(|name| groups.group_of(*entity, &name)) else {
                continue;
            };
            let cell = TileCoord::containing(transform.local_position(), transform.level());
            by_cell.insert((grid, cell, group), ());
            cell_of.insert(*entity, (grid, cell, group));
        }
        for (entity, _) in &cables {
            let Some(&(grid, cell, group)) = cell_of.get(entity) else {
                continue;
            };
            let mut mask = 0;
            for (bit, dx, dy) in [(1, 0, 1), (2, 0, -1), (4, 1, 0), (8, -1, 0)] {
                let neighbour = TileCoord::new(cell.x + dx, cell.y + dy, cell.z);
                if by_cell.contains_key(&(grid, neighbour, group)) {
                    mask |= bit;
                }
            }
            masks.push((*entity, mask));
        }
    }
    for (entity, mask) in masks {
        astrocore_net::AppearanceWorldExt::set_appearance(world, entity, CONNECTED_MASK, mask);
    }
}

/// Укладка кабеля из мотка (`OnCablePlacerAfterInteract`).
fn place_cable(world: &mut World, placer: Entity, event: &mut AfterInteract) {
    if event.handled || !event.can_reach {
        return;
    }
    let Some(component) = world.get::<CablePlacer>(placer).cloned() else {
        return;
    };
    let Some(grid) = world.find_grid_at(event.click) else {
        return;
    };
    let local = world
        .world_transform(grid)
        .map_or(event.click.position, |t| t.to_local(event.click.position));
    let cell = TileCoord::containing(local, 0);
    let Some(name) = tile_name(world, grid, cell) else {
        return;
    };
    let properties = world
        .get_resource::<TileProperties>()
        .cloned()
        .unwrap_or_default();
    if name == astrocore_map::SPACE_TILE
        || (!component.over_tile && !properties.is_subfloor(&name))
        || !properties.is_sturdy(&name)
    {
        return;
    }
    let anchored: Vec<Entity> = world
        .get::<MapGrid>(grid)
        .map(|g| g.anchored_at(cell).to_vec())
        .unwrap_or_default();
    for entity in anchored {
        if component
            .blacklist
            .as_ref()
            .is_some_and(|b| b.is_valid(world, entity))
        {
            return;
        }
        if world
            .get::<Cable>(entity)
            .is_some_and(|c| c.cable_type.is_some() && c.cable_type == component.blocking_wire_type)
        {
            return;
        }
    }
    if world.get::<crate::stack::Stack>(placer).is_some() && !world.try_use_stack(placer, 1) {
        return;
    }
    let _ = world.spawn_attached(
        &component.cable_prototype_id,
        EntityCoordinates::new(grid, cell.center()),
    );
    event.handled = true;
}

fn on_cut(world: &mut World, cable: Entity, event: &mut DoAfterDone<CableCuttingFinished>) {
    if event.cancelled {
        return;
    }
    drop_and_delete(world, cable);
}

/// Кабель рассыпается в моток и исчезает.
fn drop_and_delete(world: &mut World, cable: Entity) {
    let dropped = world
        .get::<Cable>(cable)
        .and_then(|c| c.cable_dropped_on_cut_prototype.clone());
    if let (Some(prototype), Some(coordinates)) = (dropped, world.coordinates(cable)) {
        let _ = world.spawn_attached(&prototype, coordinates);
    }
    world.queue_delete(cable);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Cable>();
    world.register_data_component::<CablePlacer>();
    world.register_data_component::<CableVis>();
    world.register_data_component::<CableVisualizer>();
    world.register_data_component::<SubFloorHide>();
    if !side.is_server() {
        return;
    }
    register_tool_event::<CableCuttingFinished>(world);
    world.subscribe::<Cable, InteractUsing>(|world, cable, event| {
        if event.handled {
            return;
        }
        let Some(component) = world.get::<Cable>(cable).cloned() else {
            return;
        };
        if let Some(quality) = component.cutting_quality {
            event.handled = world.use_tool(
                event.used,
                event.user,
                Some(cable),
                component.cutting_delay,
                &[&quality],
                CableCuttingFinished,
                0.0,
            );
        }
    });
    world.subscribe::<Cable, DoAfterDone<CableCuttingFinished>>(on_cut);
    // Отвязанный не удалением кабель ведёт себя как перерезанный (`OnAnchorChanged`).
    world.subscribe::<Cable, AnchorStateChanged>(|world, cable, event| {
        if event.anchored || event.detaching {
            return;
        }
        drop_and_delete(world, cable);
    });
    world.subscribe::<CablePlacer, AfterInteract>(place_cable);
    world.add_game_systems(
        Update,
        bevy_ecs::schedule::IntoScheduleConfigs::after(
            update_masks,
            crate::node_container::update_system,
        ),
    );
}

#[cfg(test)]
mod tests;
