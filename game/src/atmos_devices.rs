//! Атмосферная техника на трубах: майнеры газа, пассивные вентиляции, инжекторы, насосы
//! давления и объёма, клапаны, фильтры, вентиляции и скрубберы. Образец —
//! `AtmosDeviceComponent`/`AtmosDeviceSystem`, `GasMinerSystem`, `GasPassiveVentSystem`,
//! `GasOutletInjectorSystem`, `GasPressurePumpSystem`, `GasVolumePumpSystem`,
//! `GasValveSystem`, `GasFilterSystem`, `GasVentPumpSystem`, `GasVentScrubberSystem` SS14 @
//! 2f4a2aad.
//!
//! Устройства обновляются на шаге `AtmosDevices` атмосферы своего грида событием
//! [`AtmosDeviceUpdate`]. Настройка через окна и сигналы тревог — с устройствами связи.

use std::collections::BTreeSet;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{ComponentInit, EventWorldExt, GameTime, RegistryWorldExt};
use astrocore_map::{TileCoord, Transform};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::atmos::gas::*;
use crate::atmos::{GasMixture, gas_data, is_tile_space, with_tile_mixture};
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterWorldExt};
use crate::examine::{Examined, loc};
use crate::interaction::ActivateInWorld;
use crate::node_container::{AlwaysReachable, mark_dirty};
use crate::pipes::{with_pipe_air, with_two_pipe_airs};
use crate::power::is_powered;
use crate::verbs::{GetVerbs, Verb, VerbKind};
use crate::weldable::is_welded;

/// Ускорение насосов (`atmos.speedup`).
pub const PUMP_SPEEDUP: f32 = 8.0;
/// Наибольшее давление на выходе (`MaxOutputPressure`).
pub const MAX_OUTPUT_PRESSURE: f32 = 4500.0;
/// Наибольшая скорость, л/с (`MaxTransferRate`).
pub const MAX_TRANSFER_RATE: f32 = 200.0;

fn yes() -> bool {
    true
}
fn pipe() -> String {
    "pipe".into()
}
fn inlet() -> String {
    "inlet".into()
}
fn outlet() -> String {
    "outlet".into()
}
fn filter() -> String {
    "filter".into()
}
fn max_rate() -> f32 {
    MAX_TRANSFER_RATE
}
fn max_output() -> f32 {
    MAX_OUTPUT_PRESSURE
}
fn atmosphere() -> f32 {
    ONE_ATMOSPHERE
}
fn double_output() -> f32 {
    2.0 * MAX_OUTPUT_PRESSURE
}
fn t20c() -> f32 {
    T20C
}

/// Устройство атмосферы (`AtmosDeviceComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtmosDevice {
    #[serde(default = "yes")]
    pub require_anchored: bool,
    #[serde(default)]
    pub join_system: bool,
}

/// Шаг устройства (`AtmosDeviceUpdateEvent`): `dt` — время между шагами, секунды.
#[derive(Debug, Clone, Copy)]
pub struct AtmosDeviceUpdate {
    pub dt: f32,
    pub grid: Entity,
}

/// Клетка устройства и её грид.
fn device_cell(world: &World, entity: Entity) -> Option<(Entity, TileCoord)> {
    let transform = world.get::<Transform>(entity)?;
    Some((
        transform.grid()?,
        TileCoord::containing(transform.local_position(), transform.level()),
    ))
}

/// Устройства на гриде для шага (`AtmosDevices`).
pub fn devices_on(world: &mut World, grid: Entity) -> Vec<Entity> {
    let mut devices: Vec<Entity> = world
        .query::<(Entity, &AtmosDevice, &Transform)>()
        .iter(world)
        .filter(|(_, device, transform)| {
            transform.grid() == Some(grid) && (!device.require_anchored || transform.anchored())
        })
        .map(|(e, _, _)| e)
        .collect();
    devices.sort();
    devices
}

// Майнер.

/// Майнер газа (`GasMinerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasMiner {
    pub spawn_gas: String,
    #[serde(default = "t20c")]
    pub spawn_temperature: f32,
    #[serde(default = "spawn_amount")]
    pub spawn_amount: f32,
    #[serde(default = "miner_pressure")]
    pub max_external_pressure: f32,
    #[serde(default = "infinity")]
    pub max_external_amount: f32,
    #[serde(default = "yes")]
    pub show_examine_text: bool,
}

fn spawn_amount() -> f32 {
    MOLES_CELL_STANDARD * 20.0
}
fn miner_pressure() -> f32 {
    6500.0
}
fn infinity() -> f32 {
    f32::INFINITY
}

fn miner_update(world: &mut World, entity: Entity, dt: f32) {
    let Some(miner) = world.get::<GasMiner>(entity).cloned() else {
        return;
    };
    let Some(gas_id) = gas_index(&miner.spawn_gas) else {
        return;
    };
    let Some((grid, cell)) = device_cell(world, entity) else {
        return;
    };
    if is_tile_space(world, grid, cell) {
        return;
    }
    let data = gas_data(world);
    with_tile_mixture(world, grid, cell, |environment| {
        let allowable = ((miner.max_external_pressure - environment.pressure())
            * environment.volume
            / (miner.spawn_temperature * R))
            .min(miner.max_external_amount - environment.total_moles());
        let to_spawn = allowable.clamp(0.0, miner.spawn_amount * dt);
        if to_spawn < GAS_MIN_MOLES {
            return;
        }
        let mut merger = GasMixture::new(1.0).with_temperature(miner.spawn_temperature);
        merger.set_moles(gas_id, to_spawn);
        data.merge(environment, &merger);
    });
}

// Пассивная вентиляция и инжектор.

/// Пассивная вентиляция (`GasPassiveVentComponent`): уравнивает трубу и клетку.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasPassiveVent {
    #[serde(default = "pipe", rename = "inlet")]
    pub inlet_name: String,
}

fn passive_vent_update(world: &mut World, entity: Entity) {
    let Some(vent) = world.get::<GasPassiveVent>(entity).cloned() else {
        return;
    };
    let Some((grid, cell)) = device_cell(world, entity) else {
        return;
    };
    let Some(mut inlet) = crate::pipes::pipe_air(world, entity, &vent.inlet_name).cloned() else {
        return;
    };
    let data = gas_data(world);
    let done = with_tile_mixture(world, grid, cell, |environment| {
        let inlet_air = inlet.remove_ratio(1.0);
        let env_air = environment.remove_ratio(1.0);
        let mut merge = GasMixture::new(inlet_air.volume + env_air.volume);
        data.merge(&mut merge, &inlet_air);
        data.merge(&mut merge, &env_air);
        let back = merge.remove_volume(inlet_air.volume);
        data.merge(&mut inlet, &back);
        data.merge(environment, &merge);
    });
    if done.is_some() {
        with_pipe_air(world, entity, &vent.inlet_name, |air| *air = inlet);
    }
}

/// Инжектор (`GasOutletInjectorComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasOutletInjector {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "max_rate")]
    pub transfer_rate: f32,
    #[serde(default = "max_rate")]
    pub max_transfer_rate: f32,
    #[serde(default = "double_output")]
    pub max_pressure: f32,
    #[serde(default = "pipe", rename = "inlet")]
    pub inlet_name: String,
}

fn injector_update(world: &mut World, entity: Entity, dt: f32) {
    let Some(injector) = world.get::<GasOutletInjector>(entity).cloned() else {
        return;
    };
    if !injector.enabled {
        return;
    }
    let Some((grid, cell)) = device_cell(world, entity) else {
        return;
    };
    let Some(mut inlet) = crate::pipes::pipe_air(world, entity, &injector.inlet_name).cloned()
    else {
        return;
    };
    let data = gas_data(world);
    let changed = with_tile_mixture(world, grid, cell, |environment| {
        if environment.pressure() > injector.max_pressure {
            return false;
        }
        let ratio = (dt * injector.transfer_rate * PUMP_SPEEDUP / inlet.volume).min(1.0);
        let removed = inlet.remove_ratio(ratio);
        data.merge(environment, &removed);
        true
    });
    if changed == Some(true) {
        with_pipe_air(world, entity, &injector.inlet_name, |air| *air = inlet);
    }
}

// Насосы.

/// Насос давления (`GasPressurePumpComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasPressurePump {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "inlet", rename = "inlet")]
    pub inlet_name: String,
    #[serde(default = "outlet", rename = "outlet")]
    pub outlet_name: String,
    #[serde(default = "atmosphere")]
    pub target_pressure: f32,
    #[serde(default = "max_output")]
    pub max_target_pressure: f32,
}

fn pressure_pump_update(world: &mut World, entity: Entity) {
    let Some(pump) = world.get::<GasPressurePump>(entity).cloned() else {
        return;
    };
    if !pump.enabled || !is_powered(world, entity) {
        return;
    }
    let data = gas_data(world);
    with_two_pipe_airs(
        world,
        entity,
        &pump.inlet_name,
        &pump.outlet_name,
        |inlet, outlet| {
            let output_pressure = outlet.pressure();
            if output_pressure >= pump.target_pressure {
                return;
            }
            if inlet.total_moles() > 0.0 && inlet.temperature() > 0.0 {
                let delta = pump.target_pressure - output_pressure;
                let moles = delta * outlet.volume / (inlet.temperature() * R);
                let removed = inlet.remove(moles);
                data.merge(outlet, &removed);
            }
        },
    );
}

/// Насос объёма (`GasVolumePumpComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasVolumePump {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "inlet", rename = "inlet")]
    pub inlet_name: String,
    #[serde(default = "outlet", rename = "outlet")]
    pub outlet_name: String,
    #[serde(default = "max_rate")]
    pub transfer_rate: f32,
    #[serde(default = "max_rate")]
    pub max_transfer_rate: f32,
    #[serde(default = "lower_threshold")]
    pub lower_threshold: f32,
    #[serde(default = "double_output")]
    pub higher_threshold: f32,
    #[serde(default = "overclock_threshold")]
    pub overclock_threshold: f32,
    #[serde(default = "leak_ratio")]
    pub leak_ratio: f32,
    #[serde(skip)]
    pub overclocked: bool,
    #[serde(skip)]
    pub blocked: bool,
    #[serde(skip)]
    pub last_moles_transferred: f32,
}

fn lower_threshold() -> f32 {
    0.01
}
fn overclock_threshold() -> f32 {
    1000.0
}
fn leak_ratio() -> f32 {
    0.1
}

/// Доля смеси для давления `target` в другой смеси (`FractionToMaxPressure`).
pub fn fraction_to_max_pressure(from: &GasMixture, to: &GasMixture, target: f32) -> f32 {
    let moles = (target - to.pressure()) * to.volume / (from.temperature() * R);
    moles / from.total_moles()
}

fn volume_pump_update(world: &mut World, entity: Entity, dt: f32) {
    let Some(pump) = world.get::<GasVolumePump>(entity).cloned() else {
        return;
    };
    let unpowered = world
        .get::<crate::power::ApcPowerReceiver>(entity)
        .is_some()
        && !is_powered(world, entity);
    if !pump.enabled || unpowered {
        return;
    }
    let data = gas_data(world);
    let result = with_two_pipe_airs(
        world,
        entity,
        &pump.inlet_name,
        &pump.outlet_name,
        |inlet, outlet| {
            let input = inlet.pressure();
            let output = outlet.pressure();
            let mut blocked = (input <= pump.lower_threshold || output >= pump.higher_threshold)
                && !pump.overclocked;
            if output - input >= pump.overclock_threshold && pump.overclocked {
                blocked = true;
            }
            if blocked {
                return (true, 0.0);
            }
            let transfer_ratio = pump.transfer_rate * PUMP_SPEEDUP * dt / inlet.volume;
            let limit = fraction_to_max_pressure(inlet, outlet, pump.higher_threshold);
            let removed = inlet.remove_ratio(transfer_ratio.min(limit));
            let moles = removed.total_moles();
            data.merge(outlet, &removed);
            (false, moles)
        },
    );
    if let (Some((blocked, moles)), Some(mut component)) =
        (result, world.get_mut::<GasVolumePump>(entity))
    {
        component.blocked = blocked;
        component.last_moles_transferred = moles;
    }
}

// Клапан.

/// Клапан (`GasValveComponent`): открытый соединяет вход и выход в одну сеть.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasValve {
    #[serde(default = "yes")]
    pub open: bool,
    #[serde(default = "inlet", rename = "inlet")]
    pub inlet_name: String,
    #[serde(default = "outlet", rename = "outlet")]
    pub outlet_name: String,
    #[serde(default = "valve_sound")]
    pub valve_sound: SoundSpecifier,
}

fn valve_sound() -> SoundSpecifier {
    SoundSpecifier::collection("valveSqueak")
}

/// Открыть или закрыть клапан (`GasValveSystem.Set`).
pub fn set_valve(world: &mut World, entity: Entity, open: bool) {
    let Some(valve) = world.get::<GasValve>(entity).cloned() else {
        return;
    };
    if let Some(mut component) = world.get_mut::<GasValve>(entity) {
        component.open = open;
    }
    let links = if open {
        vec![
            (valve.inlet_name.clone(), valve.outlet_name.clone()),
            (valve.outlet_name.clone(), valve.inlet_name.clone()),
        ]
    } else {
        Vec::new()
    };
    world.entity_mut(entity).insert(AlwaysReachable(links));
    astrocore_net::AppearanceWorldExt::set_appearance(
        world,
        entity,
        "enum.FilterVisuals.Enabled",
        open,
    );
    mark_dirty(world);
}

// Фильтр.

/// Фильтр (`GasFilterComponent`): выбранный газ — в боковой выход, остальное — дальше.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasFilter {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "inlet")]
    pub inlet: String,
    #[serde(default = "outlet")]
    pub outlet: String,
    #[serde(default = "filter")]
    pub filter: String,
    #[serde(default = "max_rate")]
    pub transfer_rate: f32,
    #[serde(default = "max_rate")]
    pub max_transfer_rate: f32,
    #[serde(default)]
    pub filtered_gas: Option<String>,
}

fn filter_update(world: &mut World, entity: Entity, dt: f32) {
    let Some(component) = world.get::<GasFilter>(entity).cloned() else {
        return;
    };
    if !component.enabled {
        return;
    }
    let names = [&component.inlet, &component.filter, &component.outlet];
    let nets: Vec<Option<usize>> = names
        .iter()
        .map(|n| crate::pipes::net_of(world, entity, n))
        .collect();
    let (Some(a), Some(b), Some(c)) = (nets[0], nets[1], nets[2]) else {
        return;
    };
    if a == b || a == c || b == c {
        return;
    }
    let data = gas_data(world);
    let mut pipe_nets = world.resource_mut::<crate::pipes::PipeNets>();
    let (Some(mut inlet), Some(mut filter), Some(mut outlet)) = (
        pipe_nets.nets.get(&a).map(|n| n.air.clone()),
        pipe_nets.nets.get(&b).map(|n| n.air.clone()),
        pipe_nets.nets.get(&c).map(|n| n.air.clone()),
    ) else {
        return;
    };
    if outlet.pressure() >= MAX_OUTPUT_PRESSURE && filter.pressure() >= MAX_OUTPUT_PRESSURE {
        return;
    }
    let transfer_volume = component.transfer_rate * PUMP_SPEEDUP * dt;
    if transfer_volume <= 0.0 {
        return;
    }
    let mut removed = inlet.remove_volume(transfer_volume);
    if let Some(gas) = component.filtered_gas.as_deref().and_then(gas_index) {
        let limit =
            (MAX_OUTPUT_PRESSURE - filter.pressure()) * filter.volume / (removed.temperature() * R);
        let filtered = limit.min(removed.moles[gas]).max(0.0);
        let mut filtered_mix = GasMixture::new(0.0).with_temperature(removed.temperature());
        filtered_mix.set_moles(gas, filtered);
        removed.adjust_moles(gas, -filtered);
        data.merge(&mut filter, &filtered_mix);
    }
    let limit = fraction_to_max_pressure(&removed, &outlet, MAX_OUTPUT_PRESSURE);
    let passthrough = removed.remove_ratio(limit);
    data.merge(&mut outlet, &passthrough);
    data.merge(&mut inlet, &removed);
    for (net, air) in [(a, inlet), (b, filter), (c, outlet)] {
        if let Some(n) = pipe_nets.nets.get_mut(&net) {
            n.air = air;
        }
    }
}

// Вентиляция и скруббер.

/// Направление вентиляции (`VentPumpDirection`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VentPumpDirection {
    #[default]
    Releasing,
    Siphoning,
}

/// Вентиляция (`GasVentPumpComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasVentPump {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "pipe")]
    pub inlet: String,
    #[serde(default = "pipe")]
    pub outlet: String,
    #[serde(default)]
    pub pump_direction: VentPumpDirection,
    /// Проверки давления (`VentPressureBound`): `ExternalBound`, `InternalBound`, оба.
    #[serde(default = "external_bound")]
    pub pressure_checks: String,
    #[serde(default = "atmosphere")]
    pub external_pressure_bound: f32,
    #[serde(default)]
    pub internal_pressure_bound: f32,
    #[serde(default = "max_output")]
    pub max_pressure: f32,
    #[serde(default = "atmosphere")]
    pub target_pressure_change: f32,
    #[serde(default = "two")]
    pub pump_power: f32,
    #[serde(default = "lockout_threshold")]
    pub under_pressure_lockout_threshold: f32,
    #[serde(default = "lockout_leaking")]
    pub under_pressure_lockout_leaking: f32,
    #[serde(default)]
    pub pressure_lockout_override: bool,
    #[serde(default = "lockout_duration")]
    pub manual_lockout_disabled_duration: f64,
    #[serde(default = "two")]
    pub manual_lockout_disable_do_after: f32,
    /// Связь с кнопками и датчиками (`CanLink`) — с устройствами связи.
    #[serde(default)]
    pub can_link: bool,
    #[serde(skip)]
    pub under_pressure_lockout: bool,
    #[serde(skip)]
    pub manual_lockout_until: Option<f64>,
}

fn external_bound() -> String {
    "ExternalBound".into()
}
fn two() -> f32 {
    2.0
}
fn lockout_threshold() -> f32 {
    80.0
}
fn lockout_leaking() -> f32 {
    0.0001
}
fn lockout_duration() -> f64 {
    30.0
}

fn bounds(text: &str) -> (bool, bool) {
    let external = text.contains("External") || text.contains("Both");
    let internal = text.contains("Internal") || text.contains("Both");
    (external, internal)
}

fn vent_pump_update(world: &mut World, entity: Entity, dt: f32) {
    if is_welded(world, entity) || !is_powered(world, entity) {
        return;
    }
    let Some(vent) = world.get::<GasVentPump>(entity).cloned() else {
        return;
    };
    let node = match vent.pump_direction {
        VentPumpDirection::Releasing => vent.inlet.clone(),
        VentPumpDirection::Siphoning => vent.outlet.clone(),
    };
    if !vent.enabled {
        return;
    }
    let Some(mut pipe) = crate::pipes::pipe_air(world, entity, &node).cloned() else {
        return;
    };
    let Some((grid, cell)) = device_cell(world, entity) else {
        return;
    };
    let now = world.resource::<GameTime>().cur_time().as_secs_f64();
    let manually_disabled = vent.manual_lockout_until.is_some_and(|until| now < until);
    let data = gas_data(world);
    let (external, internal) = bounds(&vent.pressure_checks);
    let result = with_tile_mixture(world, grid, cell, |environment| {
        let mut pressure_delta = dt * vent.target_pressure_change;
        let lockout =
            environment.pressure() < vent.under_pressure_lockout_threshold && !manually_disabled;
        if vent.pump_direction == VentPumpDirection::Releasing && pipe.pressure() > 0.0 {
            if environment.pressure() > vent.max_pressure {
                return lockout;
            }
            if external {
                let supply = (pipe.pressure() * vent.pump_power).min(vent.external_pressure_bound);
                pressure_delta = pressure_delta.min(supply - environment.pressure());
            }
            if pressure_delta <= 0.0 {
                return lockout;
            }
            let mut moles = pressure_delta * environment.volume / (pipe.temperature() * R);
            if lockout && !vent.pressure_lockout_override && !manually_disabled {
                let pipe_delta = pipe.pressure() - environment.pressure();
                moles = dt * pipe_delta * vent.under_pressure_lockout_leaking;
                if moles < 0.0 {
                    return lockout;
                }
            }
            if internal {
                let internal_delta = pipe.pressure() - vent.internal_pressure_bound;
                if internal_delta <= 0.0 {
                    return lockout;
                }
                let max = internal_delta * pipe.volume / (pipe.temperature() * R);
                moles = moles.min(max);
            }
            let removed = pipe.remove(moles);
            data.merge(environment, &removed);
        } else if vent.pump_direction == VentPumpDirection::Siphoning
            && environment.pressure() > 0.0
        {
            if pipe.pressure() > vent.max_pressure {
                return lockout;
            }
            if internal {
                pressure_delta = pressure_delta.min(vent.internal_pressure_bound - pipe.pressure());
            }
            if pressure_delta <= 0.0 {
                return lockout;
            }
            let mut moles = pressure_delta * pipe.volume / (environment.temperature() * R);
            if external {
                let external_delta = environment.pressure() - vent.external_pressure_bound;
                if external_delta <= 0.0 {
                    return lockout;
                }
                let max = external_delta * environment.volume / (environment.temperature() * R);
                moles = moles.min(max);
            }
            let removed = environment.remove(moles);
            data.merge(&mut pipe, &removed);
        }
        lockout
    });
    let Some(lockout) = result else {
        return;
    };
    with_pipe_air(world, entity, &node, |air| *air = pipe);
    if let Some(mut component) = world.get_mut::<GasVentPump>(entity) {
        component.under_pressure_lockout = lockout;
        if !manually_disabled {
            component.manual_lockout_until = None;
        }
    }
}

/// Отвёрткой снять блокировку низкого давления на 30 секунд (`VentScrewedDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
struct VentScrewed;

/// Направление скруббера (`ScrubberPumpDirection`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScrubberPumpDirection {
    Siphoning,
    #[default]
    Scrubbing,
}

fn default_filter_gases() -> BTreeSet<String> {
    [
        "CarbonDioxide",
        "Plasma",
        "Tritium",
        "WaterVapor",
        "Ammonia",
        "NitrousOxide",
        "Frezon",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Скруббер (`GasVentScrubberComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasVentScrubber {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "pipe", rename = "outlet")]
    pub outlet_name: String,
    #[serde(default = "default_filter_gases")]
    pub filter_gases: BTreeSet<String>,
    #[serde(default)]
    pub pump_direction: ScrubberPumpDirection,
    #[serde(default = "max_rate")]
    pub transfer_rate: f32,
    #[serde(default = "max_rate")]
    pub max_transfer_rate: f32,
    #[serde(default = "max_output")]
    pub max_pressure: f32,
    #[serde(default)]
    pub wide_net: bool,
}

/// Забрать воздух клетки (`Scrub`): `true` — что-то забрали.
fn scrub(
    data: &crate::atmos::GasData,
    dt: f32,
    scrubber: &GasVentScrubber,
    tile: &mut GasMixture,
    destination: &mut GasMixture,
) -> bool {
    if destination.pressure() >= 50.0 * ONE_ATMOSPHERE {
        return false;
    }
    let ratio = (dt * scrubber.transfer_rate * PUMP_SPEEDUP / tile.volume).min(1.0);
    let mut removed = tile.remove_ratio(ratio);
    if removed.total_moles().abs() < 1e-7 {
        return false;
    }
    match scrubber.pump_direction {
        ScrubberPumpDirection::Scrubbing => {
            // `ScrubInto`: выбранные газы — в трубу, остальное — обратно.
            let mut buffer =
                GasMixture::new(removed.volume).with_temperature(removed.temperature());
            for gas in scrubber.filter_gases.iter().filter_map(|g| gas_index(g)) {
                buffer.adjust_moles(gas, removed.moles[gas]);
                removed.set_moles(gas, 0.0);
            }
            data.merge(destination, &buffer);
            data.merge(tile, &removed);
        }
        ScrubberPumpDirection::Siphoning => data.merge(destination, &removed),
    }
    true
}

fn scrubber_update(world: &mut World, entity: Entity, dt: f32) {
    if is_welded(world, entity) || !is_powered(world, entity) {
        return;
    }
    let Some(scrubber) = world.get::<GasVentScrubber>(entity).cloned() else {
        return;
    };
    if !scrubber.enabled {
        return;
    }
    let Some(mut outlet) = crate::pipes::pipe_air(world, entity, &scrubber.outlet_name).cloned()
    else {
        return;
    };
    let Some((grid, cell)) = device_cell(world, entity) else {
        return;
    };
    let data = gas_data(world);
    let mut cells = vec![cell];
    if scrubber.wide_net {
        cells.extend((0..4).map(|i| crate::atmos::grid::offset(cell, i)));
    }
    for cell in cells {
        with_tile_mixture(world, grid, cell, |tile| {
            scrub(&data, dt, &scrubber, tile, &mut outlet)
        });
    }
    with_pipe_air(world, entity, &scrubber.outlet_name, |air| *air = outlet);
}

/// Обновить устройство (`AtmosDeviceUpdateEvent` всем его системам).
pub fn update_device(world: &mut World, entity: Entity, dt: f32) {
    if world.get::<GasMiner>(entity).is_some() {
        miner_update(world, entity, dt);
    }
    if world.get::<GasPassiveVent>(entity).is_some() {
        passive_vent_update(world, entity);
    }
    if world.get::<GasOutletInjector>(entity).is_some() {
        injector_update(world, entity, dt);
    }
    if world.get::<GasPressurePump>(entity).is_some() {
        pressure_pump_update(world, entity);
    }
    if world.get::<GasVolumePump>(entity).is_some() {
        volume_pump_update(world, entity, dt);
    }
    if world.get::<GasFilter>(entity).is_some() {
        filter_update(world, entity, dt);
    }
    if world.get::<GasVentPump>(entity).is_some() {
        vent_pump_update(world, entity, dt);
    }
    if world.get::<GasVentScrubber>(entity).is_some() {
        scrubber_update(world, entity, dt);
    }
    let grid = device_cell(world, entity).map(|(g, _)| g);
    if let Some(grid) = grid {
        world.raise(entity, &mut AtmosDeviceUpdate { dt, grid });
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<AtmosDevice>();
    world.register_data_component::<GasMiner>();
    world.register_data_component::<GasPassiveVent>();
    world.register_data_component::<GasOutletInjector>();
    world.register_data_component::<GasPressurePump>();
    world.register_data_component::<GasVolumePump>();
    world.register_data_component::<GasValve>();
    world.register_data_component::<GasFilter>();
    world.register_data_component::<GasVentPump>();
    world.register_data_component::<GasVentScrubber>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<GasValve, ComponentInit>(|world, entity, _| {
        let open = world.get::<GasValve>(entity).is_some_and(|v| v.open);
        set_valve(world, entity, open);
    });
    // `SharedGasValveSystem.OnActivate`: щелчок открывает и закрывает.
    world.subscribe::<GasValve, ActivateInWorld>(|world, entity, event| {
        if event.handled || !event.complex {
            return;
        }
        let Some(valve) = world.get::<GasValve>(entity).cloned() else {
            return;
        };
        set_valve(world, entity, !valve.open);
        world.play_entity(&valve.valve_sound, entity, None);
        event.handled = true;
    });
    world.subscribe::<GasOutletInjector, ActivateInWorld>(|world, entity, event| {
        if event.handled || !event.complex {
            return;
        }
        if let Some(mut injector) = world.get_mut::<GasOutletInjector>(entity) {
            injector.enabled = !injector.enabled;
        }
        event.handled = true;
    });
    world.subscribe::<GasVentPump, Examined>(|world, entity, event| {
        let Some(vent) = world.get::<GasVentPump>(entity).cloned() else {
            return;
        };
        if event.is_in_details_range
            && vent.pump_direction == VentPumpDirection::Releasing
            && vent.under_pressure_lockout
            && !vent.pressure_lockout_override
            && vent.manual_lockout_until.is_none()
        {
            let text = loc(world, "gas-vent-pump-uvlo", &[]);
            event.push_markup(text, 0);
        }
    });
    world.subscribe::<GasVentPump, GetVerbs>(|world, entity, event| {
        let Some(vent) = world.get::<GasVentPump>(entity).cloned() else {
            return;
        };
        let anchored = world
            .get::<Transform>(entity)
            .is_some_and(Transform::anchored);
        if event.kind != VerbKind::Other || !vent.under_pressure_lockout || !anchored {
            return;
        }
        let user = event.user;
        let text = loc(world, "gas-vent-pump-release-lockout", &[]);
        let mut verb = Verb::new(VerbKind::Other, text, move |world| {
            let delay = world
                .get::<GasVentPump>(entity)
                .map_or(2.0, |v| f64::from(v.manual_lockout_disable_do_after));
            let mut args = DoAfterArgs::new(user, delay, Some(entity), None, Some(entity));
            args.break_on_damage = true;
            args.need_hand = true;
            args.break_on_move = true;
            world.try_start_do_after(args, VentScrewed);
        });
        verb.priority = 1;
        verb.icon = Some("/Textures/Interface/VerbIcons/unlock.svg.192dpi.png".into());
        event.verbs.push(verb);
    });
    world.subscribe::<GasVentPump, DoAfterDone<VentScrewed>>(|world, entity, event| {
        if event.cancelled || event.handled {
            return;
        }
        let now = world.resource::<GameTime>().cur_time().as_secs_f64();
        if let Some(mut vent) = world.get_mut::<GasVentPump>(entity) {
            vent.manual_lockout_until = Some(now + vent.manual_lockout_disabled_duration);
        }
    });
}

#[cfg(test)]
mod tests;
