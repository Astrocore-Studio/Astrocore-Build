use astrocore_ecs::GameWorldExt;
use astrocore_map::TileCoord;

use super::*;
use crate::atmos::gas::CELL_VOLUME;
use crate::atmos::{ensure_grid_atmosphere, with_tile_mixture};
use crate::device_network::update_device_list;
use crate::doors::{Door, DoorState};
use crate::testing::{spawn_at, station, world_with};

pub(crate) const PROTOTYPES: &str = r#"
- type: deviceFrequency
  id: AtmosMonitor
  frequency: 1621
- type: alarmThreshold
  id: testPressure
  upperBound: !type:AlarmThresholdSetting
    threshold: 550
  lowerBound: !type:AlarmThresholdSetting
    threshold: 20
  upperWarnAround: !type:AlarmThresholdSetting
    threshold: 0.7
  lowerWarnAround: !type:AlarmThresholdSetting
    threshold: 2.5
- type: entity
  id: AtmosNetDevice
  abstract: true
  components:
  - type: DeviceNetwork
    deviceNetId: AtmosDevices
    receiveFrequencyId: AtmosMonitor
    transmitFrequencyId: AtmosMonitor
    sendBroadcastAttemptEvent: true
  - type: WiredNetwork
  - type: ApcPowerReceiver
    needsPower: false
- type: entity
  id: TestSensor
  parent: AtmosNetDevice
  components:
  - type: Transform
    anchored: true
  - type: AtmosDevice
  - type: Tag
    tags: [ AirSensor ]
  - type: AtmosMonitor
    pressureThresholdId: testPressure
  - type: LinkedDeviceNetwork
- type: entity
  id: TestFireAlarm
  parent: AtmosNetDevice
  components:
  - type: Transform
    anchored: true
  - type: DeviceList
  - type: AtmosAlarmable
    syncWith: [ FireAlarm, AirSensor, GasVent ]
  - type: Tag
    tags: [ FireAlarm ]
  - type: FireAlarm
  - type: Appearance
- type: entity
  id: TestFirelock
  parent: AtmosNetDevice
  components:
  - type: Transform
    anchored: true
  - type: Airtight
  - type: Door
    state: Open
  - type: Firelock
  - type: AtmosAlarmable
    syncWith: [ FireAlarm, AirAlarm ]
  - type: Appearance
  - type: LinkedDeviceNetwork
- type: entity
  id: TestVent
  parent: AtmosNetDevice
  components:
  - type: Transform
    anchored: true
  - type: GasVentPump
  - type: Tag
    tags: [ GasVent ]
  - type: LinkedDeviceNetwork
- type: entity
  id: TestAirAlarm
  parent: AtmosNetDevice
  components:
  - type: Transform
    anchored: true
  - type: DeviceList
  - type: AtmosAlarmable
    syncWith: [ AirAlarm, AirSensor, GasVent ]
  - type: Tag
    tags: [ AirAlarm ]
  - type: AirAlarm
  - type: Appearance
"#;

fn ticks(world: &mut World, count: usize) {
    for _ in 0..count {
        world.run_tick();
    }
}

fn fill_air(world: &mut World, grid: Entity) {
    for x in 0..16 {
        for y in 0..16 {
            with_tile_mixture(world, grid, TileCoord::new(x, y, 0), |tile| {
                tile.copy_from(&GasMixture::standard_air(CELL_VOLUME))
            });
        }
    }
}

fn setup() -> (World, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    ensure_grid_atmosphere(&mut world, grid);
    ticks(&mut world, 10);
    (world, grid)
}

fn door_state(world: &World, door: Entity) -> DoorState {
    world.get::<Door>(door).unwrap().state
}

#[test]
fn threshold_levels_match_ss14() {
    let mut world = world_with(PROTOTYPES);
    world.run_tick();
    let threshold = threshold_prototype(&world, "testPressure").unwrap();
    assert_eq!(threshold.check(101.0), None);
    // Предупреждение ниже 20 × 2,5 = 50 кПа и выше 550 × 0,7 = 385 кПа.
    assert_eq!(threshold.check(45.0), Some(AtmosAlarmType::Warning));
    assert_eq!(threshold.check(400.0), Some(AtmosAlarmType::Warning));
    assert_eq!(threshold.check(10.0), Some(AtmosAlarmType::Danger));
    assert_eq!(threshold.check(600.0), Some(AtmosAlarmType::Danger));
}

#[test]
fn fire_alarm_pull_closes_and_reset_opens_firelock() {
    let (mut world, grid) = setup();
    fill_air(&mut world, grid);
    let alarm = spawn_at(&mut world, "TestFireAlarm", grid, 2, 2);
    let firelock = spawn_at(&mut world, "TestFirelock", grid, 5, 5);
    update_device_list(&mut world, alarm, &[firelock], false);
    ticks(&mut world, 2);
    assert_eq!(door_state(&world, firelock), DoorState::Open);
    let mut hand = crate::interaction::InteractHand {
        user: alarm,
        target: alarm,
        handled: false,
    };
    world.raise(alarm, &mut hand);
    assert_eq!(
        world.get::<AtmosAlarmable>(alarm).unwrap().last_alarm_state,
        AtmosAlarmType::Danger
    );
    ticks(&mut world, 60);
    assert_eq!(door_state(&world, firelock), DoorState::Closed);
    let mut hand = crate::interaction::InteractHand {
        user: alarm,
        target: alarm,
        handled: false,
    };
    world.raise(alarm, &mut hand);
    ticks(&mut world, 60);
    assert_eq!(door_state(&world, firelock), DoorState::Open);
}

#[test]
fn vacuum_trips_sensor_and_fire_alarm() {
    let (mut world, grid) = setup();
    fill_air(&mut world, grid);
    let sensor = spawn_at(&mut world, "TestSensor", grid, 8, 8);
    let alarm = spawn_at(&mut world, "TestFireAlarm", grid, 2, 2);
    let firelock = spawn_at(&mut world, "TestFirelock", grid, 5, 5);
    update_device_list(&mut world, alarm, &[sensor, firelock], false);
    ticks(&mut world, 30);
    assert!(
        world
            .get::<AtmosMonitor>(sensor)
            .unwrap()
            .registered_devices
            .contains(&world.get::<DeviceNetwork>(alarm).unwrap().address)
    );
    assert_ne!(
        world.get::<AtmosAlarmable>(alarm).unwrap().last_alarm_state,
        AtmosAlarmType::Danger
    );
    // Клетка датчика без воздуха: ниже 20 кПа — опасность.
    for _ in 0..10 {
        with_tile_mixture(&mut world, grid, TileCoord::new(8, 8, 0), |tile| {
            tile.clear()
        });
        ticks(&mut world, 3);
    }
    assert_eq!(
        world.get::<AtmosMonitor>(sensor).unwrap().last_alarm_state,
        AtmosAlarmType::Danger
    );
    assert_eq!(
        world.get::<AtmosAlarmable>(alarm).unwrap().last_alarm_state,
        AtmosAlarmType::Danger
    );
    // Тревога дошла до шлюза через пожарную тревогу, и он закрывается. Когда воздух натечёт
    // обратно, датчик вернётся в норму и шлюз откроется сам — как в SS14.
    assert_eq!(
        world
            .get::<AtmosAlarmable>(firelock)
            .unwrap()
            .last_alarm_state,
        AtmosAlarmType::Danger
    );
    assert!(matches!(
        door_state(&world, firelock),
        DoorState::Closing | DoorState::Closed
    ));
}

#[test]
fn air_alarm_panic_mode_turns_vents_off() {
    let (mut world, grid) = setup();
    fill_air(&mut world, grid);
    let alarm = spawn_at(&mut world, "TestAirAlarm", grid, 2, 2);
    let vent = spawn_at(&mut world, "TestVent", grid, 4, 4);
    update_device_list(&mut world, alarm, &[vent], false);
    ticks(&mut world, 2);
    // Вентиляция ответила своими данными на опрос.
    assert_eq!(world.get::<AirAlarm>(alarm).unwrap().vent_data.len(), 1);
    set_mode(&mut world, alarm, AirAlarmMode::Panic, false);
    assert!(!world.get::<GasVentPump>(vent).unwrap().enabled);
    set_mode(&mut world, alarm, AirAlarmMode::Fill, false);
    let vent_state = world.get::<GasVentPump>(vent).unwrap();
    assert!(vent_state.enabled);
    assert!((vent_state.external_pressure_bound - 5066.25).abs() < 0.1);
}
