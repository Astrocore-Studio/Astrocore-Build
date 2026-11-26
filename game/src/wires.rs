//! Провода и панели техобслуживания: отвёрткой открыть панель, кусачками резать и чинить
//! провода, мультитулом подавать импульс. Образец — `WiresComponent`, `WiresPanelComponent`,
//! `WiresPanelSecurityComponent`, `ActivatableUIRequiresPanelComponent`, `WireLayoutPrototype`,
//! `WiresSystem`, `SharedWiresSystem`, `BaseWireAction`, `PowerWireAction`, `AccessWireAction`,
//! `LogWireAction` и провода шлюзов (`DoorBoltWireAction`, `DoorBoltLightWireAction`,
//! `DoorTimingWireAction`, `DoorSafetyWireAction`) SS14 @ 2f4a2aad.
//!
//! Раскладка (цвет и буква каждого провода) одна на весь раунд для каждого вида раскладки:
//! узнав провода одного шлюза, инженер знает провода всех. Провода остальных видов
//! (торговые автоматы, камеры, ИИ) пока режутся и чинятся без последствий.

use std::collections::{BTreeMap, HashMap};

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_data::DataNode;
use astrocore_ecs::{
    ComponentInit, EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update,
};
use astrocore_net::AppearanceWorldExt;
use astrocore_prototypes::Prototypes;
use astrocore_ui::{BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::access::AccessReader;
use crate::activatable_ui::ActivatableUiOpenAttempt;
use crate::do_after::DoAfterDone;
use crate::doors::{
    Airlock, DoorBolt, set_auto_close_delay_modifier, set_bolt_lights_enabled, set_bolt_wire_cut,
    set_safety, try_set_bolts_down,
};
use crate::electrocution::{
    Electrified, set_electrified, set_electrified_wire_cut, try_do_electrified_act,
};
use crate::examine::{Examined, loc};
use crate::hands::HandsWorldExt;
use crate::interaction::{INTERACTION_RANGE, InteractUsing, InteractionWorldExt};
use crate::popups::{PopupType, PopupWorldExt};
use crate::power::{PowerChanged, is_powered, set_power_disabled};
use crate::random::random;
use crate::tools::{Tool, ToolWorldExt, register_tool_event};
use crate::verbs::{GetVerbs, Verb, VerbKind};

/// Ключ окна проводов (`WiresUiKey.Key`).
pub const WIRES_UI_KEY: &str = "enum.WiresUiKey.Key";
/// Внешний вид: панель открыта (`WiresVisuals.MaintenancePanelState`).
pub const PANEL_STATE: &str = "enum.WiresVisuals.MaintenancePanelState";

const CUTTING: &str = "Cutting";
const PULSING: &str = "Pulsing";

fn yes() -> bool {
    true
}
fn one_second() -> f64 {
    1.0
}
fn screwing() -> String {
    "Screwing".into()
}
fn open_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/screwdriveropen.ogg")
}
fn close_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/screwdriverclose.ogg")
}
fn examine_closed() -> Option<String> {
    Some("wires-panel-component-on-examine-closed".into())
}
fn examine_open() -> Option<String> {
    Some("wires-panel-component-on-examine-open".into())
}
fn default_board() -> String {
    "wires-board-name-default".into()
}
fn pulse_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Effects/multitool_pulse.ogg")
}

/// Панель техобслуживания (`WiresPanelComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WiresPanel {
    #[serde(default)]
    pub open: bool,
    /// Видна ли панель (у открытого шлюза — нет).
    #[serde(skip, default = "yes")]
    pub visible: bool,
    #[serde(default = "open_sound")]
    pub screwdriver_open_sound: SoundSpecifier,
    #[serde(default = "close_sound")]
    pub screwdriver_close_sound: SoundSpecifier,
    /// Секунды.
    #[serde(default = "one_second")]
    pub open_delay: f64,
    #[serde(default = "screwing")]
    pub opening_tool: String,
    #[serde(default = "examine_closed")]
    pub examine_text_closed: Option<String>,
    #[serde(default = "examine_open")]
    pub examine_text_open: Option<String>,
}

/// Защита панели (`WiresPanelSecurityComponent`): до вскрытия по графу строительства
/// провода недоступны.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WiresPanelSecurity {
    #[serde(default)]
    pub examine: Option<String>,
    #[serde(default = "yes")]
    pub wires_accessible: bool,
    #[serde(default)]
    pub security_level: String,
}

/// Окно открывается только при открытой (или закрытой) панели
/// (`ActivatableUIRequiresPanelComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivatableUiRequiresPanel {
    #[serde(default = "yes")]
    pub require_open: bool,
}

/// Цвет провода (`WireColor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WireColor {
    Red,
    Blue,
    Green,
    Orange,
    Brown,
    Gold,
    Gray,
    Cyan,
    Navy,
    Purple,
    Pink,
    Fuchsia,
}

impl WireColor {
    pub const ALL: [WireColor; 12] = [
        Self::Red,
        Self::Blue,
        Self::Green,
        Self::Orange,
        Self::Brown,
        Self::Gold,
        Self::Gray,
        Self::Cyan,
        Self::Navy,
        Self::Purple,
        Self::Pink,
        Self::Fuchsia,
    ];

    /// Ключ перевода названия (`wire-name-color-red`).
    pub fn loc_id(self) -> String {
        format!("wire-name-color-{}", format!("{self:?}").to_lowercase())
    }

    /// Цвет для окна, RGB (`ColorValue`).
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Self::Red => [255, 0, 0],
            Self::Blue => [0, 0, 255],
            Self::Green => [50, 205, 50],
            Self::Orange => [255, 165, 0],
            Self::Brown => [165, 42, 42],
            Self::Gold => [255, 215, 0],
            Self::Gray => [128, 128, 128],
            Self::Cyan => [0, 255, 255],
            Self::Navy => [0, 0, 128],
            Self::Purple => [128, 0, 128],
            Self::Pink => [255, 192, 203],
            Self::Fuchsia => [255, 0, 255],
        }
    }
}

/// Буквы проводов (`WireLetter`): греческий алфавит.
pub const WIRE_LETTERS: [char; 24] = [
    'α', 'β', 'γ', 'δ', 'ε', 'ζ', 'η', 'θ', 'ι', 'κ', 'λ', 'μ', 'ν', 'ξ', 'ο', 'π', 'ρ', 'σ', 'τ',
    'υ', 'φ', 'χ', 'ψ', 'ω',
];

/// Действие провода (`IWireAction` и его виды).
#[derive(Debug, Clone, PartialEq)]
pub enum WireAction {
    /// Питание (`PowerWireAction`).
    Power { pulse_timeout: f64 },
    /// Болты (`DoorBoltWireAction`).
    DoorBolt,
    /// Огни болтов (`DoorBoltLightWireAction`).
    DoorBoltLight,
    /// Таймер автозакрытия (`DoorTimingWireAction`).
    DoorTiming { timeout: f64 },
    /// Защита от закрытия на людей (`DoorSafetyWireAction`).
    DoorSafety { timeout: f64 },
    /// Считыватель доступа (`AccessWireAction`).
    Access { pulse_timeout: f64 },
    /// Журнал доступа (`LogWireAction`).
    Log { pulse_timeout: f64 },
    /// Действие, которого ещё нет: режется без последствий.
    Other(String),
}

fn seconds_field(node: &DataNode, key: &str, default: f64) -> f64 {
    node.get(key)
        .and_then(DataNode::as_str)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

impl WireAction {
    /// Из записи раскладки с тегом `!type:`.
    pub fn from_node(node: &DataNode) -> Option<Self> {
        let name = node.type_tag()?;
        Some(match name {
            "PowerWireAction" => Self::Power {
                pulse_timeout: seconds_field(node, "pulseTimeout", 30.0),
            },
            "DoorBoltWireAction" => Self::DoorBolt,
            "DoorBoltLightWireAction" => Self::DoorBoltLight,
            "DoorTimingWireAction" => Self::DoorTiming {
                timeout: seconds_field(node, "timeout", 30.0),
            },
            "DoorSafetyWireAction" => Self::DoorSafety {
                timeout: seconds_field(node, "timeout", 30.0),
            },
            "AccessWireAction" => Self::Access {
                pulse_timeout: seconds_field(node, "pulseTimeout", 30.0),
            },
            "LogWireAction" => Self::Log {
                pulse_timeout: seconds_field(node, "pulseTimeout", 30.0),
            },
            other => Self::Other(other.to_owned()),
        })
    }

    /// Вид действия: провода одного вида нумеруются вместе (`AddWire(wire, count)`).
    fn kind(&self) -> &str {
        match self {
            Self::Power { .. } => "Power",
            Self::DoorBolt => "DoorBolt",
            Self::DoorBoltLight => "DoorBoltLight",
            Self::DoorTiming { .. } => "DoorTiming",
            Self::DoorSafety { .. } => "DoorSafety",
            Self::Access { .. } => "Access",
            Self::Log { .. } => "Log",
            Self::Other(name) => name,
        }
    }

    /// Название и цвет лампочки (`Name`, `Color`).
    fn light(&self) -> Option<(&'static str, [u8; 3], &'static str)> {
        Some(match self {
            Self::Power { .. } => ("wire-name-power", [255, 0, 0], "PowerWireActionKey.Status"),
            Self::DoorBolt => (
                "wire-name-door-bolt",
                [255, 0, 0],
                "AirlockWireStatus.BoltIndicator",
            ),
            Self::DoorBoltLight => (
                "wire-name-bolt-light",
                [0, 255, 0],
                "AirlockWireStatus.BoltLightIndicator",
            ),
            Self::DoorTiming { .. } => (
                "wire-name-door-timer",
                [255, 165, 0],
                "AirlockWireStatus.TimingIndicator",
            ),
            Self::DoorSafety { .. } => (
                "wire-name-door-safety",
                [255, 0, 0],
                "AirlockWireStatus.SafetyIndicator",
            ),
            Self::Access { .. } => (
                "wire-name-access",
                [0, 128, 0],
                "AccessWireActionKey.Status",
            ),
            Self::Log { .. } => ("wire-name-log", [0, 0, 255], "LogWireActionKey.Status"),
            Self::Other(_) => return None,
        })
    }
}

/// Провод (`Wire`).
#[derive(Debug, Clone, PartialEq)]
pub struct Wire {
    pub id: usize,
    pub original_position: usize,
    pub is_cut: bool,
    pub color: WireColor,
    pub letter: usize,
    pub action: Option<WireAction>,
}

/// Состояние лампочки (`StatusLightState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusLightState {
    Off,
    On,
    BlinkingFast,
    BlinkingSlow,
}

/// Лампочка в окне (`StatusLightData`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusLight {
    pub color: [u8; 3],
    pub state: StatusLightState,
    pub text: String,
}

/// Отложенное действие провода (`StartWireAction`, `TimedWireEvent`).
#[derive(Debug, Clone, PartialEq)]
pub struct WireTimer {
    pub key: &'static str,
    pub until: f64,
    pub wire: usize,
    /// Отменено: действие выполнится на следующем такте (`TryCancelWireAction`).
    pub cancelled: bool,
}

/// Данные провода питания (`PowerWireActionKey`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PowerWireData {
    pub cut_wires: Option<i32>,
    pub wire_count: Option<i32>,
    pub main_wire: Option<usize>,
    pub pulsed: Option<bool>,
}

/// Провода устройства (`WiresComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Wires {
    #[serde(default = "default_board")]
    pub board_name: String,
    #[serde(default)]
    pub layout_id: String,
    #[serde(default)]
    pub always_randomize: bool,
    #[serde(default = "pulse_sound")]
    pub pulse_sound: SoundSpecifier,
    #[serde(skip)]
    pub serial_number: Option<String>,
    #[serde(skip)]
    pub wire_seed: i32,
    #[serde(skip)]
    pub wires: Vec<Wire>,
    /// Провода, над которыми идёт действие (`WiresQueue`).
    #[serde(skip)]
    pub queue: Vec<usize>,
    /// Лампочки по ключу: место и состояние (`Statuses`).
    #[serde(skip)]
    pub statuses: BTreeMap<String, (usize, StatusLight)>,
    #[serde(skip)]
    pub power: PowerWireData,
    #[serde(skip)]
    pub timers: Vec<WireTimer>,
}

/// Провод в окне (`ClientWire`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientWire {
    pub id: usize,
    pub is_cut: bool,
    pub color: WireColor,
    pub letter: char,
}

/// Состояние окна (`WiresBoundUserInterfaceState`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WiresUiState {
    pub wires: Vec<ClientWire>,
    pub statuses: Vec<StatusLight>,
    pub board_name: String,
    pub serial_number: Option<String>,
    pub wire_seed: i32,
}

/// Что сделать с проводом (`WiresAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WiresAction {
    Mend,
    Cut,
    Pulse,
}

/// Сообщение окна (`WiresActionMessage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WiresActionMessage {
    pub id: usize,
    pub action: WiresAction,
}

/// Панель открыта или закрыта (`PanelChangedEvent`).
#[derive(Debug, Clone, Copy)]
pub struct PanelChanged {
    pub open: bool,
}

/// Можно ли менять панель (`AttemptChangePanelEvent`).
#[derive(Debug, Clone, Copy)]
pub struct AttemptChangePanel {
    pub open: bool,
    pub user: Option<Entity>,
    pub cancelled: bool,
}

/// Инструмент открывает провода без панели (`PanelOverrideEvent`).
#[derive(Debug, Clone, Copy)]
pub struct PanelOverride {
    pub allowed: bool,
}

/// Отвёртка докрутила панель (`WirePanelDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
pub struct WirePanelDoAfter;

/// Место провода в раскладке (`WireLayout.WireData`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct WireData {
    letter: usize,
    color: WireColor,
    position: usize,
}

/// Раскладки раунда по виду (`_layouts`). Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct WireLayouts(HashMap<String, HashMap<usize, WireData>>);

// Панель.

/// Открыта ли панель (`IsPanelOpen`). Без панели — открыта.
pub fn is_panel_open(world: &mut World, entity: Entity, tool: Option<Entity>) -> bool {
    if world.get::<WiresPanel>(entity).is_none() {
        return true;
    }
    if let Some(tool) = tool {
        let mut event = PanelOverride { allowed: true };
        world.raise(tool, &mut event);
        if event.allowed {
            return true;
        }
    }
    if world
        .get::<WiresPanelSecurity>(entity)
        .is_some_and(|s| !s.wires_accessible)
    {
        return false;
    }
    world.get::<WiresPanel>(entity).is_some_and(|p| p.open)
}

fn update_panel_appearance(world: &mut World, entity: Entity) {
    if let Some(open) = world.get::<WiresPanel>(entity).map(|p| p.open && p.visible) {
        world.set_appearance(entity, PANEL_STATE, open);
    }
}

/// Скрыть или показать панель (`ChangePanelVisibility`).
pub fn change_panel_visibility(world: &mut World, entity: Entity, visible: bool) {
    if let Some(mut panel) = world.get_mut::<WiresPanel>(entity) {
        panel.visible = visible;
    }
    update_panel_appearance(world, entity);
}

fn can_toggle_panel(world: &mut World, entity: Entity, user: Option<Entity>) -> bool {
    let Some(open) = world.get::<WiresPanel>(entity).map(|p| p.open) else {
        return false;
    };
    let mut attempt = AttemptChangePanel {
        open,
        user,
        cancelled: false,
    };
    world.raise(entity, &mut attempt);
    !attempt.cancelled
}

/// Открыть или закрыть панель (`TogglePanel`).
pub fn toggle_panel(world: &mut World, entity: Entity, open: bool, user: Option<Entity>) -> bool {
    if !can_toggle_panel(world, entity, user) {
        return false;
    }
    if let Some(mut panel) = world.get_mut::<WiresPanel>(entity) {
        panel.open = open;
    }
    update_panel_appearance(world, entity);
    world.raise(entity, &mut PanelChanged { open });
    true
}

// Провода.

/// Действие провода по месту (`TryGetWire`).
fn wire(world: &World, entity: Entity, id: usize) -> Option<Wire> {
    world.get::<Wires>(entity)?.wires.get(id).cloned()
}

fn with_wires<R>(world: &mut World, entity: Entity, f: impl FnOnce(&mut Wires) -> R) -> Option<R> {
    world.get_mut::<Wires>(entity).map(|mut w| f(&mut w))
}

/// Есть ли отложенное действие с ключом (`HasData` для токена).
fn has_timer(world: &World, entity: Entity, key: &str) -> bool {
    world
        .get::<Wires>(entity)
        .is_some_and(|w| w.timers.iter().any(|t| t.key == key))
}

/// Отменить отложенное действие (`TryCancelWireAction`): оно выполнится на следующем такте.
pub fn cancel_wire_action(world: &mut World, entity: Entity, key: &str) -> bool {
    with_wires(world, entity, |wires| {
        let mut found = false;
        for timer in wires.timers.iter_mut().filter(|t| t.key == key) {
            timer.cancelled = true;
            found = true;
        }
        found
    })
    .unwrap_or(false)
}

/// Отложить действие (`StartWireAction`). Повторный запуск с тем же ключом ничего не делает.
pub fn start_wire_action(
    world: &mut World,
    entity: Entity,
    delay: f64,
    key: &'static str,
    wire: usize,
) {
    if has_timer(world, entity, key) {
        return;
    }
    let until = world.resource::<GameTime>().cur_time().as_secs_f64() + delay;
    with_wires(world, entity, |wires| {
        wires.timers.push(WireTimer {
            key,
            until,
            wire,
            cancelled: false,
        })
    });
}

const POWER_PULSE_CANCEL: &str = "PowerWireActionKey.PulseCancel";
const POWER_ELECTRIFIED_CANCEL: &str = "PowerWireActionKey.ElectrifiedCancel";
const SAFETY_TIMEOUT: &str = "DoorSafetyWireAction.PulseTimeoutKey";
const TIMING_TIMEOUT: &str = "DoorTimingWireAction.PulseTimeoutKey";
const ACCESS_TIMEOUT: &str = "AccessWireAction.PulseTimeoutKey";
const LOG_TIMEOUT: &str = "LogWireAction.PulseTimeoutKey";

fn power(world: &World, entity: Entity) -> PowerWireData {
    world
        .get::<Wires>(entity)
        .map(|w| w.power.clone())
        .unwrap_or_default()
}

fn all_power_wires_cut(world: &World, entity: Entity) -> bool {
    let data = power(world, entity);
    matches!((data.cut_wires, data.wire_count), (Some(cut), Some(count)) if cut == count)
}

fn all_power_wires_mended(world: &World, entity: Entity) -> bool {
    power(world, entity).cut_wires == Some(0)
}

/// `PowerWireAction.SetPower`.
fn set_power(world: &mut World, entity: Entity, pulsed: bool) {
    if world
        .get::<crate::power::ApcPowerReceiver>(entity)
        .is_none()
    {
        return;
    }
    if pulsed || all_power_wires_cut(world, entity) {
        set_power_disabled(world, entity, true);
    } else if power(world, entity).pulsed != Some(true) {
        set_power_disabled(world, entity, false);
    }
}

fn set_power_wire_cuts(world: &mut World, entity: Entity, is_cut: bool) {
    with_wires(world, entity, |wires| {
        if let (Some(cut), Some(count)) = (wires.power.cut_wires, wires.power.wire_count) {
            if (cut == count && is_cut) || (cut <= 0 && !is_cut) {
                return;
            }
            wires.power.cut_wires = Some(if is_cut { cut + 1 } else { cut - 1 });
        }
    });
}

fn set_wire_electrified(world: &mut World, entity: Entity, value: bool) {
    if world.get::<Electrified>(entity).is_none() {
        return;
    }
    set_electrified_wire_cut(world, entity, value);
    set_electrified(world, entity, value);
}

/// Под током — ударить действующего; `false`, если ударило (`TrySetElectrocution`).
fn try_set_electrocution(world: &mut World, entity: Entity, user: Entity) -> bool {
    if world.get::<Electrified>(entity).is_none() {
        return true;
    }
    set_wire_electrified(world, entity, true);
    !try_do_electrified_act(world, entity, user, 1.0)
}

/// `PowerWireAction.UpdateElectrocution`.
fn update_electrocution(world: &mut World, entity: Entity, wire: &Wire, pulse_timeout: f64) {
    let all_cut = all_power_wires_cut(world, entity);
    let active_pulse = power(world, entity).pulsed.unwrap_or(false);
    if !has_timer(world, entity, POWER_ELECTRIFIED_CANCEL)
        && active_pulse
        && is_powered(world, entity)
        && !all_cut
    {
        start_wire_action(
            world,
            entity,
            pulse_timeout,
            POWER_ELECTRIFIED_CANCEL,
            wire.id,
        );
    } else if (!active_pulse && all_cut) || all_power_wires_mended(world, entity) {
        set_wire_electrified(world, entity, false);
    }
}

/// Состояние лампочки провода (`GetLightState`).
fn light_state(world: &World, entity: Entity, wire: &Wire) -> Option<StatusLightState> {
    use StatusLightState::*;
    let on_off = |value: bool| Some(if value { On } else { Off });
    match wire.action.as_ref()? {
        WireAction::Power { .. } => {
            let data = power(world, entity);
            if data.main_wire.is_some_and(|main| main != wire.id) {
                return None;
            }
            if !all_power_wires_mended(world, entity) || data.pulsed == Some(true) {
                return Some(BlinkingSlow);
            }
            Some(if all_power_wires_cut(world, entity) {
                Off
            } else {
                On
            })
        }
        WireAction::DoorBolt => match world.get::<DoorBolt>(entity) {
            Some(bolt) => on_off(bolt.bolts_down),
            None => Some(Off),
        },
        WireAction::DoorBoltLight => match world.get::<DoorBolt>(entity) {
            Some(bolt) => on_off(bolt.bolt_lights_enabled),
            None => Some(Off),
        },
        WireAction::DoorTiming { .. } => match world.get::<Airlock>(entity) {
            Some(airlock) if airlock.auto_close_delay_modifier == 0.01 => Some(Off),
            Some(airlock) if airlock.auto_close_delay_modifier <= 0.5 => Some(BlinkingSlow),
            Some(_) => Some(On),
            None => Some(Off),
        },
        WireAction::DoorSafety { .. } => match world.get::<Airlock>(entity) {
            Some(airlock) => on_off(airlock.safety),
            None => Some(Off),
        },
        WireAction::Access { .. } => match world.get::<AccessReader>(entity) {
            Some(reader) => on_off(reader.enabled),
            None => Some(Off),
        },
        WireAction::Log { .. } => match world.get::<AccessReader>(entity) {
            Some(reader) => on_off(!reader.logging_disabled),
            None => Some(Off),
        },
        WireAction::Other(_) => None,
    }
}

/// Лампочка провода (`GetStatusLightData`): без питания погашена.
fn status_light(world: &World, entity: Entity, wire: &Wire) -> Option<StatusLight> {
    let (name, color, _) = wire.action.as_ref()?.light()?;
    let text = loc(world, name, &[]);
    if !is_powered(world, entity) {
        return Some(StatusLight {
            color,
            state: StatusLightState::Off,
            text,
        });
    }
    light_state(world, entity, wire).map(|state| StatusLight { color, state, text })
}

/// Обновить окно (`UpdateUserInterface`).
pub fn update_ui(world: &mut World, entity: Entity) {
    if let Some(state) = ui_state(world, entity) {
        world.set_ui_state(entity, WIRES_UI_KEY, &state);
    }
}

/// Состояние окна; запоминает лампочки (`Statuses`).
pub fn ui_state(world: &mut World, entity: Entity) -> Option<WiresUiState> {
    let wires = world.get::<Wires>(entity).cloned()?;
    let mut statuses = wires.statuses.clone();
    for wire in &wires.wires {
        if let (Some(light), Some((_, _, key))) = (
            status_light(world, entity, wire),
            wire.action.as_ref().and_then(WireAction::light),
        ) {
            statuses.insert(key.to_owned(), (wire.original_position, light));
        }
    }
    let mut sorted: Vec<(usize, StatusLight)> = statuses.values().cloned().collect();
    sorted.sort_by_key(|(position, _)| *position);
    let state = WiresUiState {
        wires: wires
            .wires
            .iter()
            .map(|w| ClientWire {
                id: w.id,
                is_cut: w.is_cut,
                color: w.color,
                letter: WIRE_LETTERS[w.letter],
            })
            .collect(),
        statuses: sorted.into_iter().map(|(_, light)| light).collect(),
        board_name: loc(world, &wires.board_name, &[]),
        serial_number: wires.serial_number.clone(),
        wire_seed: wires.wire_seed,
    };
    with_wires(world, entity, |w| w.statuses = statuses);
    Some(state)
}

/// Добавить провод к своему виду (`AddWire`). `false` — провод становится пустышкой.
fn add_wire(world: &mut World, entity: Entity, wire: &Wire, count: i32) -> bool {
    match &wire.action {
        Some(WireAction::Power { .. }) => {
            with_wires(world, entity, |wires| {
                if wires.power.cut_wires.is_none() {
                    wires.power.cut_wires = Some(0);
                }
                if count == 1 {
                    wires.power.main_wire = Some(wire.id);
                }
                wires.power.wire_count = Some(count);
            });
            true
        }
        _ => count == 1,
    }
}

/// Действия и пустышки раскладки (`SetOrCreateWireLayout`, начало).
fn layout_actions(world: &World, layout: &str) -> Option<(Vec<WireAction>, usize)> {
    let node = world
        .get_resource::<Prototypes>()?
        .resolved_node("wireLayout", layout)?;
    let actions: Vec<WireAction> = node
        .get("wires")
        .and_then(DataNode::as_seq)
        .map(|items| items.iter().filter_map(WireAction::from_node).collect())
        .unwrap_or_default();
    let dummies = node
        .get("dummyWires")
        .and_then(DataNode::as_str)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    Some((actions, dummies))
}

/// Создать провода по раскладке (`SetOrCreateWireLayout`, `CreateWireSet`, `CreateWire`).
fn set_or_create_layout(world: &mut World, entity: Entity) {
    let Some(component) = world.get::<Wires>(entity).cloned() else {
        return;
    };
    let layout = if component.always_randomize {
        None
    } else {
        world
            .get_resource::<WireLayouts>()
            .and_then(|l| l.0.get(&component.layout_id).cloned())
    };
    let Some((actions, dummies)) = layout_actions(world, &component.layout_id) else {
        return;
    };
    if actions.is_empty() {
        return;
    }
    let mut colors: Vec<WireColor> = WireColor::ALL.to_vec();
    let mut letters: Vec<usize> = (0..WIRE_LETTERS.len()).collect();
    let mut create = |world: &mut World, action: Option<WireAction>, position: usize| -> Wire {
        let (color, letter) = match layout.as_ref().and_then(|l| l.get(&position)) {
            Some(spec) => {
                colors.retain(|c| *c != spec.color);
                letters.retain(|l| *l != spec.letter);
                (spec.color, spec.letter)
            }
            None => {
                let mut rng = random(world);
                let color = if colors.is_empty() {
                    WireColor::Red
                } else {
                    colors.remove(rng.range(0, colors.len() as i32) as usize)
                };
                let letter = if letters.is_empty() {
                    0
                } else {
                    letters.remove(rng.range(0, letters.len() as i32) as usize)
                };
                (color, letter)
            }
        };
        Wire {
            id: 0,
            original_position: position,
            is_cut: false,
            color,
            letter,
            action,
        }
    };
    let mut set: Vec<Wire> = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        set.push(create(world, Some(action.clone()), index));
    }
    for index in 1..=dummies {
        set.push(create(world, None, actions.len() + index));
    }
    let mut counts: HashMap<String, i32> = HashMap::new();
    match layout {
        Some(layout) => {
            let mut list = set.clone();
            for (index, wire) in set.iter().enumerate() {
                if let Some(spec) = layout.get(&index)
                    && spec.position < list.len()
                {
                    list[spec.position] = wire.clone();
                }
            }
            for (id, wire) in list.iter_mut().enumerate() {
                wire.id = id;
            }
            with_wires(world, entity, |w| w.wires = list.clone());
            for id in 0..list.len() {
                let Some(wire) = wire(world, entity, id) else {
                    continue;
                };
                let Some(action) = &wire.action else {
                    continue;
                };
                let count = counts.entry(action.kind().to_owned()).or_default();
                *count += 1;
                let count = *count;
                add_wire(world, entity, &wire, count);
            }
        }
        None => {
            let mut order: Vec<usize> = (0..set.len()).collect();
            {
                let mut rng = random(world);
                for i in (1..order.len()).rev() {
                    let j = rng.range(0, i as i32 + 1) as usize;
                    order.swap(i, j);
                }
            }
            let mut data = HashMap::new();
            let mut list = vec![set[0].clone(); set.len()];
            for (i, original) in order.iter().copied().enumerate() {
                let mut wire = set[original].clone();
                wire.id = i;
                if let Some(action) = &wire.action {
                    let count = counts.entry(action.kind().to_owned()).or_default();
                    *count += 1;
                    let count = *count;
                    if !add_wire(world, entity, &wire, count) {
                        wire.action = None;
                    }
                }
                data.insert(
                    original,
                    WireData {
                        letter: wire.letter,
                        color: wire.color,
                        position: i,
                    },
                );
                // `wires.WiresList[i] = wireSet[id]`: место i получает провод с его новым номером.
                list[i] = wire;
            }
            with_wires(world, entity, |w| w.wires = list);
            if !component.always_randomize && !component.layout_id.is_empty() {
                world
                    .get_resource_or_insert_with(WireLayouts::default)
                    .0
                    .insert(component.layout_id.clone(), data);
            }
        }
    }
}

/// Серийный номер `ABCD-1234` (`GenerateSerialNumber`); один из ста — кириллицей.
fn generate_serial_number(world: &mut World) -> String {
    let mut rng = random(world);
    let cyrillic = rng.prob(0.01);
    let mut text = String::new();
    for _ in 0..4 {
        let code = if cyrillic {
            rng.range(0x0410, 0x0430)
        } else {
            rng.range(0x41, 0x5B)
        };
        text.push(char::from_u32(code as u32).unwrap_or('A'));
    }
    text.push('-');
    for _ in 0..4 {
        text.push(char::from_u32(rng.range(0x30, 0x3A) as u32).unwrap_or('0'));
    }
    text
}

// Действия проводов.

fn cut_action(world: &mut World, entity: Entity, user: Entity, wire: &Wire) -> bool {
    match wire.action.clone() {
        Some(WireAction::Power { .. }) => {
            if !try_set_electrocution(world, entity, user) {
                return false;
            }
            set_power_wire_cuts(world, entity, true);
            set_power(world, entity, false);
            true
        }
        Some(WireAction::DoorBolt) => {
            if world.get::<DoorBolt>(entity).is_some() {
                set_bolt_wire_cut(world, entity, true);
                let down = world.get::<DoorBolt>(entity).is_some_and(|b| b.bolts_down);
                if !down && is_powered(world, entity) {
                    try_set_bolts_down(world, entity, true);
                }
            }
            true
        }
        Some(WireAction::DoorBoltLight) => {
            set_bolt_lights_enabled(world, entity, false);
            true
        }
        Some(WireAction::DoorTiming { .. }) => {
            if world.get::<Airlock>(entity).is_some() {
                cancel_wire_action(world, entity, TIMING_TIMEOUT);
                set_auto_close_delay_modifier(world, entity, 0.01);
            }
            true
        }
        Some(WireAction::DoorSafety { .. }) => {
            if world.get::<Airlock>(entity).is_some() {
                // Как в SS14: отменяется ключ таймера провода безопасности.
                cancel_wire_action(world, entity, SAFETY_TIMEOUT);
                set_safety(world, entity, false);
            }
            true
        }
        Some(WireAction::Access { .. }) => {
            if world.get::<AccessReader>(entity).is_some() {
                cancel_wire_action(world, entity, ACCESS_TIMEOUT);
                set_reader_enabled(world, entity, false);
            }
            true
        }
        Some(WireAction::Log { .. }) => {
            if world.get::<AccessReader>(entity).is_some() {
                cancel_wire_action(world, entity, LOG_TIMEOUT);
                set_logging(world, entity, false);
            }
            true
        }
        Some(WireAction::Other(_)) | None => true,
    }
}

fn mend_action(world: &mut World, entity: Entity, user: Entity, wire: &Wire) -> bool {
    match wire.action.clone() {
        Some(WireAction::Power { .. }) => {
            if !try_set_electrocution(world, entity, user) {
                return false;
            }
            // Починка любого провода питания снимает замыкание.
            cancel_wire_action(world, entity, POWER_PULSE_CANCEL);
            cancel_wire_action(world, entity, POWER_ELECTRIFIED_CANCEL);
            set_power_wire_cuts(world, entity, false);
            set_power(world, entity, false);
            true
        }
        Some(WireAction::DoorBolt) => {
            set_bolt_wire_cut(world, entity, false);
            true
        }
        Some(WireAction::DoorBoltLight) => {
            set_bolt_lights_enabled(world, entity, true);
            true
        }
        Some(WireAction::DoorTiming { .. }) => {
            set_auto_close_delay_modifier(world, entity, 1.0);
            true
        }
        Some(WireAction::DoorSafety { .. }) => {
            set_safety(world, entity, true);
            true
        }
        Some(WireAction::Access { .. }) => {
            set_reader_enabled(world, entity, true);
            true
        }
        Some(WireAction::Log { .. }) => {
            set_logging(world, entity, true);
            true
        }
        Some(WireAction::Other(_)) | None => true,
    }
}

fn pulse_action(world: &mut World, entity: Entity, user: Entity, wire: &Wire) {
    match wire.action.clone() {
        Some(WireAction::Power { pulse_timeout }) => {
            cancel_wire_action(world, entity, POWER_ELECTRIFIED_CANCEL);
            let electrocuted = !try_set_electrocution(world, entity, user);
            if power(world, entity).pulsed == Some(true) {
                return;
            }
            with_wires(world, entity, |w| w.power.pulsed = Some(true));
            start_wire_action(world, entity, pulse_timeout, POWER_PULSE_CANCEL, wire.id);
            if !electrocuted {
                set_power(world, entity, true);
            }
        }
        Some(WireAction::DoorBolt) => {
            let down = world.get::<DoorBolt>(entity).map(|b| b.bolts_down);
            match down {
                Some(down) if is_powered(world, entity) => {
                    try_set_bolts_down(world, entity, !down);
                }
                Some(false) => {
                    try_set_bolts_down(world, entity, true);
                }
                _ => {}
            }
        }
        Some(WireAction::DoorBoltLight) => {
            if let Some(enabled) = world.get::<DoorBolt>(entity).map(|b| b.bolt_lights_enabled) {
                set_bolt_lights_enabled(world, entity, !enabled);
            }
        }
        Some(WireAction::DoorTiming { timeout }) => {
            if world.get::<Airlock>(entity).is_some() {
                set_auto_close_delay_modifier(world, entity, 0.5);
                start_wire_action(world, entity, timeout, TIMING_TIMEOUT, wire.id);
            }
        }
        Some(WireAction::DoorSafety { timeout }) => {
            if world.get::<Airlock>(entity).is_some() {
                set_safety(world, entity, false);
                start_wire_action(world, entity, timeout, SAFETY_TIMEOUT, wire.id);
            }
        }
        Some(WireAction::Access { pulse_timeout }) => {
            if world.get::<AccessReader>(entity).is_some() {
                set_reader_enabled(world, entity, false);
                start_wire_action(world, entity, pulse_timeout, ACCESS_TIMEOUT, wire.id);
            }
        }
        Some(WireAction::Log { pulse_timeout }) => {
            if world.get::<AccessReader>(entity).is_some() {
                set_logging(world, entity, false);
                start_wire_action(world, entity, pulse_timeout, LOG_TIMEOUT, wire.id);
            }
        }
        Some(WireAction::Other(_)) | None => {}
    }
}

/// `Update` действия провода: без питания таймеры отменяются.
fn update_action(world: &mut World, entity: Entity, wire: &Wire) {
    let powered = is_powered(world, entity);
    match wire.action.clone() {
        Some(WireAction::Power { pulse_timeout }) => {
            update_electrocution(world, entity, wire, pulse_timeout);
            if !powered && power(world, entity).pulsed != Some(true) {
                cancel_wire_action(world, entity, POWER_ELECTRIFIED_CANCEL);
                cancel_wire_action(world, entity, POWER_PULSE_CANCEL);
            }
        }
        Some(WireAction::DoorTiming { .. }) if !powered => {
            cancel_wire_action(world, entity, TIMING_TIMEOUT);
        }
        Some(WireAction::DoorSafety { .. }) if !powered => {
            cancel_wire_action(world, entity, SAFETY_TIMEOUT);
        }
        Some(WireAction::Access { .. }) if !powered => {
            cancel_wire_action(world, entity, ACCESS_TIMEOUT);
        }
        Some(WireAction::Log { .. }) if !powered => {
            cancel_wire_action(world, entity, LOG_TIMEOUT);
        }
        _ => {}
    }
}

/// Таймер истёк или отменён (`AwaitPulseCancel` и соседи).
fn finish_timer(world: &mut World, entity: Entity, timer: &WireTimer) {
    let Some(wire) = wire(world, entity, timer.wire) else {
        return;
    };
    match timer.key {
        POWER_PULSE_CANCEL => {
            with_wires(world, entity, |w| w.power.pulsed = Some(false));
            set_power(world, entity, false);
        }
        POWER_ELECTRIFIED_CANCEL => {
            if all_power_wires_mended(world, entity) {
                set_wire_electrified(world, entity, false);
            }
        }
        TIMING_TIMEOUT if !wire.is_cut => set_auto_close_delay_modifier(world, entity, 1.0),
        SAFETY_TIMEOUT if !wire.is_cut => set_safety(world, entity, true),
        ACCESS_TIMEOUT if !wire.is_cut => set_reader_enabled(world, entity, true),
        LOG_TIMEOUT if !wire.is_cut => set_logging(world, entity, true),
        _ => {}
    }
}

fn set_reader_enabled(world: &mut World, entity: Entity, enabled: bool) {
    if let Some(mut reader) = world.get_mut::<AccessReader>(entity) {
        reader.enabled = enabled;
    }
}

fn set_logging(world: &mut World, entity: Entity, enabled: bool) {
    if let Some(mut reader) = world.get_mut::<AccessReader>(entity) {
        reader.logging_disabled = !enabled;
    }
}

/// Выполнить действие над проводом (`UpdateWires`).
fn update_wires(
    world: &mut World,
    entity: Entity,
    user: Entity,
    tool: Entity,
    id: usize,
    action: WiresAction,
) {
    let queued = world
        .get::<Wires>(entity)
        .is_some_and(|w| w.queue.contains(&id));
    if !queued {
        return;
    }
    let dequeue = |world: &mut World| {
        with_wires(world, entity, |w| w.queue.retain(|q| *q != id));
    };
    let (Some(_), Some(current)) = (world.get::<Tool>(tool), wire(world, entity, id)) else {
        dequeue(world);
        return;
    };
    let fail = |world: &mut World, key: &str| {
        let text = loc(world, key, &[]);
        world.popup_cursor(&text, user, PopupType::Small);
    };
    match action {
        WiresAction::Cut | WiresAction::Mend => {
            let cutting = world.has_quality(tool, CUTTING);
            if !cutting {
                fail(
                    world,
                    "wires-component-ui-on-receive-message-need-wirecutters",
                );
            } else if action == WiresAction::Cut && current.is_cut {
                fail(
                    world,
                    "wires-component-ui-on-receive-message-cannot-cut-cut-wire",
                );
            } else if action == WiresAction::Mend && !current.is_cut {
                fail(
                    world,
                    "wires-component-ui-on-receive-message-cannot-mend-uncut-wire",
                );
            } else {
                if let Some(sound) = world.get::<Tool>(tool).and_then(|t| t.use_sound.clone()) {
                    world.play_entity(&sound, tool, None);
                }
                let ok = if action == WiresAction::Cut {
                    cut_action(world, entity, user, &current)
                } else {
                    mend_action(world, entity, user, &current)
                };
                if ok {
                    with_wires(world, entity, |w| {
                        if let Some(wire) = w.wires.get_mut(id) {
                            wire.is_cut = action == WiresAction::Cut;
                        }
                    });
                }
                update_ui(world, entity);
            }
        }
        WiresAction::Pulse => {
            if !world.has_quality(tool, PULSING) {
                fail(
                    world,
                    "wires-component-ui-on-receive-message-need-multitool",
                );
            } else if current.is_cut {
                fail(
                    world,
                    "wires-component-ui-on-receive-message-cannot-pulse-cut-wire",
                );
            } else {
                pulse_action(world, entity, user, &current);
                update_ui(world, entity);
                if let Some(sound) = world.get::<Wires>(entity).map(|w| w.pulse_sound.clone()) {
                    world.play_entity(&sound, entity, None);
                }
            }
        }
    }
    if let Some(current) = wire(world, entity, id) {
        update_action(world, entity, &current);
    }
    dequeue(world);
}

/// Проверить инструмент и выполнить действие (`TryDoWireAction`). Время инструмента —
/// ноль, поэтому без ожидания.
pub fn try_do_wire_action(
    world: &mut World,
    entity: Entity,
    user: Entity,
    tool: Entity,
    id: usize,
    action: WiresAction,
) {
    let Some(wires) = world.get::<Wires>(entity) else {
        return;
    };
    if world.get::<Tool>(tool).is_none() || wires.queue.contains(&id) {
        return;
    }
    let Some(current) = wire(world, entity, id) else {
        return;
    };
    let message = match action {
        WiresAction::Cut | WiresAction::Mend if !world.has_quality(tool, CUTTING) => {
            Some("wires-component-ui-on-receive-message-need-wirecutters")
        }
        WiresAction::Cut if current.is_cut => {
            Some("wires-component-ui-on-receive-message-cannot-cut-cut-wire")
        }
        WiresAction::Mend if !current.is_cut => {
            Some("wires-component-ui-on-receive-message-cannot-mend-uncut-wire")
        }
        WiresAction::Pulse if !world.has_quality(tool, PULSING) => {
            Some("wires-component-ui-on-receive-message-need-multitool")
        }
        WiresAction::Pulse if current.is_cut => {
            Some("wires-component-ui-on-receive-message-cannot-pulse-cut-wire")
        }
        _ => None,
    };
    if let Some(message) = message {
        let text = loc(world, message, &[]);
        world.popup_cursor(&text, user, PopupType::Small);
        return;
    }
    with_wires(world, entity, |w| w.queue.push(id));
    update_wires(world, entity, user, tool, id, action);
}

/// Таймеры проводов (`WiresSystem.Update`).
fn update_timers(world: &mut World) {
    let time = world.resource::<GameTime>().cur_time().as_secs_f64();
    let owners: Vec<Entity> = world
        .query::<(Entity, &Wires)>()
        .iter(world)
        .filter(|(_, w)| !w.timers.is_empty())
        .map(|(e, _)| e)
        .collect();
    for entity in owners {
        let finished: Vec<WireTimer> = world
            .get::<Wires>(entity)
            .map(|w| {
                w.timers
                    .iter()
                    .filter(|t| t.cancelled || t.until <= time)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if finished.is_empty() {
            continue;
        }
        for timer in &finished {
            finish_timer(world, entity, timer);
        }
        with_wires(world, entity, |w| {
            w.timers.retain(|t| !finished.contains(t));
        });
        update_ui(world, entity);
    }
}

fn install_panel(world: &mut World) {
    world.subscribe::<WiresPanel, ComponentInit>(|world, entity, _| {
        update_panel_appearance(world, entity);
    });
    world.subscribe::<WiresPanel, InteractUsing>(|world, entity, event| {
        let Some(panel) = world.get::<WiresPanel>(entity).cloned() else {
            return;
        };
        if event.handled || !world.has_quality(event.used, &panel.opening_tool) {
            return;
        }
        if !can_toggle_panel(world, entity, Some(event.user)) {
            return;
        }
        if world.use_tool(
            event.used,
            event.user,
            Some(entity),
            panel.open_delay,
            &[&panel.opening_tool],
            WirePanelDoAfter,
            0.0,
        ) {
            event.handled = true;
        }
    });
    world.subscribe::<WiresPanel, DoAfterDone<WirePanelDoAfter>>(|world, entity, event| {
        if event.cancelled {
            return;
        }
        let Some(open) = world.get::<WiresPanel>(entity).map(|p| p.open) else {
            return;
        };
        if !toggle_panel(world, entity, !open, Some(event.user)) {
            return;
        }
        let Some(panel) = world.get::<WiresPanel>(entity).cloned() else {
            return;
        };
        let sound = if panel.open {
            panel.screwdriver_open_sound
        } else {
            panel.screwdriver_close_sound
        };
        world.play_entity(&sound, entity, None);
        event.handled = true;
    });
    world.subscribe::<WiresPanel, Examined>(|world, entity, event| {
        let Some(panel) = world.get::<WiresPanel>(entity).cloned() else {
            return;
        };
        let text = if panel.open {
            panel.examine_text_open
        } else {
            panel.examine_text_closed
        };
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            let text = loc(world, &text, &[]);
            event.push_markup(text, 0);
        }
        if panel.open
            && let Some(extra) = world
                .get::<WiresPanelSecurity>(entity)
                .and_then(|s| s.examine.clone())
        {
            let text = loc(world, &extra, &[]);
            event.push_markup(text, 0);
        }
    });
    world.subscribe::<WiresPanel, GetVerbs>(|world, entity, event| {
        if event.kind != VerbKind::Alternative || !is_panel_open(world, entity, None) {
            return;
        }
        let user = event.user;
        let text = loc(world, "wires-panel-verb-view-panel", &[]);
        let mut verb = Verb::new(VerbKind::Alternative, text, move |world| {
            world.open_ui(entity, WIRES_UI_KEY, user);
        });
        verb.icon = Some("/Textures/Interface/VerbIcons/screwdriver.png".into());
        event.verbs.push(verb);
    });
    world.subscribe::<ActivatableUiRequiresPanel, ActivatableUiOpenAttempt>(
        |world, entity, event| {
            let Some(open) = world.get::<WiresPanel>(entity).map(|p| p.open) else {
                return;
            };
            if event.cancelled {
                return;
            }
            if world
                .get::<ActivatableUiRequiresPanel>(entity)
                .is_some_and(|r| r.require_open != open)
            {
                event.cancelled = true;
            }
        },
    );
    world.subscribe::<ActivatableUiRequiresPanel, PanelChanged>(|world, entity, event| {
        let require_open = world
            .get::<ActivatableUiRequiresPanel>(entity)
            .is_some_and(|r| r.require_open);
        if event.open == require_open {
            return;
        }
        crate::activatable_ui::close_all(world, entity);
    });
}

fn install_wires(world: &mut World) {
    world.subscribe::<Wires, MapInit>(|world, entity, _| {
        let layout = world.get::<Wires>(entity).map(|w| w.layout_id.clone());
        if layout.is_some_and(|l| !l.is_empty()) {
            set_or_create_layout(world, entity);
        }
        if world
            .get::<Wires>(entity)
            .is_some_and(|w| w.serial_number.is_none())
        {
            let serial = generate_serial_number(world);
            with_wires(world, entity, |w| w.serial_number = Some(serial));
        }
        if world.get::<Wires>(entity).is_some_and(|w| w.wire_seed == 0) {
            let seed = random(world).range(1, i32::MAX);
            with_wires(world, entity, |w| w.wire_seed = seed);
        }
        update_ui(world, entity);
    });
    world.subscribe::<Wires, PanelChanged>(|world, entity, event| {
        if !event.open {
            world.close_ui_for_all(entity, WIRES_UI_KEY);
        }
    });
    world.subscribe::<Wires, InteractUsing>(|world, entity, event| {
        if event.handled || world.get::<Tool>(event.used).is_none() {
            return;
        }
        if !is_panel_open(world, entity, None) {
            return;
        }
        if world.has_quality(event.used, CUTTING) || world.has_quality(event.used, PULSING) {
            world.open_ui(entity, WIRES_UI_KEY, event.user);
            event.handled = true;
        }
    });
    world.subscribe::<Wires, PowerChanged>(|world, entity, _| {
        update_ui(world, entity);
        let wires: Vec<Wire> = world
            .get::<Wires>(entity)
            .map(|w| w.wires.clone())
            .unwrap_or_default();
        for wire in wires {
            update_action(world, entity, &wire);
        }
    });
    world.subscribe::<Wires, BoundUiMessage<WiresActionMessage>>(|world, entity, event| {
        let user = event.actor;
        let message = event.message;
        if world.get::<crate::hands::Hands>(user).is_none() {
            let text = loc(world, "wires-component-ui-on-receive-message-no-hands", &[]);
            world.popup_entity_for(&text, entity, user, PopupType::Small);
            return;
        }
        if !world.in_range_unobstructed(user, entity, INTERACTION_RANGE) {
            let text = loc(
                world,
                "wires-component-ui-on-receive-message-cannot-reach",
                &[],
            );
            world.popup_entity_for(&text, entity, user, PopupType::Small);
            return;
        }
        let Some(held) = world.active_item(user) else {
            return;
        };
        if world.get::<Tool>(held).is_none() {
            return;
        }
        try_do_wire_action(world, entity, user, held, message.id, message.action);
    });
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<WiresPanel>();
    world.register_data_component::<WiresPanelSecurity>();
    world.register_data_component::<ActivatableUiRequiresPanel>();
    world.register_data_component::<Wires>();
    world.register_ui_message::<WiresActionMessage>();
    if !side.is_server() {
        return;
    }
    world.init_resource::<WireLayouts>();
    register_tool_event::<WirePanelDoAfter>(world);
    install_panel(world);
    install_wires(world);
    world.add_game_systems(Update, update_timers);
}

#[cfg(test)]
mod tests;
