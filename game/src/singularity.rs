//! Сингулярность: копит энергию, растёт по уровням, поглощает сущности и тайлы в горизонте
//! событий, притягивает тела гравитационным колодцем и блуждает. Появляется из генератора
//! сингулярности, когда в него попадает достаточно частиц ускорителя. Поле сдерживания её
//! не пускает, пока уровень ниже пятого. Образец — `SharedSingularitySystem`,
//! `SingularitySystem`, `SharedEventHorizonSystem`, `EventHorizonSystem`,
//! `GravityWellSystem`, `RandomWalkController` и `SingularityGeneratorSystem`
//! SS14 @ 2f4a2aad.
//!
//! Нет пока: искажения пространства (шейдер), радиации (её задаёт `radiation`), поглощения
//! содержимого контейнеров по одному и сингулярности внутри контейнера.

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{
    EventWorldExt, GameTime, GameWorldExt, LifecycleWorldExt, MapInit, RegistryWorldExt, Update,
};
use astrocore_light::PointLight;
use astrocore_map::{
    ContainerWorldExt, Map, MapGrid, MapWorldExt, SpawnWorldExt, Tile, TileCoord, TransformWorldExt,
};
use astrocore_math::{Angle, Box2, Direction, Vec2};
use astrocore_net::AppearanceWorldExt;
use astrocore_physics::{
    BodyType, CollisionGroups, Fixtures, Physics, PhysicsWorldExt, PreventCollide, Ray,
    RayWorldExt, Shape, StartCollide, group_bits,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::containment::{ContainmentField, ContainmentFieldGenerator};
use crate::examine::loc;
use crate::popups::{PopupType, PopupWorldExt};
use crate::random::random;

/// Наибольший уровень (`MaxSingularityLevel`).
pub const MAX_LEVEL: u8 = 6;
/// С этого уровня сингулярность проходит поле (`SingularityBreachThreshold`).
pub const BREACH_THRESHOLD: u8 = 5;
/// Уровень на клиенте (`SingularityAppearanceKeys.Singularity`).
pub const SINGULARITY_VISUALS: &str = "enum.SingularityAppearanceKeys.Singularity";

/// Сингулярность (`SingularityComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Singularity {
    #[serde(default = "one_level")]
    pub level: u8,
    #[serde(default = "two")]
    pub rads_per_level: f32,
    #[serde(default = "start_energy")]
    pub energy: f32,
    /// Потеря энергии в секунду (`EnergyDrain`).
    #[serde(default, rename = "energyLoss")]
    pub energy_drain: f32,
    #[serde(default)]
    pub formation_sound: Option<SoundSpecifier>,
    #[serde(default = "dissipation")]
    pub dissipation_sound: Option<SoundSpecifier>,
}

fn one_level() -> u8 {
    1
}
fn two() -> f32 {
    2.0
}
fn start_energy() -> f32 {
    180.0
}
fn dissipation() -> Option<SoundSpecifier> {
    Some(SoundSpecifier::path(
        "/Audio/Effects/singularity_collapse.ogg",
    ))
}

/// Горизонт событий (`EventHorizonComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventHorizon {
    #[serde(default)]
    pub radius: f32,
    #[serde(default = "yes")]
    pub consume_tiles: bool,
    #[serde(default = "yes")]
    pub consume_entities: bool,
    #[serde(default)]
    pub can_breach_containment: bool,
    #[serde(default = "consumer")]
    pub consumer_fixture_id: Option<String>,
    #[serde(default = "collider")]
    pub collider_fixture_id: Option<String>,
    /// Секунд между волнами поглощения (`consumePeriod`).
    #[serde(default = "half", rename = "consumePeriod")]
    pub target_consume_period: f64,
    #[serde(skip)]
    pub next_consume_wave_time: f64,
    #[serde(skip)]
    pub being_consumed: bool,
}

fn yes() -> bool {
    true
}
fn consumer() -> Option<String> {
    Some("EventHorizonConsumer".into())
}
fn collider() -> Option<String> {
    Some("EventHorizonCollider".into())
}
fn half() -> f64 {
    0.5
}

/// Гравитационный колодец (`GravityWellComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GravityWell {
    #[serde(default)]
    pub max_range: f32,
    #[serde(default)]
    pub min_range: f32,
    #[serde(default)]
    pub base_radial_acceleration: f32,
    #[serde(default)]
    pub base_tangential_acceleration: f32,
    #[serde(default = "half", rename = "gravPulsePeriod")]
    pub target_pulse_period: f64,
    #[serde(skip)]
    pub next_pulse_time: f64,
}

/// Случайное блуждание (`RandomWalkComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RandomWalk {
    #[serde(default = "min_speed")]
    pub min_speed: f32,
    #[serde(default = "max_speed")]
    pub max_speed: f32,
    #[serde(default)]
    pub accumulator_ratio: f32,
    #[serde(default)]
    pub step_on_startup: bool,
    #[serde(default = "min_cooldown")]
    pub min_step_cooldown: f64,
    #[serde(default = "max_cooldown")]
    pub max_step_cooldown: f64,
    #[serde(skip)]
    pub next_step_time: f64,
}

fn min_speed() -> f32 {
    7.5
}
fn max_speed() -> f32 {
    10.0
}
fn min_cooldown() -> f64 {
    2.0
}
fn max_cooldown() -> f64 {
    5.0
}

/// Пища сингулярности (`SinguloFoodComponent`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SinguloFood {
    #[serde(default = "one")]
    pub energy: f32,
    #[serde(default = "one")]
    pub energy_factor: f32,
}

fn one() -> f32 {
    1.0
}

/// Генератор сингулярности (`SingularityGeneratorComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SingularityGenerator {
    #[serde(default)]
    pub power: f32,
    #[serde(default = "sixteen")]
    pub threshold: f32,
    #[serde(default)]
    pub inert: bool,
    #[serde(default)]
    pub failsafe_disabled: bool,
    #[serde(default = "sixteen")]
    pub failsafe_distance: f32,
    #[serde(default = "singularity")]
    pub spawn_prototype: Option<String>,
    #[serde(default = "ten")]
    pub failsafe_cooldown: f64,
    #[serde(skip)]
    pub next_failsafe: f64,
}

fn sixteen() -> f32 {
    16.0
}
fn ten() -> f64 {
    10.0
}
fn singularity() -> Option<String> {
    Some("Singularity".into())
}

/// Частица ускорителя (`ParticleProjectileComponent`): мощность ускорителя при выстреле.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParticleProjectile {
    /// `Standby`, `Level0`…`Level3`.
    #[serde(default)]
    pub state: String,
}

/// Сущность уже поглощена и ждёт удаления.
#[derive(Component, Debug, Clone, Copy)]
struct Consumed;

/// Можно ли поглотить (`EventHorizonAttemptConsumeEntityEvent`). На цели.
#[derive(Debug, Clone, Copy)]
pub struct AttemptConsume {
    pub horizon: Entity,
    pub can_breach_containment: bool,
    pub cancelled: bool,
}

/// Горизонт поглотил сущность (`EntityConsumedByEventHorizonEvent`). На горизонте.
#[derive(Debug, Clone, Copy)]
pub struct ConsumedEntity {
    pub entity: Entity,
}

/// Сущность поглощена горизонтом (`EventHorizonConsumedEntityEvent`). На поглощённой.
#[derive(Debug, Clone, Copy)]
pub struct ConsumedBy {
    pub horizon: Entity,
}

/// Уровень сменился (`SingularityLevelChangedEvent`). На сингулярности.
#[derive(Debug, Clone, Copy)]
pub struct LevelChanged {
    pub new_value: u8,
    pub old_value: u8,
}

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

// Уровень и энергия.

/// Радиус горизонта по уровню (`EventHorizonRadius`).
pub fn horizon_radius(level: u8) -> f32 {
    f32::from(level) - 0.5
}

/// Уровень по энергии (`SetEnergy`).
pub fn level_for_energy(energy: f32) -> u8 {
    match energy {
        e if e >= 5000.0 => 6,
        e if e >= 2000.0 => 5,
        e if e >= 1000.0 => 4,
        e if e >= 500.0 => 3,
        e if e >= 200.0 => 2,
        e if e > 0.0 => 1,
        _ => 0,
    }
}

fn set_fixture_radius(fixtures: &mut Fixtures, id: &str, radius: f32, hard: bool) {
    if let Some(fixture) = fixtures.fixtures.get_mut(id) {
        if let Shape::PhysShapeCircle { radius: r, .. } = &mut fixture.shape {
            *r = radius;
        }
        fixture.hard = hard;
    }
}

/// Фикстуры горизонта по радиусу (`UpdateEventHorizonFixture`).
fn update_horizon_fixture(world: &mut World, entity: Entity) {
    let Some(horizon) = world.get::<EventHorizon>(entity).cloned() else {
        return;
    };
    let (Some(consumer), Some(collider)) =
        (horizon.consumer_fixture_id, horizon.collider_fixture_id)
    else {
        return;
    };
    let radius = horizon.radius.max(0.0);
    let Some(mut fixtures) = world.get::<Fixtures>(entity).cloned() else {
        return;
    };
    set_fixture_radius(&mut fixtures, &consumer, radius, false);
    set_fixture_radius(&mut fixtures, &collider, radius, true);
    world.entity_mut(entity).insert(fixtures);
    world.refresh_fixtures(entity);
}

/// Пересчёт по уровню (`UpdateSingularityLevel` и обработчики `SingularityLevelChangedEvent`).
fn update_level(world: &mut World, entity: Entity, old: u8) {
    let Some(singularity) = world.get::<Singularity>(entity).cloned() else {
        return;
    };
    let level = singularity.level;
    if let Some(mut horizon) = world.get_mut::<EventHorizon>(entity) {
        horizon.radius = horizon_radius(level);
        horizon.can_breach_containment = level >= BREACH_THRESHOLD;
    }
    update_horizon_fixture(world, entity);
    if level <= 1 && old > 1 {
        world.set_linear_velocity(entity, Vec2::ZERO);
    }
    world.set_appearance(entity, SINGULARITY_VISUALS, i64::from(level));
    crate::radiation::set_intensity(world, entity, f32::from(level) * singularity.rads_per_level);
    // `UpdateEnergyDrain`.
    if let Some(mut singularity) = world.get_mut::<Singularity>(entity) {
        singularity.energy_drain = match level {
            4 => 20.0,
            3 => 10.0,
            2 => 5.0,
            1 => 1.0,
            _ => 0.0,
        };
    }
    // `UpdateRandomWalk`.
    if let Some(mut walk) = world.get_mut::<RandomWalk>(entity) {
        let scale = f32::from(level).max(4.0);
        walk.min_speed = 7.5 / scale;
        walk.max_speed = 10.0 / scale;
    }
    // `UpdateGravityWell`.
    if let Some(mut well) = world.get_mut::<GravityWell>(entity) {
        well.max_range = 2.0 * (f32::from(level) + 1.0);
        well.base_radial_acceleration = 10.0 * f32::from(level);
        well.base_tangential_acceleration = 0.0;
    }
    world.raise(
        entity,
        &mut LevelChanged {
            new_value: level,
            old_value: old,
        },
    );
    if level == 0 {
        if let Some(sound) = &singularity.dissipation_sound
            && let Some(at) = world.map_coordinates(entity)
        {
            world.play_at(sound, at, None);
        }
        world.queue_delete(entity);
    }
}

/// Уровень (`SetLevel`).
pub fn set_level(world: &mut World, entity: Entity, value: u8) {
    let value = value.min(MAX_LEVEL);
    let Some(mut singularity) = world.get_mut::<Singularity>(entity) else {
        return;
    };
    let old = singularity.level;
    if old == value {
        return;
    }
    singularity.level = value;
    update_level(world, entity, old);
}

/// Энергия (`SetEnergy`).
pub fn set_energy(world: &mut World, entity: Entity, value: f32) {
    let Some(mut singularity) = world.get_mut::<Singularity>(entity) else {
        return;
    };
    if singularity.energy == value {
        return;
    }
    singularity.energy = value;
    set_level(world, entity, level_for_energy(value));
}

/// Добавить энергию (`AdjustEnergy`).
pub fn adjust_energy(world: &mut World, entity: Entity, delta: f32) {
    if let Some(energy) = world.get::<Singularity>(entity).map(|s| s.energy) {
        set_energy(world, entity, energy + delta);
    }
}

// Горизонт событий.

/// Можно ли поглотить (`CanConsumeEntity`).
pub fn can_consume(world: &mut World, horizon: Entity, target: Entity) -> bool {
    let can_breach = world
        .get::<EventHorizon>(horizon)
        .is_some_and(|h| h.can_breach_containment);
    let mut event = AttemptConsume {
        horizon,
        can_breach_containment: can_breach,
        cancelled: false,
    };
    world.raise(target, &mut event);
    !event.cancelled
}

/// Поглотить (`ConsumeEntity`).
pub fn consume(world: &mut World, horizon: Entity, target: Entity) {
    // Уже поглощена в этой волне (`IsQueuedForDeletion`).
    if !world.is_alive(target) || world.get::<Consumed>(target).is_some() {
        return;
    }
    world.entity_mut(target).insert(Consumed);
    world.raise(horizon, &mut ConsumedEntity { entity: target });
    world.raise(target, &mut ConsumedBy { horizon });
    world.queue_delete(target);
}

fn attempt_consume(world: &mut World, horizon: Entity, target: Entity) -> bool {
    if !can_consume(world, horizon, target) {
        return false;
    }
    consume(world, horizon, target);
    true
}

fn layers(fixtures: &Fixtures) -> (i32, i32) {
    fixtures
        .fixtures
        .values()
        .filter(|f| f.hard)
        .fold((0, 0), |(l, m), f| (l | f.layer.0, m | f.mask.0))
}

/// Тело столкнулось бы с горизонтом (`IsHardCollidable`).
fn hard_collidable(world: &World, horizon: Entity, other: Entity) -> bool {
    let (Some(ours), Some(theirs)) = (world.get::<Fixtures>(horizon), world.get::<Fixtures>(other))
    else {
        return false;
    };
    let (layer, mask) = layers(ours);
    let (other_layer, other_mask) = layers(theirs);
    mask & other_layer != 0 || other_mask & layer != 0
}

/// Сущности в радиусе: тела по широкой фазе и закреплённые в клетках.
fn entities_in_range(world: &World, entity: Entity, range: f32) -> Vec<Entity> {
    let Some(at) = world.map_coordinates(entity) else {
        return Vec::new();
    };
    let Some(map) = world.map_entity(at.map) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut roots = vec![(map, at.position, at.z)];
    for grid in world
        .get::<Map>(map)
        .map(|m| m.grids().to_vec())
        .unwrap_or_default()
    {
        if let Some(placement) = world.world_transform(grid) {
            roots.push((
                grid,
                placement.to_local(at.position),
                at.z - placement.level,
            ));
        }
    }
    for (root, local, z) in roots {
        let bounds = Box2::centered(local, Vec2::splat(range * 2.0));
        found.extend(world.entities_intersecting(root, z, bounds));
        if let Some(grid) = world.get::<MapGrid>(root) {
            let r = range.ceil() as i32;
            let center = TileCoord::containing(local, z);
            for dx in -r..=r {
                for dy in -r..=r {
                    let cell = center.offset(astrocore_math::IVec2::new(dx, dy));
                    if (cell.center() - local).length() <= range + 0.5 {
                        found.extend(grid.anchored_at(cell).iter().copied());
                    }
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found.retain(|e| {
        *e != entity
            && world.containing_container(*e).is_none()
            && world
                .map_coordinates(*e)
                .is_some_and(|p| (p.position - at.position).length() <= range)
    });
    found
}

/// Поглотить всё в радиусе (`ConsumeEntitiesInRange`).
fn consume_entities_in_range(world: &mut World, horizon: Entity, range: f32) {
    for target in entities_in_range(world, horizon, range) {
        if world.get::<Physics>(target).is_some() && !hard_collidable(world, horizon, target) {
            continue;
        }
        attempt_consume(world, horizon, target);
    }
}

/// Тайлы в круге (`ConsumeTilesInRange`, `AttemptConsumeTiles`).
fn consume_tiles_in_range(world: &mut World, horizon: Entity, range: f32) {
    let Some(at) = world.map_coordinates(horizon) else {
        return;
    };
    let Some(map) = world.map_entity(at.map) else {
        return;
    };
    let grids = world
        .get::<Map>(map)
        .map(|m| m.grids().to_vec())
        .unwrap_or_default();
    let mut consumed = 0usize;
    for grid in grids {
        let Some(placement) = world.world_transform(grid) else {
            continue;
        };
        let local = placement.to_local(at.position);
        let center = TileCoord::containing(local, at.z - placement.level);
        let r = range.ceil() as i32 + 1;
        let mut cells = Vec::new();
        if let Some(map_grid) = world.get::<MapGrid>(grid) {
            for dx in -r..=r {
                for dy in -r..=r {
                    let cell = center.offset(astrocore_math::IVec2::new(dx, dy));
                    if map_grid.tile(cell).is_empty() {
                        continue;
                    }
                    // Клетка задевает круг, если ближайшая к центру её точка внутри.
                    let min = Vec2::new(cell.x as f32, cell.y as f32);
                    let nearest = local.clamp(min, min + Vec2::ONE);
                    if (nearest - local).length() < range {
                        cells.push(cell);
                    }
                }
            }
        }
        for cell in cells {
            let blockers: Vec<Entity> = world
                .get::<MapGrid>(grid)
                .map(|g| g.anchored_at(cell).to_vec())
                .unwrap_or_default();
            if blockers.into_iter().all(|e| can_consume(world, horizon, e)) {
                world.set_tile(grid, cell, Tile::default());
                consumed += 1;
            }
        }
    }
    if consumed > 0 && world.get::<Singularity>(horizon).is_some() {
        adjust_energy(world, horizon, consumed as f32);
    }
}

fn horizon_update(world: &mut World) {
    let time = now(world);
    let due: Vec<Entity> = world
        .query::<(Entity, &EventHorizon)>()
        .iter(world)
        .filter(|(_, h)| h.next_consume_wave_time <= time)
        .map(|(e, _)| e)
        .collect();
    for entity in due {
        let Some(horizon) = world.get::<EventHorizon>(entity).cloned() else {
            continue;
        };
        if let Some(mut h) = world.get_mut::<EventHorizon>(entity) {
            h.next_consume_wave_time = h.next_consume_wave_time.max(time - h.target_consume_period)
                + h.target_consume_period;
        }
        if horizon.being_consumed
            || world.containing_container(entity).is_some()
            || horizon.radius <= 0.0
        {
            continue;
        }
        if horizon.consume_entities {
            consume_entities_in_range(world, entity, horizon.radius);
        }
        if horizon.consume_tiles && world.is_alive(entity) {
            consume_tiles_in_range(world, entity, horizon.radius);
        }
    }
}

fn on_prevent_collide(world: &mut World, entity: Entity, event: &mut PreventCollide) {
    if event.cancelled {
        return;
    }
    let other = event.other_entity;
    if world.get::<MapGrid>(other).is_some() {
        event.cancelled = true;
        return;
    }
    let breach = world
        .get::<EventHorizon>(entity)
        .is_some_and(|h| h.can_breach_containment);
    if world.get::<ContainmentField>(other).is_some()
        || world.get::<ContainmentFieldGenerator>(other).is_some()
    {
        if breach {
            event.cancelled = true;
        }
        return;
    }
    let our_hard = world
        .get::<Fixtures>(entity)
        .and_then(|f| f.fixtures.get(&event.our_fixture))
        .is_some_and(|f| f.hard);
    event.cancelled = our_hard && can_consume(world, entity, other);
}

fn on_start_collide(world: &mut World, entity: Entity, event: &mut StartCollide) {
    let Some(horizon) = world.get::<EventHorizon>(entity).cloned() else {
        return;
    };
    if horizon.being_consumed
        || horizon.consumer_fixture_id.as_deref() != Some(event.our_fixture.as_str())
    {
        return;
    }
    attempt_consume(world, entity, event.other_entity);
}

// Гравитационный колодец.

/// Импульс к центру (`GravPulse`): тела в радиусе получают скорость к точке, обратно
/// пропорциональную квадрату расстояния.
pub fn grav_pulse(
    world: &mut World,
    well: Entity,
    max_range: f32,
    min_range: f32,
    radial: f32,
    tangential: f32,
) {
    let Some(at) = world.map_coordinates(well) else {
        return;
    };
    let min_range2 = (min_range * min_range).max(0.00001);
    for entity in entities_in_range(world, well, max_range) {
        let Some(body) = world.get::<Physics>(entity) else {
            continue;
        };
        if body.body_type == BodyType::Static
            || world.get::<GravityWell>(entity).is_some()
            || world.get::<MapGrid>(entity).is_some()
        {
            continue;
        }
        let velocity = body.linear_velocity;
        let Some(position) = world.map_coordinates(entity).map(|p| p.position) else {
            continue;
        };
        let displacement = at.position - position;
        let distance2 = displacement.length_squared();
        if distance2 < min_range2 {
            continue;
        }
        // Матрица (radial, -tangential; tangential, radial) поворачивает и тянет.
        let delta = Vec2::new(
            displacement.x * radial + displacement.y * tangential,
            -displacement.x * tangential + displacement.y * radial,
        ) / distance2;
        world.set_linear_velocity(entity, velocity + delta);
        world.wake_body(entity);
    }
}

fn gravity_well_update(world: &mut World) {
    let time = now(world);
    let due: Vec<(Entity, GravityWell)> = world
        .query::<(Entity, &GravityWell)>()
        .iter(world)
        .filter(|(_, w)| w.next_pulse_time <= time)
        .map(|(e, w)| (e, w.clone()))
        .collect();
    for (entity, well) in due {
        let last = well.next_pulse_time - well.target_pulse_period;
        let scale = (time - last).max(0.0) as f32;
        if let Some(mut w) = world.get_mut::<GravityWell>(entity) {
            w.next_pulse_time =
                w.next_pulse_time.max(time - w.target_pulse_period) + w.target_pulse_period;
        }
        if well.max_range < 0.0 {
            continue;
        }
        grav_pulse(
            world,
            entity,
            well.max_range,
            well.min_range,
            well.base_radial_acceleration * scale,
            well.base_tangential_acceleration * scale,
        );
    }
}

// Блуждание.

fn random_walk_step(world: &mut World, entity: Entity) {
    let Some(walk) = world.get::<RandomWalk>(entity).cloned() else {
        return;
    };
    let (cooldown, angle, strength) = {
        let mut rng = random(world);
        let cooldown = walk.min_step_cooldown
            + f64::from(rng.next_f32()) * (walk.max_step_cooldown - walk.min_step_cooldown);
        let angle = rng.next_f32() * std::f32::consts::TAU;
        let strength = walk.min_speed + rng.next_f32() * (walk.max_speed - walk.min_speed);
        (cooldown, angle, strength)
    };
    let time = now(world);
    if let Some(mut w) = world.get_mut::<RandomWalk>(entity) {
        w.next_step_time = time + cooldown;
    }
    let Some(velocity) = world.get::<Physics>(entity).map(|b| b.linear_velocity) else {
        return;
    };
    let push = Angle(angle).to_world_vec();
    world.set_linear_velocity(entity, velocity * walk.accumulator_ratio + push * strength);
}

fn random_walk_update(world: &mut World) {
    let time = now(world);
    let due: Vec<Entity> = world
        .query::<(Entity, &RandomWalk)>()
        .iter(world)
        .filter(|(_, w)| w.next_step_time <= time)
        .map(|(e, _)| e)
        .collect();
    for entity in due {
        if world.get::<crate::throwing::ThrownItem>(entity).is_some() {
            continue;
        }
        random_walk_step(world, entity);
    }
}

fn singularity_update(world: &mut World) {
    let dt = world.resource::<GameTime>().frame_time();
    let all: Vec<(Entity, f32)> = world
        .query::<(Entity, &Singularity)>()
        .iter(world)
        .map(|(e, s)| (e, s.energy_drain))
        .collect();
    for (entity, drain) in all {
        if drain != 0.0 {
            adjust_energy(world, entity, -drain * dt);
        }
    }
}

// Генератор сингулярности.

/// Мощность генератора (`SetPower`): на пороге — сингулярность.
pub fn set_generator_power(world: &mut World, generator: Entity, value: f32) {
    let Some(mut component) = world.get_mut::<SingularityGenerator>(generator) else {
        return;
    };
    let old = component.power;
    if old == value {
        return;
    }
    component.power = value;
    if !(value >= component.threshold && old < component.threshold) {
        return;
    }
    // `OnPassThreshold`.
    component.power = 0.0;
    component.inert = true;
    let prototype = component.spawn_prototype.clone();
    if let (Some(prototype), Some(at)) = (prototype, world.coordinates(generator)) {
        let _ = world.spawn_attached(&prototype, at);
    }
}

/// Поле есть по стороне (`CheckContainmentField`).
fn field_on_side(world: &World, generator: Entity, index: u8, distance: f32) -> bool {
    let (Some(origin), Some(placement)) = (
        world.map_coordinates(generator),
        world.world_transform(generator),
    ) else {
        return false;
    };
    let mask = CollisionGroups(group_bits("FullTileMask").unwrap_or(-1));
    let ray = Ray {
        origin,
        direction: (Direction::from_index(index).angle() + placement.rotation).to_world_vec(),
        max_distance: distance,
        mask,
    };
    world
        .intersect_ray(ray, &|e| e == generator, false)
        .iter()
        .find(|h| world.get::<ContainmentField>(h.entity).is_some())
        .is_some_and(|h| {
            world
                .get::<Physics>(h.entity)
                .is_some_and(|b| b.body_type == BodyType::Static)
        })
}

/// Частица попала в генератор (`HandleParticleCollide`).
pub fn particle_hit_generator(world: &mut World, particle: Entity, generator: Entity) {
    let Some(component) = world.get::<SingularityGenerator>(generator).cloned() else {
        return;
    };
    let time = now(world);
    if component.inert || (time < component.next_failsafe && !component.failsafe_disabled) {
        world.queue_delete(particle);
        return;
    }
    let contained = component.failsafe_disabled
        || (0..8u8)
            .step_by(2)
            .all(|i| field_on_side(world, generator, i, component.failsafe_distance));
    if !contained {
        if let Some(mut g) = world.get_mut::<SingularityGenerator>(generator) {
            g.next_failsafe = time + g.failsafe_cooldown;
        }
        let name = astrocore_loc::EntityNameWorldExt::entity_arg(world, generator);
        let text = loc(world, "comp-generator-failsafe", &[("target", name.into())]);
        world.popup_entity(&text, generator, PopupType::LargeCaution);
    } else {
        let state = world
            .get::<ParticleProjectile>(particle)
            .map(|p| p.state.clone())
            .unwrap_or_default();
        let gain = match state.as_str() {
            "Level0" => 1.0,
            "Level1" => 2.0,
            "Level2" => 4.0,
            "Level3" => 8.0,
            _ => 0.0,
        };
        set_generator_power(world, generator, component.power + gain);
    }
    world.queue_delete(particle);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Singularity>();
    world.register_data_component::<EventHorizon>();
    world.register_data_component::<GravityWell>();
    world.register_data_component::<RandomWalk>();
    world.register_data_component::<SinguloFood>();
    world.register_data_component::<SingularityGenerator>();
    world.register_data_component::<ParticleProjectile>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Singularity, MapInit>(|world, entity, _| {
        if let Some(sound) = world
            .get::<Singularity>(entity)
            .and_then(|s| s.formation_sound.clone())
        {
            world.play_entity(&sound, entity, None);
        }
        let level = world.get::<Singularity>(entity).map_or(1, |s| s.level);
        update_level(world, entity, level);
        if let Some(mut light) = world.get_mut::<PointLight>(entity) {
            light.enabled = true;
        }
    });
    world.subscribe::<EventHorizon, MapInit>(|world, entity, _| {
        let time = now(world);
        if let Some(mut h) = world.get_mut::<EventHorizon>(entity) {
            h.next_consume_wave_time = time;
        }
        update_horizon_fixture(world, entity);
    });
    world.subscribe::<GravityWell, MapInit>(|world, entity, _| {
        let time = now(world);
        if let Some(mut w) = world.get_mut::<GravityWell>(entity) {
            w.next_pulse_time = time + w.target_pulse_period;
        }
    });
    world.subscribe::<RandomWalk, MapInit>(|world, entity, _| {
        if world
            .get::<RandomWalk>(entity)
            .is_some_and(|w| w.step_on_startup)
        {
            random_walk_step(world, entity);
            return;
        }
        let cooldown = {
            let walk = world.get::<RandomWalk>(entity).cloned();
            let roll = random(world).next_f32();
            walk.map_or(2.0, |w| {
                w.min_step_cooldown + f64::from(roll) * (w.max_step_cooldown - w.min_step_cooldown)
            })
        };
        let time = now(world);
        if let Some(mut w) = world.get_mut::<RandomWalk>(entity) {
            w.next_step_time = time + cooldown;
        }
    });
    world.subscribe::<EventHorizon, PreventCollide>(on_prevent_collide);
    world.subscribe::<EventHorizon, StartCollide>(on_start_collide);
    // Что поглотить нельзя.
    world.subscribe::<MapGrid, AttemptConsume>(|_, _, event| event.cancelled = true);
    world.subscribe::<ContainmentField, AttemptConsume>(|_, _, event| {
        if !event.can_breach_containment {
            event.cancelled = true;
        }
    });
    world.subscribe::<ContainmentFieldGenerator, AttemptConsume>(|world, entity, event| {
        if !event.can_breach_containment
            && world
                .get::<ContainmentFieldGenerator>(entity)
                .is_some_and(|g| g.is_connected)
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<EventHorizon, AttemptConsume>(|world, entity, event| {
        if event.horizon == entity
            || world
                .get::<EventHorizon>(entity)
                .is_some_and(|h| h.being_consumed)
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<EventHorizon, ConsumedBy>(|world, entity, _| {
        if let Some(mut h) = world.get_mut::<EventHorizon>(entity) {
            h.being_consumed = true;
        }
    });
    // Энергия от поглощения.
    world.subscribe::<Singularity, ConsumedEntity>(|world, entity, event| {
        if world.get::<SinguloFood>(event.entity).is_none() {
            adjust_energy(world, entity, 1.0);
        }
    });
    world.subscribe::<Singularity, ConsumedBy>(|world, entity, event| {
        let energy = world.get::<Singularity>(entity).map_or(0.0, |s| s.energy);
        if world.get::<Singularity>(event.horizon).is_some() {
            adjust_energy(world, event.horizon, energy);
            set_energy(world, entity, 0.0);
        }
    });
    world.subscribe::<SinguloFood, ConsumedBy>(|world, entity, event| {
        let Some(food) = world.get::<SinguloFood>(entity).copied() else {
            return;
        };
        if let Some(energy) = world.get::<Singularity>(event.horizon).map(|s| s.energy) {
            adjust_energy(
                world,
                event.horizon,
                food.energy + energy * (food.energy_factor - 1.0),
            );
        }
    });
    world.subscribe::<ParticleProjectile, StartCollide>(|world, particle, event| {
        if world
            .get::<SingularityGenerator>(event.other_entity)
            .is_some()
        {
            particle_hit_generator(world, particle, event.other_entity);
        }
    });
    world.subscribe::<SingularityGenerator, crate::projectiles::HitByProjectile>(
        |world, generator, event| {
            if world.get::<ParticleProjectile>(event.projectile).is_some() {
                particle_hit_generator(world, event.projectile, generator);
            }
        },
    );
    world.add_game_systems(
        Update,
        (
            singularity_update,
            horizon_update,
            gravity_well_update,
            random_walk_update,
        ),
    );
}

#[cfg(test)]
mod tests;
