use astrocore_ecs::{EventWorldExt, LifecycleWorldExt};

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
- type: entity
  id: ActionClearNetworkLinkOverlays
  components:
  - type: Action
    clientExclusive: true
    temporary: true
  - type: InstantAction
    event: !type:ClearAllOverlaysEvent
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

#[test]
fn addresses_read_as_numbers_or_hex() {
    assert_eq!(parse_address("1234"), Some(1234));
    assert_eq!(parse_address("Vnt-00AB-00CD"), Some(0x00AB));
    assert_eq!(parse_address("00AB"), Some(0xAB));
    assert_eq!(parse_address("zz"), None);
}

#[test]
fn saved_devices_go_into_the_map_and_come_back() {
    use astrocore_map::{MapSaveOptions, MapSaveWorldExt};
    let mut s = scene();
    let vent = spawn_at(&mut s.world, "Vent", s.grid, 2, 2);
    switch_mode(&mut s.world, s.tool);
    on_used(&mut s.world, s.tool, Some(vent), s.user, true);
    let map = s.world.get::<Transform>(s.grid).unwrap().map().unwrap();
    let mut save = s
        .world
        .save_map(
            map,
            MapSaveOptions {
                allow_initialized: true,
            },
        )
        .unwrap();
    write_references(&mut s.world, &mut save);
    let devices =
        crate::device_network::tests::saved_field(&save, s.tool, "NetworkConfigurator", "devices")
            .unwrap();
    let address = s.world.get::<DeviceNetwork>(vent).unwrap().address;
    let (key, uid) = {
        let map = devices.as_map().unwrap();
        let (key, uid) = map.iter().next().unwrap();
        (key.to_owned(), uid.as_str().unwrap().to_owned())
    };
    assert_eq!(key, address.to_string());
    assert_eq!(uid, save.uids[&vent].to_string());
    assert!(
        crate::device_network::tests::saved_field(
            &save,
            s.tool,
            "NetworkConfigurator",
            "namedDevices"
        )
        .is_some()
    );

    // Загрузка: настройщик из карты помнит устройство.
    let other = spawn_at(&mut s.world, "Configurator", s.grid, 5, 5);
    let fields = astrocore_data::parse_yaml(&format!("devices:\n  {address}: 40\n"))
        .unwrap()
        .as_map()
        .cloned()
        .unwrap();
    s.world.entity_mut(other).insert(MapPreserved {
        components: Vec::new(),
        missing: Vec::new(),
        fields: vec![("NetworkConfigurator".into(), fields)],
    });
    let loaded = MapLoaded {
        maps: Vec::new(),
        grids: vec![s.grid],
        uids: std::collections::HashMap::from([(39, other), (40, vent)]),
    };
    adopt_devices(&mut s.world, &loaded);
    assert_eq!(state(&s.world, other).devices[&address], vent);
    assert!(state(&s.world, other).named_devices.contains_key(&address));
}

#[test]
fn shown_lists_give_a_clear_action_until_cleared() {
    let mut s = scene();
    let alarm = spawn_at(&mut s.world, "Alarm", s.grid, 4, 2);
    show_list(&mut s.world, s.user, alarm, true);
    let action = s.world.resource::<NetworkLinkOverlays>().shown[&s.user]
        .1
        .unwrap();
    assert!(crate::actions::ActionsWorldExt::actions_of(&s.world, s.user).contains(&action));
    // Убрать последний показанный — действие пропадает.
    show_list(&mut s.world, s.user, alarm, false);
    assert!(
        !s.world
            .resource::<NetworkLinkOverlays>()
            .shown
            .contains_key(&s.user)
    );
    assert!(!s.world.is_alive(action));
    // Действие «убрать показ» убирает всё.
    show_list(&mut s.world, s.user, alarm, true);
    let action = s.world.resource::<NetworkLinkOverlays>().shown[&s.user]
        .1
        .unwrap();
    let mut event = crate::actions::ActionEvent {
        kind: "ClearAllOverlaysEvent".into(),
        data: astrocore_data::DataNode::plain(""),
        performer: s.user,
        action,
        target: None,
        coordinates: None,
        handled: false,
        toggle: false,
    };
    s.world.raise_broadcast(&mut event);
    assert!(event.handled);
    assert!(
        !s.world
            .resource::<NetworkLinkOverlays>()
            .shown
            .contains_key(&s.user)
    );
}
