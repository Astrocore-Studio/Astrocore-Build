use astrocore_ecs::{GameWorldExt, LifecycleWorldExt};
use astrocore_map::{ContainerKind, ContainerWorldExt};

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Source
  components:
  - type: RadiationSource
    intensity: 10
    slope: 1
- type: entity
  id: Victim
  components:
  - type: Damageable
  - type: Injurable
  - type: RadiationReceiver
- type: entity
  id: LeadWall
  components:
  - type: Transform
    anchored: true
  - type: RadiationBlocker
    resistance: 2
- type: entity
  id: PlasmaTank
  components:
  - type: Item
  - type: GasTank
    air:
      volume: 5
      temperature: 293.15
      moles: [ 0, 0, 0, 10 ]
- type: entity
  id: Collector
  components:
  - type: Transform
    anchored: true
  - type: Appearance
  - type: RadiationReceiver
  - type: PowerSupplier
  - type: RadiationCollector
    enabled: true
    chargeModifier: 15000
    radiationReactiveGases:
    - reactantPrototype: Plasma
      powerGenerationEfficiency: 1
      reactantBreakdownRate: 0.0001
"#;

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

#[test]
fn line_visits_every_cell_between_ends() {
    assert_eq!(grid_line((0, 0), (3, 1)), [(0, 0), (1, 0), (2, 1), (3, 1)]);
    assert_eq!(
        grid_line((2, 2), (2, -1)),
        [(2, 2), (2, 1), (2, 0), (2, -1)]
    );
}

#[test]
fn radiation_fades_with_distance_and_walls_and_hurts() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    spawn_at(&mut world, "Source", grid, 2, 8);
    let victim = spawn_at(&mut world, "Victim", grid, 6, 8);
    run(&mut world, 1.05);
    // 10 − 1 × 4 м = 6 рад/с за секунду.
    assert_eq!(
        world
            .get::<RadiationReceiver>(victim)
            .unwrap()
            .current_radiation,
        6.0
    );
    assert_eq!(world.total_damage(victim).to_string(), "6");

    let wall = spawn_at(&mut world, "LeadWall", grid, 4, 8);
    run(&mut world, 1.0);
    assert_eq!(
        world
            .get::<RadiationReceiver>(victim)
            .unwrap()
            .current_radiation,
        4.0
    );
    world.queue_delete(wall);
    run(&mut world, 1.0);
    assert_eq!(
        world
            .get::<RadiationReceiver>(victim)
            .unwrap()
            .current_radiation,
        6.0
    );
}

#[test]
fn collector_turns_radiation_and_plasma_into_power() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let collector = spawn_at(&mut world, "Collector", grid, 6, 8);
    let tank = spawn_at(&mut world, "PlasmaTank", grid, 6, 8);
    world.ensure_container(collector, GAS_TANK_CONTAINER, ContainerKind::Slot);
    world
        .insert_into_container(tank, collector, GAS_TANK_CONTAINER)
        .unwrap();
    let source = spawn_at(&mut world, "Source", grid, 2, 8);
    run(&mut world, 1.05);
    // 6 рад × 10 моль × 15000 × 1.5 × 293.15 / 443.15.
    let supply = world.get::<PowerSupplier>(collector).unwrap().supply_rate;
    assert!(
        (supply - 6.0 * 10.0 * 15000.0 * 1.5 * 293.15 / 443.15).abs() < 1.0,
        "{supply}"
    );
    let plasma = crate::gas_tank::tank_air(&world, tank).unwrap().moles[3];
    assert!(plasma < 10.0 && plasma > 9.99);

    // Без излучения ток пропадает через две секунды.
    world.queue_delete(source);
    run(&mut world, 3.1);
    assert_eq!(
        world.get::<PowerSupplier>(collector).unwrap().supply_rate,
        0.0
    );
}
