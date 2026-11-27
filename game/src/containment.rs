//! Поле сдерживания: генератор поля копит заряд от болтов излучателя, при достаточном заряде
//! находит лучом другой включённый генератор по четырём сторонам и ставит между ними поле;
//! без болтов заряд утекает, и поле пропадает. Поле отбрасывает тех, кто его коснулся.
//! Образец — `ContainmentFieldGeneratorSystem` и `ContainmentFieldSystem` SS14 @ 2f4a2aad.
//!
//! Что поле не пускает сингулярность, задаёт `singularity`.

use std::collections::BTreeMap;

use astrocore_ecs::{
    EntityTerminating, EventWorldExt, GameTime, GameWorldExt, LifecycleWorldExt, MapInit,
    RegistryWorldExt, Update,
};
use astrocore_light::PointLight;
use astrocore_map::{
    AnchorStateChanged, EntityCoordinates, SpawnWorldExt, Transform, TransformWorldExt,
};
use astrocore_math::{Angle, Direction, Vec2};
use astrocore_net::AppearanceWorldExt;
use astrocore_physics::{
    BodyType, CollisionGroups, Fixtures, Physics, Ray, RayWorldExt, StartCollide, group_bits,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::anchorable::AnchorAttempt;
use crate::examine::loc;
use crate::interaction::ActivateInWorld;
use crate::popups::{PopupType, PopupWorldExt};
use crate::projectiles::HitByProjectile;
use crate::tag::TagWorldExt;

/// Генератор поля (`ContainmentFieldGeneratorComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainmentFieldGenerator {
    /// Заряд от 0 до 25.
    #[serde(default)]
    pub power_buffer: i32,
    #[serde(default = "six")]
    pub power_minimum: i32,
    /// Заряд за болт.
    #[serde(default = "three", rename = "power")]
    pub power_received: i32,
    #[serde(default = "two")]
    pub power_loss: i32,
    #[serde(default)]
    pub accumulator: f32,
    /// Раз в сколько секунд без болтов теряется заряд.
    #[serde(default = "twenty")]
    pub threshold: f32,
    #[serde(default = "eight")]
    pub max_length: f32,
    #[serde(default = "bolt_tag", rename = "idTag")]
    pub id_tag: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub is_connected: bool,
    #[serde(default = "field")]
    pub created_field: String,
    /// Связи по сторонам (номер `Direction`): другой генератор и поля между ними.
    #[serde(skip)]
    pub connections: BTreeMap<u8, (Entity, Vec<Entity>)>,
}

fn six() -> i32 {
    6
}
fn three() -> i32 {
    3
}
fn two() -> i32 {
    2
}
fn twenty() -> f32 {
    20.0
}
fn eight() -> f32 {
    8.0
}
fn bolt_tag() -> String {
    "EmitterBolt".into()
}
fn field() -> String {
    "ContainmentField".into()
}

/// Поле (`ContainmentFieldComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainmentField {
    #[serde(default = "hundred")]
    pub throw_force: f32,
    #[serde(default = "max_mass")]
    pub max_mass: f32,
    #[serde(default = "yes")]
    pub destroy_garbage: bool,
}

fn hundred() -> f32 {
    100.0
}
fn max_mass() -> f32 {
    10000.0
}
fn yes() -> bool {
    true
}

/// Огонь заряда (`ContainmentFieldGeneratorVisuals.PowerLight`).
pub const POWER_LIGHT: &str = "enum.ContainmentFieldGeneratorVisuals.PowerLight";
/// Огонь поля (`ContainmentFieldGeneratorVisuals.FieldLight`).
pub const FIELD_LIGHT: &str = "enum.ContainmentFieldGeneratorVisuals.FieldLight";
/// Огонь связи (`ContainmentFieldGeneratorVisuals.OnLight`).
pub const ON_LIGHT: &str = "enum.ContainmentFieldGeneratorVisuals.OnLight";

/// Маска луча поиска генератора (`MobMask | Impassable | MachineMask | Opaque`).
fn collision_mask() -> CollisionGroups {
    CollisionGroups(
        ["MobMask", "Impassable", "MachineMask", "Opaque"]
            .iter()
            .filter_map(|name| group_bits(name))
            .fold(0, |mask, bits| mask | bits),
    )
}

fn generator(world: &World, entity: Entity) -> Option<ContainmentFieldGenerator> {
    world.get::<ContainmentFieldGenerator>(entity).cloned()
}

fn with_generator(
    world: &mut World,
    entity: Entity,
    f: impl FnOnce(&mut ContainmentFieldGenerator),
) {
    if let Some(mut component) = world.get_mut::<ContainmentFieldGenerator>(entity) {
        f(&mut component);
    }
}

// Вид.

fn power_visual(world: &mut World, entity: Entity) {
    let Some(component) = generator(world, entity) else {
        return;
    };
    let level = if component.power_buffer <= 0 {
        "NoPower"
    } else if component.power_buffer >= 25 {
        "HighPower"
    } else if component.power_buffer < component.power_minimum {
        "LowPower"
    } else {
        "MediumPower"
    };
    world.set_appearance(entity, POWER_LIGHT, level);
}

fn field_visual(world: &mut World, entity: Entity) {
    let Some(component) = generator(world, entity) else {
        return;
    };
    let level = match component.connections.len() {
        n if n > 1 => "MultipleFields",
        1 => "OneField",
        _ if component.enabled => "On",
        _ => "NoLevel",
    };
    world.set_appearance(entity, FIELD_LIGHT, level);
}

fn on_light_visual(world: &mut World, entity: Entity) {
    let Some(component) = generator(world, entity) else {
        return;
    };
    world.set_appearance(entity, ON_LIGHT, component.is_connected);
    if let Some(mut light) = world.get_mut::<PointLight>(entity) {
        light.enabled = !component.connections.is_empty();
    }
}

fn popup_at(world: &mut World, key: &str, entity: Entity) {
    let text = loc(world, key, &[]);
    world.popup_entity(&text, entity, PopupType::Small);
}

// Связи.

/// Снять связи генератора; `keep` оставляет связь с этим соседом (`RemoveConnections`).
fn remove_connections(world: &mut World, entity: Entity, keep: impl Fn(&World, Entity) -> bool) {
    let Some(component) = generator(world, entity) else {
        return;
    };
    let mut removed = false;
    for (direction, (other, fields)) in component.connections {
        if keep(world, other) {
            continue;
        }
        removed = true;
        for field in fields {
            world.queue_delete(field);
        }
        with_generator(world, entity, |g| {
            g.connections.remove(&direction);
        });
        let opposite = (direction + 4) % 8;
        with_generator(world, other, |g| {
            g.connections.remove(&opposite);
            if g.connections.is_empty() {
                g.is_connected = false;
            }
        });
        on_light_visual(world, other);
        field_visual(world, other);
    }
    if !removed {
        return;
    }
    popup_at(world, "comp-containment-disconnected", entity);
    with_generator(world, entity, |g| {
        if g.connections.is_empty() {
            g.is_connected = false;
        }
    });
    on_light_visual(world, entity);
    field_visual(world, entity);
}

/// Заряд от болта (`ReceivePower`): при достаточном заряде — поиск связей.
pub fn receive_power(world: &mut World, entity: Entity, power: i32) {
    with_generator(world, entity, |g| {
        g.power_buffer = (g.power_buffer + power).clamp(0, 25);
    });
    let Some(component) = generator(world, entity) else {
        return;
    };
    if component.power_buffer >= component.power_minimum {
        for index in (0..8u8).step_by(2) {
            if !generator(world, entity).is_some_and(|g| g.connections.contains_key(&index)) {
                try_generate_connection(world, entity, index);
            }
        }
    }
    power_visual(world, entity);
}

/// Потеря заряда (`LosePower`): связи с разряженными соседями пропадают.
pub fn lose_power(world: &mut World, entity: Entity, power: i32) {
    with_generator(world, entity, |g| {
        g.power_buffer = (g.power_buffer - power).clamp(0, 25);
    });
    let Some(component) = generator(world, entity) else {
        return;
    };
    if component.power_buffer < component.power_minimum && !component.connections.is_empty() {
        remove_connections(world, entity, |world, other| {
            generator(world, other).is_some_and(|g| g.power_buffer >= g.power_minimum)
        });
    }
    power_visual(world, entity);
}

/// Связь по стороне (`TryGenerateFieldConnection`).
fn try_generate_connection(world: &mut World, entity: Entity, index: u8) -> bool {
    let Some(component) = generator(world, entity) else {
        return false;
    };
    let anchored = world
        .get::<Transform>(entity)
        .is_some_and(Transform::anchored);
    if !component.enabled || !anchored {
        return false;
    }
    let (Some(origin), Some(placement)) =
        (world.map_coordinates(entity), world.world_transform(entity))
    else {
        return false;
    };
    let angle = Direction::from_index(index).angle() + placement.rotation;
    let ray = Ray {
        origin,
        direction: angle.to_world_vec(),
        max_distance: component.max_length,
        mask: collision_mask(),
    };
    let Some(hit) = world
        .intersect_ray(ray, &|e| e == entity, true)
        .first()
        .map(|h| h.entity)
    else {
        return false;
    };
    if hit == entity
        || world.get::<ContainmentFieldGenerator>(hit).is_none()
        || world
            .get::<Physics>(hit)
            .is_none_or(|b| b.body_type != BodyType::Static)
        || world.parent(hit) != world.parent(entity)
    {
        return false;
    }
    let fields = generate_fields(world, entity, hit);
    let opposite = (index + 4) % 8;
    with_generator(world, entity, |g| {
        g.connections.insert(index, (hit, fields.clone()));
    });
    with_generator(world, hit, |g| {
        g.connections.insert(opposite, (entity, fields));
    });
    field_visual(world, hit);
    for side in [entity, hit] {
        let newly = generator(world, side).is_some_and(|g| !g.is_connected);
        if newly {
            with_generator(world, side, |g| g.is_connected = true);
        }
        on_light_visual(world, side);
    }
    field_visual(world, entity);
    popup_at(world, "comp-containment-connected", entity);
    true
}

/// Поля между генераторами (`GenerateFieldConnection`).
fn generate_fields(world: &mut World, first: Entity, second: Entity) -> Vec<Entity> {
    let (Some(a), Some(b)) = (
        world.get::<Transform>(first),
        world.get::<Transform>(second),
    ) else {
        return Vec::new();
    };
    let Some(parent) = world.parent(first) else {
        return Vec::new();
    };
    let (start, end) = (a.local_position(), b.local_position());
    let prototype = generator(world, first).map_or_else(field, |g| g.created_field);
    let delta = end - start;
    let length = delta.length();
    if length <= 0.0 {
        return Vec::new();
    }
    let direction = delta / length;
    let horizontal = matches!(
        Direction::from_vec(direction),
        Direction::East | Direction::West
    );
    let mut fields = Vec::new();
    let mut offset = direction;
    while offset.length() < length {
        let rotation = if horizontal {
            Angle::from_world_vec(offset) + Angle::degrees(90.0)
        } else {
            Angle(0.0)
        };
        let at = EntityCoordinates::new(parent, start + offset);
        if let Ok(field) = world.spawn_attached(&prototype, at) {
            let _ = world.set_coordinates(field, at, Some(rotation));
            fields.push(field);
        }
        offset += direction;
    }
    fields
}

fn grid_check(world: &mut World, entity: Entity) {
    let parent = world.parent(entity);
    let mismatch = generator(world, entity).is_some_and(|g| {
        g.connections
            .values()
            .any(|(other, _)| world.parent(*other) != parent)
    });
    if mismatch {
        remove_connections(world, entity, |_, _| false);
    }
}

fn on_activate(world: &mut World, entity: Entity, event: &mut ActivateInWorld) {
    if event.handled {
        return;
    }
    let Some(component) = generator(world, entity) else {
        return;
    };
    if world
        .get::<Transform>(entity)
        .is_some_and(Transform::anchored)
    {
        if !component.enabled {
            with_generator(world, entity, |g| g.enabled = true);
            field_visual(world, entity);
            popup_at(world, "comp-containment-turned-on", entity);
        } else if component.is_connected {
            let text = loc(world, "comp-containment-toggle-warning", &[]);
            world.popup_entity_for(&text, event.user, event.user, PopupType::LargeCaution);
            return;
        } else {
            with_generator(world, entity, |g| g.enabled = false);
            field_visual(world, entity);
            popup_at(world, "comp-containment-turned-off", entity);
        }
    }
    event.handled = true;
}

fn update(world: &mut World) {
    let dt = world.resource::<GameTime>().frame_time();
    let charged: Vec<Entity> = world
        .query::<(Entity, &ContainmentFieldGenerator)>()
        .iter(world)
        .filter(|(_, g)| g.power_buffer > 0)
        .map(|(e, _)| e)
        .collect();
    for entity in charged {
        let mut lose = None;
        with_generator(world, entity, |g| {
            g.accumulator += dt;
            if g.accumulator >= g.threshold {
                g.accumulator -= g.threshold;
                lose = Some(g.power_loss);
            }
        });
        if let Some(power) = lose {
            lose_power(world, entity, power);
        }
    }
}

/// Поле отбрасывает лёгкие твёрдые тела (`HandleFieldCollide`).
fn on_field_collide(world: &mut World, field: Entity, event: &mut StartCollide) {
    let Some(component) = world.get::<ContainmentField>(field).cloned() else {
        return;
    };
    let other = event.other_entity;
    if component.destroy_garbage
        && crate::whitelist::has_component_named(world, other, "SpaceGarbage")
    {
        let name = astrocore_loc::EntityNameWorldExt::entity_name(world, other);
        let text = loc(world, "comp-field-vaporized", &[("entity", name.into())]);
        world.popup_entity(&text, field, PopupType::LargeCaution);
        world.queue_delete(other);
        return;
    }
    let hard = world
        .get::<Fixtures>(other)
        .is_some_and(|f| f.fixtures.values().any(|x| x.hard));
    let light = world
        .get::<Fixtures>(other)
        .is_some_and(|f| f.mass() <= component.max_mass);
    let movable = world
        .get::<Physics>(other)
        .is_some_and(|b| b.body_type != BodyType::Static);
    if !(hard && light && movable) {
        return;
    }
    let (Some(field_at), Some(other_at)) =
        (world.map_coordinates(field), world.map_coordinates(other))
    else {
        return;
    };
    let direction: Vec2 = other_at.position - field_at.position;
    crate::throwing::try_throw(world, other, direction, component.throw_force, None, true);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<ContainmentFieldGenerator>();
    world.register_data_component::<ContainmentField>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<ContainmentFieldGenerator, MapInit>(|world, entity, _| {
        if generator(world, entity).is_some_and(|g| g.enabled) {
            field_visual(world, entity);
        }
    });
    world.subscribe::<ContainmentFieldGenerator, HitByProjectile>(|world, entity, event| {
        let Some(component) = generator(world, entity) else {
            return;
        };
        if world.has_tag(event.projectile, &component.id_tag) {
            receive_power(world, entity, component.power_received);
            with_generator(world, entity, |g| g.accumulator = 0.0);
        }
    });
    world.subscribe::<ContainmentFieldGenerator, ActivateInWorld>(on_activate);
    world.subscribe::<ContainmentFieldGenerator, AnchorStateChanged>(|world, entity, event| {
        if event.anchored {
            grid_check(world, entity);
        } else {
            remove_connections(world, entity, |_, _| false);
        }
    });
    world.subscribe::<ContainmentFieldGenerator, AnchorAttempt>(|world, entity, event| {
        if event.anchoring || event.cancelled {
            return;
        }
        if generator(world, entity).is_some_and(|g| g.enabled || g.is_connected) {
            let text = loc(world, "comp-containment-anchor-warning", &[]);
            world.popup_entity_for(&text, event.user, event.user, PopupType::LargeCaution);
            event.cancelled = true;
        }
    });
    // `OnComponentRemoved`: связи уходят вместе с генератором.
    world.subscribe::<ContainmentFieldGenerator, EntityTerminating>(|world, entity, _| {
        remove_connections(world, entity, |_, _| false);
    });
    world.subscribe::<ContainmentField, StartCollide>(on_field_collide);
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
