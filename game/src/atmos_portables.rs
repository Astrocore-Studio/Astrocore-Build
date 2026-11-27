//! Переносная и стационарная атмосферная техника: газовые канистры (клапан выпуска наружу
//! или в баллон, предохранитель, разрыв), порты для них, переносные устройства на порту
//! (крепятся ключом только на порт) и термомашины (нагреватель и охладитель трубы).
//! Образец — `GasCanisterComponent`, `SharedGasCanisterSystem`, `GasCanisterSystem`,
//! `GasPortComponent`, `GasPortableComponent`, `GasPortableSystem`,
//! `GasThermoMachineComponent`, `SharedGasThermoMachineSystem`, `GasThermoMachineSystem`,
//! `DumpCanisterBehavior` и `UIRequiresLockComponent` (`LockSystem`) SS14 @ 2f4a2aad.
//!
//! Нет пока: обмена данными термомашины по сети устройств (`AtmosSyncPayload`),
//! обогревателя-«космонагревателя» (`SpaceHeater`), цены.

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, MapInit, RegistryWorldExt};
use astrocore_map::{
    AnchorStateChanged, EntInsertedIntoContainer, EntRemovedFromContainer, TransformWorldExt,
};
use astrocore_net::AppearanceWorldExt;
use astrocore_ui::{BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::anchorable::AnchorAttempt;
use crate::atmos::{GasData, GasMixture, gas};
use crate::atmos_devices::AtmosDeviceUpdate;
use crate::gas_tank::{AirData, GasTank, PressureHolder, check_status, flow_gas};
use crate::item_slots::{ItemSlot, add_item_slot, slot_item, try_eject_to_hands};
use crate::popups::{PopupType, PopupWorldExt};

/// Окно канистры (`GasCanisterUiKey.Key`).
pub const CANISTER_UI_KEY: &str = "enum.GasCanisterUiKey.Key";
/// Окно термомашины (`ThermomachineUiKey.Key`).
pub const THERMOMACHINE_UI_KEY: &str = "enum.ThermomachineUiKey.Key";
/// Ступень давления для вида канистры (`GasCanisterVisuals.PressureState`).
pub const PRESSURE_STATE: &str = "enum.GasCanisterVisuals.PressureState";
/// Баллон вставлен (`GasCanisterVisuals.TankInserted`).
pub const TANK_INSERTED: &str = "enum.GasCanisterVisuals.TankInserted";
/// Сечение клапана канистры (`ReleaseArea`), м².
const CANISTER_RELEASE_AREA: f32 = 0.05;

fn port_name() -> String {
    "port".into()
}
fn slot_name() -> String {
    "tank_slot".into()
}
fn one_atmosphere() -> f32 {
    gas::ONE_ATMOSPHERE
}
fn min_release() -> f32 {
    gas::ONE_ATMOSPHERE / 10.0
}
fn max_release() -> f32 {
    3.0 * gas::ONE_ATMOSPHERE
}
fn safety() -> f32 {
    15.0 * gas::ONE_ATMOSPHERE
}
fn overpressure() -> f32 {
    20.0 * gas::ONE_ATMOSPHERE
}
fn integrity() -> f32 {
    3.0
}
fn valve_sound() -> SoundSpecifier {
    let mut sound = SoundSpecifier::collection("valveSqueak");
    if let SoundSpecifier::Collection { params, .. } = &mut sound {
        params.volume = -5.0;
    }
    sound
}
fn rupture_sound() -> Option<SoundSpecifier> {
    Some(SoundSpecifier::path("/Audio/Effects/spray.ogg"))
}
fn safety_alert() -> Option<String> {
    Some("gas-max-pressure-alert".into())
}

/// Газовая канистра (`GasCanisterComponent` и поля `GasMaxPressureHolderComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasCanister {
    #[serde(rename = "port", default = "port_name")]
    pub port_name: String,
    #[serde(rename = "container", default = "slot_name")]
    pub container_name: String,
    #[serde(default)]
    pub gas_tank_slot: ItemSlot,
    #[serde(default)]
    pub safety_valve_open: bool,
    #[serde(default)]
    pub air: AirData,
    #[serde(default = "min_release")]
    pub min_release_pressure: f32,
    #[serde(default = "max_release")]
    pub max_release_pressure: f32,
    #[serde(default = "one_atmosphere")]
    pub release_pressure: f32,
    #[serde(default)]
    pub release_valve_open: bool,
    #[serde(default = "valve_sound")]
    pub valve_sound: SoundSpecifier,
    #[serde(default = "rupture_sound")]
    pub rupture_sound: Option<SoundSpecifier>,
    #[serde(default = "safety")]
    pub safety_pressure: f32,
    #[serde(default = "overpressure")]
    pub overpressure: f32,
    #[serde(default = "safety_alert")]
    pub safety_alert: Option<String>,
    #[serde(default = "integrity")]
    pub max_integrity: f32,
    #[serde(default = "integrity")]
    pub integrity: f32,
    #[serde(skip)]
    pub mixture: Option<GasMixture>,
    #[serde(skip)]
    pub last_pressure: f32,
}

impl PressureHolder for GasCanister {
    fn air(&self) -> Option<&GasMixture> {
        self.mixture.as_ref()
    }
    fn integrity(&self) -> f32 {
        self.integrity
    }
    fn integrity_mut(&mut self) -> &mut f32 {
        &mut self.integrity
    }
    fn max_integrity(&self) -> f32 {
        self.max_integrity
    }
    fn overpressure(&self) -> f32 {
        self.overpressure
    }
    fn safety_pressure(&self) -> f32 {
        self.safety_pressure
    }
    fn rupture_sound(&self) -> Option<SoundSpecifier> {
        self.rupture_sound.clone()
    }
}

/// Порт для канистр (`GasPortComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasPort {
    #[serde(rename = "pipe", default = "connected")]
    pub pipe_name: String,
}

fn connected() -> String {
    "connected".into()
}

/// Переносное устройство, которое крепится на порт (`GasPortableComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasPortable {
    #[serde(rename = "port", default = "port_name")]
    pub port_name: String,
}

fn pipe() -> String {
    "pipe".into()
}
fn heat_capacity() -> f32 {
    5000.0
}
fn t20c() -> f32 {
    gas::T20C
}
fn two() -> f32 {
    2.0
}
fn cp() -> f32 {
    0.9
}
fn min_temperature() -> f32 {
    73.15
}
fn max_temperature() -> f32 {
    593.15
}

/// Термомашина (`GasThermoMachineComponent`): `coefficientOfPerformance` больше нуля —
/// нагреватель, меньше — охладитель.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasThermoMachine {
    #[serde(rename = "inlet", default = "pipe")]
    pub inlet_name: String,
    #[serde(default = "heat_capacity")]
    pub heat_capacity: f32,
    #[serde(default = "t20c")]
    pub target_temperature: f32,
    #[serde(default = "two")]
    pub temperature_tolerance: f32,
    #[serde(skip)]
    pub hysteresis_state: bool,
    #[serde(rename = "coefficientOfPerformance", default = "cp")]
    pub cp: f32,
    #[serde(default = "min_temperature")]
    pub min_temperature: f32,
    #[serde(default = "max_temperature")]
    pub max_temperature: f32,
    #[serde(default)]
    pub last_energy_delta: f32,
    #[serde(default)]
    pub energy_leak_percentage: f32,
    #[serde(default)]
    pub atmospheric: bool,
}

impl GasThermoMachine {
    /// Нагреватель (`IsHeater`).
    pub fn is_heater(&self) -> bool {
        self.cp >= 0.0
    }
}

fn deny_sound() -> Option<SoundSpecifier> {
    Some(SoundSpecifier::path("/Audio/Machines/custom_deny.ogg"))
}
fn locked_popup() -> Option<String> {
    Some("entity-storage-component-locked-message".into())
}

/// Окна только при нужном состоянии замка (`UIRequiresLockComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiRequiresLock {
    #[serde(default)]
    pub user_interface_keys: Option<Vec<String>>,
    #[serde(default)]
    pub require_locked: bool,
    #[serde(default = "deny_sound")]
    pub access_denied_sound: Option<SoundSpecifier>,
    #[serde(default = "locked_popup")]
    pub popup: Option<String>,
}

/// Состояние окна канистры (`GasCanisterBoundUserInterfaceState` и поля канистры).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasCanisterUiState {
    pub canister_pressure: f32,
    pub port_status: bool,
    pub tank_pressure: f32,
    pub has_tank: bool,
    pub release_pressure: f32,
    pub min_release_pressure: f32,
    pub max_release_pressure: f32,
    pub release_valve_open: bool,
}

/// Вынуть баллон (`GasCanisterHoldingTankEjectMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasCanisterHoldingTankEject;

/// Давление выпуска (`GasCanisterChangeReleasePressureMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasCanisterChangeReleasePressure {
    pub pressure: f32,
}

/// Клапан выпуска (`GasCanisterChangeReleaseValveMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasCanisterChangeReleaseValve {
    pub valve: bool,
}

/// Состояние окна термомашины (поля `GasThermoMachineComponent` и питание).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThermoMachineUiState {
    pub enabled: bool,
    pub heater: bool,
    pub target_temperature: f32,
    pub min_temperature: f32,
    pub max_temperature: f32,
    pub inlet_temperature: Option<f32>,
}

/// Включить или выключить (`GasThermomachineToggleMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasThermomachineToggle;

/// Целевая температура (`GasThermomachineChangeTemperatureMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasThermomachineChangeTemperature {
    pub temperature: f32,
}

// Канистра.

fn canister_air<R>(
    world: &mut World,
    canister: Entity,
    f: impl FnOnce(&mut GasMixture) -> R,
) -> Option<R> {
    world
        .get_mut::<GasCanister>(canister)?
        .mixture
        .as_mut()
        .map(f)
}

/// Воздух канистры.
pub fn canister_mixture(world: &World, canister: Entity) -> Option<&GasMixture> {
    world.get::<GasCanister>(canister)?.mixture.as_ref()
}

/// Вставленный баллон.
pub fn inserted_tank(world: &World, canister: Entity) -> Option<Entity> {
    let name = &world.get::<GasCanister>(canister)?.container_name;
    slot_item(world, canister, name).filter(|t| world.get::<GasTank>(*t).is_some())
}

/// Порт канистры подключён: в сети порта есть ещё узлы (`PortStatus`).
pub fn port_connected(world: &World, canister: Entity) -> bool {
    let Some(name) = world
        .get::<GasCanister>(canister)
        .map(|c| c.port_name.clone())
    else {
        return false;
    };
    let Some(groups) = world.get_resource::<crate::node_container::NodeGroups>() else {
        return false;
    };
    groups
        .group_of(canister, &name)
        .and_then(|g| groups.groups.get(g))
        .is_some_and(|g| g.nodes.len() > 1)
}

/// Окно канистры (`DirtyUI`).
pub fn update_canister_ui(world: &mut World, canister: Entity) {
    let Some(component) = world.get::<GasCanister>(canister).cloned() else {
        return;
    };
    let tank = inserted_tank(world, canister);
    let state = GasCanisterUiState {
        canister_pressure: component.mixture.as_ref().map_or(0.0, GasMixture::pressure),
        port_status: port_connected(world, canister),
        tank_pressure: tank
            .and_then(|t| crate::gas_tank::tank_air(world, t))
            .map_or(0.0, GasMixture::pressure),
        has_tank: tank.is_some(),
        release_pressure: component.release_pressure,
        min_release_pressure: component.min_release_pressure,
        max_release_pressure: component.max_release_pressure,
        release_valve_open: component.release_valve_open,
    };
    world.set_ui_state(canister, CANISTER_UI_KEY, &state);
}

/// Вид по давлению (`UpdateAppearance`): пусто, меньше атмосферы, меньше 15 атм, больше.
fn update_canister_appearance(world: &mut World, canister: Entity) {
    let Some(pressure) = canister_mixture(world, canister).map(GasMixture::pressure) else {
        return;
    };
    let state = if pressure < 10.0 {
        0
    } else if pressure < gas::ONE_ATMOSPHERE {
        1
    } else if pressure < 15.0 * gas::ONE_ATMOSPHERE {
        2
    } else {
        3
    };
    world.set_appearance(canister, PRESSURE_STATE, state);
}

/// Клапан выпуска (`ToggleValve`).
pub fn set_canister_valve(world: &mut World, canister: Entity, open: bool, user: Option<Entity>) {
    let Some(mut component) = world.get_mut::<GasCanister>(canister) else {
        return;
    };
    component.release_valve_open = open;
    let sound = component.valve_sound.clone();
    world.play_entity(&sound, canister, user);
}

/// Предохранитель канистры (`SafetyMeasures`): открыть клапан сброса.
fn canister_safety(world: &mut World, canister: Entity) {
    let Some(component) = world.get::<GasCanister>(canister).cloned() else {
        return;
    };
    if component.safety_valve_open {
        return;
    }
    set_safety_valve(world, canister, true);
    if let Some(alert) = &component.safety_alert {
        let text = crate::examine::loc(world, alert, &[]);
        world.popup_entity(&text, canister, PopupType::LargeCaution);
    }
}

fn set_safety_valve(world: &mut World, canister: Entity, open: bool) {
    let Some(mut component) = world.get_mut::<GasCanister>(canister) else {
        return;
    };
    component.safety_valve_open = open;
    let sound = component.valve_sound.clone();
    world.play_entity(&sound, canister, None);
}

/// Выпустить всё наружу (`PurgeContents`): разрушенная канистра.
pub fn purge_contents(world: &mut World, canister: Entity) {
    let data = crate::atmos::gas_data(world);
    let Some(air) = canister_mixture(world, canister).cloned() else {
        return;
    };
    crate::barotrauma::with_containing_mixture(world, canister, true, |outside| {
        data.merge(outside, &air)
    });
    canister_air(world, canister, GasMixture::clear);
}

/// Смешать воздух сосуда с сетью труб по объёмам (`MixContainerWithPipeNet`).
pub fn mix_container_with_pipe_net(
    data: &GasData,
    container: &mut GasMixture,
    net: &mut GasMixture,
) {
    let mut buffer = GasMixture::new(net.volume + container.volume);
    data.merge(&mut buffer, net);
    data.merge(&mut buffer, container);
    let (net_volume, container_volume) = (net.volume, container.volume);
    net.clear();
    data.merge(net, &buffer);
    net.multiply(net_volume / buffer.volume);
    container.clear();
    data.merge(container, &buffer);
    container.multiply(container_volume / buffer.volume);
}

/// Шаг канистры (`DeviceUpdated`).
fn canister_update(world: &mut World, canister: Entity, dt: f32) {
    let data = crate::atmos::gas_data(world);
    if !check_status::<GasCanister>(world, &data, canister, dt, canister_safety) {
        return;
    }
    let reactions = crate::atmos::gas_reactions(world);
    canister_air(world, canister, |air| {
        let mut exposures = Vec::new();
        crate::atmos::fire::react(&data, &reactions, air, &mut exposures);
    });
    let Some(component) = world.get::<GasCanister>(canister).cloned() else {
        return;
    };
    // Порт подключён — канистра и сеть труб выравниваются.
    if port_connected(world, canister)
        && let Some(mut air) = component.mixture.clone()
    {
        crate::pipes::with_pipe_air(world, canister, &component.port_name, |net| {
            mix_container_with_pipe_net(&data, &mut air, net)
        });
        canister_air(world, canister, |own| *own = air);
    }
    if component.safety_valve_open {
        // Сброс наружу через клапан предохранителя.
        let outside = crate::barotrauma::containing_mixture(world, canister).map(|m| m.pressure());
        let removed = canister_air(world, canister, |air| {
            let delta = air.pressure() - outside.unwrap_or(0.0);
            flow_gas(&data, air, delta, dt, CANISTER_RELEASE_AREA)
        })
        .flatten();
        if let Some(removed) = removed {
            crate::barotrauma::with_containing_mixture(world, canister, true, |outside| {
                data.merge(outside, &removed)
            });
        }
        if canister_mixture(world, canister).map_or(0.0, GasMixture::pressure)
            < component.safety_pressure
        {
            set_safety_valve(world, canister, false);
        }
    } else if component.release_valve_open {
        // Выпуск в баллон или наружу, только из канистры.
        let release = component.release_pressure;
        match inserted_tank(world, canister) {
            Some(tank) => {
                let Some(mut air) = component.mixture.clone() else {
                    return;
                };
                crate::gas_tank::with_tank_air(world, tank, |tank_air| {
                    data.release_gas_to(&mut air, Some(tank_air), release)
                });
                canister_air(world, canister, |own| *own = air);
            }
            None => {
                let Some(mut air) = component.mixture.clone() else {
                    return;
                };
                let done =
                    crate::barotrauma::with_containing_mixture(world, canister, true, |outside| {
                        data.release_gas_to(&mut air, Some(outside), release)
                    });
                if done.is_none() {
                    data.release_gas_to(&mut air, None, release);
                }
                canister_air(world, canister, |own| *own = air);
            }
        }
    }
    let pressure = canister_mixture(world, canister).map_or(0.0, GasMixture::pressure);
    let last = world
        .get::<GasCanister>(canister)
        .map_or(0.0, |c| c.last_pressure);
    // Давление почти не изменилось — окно и вид не трогаем (`CloseToPercent`).
    if (pressure - last).abs() <= 0.000_01 * pressure.abs().max(last.abs()) {
        return;
    }
    if let Some(mut component) = world.get_mut::<GasCanister>(canister) {
        component.last_pressure = pressure;
    }
    update_canister_ui(world, canister);
    update_canister_appearance(world, canister);
}

// Термомашина.

/// Окно термомашины.
pub fn update_thermomachine_ui(world: &mut World, entity: Entity) {
    let Some(machine) = world.get::<GasThermoMachine>(entity).cloned() else {
        return;
    };
    let enabled = world
        .get::<crate::power::ApcPowerReceiver>(entity)
        .is_none_or(|r| !r.power_disabled);
    let inlet_temperature = if machine.atmospheric {
        crate::barotrauma::containing_mixture(world, entity).map(|m| m.temperature())
    } else {
        crate::pipes::pipe_air(world, entity, &machine.inlet_name).map(GasMixture::temperature)
    };
    let state = ThermoMachineUiState {
        enabled,
        heater: machine.is_heater(),
        target_temperature: machine.target_temperature,
        min_temperature: machine.min_temperature,
        max_temperature: machine.max_temperature,
        inlet_temperature,
    };
    world.set_ui_state(entity, THERMOMACHINE_UI_KEY, &state);
}

/// Нагреть или охладить смесь (`AddHeat`).
fn add_heat(data: &GasData, mixture: &mut GasMixture, heat: f32) {
    let capacity = data.heat_capacity(mixture);
    if capacity > 0.0 {
        let temperature = mixture.temperature() + heat / capacity;
        mixture.set_temperature(temperature.max(gas::TCMB));
    }
}

/// Шаг термомашины (`OnThermoMachineUpdated`): с гистерезисом греет или охлаждает трубу
/// (или воздух вокруг) мощностью `теплоёмкость × КПД`, не перескакивая цель; часть тепла
/// уходит наружу (`EnergyLeakPercentage`).
fn thermomachine_update(world: &mut World, entity: Entity, dt: f32) {
    let Some(mut machine) = world.get::<GasThermoMachine>(entity).cloned() else {
        return;
    };
    machine.last_energy_delta = 0.0;
    if !crate::power::is_powered(world, entity)
        || world
            .get::<crate::power::ApcPowerReceiver>(entity)
            .is_none()
    {
        store_machine(world, entity, machine);
        return;
    }
    let data = crate::atmos::gas_data(world);
    let mixture = if machine.atmospheric {
        crate::barotrauma::containing_mixture(world, entity)
    } else {
        crate::pipes::pipe_air(world, entity, &machine.inlet_name).cloned()
    };
    let Some(mixture) = mixture else {
        store_machine(world, entity, machine);
        return;
    };
    let sign = machine.cp.signum();
    let mut target = machine.target_temperature;
    let high = target + sign * machine.temperature_tolerance;
    let temperature = mixture.temperature();
    if sign * temperature >= sign * high {
        machine.hysteresis_state = false;
    } else if sign * temperature < sign * target {
        machine.hysteresis_state = true;
    }
    if !machine.hysteresis_state {
        store_machine(world, entity, machine);
        return;
    }
    target = high;
    let dq = machine.heat_capacity * machine.cp * dt;
    // Тепло не больше, чем нужно до цели (теплоёмкость со шкалой, `applyScaling: true`).
    let limit = (target - temperature) * data.heat_capacity(&mixture);
    let mut scale = 1.0;
    if dq.abs() > limit.abs() {
        scale = if dq != 0.0 { limit / dq } else { 0.0 };
        machine.hysteresis_state = false;
    }
    let actual = dq * scale;
    if machine.atmospheric {
        crate::barotrauma::with_containing_mixture(world, entity, true, |air| {
            add_heat(&data, air, actual)
        });
        machine.last_energy_delta = actual;
    } else {
        let leak = actual * machine.energy_leak_percentage;
        let pipe = actual - leak;
        crate::pipes::with_pipe_air(world, entity, &machine.inlet_name, |air| {
            add_heat(&data, air, pipe)
        });
        machine.last_energy_delta = pipe;
        if leak != 0.0 {
            crate::barotrauma::with_containing_mixture(world, entity, true, |air| {
                add_heat(&data, air, leak)
            });
        }
    }
    let load = machine.heat_capacity;
    store_machine(world, entity, machine);
    if let Some(mut receiver) = world.get_mut::<crate::power::ApcPowerReceiver>(entity) {
        receiver.power_load = load;
    }
}

fn store_machine(world: &mut World, entity: Entity, machine: GasThermoMachine) {
    if let Some(mut component) = world.get_mut::<GasThermoMachine>(entity) {
        *component = machine;
    }
}

/// Целевая температура (`OnChangeTemperature`): не выше предела нагревателя, не ниже
/// предела охладителя и не ниже реликтового излучения.
pub fn set_target_temperature(world: &mut World, entity: Entity, temperature: f32) {
    let Some(mut machine) = world.get_mut::<GasThermoMachine>(entity) else {
        return;
    };
    let mut target = if machine.is_heater() {
        temperature.min(machine.max_temperature)
    } else {
        temperature.max(machine.min_temperature)
    };
    target = target.max(gas::TCMB);
    machine.target_temperature = target;
}

/// Строки осмотра термомашины (`OnExamined`).
fn thermomachine_examined(world: &mut World, entity: Entity, event: &mut crate::examine::Examined) {
    let Some(machine) = world.get::<GasThermoMachine>(entity).cloned() else {
        return;
    };
    let name = astrocore_loc::EntityNameWorldExt::entity_name(world, entity);
    let color = if machine.is_heater() {
        "red"
    } else {
        "deepskyblue"
    };
    let temperature = (machine.target_temperature * 100.0).round() / 100.0;
    let text = crate::examine::loc(
        world,
        "gas-thermomachine-system-examined",
        &[
            ("machineName", astrocore_loc::LocArg::from(name.as_str())),
            ("tempColor", astrocore_loc::LocArg::from(color)),
            ("temp", astrocore_loc::LocArg::from(f64::from(temperature))),
        ],
    );
    event.push_markup(text, 0);
}

// Переносные устройства на порту.

/// Есть ли порт на клетке (`FindGasPortIn`).
pub fn gas_port_at(world: &World, entity: Entity) -> Option<Entity> {
    let transform = world.get::<astrocore_map::Transform>(entity)?;
    let grid = transform.grid()?;
    let coordinates = world.coordinates(entity)?;
    let local = if coordinates.parent == grid {
        coordinates.position
    } else {
        let position = world.world_transform(entity)?.position;
        world.world_transform(grid)?.to_local(position)
    };
    let cell = astrocore_map::TileCoord::containing(local, transform.level());
    world
        .get::<astrocore_map::MapGrid>(grid)?
        .anchored_at(cell)
        .iter()
        .copied()
        .find(|e| world.get::<GasPort>(*e).is_some())
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<GasCanister>();
    world.register_data_component::<GasPort>();
    world.register_data_component::<GasPortable>();
    world.register_data_component::<GasThermoMachine>();
    world.register_data_component_as::<UiRequiresLock>("UIRequiresLock");
    world.register_ui_message::<GasCanisterHoldingTankEject>();
    world.register_ui_message::<GasCanisterChangeReleasePressure>();
    world.register_ui_message::<GasCanisterChangeReleaseValve>();
    world.register_ui_message::<GasThermomachineToggle>();
    world.register_ui_message::<GasThermomachineChangeTemperature>();
    if !side.is_server() {
        return;
    }
    // Канистра.
    world.subscribe::<GasCanister, MapInit>(|world, canister, _| {
        let Some(component) = world.get::<GasCanister>(canister).cloned() else {
            return;
        };
        add_item_slot(
            world,
            canister,
            &component.container_name,
            component.gas_tank_slot.clone(),
        );
        if let Some(mut c) = world.get_mut::<GasCanister>(canister)
            && c.mixture.is_none()
        {
            c.mixture = Some(component.air.mixture());
        }
        update_canister_ui(world, canister);
        update_canister_appearance(world, canister);
    });
    world.subscribe::<GasCanister, AtmosDeviceUpdate>(|world, canister, event| {
        canister_update(world, canister, event.dt);
    });
    world.subscribe::<GasCanister, EntInsertedIntoContainer>(|world, canister, event| {
        on_slot_changed(world, canister, &event.container, true);
    });
    world.subscribe::<GasCanister, EntRemovedFromContainer>(|world, canister, event| {
        on_slot_changed(world, canister, &event.container, false);
    });
    world.subscribe::<GasCanister, BoundUiMessage<GasCanisterHoldingTankEject>>(
        |world, canister, event| {
            let Some(name) = world
                .get::<GasCanister>(canister)
                .map(|c| c.container_name.clone())
            else {
                return;
            };
            if slot_item(world, canister, &name).is_none() {
                return;
            }
            try_eject_to_hands(world, canister, &name, Some(event.actor));
            update_canister_ui(world, canister);
        },
    );
    world.subscribe::<GasCanister, BoundUiMessage<GasCanisterChangeReleasePressure>>(
        |world, canister, event| {
            if let Some(mut c) = world.get_mut::<GasCanister>(canister) {
                c.release_pressure = event
                    .message
                    .pressure
                    .clamp(c.min_release_pressure, c.max_release_pressure);
            }
            update_canister_ui(world, canister);
        },
    );
    world.subscribe::<GasCanister, BoundUiMessage<GasCanisterChangeReleaseValve>>(
        |world, canister, event| {
            set_canister_valve(world, canister, event.message.valve, Some(event.actor));
            update_canister_ui(world, canister);
        },
    );

    // Крепление на порт (`OnPortableAnchorAttempt`) и подключение узла.
    world.subscribe::<GasPortable, AnchorAttempt>(|world, entity, event| {
        if event.anchoring && gas_port_at(world, entity).is_none() {
            event.cancelled = true;
        }
    });
    world.subscribe::<GasPortable, AnchorStateChanged>(|world, _, _| {
        crate::node_container::mark_dirty(world);
    });

    // Термомашина.
    world.subscribe::<GasThermoMachine, AtmosDeviceUpdate>(|world, entity, event| {
        thermomachine_update(world, entity, event.dt);
    });
    world.subscribe::<GasThermoMachine, MapInit>(|world, entity, _| {
        update_thermomachine_ui(world, entity);
    });
    world.subscribe::<GasThermoMachine, crate::examine::Examined>(thermomachine_examined);
    world.subscribe::<GasThermoMachine, BoundUiMessage<GasThermomachineToggle>>(
        |world, entity, _| {
            let disabled = world
                .get::<crate::power::ApcPowerReceiver>(entity)
                .is_some_and(|r| r.power_disabled);
            crate::power::set_power_disabled(world, entity, !disabled);
            update_thermomachine_ui(world, entity);
        },
    );
    world.subscribe::<GasThermoMachine, BoundUiMessage<GasThermomachineChangeTemperature>>(
        |world, entity, event| {
            set_target_temperature(world, entity, event.message.temperature);
            update_thermomachine_ui(world, entity);
        },
    );

    // Окна за замком (`OnUIOpenAttempt`, `LockToggled`).
    world.subscribe::<UiRequiresLock, crate::activatable_ui::ActivatableUiOpenAttempt>(
        |world, entity, event| {
            if event.cancelled {
                return;
            }
            let Some(requires) = world.get::<UiRequiresLock>(entity).cloned() else {
                return;
            };
            let Some(locked) = world.get::<crate::lock::Lock>(entity).map(|l| l.locked) else {
                return;
            };
            if locked == requires.require_locked {
                return;
            }
            event.cancelled = true;
            if locked && let Some(popup) = &requires.popup {
                let text = crate::examine::loc(world, popup, &[]);
                world.popup_entity_for(&text, entity, event.user, PopupType::Small);
            }
            if let Some(sound) = &requires.access_denied_sound {
                world.play_entity(sound, entity, None);
            }
        },
    );
    world.subscribe::<UiRequiresLock, crate::lock::LockToggled>(|world, entity, event| {
        let Some(requires) = world.get::<UiRequiresLock>(entity).cloned() else {
            return;
        };
        if event.locked == requires.require_locked {
            return;
        }
        match &requires.user_interface_keys {
            Some(keys) => {
                for key in keys {
                    world.close_ui_for_all(entity, key);
                }
            }
            None => crate::activatable_ui::close_all(world, entity),
        }
    });
}

/// Баллон вставлен или вынут (`OnCanisterContainerModified`).
fn on_slot_changed(world: &mut World, canister: Entity, container: &str, inserted: bool) {
    let Some(name) = world
        .get::<GasCanister>(canister)
        .map(|c| c.container_name.clone())
    else {
        return;
    };
    if container != name {
        return;
    }
    update_canister_ui(world, canister);
    world.set_appearance(canister, TANK_INSERTED, inserted);
}

#[cfg(test)]
mod tests;
