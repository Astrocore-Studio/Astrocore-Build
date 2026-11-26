use astrocore_ecs::{EventWorldExt, GameWorldExt};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Anchored
  abstract: true
  components:
  - type: Transform
    anchored: true
- type: entity
  parent: Anchored
  id: CableHV
  components:
  - type: NodeContainer
    nodes:
      power: !type:CableNode
        nodeGroupID: HVPower
- type: entity
  parent: Anchored
  id: CableMV
  components:
  - type: NodeContainer
    nodes:
      power: !type:CableNode
        nodeGroupID: MVPower
- type: entity
  parent: Anchored
  id: Generator
  components:
  - type: NodeContainer
    nodes:
      output: !type:CableDeviceNode
        nodeGroupID: HVPower
  - type: PowerSupplier
    supplyRate: 3000
- type: entity
  parent: Anchored
  id: Substation
  components:
  - type: NodeContainer
    nodes:
      input: !type:CableDeviceNode
        nodeGroupID: HVPower
      output: !type:CableDeviceNode
        nodeGroupID: MVPower
  - type: BatteryCharger
    voltage: High
  - type: BatteryDischarger
    voltage: Medium
  - type: Battery
    maxCharge: 2500000
    startingCharge: 0
  - type: PowerNetworkBattery
    maxSupply: 150000
    maxChargeRate: 5000
    supplyRampTolerance: 5000
    supplyRampRate: 1000
- type: entity
  parent: Anchored
  id: APC
  components:
  - type: NodeContainer
    nodes:
      input: !type:CableDeviceNode
        nodeGroupID: MVPower
      output: !type:CableDeviceNode
        nodeGroupID: Apc
  - type: BatteryCharger
    voltage: Medium
  - type: PowerProvider
    voltage: Apc
  - type: Apc
    voltage: Apc
  - type: ExtensionCableProvider
  - type: Battery
    maxCharge: 50000
    startingCharge: 0
  - type: PowerNetworkBattery
    maxSupply: 10000
    maxChargeRate: 5000
    supplyRampTolerance: 1000
    supplyRampRate: 500
- type: entity
  parent: Anchored
  id: Lamp
  components:
  - type: ApcPowerReceiver
    powerLoad: 60
  - type: ExtensionCableReceiver
"#;

#[derive(bevy_ecs::resource::Resource, Default)]
struct Changes(Vec<bool>);

/// Генератор → кабели ВН → подстанция → кабели СН → АПЦ → лампа в двух клетках от АПЦ.
fn station_grid(world: &mut World) -> (Entity, Entity) {
    let grid = station(world);
    spawn_at(world, "Generator", grid, 1, 1);
    for x in 1..=3 {
        spawn_at(world, "CableHV", grid, x, 1);
    }
    spawn_at(world, "Substation", grid, 3, 1);
    for x in 3..=6 {
        spawn_at(world, "CableMV", grid, x, 1);
    }
    let apc = spawn_at(world, "APC", grid, 6, 1);
    let lamp = spawn_at(world, "Lamp", grid, 6, 3);
    (apc, lamp)
}

#[test]
fn lamp_gets_power_through_substation_and_apc() {
    let mut world = world_with(PROTOTYPES);
    world.init_resource::<Changes>();
    world.subscribe::<ApcPowerReceiver, PowerChanged>(|world, _, event: &mut PowerChanged| {
        world.resource_mut::<Changes>().0.push(event.powered);
    });
    let (apc, lamp) = station_grid(&mut world);
    // Рампы источников и аккумуляторов разгоняются за несколько секунд.
    for _ in 0..300 {
        world.run_tick();
    }
    assert!(is_powered(&world, lamp));
    assert_eq!(world.resource::<Changes>().0, [true]);
    let received = world.get::<ApcPowerReceiver>(lamp).unwrap().received_power;
    assert!((received - 60.0).abs() < 0.01, "{received}");
    // Аккумулятор АПЦ заряжается.
    assert!(world.get::<Battery>(apc).unwrap().last_charge > 0.0);

    // Главный выключатель АПЦ гасит лампу.
    world.get_mut::<Apc>(apc).unwrap().enabled = false;
    world.run_tick();
    assert!(!is_powered(&world, lamp));
    assert_eq!(world.resource::<Changes>().0, [true, false]);
}

#[test]
fn lamp_out_of_range_of_any_apc_stays_dark() {
    let mut world = world_with(PROTOTYPES);
    let (_, _) = station_grid(&mut world);
    let far = {
        let grid = world
            .query::<(Entity, &astrocore_map::MapGrid)>()
            .iter(&world)
            .next()
            .unwrap()
            .0;
        spawn_at(&mut world, "Lamp", grid, 6, 8)
    };
    for _ in 0..300 {
        world.run_tick();
    }
    assert!(!is_powered(&world, far));
    assert!(world.get::<ApcPowered>(far).is_none());
}

#[test]
fn paused_entities_take_no_part_in_the_grid() {
    use astrocore_ecs::LifecycleWorldExt;

    let mut world = world_with(PROTOTYPES);
    let (_, lamp) = station_grid(&mut world);
    world.set_entity_paused(lamp, true);
    for _ in 0..300 {
        world.run_tick();
    }
    assert!(!is_powered(&world, lamp));
}
