//! Размещение в игре для маппинга: клиент просит поставить сущность или тайл, стереть
//! сущность или всё в прямоугольнике; сервер проверяет права и выполняет. Образец —
//! `Robust.Server/Placement/PlacementManager.cs` и `MsgPlacement` Robust @ 1afd692.
//!
//! Точки приходят в координатах карты: сервер сам находит грид под точкой. Тайл вне гридов
//! создаёт новый грид, как в Robust. Права те же, что у команд консоли (`map_commands`).

use astrocore_console::ConsoleShell;
use astrocore_ecs::{LifecycleWorldExt, MetaData};
use astrocore_map::{
    Map, MapCoordinates, MapGrid, MapId, MapWorldExt, SpawnWorldExt, Tile, TileCoord,
    TileDefinitions, TransformWorldExt,
};
use astrocore_math::{Angle, Vec2};
use astrocore_net::{Actor, NetEntities, NetEntity, NetEvent, NetEventWorldExt};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;

/// Запрос размещения (`MsgPlacement`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlacementRequest {
    /// Сущность прототипа в точке карты с поворотом.
    Entity {
        prototype: String,
        map: u32,
        position: Vec2,
        rotation: f32,
    },
    /// Тайл по идентификатору в клетке под точкой карты; `Space` стирает тайл.
    Tile {
        tile: String,
        map: u32,
        position: Vec2,
    },
    /// Удалить сущность (`RequestEntRemove`).
    EraseEntity { entity: NetEntity },
    /// Удалить всё в прямоугольнике карты, кроме гридов и игроков (`RequestRectRemove`), и
    /// декали в нём.
    EraseRect { map: u32, min: Vec2, max: Vec2 },
    /// Декаль прототипа: левый нижний угол в точке карты (`RequestDecalPlacementEvent`).
    Decal {
        id: String,
        map: u32,
        corner: Vec2,
        rotation: f32,
        color: Option<String>,
    },
    /// Удалить декали в прямоугольнике карты (`RequestDecalRemovalEvent`).
    EraseDecals { map: u32, min: Vec2, max: Vec2 },
}

/// Может ли сессия мапить: те же права, что у консольной команды `mapping`.
pub fn can_place(world: &World, session: Entity) -> bool {
    let shell = ConsoleShell::player(session);
    crate::admin_manager::permission(world, &shell, "mapping")
}

/// Сущность на карте: прямой потомок грида или карты (вложенное уходит с владельцем).
fn top_level(world: &World, entity: Entity) -> bool {
    world.parent(entity).is_some_and(|parent| {
        world.get::<Map>(parent).is_some() || world.get::<MapGrid>(parent).is_some()
    })
}

/// Сущность игрока или вещь на нём: такие не стираются.
fn held_by_player(world: &World, entity: Entity) -> bool {
    let mut current = Some(entity);
    while let Some(entity) = current {
        if world.get::<Actor>(entity).is_some() {
            return true;
        }
        current = world.parent(entity);
    }
    false
}

fn erasable(world: &World, entity: Entity) -> bool {
    world.get::<Map>(entity).is_none()
        && world.get::<MapGrid>(entity).is_none()
        && !held_by_player(world, entity)
}

/// Ставит тайл в точке карты; без грида под точкой создаёт грид.
pub fn place_tile(world: &mut World, map: MapId, position: Vec2, tile: &str) -> bool {
    let Some(tile) = world.resource::<TileDefinitions>().tile(tile) else {
        return false;
    };
    let at = MapCoordinates::new(map, position);
    if let Some(grid) = world.find_grid_at(at) {
        let local = world
            .world_transform(grid)
            .map_or(position, |transform| transform.to_local(position));
        return world.set_tile(grid, TileCoord::containing(local, 0), tile);
    }
    if tile == Tile::EMPTY {
        return false;
    }
    let Some(map_entity) = world.map_entity(map) else {
        return false;
    };
    // Новый грид: начало координат в углу клетки (`PlaceNewTile`).
    let grid = world.create_grid(map_entity);
    let corner = position.floor();
    if world.set_world_position(grid, corner).is_err() {
        return false;
    }
    world.set_tile(grid, TileCoord::new(0, 0, 0), tile)
}

/// Удаляет всё в прямоугольнике карты; возвращает, сколько удалено.
pub fn erase_rect(world: &mut World, map: MapId, min: Vec2, max: Vec2) -> usize {
    let found: Vec<Entity> = world
        .query::<(Entity, &MetaData)>()
        .iter(world)
        .map(|(entity, _)| entity)
        .collect();
    let mut erased = 0;
    for entity in found {
        if !top_level(world, entity) || !erasable(world, entity) {
            continue;
        }
        let Some(at) = world.map_coordinates(entity) else {
            continue;
        };
        let inside = at.map == map
            && at.position.x >= min.x
            && at.position.y >= min.y
            && at.position.x <= max.x
            && at.position.y <= max.y;
        if inside {
            world.delete_entity(entity);
            erased += 1;
        }
    }
    erased
}

/// Удаляет декали гридов карты в прямоугольнике карты.
pub fn erase_decals(world: &mut World, map: MapId, min: Vec2, max: Vec2) -> usize {
    let Some(grids) = world
        .map_entity(map)
        .and_then(|map| world.get::<Map>(map))
        .map(|m| m.grids().to_vec())
    else {
        return 0;
    };
    let mut removed = 0;
    for grid in grids {
        let Some(transform) = world.world_transform(grid) else {
            continue;
        };
        let (a, b) = (transform.to_local(min), transform.to_local(max));
        removed += crate::decals::remove_in(world, grid, a.min(b), a.max(b));
    }
    removed
}

/// Выполняет запрос от имени сессии.
pub fn handle(world: &mut World, session: Entity, request: &PlacementRequest) {
    if !can_place(world, session) {
        return;
    }
    match request {
        PlacementRequest::Entity {
            prototype,
            map,
            position,
            rotation,
        } => {
            if !position.is_finite() || !rotation.is_finite() {
                return;
            }
            let at = MapCoordinates::new(MapId(*map), *position);
            if let Ok(entity) = world.spawn_on_map(prototype, at)
                && *rotation != 0.0
            {
                let _ = world.set_local_rotation(entity, Angle(*rotation));
            }
        }
        PlacementRequest::Tile {
            tile,
            map,
            position,
        } => {
            if position.is_finite() {
                place_tile(world, MapId(*map), *position, tile);
            }
        }
        PlacementRequest::EraseEntity { entity } => {
            let Some(entity) = world.resource::<NetEntities>().entity(*entity) else {
                return;
            };
            if erasable(world, entity) {
                world.delete_entity(entity);
            }
        }
        PlacementRequest::EraseRect { map, min, max } => {
            if min.is_finite() && max.is_finite() {
                erase_rect(world, MapId(*map), min.min(*max), min.max(*max));
                erase_decals(world, MapId(*map), min.min(*max), min.max(*max));
            }
        }
        PlacementRequest::EraseDecals { map, min, max } => {
            if min.is_finite() && max.is_finite() {
                erase_decals(world, MapId(*map), min.min(*max), min.max(*max));
            }
        }
        PlacementRequest::Decal {
            id,
            map,
            corner,
            rotation,
            color,
        } => {
            let known = world
                .get_resource::<astrocore_prototypes::Prototypes>()
                .is_some_and(|p| p.resolved_node("decal", id).is_some());
            if !known || !corner.is_finite() || !rotation.is_finite() {
                return;
            }
            // Угол декали — в клетке под её центром.
            let center = *corner + Vec2::splat(0.5);
            let Some(grid) = world.find_grid_at(MapCoordinates::new(MapId(*map), center)) else {
                return;
            };
            let local = world
                .world_transform(grid)
                .map_or(*corner, |transform| transform.to_local(*corner));
            crate::decals::place(
                world,
                grid,
                local,
                crate::decals::Decal {
                    coordinates: local,
                    id: id.clone(),
                    color: color.clone(),
                    angle: *rotation,
                    z_index: 0,
                    cleanable: false,
                },
            );
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_net_event::<PlacementRequest>();
    if side.is_server() {
        world.subscribe_net_event::<PlacementRequest>(
            |world, event: &mut NetEvent<PlacementRequest>| {
                if let Some(session) = event.session {
                    handle(world, session, &event.event);
                }
            },
        );
    }
}

#[cfg(test)]
mod tests;
