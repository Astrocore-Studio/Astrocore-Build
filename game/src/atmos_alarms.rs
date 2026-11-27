//! Атмосферные датчики и тревоги: датчик (`AtmosMonitor`) сверяет давление, температуру и
//! доли газов с порогами и рассылает тревогу зарегистрированным устройствам; тревожное
//! устройство (`AtmosAlarmable`) собирает тревоги сети, показывает худшую, воет при
//! опасности и поднимает `AtmosAlarm` — по нему закрываются пожарные шлюзы, а воздушная
//! тревога переключает режим вентиляций и скрубберов. Пожарная тревога включается и
//! сбрасывается рукой. Образец — `AtmosAlarmThreshold`, `AtmosMonitorComponent`,
//! `AtmosMonitorSystem`, `AtmosAlarmableComponent`, `AtmosAlarmableSystem`,
//! `AtmosDeviceNetworkSystem`, `FireAlarmSystem`, `AirAlarmComponent`, `AirAlarmSystem`,
//! `AirAlarmModes`, `GasVentPumpData`, `GasVentScrubberData`, обработчики пакетов
//! `GasVentPumpSystem` и `GasVentScrubberSystem`, `DeviceNetworkRequiresPower` SS14
//! @ 2f4a2aad (ADR-0010).
//!
//! Воздушная тревога шлёт уровень на порт своего состояния (`AirDanger`, `AirWarning`,
//! `AirNormal`): высокий на новый, низкий на прежний.
//!
//! Нет пока: провода «паника» (`AirAlarmPanicWire`), взлома электромагнитом, пульта тревог станции.

use std::collections::{BTreeMap, BTreeSet};

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_data::DataNode;
use astrocore_ecs::{EventWorldExt, MapInit, RegistryWorldExt};
use astrocore_net::AppearanceWorldExt;
use astrocore_prototypes::Prototypes;
use astrocore_ui::{BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::atmos::gas::{GAS_COUNT, GAS_NAMES, gas_index};
use crate::atmos::{GasMixture, TileFire};
use crate::atmos_devices::{
    AtmosDeviceUpdate, GasVentPump, GasVentScrubber, ScrubberPumpDirection, VentPumpDirection,
};
use crate::device_network::{
    BeforePacketSent, DeviceListUpdate, DeviceNetwork, DeviceNetworkPacket, address_of, send_packet,
};
use crate::power::{PowerChanged, is_powered};

/// Окно воздушной тревоги (`SharedAirAlarmInterfaceKey.Key`).
pub const AIR_ALARM_UI_KEY: &str = "enum.SharedAirAlarmInterfaceKey.Key";
/// Вид тревоги (`AtmosMonitorVisuals.AlarmType`).
pub const ALARM_TYPE: &str = "enum.AtmosMonitorVisuals.AlarmType";

// Пороги.

/// Уровень тревоги (`AtmosAlarmType`): чем больше, тем хуже.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum AtmosAlarmType {
    #[default]
    Invalid,
    Normal,
    Warning,
    Danger,
    Emagged,
}

impl AtmosAlarmType {
    fn name(self) -> &'static str {
        match self {
            Self::Invalid => "Invalid",
            Self::Normal => "Normal",
            Self::Warning => "Warning",
            Self::Danger => "Danger",
            Self::Emagged => "Emagged",
        }
    }
}

/// Что сработало (`AtmosMonitorThresholdTypeFlags`).
pub mod tripped {
    pub const NONE: u8 = 0;
    pub const TEMPERATURE: u8 = 1 << 0;
    pub const PRESSURE: u8 = 1 << 1;
    pub const GAS: u8 = 1 << 2;

    /// Флаги из данных: имена через запятую или список.
    pub fn parse(names: &[String]) -> u8 {
        names.iter().fold(NONE, |acc, name| {
            acc | match name.trim() {
                "Temperature" => TEMPERATURE,
                "Pressure" => PRESSURE,
                "Gas" => GAS,
                _ => NONE,
            }
        })
    }
}

fn disabled() -> AlarmThresholdSetting {
    AlarmThresholdSetting {
        enabled: false,
        threshold: 0.0,
    }
}
fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}

/// Граница порога (`AlarmThresholdSetting`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AlarmThresholdSetting {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "one")]
    pub threshold: f32,
}

impl AlarmThresholdSetting {
    fn at_or_below(self, value: f32) -> bool {
        self.enabled && value <= self.threshold
    }
    fn at_or_above(self, value: f32) -> bool {
        self.enabled && value >= self.threshold
    }
}

/// Порог тревоги (`AtmosAlarmThreshold`, прототип `alarmThreshold`): опасность за
/// границами, предупреждение за долей от них.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtmosAlarmThreshold {
    #[serde(default)]
    pub ignore: bool,
    #[serde(default = "disabled")]
    pub upper_bound: AlarmThresholdSetting,
    #[serde(default = "disabled")]
    pub lower_bound: AlarmThresholdSetting,
    #[serde(rename = "upperWarnAround", default = "disabled")]
    pub upper_warning_percentage: AlarmThresholdSetting,
    #[serde(rename = "lowerWarnAround", default = "disabled")]
    pub lower_warning_percentage: AlarmThresholdSetting,
}

impl Default for AtmosAlarmThreshold {
    fn default() -> Self {
        Self {
            ignore: false,
            upper_bound: disabled(),
            lower_bound: disabled(),
            upper_warning_percentage: disabled(),
            lower_warning_percentage: disabled(),
        }
    }
}

impl AtmosAlarmThreshold {
    /// Граница предупреждения сверху (`UpperWarningBound`).
    pub fn upper_warning_bound(&self) -> AlarmThresholdSetting {
        AlarmThresholdSetting {
            enabled: self.upper_warning_percentage.enabled,
            threshold: self.upper_bound.threshold * self.upper_warning_percentage.threshold,
        }
    }

    /// Граница предупреждения снизу (`LowerWarningBound`).
    pub fn lower_warning_bound(&self) -> AlarmThresholdSetting {
        AlarmThresholdSetting {
            enabled: self.lower_warning_percentage.enabled,
            threshold: self.lower_bound.threshold * self.lower_warning_percentage.threshold,
        }
    }

    /// Проверить значение (`CheckThreshold`): `None` — в норме или порог отключён.
    pub fn check(&self, value: f32) -> Option<AtmosAlarmType> {
        if self.ignore {
            return None;
        }
        if self.upper_bound.at_or_above(value) {
            return Some(AtmosAlarmType::Danger);
        }
        if self.lower_bound.at_or_below(value) {
            return Some(AtmosAlarmType::Danger);
        }
        if self.upper_warning_bound().at_or_above(value) {
            return Some(AtmosAlarmType::Warning);
        }
        if self.lower_warning_bound().at_or_below(value) {
            return Some(AtmosAlarmType::Warning);
        }
        None
    }
}

/// Порог из прототипа `alarmThreshold`.
pub fn threshold_prototype(world: &World, id: &str) -> Option<AtmosAlarmThreshold> {
    let node = world
        .get_resource::<Prototypes>()?
        .resolved_node("alarmThreshold", id)?
        .clone();
    astrocore_data::from_node(&node).ok()
}

// Данные устройств для воздушной тревоги.

/// Настройки вентиляции для тревоги (`GasVentPumpData`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasVentPumpData {
    pub enabled: bool,
    pub dirty: bool,
    pub ignore_alarms: bool,
    pub releasing: bool,
    pub pressure_checks: String,
    pub external_pressure_bound: f32,
    pub internal_pressure_bound: f32,
    pub pressure_lockout_override: bool,
}

impl GasVentPumpData {
    fn preset(enabled: bool, dirty: bool, external: f32, override_lockout: bool) -> Self {
        Self {
            enabled,
            dirty,
            ignore_alarms: false,
            releasing: true,
            pressure_checks: "ExternalBound".into(),
            external_pressure_bound: external,
            internal_pressure_bound: 0.0,
            pressure_lockout_override: override_lockout,
        }
    }

    /// Режим фильтрации (`FilterModePreset`).
    pub fn filter_mode() -> Self {
        Self::preset(true, false, crate::atmos::gas::ONE_ATMOSPHERE, false)
    }

    /// Режим наполнения (`FillModePreset`): до 50 атм.
    pub fn fill_mode() -> Self {
        Self::preset(true, true, crate::atmos::gas::ONE_ATMOSPHERE * 50.0, true)
    }

    /// Режим паники (`PanicModePreset`): вентиляции выключены.
    pub fn panic_mode() -> Self {
        Self::preset(false, true, crate::atmos::gas::ONE_ATMOSPHERE, false)
    }
}

/// Настройки скруббера для тревоги (`GasVentScrubberData`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasVentScrubberData {
    pub enabled: bool,
    pub dirty: bool,
    pub ignore_alarms: bool,
    pub filter_gases: BTreeSet<String>,
    pub scrubbing: bool,
    pub volume_rate: f32,
    pub wide_net: bool,
}

impl GasVentScrubberData {
    /// Газы по умолчанию (`DefaultFilterGases`).
    pub fn default_filter_gases() -> BTreeSet<String> {
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
        .map(str::to_owned)
        .collect()
    }

    fn preset(enabled: bool, dirty: bool, scrubbing: bool, wide_net: bool) -> Self {
        Self {
            enabled,
            dirty,
            ignore_alarms: false,
            filter_gases: Self::default_filter_gases(),
            scrubbing,
            volume_rate: 200.0,
            wide_net,
        }
    }

    pub fn filter_mode() -> Self {
        Self::preset(true, false, true, false)
    }
    pub fn wide_filter_mode() -> Self {
        Self::preset(true, false, true, true)
    }
    pub fn fill_mode() -> Self {
        Self::preset(false, true, true, false)
    }
    pub fn panic_mode() -> Self {
        Self::preset(true, true, false, true)
    }
}

/// Показания датчика (`AtmosMonitorData`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AtmosMonitorData {
    pub pressure: f32,
    pub temperature: f32,
    pub total_moles: f32,
    pub alarm_state: AtmosAlarmType,
    pub gases: BTreeMap<String, f32>,
    pub pressure_threshold: AtmosAlarmThreshold,
    pub temperature_threshold: AtmosAlarmThreshold,
    pub gas_thresholds: BTreeMap<String, AtmosAlarmThreshold>,
}

/// Данные устройства для тревоги (`IAtmosDeviceData`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AtmosDeviceData {
    Vent(GasVentPumpData),
    Scrubber(GasVentScrubberData),
    Sensor(AtmosMonitorData),
}

// Пакеты сети (`Payloads`).

/// Пришлите свои данные (`AtmosSyncPayload`).
#[derive(Debug, Clone, Copy)]
pub struct AtmosSync;
/// Слать мне тревоги (`AtmosMonitorRegisterDevicePayload`).
#[derive(Debug, Clone, Copy)]
pub struct AtmosMonitorRegisterDevice;
/// Больше не слать (`AtmosMonitorDeregisterDevicePayload`).
#[derive(Debug, Clone, Copy)]
pub struct AtmosMonitorDeregisterDevice;
/// Сбросить тревогу (`AtmosMonitorResetPayload`).
#[derive(Debug, Clone, Copy)]
pub struct AtmosMonitorReset;

/// Задать порог (`AtmosMonitorSetThresholdPayload`).
#[derive(Debug, Clone)]
pub struct AtmosMonitorSetThreshold {
    pub kind: ThresholdKind,
    pub threshold: AtmosAlarmThreshold,
    pub gas: Option<usize>,
}

/// Вид порога (`AtmosMonitorThresholdType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThresholdKind {
    Temperature,
    Pressure,
    Gas,
}

/// Тревога датчика или устройства (`AtmosAlarmPayload`).
#[derive(Debug, Clone)]
pub struct AtmosAlarmPayload {
    pub kind: AtmosAlarmType,
    pub tripped_thresholds: Option<u8>,
    pub source: BTreeSet<String>,
}

/// Тревоги сети (`AtmosAlarmableSyncAlertsPayload`).
#[derive(Debug, Clone)]
pub struct AtmosAlarmableSyncAlerts {
    pub alarm_states: BTreeMap<i32, AtmosAlarmType>,
    pub source: BTreeSet<String>,
}

/// Сбросить всё (`AtmosAlarmableResetAllPayload`).
#[derive(Debug, Clone)]
pub struct AtmosAlarmableResetAll {
    pub source: BTreeSet<String>,
}

/// Режим воздушной тревоги (`AirAlarmSetModePayload`).
#[derive(Debug, Clone, Copy)]
pub struct AirAlarmSetMode {
    pub mode: AirAlarmMode,
}

/// Данные устройства тревоге (`AirAlarmSetDataPayload`).
#[derive(Debug, Clone)]
pub struct AirAlarmSetData {
    pub payload: AtmosDeviceData,
}

/// Новые настройки вентиляции (`GasVentPumpSetDataPayload`).
#[derive(Debug, Clone)]
pub struct GasVentPumpSetData {
    pub data: GasVentPumpData,
}

/// Новые настройки скруббера (`GasVentScrubberSetDataPayload`).
#[derive(Debug, Clone)]
pub struct GasVentScrubberSetData {
    pub data: GasVentScrubberData,
}

/// Режим воздушной тревоги (`AirAlarmMode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AirAlarmMode {
    None,
    #[default]
    Filtering,
    WideFiltering,
    Fill,
    Panic,
}

/// Тревога изменилась (`AtmosAlarmEvent`). На устройстве и широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct AtmosAlarm {
    pub alarm_type: AtmosAlarmType,
}

// Компоненты.

/// Датчик (`AtmosMonitorComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtmosMonitor {
    #[serde(default = "yes")]
    pub net_enabled: bool,
    #[serde(default)]
    pub temperature_threshold_id: Option<String>,
    #[serde(default)]
    pub temperature_threshold: Option<AtmosAlarmThreshold>,
    #[serde(default)]
    pub pressure_threshold_id: Option<String>,
    #[serde(default)]
    pub pressure_threshold: Option<AtmosAlarmThreshold>,
    #[serde(default)]
    pub monitor_fire: bool,
    #[serde(default)]
    pub gas_threshold_prototypes: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub gas_thresholds: Option<BTreeMap<String, AtmosAlarmThreshold>>,
    #[serde(skip)]
    pub last_alarm_state: AtmosAlarmType,
    #[serde(skip)]
    pub tripped_thresholds: u8,
    #[serde(skip)]
    pub registered_devices: BTreeSet<i32>,
    #[serde(default)]
    pub monitors_pipe_net: bool,
    #[serde(default = "monitored")]
    pub node_name_monitored_pipe: String,
}

fn monitored() -> String {
    "monitored".into()
}

fn alarm_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/alarm.ogg")
}
fn alarm_volume() -> f32 {
    -10.0
}

/// Тревожное устройство (`AtmosAlarmableComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtmosAlarmable {
    #[serde(skip)]
    pub network_alarm_states: BTreeMap<i32, AtmosAlarmType>,
    #[serde(skip)]
    pub last_alarm_state: AtmosAlarmType,
    #[serde(skip)]
    pub ignore_alarms: bool,
    #[serde(default = "alarm_sound")]
    pub alarm_sound: SoundSpecifier,
    #[serde(default = "alarm_volume")]
    pub alarm_volume: f32,
    #[serde(rename = "syncWith", default)]
    pub sync_with_tags: BTreeSet<String>,
    #[serde(default)]
    pub monitor_alert_types: Option<DataNode>,
    #[serde(default)]
    pub receive_only: bool,
}

impl AtmosAlarmable {
    fn monitor_flags(&self) -> u8 {
        let Some(node) = &self.monitor_alert_types else {
            return tripped::NONE;
        };
        let names: Vec<String> = match node.as_seq() {
            Some(seq) => seq
                .iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect(),
            None => node
                .as_str()
                .map(|s| s.split(',').map(str::to_owned).collect())
                .unwrap_or_default(),
        };
        tripped::parse(&names)
    }
}

/// Пожарная тревога (`FireAlarmComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FireAlarm {}

/// Воздушная тревога (`AirAlarmComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AirAlarm {
    #[serde(default)]
    pub current_mode: AirAlarmMode,
    #[serde(default = "yes")]
    pub auto_mode: bool,
    #[serde(skip)]
    pub known_devices: BTreeSet<i32>,
    #[serde(skip)]
    pub vent_data: BTreeMap<i32, GasVentPumpData>,
    #[serde(skip)]
    pub scrubber_data: BTreeMap<i32, GasVentScrubberData>,
    #[serde(skip)]
    pub sensor_data: BTreeMap<i32, AtmosMonitorData>,
    #[serde(skip)]
    pub state: AtmosAlarmType,
    #[serde(default)]
    pub panic_wire_cut: bool,
}

/// Порты воздушной тревоги (`DangerPort`, `WarningPort`, `NormalPort`).
pub const AIR_DANGER_PORT: &str = "AirDanger";
pub const AIR_WARNING_PORT: &str = "AirWarning";
pub const AIR_NORMAL_PORT: &str = "AirNormal";

/// Порт состояния (`GetPort`).
pub fn alarm_port(state: AtmosAlarmType) -> &'static str {
    match state {
        AtmosAlarmType::Danger => AIR_DANGER_PORT,
        AtmosAlarmType::Warning => AIR_WARNING_PORT,
        _ => AIR_NORMAL_PORT,
    }
}

/// Сеть устройства только с питанием (`DeviceNetworkRequiresPowerComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceNetworkRequiresPower {}

fn tags_of(world: &World, entity: Entity) -> BTreeSet<String> {
    world
        .get::<crate::tag::Tag>(entity)
        .map(|t| t.tags.clone())
        .unwrap_or_default()
}

fn own_address(world: &World, entity: Entity) -> i32 {
    world.get::<DeviceNetwork>(entity).map_or(0, |d| d.address)
}

// Сеть атмосферных устройств (`AtmosDeviceNetworkSystem`).

/// Зарегистрироваться у датчиков (`Register`).
pub fn register(world: &mut World, entity: Entity, address: Option<i32>) {
    send_packet(
        world,
        entity,
        address,
        AtmosMonitorRegisterDevice,
        None,
        None,
    );
}

/// Отписаться (`Deregister`).
pub fn deregister(world: &mut World, entity: Entity, address: Option<i32>) {
    send_packet(
        world,
        entity,
        address,
        AtmosMonitorDeregisterDevice,
        None,
        None,
    );
}

/// Попросить данные (`Sync`).
pub fn sync(world: &mut World, entity: Entity, address: Option<i32>) {
    send_packet(world, entity, address, AtmosSync, None, None);
}

// Датчик.

/// Воздух датчика: клетка или сеть трубы.
fn monitored_gas(world: &mut World, entity: Entity) -> Option<GasMixture> {
    let monitor = world.get::<AtmosMonitor>(entity)?;
    if monitor.monitors_pipe_net {
        let node = monitor.node_name_monitored_pipe.clone();
        if let Some(air) = crate::pipes::pipe_air(world, entity, &node) {
            return Some(air.clone());
        }
    }
    crate::barotrauma::containing_mixture(world, entity)
}

/// Разослать тревогу зарегистрированным (`BroadcastAlertPacket`).
fn broadcast_alert(world: &mut World, entity: Entity) {
    let Some(monitor) = world.get::<AtmosMonitor>(entity).cloned() else {
        return;
    };
    if !monitor.net_enabled || world.get::<crate::tag::Tag>(entity).is_none() {
        return;
    }
    let payload = AtmosAlarmPayload {
        kind: monitor.last_alarm_state,
        tripped_thresholds: Some(monitor.tripped_thresholds),
        source: tags_of(world, entity),
    };
    for address in monitor.registered_devices {
        send_packet(world, entity, Some(address), payload.clone(), None, None);
    }
}

/// Объявить состояние датчика (`Alert`).
pub fn monitor_alert(world: &mut World, entity: Entity, state: AtmosAlarmType, flags: Option<u8>) {
    let Some(mut monitor) = world.get_mut::<AtmosMonitor>(entity) else {
        return;
    };
    monitor.last_alarm_state = state;
    if let Some(flags) = flags {
        monitor.tripped_thresholds = flags;
    }
    broadcast_alert(world, entity);
}

/// Сверить воздух с порогами (`UpdateState`).
fn update_monitor_state(world: &mut World, entity: Entity, air: &GasMixture) {
    let Some(monitor) = world.get::<AtmosMonitor>(entity).cloned() else {
        return;
    };
    let mut state = AtmosAlarmType::Normal;
    let mut flags = monitor.tripped_thresholds;
    let mut check = |threshold: &Option<AtmosAlarmThreshold>, value: f32, flag: u8| {
        let Some(threshold) = threshold else {
            return;
        };
        if threshold.ignore {
            return;
        }
        match threshold.check(value) {
            Some(found) if found > state => {
                state = found;
                flags |= flag;
            }
            None => flags &= !flag,
            _ => {}
        }
    };
    check(
        &monitor.temperature_threshold,
        air.temperature(),
        tripped::TEMPERATURE,
    );
    check(
        &monitor.pressure_threshold,
        air.pressure(),
        tripped::PRESSURE,
    );
    if let Some(gases) = &monitor.gas_thresholds {
        let total = air.total_moles();
        let mut gas_tripped = false;
        for (gas, threshold) in gases {
            let Some(index) = gas_index(gas) else {
                continue;
            };
            let ratio = if total > 0.0 {
                air.moles[index] / total
            } else {
                0.0
            };
            if let Some(found) = threshold.check(ratio)
                && found > state
            {
                state = found;
                gas_tripped = true;
            }
        }
        if gas_tripped {
            flags |= tripped::GAS;
        } else {
            flags &= !tripped::GAS;
        }
    }
    if state != monitor.last_alarm_state || flags != monitor.tripped_thresholds {
        monitor_alert(world, entity, state, Some(flags));
    }
}

/// Данные датчика для тревоги (`OnSyncPayload`).
fn monitor_data(world: &mut World, entity: Entity) -> AtmosMonitorData {
    let monitor = world.get::<AtmosMonitor>(entity).cloned();
    let air = monitored_gas(world, entity);
    let monitor = monitor.unwrap_or_else(|| {
        astrocore_data::from_node(&DataNode::map(astrocore_data::DataMap::new()))
            .expect("пустой датчик разбирается")
    });
    let Some(air) = air else {
        return AtmosMonitorData {
            pressure: 0.0,
            temperature: 0.0,
            total_moles: 0.0,
            alarm_state: AtmosAlarmType::Normal,
            gases: BTreeMap::new(),
            pressure_threshold: AtmosAlarmThreshold::default(),
            temperature_threshold: AtmosAlarmThreshold::default(),
            gas_thresholds: BTreeMap::new(),
        };
    };
    AtmosMonitorData {
        pressure: air.pressure(),
        temperature: air.temperature(),
        total_moles: air.total_moles(),
        alarm_state: monitor.last_alarm_state,
        gases: (0..GAS_COUNT)
            .map(|i| (GAS_NAMES[i].to_owned(), air.moles[i]))
            .collect(),
        pressure_threshold: monitor.pressure_threshold.unwrap_or_default(),
        temperature_threshold: monitor.temperature_threshold.unwrap_or_default(),
        gas_thresholds: monitor.gas_thresholds.unwrap_or_default(),
    }
}

/// Задать порог (`SetThreshold`).
pub fn set_threshold(
    world: &mut World,
    entity: Entity,
    kind: ThresholdKind,
    threshold: AtmosAlarmThreshold,
    gas: Option<usize>,
) {
    let Some(mut monitor) = world.get_mut::<AtmosMonitor>(entity) else {
        return;
    };
    match kind {
        ThresholdKind::Pressure => monitor.pressure_threshold = Some(threshold),
        ThresholdKind::Temperature => monitor.temperature_threshold = Some(threshold),
        ThresholdKind::Gas => {
            let (Some(gas), Some(gases)) = (gas, monitor.gas_thresholds.as_mut()) else {
                return;
            };
            gases.insert(GAS_NAMES[gas].to_owned(), threshold);
        }
    }
}

// Тревожное устройство.

/// Худшая тревога сети (`TryGetHighestAlert`).
pub fn highest_alert(world: &World, entity: Entity) -> Option<AtmosAlarmType> {
    world
        .get::<AtmosAlarmable>(entity)?
        .network_alarm_states
        .values()
        .copied()
        .max()
}

/// Сменить тревогу (`TryUpdateAlert`): вид, сирена при опасности, событие.
fn try_update_alert(world: &mut World, entity: Entity, kind: AtmosAlarmType, sync_network: bool) {
    let Some(alarmable) = world.get::<AtmosAlarmable>(entity).cloned() else {
        return;
    };
    if alarmable.last_alarm_state == kind {
        return;
    }
    if sync_network {
        sync_alerts_to_network(world, entity, None);
    }
    if let Some(mut a) = world.get_mut::<AtmosAlarmable>(entity) {
        a.last_alarm_state = kind;
    }
    world.set_appearance(entity, ALARM_TYPE, kind.name());
    if kind == AtmosAlarmType::Danger {
        let mut sound = alarmable.alarm_sound.clone();
        match &mut sound {
            SoundSpecifier::Path { params, .. } | SoundSpecifier::Collection { params, .. } => {
                params.volume += alarmable.alarm_volume;
            }
        }
        world.play_entity(&sound, entity, None);
    }
    world.raise_and_broadcast(entity, &mut AtmosAlarm { alarm_type: kind });
}

/// Разослать тревоги сети (`SyncAlertsToNetwork`).
pub fn sync_alerts_to_network(world: &mut World, entity: Entity, address: Option<i32>) {
    let Some(alarmable) = world.get::<AtmosAlarmable>(entity).cloned() else {
        return;
    };
    if alarmable.receive_only || world.get::<crate::tag::Tag>(entity).is_none() {
        return;
    }
    let payload = AtmosAlarmableSyncAlerts {
        alarm_states: alarmable.network_alarm_states,
        source: tags_of(world, entity),
    };
    send_packet(world, entity, address, payload, None, None);
}

/// Объявить тревогу самому (`ForceAlert`).
pub fn force_alert(world: &mut World, entity: Entity, kind: AtmosAlarmType) {
    if world.get::<AtmosAlarmable>(entity).is_none() || world.get::<DeviceNetwork>(entity).is_none()
    {
        return;
    }
    try_update_alert(world, entity, kind, false);
    let receive_only = world
        .get::<AtmosAlarmable>(entity)
        .is_some_and(|a| a.receive_only);
    if receive_only {
        return;
    }
    let address = own_address(world, entity);
    if let Some(mut a) = world.get_mut::<AtmosAlarmable>(entity) {
        a.network_alarm_states.insert(address, kind);
    }
    let payload = AtmosAlarmPayload {
        kind,
        tripped_thresholds: None,
        source: tags_of(world, entity),
    };
    send_packet(world, entity, None, payload, None, None);
}

/// Сбросить тревогу (`Reset`): всем датчикам — сброс.
pub fn reset(world: &mut World, entity: Entity) {
    let Some(alarmable) = world.get::<AtmosAlarmable>(entity).cloned() else {
        return;
    };
    if alarmable.last_alarm_state == AtmosAlarmType::Normal
        || world.get::<DeviceNetwork>(entity).is_none()
    {
        return;
    }
    let address = own_address(world, entity);
    if let Some(mut a) = world.get_mut::<AtmosAlarmable>(entity)
        && a.network_alarm_states.contains_key(&address)
    {
        a.network_alarm_states
            .insert(address, AtmosAlarmType::Normal);
    }
    if !alarmable.receive_only {
        send_packet(world, entity, None, AtmosMonitorReset, None, None);
    }
    try_update_alert(world, entity, AtmosAlarmType::Normal, true);
    if let Some(mut a) = world.get_mut::<AtmosAlarmable>(entity) {
        a.network_alarm_states.clear();
    }
}

fn check_tags(world: &World, entity: Entity, source: &BTreeSet<String>) -> bool {
    world
        .get::<AtmosAlarmable>(entity)
        .is_some_and(|a| !a.ignore_alarms && source.iter().any(|t| a.sync_with_tags.contains(t)))
}

/// Тревога пришла (`OnAlarmPayload`).
fn on_alarm_payload(
    world: &mut World,
    entity: Entity,
    packet: &mut DeviceNetworkPacket<AtmosAlarmPayload>,
) {
    if !check_tags(world, entity, &packet.data.source) {
        return;
    }
    let Some(mut alarmable) = world.get_mut::<AtmosAlarmable>(entity) else {
        return;
    };
    let wanted = alarmable.monitor_flags();
    let valid = packet.data.tripped_thresholds.is_none()
        || wanted == tripped::NONE
        || packet.data.tripped_thresholds.unwrap_or(0) & wanted != 0;
    match alarmable.network_alarm_states.get(&packet.sender_address) {
        None => {
            if !valid {
                return;
            }
            alarmable
                .network_alarm_states
                .insert(packet.sender_address, packet.data.kind);
        }
        Some(_) => {
            let kind = if valid {
                packet.data.kind
            } else {
                AtmosAlarmType::Normal
            };
            alarmable
                .network_alarm_states
                .insert(packet.sender_address, kind);
        }
    }
    let highest = highest_alert(world, entity).unwrap_or(AtmosAlarmType::Normal);
    try_update_alert(world, entity, highest, true);
}

// Воздушная тревога.

/// Настройки вентиляции (`SetData`).
pub fn set_vent_data(world: &mut World, alarm: Entity, address: i32, data: GasVentPumpData) {
    send_packet(
        world,
        alarm,
        Some(address),
        GasVentPumpSetData { data: data.clone() },
        None,
        None,
    );
    if let Some(mut a) = world.get_mut::<AirAlarm>(alarm) {
        a.vent_data.insert(address, data);
    }
}

/// Настройки скруббера (`SetData`).
pub fn set_scrubber_data(
    world: &mut World,
    alarm: Entity,
    address: i32,
    data: GasVentScrubberData,
) {
    send_packet(
        world,
        alarm,
        Some(address),
        GasVentScrubberSetData { data: data.clone() },
        None,
        None,
    );
    if let Some(mut a) = world.get_mut::<AirAlarm>(alarm) {
        a.scrubber_data.insert(address, data);
    }
}

/// Выполнить режим (`AirAlarmModeFactory`, `IAirAlarmMode.Execute`).
fn execute_mode(world: &mut World, alarm: Entity, mode: AirAlarmMode) {
    let Some(component) = world.get::<AirAlarm>(alarm).cloned() else {
        return;
    };
    let vents: Vec<(i32, GasVentPumpData)> = component.vent_data.into_iter().collect();
    let scrubbers: Vec<(i32, GasVentScrubberData)> = component.scrubber_data.into_iter().collect();
    for (address, mut data) in vents {
        let preset = match mode {
            AirAlarmMode::None => {
                data.enabled = false;
                data
            }
            AirAlarmMode::Filtering | AirAlarmMode::WideFiltering => GasVentPumpData::filter_mode(),
            AirAlarmMode::Fill => GasVentPumpData::fill_mode(),
            AirAlarmMode::Panic => GasVentPumpData::panic_mode(),
        };
        set_vent_data(world, alarm, address, preset);
    }
    for (address, mut data) in scrubbers {
        let preset = match mode {
            AirAlarmMode::None => {
                data.enabled = false;
                data
            }
            AirAlarmMode::Filtering => GasVentScrubberData::filter_mode(),
            AirAlarmMode::WideFiltering => GasVentScrubberData::wide_filter_mode(),
            AirAlarmMode::Fill => GasVentScrubberData::fill_mode(),
            AirAlarmMode::Panic => GasVentScrubberData::panic_mode(),
        };
        set_scrubber_data(world, alarm, address, preset);
    }
}

/// Сменить режим (`SetMode`); `ui_only` — только запомнить и разослать.
pub fn set_mode(world: &mut World, alarm: Entity, mode: AirAlarmMode, ui_only: bool) {
    let Some(mut component) = world.get_mut::<AirAlarm>(alarm) else {
        return;
    };
    let mode = if component.panic_wire_cut {
        AirAlarmMode::Panic
    } else {
        mode
    };
    component.current_mode = mode;
    if !ui_only {
        execute_mode(world, alarm, mode);
    }
    update_air_alarm_ui(world, alarm);
    // Режим остальным тревогам сети (`SyncMode`).
    if world
        .get::<AtmosMonitor>(alarm)
        .is_some_and(|m| !m.net_enabled)
    {
        return;
    }
    send_packet(world, alarm, None, AirAlarmSetMode { mode }, None, None);
}

/// Окно воздушной тревоги (`UpdateUI`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AirAlarmUiState {
    pub address: String,
    pub mode: AirAlarmMode,
    pub auto_mode: bool,
    pub alarm_type: AtmosAlarmType,
    pub pressure: f32,
    pub temperature: f32,
    pub total_moles: f32,
    pub sensors: Vec<(String, AtmosMonitorData)>,
    pub vents: Vec<(String, GasVentPumpData)>,
    pub scrubbers: Vec<(String, GasVentScrubberData)>,
}

/// Сменить режим (`AirAlarmUpdateAlarmModeMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AirAlarmUpdateAlarmMode {
    pub mode: AirAlarmMode,
}

/// Автоматический режим (`AirAlarmUpdateAutoModeMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AirAlarmUpdateAutoMode {
    pub enabled: bool,
}

/// Опросить все устройства заново (`AirAlarmResyncAllDevicesMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AirAlarmResyncAllDevices;

fn address_text(address: i32) -> String {
    crate::device_network::address_string(address)
}

/// Состояние окна.
pub fn update_air_alarm_ui(world: &mut World, alarm: Entity) {
    let Some(component) = world.get::<AirAlarm>(alarm).cloned() else {
        return;
    };
    let air = monitored_gas(world, alarm);
    let state = AirAlarmUiState {
        address: address_of(world, alarm),
        mode: component.current_mode,
        auto_mode: component.auto_mode,
        alarm_type: component.state,
        pressure: air.as_ref().map_or(0.0, GasMixture::pressure),
        temperature: air.as_ref().map_or(0.0, GasMixture::temperature),
        total_moles: air.as_ref().map_or(0.0, GasMixture::total_moles),
        sensors: component
            .sensor_data
            .into_iter()
            .map(|(a, d)| (address_text(a), d))
            .collect(),
        vents: component
            .vent_data
            .into_iter()
            .map(|(a, d)| (address_text(a), d))
            .collect(),
        scrubbers: component
            .scrubber_data
            .into_iter()
            .map(|(a, d)| (address_text(a), d))
            .collect(),
    };
    world.set_ui_state(alarm, AIR_ALARM_UI_KEY, &state);
}

/// Доступ к тревоге (`AccessCheck`).
fn access_check(world: &mut World, alarm: Entity, user: Entity) -> bool {
    if world.get::<crate::access::AccessReader>(alarm).is_none() {
        return true;
    }
    if crate::access::is_allowed(world, user, alarm) {
        return true;
    }
    let text = crate::examine::loc(world, "air-alarm-ui-access-denied", &[]);
    crate::popups::PopupWorldExt::popup_entity_for(
        world,
        &text,
        user,
        user,
        crate::popups::PopupType::Small,
    );
    false
}

fn clear_air_alarm_devices(world: &mut World, alarm: Entity) {
    if let Some(mut a) = world.get_mut::<AirAlarm>(alarm) {
        a.known_devices.clear();
        a.vent_data.clear();
        a.scrubber_data.clear();
        a.sensor_data.clear();
    }
}

// Вентиляции и скрубберы в сети.

fn vent_to_data(vent: &GasVentPump) -> GasVentPumpData {
    GasVentPumpData {
        enabled: vent.enabled,
        dirty: false,
        ignore_alarms: false,
        releasing: vent.pump_direction == VentPumpDirection::Releasing,
        pressure_checks: vent.pressure_checks.clone(),
        external_pressure_bound: vent.external_pressure_bound,
        internal_pressure_bound: vent.internal_pressure_bound,
        pressure_lockout_override: vent.pressure_lockout_override,
    }
}

fn vent_from_data(vent: &mut GasVentPump, data: &GasVentPumpData) {
    vent.enabled = data.enabled;
    vent.pump_direction = if data.releasing {
        VentPumpDirection::Releasing
    } else {
        VentPumpDirection::Siphoning
    };
    vent.pressure_checks = data.pressure_checks.clone();
    vent.external_pressure_bound = data.external_pressure_bound;
    vent.internal_pressure_bound = data.internal_pressure_bound;
    vent.pressure_lockout_override = data.pressure_lockout_override;
}

fn scrubber_to_data(scrubber: &GasVentScrubber) -> GasVentScrubberData {
    GasVentScrubberData {
        enabled: scrubber.enabled,
        dirty: false,
        ignore_alarms: false,
        filter_gases: scrubber.filter_gases.clone(),
        scrubbing: scrubber.pump_direction == ScrubberPumpDirection::Scrubbing,
        volume_rate: scrubber.transfer_rate,
        wide_net: scrubber.wide_net,
    }
}

fn scrubber_from_data(scrubber: &mut GasVentScrubber, data: &GasVentScrubberData) {
    scrubber.enabled = data.enabled;
    scrubber.filter_gases = data.filter_gases.clone();
    scrubber.pump_direction = if data.scrubbing {
        ScrubberPumpDirection::Scrubbing
    } else {
        ScrubberPumpDirection::Siphoning
    };
    scrubber.transfer_rate = data.volume_rate.clamp(0.0, scrubber.max_transfer_rate);
    scrubber.wide_net = data.wide_net;
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<AtmosMonitor>();
    world.register_data_component::<AtmosAlarmable>();
    world.register_data_component::<FireAlarm>();
    world.register_data_component::<AirAlarm>();
    world.register_data_component::<DeviceNetworkRequiresPower>();
    world.register_ui_message::<AirAlarmUpdateAlarmMode>();
    world.register_ui_message::<AirAlarmUpdateAutoMode>();
    world.register_ui_message::<AirAlarmResyncAllDevices>();
    if !side.is_server() {
        return;
    }
    // Сеть только с питанием (`DeviceNetworkRequiresPower`).
    world.subscribe::<DeviceNetworkRequiresPower, BeforePacketSent>(|world, entity, event| {
        if !is_powered(world, entity) {
            event.cancelled = true;
        }
    });

    // Датчик.
    world.subscribe::<AtmosMonitor, MapInit>(|world, entity, _| {
        let Some(monitor) = world.get::<AtmosMonitor>(entity).cloned() else {
            return;
        };
        let temperature = monitor
            .temperature_threshold
            .or_else(|| threshold_prototype(world, monitor.temperature_threshold_id.as_deref()?));
        let pressure = monitor
            .pressure_threshold
            .or_else(|| threshold_prototype(world, monitor.pressure_threshold_id.as_deref()?));
        let gases = monitor.gas_threshold_prototypes.as_ref().map(|protos| {
            let mut gases = monitor.gas_thresholds.clone().unwrap_or_default();
            for (gas, id) in protos {
                if let Some(t) = threshold_prototype(world, id) {
                    gases.entry(gas.clone()).or_insert(t);
                }
            }
            gases
        });
        if let Some(mut m) = world.get_mut::<AtmosMonitor>(entity) {
            m.temperature_threshold = temperature;
            m.pressure_threshold = pressure;
            if gases.is_some() {
                m.gas_thresholds = gases;
            }
            m.last_alarm_state = AtmosAlarmType::Normal;
        }
    });
    world.subscribe::<AtmosMonitor, AtmosDeviceUpdate>(|world, entity, _| {
        if !is_powered(world, entity) {
            return;
        }
        let has_thresholds = world.get::<AtmosMonitor>(entity).is_some_and(|m| {
            m.temperature_threshold.is_some()
                || m.pressure_threshold.is_some()
                || m.gas_thresholds.is_some()
        });
        if !has_thresholds {
            return;
        }
        if let Some(air) = monitored_gas(world, entity) {
            update_monitor_state(world, entity, &air);
        }
    });
    world.subscribe::<AtmosMonitor, TileFire>(|world, entity, event| {
        if !is_powered(world, entity) {
            return;
        }
        let Some(monitor) = world.get::<AtmosMonitor>(entity).cloned() else {
            return;
        };
        let flags = monitor.tripped_thresholds | tripped::TEMPERATURE;
        if monitor.monitor_fire && monitor.last_alarm_state != AtmosAlarmType::Danger {
            monitor_alert(world, entity, AtmosAlarmType::Danger, Some(flags));
        }
        let last = world
            .get::<AtmosMonitor>(entity)
            .map_or(monitor.last_alarm_state, |m| m.last_alarm_state);
        if let Some(threshold) = monitor.temperature_threshold
            && threshold.check(event.temperature).is_some_and(|s| s > last)
        {
            monitor_alert(world, entity, AtmosAlarmType::Danger, Some(flags));
        }
    });
    world.subscribe::<AtmosMonitor, BeforePacketSent>(|world, entity, event| {
        if world
            .get::<AtmosMonitor>(entity)
            .is_some_and(|m| !m.net_enabled)
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<AtmosMonitor, DeviceNetworkPacket<AtmosMonitorRegisterDevice>>(
        |world, entity, packet| {
            if let Some(mut m) = world.get_mut::<AtmosMonitor>(entity) {
                m.registered_devices.insert(packet.sender_address);
            }
        },
    );
    world.subscribe::<AtmosMonitor, DeviceNetworkPacket<AtmosMonitorDeregisterDevice>>(
        |world, entity, packet| {
            if let Some(mut m) = world.get_mut::<AtmosMonitor>(entity) {
                m.registered_devices.remove(&packet.sender_address);
            }
        },
    );
    world.subscribe::<AtmosMonitor, DeviceNetworkPacket<AtmosMonitorReset>>(|world, entity, _| {
        monitor_alert(world, entity, AtmosAlarmType::Normal, None);
    });
    world.subscribe::<AtmosMonitor, DeviceNetworkPacket<AtmosMonitorSetThreshold>>(
        |world, entity, packet| {
            let data = packet.data.clone();
            set_threshold(world, entity, data.kind, data.threshold, data.gas);
        },
    );
    world.subscribe::<AtmosMonitor, DeviceNetworkPacket<AtmosSync>>(|world, entity, packet| {
        // Вентиляции и скрубберы отвечают своими данными, остальные датчики — показаниями.
        if world.get::<GasVentPump>(entity).is_some()
            || world.get::<GasVentScrubber>(entity).is_some()
        {
            return;
        }
        let data = monitor_data(world, entity);
        let sender = packet.sender_address;
        send_packet(
            world,
            entity,
            Some(sender),
            AirAlarmSetData {
                payload: AtmosDeviceData::Sensor(data),
            },
            None,
            None,
        );
        let state = world
            .get::<AtmosMonitor>(entity)
            .map_or(AtmosAlarmType::Normal, |m| m.last_alarm_state);
        monitor_alert(world, entity, state, None);
    });
    world.subscribe::<AtmosMonitor, PowerChanged>(|world, entity, event| {
        if event.powered {
            let state = world
                .get::<AtmosMonitor>(entity)
                .map_or(AtmosAlarmType::Normal, |m| m.last_alarm_state);
            monitor_alert(world, entity, state, None);
        }
    });

    // Тревожное устройство.
    world.subscribe::<AtmosAlarmable, MapInit>(|world, entity, _| {
        let highest = highest_alert(world, entity).unwrap_or(AtmosAlarmType::Normal);
        try_update_alert(world, entity, highest, false);
    });
    world.subscribe::<AtmosAlarmable, PowerChanged>(|world, entity, event| {
        if !event.powered {
            reset(world, entity);
        } else {
            register(world, entity, None);
            sync(world, entity, None);
            let highest = highest_alert(world, entity).unwrap_or(AtmosAlarmType::Normal);
            try_update_alert(world, entity, highest, false);
        }
    });
    world.subscribe::<AtmosAlarmable, DeviceNetworkPacket<AtmosAlarmPayload>>(on_alarm_payload);
    world.subscribe::<AtmosAlarmable, DeviceNetworkPacket<AtmosAlarmableResetAll>>(
        |world, entity, packet| {
            if check_tags(world, entity, &packet.data.source) {
                reset(world, entity);
            }
        },
    );
    world.subscribe::<AtmosAlarmable, DeviceNetworkPacket<AtmosAlarmableSyncAlerts>>(
        |world, entity, packet| {
            if !check_tags(world, entity, &packet.data.source) {
                return;
            }
            if let Some(mut a) = world.get_mut::<AtmosAlarmable>(entity) {
                for (address, kind) in &packet.data.alarm_states {
                    a.network_alarm_states.insert(*address, *kind);
                }
            }
            if let Some(highest) = highest_alert(world, entity) {
                try_update_alert(world, entity, highest, true);
            }
        },
    );

    // Пожарная тревога: рукой включить или сбросить (`OnInteractHand`).
    world.subscribe::<FireAlarm, crate::interaction::InteractHand>(|world, entity, event| {
        if !crate::interaction::InteractionWorldExt::in_range_unobstructed(
            world,
            event.user,
            event.target,
            crate::interaction::INTERACTION_RANGE,
        ) {
            return;
        }
        if world.get::<crate::access::AccessReader>(entity).is_some()
            && !crate::access::is_allowed(world, event.user, entity)
        {
            return;
        }
        if !is_powered(world, entity) {
            return;
        }
        let alarm = highest_alert(world, entity).unwrap_or(AtmosAlarmType::Normal);
        if alarm == AtmosAlarmType::Normal {
            force_alert(world, entity, AtmosAlarmType::Danger);
        } else if !world
            .get::<AtmosAlarmable>(entity)
            .is_some_and(|a| a.receive_only)
        {
            reset(world, entity);
        }
    });
    world.subscribe::<FireAlarm, DeviceListUpdate>(|world, entity, event| {
        for device in &event.old_devices {
            if let Some(address) = world.get::<DeviceNetwork>(*device).map(|d| d.address) {
                deregister(world, entity, Some(address));
            }
        }
        register(world, entity, None);
        sync(world, entity, None);
    });

    // Воздушная тревога.
    world.subscribe::<AirAlarm, DeviceListUpdate>(|world, entity, event| {
        for device in &event.old_devices {
            if let Some(address) = world.get::<DeviceNetwork>(*device).map(|d| d.address) {
                deregister(world, entity, Some(address));
            }
        }
        clear_air_alarm_devices(world, entity);
        update_air_alarm_ui(world, entity);
        register(world, entity, None);
        sync(world, entity, None);
    });
    world.subscribe::<AirAlarm, PowerChanged>(|world, entity, event| {
        if event.powered {
            return;
        }
        crate::activatable_ui::close_all(world, entity);
        clear_air_alarm_devices(world, entity);
    });
    world.subscribe::<AirAlarm, AtmosAlarm>(|world, entity, event| {
        sync(world, entity, None);
        let auto = world.get::<AirAlarm>(entity).is_some_and(|a| a.auto_mode);
        if auto {
            match event.alarm_type {
                AtmosAlarmType::Danger => {
                    set_mode(world, entity, AirAlarmMode::WideFiltering, false)
                }
                AtmosAlarmType::Normal | AtmosAlarmType::Warning => {
                    set_mode(world, entity, AirAlarmMode::Filtering, false)
                }
                _ => {}
            }
        }
        let previous = world.get::<AirAlarm>(entity).map(|a| a.state);
        if let Some(previous) = previous
            && previous != event.alarm_type
        {
            // Низкий — на порт прежнего состояния, высокий — на новый.
            crate::device_linking::send_signal(world, entity, alarm_port(previous), false);
            if let Some(mut a) = world.get_mut::<AirAlarm>(entity) {
                a.state = event.alarm_type;
            }
            crate::device_linking::send_signal(world, entity, alarm_port(event.alarm_type), true);
        }
        update_air_alarm_ui(world, entity);
    });
    world.subscribe::<AirAlarm, astrocore_ecs::ComponentInit>(|world, entity, _| {
        crate::device_linking::ensure_source_ports(
            world,
            entity,
            &[AIR_DANGER_PORT, AIR_WARNING_PORT, AIR_NORMAL_PORT],
        );
    });
    // Связанная на карте тревога начинает с высокого уровня: первая смена даст низкий.
    world.subscribe::<AirAlarm, MapInit>(|world, entity, _| {
        let Some(state) = world.get::<AirAlarm>(entity).map(|a| a.state) else {
            return;
        };
        crate::device_linking::send_signal(world, entity, alarm_port(state), true);
    });
    world.subscribe::<AirAlarm, DeviceNetworkPacket<AirAlarmSetData>>(|world, entity, packet| {
        let address = packet.sender_address;
        let Some(mut alarm) = world.get_mut::<AirAlarm>(entity) else {
            return;
        };
        match &packet.data.payload {
            AtmosDeviceData::Vent(data) => {
                alarm.vent_data.insert(address, data.clone());
            }
            AtmosDeviceData::Scrubber(data) => {
                alarm.scrubber_data.insert(address, data.clone());
            }
            AtmosDeviceData::Sensor(data) => {
                alarm.sensor_data.insert(address, data.clone());
            }
        }
        alarm.known_devices.insert(address);
        update_air_alarm_ui(world, entity);
    });
    world.subscribe::<AirAlarm, DeviceNetworkPacket<AirAlarmSetMode>>(|world, entity, packet| {
        set_mode(world, entity, packet.data.mode, false);
    });
    world.subscribe::<AirAlarm, astrocore_ui::BoundUiOpened>(|world, entity, event| {
        if event.key != AIR_ALARM_UI_KEY {
            return;
        }
        sync(world, entity, None);
        update_air_alarm_ui(world, entity);
    });
    world.subscribe::<AirAlarm, BoundUiMessage<AirAlarmUpdateAlarmMode>>(|world, entity, event| {
        if access_check(world, entity, event.actor) {
            set_mode(world, entity, event.message.mode, false);
        } else {
            update_air_alarm_ui(world, entity);
        }
    });
    world.subscribe::<AirAlarm, BoundUiMessage<AirAlarmUpdateAutoMode>>(|world, entity, event| {
        if access_check(world, entity, event.actor)
            && let Some(mut a) = world.get_mut::<AirAlarm>(entity)
        {
            a.auto_mode = event.message.enabled;
        }
        update_air_alarm_ui(world, entity);
    });
    world.subscribe::<AirAlarm, BoundUiMessage<AirAlarmResyncAllDevices>>(
        |world, entity, event| {
            if !access_check(world, entity, event.actor) {
                return;
            }
            clear_air_alarm_devices(world, entity);
            register(world, entity, None);
            sync(world, entity, None);
        },
    );

    // Вентиляции и скрубберы: данные тревоге и её настройки (`OnSyncPayload`,
    // `OnSetPayload`), опасность выключает, норма включает (`OnAtmosAlarm`).
    world.subscribe::<GasVentPump, DeviceNetworkPacket<AtmosSync>>(|world, entity, packet| {
        let Some(data) = world.get::<GasVentPump>(entity).map(vent_to_data) else {
            return;
        };
        let sender = packet.sender_address;
        send_packet(
            world,
            entity,
            Some(sender),
            AirAlarmSetData {
                payload: AtmosDeviceData::Vent(data),
            },
            None,
            None,
        );
    });
    world.subscribe::<GasVentPump, DeviceNetworkPacket<GasVentPumpSetData>>(
        |world, entity, packet| {
            if let Some(mut vent) = world.get_mut::<GasVentPump>(entity) {
                vent_from_data(&mut vent, &packet.data.data);
            }
        },
    );
    world.subscribe::<GasVentPump, AtmosAlarm>(|world, entity, event| {
        if let Some(mut vent) = world.get_mut::<GasVentPump>(entity) {
            match event.alarm_type {
                AtmosAlarmType::Danger => vent.enabled = false,
                AtmosAlarmType::Normal => vent.enabled = true,
                _ => {}
            }
        }
    });
    world.subscribe::<GasVentScrubber, DeviceNetworkPacket<AtmosSync>>(|world, entity, packet| {
        let Some(data) = world.get::<GasVentScrubber>(entity).map(scrubber_to_data) else {
            return;
        };
        let sender = packet.sender_address;
        send_packet(
            world,
            entity,
            Some(sender),
            AirAlarmSetData {
                payload: AtmosDeviceData::Scrubber(data),
            },
            None,
            None,
        );
    });
    world.subscribe::<GasVentScrubber, DeviceNetworkPacket<GasVentScrubberSetData>>(
        |world, entity, packet| {
            if let Some(mut scrubber) = world.get_mut::<GasVentScrubber>(entity) {
                scrubber_from_data(&mut scrubber, &packet.data.data);
            }
        },
    );
    world.subscribe::<GasVentScrubber, AtmosAlarm>(|world, entity, event| {
        if let Some(mut scrubber) = world.get_mut::<GasVentScrubber>(entity) {
            match event.alarm_type {
                AtmosAlarmType::Danger => scrubber.enabled = false,
                AtmosAlarmType::Normal => scrubber.enabled = true,
                _ => {}
            }
        }
    });
}

#[cfg(test)]
pub(crate) mod tests;
