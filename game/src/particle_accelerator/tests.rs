use astrocore_ecs::GameWorldExt;
use astrocore_map::{EntityCoordinates, SpawnWorldExt, TransformWorldExt};
use astrocore_math::{Angle, Vec2};

use super::*;
use crate::multipart::{MultipartMachine, rescan};
use crate::testing::{station, world_with};

const PROTOTYPES: &str = r#"
- type: constructionGraph
  id: TestPart
  start: start
  graph:
  - node: start
  - node: completed
- type: entity
  id: TestPartBase
  abstract: true
  components:
  - type: Transform
    anchored: true
  - type: Appearance
  - type: MultipartMachinePart
  - type: Construction
    graph: TestPart
    node: completed
- type: entity
  id: EndCap
  parent: TestPartBase
  components:
  - type: ParticleAcceleratorEndCap
- type: entity
  id: FuelChamber
  parent: TestPartBase
  components:
  - type: ParticleAcceleratorFuelChamber
- type: entity
  id: PowerBox
  parent: TestPartBase
  components:
  - type: ParticleAcceleratorPowerBox
  - type: PowerConsumer
    drawRate: 500
- type: entity
  id: PaEmitter
  parent: TestPartBase
  components:
  - type: ParticleAcceleratorEmitter
    emittedPrototype: Particle
- type: entity
  id: Particle
  components:
  - type: Physics
  - type: Appearance
  - type: ParticleProjectile
  - type: SinguloFood
- type: entity
  id: ControlBox
  components:
  - type: Transform
    anchored: true
  - type: Appearance
  - type: ParticleAcceleratorControlBox
  - type: MultipartMachine
    parts:
      enum.AcceleratorParts.EndCap:
        component: ParticleAcceleratorEndCap
        offset: 1, -1
        rotation: -90
        graph: TestPart
        expectedNode: completed
      enum.AcceleratorParts.FuelChamber:
        component: ParticleAcceleratorFuelChamber
        offset: 0, -1
        rotation: -90
        graph: TestPart
        expectedNode: completed
      enum.AcceleratorParts.PowerBox:
        component: ParticleAcceleratorPowerBox
        offset: -1, -1
        rotation: -90
        graph: TestPart
        expectedNode: completed
      enum.AcceleratorParts.PortEmitter:
        component: ParticleAcceleratorEmitter
        offset: -2, -2
        rotation: -90
        graph: TestPart
        expectedNode: completed
      enum.AcceleratorParts.ForeEmitter:
        component: ParticleAcceleratorEmitter
        offset: -2, -1
        rotation: -90
        graph: TestPart
        expectedNode: completed
      enum.AcceleratorParts.StarboardEmitter:
        component: ParticleAcceleratorEmitter
        offset: -2, 0
        rotation: -90
        graph: TestPart
        expectedNode: completed
"#;

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

fn place(world: &mut World, grid: Entity, prototype: &str, x: i32, y: i32) -> Entity {
    let at = EntityCoordinates::new(grid, Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
    let entity = world.spawn_attached(prototype, at).unwrap();
    world
        .set_coordinates(entity, at, Some(Angle::degrees(-90.0)))
        .unwrap();
    entity
}

#[test]
fn assembled_accelerator_fires_three_particles_when_powered() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let parts = [
        ("EndCap", 9, 7),
        ("FuelChamber", 8, 7),
        ("PowerBox", 7, 7),
        ("PaEmitter", 6, 6),
        ("PaEmitter", 6, 7),
    ];
    for (prototype, x, y) in parts {
        place(&mut world, grid, prototype, x, y);
    }
    let control = world
        .spawn_attached(
            "ControlBox",
            EntityCoordinates::new(grid, Vec2::new(8.5, 8.5)),
        )
        .unwrap();
    // Не хватает правого излучателя.
    assert!(!world.get::<MultipartMachine>(control).unwrap().is_assembled);
    // Часть закреплена ещё до поворота, поэтому пересмотр — кнопкой пульта.
    place(&mut world, grid, "PaEmitter", 6, 8);
    rescan(&mut world, control, None);
    assert!(world.get::<MultipartMachine>(control).unwrap().is_assembled);

    switch_on(&mut world, control);
    set_strength(&mut world, control, 4);
    let power_box = part_entity(&world, control, PART_POWER_BOX).unwrap();
    // 500 + 1500 × 4.
    assert_eq!(
        world.get::<PowerConsumer>(power_box).unwrap().draw_rate,
        6500.0
    );
    assert!(
        !world
            .get::<ParticleAcceleratorControlBox>(control)
            .unwrap()
            .powered
    );
    let ui = ui_state(&world, control).unwrap();
    assert!(ui.assembled && ui.enabled && ui.emitter_starboard);

    // Силовой блок получил мощность — пульт запитан и через 6 секунд стреляет.
    world
        .get_mut::<PowerConsumer>(power_box)
        .unwrap()
        .received_power = 6500.0;
    world.raise(
        power_box,
        &mut PowerConsumerReceivedChanged {
            received_power: 6500.0,
            draw_rate: 6500.0,
        },
    );
    assert!(
        world
            .get::<ParticleAcceleratorControlBox>(control)
            .unwrap()
            .firing
    );
    run(&mut world, 6.2);
    let particles: Vec<(String, f32)> = world
        .query::<(&ParticleProjectile, &SinguloFood)>()
        .iter(&world)
        .map(|(p, f)| (p.state.clone(), f.energy))
        .collect();
    assert_eq!(particles.len(), 3);
    assert!(
        particles
            .iter()
            .all(|(state, energy)| state == "Level2" && *energy == 30.0)
    );

    // Выключение сбрасывает силу и стрельбу.
    switch_off(&mut world, control);
    let component = world.get::<ParticleAcceleratorControlBox>(control).unwrap();
    assert!(!component.firing && component.selected_strength == STANDBY);
}
