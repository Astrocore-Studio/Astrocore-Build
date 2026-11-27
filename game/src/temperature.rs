//! Температура тел: теплоёмкость по массе, теплообмен с воздухом вокруг раз в секунду, урон
//! от жары и холода по логистической кривой, замедление на холоде, защита одеждой и
//! терморегуляция живых тел (обмен веществ, пот, дрожь). Образец — `TemperatureComponent`,
//! `SharedTemperatureSystem`, `TemperatureSystem` (с `.Damage`), `HeatContainerHelpers`,
//! `TemperatureDamageComponent`, `TemperatureSpeedComponent`,
//! `TemperatureProtectionComponent`, `ChangeTemperatureOnCollideComponent`,
//! `ThermalRegulatorSystem`, `AtmosExposedComponent` и `AtmosphereSystem.Update` SS14
//! @ 2f4a2aad.
//!
//! Нет пока: значков тревоги (`Alerts`) — вместе с интерфейсом состояния; порогов от
//! контейнера (`ContainerTemperature`) и внутренней температуры (`InternalTemperature`) —
//! их используют криокапсулы и готовка.

use std::collections::BTreeSet;
use std::time::Duration;

use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update};
use astrocore_physics::Fixtures;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::atmos::gas::{HEAT_SCALE, T20C};
use crate::damage::{DamageSpecifier, Damageable, DamageableWorldExt, Rejuvenate};
use crate::fixed_point::FixedPoint2;
use crate::inventory::{Inventory, InventoryWorldExt, SlotFlags};
use crate::mobs::MobStateWorldExt;
use crate::movement_speed::{RefreshMovementSpeedModifiers, refresh_movement_speed_modifiers};
use crate::projectiles::ProjectileHit;

/// Задержка обновления от воздуха (`ExposedUpdateDelay`), с.
pub const EXPOSED_UPDATE_DELAY: f32 = 1.0;
/// Задержка замедления после смены температуры (`SlowdownApplicationDelay`), с.
const SLOWDOWN_APPLICATION_DELAY: Duration = Duration::from_secs(1);

fn specific_heat() -> f32 {
    3000.0
}
fn t20c() -> f32 {
    T20C
}
fn conductivity() -> f32 {
    150.0
}

/// Температура тела (`TemperatureComponent`): теплоёмкость — масса × удельная теплоёмкость.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Temperature {
    #[serde(default = "specific_heat")]
    pub specific_heat: f32,
    #[serde(default)]
    pub heat_capacity: f32,
    #[serde(default = "t20c")]
    pub temperature: f32,
    #[serde(default = "conductivity")]
    pub thermal_conductivity: f32,
}

impl Default for Temperature {
    fn default() -> Self {
        Self {
            specific_heat: specific_heat(),
            heat_capacity: 0.0,
            temperature: T20C,
            thermal_conductivity: conductivity(),
        }
    }
}

fn heat_threshold() -> f32 {
    360.0
}
fn cold_threshold() -> f32 {
    260.0
}
fn damage_cap() -> FixedPoint2 {
    FixedPoint2::from_f32(8.0)
}

/// Урон от температуры (`TemperatureDamageComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemperatureDamage {
    #[serde(default = "heat_threshold")]
    pub heat_damage_threshold: f32,
    #[serde(default = "cold_threshold")]
    pub cold_damage_threshold: f32,
    #[serde(default)]
    pub cold_damage: DamageSpecifier,
    #[serde(default)]
    pub heat_damage: DamageSpecifier,
    #[serde(default = "damage_cap")]
    pub damage_cap: FixedPoint2,
    #[serde(default)]
    pub taking_damage: bool,
    #[serde(skip)]
    pub last_update: Duration,
}

/// Замедление на холоде (`TemperatureSpeedComponent`): порог температуры → множитель.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemperatureSpeed {
    #[serde(default)]
    pub thresholds: crate::ordered::OrderedMap<f32>,
    #[serde(skip)]
    pub current_speed_modifier: Option<f32>,
    #[serde(skip)]
    pub next_slowdown_update: Option<Duration>,
}

impl TemperatureSpeed {
    fn thresholds(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.thresholds
            .iter()
            .filter_map(|(k, v)| Some((k.parse::<f32>().ok()?, *v)))
    }
}

fn one() -> f32 {
    1.0
}

/// Одежда ослабляет теплообмен (`TemperatureProtectionComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemperatureProtection {
    #[serde(default = "one")]
    pub coefficient: f32,
}

/// Снаряд меняет температуру цели (`ChangeTemperatureOnCollideComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeTemperatureOnCollide {
    #[serde(default)]
    pub heat: f32,
    #[serde(default = "yes")]
    pub ignore_heat_resistance: bool,
}

fn yes() -> bool {
    true
}

/// Терморегуляция тела (`ThermalRegulatorComponent`), тепло в джоулях за обновление.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThermalRegulator {
    #[serde(default = "update_interval")]
    pub update_interval: f32,
    #[serde(default)]
    pub metabolism_heat: f32,
    #[serde(default)]
    pub radiated_heat: f32,
    #[serde(default)]
    pub sweat_heat_regulation: f32,
    #[serde(default)]
    pub shivering_heat_regulation: f32,
    #[serde(default)]
    pub implicit_heat_regulation: f32,
    #[serde(default)]
    pub normal_body_temperature: f32,
    #[serde(default)]
    pub thermal_regulation_temperature_threshold: f32,
    #[serde(skip)]
    pub next_update: Duration,
}

fn update_interval() -> f32 {
    1.0
}

fn exposed_area() -> f32 {
    2.0
}

/// Обменивается теплом с воздухом вокруг (`AtmosExposedComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtmosExposed {
    #[serde(default = "exposed_area")]
    pub exposed_area: f32,
    #[serde(skip)]
    pub last_exposure: Duration,
}

/// Температура изменилась (`TemperatureChangedEvent`). На сущности.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemperatureChanged {
    pub current: f32,
    pub last: f32,
}

/// Перед теплообменом (`BeforeHeatExchangeEvent`): одежда ослабляет передачу тепла.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeforeHeatExchange {
    pub heat_transfer_modifier: f32,
}

/// Можно ли потеть (`SweatAttemptEvent`) и дрожать (`ShiverAttemptEvent`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SweatAttempt {
    pub cancelled: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShiverAttempt {
    pub cancelled: bool,
}

/// Сущности, которым пора пересчитать урон (`ShouldUpdateDamage`).
#[derive(Resource, Debug, Default)]
struct ShouldUpdateDamage(BTreeSet<Entity>);

fn now(world: &World) -> Duration {
    world.resource::<GameTime>().cur_time()
}

/// Множитель теплообмена от одежды (`BeforeHeatExchangeEvent` с передачей в инвентарь, без
/// рук).
fn heat_transfer_modifier(world: &mut World, entity: Entity) -> f32 {
    let mut event = BeforeHeatExchange {
        heat_transfer_modifier: 1.0,
    };
    world.raise(entity, &mut event);
    for item in worn_items(world, entity) {
        world.raise(item, &mut event);
    }
    event.heat_transfer_modifier
}

/// Надетое, кроме карманов: куда передаются события теплообмена и защиты от огня
/// (`IInventoryRelayEvent.TargetSlots = ~POCKET`).
pub(crate) fn worn_items(world: &World, entity: Entity) -> Vec<Entity> {
    let Some(inventory) = world.get::<Inventory>(entity) else {
        return Vec::new();
    };
    let targets = SlotFlags(!SlotFlags::POCKET.0);
    inventory
        .slots
        .iter()
        .filter(|slot| slot.slot_flags.intersects(targets))
        .filter_map(|slot| world.slot_entity(entity, &slot.name))
        .collect()
}

fn changed(world: &mut World, entity: Entity, last: f32) {
    let Some(current) = world.get::<Temperature>(entity).map(|t| t.temperature) else {
        return;
    };
    world.raise(entity, &mut TemperatureChanged { current, last });
}

/// Тепло за время `delta` при проводимости `g` (`ConductHeatQuery`), не больше, чем нужно
/// для равновесия с телом теплоёмкости `other_capacity`; без неё — до температуры `other`.
fn conduct_query(
    temperature: f32,
    capacity: f32,
    other: f32,
    other_capacity: Option<f32>,
    delta: f32,
    g: f32,
) -> f32 {
    let dq = g * (other - temperature) * delta;
    let max = match other_capacity {
        // `EquilibriumHeatQuery`: тепло до общей температуры.
        Some(other_capacity) => {
            let total = capacity + other_capacity;
            if total <= 0.0 {
                0.0
            } else {
                let equilibrium = (temperature * capacity + other * other_capacity) / total;
                ((equilibrium - temperature) * capacity).abs()
            }
        }
        None => ((other - temperature) * capacity).abs(),
    };
    dq.clamp(-max, max)
}

/// Новая температура после тепла `dq` (`AddHeatQuery`): не ниже нуля.
fn add_heat_query(temperature: f32, capacity: f32, dq: f32) -> f32 {
    if capacity <= 0.0 {
        return temperature;
    }
    (temperature + dq / capacity).max(0.0)
}

/// Обмен теплом с телом теплоёмкости `other_capacity` и температуры `other` за время
/// `delta` (`ConductHeat`). Возвращает тепло, полученное сущностью, и новую температуру
/// другого тела.
pub fn conduct_heat(
    world: &mut World,
    entity: Entity,
    other: f32,
    other_capacity: f32,
    delta: f32,
    heat_transfer_mod: f32,
    ignore_heat_resistance: bool,
) -> Option<(f32, f32)> {
    let temperature = world.get::<Temperature>(entity)?.clone();
    if (temperature.temperature - other).abs() < 0.000_01 {
        return Some((0.0, other));
    }
    let mut g = temperature.thermal_conductivity * heat_transfer_mod;
    if !ignore_heat_resistance {
        g *= heat_transfer_modifier(world, entity);
    }
    let dq = conduct_query(
        temperature.temperature,
        temperature.heat_capacity,
        other,
        Some(other_capacity),
        delta,
        g,
    );
    let last = temperature.temperature;
    if let Some(mut t) = world.get_mut::<Temperature>(entity) {
        t.temperature = add_heat_query(last, temperature.heat_capacity, dq);
    }
    changed(world, entity, last);
    Some((dq, add_heat_query(other, other_capacity, -dq)))
}

/// Добавить или забрать тепло (`ChangeHeat`). Возвращает тепло после защиты одеждой.
pub fn change_heat(
    world: &mut World,
    entity: Entity,
    mut heat: f32,
    ignore_heat_resistance: bool,
) -> f32 {
    let Some(temperature) = world.get::<Temperature>(entity).cloned() else {
        return 0.0;
    };
    if heat == 0.0 {
        return 0.0;
    }
    if !ignore_heat_resistance {
        heat *= heat_transfer_modifier(world, entity);
    }
    let last = temperature.temperature;
    if let Some(mut t) = world.get_mut::<Temperature>(entity) {
        t.temperature = add_heat_query(last, temperature.heat_capacity, heat);
    }
    changed(world, entity, last);
    heat
}

/// Задать температуру (`SetTemperature`).
pub fn set_temperature(world: &mut World, entity: Entity, value: f32) {
    let Some(mut temperature) = world.get_mut::<Temperature>(entity) else {
        return;
    };
    let last = temperature.temperature;
    temperature.temperature = value;
    changed(world, entity, last);
}

/// Температура сущности.
pub fn temperature_of(world: &World, entity: Entity) -> Option<f32> {
    world.get::<Temperature>(entity).map(|t| t.temperature)
}

/// Теплообмен с воздухом вокруг (`OnAtmosExposedUpdate`): тело и воздух клетки меняют
/// температуру. Воздух клетки при этом не будится, как в SS14.
fn exposed_update(world: &mut World, gas_data: &crate::atmos::GasData, entity: Entity, area: f32) {
    let Some(temperature) = world.get::<Temperature>(entity).cloned() else {
        return;
    };
    let Some((other, other_capacity)) =
        crate::barotrauma::with_containing_mixture(world, entity, false, |mixture| {
            (
                mixture.temperature(),
                gas_data.heat_capacity(mixture) * HEAT_SCALE,
            )
        })
    else {
        return;
    };
    if (temperature.temperature - other).abs() < 0.000_01 {
        return;
    }
    let Some((_, air_temperature)) = conduct_heat(
        world,
        entity,
        other,
        other_capacity,
        EXPOSED_UPDATE_DELAY,
        area,
        false,
    ) else {
        return;
    };
    crate::barotrauma::with_containing_mixture(world, entity, false, |mixture| {
        mixture.set_temperature(air_temperature)
    });
}

/// Урон от температуры (`ChangeDamage`): жара — логистическая кривая до предела, холод —
/// корень от разницы.
fn change_damage(world: &mut World, entity: Entity, delta: Duration) {
    let time = now(world);
    let Some(mut component) = world.get_mut::<TemperatureDamage>(entity) else {
        return;
    };
    component.last_update = time;
    let component = component.clone();
    if world.get::<Damageable>(entity).is_none() {
        return;
    }
    let Some(temperature) = temperature_of(world, entity) else {
        return;
    };
    let heat_k = 0.005f64;
    let y = f64::from(component.damage_cap.float());
    let c = y * 2.0;
    let seconds = delta.as_secs_f64();
    let damage = if temperature >= component.heat_damage_threshold {
        let diff = f64::from((temperature - component.heat_damage_threshold).abs());
        let value = c / (1.0 + (-heat_k * diff).exp()) - y;
        Some(component.heat_damage.scale((value * seconds) as f32))
    } else if temperature <= component.cold_damage_threshold {
        let diff = f64::from((temperature - component.cold_damage_threshold).abs());
        let value = (diff * (y * y / f64::from(component.cold_damage_threshold))).sqrt();
        Some(component.cold_damage.scale((value * seconds) as f32))
    } else {
        None
    };
    let taking = damage.is_some();
    if let Some(damage) = damage {
        world.change_damage(entity, &damage, true, false, None);
    }
    if let Some(mut component) = world.get_mut::<TemperatureDamage>(entity) {
        component.taking_damage = taking;
    }
}

/// Замедление: порог пересечён — новый множитель через секунду (`OnTemperatureChanged`).
fn on_speed_temperature_changed(world: &mut World, entity: Entity, event: &mut TemperatureChanged) {
    let time = now(world);
    let Some(mut speed) = world.get_mut::<TemperatureSpeed>(entity) else {
        return;
    };
    let thresholds: Vec<(f32, f32)> = speed.thresholds().collect();
    for (threshold, modifier) in &thresholds {
        if (event.current < *threshold && event.last > *threshold)
            || (event.current > *threshold && event.last < *threshold)
        {
            speed.next_slowdown_update = Some(time + SLOWDOWN_APPLICATION_DELAY);
            speed.current_speed_modifier = Some(*modifier);
            break;
        }
    }
    let Some(max) = thresholds
        .iter()
        .map(|(t, _)| *t)
        .max_by(|a, b| a.total_cmp(b))
    else {
        return;
    };
    if event.current > max && event.last < max {
        speed.next_slowdown_update = Some(time + SLOWDOWN_APPLICATION_DELAY);
        speed.current_speed_modifier = None;
    }
}

/// Терморегуляция (`ProcessThermalRegulation`).
fn thermal_regulation(world: &mut World, entity: Entity) {
    let (Some(regulator), Some(temperature)) = (
        world.get::<ThermalRegulator>(entity).cloned(),
        world.get::<Temperature>(entity).cloned(),
    ) else {
        return;
    };
    let mut total = regulator.metabolism_heat - regulator.radiated_heat;
    let capacity = temperature.heat_capacity;
    let diff = (temperature.temperature - regulator.normal_body_temperature).abs();
    let target = diff * capacity;
    if temperature.temperature > regulator.normal_body_temperature {
        total -= target.min(regulator.implicit_heat_regulation);
    } else {
        total += target.min(regulator.implicit_heat_regulation);
    }
    change_heat(world, entity, total, true);
    let Some(current) = temperature_of(world, entity) else {
        return;
    };
    let diff = (current - regulator.normal_body_temperature).abs();
    let target = diff * capacity;
    if diff < regulator.thermal_regulation_temperature_threshold {
        return;
    }
    if current > regulator.normal_body_temperature {
        let mut attempt = SweatAttempt::default();
        world.raise(entity, &mut attempt);
        if attempt.cancelled {
            return;
        }
        change_heat(
            world,
            entity,
            -target.min(regulator.sweat_heat_regulation),
            true,
        );
    } else {
        let mut attempt = ShiverAttempt::default();
        world.raise(entity, &mut attempt);
        if attempt.cancelled {
            return;
        }
        change_heat(
            world,
            entity,
            target.min(regulator.shivering_heat_regulation),
            true,
        );
    }
}

fn update(world: &mut World) {
    let time = now(world);
    let delay = Duration::from_secs_f32(EXPOSED_UPDATE_DELAY);

    // Обмен теплом с воздухом (`AtmosphereSystem.Update`).
    let exposed: Vec<(Entity, f32)> = world
        .query::<(Entity, &AtmosExposed, &Temperature)>()
        .iter(world)
        .filter(|(_, exposed, _)| exposed.last_exposure + delay <= time)
        .map(|(e, exposed, _)| (e, exposed.exposed_area))
        .collect();
    let gas_data = (!exposed.is_empty()).then(|| crate::atmos::gas_data(world));
    for (entity, area) in exposed {
        if let Some(gas_data) = &gas_data {
            exposed_update(world, gas_data, entity, area);
        }
        if let Some(mut exposed) = world.get_mut::<AtmosExposed>(entity) {
            exposed.last_exposure = time;
        }
    }

    // Терморегуляция (`ThermalRegulatorSystem.Update`).
    let regulators: Vec<Entity> = world
        .query::<(Entity, &ThermalRegulator)>()
        .iter(world)
        .filter(|(_, r)| time >= r.next_update)
        .map(|(e, _)| e)
        .collect();
    for entity in regulators {
        if let Some(mut regulator) = world.get_mut::<ThermalRegulator>(entity) {
            let interval = Duration::from_secs_f32(regulator.update_interval);
            regulator.next_update += interval;
            if regulator.next_update < time {
                regulator.next_update = time + interval;
            }
        }
        if world.is_mob_dead(entity) {
            continue;
        }
        thermal_regulation(world, entity);
    }

    // Замедление (`SharedTemperatureSystem.Update`).
    let slowed: Vec<Entity> = world
        .query::<(Entity, &TemperatureSpeed)>()
        .iter(world)
        .filter(|(_, s)| s.next_slowdown_update.is_some_and(|t| time >= t))
        .map(|(e, _)| e)
        .collect();
    for entity in slowed {
        if let Some(mut speed) = world.get_mut::<TemperatureSpeed>(entity) {
            speed.next_slowdown_update = None;
        }
        refresh_movement_speed_modifiers(world, entity);
    }

    // Урон (`UpdateDamage`).
    let queued = std::mem::take(&mut world.resource_mut::<ShouldUpdateDamage>().0);
    for entity in queued {
        let Some(component) = world.get::<TemperatureDamage>(entity) else {
            continue;
        };
        let delta = time.saturating_sub(component.last_update);
        if component.taking_damage && delta < Duration::from_secs(1) {
            continue;
        }
        change_damage(world, entity, delta);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Temperature>();
    world.register_data_component::<TemperatureDamage>();
    world.register_data_component::<TemperatureSpeed>();
    world.register_data_component::<TemperatureProtection>();
    world.register_data_component::<ChangeTemperatureOnCollide>();
    world.register_data_component::<ThermalRegulator>();
    world.register_data_component::<AtmosExposed>();
    if !side.is_server() {
        return;
    }
    world.init_resource::<ShouldUpdateDamage>();
    // Теплоёмкость по массе; живые тела — сразу с нормальной температурой (`OnMapInit`).
    world.subscribe::<Temperature, MapInit>(|world, entity, _| {
        let mass = world
            .get::<Fixtures>(entity)
            .map(Fixtures::mass)
            .filter(|m| *m > 0.0)
            .unwrap_or(1.0);
        let normal = world
            .get::<ThermalRegulator>(entity)
            .map(|r| r.normal_body_temperature);
        if let Some(mut temperature) = world.get_mut::<Temperature>(entity) {
            temperature.heat_capacity = mass * temperature.specific_heat;
            if let Some(normal) = normal {
                temperature.temperature = normal;
            }
        }
    });
    world.subscribe::<ThermalRegulator, MapInit>(|world, entity, _| {
        let time = now(world);
        if let Some(mut regulator) = world.get_mut::<ThermalRegulator>(entity) {
            regulator.next_update = time + Duration::from_secs_f32(regulator.update_interval);
        }
    });
    world.subscribe::<Temperature, Rejuvenate>(|world, entity, _| {
        let normal = world
            .get::<ThermalRegulator>(entity)
            .map_or(T20C, |r| r.normal_body_temperature);
        set_temperature(world, entity, normal);
    });
    world.subscribe::<TemperatureProtection, BeforeHeatExchange>(|world, entity, event| {
        if let Some(protection) = world.get::<TemperatureProtection>(entity) {
            event.heat_transfer_modifier *= protection.coefficient;
        }
    });
    // Урон пересчитывается при смене температуры (`EnqueueDamage`).
    world.subscribe::<TemperatureDamage, TemperatureChanged>(|world, entity, _| {
        let time = now(world);
        let added = world.resource_mut::<ShouldUpdateDamage>().0.insert(entity);
        if let Some(mut component) = world.get_mut::<TemperatureDamage>(entity)
            && added
            && !component.taking_damage
        {
            component.last_update = time;
        }
    });
    world.subscribe::<TemperatureSpeed, TemperatureChanged>(on_speed_temperature_changed);
    world.subscribe::<TemperatureSpeed, RefreshMovementSpeedModifiers>(|world, entity, event| {
        let Some(speed) = world.get::<TemperatureSpeed>(entity) else {
            return;
        };
        if speed.next_slowdown_update.is_some() {
            return;
        }
        if let Some(modifier) = speed.current_speed_modifier {
            event.modify_speed(modifier, modifier);
        }
    });
    world.subscribe::<ChangeTemperatureOnCollide, ProjectileHit>(|world, projectile, event| {
        let Some(component) = world.get::<ChangeTemperatureOnCollide>(projectile).cloned() else {
            return;
        };
        change_heat(
            world,
            event.target,
            component.heat,
            component.ignore_heat_resistance,
        );
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
