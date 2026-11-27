use std::collections::HashMap;

use astrocore_ecs::GameWorldExt;

use bevy_ecs::resource::Resource;

use super::*;
use crate::testing::{spawn_at, station, world_with};

pub(crate) const PROTOTYPES: &str = r#"
- type: sourcePort
  id: Pressed
  name: signal-port-name-pressed
  defaultLinks: [ Toggle, Trigger ]
- type: sourcePort
  id: Status
  name: signal-port-name-status-transmitter
- type: sinkPort
  id: Toggle
  name: signal-port-name-toggle
- type: sinkPort
  id: Open
  name: signal-port-name-open
- type: deviceFrequency
  id: BasicDevice
  frequency: 1280
- type: entity
  id: Button
  components:
  - type: DeviceLinkSource
    ports: [ Pressed, Status ]
- type: entity
  id: Lamp
  components:
  - type: DeviceLinkSink
    ports: [ Toggle, Open ]
- type: entity
  id: RadioButton
  parent: Button
  components:
  - type: DeviceNetwork
    deviceNetId: Wireless
  - type: WirelessNetwork
    range: 5
- type: entity
  id: RadioLamp
  parent: Lamp
  components:
  - type: DeviceNetwork
    deviceNetId: Wireless
    receiveFrequencyId: BasicDevice
  - type: WirelessNetwork
    range: 5
"#;

/// Пришедшие сигналы: приёмник → «порт» или «порт:уровень».
#[derive(Resource, Default)]
struct Inbox(HashMap<Entity, Vec<String>>);

fn setup() -> World {
    let mut world = world_with(PROTOTYPES);
    world.init_resource::<Inbox>();
    world.subscribe::<DeviceLinkSink, SignalReceived>(|world, entity, event| {
        let text = event.port.clone();
        world
            .resource_mut::<Inbox>()
            .0
            .entry(entity)
            .or_default()
            .push(text);
    });
    world.subscribe::<DeviceLinkSink, LogicSignalReceived>(|world, entity, event| {
        let text = format!("{}:{:?}", event.port, event.state);
        world
            .resource_mut::<Inbox>()
            .0
            .entry(entity)
            .or_default()
            .push(text);
    });
    world.subscribe::<DeviceLinkSink, DeviceLinkOverloaded>(|world, entity, _| {
        let text = "перегрузка".to_owned();
        world
            .resource_mut::<Inbox>()
            .0
            .entry(entity)
            .or_default()
            .push(text);
    });
    world
}

fn inbox(world: &World, entity: Entity) -> Vec<String> {
    world
        .resource::<Inbox>()
        .0
        .get(&entity)
        .cloned()
        .unwrap_or_default()
}

#[test]
fn signal_reaches_linked_sink_directly_without_level() {
    let mut world = setup();
    let grid = station(&mut world);
    let button = spawn_at(&mut world, "Button", grid, 1, 1);
    let lamp = spawn_at(&mut world, "Lamp", grid, 3, 1);
    let other = spawn_at(&mut world, "Lamp", grid, 4, 1);
    assert!(toggle_link(
        &mut world, None, button, lamp, "Pressed", "Toggle"
    ));
    // Несуществующий порт приёмника не связывается.
    assert!(!toggle_link(
        &mut world, None, button, lamp, "Pressed", "Close"
    ));
    invoke_port(&mut world, button, "Pressed");
    invoke_port(&mut world, button, "Status");
    assert_eq!(inbox(&world, lamp), vec!["Toggle"]);
    assert!(inbox(&world, other).is_empty());
    // Без сети уровень не передаётся: приходит простой сигнал.
    toggle_link(&mut world, None, button, lamp, "Status", "Open");
    send_signal(&mut world, button, "Status", true);
    assert_eq!(inbox(&world, lamp), vec!["Toggle", "Open"]);
    assert!(world.get::<DeviceLinkSource>(button).unwrap().last_signals["Status"]);
    // Повторное переключение развязывает.
    assert!(toggle_link(
        &mut world, None, button, lamp, "Pressed", "Toggle"
    ));
    invoke_port(&mut world, button, "Pressed");
    assert_eq!(inbox(&world, lamp).len(), 2);
    assert_eq!(
        linked_sinks(&world, button, "Status"),
        BTreeSet::from([lamp])
    );
}

#[test]
fn level_travels_over_wireless_network_within_range() {
    let mut world = setup();
    let grid = station(&mut world);
    let button = spawn_at(&mut world, "RadioButton", grid, 1, 1);
    let near = spawn_at(&mut world, "RadioLamp", grid, 4, 1);
    let far = spawn_at(&mut world, "RadioLamp", grid, 13, 1);
    world.run_tick();
    assert!(toggle_link(
        &mut world, None, button, near, "Status", "Open"
    ));
    assert!(toggle_link(&mut world, None, button, far, "Status", "Open"));
    send_signal(&mut world, button, "Status", true);
    send_signal(&mut world, button, "Status", false);
    invoke_port(&mut world, button, "Status");
    assert_eq!(inbox(&world, near), vec!["Open:High", "Open:Low", "Open"]);
    // Связь в пределах 30, но беспроводная сеть кнопки достаёт только на 5.
    assert!(inbox(&world, far).is_empty());
}

#[test]
fn new_link_receives_last_level_and_defaults_follow_prototype() {
    let mut world = setup();
    let grid = station(&mut world);
    let button = spawn_at(&mut world, "RadioButton", grid, 1, 1);
    let lamp = spawn_at(&mut world, "RadioLamp", grid, 3, 1);
    world.run_tick();
    send_signal(&mut world, button, "Status", true);
    toggle_link(&mut world, None, button, lamp, "Status", "Open");
    assert_eq!(inbox(&world, lamp), vec!["Open:High"]);
    // По умолчанию у Pressed — Toggle и Trigger, но у Status связей по умолчанию нет:
    // тогда не связывается ничего (как `GetDefaults`).
    link_defaults(&mut world, None, button, lamp);
    assert!(links_of(&world, button, lamp).is_empty());
    let ports = BTreeSet::from(["Pressed".to_owned()]);
    assert_eq!(
        default_links(&world, &ports),
        vec![
            DeviceLink::new("Pressed", "Toggle"),
            DeviceLink::new("Pressed", "Trigger")
        ]
    );
    let defaults = default_links(&world, &ports);
    save_links(&mut world, None, button, lamp, &defaults);
    assert_eq!(
        links_of(&world, button, lamp),
        BTreeSet::from([DeviceLink::new("Pressed", "Toggle")])
    );
}

#[test]
fn overloaded_sink_loses_all_links() {
    let mut world = setup();
    let grid = station(&mut world);
    let button = spawn_at(&mut world, "Button", grid, 1, 1);
    let lamp = spawn_at(&mut world, "Lamp", grid, 3, 1);
    toggle_link(&mut world, None, button, lamp, "Pressed", "Toggle");
    for _ in 0..12 {
        invoke_port(&mut world, button, "Pressed");
    }
    let got = inbox(&world, lamp);
    assert_eq!(got.len(), 12);
    assert_eq!(got.last().map(String::as_str), Some("перегрузка"));
    assert!(links_of(&world, button, lamp).is_empty());
    assert!(
        world
            .get::<DeviceLinkSink>(lamp)
            .unwrap()
            .linked_sources
            .is_empty()
    );
    // Счётчик убывает на единицу за такт: через несколько тактов можно снова.
    toggle_link(&mut world, None, button, lamp, "Pressed", "Toggle");
    for _ in 0..3 {
        world.run_tick();
    }
    invoke_port(&mut world, button, "Pressed");
    assert_eq!(inbox(&world, lamp).len(), 13);
}

#[test]
fn deleting_sink_or_source_unlinks() {
    let mut world = setup();
    let grid = station(&mut world);
    let button = spawn_at(&mut world, "Button", grid, 1, 1);
    let lamp = spawn_at(&mut world, "Lamp", grid, 3, 1);
    let other = spawn_at(&mut world, "Button", grid, 5, 1);
    toggle_link(&mut world, None, button, lamp, "Pressed", "Toggle");
    toggle_link(&mut world, None, other, lamp, "Pressed", "Toggle");
    astrocore_ecs::LifecycleWorldExt::delete_entity(&mut world, other);
    assert_eq!(
        world.get::<DeviceLinkSink>(lamp).unwrap().linked_sources,
        BTreeSet::from([button])
    );
    astrocore_ecs::LifecycleWorldExt::delete_entity(&mut world, lamp);
    let source = world.get::<DeviceLinkSource>(button).unwrap();
    assert!(source.linked_ports.is_empty());
    assert!(source.outputs.values().all(BTreeSet::is_empty));
}

#[test]
fn links_from_map_are_adopted_and_invalid_ones_dropped() {
    let mut world = setup();
    let grid = station(&mut world);
    let button = spawn_at(&mut world, "Button", grid, 1, 1);
    let lamp = spawn_at(&mut world, "Lamp", grid, 3, 1);
    let fields = astrocore_data::parse_yaml(
        "linkedPorts:\n  7:\n  - - Pressed\n    - Toggle\n  - - Pressed\n    - Close\n  9:\n  - - Pressed\n    - Toggle\n",
    )
    .unwrap()
    .as_map()
    .cloned()
    .unwrap();
    world.entity_mut(button).insert(MapPreserved {
        components: Vec::new(),
        missing: Vec::new(),
        fields: vec![("DeviceLinkSource".into(), fields)],
    });
    let uids = HashMap::from([(3, button), (7, lamp)]);
    world.raise_broadcast(&mut MapLoaded {
        maps: Vec::new(),
        grids: vec![grid],
        uids,
    });
    assert_eq!(
        links_of(&world, button, lamp),
        BTreeSet::from([DeviceLink::new("Pressed", "Toggle")])
    );
    assert_eq!(
        world.get::<DeviceLinkSink>(lamp).unwrap().linked_sources,
        BTreeSet::from([button])
    );
    invoke_port(&mut world, button, "Pressed");
    assert_eq!(inbox(&world, lamp), vec!["Toggle"]);
}
