use std::collections::HashMap;

use astrocore_ecs::LifecycleWorldExt;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: deviceFrequency
  id: AtmosMonitor
  frequency: 1621
- type: entity
  id: WiredDevice
  components:
  - type: DeviceNetwork
    deviceNetId: Wired
    receiveFrequencyId: AtmosMonitor
    transmitFrequencyId: AtmosMonitor
  - type: WiredNetwork
  - type: LinkedDeviceNetwork
- type: entity
  id: Radio
  components:
  - type: DeviceNetwork
    deviceNetId: Wireless
    receiveFrequency: 1459
    transmitFrequency: 1459
  - type: WirelessNetwork
    range: 5
- type: entity
  id: Alarm
  parent: WiredDevice
  components:
  - type: DeviceNetwork
    deviceNetId: Wired
    receiveFrequencyId: AtmosMonitor
    transmitFrequencyId: AtmosMonitor
    sendBroadcastAttemptEvent: true
  - type: DeviceList
"#;

/// Полученные пакеты: получатель → данные.
#[derive(bevy_ecs::resource::Resource, Default)]
struct Inbox(HashMap<Entity, Vec<String>>);

#[derive(Debug, Clone)]
struct Ping(String);

fn setup() -> World {
    let mut world = world_with(PROTOTYPES);
    world.init_resource::<Inbox>();
    world.subscribe::<DeviceNetwork, DeviceNetworkPacket<Ping>>(|world, entity, packet| {
        let text = packet.data.0.clone();
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
fn wired_broadcast_stays_on_grid_and_addresses_work() {
    let mut world = setup();
    let grid = station(&mut world);
    let other = station(&mut world);
    let a = spawn_at(&mut world, "WiredDevice", grid, 1, 1);
    let b = spawn_at(&mut world, "WiredDevice", grid, 9, 9);
    let c = spawn_at(&mut world, "WiredDevice", grid, 5, 5);
    let far = spawn_at(&mut world, "WiredDevice", other, 1, 1);
    // Частота из прототипа, адрес случайный.
    assert_eq!(
        world.get::<DeviceNetwork>(a).unwrap().receive_frequency,
        Some(1621)
    );
    assert!(is_device_connected(&world, a));
    assert_ne!(world.get::<DeviceNetwork>(a).unwrap().address, 0);
    assert!(send_packet(
        &mut world,
        a,
        None,
        Ping("всем".into()),
        None,
        None
    ));
    assert_eq!(inbox(&world, b), ["всем"]);
    assert_eq!(inbox(&world, c), ["всем"]);
    assert!(inbox(&world, a).is_empty());
    assert!(inbox(&world, far).is_empty());
    let address = world.get::<DeviceNetwork>(b).unwrap().address;
    send_packet(
        &mut world,
        a,
        Some(address),
        Ping("тебе".into()),
        None,
        None,
    );
    assert_eq!(inbox(&world, b), ["всем", "тебе"]);
    assert_eq!(inbox(&world, c), ["всем"]);
    // Удалённое устройство выходит из сети.
    world.delete_entity(c);
    assert!(
        !world
            .resource::<DeviceNetworks>()
            .0
            .get(&WIRED)
            .unwrap()
            .devices
            .values()
            .any(|d| d.owner == c)
    );
}

#[test]
fn wireless_respects_range() {
    let mut world = setup();
    let grid = station(&mut world);
    let a = spawn_at(&mut world, "Radio", grid, 1, 1);
    let near = spawn_at(&mut world, "Radio", grid, 4, 1);
    let far = spawn_at(&mut world, "Radio", grid, 12, 1);
    send_packet(&mut world, a, None, Ping("приём".into()), None, None);
    assert_eq!(inbox(&world, near), ["приём"]);
    assert!(inbox(&world, far).is_empty());
}

#[test]
fn device_list_limits_broadcast() {
    let mut world = setup();
    let grid = station(&mut world);
    let alarm = spawn_at(&mut world, "Alarm", grid, 1, 1);
    let listed = spawn_at(&mut world, "WiredDevice", grid, 2, 1);
    let other = spawn_at(&mut world, "WiredDevice", grid, 3, 1);
    // Пустой список-разрешение — рассылки нет.
    send_packet(&mut world, alarm, None, Ping("1".into()), None, None);
    assert!(inbox(&world, listed).is_empty());
    assert_eq!(
        update_device_list(&mut world, alarm, &[listed], false),
        DeviceListUpdateResult::UpdateOk
    );
    assert!(
        world
            .get::<LinkedDeviceNetwork>(listed)
            .unwrap()
            .device_lists
            .contains(&alarm)
    );
    send_packet(&mut world, alarm, None, Ping("2".into()), None, None);
    assert_eq!(inbox(&world, listed), ["2"]);
    assert!(inbox(&world, other).is_empty());
    // Устройство удалено — пропало из списка.
    world.delete_entity(listed);
    assert!(devices_of(&world, alarm).is_empty());
}

#[test]
fn map_device_lists_are_adopted() {
    let mut world = setup();
    let grid = station(&mut world);
    let alarm = spawn_at(&mut world, "Alarm", grid, 1, 1);
    let device = spawn_at(&mut world, "WiredDevice", grid, 2, 1);
    let node = astrocore_data::parse_yaml("devices:\n- 17\n- 99\n").unwrap();
    let fields = node.as_map().unwrap().clone();
    world.entity_mut(alarm).insert(MapPreserved {
        components: Vec::new(),
        missing: Vec::new(),
        fields: vec![("DeviceList".into(), fields)],
    });
    let loaded = MapLoaded {
        maps: Vec::new(),
        grids: vec![grid],
        uids: HashMap::from([(5, alarm), (17, device)]),
    };
    adopt_device_lists(&mut world, &loaded);
    assert_eq!(devices_of(&world, alarm), [device]);
}

#[test]
fn address_text_matches_ss14() {
    assert_eq!(address_string(0x1234_ABCD), "1234-ABCD");
}

/// Поле компонента записи сущности в сохранённом документе.
pub(crate) fn saved_field(
    save: &astrocore_map::MapSave,
    entity: Entity,
    component: &str,
    field: &str,
) -> Option<DataNode> {
    let uid = save.uids[&entity].to_string();
    save.document
        .as_map()?
        .get("entities")?
        .as_seq()?
        .iter()
        .filter_map(|group| group.as_map()?.get("entities")?.as_seq())
        .flatten()
        .find(|e| {
            e.as_map()
                .and_then(|m| m.get("uid"))
                .and_then(DataNode::as_str)
                == Some(&uid)
        })?
        .as_map()?
        .get("components")?
        .as_seq()?
        .iter()
        .find(|c| {
            c.as_map()
                .and_then(|m| m.get("type"))
                .and_then(DataNode::as_str)
                == Some(component)
        })?
        .as_map()?
        .get(field)
        .cloned()
}

#[test]
fn device_lists_changed_in_game_are_saved() {
    use astrocore_map::{MapSaveOptions, MapSaveWorldExt};
    let mut world = setup();
    let grid = station(&mut world);
    let map = world.get::<Transform>(grid).unwrap().map().unwrap();
    let alarm = spawn_at(&mut world, "Alarm", grid, 1, 1);
    let first = spawn_at(&mut world, "WiredDevice", grid, 2, 1);
    let second = spawn_at(&mut world, "WiredDevice", grid, 3, 1);
    update_device_list(&mut world, alarm, &[first, second], false);
    let save = |world: &mut World| {
        let mut save = world
            .save_map(
                map,
                MapSaveOptions {
                    allow_initialized: true,
                },
            )
            .unwrap();
        write_references(world, &mut save);
        save
    };
    let saved = save(&mut world);
    let devices: Vec<String> = saved_field(&saved, alarm, "DeviceList", "devices")
        .unwrap()
        .as_seq()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_owned())
        .collect();
    let mut expected = vec![
        saved.uids[&first].to_string(),
        saved.uids[&second].to_string(),
    ];
    expected.sort_by_key(|u| u.parse::<u32>().unwrap());
    let mut sorted = devices.clone();
    sorted.sort_by_key(|u| u.parse::<u32>().unwrap());
    assert_eq!(sorted, expected);

    // Как в файле — поле не трогается (порядок номеров файла сохраняется).
    let reversed = astrocore_data::parse_yaml(&format!(
        "devices:\n- {}\n- {}\n",
        saved.uids[&second], saved.uids[&first]
    ))
    .unwrap();
    world.entity_mut(alarm).insert(MapPreserved {
        components: Vec::new(),
        missing: Vec::new(),
        fields: vec![("DeviceList".into(), reversed.as_map().unwrap().clone())],
    });
    let again = save(&mut world);
    let kept: Vec<String> = saved_field(&again, alarm, "DeviceList", "devices")
        .unwrap()
        .as_seq()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        kept,
        [
            again.uids[&second].to_string(),
            again.uids[&first].to_string()
        ]
    );
    // Очищенный список — поля нет.
    update_device_list(&mut world, alarm, &[], false);
    let cleared = save(&mut world);
    assert!(saved_field(&cleared, alarm, "DeviceList", "devices").is_none());
}
