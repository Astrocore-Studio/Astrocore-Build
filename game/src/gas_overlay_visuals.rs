//! Газы и пламя на клетках у клиента: для клетки с видимым газом или пламенем — местная
//! сущность со слоями рисунков газов (непрозрачность по данным сервера) и пламени.
//! Образец — `GasTileVisibleGasOverlay` и `GasTileFireOverlay` клиента SS14 @ 2f4a2aad.

use std::collections::HashMap;

use astrocore_client::{DrawDepth, Sprite, SpriteLayer, draw_depth_named};
use astrocore_ecs::{GameWorldExt, LifecycleWorldExt, Update};
use astrocore_map::{EntityCoordinates, TileCoord, Transform, TransformWorldExt};
use astrocore_math::{Color, Vec2};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::atmos::overlay::{GasTileOverlay, OverlayTile, chunk_tiles};

const FIRE_RSI: &str = "/Textures/Effects/fire.rsi";

/// Местные сущности вида: (грид, клетка) → сущность и её данные.
#[derive(Resource, Default)]
struct OverlayEntities {
    tiles: HashMap<(Entity, TileCoord), (Entity, OverlayTile)>,
    seen: HashMap<Entity, GasTileOverlay>,
}

fn sprite_for(world: &mut World, tile: &OverlayTile) -> Sprite {
    let gas = crate::atmos::gas_data(world);
    let visible = gas.visible_gases();
    let mut layers = Vec::new();
    for (slot, opacity) in tile.opacity.iter().enumerate() {
        let Some((rsi, state)) = visible.get(slot).and_then(|id| gas.overlay[*id].clone()) else {
            continue;
        };
        if *opacity == 0 {
            continue;
        }
        layers.push(SpriteLayer {
            sprite: Some(rsi),
            state: Some(state),
            color: Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: f32::from(*opacity) / 255.0,
            },
            ..SpriteLayer::default()
        });
    }
    if tile.fire > 0 {
        layers.push(SpriteLayer {
            sprite: Some(FIRE_RSI.into()),
            state: Some(tile.fire.min(3).to_string()),
            ..SpriteLayer::default()
        });
    }
    Sprite {
        layers,
        drawdepth: draw_depth_named("Gasses").unwrap_or(DrawDepth(17)),
        no_rot: true,
        ..Sprite::default()
    }
}

fn spawn(world: &mut World, grid: Entity, coord: TileCoord, tile: &OverlayTile) -> Entity {
    let entity = world.spawn_uninitialized();
    world.entity_mut(entity).insert(Transform::default());
    let _ = world.set_coordinates(
        entity,
        EntityCoordinates::on_level(grid, coord.center() - Vec2::ZERO, coord.z),
        None,
    );
    let sprite = sprite_for(world, tile);
    world.entity_mut(entity).insert(sprite);
    world.initialize_and_start(entity, false);
    entity
}

/// Сверить местные сущности с данными сервера.
fn update(world: &mut World) {
    world.init_resource::<OverlayEntities>();
    let grids: Vec<(Entity, GasTileOverlay)> = world
        .query::<(Entity, &GasTileOverlay)>()
        .iter(world)
        .map(|(e, o)| (e, o.clone()))
        .collect();
    let mut wanted: HashMap<(Entity, TileCoord), OverlayTile> = HashMap::new();
    let mut changed = false;
    for (grid, overlay) in &grids {
        if world.resource::<OverlayEntities>().seen.get(grid) != Some(overlay) {
            changed = true;
        }
        for (chunk, tiles) in &overlay.chunks {
            for (x, y, tile) in chunk_tiles(*chunk, tiles) {
                wanted.insert((*grid, TileCoord::new(x, y, chunk.2)), tile.clone());
            }
        }
    }
    let gone_grids = world
        .resource::<OverlayEntities>()
        .seen
        .keys()
        .any(|grid| !grids.iter().any(|(g, _)| g == grid));
    if !changed && !gone_grids {
        return;
    }
    let mut entities = std::mem::take(&mut *world.resource_mut::<OverlayEntities>());
    entities.seen = grids.into_iter().collect();
    let stale: Vec<(Entity, TileCoord)> = entities
        .tiles
        .keys()
        .filter(|key| !wanted.contains_key(key))
        .copied()
        .collect();
    for key in stale {
        if let Some((entity, _)) = entities.tiles.remove(&key)
            && world.get_entity(entity).is_ok()
        {
            world.delete_entity(entity);
        }
    }
    for (key, tile) in wanted {
        match entities.tiles.get(&key) {
            Some((_, current)) if *current == tile => {}
            Some((entity, _)) => {
                let entity = *entity;
                let sprite = sprite_for(world, &tile);
                world.entity_mut(entity).insert(sprite);
                entities.tiles.insert(key, (entity, tile));
            }
            None => {
                let entity = spawn(world, key.0, key.1, &tile);
                entities.tiles.insert(key, (entity, tile));
            }
        }
    }
    *world.resource_mut::<OverlayEntities>() = entities;
}

pub(crate) fn install(world: &mut World) {
    world.add_game_systems(Update, update);
}
