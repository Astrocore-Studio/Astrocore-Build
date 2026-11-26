//! Броски: предмет получает скорость, летит до точки и приземляется; в полёте попадает в
//! тех, кого задевает. Образец — `Content.Shared/Throwing` SS14 @ 2f4a2aad
//! (`ThrowingSystem.TryThrow`, `ThrownItemSystem`).

use astrocore_console::ConfigWorldExt;
use astrocore_ecs::{
    EventWorldExt, GameTime, GameWorldExt, LifecycleWorldExt, RegistryWorldExt, Update,
};
use astrocore_map::TransformWorldExt;
use astrocore_math::{Angle, Vec2};
use astrocore_movement::TILE_FRICTION;
use astrocore_net::NetRegistryWorldExt;
use astrocore_physics::{
    BodyStatus, BodyType, CollisionGroups, Fixture, Fixtures, Physics, PhysicsSleep,
    PhysicsWorldExt, PreventCollide, StartCollide,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::content::groups;

/// Вращающий импульс броска (`ThrowAngularImpulse`).
pub const THROW_ANGULAR_IMPULSE: f32 = 5.0;
/// Отдача на бросающего по умолчанию (`PushbackDefault`).
pub const PUSHBACK_DEFAULT: f32 = 2.0;
/// Доля полёта при поправке на скольжение (`FlyTimePercentage`).
pub const FLY_TIME_PERCENTAGE: f32 = 0.8;
/// Множитель трения при расчёте броска (`TileFrictionMod`).
const TILE_FRICTION_MOD: f32 = 1.5;
/// Имя фикстуры, которой летящий предмет задевает других (`ThrowingFixture`).
pub const THROWING_FIXTURE: &str = "throw-fixture";

/// Предмет в полёте. Есть только пока он летит.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ThrownItem {
    #[serde(skip)]
    pub thrower: Option<Entity>,
    /// Время броска и приземления, секунды игры.
    #[serde(skip)]
    pub thrown_time: f64,
    #[serde(skip)]
    pub land_time: Option<f64>,
    #[serde(skip)]
    pub landed: bool,
    #[serde(skip)]
    pub play_land_sound: bool,
}

/// Предмет брошенный летит точно в точку, без поправки на скольжение после приземления.
/// В данных — компонент `LandAtCursor`.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandAtCursor {}

/// Под каким углом летит предмет без вращения. В данных — компонент `ThrowingAngle`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThrowingAngle {
    #[serde(default)]
    pub angle: Angle,
    /// Вращается ли в полёте.
    #[serde(default)]
    pub angular_velocity: bool,
}

/// Предмет брошен (`ThrownEvent`). Поднимается на предмете и широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct Thrown {
    pub user: Option<Entity>,
    pub thrown: Entity,
}

/// Сущность что-то бросила (`ThrowEvent`). Поднимается на бросившем и широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct Throw {
    pub user: Option<Entity>,
    pub thrown: Entity,
}

/// В сущность попал брошенный предмет (`ThrowHitByEvent`). Поднимается на цели и
/// широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct ThrowHitBy {
    pub thrown: Entity,
    pub target: Entity,
    pub thrower: Option<Entity>,
}

/// Брошенный предмет попал в сущность (`ThrowDoHitEvent`). Поднимается на предмете и
/// широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct ThrowDoHit {
    pub thrown: Entity,
    pub target: Entity,
    pub thrower: Option<Entity>,
}

/// Предмет приземлился (`LandEvent`). Поднимается на предмете.
#[derive(Debug, Clone, Copy)]
pub struct Land {
    pub user: Option<Entity>,
    pub play_sound: bool,
}

/// Полёт закончился (`StopThrowEvent`). Поднимается на предмете.
#[derive(Debug, Clone, Copy)]
pub struct StopThrow {
    pub user: Option<Entity>,
}

/// Можно ли толкнуть бросающего отдачей (`ThrowPushbackAttemptEvent`). Поднимается на предмете.
#[derive(Debug, Clone, Copy)]
pub struct ThrowPushbackAttempt {
    pub cancelled: bool,
}

/// Толкает ли бросок самого бросающего (`ThrowerImpulseEvent`): в невесомости — да.
/// Поднимается на бросающем.
#[derive(Debug, Clone, Copy)]
pub struct ThrowerImpulse {
    pub push: bool,
}

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

/// Бросить сущность в направлении `direction` (длина — дальность) со скоростью
/// `base_speed` (`TryThrow`). `compensate_friction` — уменьшить скорость так, чтобы
/// предмет остановился у точки, проскользив после приземления.
pub fn try_throw(
    world: &mut World,
    entity: Entity,
    direction: Vec2,
    base_speed: f32,
    user: Option<Entity>,
    compensate_friction: bool,
) -> bool {
    try_throw_with(
        world,
        entity,
        direction,
        base_speed,
        user,
        PUSHBACK_DEFAULT,
        compensate_friction,
    )
}

/// То же с силой отдачи на бросающего.
pub fn try_throw_with(
    world: &mut World,
    entity: Entity,
    direction: Vec2,
    base_speed: f32,
    user: Option<Entity>,
    pushback_ratio: f32,
    mut compensate_friction: bool,
) -> bool {
    if base_speed <= 0.0 || !direction.is_finite() || direction == Vec2::ZERO {
        return false;
    }
    let Some(body) = world.get::<Physics>(entity).cloned() else {
        return false;
    };
    if !matches!(
        body.body_type,
        BodyType::Dynamic | BodyType::KinematicController
    ) {
        return false;
    }
    let tile_friction = world.cvar(&TILE_FRICTION) * TILE_FRICTION_MOD;
    if tile_friction == 0.0 {
        compensate_friction = false;
    }
    let mut fly_time = direction.length() / base_speed;
    if compensate_friction {
        fly_time *= FLY_TIME_PERCENTAGE;
    }
    let thrown_time = now(world);
    let land_time = thrown_time + f64::from(fly_time);
    world.entity_mut(entity).insert(ThrownItem {
        thrower: user,
        thrown_time,
        land_time: Some(land_time),
        landed: false,
        play_land_sound: true,
    });
    add_throw_fixture(world, entity);

    // Вращение в полёте или полёт под углом.
    let throwing_angle = world.get::<ThrowingAngle>(entity).copied();
    if body.inv_inertia() > 0.0 && throwing_angle.is_none_or(|angle| angle.angular_velocity) {
        world.apply_angular_impulse(entity, THROW_ANGULAR_IMPULSE / body.inv_inertia());
    } else {
        let parent_rotation = world
            .parent(entity)
            .and_then(|parent| world.world_transform(parent))
            .map_or(0.0, |parent| parent.rotation.0);
        let offset = throwing_angle.map_or(0.0, |angle| angle.angle.0);
        let angle = Angle::from_world_vec(direction).0 - parent_rotation + offset;
        let _ = world.set_local_rotation(entity, Angle(angle));
    }

    // Скорость с поправкой на скольжение после приземления: скорость затухает
    // экспоненциально, и предмет проходит ещё v / трение.
    let speed = if compensate_friction {
        direction.length() / (fly_time + 1.0 / tile_friction)
    } else {
        base_speed
    };
    let mass = world.get::<Physics>(entity).map_or(0.0, |body| body.mass);
    let impulse = direction.normalize() * speed * mass;
    world.apply_linear_impulse(entity, impulse);

    world.raise_and_broadcast(
        entity,
        &mut Thrown {
            user,
            thrown: entity,
        },
    );
    if let Some(user) = user {
        world.raise_and_broadcast(
            user,
            &mut Throw {
                user: Some(user),
                thrown: entity,
            },
        );
    }
    if fly_time <= 0.0 {
        land(world, entity);
    } else {
        world.set_body_status(entity, BodyStatus::InAir);
    }

    // Отдача на бросающего.
    let Some(user) = user else {
        return true;
    };
    if pushback_ratio == 0.0 || mass == 0.0 || world.get::<Physics>(user).is_none() {
        return true;
    }
    let mut attempt = ThrowPushbackAttempt { cancelled: false };
    world.raise(entity, &mut attempt);
    if attempt.cancelled {
        return true;
    }
    let mut push = ThrowerImpulse { push: false };
    world.raise(user, &mut push);
    if push.push {
        const MASS_LIMIT: f32 = 5.0;
        world.apply_linear_impulse(
            user,
            -impulse / mass * pushback_ratio * mass.min(MASS_LIMIT),
        );
    }
    true
}

/// Летящий предмет задевает тех, с кем обычно не сталкивается (`ThrowItem`): у него
/// появляется нетвёрдая фикстура той же формы с маской `ThrownItem`.
fn add_throw_fixture(world: &mut World, entity: Entity) {
    let Some(mut fixtures) = world.get_mut::<Fixtures>(entity) else {
        return;
    };
    if fixtures.fixtures.len() != 1 {
        return;
    }
    let Some(shape) = fixtures
        .fixtures
        .values()
        .next()
        .map(|fixture| fixture.shape.clone())
    else {
        return;
    };
    fixtures.fixtures.insert(
        THROWING_FIXTURE.to_owned(),
        Fixture {
            shape,
            hard: false,
            layer: CollisionGroups::NONE,
            mask: CollisionGroups(groups::IMPASSABLE | groups::HIGH | groups::BULLET),
            density: 0.0,
            ..Fixture::default()
        },
    );
    world.refresh_fixtures(entity);
}

/// Приземление (`LandComponent`).
pub fn land(world: &mut World, entity: Entity) {
    let Some(mut thrown) = world.get_mut::<ThrownItem>(entity) else {
        return;
    };
    if thrown.landed {
        return;
    }
    thrown.landed = true;
    let (user, play_sound) = (thrown.thrower, thrown.play_land_sound);
    world.raise(entity, &mut Land { user, play_sound });
}

/// Конец полёта (`StopThrow`).
pub fn stop_throw(world: &mut World, entity: Entity) {
    let Some(thrown) = world.get::<ThrownItem>(entity).cloned() else {
        return;
    };
    world.set_body_status(entity, BodyStatus::OnGround);
    let removed = world
        .get_mut::<Fixtures>(entity)
        .is_some_and(|mut fixtures| fixtures.fixtures.remove(THROWING_FIXTURE).is_some());
    if removed {
        world.refresh_fixtures(entity);
    }
    world.raise(
        entity,
        &mut StopThrow {
            user: thrown.thrower,
        },
    );
    if let Ok(mut target) = world.get_entity_mut(entity) {
        target.remove::<ThrownItem>();
    }
}

/// Приземляет и останавливает предметы, время полёта которых вышло.
fn update(world: &mut World) {
    let time = now(world);
    let flying: Vec<(Entity, ThrownItem)> = world
        .query::<(Entity, &ThrownItem)>()
        .iter(world)
        .map(|(entity, thrown)| (entity, thrown.clone()))
        .collect();
    for (entity, thrown) in flying {
        if !world.is_alive(entity) {
            continue;
        }
        if thrown.land_time.is_some_and(|land_time| land_time <= time) {
            land(world, entity);
        }
        if thrown.land_time.unwrap_or(thrown.thrown_time) <= time {
            stop_throw(world, entity);
        }
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<LandAtCursor>();
    world.register_data_component::<ThrowingAngle>();
    world.register_net_serde::<LandAtCursor>();
    world.register_net_serde::<ThrowingAngle>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<ThrownItem, StartCollide>(|world, entity, event| {
        let Some(thrown) = world.get::<ThrownItem>(entity).cloned() else {
            return;
        };
        let hard = world
            .get::<Fixtures>(event.other_entity)
            .and_then(|fixtures| fixtures.fixtures.get(&event.other_fixture))
            .is_some_and(|fixture| fixture.hard);
        if !hard || Some(event.other_entity) == thrown.thrower {
            return;
        }
        let target = event.other_entity;
        let thrower = thrown.thrower;
        world.raise_and_broadcast(
            target,
            &mut ThrowHitBy {
                thrown: entity,
                target,
                thrower,
            },
        );
        world.raise_and_broadcast(
            entity,
            &mut ThrowDoHit {
                thrown: entity,
                target,
                thrower,
            },
        );
    });
    world.subscribe::<ThrownItem, PreventCollide>(|world, entity, event| {
        if world
            .get::<ThrownItem>(entity)
            .is_some_and(|thrown| thrown.thrower == Some(event.other_entity))
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<ThrownItem, PhysicsSleep>(|world, entity, _| stop_throw(world, entity));
    world.add_game_systems(Update, update);
}
