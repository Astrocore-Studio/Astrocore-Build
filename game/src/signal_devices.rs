//! Устройства сигналов: двери по сигналу открываются, закрываются, переключаются и
//! болтируются и сами сообщают, открыты ли и опущены ли болты; выключатель и кнопка шлют
//! сигнал по нажатию (запертый — нет); двусторонний рычаг шлёт «влево», «вправо» или
//! «середина»; логический элемент собирает два входа и шлёт уровень выхода, когда тот
//! меняется (отвёрткой переключается вид элемента).
//! Образец — `DoorSignalControlComponent`/`System`, `SignalSwitchComponent`/`System`,
//! `TwoWayLeverComponent`/`System`, `LogicGateComponent`/`System` SS14 @ 2f4a2aad.
//!
//! Нет пока: таймеров (`SignalTimer`), детекторов фронта, ячеек памяти, случайных элементов,
//! передатчиков (`Signaller`), задержки нажатия (`UseDelay`).

use astrocore_audio::{AudioParams, AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{ComponentInit, EventWorldExt, GameWorldExt, RegistryWorldExt, Update};
use astrocore_net::AppearanceWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::device_linking::{
    LogicSignalReceived, SignalReceived, SignalState, ensure_sink_ports, ensure_source_ports,
    invoke_port, send_signal,
};
use crate::doors::{Door, DoorBolt, DoorBoltsChanged, DoorState, DoorStateChanged};
use crate::interaction::{ActivateInWorld, InteractUsing};
use crate::popups::{PopupType, PopupWorldExt};
use crate::tools::ToolWorldExt;
use crate::verbs::{GetVerbs, Verb, VerbKind};

fn port(name: &str) -> String {
    name.to_owned()
}

fn click_sound() -> Option<SoundSpecifier> {
    Some(SoundSpecifier::path("/Audio/Machines/lightswitch.ogg"))
}

// Двери.

/// Управление дверью сигналами (`DoorSignalControlComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoorSignalControl {
    #[serde(default = "open_port")]
    pub open_port: String,
    #[serde(default = "close_port")]
    pub close_port: String,
    #[serde(default = "toggle_port")]
    pub toggle_port: String,
    #[serde(default = "bolt_port", rename = "boltPort")]
    pub in_bolt: String,
    #[serde(default = "on_open_port", rename = "onOpenPort")]
    pub out_open: String,
    #[serde(default = "on_bolt_port", rename = "onBoltPort")]
    pub out_bolt: String,
}

fn open_port() -> String {
    port("Open")
}
fn close_port() -> String {
    port("Close")
}
fn toggle_port() -> String {
    port("Toggle")
}
fn bolt_port() -> String {
    port("DoorBolt")
}
fn on_open_port() -> String {
    port("DoorStatus")
}
fn on_bolt_port() -> String {
    port("DoorBoltStatus")
}

impl Default for DoorSignalControl {
    fn default() -> Self {
        Self {
            open_port: open_port(),
            close_port: close_port(),
            toggle_port: toggle_port(),
            in_bolt: bolt_port(),
            out_open: on_open_port(),
            out_bolt: on_bolt_port(),
        }
    }
}

fn door_state(world: &World, entity: Entity) -> Option<DoorState> {
    world.get::<Door>(entity).map(|d| d.state)
}

fn bolts_down(world: &World, entity: Entity) -> Option<bool> {
    world.get::<DoorBolt>(entity).map(|b| b.bolts_down)
}

/// Импульс на вход двери (`OnSignalReceived`).
fn door_signal(world: &mut World, entity: Entity, port: &str) {
    let Some(control) = world.get::<DoorSignalControl>(entity).cloned() else {
        return;
    };
    let Some(state) = door_state(world, entity) else {
        return;
    };
    if port == control.open_port {
        if state == DoorState::Closed {
            crate::doors::try_open(world, entity, None, false);
        }
    } else if port == control.close_port {
        if state == DoorState::Open {
            crate::doors::try_close(world, entity, None);
        }
    } else if port == control.toggle_port {
        crate::doors::try_toggle(world, entity, None);
    } else if port == control.in_bolt
        && let Some(down) = bolts_down(world, entity)
    {
        crate::doors::try_set_bolts_down(world, entity, !down);
    }
}

/// Уровень на вход двери (`OnSignalReceived` с `LogicStatePayload`).
fn door_logic_signal(world: &mut World, entity: Entity, port: &str, signal: SignalState) {
    let Some(control) = world.get::<DoorSignalControl>(entity).cloned() else {
        return;
    };
    let Some(state) = door_state(world, entity) else {
        return;
    };
    if port == control.open_port {
        if signal != SignalState::Low && state == DoorState::Closed {
            crate::doors::try_open(world, entity, None, false);
        }
    } else if port == control.close_port {
        if signal != SignalState::Low && state == DoorState::Open {
            crate::doors::try_close(world, entity, None);
        }
    } else if port == control.toggle_port {
        if signal != SignalState::Low {
            crate::doors::try_toggle(world, entity, None);
        }
    } else if port == control.in_bolt
        && let Some(down) = bolts_down(world, entity)
    {
        let bolt = if signal == SignalState::Momentary {
            !down
        } else {
            signal == SignalState::High
        };
        crate::doors::try_set_bolts_down(world, entity, bolt);
    }
}

// Выключатели и кнопки.

/// Выключатель или кнопка (`SignalSwitchComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalSwitch {
    #[serde(default = "on_port")]
    pub on_port: String,
    #[serde(default = "off_port")]
    pub off_port: String,
    #[serde(default = "status_port")]
    pub status_port: String,
    #[serde(default)]
    pub state: bool,
    #[serde(default = "click_sound")]
    pub click_sound: Option<SoundSpecifier>,
}

fn on_port() -> String {
    port("On")
}
fn off_port() -> String {
    port("Off")
}
fn status_port() -> String {
    port("Status")
}

impl Default for SignalSwitch {
    fn default() -> Self {
        Self {
            on_port: on_port(),
            off_port: off_port(),
            status_port: status_port(),
            state: false,
            click_sound: click_sound(),
        }
    }
}

/// Щёлкнуть (`OnActivated`).
fn switch_activated(world: &mut World, entity: Entity, event: &mut ActivateInWorld) {
    if event.handled || !event.complex || crate::lock::is_locked(world, entity) {
        return;
    }
    let Some(switch) = world.get_mut::<SignalSwitch>(entity).map(|mut s| {
        s.state = !s.state;
        s.clone()
    }) else {
        return;
    };
    let port = if switch.state {
        &switch.on_port
    } else {
        &switch.off_port
    };
    invoke_port(world, entity, port);
    if switch.on_port != switch.off_port {
        send_signal(world, entity, &switch.status_port, switch.state);
    }
    if let Some(sound) = switch.click_sound {
        let base = sound.params();
        let params = AudioParams {
            variation: Some(0.125),
            volume: base.volume + 8.0,
            ..base
        };
        world.play_entity(&sound.with_params(params), entity, None);
    }
    event.handled = true;
}

// Двусторонний рычаг.

/// Положение рычага (`TwoWayLeverState`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TwoWayLeverState {
    #[default]
    Middle,
    Right,
    Left,
}

impl TwoWayLeverState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Middle => "Middle",
            Self::Right => "Right",
            Self::Left => "Left",
        }
    }
}

/// Вид рычага (`TwoWayLeverVisuals.State`).
pub const LEVER_STATE: &str = "enum.TwoWayLeverVisuals.State";

/// Двусторонний рычаг (`TwoWayLeverComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TwoWayLever {
    #[serde(default)]
    pub state: TwoWayLeverState,
    #[serde(default)]
    pub next_signal_left: bool,
    #[serde(default = "left_port")]
    pub left_port: String,
    #[serde(default = "right_port")]
    pub right_port: String,
    #[serde(default = "middle_port")]
    pub middle_port: String,
}

fn left_port() -> String {
    port("Left")
}
fn right_port() -> String {
    port("Right")
}
fn middle_port() -> String {
    port("Middle")
}

impl Default for TwoWayLever {
    fn default() -> Self {
        Self {
            state: TwoWayLeverState::Middle,
            next_signal_left: false,
            left_port: left_port(),
            right_port: right_port(),
            middle_port: middle_port(),
        }
    }
}

/// Перевести рычаг (`StateChanged`): из середины следующий шаг меняет сторону.
pub fn set_lever_state(world: &mut World, entity: Entity, state: TwoWayLeverState) {
    let Some(lever) = world.get_mut::<TwoWayLever>(entity).map(|mut l| {
        l.state = state;
        if state == TwoWayLeverState::Middle {
            l.next_signal_left = !l.next_signal_left;
        }
        l.clone()
    }) else {
        return;
    };
    world.set_appearance(entity, LEVER_STATE, state.name());
    let port = match state {
        TwoWayLeverState::Left => lever.left_port,
        TwoWayLeverState::Right => lever.right_port,
        TwoWayLeverState::Middle => lever.middle_port,
    };
    invoke_port(world, entity, &port);
}

fn lever_verbs(world: &mut World, entity: Entity, event: &mut GetVerbs) {
    if event.kind != VerbKind::Interaction || !event.can_access || !event.can_interact {
        return;
    }
    let Some(state) = world.get::<TwoWayLever>(entity).map(|l| l.state) else {
        return;
    };
    let cant = crate::examine::loc(world, "two-way-lever-cant", &[]);
    let sides = [
        (
            "two-way-lever-left",
            TwoWayLeverState::Left,
            "rotate_ccw.svg.192dpi.png",
        ),
        (
            "two-way-lever-right",
            TwoWayLeverState::Right,
            "rotate_cw.svg.192dpi.png",
        ),
    ];
    for (key, side, icon) in sides {
        let disabled = state == side;
        let text = crate::examine::loc(world, key, &[]);
        let mut verb = Verb::new(VerbKind::Interaction, text, move |world| {
            let Some(current) = world.get::<TwoWayLever>(entity).map(|l| l.state) else {
                return;
            };
            let next = if current == TwoWayLeverState::Middle {
                side
            } else {
                TwoWayLeverState::Middle
            };
            set_lever_state(world, entity, next);
        });
        verb.category = Some("verb-categories-lever".into());
        verb.disabled = disabled;
        verb.message = disabled.then(|| cant.clone());
        verb.icon = Some(format!("/Textures/Interface/VerbIcons/{icon}"));
        event.verbs.push(verb);
    }
}

// Логические элементы.

/// Вид логического элемента (`LogicGate`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogicGateKind {
    #[default]
    Or,
    And,
    Xor,
    Nor,
    Nand,
    Xnor,
}

impl LogicGateKind {
    const ALL: [Self; 6] = [
        Self::Or,
        Self::And,
        Self::Xor,
        Self::Nor,
        Self::Nand,
        Self::Xnor,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Or => "Or",
            Self::And => "And",
            Self::Xor => "Xor",
            Self::Nor => "Nor",
            Self::Nand => "Nand",
            Self::Xnor => "Xnor",
        }
    }

    pub fn apply(self, a: bool, b: bool) -> bool {
        match self {
            Self::Or => a || b,
            Self::And => a && b,
            Self::Xor => a != b,
            Self::Nor => !(a || b),
            Self::Nand => !(a && b),
            Self::Xnor => a == b,
        }
    }

    fn next(self) -> Self {
        let index = Self::ALL.iter().position(|g| *g == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

/// Вид элемента (`LogicGateVisuals`).
pub const GATE_VISUALS: &str = "enum.LogicGateVisuals.Gate";
pub const GATE_INPUT_A: &str = "enum.LogicGateVisuals.InputA";
pub const GATE_INPUT_B: &str = "enum.LogicGateVisuals.InputB";
pub const GATE_OUTPUT: &str = "enum.LogicGateVisuals.Output";

/// Логический элемент (`LogicGateComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogicGate {
    #[serde(default)]
    pub gate: LogicGateKind,
    #[serde(default = "screwing")]
    pub cycle_quality: String,
    #[serde(default = "click_sound")]
    pub cycle_sound: Option<SoundSpecifier>,
    #[serde(default = "input_a")]
    pub input_port_a: String,
    #[serde(default = "input_b")]
    pub input_port_b: String,
    #[serde(default = "output_port")]
    pub output_port: String,
    #[serde(default)]
    pub state_a: SignalState,
    #[serde(default)]
    pub state_b: SignalState,
    #[serde(default)]
    pub last_output: bool,
}

fn screwing() -> String {
    port("Screwing")
}
fn input_a() -> String {
    port("InputA")
}
fn input_b() -> String {
    port("InputB")
}
fn output_port() -> String {
    port("Output")
}

impl Default for LogicGate {
    fn default() -> Self {
        Self {
            gate: LogicGateKind::Or,
            cycle_quality: screwing(),
            cycle_sound: click_sound(),
            input_port_a: input_a(),
            input_port_b: input_b(),
            output_port: output_port(),
            state_a: SignalState::Low,
            state_b: SignalState::Low,
            last_output: false,
        }
    }
}

/// Пересчитать выход (`UpdateOutput`): сигнал — только при смене уровня.
fn update_output(world: &mut World, entity: Entity) {
    let Some(gate) = world.get::<LogicGate>(entity).cloned() else {
        return;
    };
    let a = gate.state_a != SignalState::Low;
    let b = gate.state_b != SignalState::Low;
    let output = gate.gate.apply(a, b);
    world.set_appearance(entity, GATE_OUTPUT, output);
    if output == gate.last_output {
        return;
    }
    if let Some(mut g) = world.get_mut::<LogicGate>(entity) {
        g.last_output = output;
    }
    send_signal(world, entity, &gate.output_port, output);
}

fn gate_input(world: &mut World, entity: Entity, port: &str, state: SignalState, visual: bool) {
    let Some(gate) = world.get::<LogicGate>(entity).cloned() else {
        return;
    };
    if port == gate.input_port_a {
        if let Some(mut g) = world.get_mut::<LogicGate>(entity) {
            g.state_a = state;
        }
        world.set_appearance(entity, GATE_INPUT_A, visual);
    } else if port == gate.input_port_b {
        if let Some(mut g) = world.get_mut::<LogicGate>(entity) {
            g.state_b = state;
        }
        world.set_appearance(entity, GATE_INPUT_B, visual);
    }
    update_output(world, entity);
}

/// Импульс гаснет к следующему такту (`Update`).
fn update_gates(world: &mut World) {
    let gates: Vec<Entity> = world
        .query::<(Entity, &LogicGate)>()
        .iter(world)
        .map(|(e, _)| e)
        .collect();
    for entity in gates {
        if let Some(mut g) = world.get_mut::<LogicGate>(entity) {
            if g.state_a == SignalState::Momentary {
                g.state_a = SignalState::Low;
            }
            if g.state_b == SignalState::Momentary {
                g.state_b = SignalState::Low;
            }
        }
        update_output(world, entity);
    }
}

/// Отвёрткой — следующий вид (`OnInteractUsing`).
fn cycle_gate(world: &mut World, entity: Entity, event: &mut InteractUsing) {
    let Some(gate) = world.get::<LogicGate>(entity).cloned() else {
        return;
    };
    if event.handled || !world.has_quality(event.used, &gate.cycle_quality) {
        return;
    }
    let next = gate.gate.next();
    if let Some(mut g) = world.get_mut::<LogicGate>(entity) {
        g.gate = next;
    }
    update_output(world, entity);
    if let Some(sound) = &gate.cycle_sound {
        world.play_entity(sound, entity, None);
    }
    let name = next.name().to_uppercase();
    let text = crate::examine::loc(
        world,
        "logic-gate-cycle",
        &[("gate", astrocore_loc::LocArg::from(name.as_str()))],
    );
    world.popup_entity_for(&text, entity, event.user, PopupType::Small);
    world.set_appearance(entity, GATE_VISUALS, next.name());
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<DoorSignalControl>();
    world.register_data_component::<SignalSwitch>();
    world.register_data_component::<TwoWayLever>();
    world.register_data_component::<LogicGate>();
    if !side.is_server() {
        return;
    }
    // Двери (`DoorSignalControlSystem`).
    world.subscribe::<DoorSignalControl, ComponentInit>(|world, entity, _| {
        let Some(control) = world.get::<DoorSignalControl>(entity).cloned() else {
            return;
        };
        let bolted = world.get::<DoorBolt>(entity).is_some();
        ensure_sink_ports(
            world,
            entity,
            &[
                &control.open_port,
                &control.close_port,
                &control.toggle_port,
            ],
        );
        if bolted {
            ensure_sink_ports(world, entity, &[&control.in_bolt]);
        }
        ensure_source_ports(world, entity, &[&control.out_open]);
        if bolted {
            ensure_source_ports(world, entity, &[&control.out_bolt]);
        }
    });
    world.subscribe::<DoorSignalControl, SignalReceived>(|world, entity, event| {
        let port = event.port.clone();
        door_signal(world, entity, &port);
    });
    world.subscribe::<DoorSignalControl, LogicSignalReceived>(|world, entity, event| {
        let (port, state) = (event.port.clone(), event.state);
        door_logic_signal(world, entity, &port, state);
    });
    world.subscribe::<DoorSignalControl, DoorStateChanged>(|world, entity, event| {
        let Some(port) = world
            .get::<DoorSignalControl>(entity)
            .map(|c| c.out_open.clone())
        else {
            return;
        };
        match event.state {
            DoorState::Closed => send_signal(world, entity, &port, false),
            DoorState::Open | DoorState::Opening | DoorState::Closing | DoorState::Emagging => {
                send_signal(world, entity, &port, true)
            }
            _ => {}
        }
    });
    world.subscribe::<DoorSignalControl, DoorBoltsChanged>(|world, entity, event| {
        let Some(port) = world
            .get::<DoorSignalControl>(entity)
            .map(|c| c.out_bolt.clone())
        else {
            return;
        };
        send_signal(world, entity, &port, event.bolts_down);
    });
    // Выключатели (`SignalSwitchSystem`).
    world.subscribe::<SignalSwitch, ComponentInit>(|world, entity, _| {
        let Some(switch) = world.get::<SignalSwitch>(entity).cloned() else {
            return;
        };
        ensure_source_ports(
            world,
            entity,
            &[&switch.on_port, &switch.off_port, &switch.status_port],
        );
    });
    world.subscribe::<SignalSwitch, ActivateInWorld>(switch_activated);
    // Рычаг (`TwoWayLeverSystem`).
    world.subscribe::<TwoWayLever, ComponentInit>(|world, entity, _| {
        let Some(lever) = world.get::<TwoWayLever>(entity).cloned() else {
            return;
        };
        ensure_source_ports(
            world,
            entity,
            &[&lever.left_port, &lever.right_port, &lever.middle_port],
        );
    });
    world.subscribe::<TwoWayLever, ActivateInWorld>(|world, entity, event| {
        if event.handled || !event.complex {
            return;
        }
        let Some(lever) = world.get::<TwoWayLever>(entity).cloned() else {
            return;
        };
        let next = match lever.state {
            TwoWayLeverState::Middle if lever.next_signal_left => TwoWayLeverState::Left,
            TwoWayLeverState::Middle => TwoWayLeverState::Right,
            TwoWayLeverState::Left | TwoWayLeverState::Right => TwoWayLeverState::Middle,
        };
        set_lever_state(world, entity, next);
        event.handled = true;
    });
    world.subscribe::<TwoWayLever, GetVerbs>(lever_verbs);
    // Логические элементы (`LogicGateSystem`).
    world.subscribe::<LogicGate, ComponentInit>(|world, entity, _| {
        let Some(gate) = world.get::<LogicGate>(entity).cloned() else {
            return;
        };
        ensure_sink_ports(world, entity, &[&gate.input_port_a, &gate.input_port_b]);
        ensure_source_ports(world, entity, &[&gate.output_port]);
    });
    world.subscribe::<LogicGate, SignalReceived>(|world, entity, event| {
        let port = event.port.clone();
        gate_input(world, entity, &port, SignalState::Momentary, false);
    });
    world.subscribe::<LogicGate, LogicSignalReceived>(|world, entity, event| {
        let (port, state) = (event.port.clone(), event.state);
        gate_input(world, entity, &port, state, state == SignalState::High);
    });
    world.subscribe::<LogicGate, InteractUsing>(cycle_gate);
    world.subscribe::<LogicGate, crate::examine::Examined>(|world, entity, event| {
        if !event.is_in_details_range {
            return;
        }
        let Some(gate) = world.get::<LogicGate>(entity).map(|g| g.gate) else {
            return;
        };
        let name = gate.name().to_uppercase();
        let text = crate::examine::loc(
            world,
            "logic-gate-examine",
            &[("gate", astrocore_loc::LocArg::from(name.as_str()))],
        );
        event.push_markup(text, 0);
    });
    world.add_game_systems(Update, update_gates);
}

#[cfg(test)]
mod tests;
