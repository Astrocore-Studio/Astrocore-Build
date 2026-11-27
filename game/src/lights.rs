//! Светильники с лампами: лампа в гнезде светит, пока на светильник идёт питание; лампу
//! вставляют рукой, вынимают рукой (рабочую — с ожиданием), бьют уроном по светильнику или
//! броском. Образец — `SharedPoweredLightSystem`, `PoweredLightSystem`, `SharedLightBulbSystem`
//! и `LitOnPoweredSystem` SS14 @ 2f4a2aad.
//!
//! Нет пока: сигналов включения (связь устройств, 5.6б), жжения руки о горящую лампу,
//! гула и мигания по событиям станции.

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, GameTime, MapInit, RegistryWorldExt};
use astrocore_light::PointLight;
use astrocore_map::{
    ContainerKind, ContainerWorldExt, EntInsertedIntoContainer, EntRemovedFromContainer,
    SpawnWorldExt, TransformWorldExt,
};
use astrocore_math::Color;
use astrocore_net::AppearanceWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::damage::DamageChanged;
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterWorldExt};
use crate::hands::HandsWorldExt;
use crate::interaction::{InteractHand, InteractUsing};
use crate::power::{ApcPowerReceiver, PowerChanged, is_powered};
use crate::throwing::Land;

/// Гнездо лампы (`LightBulbContainer`).
pub const BULB_CONTAINER: &str = "light_bulb";
/// Состояние лампы светильника (`PoweredLightVisuals.BulbState`): `Empty`, `Off`, `On`,
/// `Broken`, `Burned`.
pub const BULB_STATE: &str = "enum.PoweredLightVisuals.BulbState";
/// Состояние лампы-предмета (`LightBulbVisuals.State`): `Normal`, `Broken`, `Burned`.
pub const LAMP_STATE: &str = "enum.LightBulbVisuals.State";
/// Цвет лампы-предмета (`LightBulbVisuals.Color`), строка `#rrggbbaa`.
pub const LAMP_COLOR: &str = "enum.LightBulbVisuals.Color";
/// Пауза между щелчками включения (`ThunkDelay`).
const THUNK_DELAY: f64 = 2.0;

/// Светильник (`PoweredLightComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoweredLight {
    /// Какие лампы подходят: `Tube` или `Bulb`.
    #[serde(default = "tube", rename = "bulb")]
    pub bulb_type: String,
    #[serde(default = "yes")]
    pub on: bool,
    #[serde(default)]
    pub has_lamp_on_spawn: Option<String>,
    #[serde(default = "two")]
    pub eject_bulb_delay: f64,
    #[serde(default = "turn_on_sound")]
    pub turn_on_sound: SoundSpecifier,
    #[serde(skip)]
    pub current_lit: bool,
    #[serde(skip)]
    pub last_thunk: f64,
}

/// Лампа (`LightBulbComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightBulb {
    #[serde(default = "white")]
    pub color: Color,
    #[serde(default = "tube", rename = "bulb")]
    pub bulb_type: String,
    /// `Normal`, `Broken` или `Burned`.
    #[serde(default = "normal", rename = "startingState")]
    pub state: String,
    #[serde(default = "energy")]
    pub light_energy: f32,
    #[serde(default = "radius")]
    pub light_radius: f32,
    #[serde(default = "one")]
    pub light_softness: f32,
    #[serde(default = "sixty", rename = "PowerUse")]
    pub power_use: f32,
    #[serde(default = "break_sound")]
    pub break_sound: SoundSpecifier,
    #[serde(default = "normal_state")]
    pub normal_sprite_state: String,
    #[serde(default = "broken_state")]
    pub broken_sprite_state: String,
    #[serde(default = "burned_state")]
    pub burned_sprite_state: String,
}

fn normal_state() -> String {
    "normal".into()
}
fn broken_state() -> String {
    "broken".into()
}
fn burned_state() -> String {
    "burned".into()
}

/// Свет горит, пока сущность запитана (`LitOnPoweredComponent`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LitOnPowered {}

fn tube() -> String {
    "Tube".into()
}
fn normal() -> String {
    "Normal".into()
}
fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}
fn two() -> f64 {
    2.0
}
fn energy() -> f32 {
    0.8
}
fn radius() -> f32 {
    10.0
}
fn sixty() -> f32 {
    60.0
}
fn white() -> Color {
    Color::default()
}
fn turn_on_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/light_tube_on.ogg")
}
fn break_sound() -> SoundSpecifier {
    SoundSpecifier::collection("GlassBreak")
}

/// Вынуть рабочую лампу (`PoweredLightDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
pub struct EjectBulbDoAfter;

/// Лампа в гнезде.
pub fn bulb_of(world: &World, light: Entity) -> Option<Entity> {
    world
        .container(light, BULB_CONTAINER)
        .and_then(|c| c.entities().first().copied())
}

fn lamp_appearance(world: &mut World, bulb: Entity) {
    let Some(lamp) = world.get::<LightBulb>(bulb).cloned() else {
        return;
    };
    world.set_appearance(bulb, LAMP_STATE, lamp.state.clone());
    world.set_appearance(bulb, LAMP_COLOR, lamp.color.to_hex());
}

/// Состояние лампы (`SetState`).
pub fn set_bulb_state(world: &mut World, bulb: Entity, state: &str) {
    let Some(mut lamp) = world.get_mut::<LightBulb>(bulb) else {
        return;
    };
    if lamp.state == state {
        return;
    }
    lamp.state = state.to_owned();
    lamp_appearance(world, bulb);
}

fn set_light(world: &mut World, entity: Entity, value: bool, lamp: Option<&LightBulb>) {
    if let Some(mut light) = world.get_mut::<PoweredLight>(entity) {
        light.current_lit = value;
    }
    if let Some(mut point) = world.get_mut::<PointLight>(entity) {
        point.enabled = value;
        if let Some(lamp) = lamp {
            point.color = lamp.color;
            point.radius = lamp.light_radius;
            point.energy = lamp.light_energy;
            point.softness = lamp.light_softness;
        }
    }
}

fn set_load(world: &mut World, entity: Entity, load: f32) {
    if let Some(mut receiver) = world.get_mut::<ApcPowerReceiver>(entity)
        && receiver.power_load != load
    {
        receiver.power_load = load;
    }
}

/// Пересчитать свет светильника (`UpdateLight`).
pub fn update_light(world: &mut World, entity: Entity) {
    let Some(light) = world.get::<PoweredLight>(entity).cloned() else {
        return;
    };
    if world.get::<ApcPowerReceiver>(entity).is_none() {
        return;
    }
    let Some(lamp) = bulb_of(world, entity).and_then(|b| world.get::<LightBulb>(b).cloned()) else {
        set_light(world, entity, false, None);
        set_load(world, entity, 0.0);
        world.set_appearance(entity, BULB_STATE, "Empty");
        return;
    };
    match lamp.state.as_str() {
        "Normal" => {
            if is_powered(world, entity) && light.on {
                set_light(world, entity, true, Some(&lamp));
                world.set_appearance(entity, BULB_STATE, "On");
                let time = world.resource::<GameTime>().cur_time().as_secs_f64();
                if time > light.last_thunk + THUNK_DELAY {
                    if let Some(mut light) = world.get_mut::<PoweredLight>(entity) {
                        light.last_thunk = time;
                    }
                    world.play_entity(&light.turn_on_sound, entity, None);
                }
            } else {
                set_light(world, entity, false, None);
                world.set_appearance(entity, BULB_STATE, "Off");
            }
        }
        state => {
            set_light(world, entity, false, None);
            world.set_appearance(entity, BULB_STATE, state);
        }
    }
    let load = if light.on && lamp.state == "Normal" {
        lamp.power_use
    } else {
        0.0
    };
    set_load(world, entity, load);
}

/// Вставить лампу (`InsertBulb`).
pub fn insert_bulb(world: &mut World, light: Entity, bulb: Entity) -> bool {
    let Some(kind) = world
        .get::<PoweredLight>(light)
        .map(|l| l.bulb_type.clone())
    else {
        return false;
    };
    if bulb_of(world, light).is_some() {
        return false;
    }
    if world
        .get::<LightBulb>(bulb)
        .is_none_or(|l| l.bulb_type != kind)
    {
        return false;
    }
    world.ensure_container(light, BULB_CONTAINER, ContainerKind::Slot);
    world
        .insert_into_container(bulb, light, BULB_CONTAINER)
        .is_ok()
}

/// Вынуть лампу в руку или на пол (`EjectBulb`).
pub fn eject_bulb(world: &mut World, light: Entity, user: Option<Entity>) -> Option<Entity> {
    let bulb = bulb_of(world, light)?;
    world.remove_from_container(bulb, None).ok()?;
    world.pickup_or_drop(user, bulb);
    Some(bulb)
}

/// Разбить лампу в светильнике (`TryDestroyBulb`).
pub fn try_destroy_bulb(world: &mut World, light: Entity) -> bool {
    let Some(bulb) = bulb_of(world, light) else {
        return false;
    };
    let Some(lamp) = world.get::<LightBulb>(bulb).cloned() else {
        return false;
    };
    if lamp.state == "Broken" {
        return false;
    }
    set_bulb_state(world, bulb, "Broken");
    world.play_entity(&lamp.break_sound, bulb, None);
    update_light(world, light);
    true
}

/// Входы светильника (`OnPort`, `OffPort`, `TogglePort`).
pub const ON_PORT: &str = "On";
pub const OFF_PORT: &str = "Off";
pub const TOGGLE_PORT: &str = "Toggle";

/// Включить или выключить светильник (`SetState`).
pub fn set_light_on(world: &mut World, light: Entity, on: bool) {
    if let Some(mut component) = world.get_mut::<PoweredLight>(light) {
        component.on = on;
    }
    update_light(world, light);
}

fn on_map_init(world: &mut World, light: Entity) {
    world.ensure_container(light, BULB_CONTAINER, ContainerKind::Slot);
    if let Some(lamp) = world
        .get::<PoweredLight>(light)
        .and_then(|l| l.has_lamp_on_spawn.clone())
        && bulb_of(world, light).is_none()
        && let Some(at) = world.coordinates(light)
        && let Ok(bulb) = world.spawn_attached(&lamp, at)
    {
        let _ = world.insert_into_container(bulb, light, BULB_CONTAINER);
    }
    update_light(world, light);
}

fn on_interact_hand(world: &mut World, light: Entity, event: &mut InteractHand) {
    if event.handled {
        return;
    }
    let Some(bulb) = bulb_of(world, light) else {
        return;
    };
    // Разбитую или перегоревшую лампу вынимают сразу.
    if world
        .get::<LightBulb>(bulb)
        .is_some_and(|l| l.state != "Normal")
    {
        event.handled = eject_bulb(world, light, Some(event.user)).is_some();
        return;
    }
    let delay = world
        .get::<PoweredLight>(light)
        .map_or(2.0, |l| l.eject_bulb_delay);
    let mut args = DoAfterArgs::new(event.user, delay, Some(light), None, Some(light));
    args.break_on_move = true;
    args.break_on_damage = true;
    world.try_start_do_after(args, EjectBulbDoAfter);
    event.handled = true;
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<PoweredLight>();
    world.register_data_component::<LightBulb>();
    world.register_data_component::<LitOnPowered>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<PoweredLight, MapInit>(|world, light, _| on_map_init(world, light));
    // Сигналы: включить, выключить, переключить (`OnSignalReceived`).
    world.subscribe::<PoweredLight, astrocore_ecs::ComponentInit>(|world, light, _| {
        crate::device_linking::ensure_sink_ports(world, light, &[ON_PORT, OFF_PORT, TOGGLE_PORT]);
    });
    world.subscribe::<PoweredLight, crate::device_linking::SignalReceived>(
        |world, light, event| match event.port.as_str() {
            OFF_PORT => set_light_on(world, light, false),
            ON_PORT => set_light_on(world, light, true),
            TOGGLE_PORT => {
                let on = world.get::<PoweredLight>(light).is_some_and(|l| l.on);
                set_light_on(world, light, !on);
            }
            _ => {}
        },
    );
    world.subscribe::<PoweredLight, EntInsertedIntoContainer>(|world, light, event| {
        if &*event.container == BULB_CONTAINER {
            update_light(world, light);
        }
    });
    world.subscribe::<PoweredLight, EntRemovedFromContainer>(|world, light, event| {
        if &*event.container == BULB_CONTAINER {
            update_light(world, light);
        }
    });
    world.subscribe::<PoweredLight, InteractUsing>(|world, light, event| {
        if !event.handled {
            event.handled = insert_bulb(world, light, event.used);
        }
    });
    world.subscribe::<PoweredLight, InteractHand>(on_interact_hand);
    world.subscribe::<PoweredLight, PowerChanged>(|world, light, _| update_light(world, light));
    world.subscribe::<PoweredLight, DoAfterDone<EjectBulbDoAfter>>(|world, light, event| {
        if event.handled || event.cancelled {
            return;
        }
        eject_bulb(world, light, Some(event.user));
        event.handled = true;
    });
    world.subscribe::<PoweredLight, DamageChanged>(|world, light, event| {
        if event.damage_increased {
            try_destroy_bulb(world, light);
        }
    });
    world.subscribe::<LightBulb, MapInit>(|world, bulb, _| lamp_appearance(world, bulb));
    world.subscribe::<LightBulb, Land>(|world, bulb, _| {
        let Some(sound) = world.get::<LightBulb>(bulb).map(|l| l.break_sound.clone()) else {
            return;
        };
        world.play_entity(&sound, bulb, None);
        set_bulb_state(world, bulb, "Broken");
    });
    // `LitOnPoweredSystem`: свет по питанию.
    world.subscribe::<LitOnPowered, PowerChanged>(|world, entity, event| {
        if let Some(mut light) = world.get_mut::<PointLight>(entity) {
            light.enabled = event.powered;
        }
    });
    world.subscribe::<LitOnPowered, MapInit>(|world, entity, _| {
        let powered = is_powered(world, entity);
        if let Some(mut light) = world.get_mut::<PointLight>(entity) {
            light.enabled = powered;
        }
    });
}

#[cfg(test)]
mod tests;
