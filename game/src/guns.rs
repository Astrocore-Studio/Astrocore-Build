//! Оружие без рук: очереди выстрелов, автострельба и боеприпасы от электросети — то, на
//! чём работает излучатель. Образец — `SharedGunSystem` (`AttemptShoot`, `ShootProjectile`),
//! `GunSystem` и `GunSystem.AutoFire`, `NetworkPoweredAmmoProviderSystem`,
//! `ServerNetworkPoweredAmmoProviderSystem`, `PowerStateSystem` и `ServerPowerStateSystem`
//! SS14 @ 2f4a2aad.
//!
//! Нет пока: оружия в руках, магазинов и патронов, разброса и отдачи, хитскана, вспышки,
//! сигналов включения (связь устройств, 5.6б), выбора вида болта излучателя.

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update};
use astrocore_map::{
    AnchorStateChanged, EntityCoordinates, MapWorldExt, SpawnWorldExt, TransformWorldExt,
};
use astrocore_math::Vec2;
use astrocore_net::AppearanceWorldExt;
use astrocore_physics::{BodyType, Physics};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::examine::loc;
use crate::interaction::ActivateInWorld;
use crate::popups::{PopupType, PopupWorldExt};
use crate::power::{
    ApcPowerReceiver, PowerChanged, PowerConsumer, PowerConsumerReceivedChanged, is_powered,
};
use crate::projectiles::{PROJECTILE_SPEED, Projectile, shoot_projectile};

/// Оружие (`GunComponent`), только поля стрельбы.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gun {
    #[serde(default = "three")]
    pub shots_per_burst: u32,
    /// Пауза после очереди, с.
    #[serde(default = "burst_cooldown")]
    pub burst_cooldown: f64,
    /// Выстрелов в секунду внутри очереди.
    #[serde(default = "eight")]
    pub burst_fire_rate: f64,
    #[serde(default = "eight")]
    pub fire_rate: f64,
    /// `SemiAuto`, `Burst` или `FullAuto`.
    #[serde(default = "semi_auto")]
    pub selected_mode: String,
    #[serde(default = "speed")]
    pub projectile_speed: f32,
    #[serde(default)]
    pub sound_gunshot: Option<SoundSpecifier>,
    /// Куда стреляет оружие без цели, в своих осях.
    #[serde(default = "down")]
    pub default_direction: Vec2,
    #[serde(skip)]
    pub next_fire: f64,
    #[serde(skip)]
    pub shot_counter: u32,
    #[serde(skip)]
    pub burst_activated: bool,
    #[serde(skip)]
    pub burst_shots_count: u32,
}

fn three() -> u32 {
    3
}
fn burst_cooldown() -> f64 {
    0.25
}
fn eight() -> f64 {
    8.0
}
fn semi_auto() -> String {
    "SemiAuto".into()
}
fn speed() -> f32 {
    PROJECTILE_SPEED
}
fn down() -> Vec2 {
    Vec2::new(0.0, -1.0)
}

/// Оружие стреляет само, пока включено (`AutoShootGunComponent`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct AutoShootGun {
    #[serde(default)]
    pub enabled: bool,
}

/// Боеприпасы из электросети: каждый выстрел создаёт снаряд прототипа
/// (`NetworkPoweredAmmoProviderComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPoweredAmmoProvider {
    pub prototype: String,
}

/// Устройство с рабочим и холостым потреблением (`PowerStateComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerState {
    #[serde(default)]
    pub is_turned_on: bool,
    #[serde(default)]
    pub is_operating: bool,
    #[serde(default = "idle_draw")]
    pub idle_power_draw: f32,
    #[serde(default = "working_draw")]
    pub working_power_draw: f32,
    #[serde(default = "yes")]
    pub ensure_apc: bool,
    /// Рисунок слоя огней в работе и при нехватке питания.
    #[serde(default = "beam")]
    pub working_state: Option<String>,
    #[serde(default = "underpowered")]
    pub underpowered_state: Option<String>,
}

fn beam() -> Option<String> {
    Some("beam".into())
}
fn underpowered() -> Option<String> {
    Some("underpowered".into())
}

fn idle_draw() -> f32 {
    20.0
}
fn working_draw() -> f32 {
    350.0
}
fn yes() -> bool {
    true
}

/// Излучатель (`EmitterComponent`). Выбора вида болта пока нет.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Emitter {
    #[serde(default)]
    pub selectable_types: Vec<String>,
}

/// Вид устройства (`PowerStateDeviceVisuals.VisualState`): `On`, `Underpowered`, `Off`.
pub const POWER_STATE_VISUALS: &str = "enum.PowerStateDeviceVisuals.VisualState";

/// Устройство заработало или встало (`OperatingStateChanged`). На устройстве.
#[derive(Debug, Clone, Copy)]
pub struct OperatingStateChanged {
    pub is_operating: bool,
}

/// Перед выстрелом (`AttemptShootEvent`): можно отменить. На оружии.
#[derive(Debug, Clone, Copy, Default)]
pub struct AttemptShoot {
    pub cancelled: bool,
}

/// Выстрел сделан (`GunShotEvent`). На оружии.
#[derive(Debug, Clone)]
pub struct GunShot {
    pub projectiles: Vec<Entity>,
}

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

// Питание.

/// Мощность и наличие питания (`SetPowerLoadGetHasPower`).
fn set_load_get_has_power(world: &mut World, entity: Entity, working: bool) -> bool {
    let Some(state) = world.get::<PowerState>(entity).cloned() else {
        return false;
    };
    let draw = if working {
        state.working_power_draw
    } else {
        state.idle_power_draw
    };
    if let Some(mut receiver) = world.get_mut::<ApcPowerReceiver>(entity) {
        receiver.power_load = draw;
        if is_powered(world, entity) {
            return true;
        }
    }
    let Some(mut consumer) = world.get_mut::<PowerConsumer>(entity) else {
        return false;
    };
    consumer.draw_rate = draw;
    consumer.draw_rate <= consumer.received_power
}

fn set_is_powered_state(world: &mut World, entity: Entity, has_power: bool) {
    let Some(state) = world.get::<PowerState>(entity).cloned() else {
        return;
    };
    let visual = match (has_power, state.is_turned_on) {
        (true, true) => "On",
        (false, true) => "Underpowered",
        _ => "Off",
    };
    world.set_appearance(entity, POWER_STATE_VISUALS, visual);
    let operating = state.is_turned_on && has_power;
    if state.is_operating == operating {
        return;
    }
    if let Some(mut state) = world.get_mut::<PowerState>(entity) {
        state.is_operating = operating;
    }
    world.raise(
        entity,
        &mut OperatingStateChanged {
            is_operating: operating,
        },
    );
}

/// Включить или выключить устройство (`SetWorkingState`).
pub fn set_working_state(world: &mut World, entity: Entity, working: bool) {
    let Some(state) = world.get::<PowerState>(entity) else {
        return;
    };
    if state.is_turned_on == working {
        return;
    }
    if let Some(mut state) = world.get_mut::<PowerState>(entity) {
        state.is_turned_on = working;
    }
    let has_power = set_load_get_has_power(world, entity, working);
    set_is_powered_state(world, entity, has_power);
}

/// Включено ли устройство (`GetWorkingState`).
pub fn working_state(world: &World, entity: Entity) -> bool {
    world
        .get::<PowerState>(entity)
        .is_some_and(|s| s.is_turned_on)
}

// Стрельба.

/// Выстрелить по направлению оружия (`AttemptShoot(gun)`): оружие само себе стрелок.
pub fn attempt_shoot(world: &mut World, gun: Entity) -> bool {
    let Some(mut component) = world.get::<Gun>(gun).cloned() else {
        return false;
    };
    if component.fire_rate <= 0.0 {
        return false;
    }
    let target = EntityCoordinates::new(gun, component.default_direction);
    let (Some(from), Some(to)) = (world.map_coordinates(gun), world.to_map_coordinates(target))
    else {
        return false;
    };
    let time = now(world);
    if component.next_fire > time {
        return false;
    }
    let burst = component.selected_mode == "Burst" || component.burst_activated;
    let fire_rate = 1.0
        / if burst {
            component.burst_fire_rate
        } else {
            component.fire_rate
        };
    if component.next_fire < time - fire_rate
        || component.shot_counter == 0 && component.next_fire < time
    {
        component.next_fire = time;
    }
    let mut shots = 0u32;
    while component.next_fire <= time {
        component.next_fire += fire_rate;
        shots += 1;
    }
    if burst {
        shots = shots.min(
            component
                .shots_per_burst
                .saturating_sub(component.shot_counter),
        );
    } else if component.selected_mode == "SemiAuto" {
        shots = shots.min(1u32.saturating_sub(component.shot_counter));
    }
    let mut attempt = AttemptShoot::default();
    world.raise(gun, &mut attempt);
    if attempt.cancelled {
        component.burst_activated = false;
        component.burst_shots_count = 0;
        world.entity_mut(gun).insert(component);
        return false;
    }
    // Боеприпасы (`TakeAmmoEvent`): снаряды из сети.
    let prototype = world
        .get::<NetworkPoweredAmmoProvider>(gun)
        .map(|p| p.prototype.clone());
    let mut ammo = Vec::new();
    if let (Some(prototype), true) = (prototype, shots > 0)
        && let Some(at) = world.snap_to_grid_or_map(from)
    {
        for _ in 0..shots {
            if let Ok(projectile) = world.spawn_attached(&prototype, at) {
                ammo.push(projectile);
            }
        }
    }
    component.shot_counter += shots;
    if ammo.is_empty() {
        component.burst_activated = false;
        component.burst_shots_count = 0;
        component.next_fire += component.burst_cooldown;
        component.shot_counter = 0;
        world.entity_mut(gun).insert(component);
        return false;
    }
    if component.selected_mode == "Burst" {
        component.burst_activated = true;
    }
    if component.burst_activated {
        component.burst_shots_count += shots;
        if component.burst_shots_count >= component.shots_per_burst {
            component.next_fire += component.burst_cooldown;
            component.burst_activated = false;
            component.burst_shots_count = 0;
        }
    }
    // `AttemptShoot(gun)` обнуляет счётчик после каждой попытки.
    component.shot_counter = 0;
    let direction = to.position - from.position;
    let speed = component.projectile_speed;
    let sound = component.sound_gunshot.clone();
    world.entity_mut(gun).insert(component);
    for projectile in &ammo {
        if world.get::<Projectile>(*projectile).is_some() {
            shoot_projectile(
                world,
                *projectile,
                direction,
                Vec2::ZERO,
                Some(gun),
                Some(gun),
                speed,
            );
        }
        if let Some(sound) = &sound {
            world.play_entity(sound, gun, None);
        }
    }
    world.raise(gun, &mut GunShot { projectiles: ammo });
    true
}

/// Автострельба (`GunSystem.Update`).
fn auto_fire(world: &mut World) {
    let time = now(world);
    let ready: Vec<Entity> = world
        .query::<(Entity, &Gun, &AutoShootGun)>()
        .iter(world)
        .filter(|(_, gun, auto)| auto.enabled && gun.next_fire <= time)
        .map(|(e, _, _)| e)
        .collect();
    for gun in ready {
        attempt_shoot(world, gun);
    }
}

fn set_auto_shoot(world: &mut World, gun: Entity, enabled: bool) {
    if let Some(mut auto) = world.get_mut::<AutoShootGun>(gun) {
        auto.enabled = enabled;
    }
}

fn is_static(world: &World, entity: Entity) -> bool {
    world
        .get::<Physics>(entity)
        .is_some_and(|b| b.body_type == BodyType::Static)
}

/// Включить или выключить рукой (`ToggleActive`): только закреплённое устройство.
fn on_activate(world: &mut World, gun: Entity, event: &mut ActivateInWorld) {
    if event.handled || !event.complex {
        return;
    }
    let user = event.user;
    if crate::lock::is_locked(world, gun) {
        let name = astrocore_loc::EntityNameWorldExt::entity_name(world, gun);
        let text = loc(world, "gun-access-locked", &[("target", name.into())]);
        world.popup_entity_for(&text, gun, user, PopupType::Small);
        return;
    }
    event.handled = true;
    let name = astrocore_loc::EntityNameWorldExt::entity_name(world, gun);
    if !is_static(world, gun) {
        let text = loc(world, "gun-not-anchored", &[("target", name.into())]);
        world.popup_entity_for(&text, gun, user, PopupType::Small);
        return;
    }
    let working = working_state(world, gun);
    set_working_state(world, gun, !working);
    let key = if working {
        "gun-toggle-off"
    } else {
        "gun-toggle-on"
    };
    let text = loc(world, key, &[("target", name.into())]);
    world.popup_entity_for(&text, gun, user, PopupType::Small);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Gun>();
    world.register_data_component::<AutoShootGun>();
    world.register_data_component::<NetworkPoweredAmmoProvider>();
    world.register_data_component::<PowerState>();
    world.register_data_component::<Emitter>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<PowerState, MapInit>(|world, entity, _| {
        let Some(state) = world.get::<PowerState>(entity).cloned() else {
            return;
        };
        if state.ensure_apc && world.get::<ApcPowerReceiver>(entity).is_none() {
            world.entity_mut(entity).insert(ApcPowerReceiver {
                needs_power: true,
                power_disabled: false,
                power_load: state.idle_power_draw,
                received_power: 0.0,
            });
        }
        let has_power = set_load_get_has_power(world, entity, state.is_turned_on);
        set_is_powered_state(world, entity, has_power);
    });
    world.subscribe::<PowerState, PowerChanged>(|world, entity, event| {
        if working_state(world, entity) {
            set_is_powered_state(world, entity, event.powered);
        }
    });
    world.subscribe::<PowerState, PowerConsumerReceivedChanged>(|world, entity, event| {
        if working_state(world, entity) {
            set_is_powered_state(world, entity, event.received_power >= event.draw_rate);
        }
    });
    world.subscribe::<NetworkPoweredAmmoProvider, OperatingStateChanged>(|world, gun, event| {
        set_auto_shoot(world, gun, event.is_operating);
    });
    world.subscribe::<NetworkPoweredAmmoProvider, ActivateInWorld>(on_activate);
    world.subscribe::<NetworkPoweredAmmoProvider, AnchorStateChanged>(|world, gun, event| {
        if !event.anchored {
            set_working_state(world, gun, false);
        }
    });
    world.add_game_systems(Update, auto_fire);
}

#[cfg(test)]
mod tests;
