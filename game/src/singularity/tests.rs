use astrocore_ecs::{GameWorldExt, LifecycleWorldExt};
use astrocore_map::{MapGrid, SpawnWorldExt};
use astrocore_math::Vec2;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Singularity
  components:
  - type: Physics
    bodyType: Dynamic
  - type: Appearance
  - type: EventHorizon
    radius: 0.5
  - type: GravityWell
    baseRadialAcceleration: 10
    maxRange: 4
  - type: Fixtures
    fixtures:
      EventHorizonCollider:
        shape: !type:PhysShapeCircle
          radius: 0.35
        hard: true
        mask: [ SingularityLayer ]
        layer: [ SingularityLayer ]
      EventHorizonConsumer:
        shape: !type:PhysShapeCircle
          radius: 0.35
        hard: false
        mask: [ SingularityLayer ]
        layer: [ SingularityLayer ]
  - type: Singularity
    energy: 180
    level: 1
  - type: RadiationSource
    intensity: 2
- type: entity
  id: Crate
  components:
  - type: Physics
    bodyType: Dynamic
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.3,-0.3,0.3,0.3"
        layer: [ Opaque ]
        mask: [ Impassable ]
- type: entity
  id: ContainmentField
  components:
  - type: Transform
    anchored: true
  - type: Physics
    bodyType: Static
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb {}
        layer: [ GlassLayer ]
        mask: [ FullTileMask ]
  - type: ContainmentField
- type: entity
  id: SingularityGenerator
  components:
  - type: Transform
    anchored: true
  - type: Physics
    bodyType: Static
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb {}
        layer: [ WallLayer ]
  - type: SingularityGenerator
- type: entity
  id: Particle
  components:
  - type: ParticleProjectile
    state: Level3
"#;

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

#[test]
fn energy_sets_level_and_level_sets_everything_else() {
    assert_eq!(level_for_energy(180.0), 1);
    assert_eq!(level_for_energy(200.0), 2);
    assert_eq!(level_for_energy(5000.0), 6);
    assert_eq!(level_for_energy(0.0), 0);
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let singularity = spawn_at(&mut world, "Singularity", grid, 8, 8);
    set_energy(&mut world, singularity, 600.0);
    let component = world.get::<Singularity>(singularity).unwrap();
    assert_eq!(component.level, 3);
    assert_eq!(component.energy_drain, 10.0);
    let horizon = world.get::<EventHorizon>(singularity).unwrap();
    assert_eq!(horizon.radius, 2.5);
    assert!(!horizon.can_breach_containment);
    let well = world.get::<GravityWell>(singularity).unwrap();
    assert_eq!((well.max_range, well.base_radial_acceleration), (8.0, 30.0));
    assert_eq!(
        world
            .get::<crate::radiation::RadiationSource>(singularity)
            .unwrap()
            .intensity,
        6.0
    );
    set_level(&mut world, singularity, 5);
    assert!(
        world
            .get::<EventHorizon>(singularity)
            .unwrap()
            .can_breach_containment
    );

    // Без пищи энергия уходит, на нуле сингулярность исчезает.
    world
        .get_mut::<EventHorizon>(singularity)
        .unwrap()
        .consume_tiles = false;
    set_energy(&mut world, singularity, 3.0);
    run(&mut world, 3.5);
    assert!(!world.is_alive(singularity));
}

#[test]
fn horizon_eats_crates_and_floor_but_not_the_field() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let singularity = spawn_at(&mut world, "Singularity", grid, 8, 8);
    // Уровень 3: радиус 2.5.
    set_energy(&mut world, singularity, 600.0);
    let near = spawn_at(&mut world, "Crate", grid, 9, 8);
    let far = spawn_at(&mut world, "Crate", grid, 13, 8);
    let field = spawn_at(&mut world, "ContainmentField", grid, 8, 10);
    let before = world.get::<Singularity>(singularity).unwrap().energy;
    run(&mut world, 0.6);
    assert!(!world.is_alive(near));
    assert!(world.is_alive(far));
    assert!(world.is_alive(field));
    let map_grid = world.get::<MapGrid>(grid).unwrap();
    assert!(map_grid.tile(TileCoord::new(8, 8, 0)).is_empty());
    assert!(map_grid.tile(TileCoord::new(9, 9, 0)).is_empty());
    // Под полем тайл цел, дальний — тоже.
    assert!(!map_grid.tile(TileCoord::new(8, 10, 0)).is_empty());
    assert!(!map_grid.tile(TileCoord::new(13, 8, 0)).is_empty());
    assert!(world.get::<Singularity>(singularity).unwrap().energy > before);
}

#[test]
fn gravity_well_pulls_loose_things_in() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let singularity = spawn_at(&mut world, "Singularity", grid, 8, 8);
    world
        .get_mut::<EventHorizon>(singularity)
        .unwrap()
        .consume_entities = false;
    world
        .get_mut::<EventHorizon>(singularity)
        .unwrap()
        .consume_tiles = false;
    let crate_ = spawn_at(&mut world, "Crate", grid, 11, 8);
    grav_pulse(&mut world, singularity, 4.0, 0.0, 10.0, 0.0);
    let velocity = world.get::<Physics>(crate_).unwrap().linear_velocity;
    assert!(velocity.x < -3.0, "{velocity:?}");
    assert!(velocity.y.abs() < 0.01);
}

#[test]
fn generator_needs_a_closed_field_to_make_a_singularity() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let generator = spawn_at(&mut world, "SingularityGenerator", grid, 8, 8);
    let particle = world
        .spawn_attached(
            "Particle",
            astrocore_map::EntityCoordinates::new(grid, Vec2::new(8.5, 6.5)),
        )
        .unwrap();
    // Поля нет: защита, мощность не растёт.
    particle_hit_generator(&mut world, particle, generator);
    assert_eq!(
        world.get::<SingularityGenerator>(generator).unwrap().power,
        0.0
    );
    world
        .get_mut::<SingularityGenerator>(generator)
        .unwrap()
        .next_failsafe = 0.0;

    // Поле с четырёх сторон: две частицы третьего уровня дают 16 — сингулярность.
    for (x, y) in [(8, 12), (8, 4), (12, 8), (4, 8)] {
        spawn_at(&mut world, "ContainmentField", grid, x, y);
    }
    for _ in 0..2 {
        let particle = world
            .spawn_attached(
                "Particle",
                astrocore_map::EntityCoordinates::new(grid, Vec2::new(8.5, 6.5)),
            )
            .unwrap();
        particle_hit_generator(&mut world, particle, generator);
    }
    let component = world.get::<SingularityGenerator>(generator).unwrap();
    assert!(component.inert);
    assert_eq!(component.power, 0.0);
    world.run_tick();
    assert_eq!(world.query::<&Singularity>().iter(&world).count(), 1);
}
