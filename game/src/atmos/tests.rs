use astrocore_ecs::{GameWorldExt, LifecycleWorldExt};

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: TestWall
  components:
  - type: Transform
    anchored: true
  - type: Airtight
- type: entity
  id: TestDoor
  components:
  - type: Transform
    anchored: true
  - type: Door
  - type: Airtight
    noAirWhenFullyAirBlocked: false
"#;

struct Room {
    world: World,
    grid: Entity,
    walls: HashMap<(i32, i32), Entity>,
}

/// Комната 8×8 (клетки 1..=8) в стенах на клетках 0 и 9.
fn room() -> Room {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let mut walls = HashMap::new();
    for i in 0..10 {
        for (x, y) in [(i, 0), (i, 9), (0, i), (9, i)] {
            walls
                .entry((x, y))
                .or_insert_with(|| spawn_at(&mut world, "TestWall", grid, x, y));
        }
    }
    ensure_grid_atmosphere(&mut world, grid);
    let mut room = Room { world, grid, walls };
    room.run(0.2);
    for x in 1..=8 {
        for y in 1..=8 {
            room.fill(x, y);
        }
    }
    room
}

impl Room {
    fn run(&mut self, seconds: f32) {
        for _ in 0..(seconds * 30.0).ceil() as u32 {
            self.world.run_tick();
        }
    }

    fn fill(&mut self, x: i32, y: i32) {
        with_tile_mixture(&mut self.world, self.grid, TileCoord::new(x, y, 0), |air| {
            air.copy_from(&GasMixture::standard_air(gas::CELL_VOLUME));
        })
        .expect("клетка с воздухом");
    }

    fn pressure(&self, x: i32, y: i32) -> f32 {
        tile_mixture(&self.world, self.grid, TileCoord::new(x, y, 0))
            .map_or(0.0, GasMixture::pressure)
    }

    fn moles(&self, xs: std::ops::RangeInclusive<i32>) -> f32 {
        let mut total = 0.0;
        for x in xs {
            for y in 1..=8 {
                total += tile_mixture(&self.world, self.grid, TileCoord::new(x, y, 0))
                    .map_or(0.0, GasMixture::total_moles);
            }
        }
        total
    }

    fn remove_wall(&mut self, x: i32, y: i32) {
        let wall = self.walls.remove(&(x, y)).unwrap();
        self.world.delete_entity(wall);
    }
}

#[test]
fn sealed_room_keeps_its_air() {
    let mut room = room();
    let before = room.moles(1..=8);
    room.run(10.0);
    let after = room.moles(1..=8);
    assert!(
        (before - after).abs() / before < 0.001,
        "{before} → {after}"
    );
    for (x, y) in [(1, 1), (4, 5), (8, 8)] {
        assert!((room.pressure(x, y) - gas::ONE_ATMOSPHERE).abs() < 0.5);
    }
    // Стены без воздуха (`noAirWhenFullyAirBlocked`).
    assert!(tile_mixture(&room.world, room.grid, TileCoord::new(0, 4, 0)).is_none());
}

#[test]
fn breach_depressurizes_the_room() {
    let mut room = room();
    room.run(1.0);
    room.remove_wall(0, 5);
    // Газ выходит постепенно (15 % за цикл, `atmos.mmos_spacing_speed`): за 4 секунды
    // давление падает, за 20 — комната пуста.
    room.run(4.0);
    assert!(room.pressure(4, 4) < 90.0, "{}", room.pressure(4, 4));
    room.run(16.0);
    for (x, y) in [(1, 5), (4, 4), (8, 8)] {
        assert!(
            room.pressure(x, y) < 1.0,
            "клетка {x},{y}: {}",
            room.pressure(x, y)
        );
    }
}

#[test]
fn closed_door_holds_and_open_door_vents() {
    let mut room = room();
    room.remove_wall(0, 5);
    let door = spawn_at(&mut room.world, "TestDoor", room.grid, 0, 5);
    room.run(4.0);
    assert!((room.pressure(4, 4) - gas::ONE_ATMOSPHERE).abs() < 1.0);
    crate::doors::start_opening(&mut room.world, door);
    room.run(20.0);
    assert!(room.pressure(4, 4) < 1.0, "{}", room.pressure(4, 4));
}

#[test]
fn rooms_equalize_through_a_gap() {
    let mut room = room();
    // Перегородка по x = 5: справа вакуум.
    let mut partition = Vec::new();
    for y in 1..=8 {
        partition.push(spawn_at(&mut room.world, "TestWall", room.grid, 5, y));
    }
    room.run(1.0);
    for x in 6..=8 {
        for y in 1..=8 {
            with_tile_mixture(&mut room.world, room.grid, TileCoord::new(x, y, 0), |air| {
                air.clear()
            });
        }
    }
    let total = room.moles(1..=8);
    room.world.delete_entity(partition[3]);
    room.run(10.0);
    let left = room.pressure(2, 4);
    let right = room.pressure(7, 4);
    assert!((left - right).abs() < 2.0, "{left} / {right}");
    let after = room.moles(1..=8);
    assert!((total - after).abs() / total < 0.01, "{total} → {after}");
}

#[test]
fn map_record_round_trip() {
    let mut room = room();
    room.run(1.0);
    let atmos = room.world.get::<GridAtmosphere>(room.grid).unwrap();
    let node = write_tiles(atmos);
    let tiles = read_tiles(&node);
    let with_air = atmos.live_tiles().filter(|(_, t)| t.air.is_some()).count();
    assert_eq!(tiles.len(), with_air);
    let (coord, air) = tiles
        .iter()
        .find(|(c, _)| *c == TileCoord::new(3, 3, 0))
        .unwrap();
    assert_eq!(Some(air), tile_mixture(&room.world, room.grid, *coord));
    assert_eq!(
        write_tiles(&{
            let mut copy = GridAtmosphere::default();
            for (coord, air) in tiles {
                copy.insert_tile(coord, air);
            }
            copy
        }),
        node
    );
}

#[test]
fn walls_turn_with_the_entity() {
    assert_eq!(rotate_directions(NORTH, std::f32::consts::FRAC_PI_2), WEST);
    assert_eq!(rotate_directions(SOUTH, std::f32::consts::FRAC_PI_2), EAST);
    assert_eq!(rotate_directions(ALL_DIRECTIONS, 1.0), ALL_DIRECTIONS);
}
