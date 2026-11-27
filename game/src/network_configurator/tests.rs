use astrocore_ecs::LifecycleWorldExt;

use super::*;
use crate::device_linking::links_of;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: sourcePort
  id: Pressed
  name: signal-port-name-pressed
  defaultLinks: [ Toggle ]
- type: sinkPort
  id: Toggle
  name: signal-port-name-toggle
- type: entity
  id: Configurator
  components:
  - type: NetworkConfigurator
  - type: UserInterface
    interfaces:
      enum.NetworkConfiguratorUiKey.List:
        type: NetworkConfiguratorListBoundUserInterface
      enum.NetworkConfiguratorUiKey.Configure:
        type: NetworkConfiguratorBoundUserInterface
      enum.NetworkConfiguratorUiKey.Link:
        type: NetworkConfiguratorLinkBoundUserInterface
- type: entity
  id: Vent
  components:
  - type: DeviceNetwork
    deviceNetId: Wired
  - type: WiredNetwork
- type: entity
  id: Alarm
  components:
  - type: DeviceNetwork
    deviceNetId: Wired
  - type: WiredNetwork
  - type: DeviceList
- type: entity
  id: Button
  components:
  - type: DeviceLinkSource
    ports: [ Pressed ]
- type: entity
  id: Lamp
  components:
  - type: DeviceLinkSink
    ports: [ Toggle ]
- type: entity
  id: Body
"#;

struct Scene {
    world: World,
    tool: Entity,
    user: Entity,
    grid: Entity,
}

fn scene() -> Scene {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let tool = spawn_at(&mut world, "Configurator", grid, 0, 0);
    let user = spawn_at(&mut world, "Body", grid, 0, 1);
    Scene {
        world,
        tool,
        user,
        grid,
    }
}

fn state(world: &World, tool: Entity) -> NetworkConfigurator {
    world.get::<NetworkConfigurator>(tool).unwrap().clone()
}

#[test]
fn list_mode_saves_devices_and_configures_device_lists() {
    let mut s = scene();
    let vent = spawn_at(&mut s.world, "Vent", s.grid, 2, 2);
    let other = spawn_at(&mut s.world, "Vent", s.grid, 3, 2);
    let alarm = spawn_at(&mut s.world, "Alarm", s.grid, 4, 2);
    // Устройство только сети переводит настройщик в режим списка и запоминается.
    on_used(&mut s.world, s.tool, Some(vent), s.user, true);
    assert!(!state(&s.world, s.tool).link_mode_active);
    on_used(&mut s.world, s.tool, Some(other), s.user, true);
    on_used(&mut s.world, s.tool, Some(vent), s.user, true);
    let saved = state(&s.world, s.tool);
    assert_eq!(saved.devices.len(), 2);
    let address = s.world.get::<DeviceNetwork>(vent).unwrap().address;
    assert_eq!(saved.devices[&address], vent);
    // Недосягаемое не трогается.
    on_used(&mut s.world, s.tool, Some(alarm), s.user, false);
    assert_eq!(state(&s.world, s.tool).active_device_list, None);
    // Список тревоги: задать, скопировать, очистить.
    on_used(&mut s.world, s.tool, Some(alarm), s.user, true);
    assert_eq!(state(&s.world, s.tool).active_device_list, Some(alarm));
    handle_message(
        &mut s.world,
        s.tool,
        s.user,
        &NetworkConfiguratorMessage::Set,
    );
    let mut listed = device_network::devices_of(&s.world, alarm);
    listed.sort();
    let mut expected = vec![vent, other];
    expected.sort();
    assert_eq!(listed, expected);
    handle_message(
        &mut s.world,
        s.tool,
        s.user,
        &NetworkConfiguratorMessage::ClearDevices,
    );
    assert!(state(&s.world, s.tool).devices.is_empty());
    handle_message(
        &mut s.world,
        s.tool,
        s.user,
        &NetworkConfiguratorMessage::Copy,
    );
    assert_eq!(state(&s.world, s.tool).devices.len(), 2);
    handle_message(
        &mut s.world,
        s.tool,
        s.user,
        &NetworkConfiguratorMessage::Clear,
    );
    assert!(device_network::devices_of(&s.world, alarm).is_empty());
    // Убрать одно; удалённое устройство пропадает само.
    handle_message(
        &mut s.world,
        s.tool,
        s.user,
        &NetworkConfiguratorMessage::RemoveDevice(address),
    );
    assert_eq!(state(&s.world, s.tool).devices.len(), 1);
    s.world.delete_entity(other);
    assert!(state(&s.world, s.tool).devices.is_empty());
}

#[test]
fn link_mode_links_ports_between_source_and_sink() {
    let mut s = scene();
    let button = spawn_at(&mut s.world, "Button", s.grid, 2, 2);
    let lamp = spawn_at(&mut s.world, "Lamp", s.grid, 3, 2);
    let other_button = spawn_at(&mut s.world, "Button", s.grid, 4, 2);
    assert!(state(&s.world, s.tool).link_mode_active);
    on_used(&mut s.world, s.tool, Some(button), s.user, true);
    assert_eq!(state(&s.world, s.tool).active_device_link, Some(button));
    // Второй источник не связывается с первым.
    on_used(&mut s.world, s.tool, Some(other_button), s.user, true);
    assert_eq!(state(&s.world, s.tool).device_link_target, None);
    on_used(&mut s.world, s.tool, Some(lamp), s.user, true);
    assert_eq!(state(&s.world, s.tool).device_link_target, Some(lamp));
    handle_message(
        &mut s.world,
        s.tool,
        s.user,
        &NetworkConfiguratorMessage::LinkToggle("Pressed".into(), "Toggle".into()),
    );
    assert_eq!(
        links_of(&s.world, button, lamp),
        [DeviceLink::new("Pressed", "Toggle")].into()
    );
    handle_message(
        &mut s.world,
        s.tool,
        s.user,
        &NetworkConfiguratorMessage::LinkClear,
    );
    assert!(links_of(&s.world, button, lamp).is_empty());
    handle_message(
        &mut s.world,
        s.tool,
        s.user,
        &NetworkConfiguratorMessage::LinkSave(vec![("Pressed".into(), "Toggle".into())]),
    );
    assert_eq!(links_of(&s.world, button, lamp).len(), 1);
    // Повторный клик по начатому устройству останавливает связь.
    on_used(&mut s.world, s.tool, Some(button), s.user, true);
    assert_eq!(state(&s.world, s.tool).active_device_link, None);
    // Смена режима глаголом.
    switch_mode(&mut s.world, s.tool);
    assert!(!state(&s.world, s.tool).link_mode_active);
}

#[test]
fn link_window_state_lists_ports_links_and_defaults() {
    let mut s = scene();
    let button = spawn_at(&mut s.world, "Button", s.grid, 2, 2);
    let lamp = spawn_at(&mut s.world, "Lamp", s.grid, 3, 2);
    edit(&mut s.world, s.tool, |c| {
        c.active_device_link = Some(lamp);
        c.device_link_target = Some(button);
    });
    // Пара в любом порядке — источник слева.
    assert_eq!(link_pair(&s.world, s.tool), Some((button, lamp)));
    let state = link_ui_state(&s.world, s.tool).unwrap();
    assert_eq!(state.sources.len(), 1);
    assert_eq!(state.sinks.len(), 1);
    assert!(state.links.is_empty());
    assert_eq!(
        state.defaults,
        Some(vec![("Pressed".to_owned(), "Toggle".to_owned())])
    );
}
