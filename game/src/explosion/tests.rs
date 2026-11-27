use astrocore_ecs::{GameWorldExt, LifecycleWorldExt};
use astrocore_map::{EntityCoordinates, TileDefinitions};

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: explosion
  id: Default
  damagePerIntensity:
    types:
      Heat: 5
      Blunt: 5
      Piercing: 5
  tileBreakChance: [ 0, 0.5, 1 ]
  tileBreakIntensity: [ 0, 50, 75 ]
  tileBreakRerollReduction: 25
- type: tile
  id: Lattice
  baseTurf: Space
- type: tile
  id: Plating
  baseTurf: Lattice
- type: entity
  id: Victim
  components:
  - type: Physics
    bodyType: KinematicController
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeCircle
          radius: 0.35
        layer: [ MobLayer ]
  - type: Damageable
  - type: Injurable
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
- type: entity
  id: IronWall
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
  - type: Airtight
- type: entity
  id: WeakWall
  parent: IronWall
  components:
  - type: Damageable
  - type: Injurable
  - type: Destructible
    thresholds:
    - trigger: !type:DamageTrigger
        damage: 60
      behaviors:
      - !type:DoActsBehavior
        acts: [ "Destruction" ]
- type: entity
  id: Bomb
  components:
  - type: Damageable
  - type: Injurable
  - type: Explosive
    explosionType: Default
    totalIntensity: 40
    intensitySlope: 5
    maxIntensity: 10
  - type: Destructible
    thresholds:
    - trigger: !type:DamageTrigger
        damage: 10
      behaviors:
      - !type:ExplodeBehavior {}
"#;

fn setup() -> (World, Entity) {
    let mut world = world_with(PROTOTYPES);
    for tile in ["Lattice", "Plating"] {
        world
            .resource_mut::<TileDefinitions>()
            .register(tile, None, 1);
    }
    let grid = station(&mut world);
    (world, grid)
}

fn run(world: &mut World, ticks: u32) {
    for _ in 0..ticks {
        world.run_tick();
    }
}

fn at(world: &World, grid: Entity, x: f32, y: f32) -> MapCoordinates {
    world
        .to_map_coordinates(EntityCoordinates::new(grid, Vec2::new(x, y)))
        .unwrap()
}

type Tiles = BTreeMap<usize, Vec<(i32, i32)>>;

fn tiles_of(world: &World, grid: Entity, total: f32, slope: f32, max: f32) -> (Vec<f32>, Tiles) {
    let prototype = explosion_prototype(world, "Default").unwrap();
    let (_, _, _, intensity, tiles) = explosion_tiles(
        world,
        at(world, grid, 8.5, 8.5),
        "Default",
        &prototype,
        total,
        slope,
        max,
    )
    .unwrap();
    (intensity, tiles)
}

#[test]
fn radius_and_intensity_are_inverse() {
    for (radius, slope, max) in [(2.0, 1.0, 4.0), (5.0, 2.0, 100.0), (8.0, 1.0, 3.0)] {
        let total = radius_to_intensity(radius, slope, max);
        assert!(
            (intensity_to_radius(total, slope, max) - radius).abs() < 0.01,
            "{radius}"
        );
    }
}

#[test]
fn flood_spends_exactly_the_total_intensity() {
    let (world, grid) = setup();
    let (intensity, tiles) = tiles_of(&world, grid, 60.0, 4.0, 10.0);
    let spent: f32 = tiles
        .iter()
        .map(|(i, list)| intensity[*i] * list.len() as f32)
        .sum();
    assert!((spent - 60.0).abs() < 0.01, "{spent}");
    // Центр сильнее краёв, не выше предела.
    assert!(intensity[0] <= 10.0 && intensity[0] >= *intensity.last().unwrap());
    // Симметрия по сторонам.
    let all: HashSet<(i32, i32)> = tiles.values().flatten().copied().collect();
    for (x, y) in all.iter().copied() {
        assert!(all.contains(&(16 - x, y)) && all.contains(&(x, 16 - y)));
    }
}

#[test]
fn indestructible_walls_hold_the_blast() {
    let (mut world, grid) = setup();
    // Комната 3×3 вокруг (8, 8).
    for x in 6..=10 {
        for y in 6..=10 {
            if x == 6 || x == 10 || y == 6 || y == 10 {
                spawn_at(&mut world, "IronWall", grid, x, y);
            }
        }
    }
    let (_, tiles) = tiles_of(&world, grid, 500.0, 2.0, 50.0);
    let all: HashSet<(i32, i32)> = tiles.values().flatten().copied().collect();
    assert!(
        all.iter()
            .all(|(x, y)| (6..=10).contains(x) && (6..=10).contains(y))
    );
    assert!(all.contains(&(6, 8)));
}

#[test]
fn weak_wall_lets_a_strong_blast_through_later() {
    let (mut world, grid) = setup();
    for y in 0..16 {
        spawn_at(&mut world, "WeakWall", grid, 10, y);
    }
    let (_, tiles) = tiles_of(&world, grid, 800.0, 2.0, 20.0);
    let iteration_of = |tile: (i32, i32)| {
        tiles
            .iter()
            .find(|(_, l)| l.contains(&tile))
            .map(|(i, _)| *i)
    };
    // За стеной взрыв есть, но позже, чем на том же расстоянии с этой стороны.
    let behind = iteration_of((11, 8)).expect("за стеной");
    let front = iteration_of((5, 8)).expect("перед стеной");
    assert!(behind > front, "{behind} {front}");
}

#[test]
fn strong_blast_hurts_destroys_and_breaks_the_floor() {
    let (mut world, grid) = setup();
    let victim = spawn_at(&mut world, "Victim", grid, 9, 8);
    let wall = spawn_at(&mut world, "WeakWall", grid, 8, 10);
    let epicenter = at(&world, grid, 8.5, 8.5);
    queue_explosion(
        &mut world,
        epicenter,
        "Default",
        5000.0,
        10.0,
        100.0,
        None,
        1.0,
        i32::MAX,
        true,
    );
    run(&mut world, 20);
    assert!(world.total_damage(victim).float() > 1000.0);
    assert!(!world.is_alive(wall));
    // В центре сила около 85: обшивка снята наверняка, решётка — с вероятностью 0.7.
    let plating = world
        .resource::<TileDefinitions>()
        .by_id("Plating")
        .unwrap()
        .type_id;
    let center = world
        .get::<MapGrid>(grid)
        .unwrap()
        .tile(TileCoord::new(8, 8, 0));
    assert_ne!(center.type_id, plating);
    assert!(world.resource::<Explosions>().active.is_none());
}

#[test]
fn small_blast_throws_loose_things_away() {
    let (mut world, grid) = setup();
    let crate_ = spawn_at(&mut world, "Crate", grid, 10, 8);
    let start = world.map_coordinates(crate_).unwrap().position;
    let epicenter = at(&world, grid, 8.5, 8.5);
    queue_explosion(
        &mut world,
        epicenter,
        "Default",
        60.0,
        4.0,
        10.0,
        None,
        1.0,
        i32::MAX,
        true,
    );
    run(&mut world, 10);
    let end = world.map_coordinates(crate_).unwrap().position;
    assert!(end.x > start.x + 0.5, "{start:?} → {end:?}");
    // Пол цел: сила мала.
    assert!(
        !world
            .get::<MapGrid>(grid)
            .unwrap()
            .tile(TileCoord::new(8, 8, 0))
            .is_empty()
    );
}

#[test]
fn damaged_bomb_explodes() {
    let (mut world, grid) = setup();
    let bomb = spawn_at(&mut world, "Bomb", grid, 8, 8);
    let victim = spawn_at(&mut world, "Victim", grid, 9, 8);
    let damage = DamageSpecifier::new([("Blunt", FixedPoint2::from_f32(15.0))]);
    world.change_damage(bomb, &damage, false, false, None);
    run(&mut world, 3);
    assert!(!world.is_alive(bomb));
    assert!(world.total_damage(victim).float() > 15.0);
}
