use astrocore_ecs::GameWorldExt;
use astrocore_net::AppearanceValue;

use super::*;
use crate::device_linking::{DeviceLinkSink, DeviceLinkSource, linked_sinks, toggle_link};
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: sourcePort
  id: Pressed
  name: signal-port-name-pressed
- type: sourcePort
  id: On
  name: signal-port-name-on-transmitter
- type: sourcePort
  id: Off
  name: signal-port-name-off-transmitter
- type: sourcePort
  id: Status
  name: signal-port-name-status-transmitter
- type: sourcePort
  id: Left
  name: signal-port-name-left
- type: sourcePort
  id: Right
  name: signal-port-name-right
- type: sourcePort
  id: Middle
  name: signal-port-name-middle
- type: sourcePort
  id: DoorStatus
  name: signal-port-name-doorstatus
- type: sourcePort
  id: DoorBoltStatus
  name: signal-port-name-boltstatus
- type: sourcePort
  id: Output
  name: signal-port-name-logic-output
- type: sourcePort
  id: AirDanger
  name: signal-port-name-air-danger
- type: sourcePort
  id: AirWarning
  name: signal-port-name-air-warning
- type: sourcePort
  id: AirNormal
  name: signal-port-name-air-normal
- type: sinkPort
  id: Open
  name: signal-port-name-open
- type: sinkPort
  id: Close
  name: signal-port-name-close
- type: sinkPort
  id: Toggle
  name: signal-port-name-toggle
- type: sinkPort
  id: DoorBolt
  name: signal-port-name-doorbolt
- type: sinkPort
  id: InputA
  name: signal-port-name-logic-input-a
- type: sinkPort
  id: InputB
  name: signal-port-name-logic-input-b
- type: deviceFrequency
  id: BasicDevice
  frequency: 1280
- type: entity
  id: TestDoor
  components:
  - type: Door
  - type: DoorBolt
  - type: DoorSignalControl
  - type: DeviceNetwork
    deviceNetId: Wireless
    receiveFrequencyId: BasicDevice
  - type: WirelessNetwork
    range: 200
- type: entity
  id: TestButton
  components:
  - type: SignalSwitch
    onPort: Pressed
    offPort: Pressed
    statusPort: Pressed
  - type: DeviceLinkSource
    ports: [ Pressed ]
- type: entity
  id: TestSwitch
  components:
  - type: SignalSwitch
  - type: DeviceNetwork
    deviceNetId: Wireless
  - type: WirelessNetwork
    range: 200
- type: entity
  id: LockedSwitch
  parent: TestSwitch
  components:
  - type: Lock
    locked: true
- type: entity
  id: TestGate
  components:
  - type: Appearance
  - type: LogicGate
  - type: DeviceNetwork
    deviceNetId: Wireless
    receiveFrequencyId: BasicDevice
  - type: WirelessNetwork
    range: 200
- type: entity
  id: TestLever
  components:
  - type: Appearance
  - type: TwoWayLever
- type: entity
  id: Probe
  components:
  - type: DeviceLinkSink
    ports: [ Open, Close, Toggle ]
- type: entity
  id: TestAirAlarm
  components:
  - type: AirAlarm
"#;

#[derive(bevy_ecs::resource::Resource, Default)]
struct Heard(Vec<(Entity, String)>);

fn setup() -> World {
    let mut world = world_with(PROTOTYPES);
    world.init_resource::<Heard>();
    world.subscribe::<DeviceLinkSink, crate::device_linking::SignalReceived>(
        |world, entity, event| {
            let text = event.port.clone();
            world.resource_mut::<Heard>().0.push((entity, text));
        },
    );
    world
}

fn heard(world: &World, entity: Entity) -> Vec<String> {
    world
        .resource::<Heard>()
        .0
        .iter()
        .filter(|(e, _)| *e == entity)
        .map(|(_, t)| t.clone())
        .collect()
}

fn activate(world: &mut World, target: Entity) -> bool {
    let user = target;
    let mut event = ActivateInWorld {
        user,
        target,
        complex: true,
        handled: false,
    };
    world.raise(target, &mut event);
    event.handled
}

fn settle(world: &mut World) {
    for _ in 0..90 {
        world.run_tick();
    }
}

fn state(world: &World, door: Entity) -> DoorState {
    world.get::<Door>(door).unwrap().state
}

#[test]
fn button_toggles_door_and_ports_are_ensured() {
    let mut world = setup();
    let grid = station(&mut world);
    let door = spawn_at(&mut world, "TestDoor", grid, 5, 5);
    let button = spawn_at(&mut world, "TestButton", grid, 4, 4);
    let sink = world.get::<DeviceLinkSink>(door).unwrap().ports.clone();
    assert!(
        ["Open", "Close", "Toggle", "DoorBolt"]
            .iter()
            .all(|p| sink.contains(*p))
    );
    let source = world.get::<DeviceLinkSource>(door).unwrap().ports.clone();
    assert!(source.contains("DoorStatus") && source.contains("DoorBoltStatus"));
    assert!(toggle_link(
        &mut world, None, button, door, "Pressed", "Toggle"
    ));
    world.run_tick();
    assert!(activate(&mut world, button));
    assert_eq!(state(&world, door), DoorState::Opening);
    settle(&mut world);
    assert_eq!(state(&world, door), DoorState::Open);
    activate(&mut world, button);
    settle(&mut world);
    assert_eq!(state(&world, door), DoorState::Closed);
    // Кнопка шлёт один и тот же порт и не шлёт уровень статуса.
    assert!(
        world
            .get::<DeviceLinkSource>(button)
            .unwrap()
            .last_signals
            .is_empty()
    );
}

#[test]
fn airlock_cycle_bolts_partner_while_open() {
    let mut world = setup();
    let grid = station(&mut world);
    let inner = spawn_at(&mut world, "TestDoor", grid, 5, 5);
    let outer = spawn_at(&mut world, "TestDoor", grid, 5, 8);
    world.run_tick();
    // Как тамбуры Saltern: статус двери — на болты соседней.
    assert!(toggle_link(
        &mut world,
        None,
        inner,
        outer,
        "DoorStatus",
        "DoorBolt"
    ));
    crate::doors::try_open(&mut world, inner, None, false);
    assert!(world.get::<DoorBolt>(outer).unwrap().bolts_down);
    assert!(!crate::doors::try_open(&mut world, outer, None, false));
    settle(&mut world);
    crate::doors::try_close(&mut world, inner, None);
    settle(&mut world);
    assert_eq!(state(&world, inner), DoorState::Closed);
    assert!(!world.get::<DoorBolt>(outer).unwrap().bolts_down);
    // Болты сами сообщают уровень.
    assert!(!world.get::<DeviceLinkSource>(outer).unwrap().last_signals["DoorBoltStatus"]);
}

#[test]
fn switch_sends_on_off_and_status_unless_locked() {
    let mut world = setup();
    let grid = station(&mut world);
    let switch = spawn_at(&mut world, "TestSwitch", grid, 1, 1);
    let locked = spawn_at(&mut world, "LockedSwitch", grid, 2, 1);
    let probe = spawn_at(&mut world, "Probe", grid, 3, 1);
    let door = spawn_at(&mut world, "TestDoor", grid, 5, 1);
    world.run_tick();
    toggle_link(&mut world, None, switch, probe, "On", "Open");
    toggle_link(&mut world, None, switch, probe, "Off", "Close");
    toggle_link(&mut world, None, switch, door, "Status", "Open");
    toggle_link(&mut world, None, locked, probe, "On", "Open");
    activate(&mut world, switch);
    activate(&mut world, switch);
    // Щелчок по запертому достаётся замку, не выключателю.
    activate(&mut world, locked);
    assert_eq!(heard(&world, probe), vec!["Open", "Close"]);
    assert!(!world.get::<SignalSwitch>(locked).unwrap().state);
    // Статус уровнем по сети: высокий открыл дверь, низкий её не трогает.
    assert_eq!(state(&world, door), DoorState::Opening);
    assert!(!world.get::<DeviceLinkSource>(switch).unwrap().last_signals["Status"]);
}

#[test]
fn or_gate_follows_inputs_and_screwdriver_cycles_kind() {
    let mut world = setup();
    let grid = station(&mut world);
    let a = spawn_at(&mut world, "TestSwitch", grid, 1, 1);
    let gate = spawn_at(&mut world, "TestGate", grid, 3, 1);
    let door = spawn_at(&mut world, "TestDoor", grid, 5, 1);
    world.run_tick();
    toggle_link(&mut world, None, a, gate, "Status", "InputA");
    toggle_link(&mut world, None, gate, door, "Output", "DoorBolt");
    activate(&mut world, a);
    assert!(world.get::<LogicGate>(gate).unwrap().last_output);
    assert!(world.get::<DoorBolt>(door).unwrap().bolts_down);
    assert_eq!(
        world.appearance(gate, GATE_INPUT_A),
        Some(&AppearanceValue::Bool(true))
    );
    activate(&mut world, a);
    assert!(!world.get::<DoorBolt>(door).unwrap().bolts_down);
    // Импульс без уровня держит вход один такт.
    world.raise(
        gate,
        &mut crate::device_linking::SignalReceived {
            port: "InputB".into(),
            trigger: None,
        },
    );
    assert!(world.get::<LogicGate>(gate).unwrap().last_output);
    world.run_tick();
    assert!(!world.get::<LogicGate>(gate).unwrap().last_output);
    assert!(LogicGateKind::Nor.apply(false, false));
    assert_eq!(LogicGateKind::Xnor.next(), LogicGateKind::Or);
    assert!(!linked_sinks(&world, gate, "Output").is_empty());
}

#[test]
fn lever_steps_through_middle_and_alternates_sides() {
    let mut world = setup();
    let grid = station(&mut world);
    let lever = spawn_at(&mut world, "TestLever", grid, 1, 1);
    let probe = spawn_at(&mut world, "Probe", grid, 2, 1);
    for (source, sink) in [("Left", "Open"), ("Right", "Close"), ("Middle", "Toggle")] {
        toggle_link(&mut world, None, lever, probe, source, sink);
    }
    let mut seen = Vec::new();
    for _ in 0..4 {
        activate(&mut world, lever);
        seen.push(world.get::<TwoWayLever>(lever).unwrap().state);
    }
    use TwoWayLeverState::*;
    assert_eq!(seen, vec![Right, Middle, Left, Middle]);
    assert_eq!(
        heard(&world, probe),
        vec!["Close", "Toggle", "Open", "Toggle"]
    );
    assert_eq!(
        world.appearance(lever, LEVER_STATE),
        Some(&AppearanceValue::Text("Middle".into()))
    );
}

#[test]
fn air_alarm_raises_new_state_port_and_lowers_old() {
    use crate::atmos_alarms::{AtmosAlarm, AtmosAlarmType};
    let mut world = setup();
    let grid = station(&mut world);
    let alarm = spawn_at(&mut world, "TestAirAlarm", grid, 1, 1);
    let probe = spawn_at(&mut world, "Probe", grid, 2, 1);
    let source = world.get::<DeviceLinkSource>(alarm).unwrap().clone();
    assert_eq!(source.ports.len(), 3);
    // На `MapInit` — высокий уровень на порт текущего состояния.
    assert!(source.last_signals["AirNormal"]);
    toggle_link(&mut world, None, alarm, probe, "AirDanger", "Close");
    toggle_link(&mut world, None, alarm, probe, "AirNormal", "Open");
    world.raise(
        alarm,
        &mut AtmosAlarm {
            alarm_type: AtmosAlarmType::Danger,
        },
    );
    // Новая связь получила высокий уровень; без сети уровни приходят импульсами.
    assert_eq!(heard(&world, probe), vec!["Open", "Open", "Close"]);
    let signals = &world.get::<DeviceLinkSource>(alarm).unwrap().last_signals;
    assert_eq!((signals["AirNormal"], signals["AirDanger"]), (false, true));
}
