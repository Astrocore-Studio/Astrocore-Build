//! Трение о пол для всего, что движется не само: брошенных и толкаемых предметов.
//! Образец — `Content.Shared/Friction/TileFrictionController.cs` SS14 @ 2f4a2aad.
//!
//! Каждый такт перед шагом физики телу задаётся затухание скорости: на полу — трение тайла
//! (поле `friction` прототипа тайла) на множитель `physics.tile_friction`, в воздухе и без
//! гравитации —
//! `physics.air_friction`, в космосе — `physics.offgrid_friction`.

use std::collections::HashMap;

use astrocore_console::ConfigWorldExt;
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, RegistryWorldExt, Update};
use astrocore_map::{MapGrid, MapWorldExt, TileDefinitions, Transform, TransformWorldExt};
use astrocore_movement::{InputMover, TILE_FRICTION};
use astrocore_net::NetRegistryWorldExt;
use astrocore_physics::{BodyStatus, BodyType, Physics, PhysicsWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

pub use astrocore_movement::{AIR_FRICTION, MIN_FRICTION, OFFGRID_FRICTION};

/// Множитель трения сущности или стоящего на тайле предмета (ковёр, лёд). В данных —
/// компонент `TileFrictionModifier`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TileFrictionModifier {
    #[serde(default = "one")]
    pub modifier: f32,
}

fn one() -> f32 {
    1.0
}

/// Тело может двигаться в воздухе, как по полу. В данных — компонент `CanMoveInAir`.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanMoveInAir {}

/// Системы меняют трение тела (`TileFrictionEvent`). Поднимается на теле.
#[derive(Debug, Clone, Copy)]
pub struct TileFriction {
    pub modifier: f32,
}

/// Трение тайлов по номерам определений (поле `friction` прототипов `tile`). Ресурс `World`.
#[derive(Resource, Debug, Default, Clone)]
pub struct TileFrictions {
    by_id: HashMap<String, f32>,
}

impl TileFrictions {
    pub fn set(&mut self, tile: &str, friction: f32) {
        self.by_id.insert(tile.to_owned(), friction);
    }

    /// Трение тайла; неизвестный тайл — 1 (`ContentTileDefinition.Friction`).
    pub fn get(&self, tile: &str) -> f32 {
        self.by_id.get(tile).copied().unwrap_or(1.0)
    }
}

/// Трение тайла под телом с учётом предметов на нём (`GetTileFriction`).
fn tile_friction(world: &World, entity: Entity, transform: &Transform) -> f32 {
    let Some(grid) = transform.grid() else {
        return transform
            .map()
            .and_then(|map| world.get::<TileFrictionModifier>(map))
            .map_or(1.0, |modifier| modifier.modifier);
    };
    let Some(at) = world.map_coordinates(entity) else {
        return 1.0;
    };
    let Some(tile) = world.tile_at(at).filter(|tile| tile.grid == grid) else {
        // Пустой тайл грида — считается, что гравитация есть.
        return 1.0;
    };
    let mut modifier = 1.0;
    if let Some(map_grid) = world.get::<MapGrid>(grid) {
        for anchored in map_grid.anchored_at(tile.coord) {
            if let Some(friction) = world.get::<TileFrictionModifier>(*anchored) {
                modifier *= friction.modifier;
            }
        }
    }
    let definition = world
        .get_resource::<TileDefinitions>()
        .and_then(|definitions| definitions.get(tile.tile.type_id))
        .map(|definition| definition.id.clone());
    let base = match (definition, world.get_resource::<TileFrictions>()) {
        (Some(id), Some(frictions)) => frictions.get(&id),
        _ => 1.0,
    };
    base * modifier
}

/// Задаёт затухание бодрствующим телам, которыми не управляет персонаж (`UpdateBeforeSolve`).
fn update(world: &mut World) {
    let bodies: Vec<Entity> = world
        .query::<(Entity, &Physics, Option<&InputMover>)>()
        .iter(world)
        .filter(|(_, body, mover)| {
            body.awake
                && body.body_type != BodyType::Static
                && mover.is_none_or(|mover| !mover.can_move)
                && (body.linear_velocity != astrocore_math::Vec2::ZERO
                    || body.angular_velocity != 0.0)
        })
        .map(|(entity, _, _)| entity)
        .collect();
    if bodies.is_empty() {
        return;
    }
    let (tile_scale, min, air, offgrid) = (
        world.cvar(&TILE_FRICTION),
        world.cvar(&MIN_FRICTION),
        world.cvar(&AIR_FRICTION),
        world.cvar(&OFFGRID_FRICTION),
    );
    let dt = world.resource::<GameTime>().frame_time();
    for entity in bodies {
        let (Some(body), Some(transform)) = (
            world.get::<Physics>(entity).cloned(),
            world.get::<Transform>(entity).cloned(),
        ) else {
            continue;
        };
        let in_air = body.body_status != BodyStatus::OnGround
            && world.get::<CanMoveInAir>(entity).is_none()
            || crate::gravity::is_weightless(world, entity);
        let mut friction = if in_air {
            if transform.grid().is_some() {
                air
            } else {
                offgrid
            }
        } else {
            tile_scale * tile_friction(world, entity, &transform)
        };
        let mut event = TileFriction {
            modifier: world
                .get::<TileFrictionModifier>(entity)
                .map_or(1.0, |modifier| modifier.modifier),
        };
        world.raise(entity, &mut event);
        friction *= event.modifier;
        friction = friction.max(min);
        world.set_damping(entity, friction, friction);
        // Физика не гасит скорость управляемых тел: гасим здесь (`Friction` движения).
        if body.body_type == BodyType::KinematicController {
            let scale = (1.0 - dt * friction).clamp(0.0, 1.0);
            world.set_linear_velocity(entity, body.linear_velocity * scale);
            world.set_angular_velocity(entity, body.angular_velocity * scale);
        }
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<TileFrictionModifier>();
    world.register_data_component::<CanMoveInAir>();
    world.register_net_serde::<TileFrictionModifier>();
    world.register_net_serde::<CanMoveInAir>();
    world.init_resource::<TileFrictions>();
    if side.is_server() {
        world.add_game_systems(Update, update);
    }
}
