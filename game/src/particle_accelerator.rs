//! Ускоритель частиц: пульт собирает машину из частей (силовой блок, топливная камера,
//! торец, три излучателя), берёт мощность через силовой блок и, пока включён и запитан,
//! раз в 6 секунд стреляет частицами из трёх излучателей. Частицы кормят генератор
//! сингулярности и саму сингулярность. Образец — `ParticleAcceleratorSystem`
//! (`ControlBox`, `Emitter`, `PowerBox`) SS14 @ 2f4a2aad.
//!
//! Нет пока: проводов пульта (блокировка окна и силы), тревоги администраторам.

use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, RegistryWorldExt, Update};
use astrocore_map::{SpawnWorldExt, TransformWorldExt};
use astrocore_net::AppearanceWorldExt;
use astrocore_physics::{BodyStatus, BodyType, Physics, PhysicsWorldExt};
use astrocore_ui::{BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::multipart::{
    AssemblyStateChanged, MultipartMachinePart, is_assembled, part_entity, rescan,
};
use crate::power::{
    ApcPowerReceiver, PowerChanged, PowerConsumer, PowerConsumerReceivedChanged, is_powered,
};
use crate::projectiles::Projectile;
use crate::singularity::{ParticleProjectile, SinguloFood};

/// Ключ окна пульта.
pub const UI_KEY: &str = "enum.ParticleAcceleratorControlBoxUiKey.Key";
/// Вид частей и частиц (`ParticleAcceleratorVisuals.VisualState`).
pub const VISUAL_STATE: &str = "enum.ParticleAcceleratorVisuals.VisualState";

pub const PART_END_CAP: &str = "enum.AcceleratorParts.EndCap";
pub const PART_FUEL_CHAMBER: &str = "enum.AcceleratorParts.FuelChamber";
pub const PART_POWER_BOX: &str = "enum.AcceleratorParts.PowerBox";
pub const PART_PORT: &str = "enum.AcceleratorParts.PortEmitter";
pub const PART_FORE: &str = "enum.AcceleratorParts.ForeEmitter";
pub const PART_STARBOARD: &str = "enum.AcceleratorParts.StarboardEmitter";

/// Сила ускорителя (`ParticleAcceleratorPowerState`): значения как в SS14, `Standby` = 1.
pub const STANDBY: u8 = 1;
pub const LEVEL3: u8 = 5;

/// Название силы (`ParticleAcceleratorPowerState`).
pub fn strength_name(strength: u8) -> &'static str {
    match strength {
        2 => "Level0",
        3 => "Level1",
        4 => "Level2",
        5 => "Level3",
        _ => "Standby",
    }
}

/// Вид по силе (`ParticleAcceleratorVisualState`): `Standby` выглядит как `Powered`.
fn visual_name(strength: u8) -> &'static str {
    match strength {
        2..=5 => strength_name(strength),
        _ => "Powered",
    }
}

/// Пульт (`ParticleAcceleratorControlBoxComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticleAcceleratorControlBox {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub powered: bool,
    #[serde(default)]
    pub firing: bool,
    #[serde(default = "standby")]
    pub selected_strength: u8,
    #[serde(default = "level2")]
    pub max_strength: u8,
    #[serde(default = "base_draw")]
    pub base_power_draw: f32,
    #[serde(default = "level_draw")]
    pub level_power_draw: f32,
    #[serde(default = "six")]
    pub charge_time: f64,
    #[serde(default)]
    pub interface_disabled: bool,
    #[serde(default)]
    pub strength_locked: bool,
    #[serde(skip)]
    pub next_fire: f64,
}

fn standby() -> u8 {
    STANDBY
}
fn level2() -> u8 {
    4
}
fn base_draw() -> f32 {
    500.0
}
fn level_draw() -> f32 {
    1500.0
}
fn six() -> f64 {
    6.0
}

/// Доля требуемой мощности (`RequiredPowerRatio`).
const REQUIRED_POWER_RATIO: f32 = 0.999;

/// Излучатель частиц (`ParticleAcceleratorEmitterComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticleAcceleratorEmitter {
    #[serde(default = "particles")]
    pub emitted_prototype: String,
    #[serde(default)]
    pub emitter_type: String,
}

fn particles() -> String {
    "ParticlesProjectile".into()
}

/// Силовой блок (`ParticleAcceleratorPowerBoxComponent`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ParticleAcceleratorPowerBox {}

/// Топливная камера (`ParticleAcceleratorFuelChamberComponent`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ParticleAcceleratorFuelChamber {}

/// Торец (`ParticleAcceleratorEndCapComponent`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ParticleAcceleratorEndCap {}

/// Окно пульта (`ParticleAcceleratorUIState`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticleAcceleratorUiState {
    pub assembled: bool,
    pub enabled: bool,
    pub state: u8,
    pub power_draw: i32,
    pub power_receive: i32,
    pub emitter_starboard: bool,
    pub emitter_fore: bool,
    pub emitter_port: bool,
    pub power_box: bool,
    pub fuel_chamber: bool,
    pub end_cap: bool,
    pub interface_block: bool,
    pub max_level: u8,
}

/// Включить или выключить (`ParticleAcceleratorSetEnableMessage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetEnable {
    pub enabled: bool,
}

/// Выбрать силу (`ParticleAcceleratorSetPowerStateMessage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetPowerState {
    pub state: u8,
}

/// Пересмотреть части (`ParticleAcceleratorRescanPartsMessage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RescanParts;

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

fn control(world: &World, entity: Entity) -> Option<ParticleAcceleratorControlBox> {
    world.get::<ParticleAcceleratorControlBox>(entity).cloned()
}

fn with_control(
    world: &mut World,
    entity: Entity,
    f: impl FnOnce(&mut ParticleAcceleratorControlBox),
) {
    if let Some(mut component) = world.get_mut::<ParticleAcceleratorControlBox>(entity) {
        f(&mut component);
    }
}

fn power_box_consumer(world: &World, entity: Entity) -> Option<Entity> {
    part_entity(world, entity, PART_POWER_BOX).filter(|b| world.get::<PowerConsumer>(*b).is_some())
}

/// Мощность силового блока (`UpdatePowerDraw`).
fn update_power_draw(world: &mut World, entity: Entity) {
    let Some(component) = control(world, entity) else {
        return;
    };
    let Some(power_box) = power_box_consumer(world, entity) else {
        return;
    };
    let mut draw = component.base_power_draw;
    if component.enabled {
        draw += component.level_power_draw * f32::from(component.selected_strength);
    }
    if let Some(mut consumer) = world.get_mut::<PowerConsumer>(power_box) {
        consumer.draw_rate = draw;
    }
}

/// Окно (`UpdateUI`).
pub fn ui_state(world: &World, entity: Entity) -> Option<ParticleAcceleratorUiState> {
    let component = control(world, entity)?;
    let (draw, receive) = power_box_consumer(world, entity)
        .and_then(|b| world.get::<PowerConsumer>(b))
        .map_or((0.0, 0.0), |c| (c.draw_rate, c.received_power));
    let has = |key: &str| part_entity(world, entity, key).is_some();
    Some(ParticleAcceleratorUiState {
        assembled: is_assembled(world, entity),
        enabled: component.enabled,
        state: component.selected_strength,
        power_draw: draw as i32,
        power_receive: receive as i32,
        emitter_starboard: has(PART_STARBOARD),
        emitter_fore: has(PART_FORE),
        emitter_port: has(PART_PORT),
        power_box: has(PART_POWER_BOX),
        fuel_chamber: has(PART_FUEL_CHAMBER),
        end_cap: has(PART_END_CAP),
        interface_block: component.interface_disabled,
        max_level: component.max_strength,
    })
}

fn update_ui(world: &mut World, entity: Entity) {
    if let Some(state) = ui_state(world, entity) {
        world.set_ui_state(entity, UI_KEY, &state);
    }
}

fn update_appearance(world: &mut World, entity: Entity) {
    let Some(component) = control(world, entity) else {
        return;
    };
    let unpowered = world.get::<ApcPowerReceiver>(entity).is_some() && !is_powered(world, entity);
    let state = if unpowered {
        "Unpowered"
    } else {
        visual_name(component.selected_strength)
    };
    world.set_appearance(entity, VISUAL_STATE, state);
}

fn update_part_visuals(world: &mut World, entity: Entity) {
    let Some(component) = control(world, entity) else {
        return;
    };
    let state = if component.powered {
        visual_name(component.selected_strength)
    } else {
        "Unpowered"
    };
    for key in [
        PART_FUEL_CHAMBER,
        PART_POWER_BOX,
        PART_PORT,
        PART_FORE,
        PART_STARBOARD,
    ] {
        if let Some(part) = part_entity(world, entity, key) {
            world.set_appearance(part, VISUAL_STATE, state);
        }
    }
}

fn update_firing(world: &mut World, entity: Entity) {
    let Some(component) = control(world, entity) else {
        return;
    };
    let time = now(world);
    with_control(world, entity, |c| {
        if !component.powered || component.selected_strength < 2 {
            c.firing = false;
        } else {
            c.next_fire = time + c.charge_time;
            c.firing = true;
        }
    });
}

/// Подать питание (`PowerOn`).
pub fn power_on(world: &mut World, entity: Entity) {
    if control(world, entity).is_none_or(|c| c.powered) {
        return;
    }
    with_control(world, entity, |c| c.powered = true);
    update_power_draw(world, entity);
    update_firing(world, entity);
    update_part_visuals(world, entity);
    update_ui(world, entity);
}

/// Снять питание (`PowerOff`).
pub fn power_off(world: &mut World, entity: Entity) {
    if control(world, entity).is_none_or(|c| !c.powered) {
        return;
    }
    with_control(world, entity, |c| c.powered = false);
    update_power_draw(world, entity);
    update_firing(world, entity);
    update_part_visuals(world, entity);
    update_ui(world, entity);
}

/// Включить (`SwitchOn`).
pub fn switch_on(world: &mut World, entity: Entity) {
    if control(world, entity).is_none_or(|c| c.enabled) || !is_assembled(world, entity) {
        return;
    }
    with_control(world, entity, |c| c.enabled = true);
    update_power_draw(world, entity);
    let enough = power_box_consumer(world, entity)
        .and_then(|b| world.get::<PowerConsumer>(b))
        .is_none_or(|c| c.received_power >= c.draw_rate * REQUIRED_POWER_RATIO);
    if enough {
        power_on(world, entity);
    }
    update_ui(world, entity);
}

/// Выключить (`SwitchOff`).
pub fn switch_off(world: &mut World, entity: Entity) {
    if control(world, entity).is_none_or(|c| !c.enabled) {
        return;
    }
    with_control(world, entity, |c| c.enabled = false);
    set_strength(world, entity, STANDBY);
    update_power_draw(world, entity);
    power_off(world, entity);
    update_ui(world, entity);
}

/// Сила (`SetStrength`).
pub fn set_strength(world: &mut World, entity: Entity, strength: u8) {
    let Some(component) = control(world, entity) else {
        return;
    };
    if component.strength_locked {
        return;
    }
    let strength = strength.clamp(STANDBY, component.max_strength);
    if strength == component.selected_strength {
        return;
    }
    with_control(world, entity, |c| c.selected_strength = strength);
    update_appearance(world, entity);
    update_part_visuals(world, entity);
    if component.enabled {
        update_power_draw(world, entity);
        update_firing(world, entity);
    }
}

/// Выстрел частицей (`FireEmitter`).
fn fire_emitter(world: &mut World, emitter: Entity, strength: u8) {
    let Some(prototype) = world
        .get::<ParticleAcceleratorEmitter>(emitter)
        .map(|e| e.emitted_prototype.clone())
    else {
        return;
    };
    let Some(at) = world.coordinates(emitter) else {
        return;
    };
    let Ok(particle) = world.spawn_attached(&prototype, at) else {
        return;
    };
    let local = world
        .get::<astrocore_map::Transform>(emitter)
        .map(|t| t.local_rotation())
        .unwrap_or_default();
    let _ = world.set_local_rotation(particle, local);
    let world_angle = world
        .world_transform(emitter)
        .map(|p| p.rotation)
        .unwrap_or_default();
    if world.get::<Physics>(particle).is_some() {
        if world
            .get::<Physics>(particle)
            .is_some_and(|b| b.body_type == BodyType::Static)
        {
            world.set_body_type(particle, BodyType::Dynamic);
        }
        world.set_body_status(particle, BodyStatus::InAir);
        let base = world
            .get::<Physics>(emitter)
            .map_or(astrocore_math::Vec2::ZERO, |b| b.linear_velocity);
        world.set_linear_velocity(particle, world_angle.to_world_vec() * 20.0 + base);
    }
    let time = now(world);
    if let Some(mut projectile) = world.get_mut::<Projectile>(particle) {
        projectile.shooter = Some(emitter);
        projectile.weapon = Some(emitter);
        projectile.when_to_stop_ignoring_shooter = time + projectile.delay_to_acknowledge_shooter;
    }
    if let Some(mut food) = world.get_mut::<SinguloFood>(particle) {
        food.energy = 10.0
            * match strength {
                2 => 1.0,
                3 => 2.0,
                4 => 3.0,
                5 => 6.0,
                _ => 0.0,
            };
    }
    if let Some(mut component) = world.get_mut::<ParticleProjectile>(particle) {
        component.state = strength_name(strength).into();
    }
    world.set_appearance(particle, VISUAL_STATE, strength_name(strength));
}

/// Залп (`Fire`).
pub fn fire(world: &mut World, entity: Entity) {
    let Some(component) = control(world, entity) else {
        return;
    };
    let time = now(world);
    with_control(world, entity, |c| c.next_fire = time + c.charge_time);
    for key in [PART_PORT, PART_FORE, PART_STARBOARD] {
        if let Some(emitter) = part_entity(world, entity, key) {
            fire_emitter(world, emitter, component.selected_strength);
        }
    }
}

fn update(world: &mut World) {
    let time = now(world);
    let due: Vec<Entity> = world
        .query::<(Entity, &ParticleAcceleratorControlBox)>()
        .iter(world)
        .filter(|(_, c)| c.firing && time >= c.next_fire)
        .map(|(e, _)| e)
        .collect();
    for entity in due {
        fire(world, entity);
    }
}

fn ui_allowed(world: &World, entity: Entity) -> bool {
    control(world, entity).is_some_and(|c| !c.interface_disabled)
        && (world.get::<ApcPowerReceiver>(entity).is_none() || is_powered(world, entity))
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<ParticleAcceleratorControlBox>();
    world.register_data_component::<ParticleAcceleratorEmitter>();
    world.register_data_component::<ParticleAcceleratorPowerBox>();
    world.register_data_component::<ParticleAcceleratorFuelChamber>();
    world.register_data_component::<ParticleAcceleratorEndCap>();
    world.register_ui_message::<SetEnable>();
    world.register_ui_message::<SetPowerState>();
    world.register_ui_message::<RescanParts>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<ParticleAcceleratorControlBox, PowerChanged>(|world, entity, event| {
        update_appearance(world, entity);
        if !event.powered {
            world.close_ui_for_all(entity, UI_KEY);
        }
    });
    world.subscribe::<ParticleAcceleratorControlBox, BoundUiMessage<SetEnable>>(
        |world, entity, event| {
            if !ui_allowed(world, entity) {
                return;
            }
            if event.message.enabled {
                switch_on(world, entity);
            } else {
                switch_off(world, entity);
            }
            update_ui(world, entity);
        },
    );
    world.subscribe::<ParticleAcceleratorControlBox, BoundUiMessage<SetPowerState>>(
        |world, entity, event| {
            if ui_allowed(world, entity) {
                set_strength(world, entity, event.message.state);
                update_ui(world, entity);
            }
        },
    );
    world.subscribe::<ParticleAcceleratorControlBox, BoundUiMessage<RescanParts>>(
        |world, entity, event| {
            if ui_allowed(world, entity) {
                rescan(world, entity, Some(event.actor));
                update_ui(world, entity);
            }
        },
    );
    world.subscribe::<ParticleAcceleratorControlBox, AssemblyStateChanged>(
        |world, entity, event| {
            if event.is_assembled {
                update_power_draw(world, entity);
            } else if control(world, entity).is_some_and(|c| c.powered) {
                switch_off(world, entity);
            } else {
                update_appearance(world, entity);
            }
            for (key, part) in &event.parts_removed {
                if key != PART_END_CAP {
                    world.set_appearance(*part, VISUAL_STATE, "Unpowered");
                }
            }
            update_ui(world, entity);
        },
    );
    // `PowerBoxReceivedChanged`.
    world.subscribe::<ParticleAcceleratorPowerBox, PowerConsumerReceivedChanged>(
        |world, power_box, event| {
            let Some(master) = world
                .get::<MultipartMachinePart>(power_box)
                .and_then(|p| p.master)
            else {
                return;
            };
            let Some(component) = control(world, master) else {
                return;
            };
            if component.enabled && event.received_power >= event.draw_rate * REQUIRED_POWER_RATIO {
                power_on(world, master);
            } else {
                power_off(world, master);
            }
            update_ui(world, master);
        },
    );
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
