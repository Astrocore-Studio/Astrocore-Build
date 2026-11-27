//! Гравитация: у грида или карты есть гравитация, пока на нём работает генератор; сущности
//! с `GravityAffected` без неё невесомы (движение — `astrocore_movement`). Генератор копит
//! заряд от питания и включается при полном заряде (`PowerCharge`). Образец —
//! `SharedGravitySystem`, `GravitySystem`, `GravityGeneratorSystem`,
//! `ServerPowerChargeSystem` и `MovementIgnoreGravitySystem` SS14 @ 2f4a2aad.
//!
//! Нет пока: тряски грида при смене гравитации, значка невесомости, магнитных ботинок,
//! запрета отвязать работающий генератор.

use std::collections::BTreeMap;

use astrocore_audio::SoundSpecifier;
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update};
use astrocore_light::PointLight;
use astrocore_map::{AnchorStateChanged, EntParentChanged, Transform, TransformWorldExt};
use astrocore_movement::GravityAffected;
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use astrocore_physics::{BodyType, Physics};
use astrocore_ui::{BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::power::{ApcPowerReceiver, is_powered};

/// Гравитация грида или карты (`GravityComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gravity {
    #[serde(default = "shake_sound")]
    pub gravity_shake_sound: SoundSpecifier,
    #[serde(default)]
    pub enabled: bool,
    /// Гравитация своя, генератор не нужен (планета).
    #[serde(default)]
    pub inherent: bool,
}

fn shake_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Effects/alert.ogg")
}

/// Сущность сама решает, невесома ли она (`MovementIgnoreGravityComponent`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovementIgnoreGravity {
    #[serde(default)]
    pub weightless: bool,
}

/// Генератор гравитации (`GravityGeneratorComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GravityGenerator {
    #[serde(default)]
    pub light_radius_min: f32,
    #[serde(default)]
    pub light_radius_max: f32,
    /// Рисунок основы по состоянию: `broken`, `unpowered`, `off`, `on`.
    #[serde(default)]
    pub sprite_map: BTreeMap<String, String>,
    #[serde(default = "startup")]
    pub core_startup_state: String,
    #[serde(default = "idle")]
    pub core_idle_state: String,
    #[serde(default = "activating")]
    pub core_activating_state: String,
    #[serde(default = "activated")]
    pub core_activated_state: String,
    #[serde(default)]
    pub gravity_active: bool,
}

fn startup() -> String {
    "startup".into()
}
fn idle() -> String {
    "idle".into()
}
fn activating() -> String {
    "activating".into()
}
fn activated() -> String {
    "activated".into()
}

/// Машина, которая копит заряд от питания (`PowerChargeComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerCharge {
    #[serde(default)]
    pub window_title: String,
    /// Доля заряда за секунду.
    #[serde(default = "rate")]
    pub charge_rate: f32,
    #[serde(default, rename = "idlePower")]
    pub idle_power_use: f32,
    #[serde(default, rename = "activePower")]
    pub active_power_use: f32,
    #[serde(default = "yes")]
    pub intact: bool,
    #[serde(default = "yes")]
    pub switched_on: bool,
    #[serde(default)]
    pub active: bool,
    #[serde(default = "full")]
    pub max_charge: f32,
    #[serde(default = "full")]
    pub charge: f32,
    #[serde(skip)]
    pub need_ui_update: bool,
}

fn rate() -> f32 {
    0.01
}
fn yes() -> bool {
    true
}
fn full() -> f32 {
    1.0
}

/// Ключ окна машины с зарядом (`PowerChargeUiKey.Key`).
pub const POWER_CHARGE_UI_KEY: &str = "enum.PowerChargeUiKey.Key";
/// Состояние машины (`PowerChargeVisuals.State`): `Broken`, `Unpowered`, `Off`, `On`.
pub const CHARGE_STATE: &str = "enum.PowerChargeVisuals.State";
/// Заряд от 0 до 1 (`PowerChargeVisuals.Charge`).
pub const CHARGE_VALUE: &str = "enum.PowerChargeVisuals.Charge";
/// Машина работает (`PowerChargeVisuals.Active`).
pub const CHARGE_ACTIVE: &str = "enum.PowerChargeVisuals.Active";

/// Что делает заряд (`PowerChargePowerStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerChargeStatus {
    Off,
    Discharging,
    Charging,
    FullyCharged,
}

/// Окно машины (`PowerChargeState`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerChargeUiState {
    pub on: bool,
    /// Заряд от 0 до 255.
    pub charge: u8,
    pub status: PowerChargeStatus,
    pub power_draw: i32,
    pub power_draw_max: i32,
    /// Секунд до цели; отрицательное — уже у цели.
    pub eta_seconds: i32,
    pub title: String,
}

/// Включить или выключить машину (`SwitchChargingMachineMessage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchChargingMachine {
    pub on: bool,
}

/// Гравитация грида сменилась (`GravityChangedEvent`). Широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct GravityChanged {
    pub grid: Entity,
    pub has_gravity: bool,
}

// Невесомость.

/// Есть ли гравитация у грида или карты сущности (`EntityGridOrMapHaveGravity`).
pub fn has_gravity(world: &World, entity: Entity) -> bool {
    let Some(transform) = world.get::<Transform>(entity) else {
        return true;
    };
    // Вне карты невесомость не включается.
    let Some(map) = transform.map() else {
        return true;
    };
    let enabled = |e: Entity| world.get::<Gravity>(e).is_some_and(|g| g.enabled);
    transform.grid().is_some_and(enabled) || enabled(map)
}

/// Невесомость и её источник (`GetWeightless`).
fn weightless_of(world: &World, entity: Entity) -> (bool, bool) {
    let Some(body) = world.get::<Physics>(entity) else {
        return (false, false);
    };
    if matches!(body.body_type, BodyType::Static | BodyType::Kinematic) {
        return (false, false);
    }
    if let Some(ignore) = world.get::<MovementIgnoreGravity>(entity) {
        return (ignore.weightless, false);
    }
    (!has_gravity(world, entity), true)
}

/// Пересчитать невесомость сущности (`RefreshWeightless`).
pub fn refresh_weightless(world: &mut World, entity: Entity) {
    let (weightless, from_grid) = weightless_of(world, entity);
    if let Some(mut affected) = world.get_mut::<GravityAffected>(entity)
        && (affected.weightless != weightless || affected.grid_weightless_status != from_grid)
    {
        affected.weightless = weightless;
        affected.grid_weightless_status = from_grid;
    }
}

/// Невесома ли сущность (`IsWeightless`).
pub fn is_weightless(world: &World, entity: Entity) -> bool {
    world
        .get::<GravityAffected>(entity)
        .is_some_and(|g| g.weightless)
}

fn set_gravity(world: &mut World, grid: Entity, enabled: bool) {
    let Some(mut gravity) = world.get_mut::<Gravity>(grid) else {
        return;
    };
    if gravity.enabled == enabled {
        return;
    }
    gravity.enabled = enabled;
    world.raise_broadcast(&mut GravityChanged {
        grid,
        has_gravity: enabled,
    });
    let affected: Vec<Entity> = world
        .query::<(Entity, &GravityAffected, &Transform)>()
        .iter(world)
        .filter(|(_, _, t)| t.grid() == Some(grid) || t.map() == Some(grid))
        .map(|(e, _, _)| e)
        .collect();
    for entity in affected {
        refresh_weightless(world, entity);
    }
}

/// Пересчитать гравитацию грида по генераторам на нём (`RefreshGravity`).
pub fn refresh_gravity(world: &mut World, grid: Entity) {
    if world.get::<Gravity>(grid).is_none_or(|g| g.inherent) {
        return;
    }
    let active: Vec<Entity> = world
        .query::<(Entity, &GravityGenerator)>()
        .iter(world)
        .filter(|(_, g)| g.gravity_active)
        .map(|(e, _)| e)
        .collect();
    let enabled = active.into_iter().any(|g| world.parent(g) == Some(grid));
    set_gravity(world, grid, enabled);
}

/// Включить гравитацию грида (`EnableGravity`).
pub fn enable_gravity(world: &mut World, grid: Entity) {
    if world.get::<Gravity>(grid).is_some_and(|g| !g.inherent) {
        set_gravity(world, grid, true);
    }
}

// Заряд машины.

fn on_activated(world: &mut World, machine: Entity, active: bool) {
    let Some(mut generator) = world.get_mut::<GravityGenerator>(machine) else {
        return;
    };
    generator.gravity_active = active;
    let Some(parent) = world.parent(machine) else {
        return;
    };
    if active {
        enable_gravity(world, parent);
    } else {
        refresh_gravity(world, parent);
    }
}

fn charge_status(world: &World, machine: Entity) -> &'static str {
    let (Some(charge), Some(receiver)) = (
        world.get::<PowerCharge>(machine),
        world.get::<ApcPowerReceiver>(machine),
    ) else {
        return "Unpowered";
    };
    if !charge.intact {
        "Broken"
    } else if !is_powered(world, machine) || receiver.received_power < charge.idle_power_use {
        "Unpowered"
    } else if !charge.switched_on {
        "Off"
    } else {
        "On"
    }
}

/// Вид машины (`UpdateState`).
fn update_state(world: &mut World, machine: Entity) {
    let Some(charge) = world.get::<PowerCharge>(machine).cloned() else {
        return;
    };
    world.set_appearance(machine, CHARGE_VALUE, charge.charge);
    world.set_appearance(machine, CHARGE_ACTIVE, charge.active);
    let status = charge_status(world, machine);
    world.set_appearance(machine, CHARGE_STATE, status);
}

/// Мощность машины по выключателю (`UpdatePowerState`).
fn update_power_state(world: &mut World, machine: Entity) {
    let Some(charge) = world.get::<PowerCharge>(machine).cloned() else {
        return;
    };
    if let Some(mut receiver) = world.get_mut::<ApcPowerReceiver>(machine) {
        receiver.power_load = if charge.switched_on {
            charge.active_power_use
        } else {
            charge.idle_power_use
        };
    }
}

/// Состояние окна (`UpdateUI`).
pub fn ui_state(world: &World, machine: Entity, charge_rate: f32) -> Option<PowerChargeUiState> {
    let charge = world.get::<PowerCharge>(machine)?;
    let receiver = world.get::<ApcPowerReceiver>(machine)?;
    let target = if charge_rate < 0.0 {
        0.0
    } else {
        charge.max_charge
    };
    let at_target = (charge.charge - target).abs() < 1e-4;
    let eta_seconds = if at_target || charge_rate == 0.0 {
        -1
    } else {
        ((target - charge.charge) / charge_rate).abs() as i32
    };
    let status = match (charge_rate > 0.0, at_target) {
        (true, true) => PowerChargeStatus::FullyCharged,
        (false, true) => PowerChargeStatus::Off,
        (true, false) => PowerChargeStatus::Charging,
        (false, false) => PowerChargeStatus::Discharging,
    };
    Some(PowerChargeUiState {
        on: charge.switched_on,
        charge: (charge.charge.clamp(0.0, 1.0) * 255.0) as u8,
        status,
        power_draw: receiver.received_power.round() as i32,
        power_draw_max: receiver.power_load.round() as i32,
        eta_seconds,
        title: charge.window_title.clone(),
    })
}

/// Скорость заряда сейчас (`Update`): отрицательная — разряд.
fn charge_rate(world: &World, machine: Entity, charge: &PowerCharge) -> f32 {
    if !charge.switched_on {
        return -charge.charge_rate;
    }
    if is_powered(world, machine) {
        return charge.charge_rate;
    }
    // При четверти мощности разряд идёт со скоростью трёх четвертей.
    let receiving = world
        .get::<ApcPowerReceiver>(machine)
        .map_or(0.0, |r| r.received_power);
    let main = (receiving - charge.idle_power_use).max(0.0);
    let span = (charge.active_power_use - charge.idle_power_use).max(f32::EPSILON);
    -((1.0 - main / span) * charge.charge_rate)
}

fn update_charges(world: &mut World) {
    let dt = world.resource::<GameTime>().frame_time();
    let machines: Vec<Entity> = world
        .query::<(Entity, &PowerCharge)>()
        .iter(world)
        .filter(|(_, c)| c.intact)
        .map(|(e, _)| e)
        .collect();
    for machine in machines {
        let Some(mut charge) = world.get::<PowerCharge>(machine).cloned() else {
            continue;
        };
        if world.get::<ApcPowerReceiver>(machine).is_none() {
            continue;
        }
        let rate = charge_rate(world, machine, &charge);
        let was_active = charge.active;
        let last = charge.charge;
        charge.charge = (charge.charge + dt * rate).clamp(0.0, charge.max_charge);
        if rate > 0.0 {
            if (charge.charge - charge.max_charge).abs() < 1e-4 && !charge.active {
                charge.active = true;
            }
        } else if charge.charge.abs() < 1e-4 && charge.active {
            charge.active = false;
        }
        let mut update_ui = charge.need_ui_update;
        charge.need_ui_update = false;
        let changed = (last - charge.charge).abs() > 1e-6;
        let active = charge.active;
        world.entity_mut(machine).insert(charge);
        if changed {
            update_state(world, machine);
            update_ui = true;
        }
        if update_ui && let Some(state) = ui_state(world, machine, rate) {
            world.set_ui_state(machine, POWER_CHARGE_UI_KEY, &state);
        }
        if active != was_active {
            on_activated(world, machine, active);
            update_state(world, machine);
        }
        // Свет ядра генератора (`GravityGeneratorSystem.Update`).
        if let Some(generator) = world.get::<GravityGenerator>(machine).cloned() {
            let value = world.get::<PowerCharge>(machine).map_or(0.0, |c| c.charge);
            if let Some(mut light) = world.get_mut::<PointLight>(machine) {
                light.enabled = value > 0.0;
                light.radius = generator.light_radius_min
                    + (generator.light_radius_max - generator.light_radius_min) * value;
            }
        }
    }
}

/// Включить или выключить машину (`SetSwitchedOn`).
pub fn set_switched_on(world: &mut World, machine: Entity, on: bool) {
    let Some(mut charge) = world.get_mut::<PowerCharge>(machine) else {
        return;
    };
    charge.switched_on = on;
    charge.need_ui_update = true;
    update_power_state(world, machine);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Gravity>();
    world.register_data_component::<GravityGenerator>();
    world.register_data_component::<PowerCharge>();
    world.register_data_component::<MovementIgnoreGravity>();
    world.register_net_serde::<Gravity>();
    world.register_ui_message::<SwitchChargingMachine>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<GravityAffected, MapInit>(|world, entity, _| {
        refresh_weightless(world, entity)
    });
    world.subscribe::<GravityAffected, EntParentChanged>(|world, entity, _| {
        refresh_weightless(world, entity)
    });
    world.subscribe::<MovementIgnoreGravity, MapInit>(|world, entity, _| {
        if world.get::<GravityAffected>(entity).is_none() {
            world.entity_mut(entity).insert(GravityAffected::default());
        }
        refresh_weightless(world, entity);
    });
    world.subscribe::<Gravity, MapInit>(|world, grid, _| refresh_gravity(world, grid));
    world.subscribe::<PowerCharge, MapInit>(|world, machine, _| {
        update_power_state(world, machine);
        update_state(world, machine);
    });
    world.subscribe::<PowerCharge, AnchorStateChanged>(|world, machine, event| {
        if event.anchored {
            return;
        }
        if let Some(mut charge) = world.get_mut::<PowerCharge>(machine) {
            charge.active = false;
            charge.charge = 0.0;
        }
        update_state(world, machine);
    });
    world.subscribe::<PowerCharge, BoundUiMessage<SwitchChargingMachine>>(
        |world, machine, event| {
            set_switched_on(world, machine, event.message.on);
        },
    );
    world.subscribe::<GravityGenerator, EntParentChanged>(|world, generator, event| {
        if world
            .get::<GravityGenerator>(generator)
            .is_some_and(|g| g.gravity_active)
            && let Some(old) = event.old_parent
        {
            refresh_gravity(world, old);
        }
    });
    world.add_game_systems(Update, update_charges);
}

#[cfg(test)]
mod tests;
