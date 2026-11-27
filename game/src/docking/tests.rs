use std::f32::consts::FRAC_PI_2;

use astrocore_ecs::LifecycleWorldExt;
use astrocore_map::{MapWorldExt, Tile, TileCoord, TransformWorldExt};
use astrocore_math::{Angle, Vec2};

use super::*;
use crate::testing::{spawn_at, station, world_with};

pub(crate) const PROTOTYPES: &str = r#"
- type: entity
  id: TestDock
  components:
  - type: Docking
- type: entity
  id: TestPriorityDock
  components:
  - type: Docking
  - type: PriorityDock
    tag: DockEmergency
"#;

/// Шлюз в клетке грида, повёрнутый на `rotation` и закреплённый.
pub(crate) fn dock_at(
    world: &mut World,
    prototype: &str,
    grid: Entity,
    x: i32,
    y: i32,
    rotation: f32,
) -> Entity {
    let dock = spawn_at(world, prototype, grid, x, y);
    world.set_local_rotation(dock, Angle(rotation)).unwrap();
    assert!(world.anchor(dock));
    dock
}

/// Шаттл 4×3 тайла на отдельной карте.
pub(crate) fn shuttle(world: &mut World) -> Entity {
    let (map, _) = world.create_map(true);
    let grid = world.create_grid(map);
    world.set_tiles(
        grid,
        (0..4).flat_map(|x| (0..3).map(move |y| (TileCoord::new(x, y, 0), Tile::new(1)))),
    );
    grid
}

fn world() -> World {
    world_with(PROTOTYPES)
}

#[test]
fn shuttle_docks_flush_against_the_station_airlock() {
    let mut world = world();
    let station = station(&mut world);
    // Шлюз станции на западном краю смотрит на запад, шлюз шаттла — на восток.
    let station_dock = dock_at(&mut world, "TestDock", station, 0, 8, -FRAC_PI_2);
    let shuttle = shuttle(&mut world);
    let shuttle_dock = dock_at(&mut world, "TestDock", shuttle, 3, 1, FRAC_PI_2);
    let config = get_docking_config(&mut world, shuttle, station, None).unwrap();
    assert_eq!(config.docks, vec![(shuttle_dock, station_dock)]);
    assert_eq!(config.target_grid, station);
    assert!(config.angle.0.abs() < 1e-4);
    assert!(config.coordinates.position.distance(Vec2::new(-4.0, 7.0)) < 1e-4);
    assert_eq!(config.area, Box2::new(-4.0, 7.0, 0.0, 10.0));
}

#[test]
fn priority_dock_wins_and_more_pairs_beat_fewer() {
    let mut world = world();
    let station = station(&mut world);
    dock_at(&mut world, "TestDock", station, 0, 12, -FRAC_PI_2);
    dock_at(&mut world, "TestDock", station, 0, 13, -FRAC_PI_2);
    let priority = dock_at(&mut world, "TestPriorityDock", station, 0, 3, -FRAC_PI_2);
    let shuttle = shuttle(&mut world);
    dock_at(&mut world, "TestDock", shuttle, 3, 1, FRAC_PI_2);
    dock_at(&mut world, "TestDock", shuttle, 3, 2, FRAC_PI_2);
    // Без тега — где совпало больше шлюзов.
    let config = get_docking_config(&mut world, shuttle, station, None).unwrap();
    assert_eq!(config.docks.len(), 2);
    assert!(!is_config_priority(&world, &config, Some("DockEmergency")));
    // С тегом — приоритетный шлюз, даже с одной парой.
    let config = get_docking_config(&mut world, shuttle, station, Some("DockEmergency")).unwrap();
    assert!(
        config
            .docks
            .iter()
            .any(|(_, grid_dock)| *grid_dock == priority)
    );
    assert!(is_config_priority(&world, &config, Some("DockEmergency")));
}

#[test]
fn shuttle_never_lands_on_station_tiles_or_other_grids() {
    let mut world = world();
    let station = station(&mut world);
    // Шлюз посреди станции: шаттл лёг бы на её тайлы.
    dock_at(&mut world, "TestDock", station, 8, 8, -FRAC_PI_2);
    let shuttle = shuttle(&mut world);
    dock_at(&mut world, "TestDock", shuttle, 3, 1, FRAC_PI_2);
    assert!(get_docking_config(&mut world, shuttle, station, None).is_none());

    // Шлюз на краю, но место занято другим гридом.
    let mut world = self::world();
    let station = crate::testing::station(&mut world);
    dock_at(&mut world, "TestDock", station, 0, 8, -FRAC_PI_2);
    let map = world
        .get::<Transform>(station)
        .and_then(Transform::map)
        .unwrap();
    let blocker = world.create_grid(map);
    world.set_tile(blocker, TileCoord::new(-3, 8, 0), Tile::new(1));
    let shuttle = self::shuttle(&mut world);
    dock_at(&mut world, "TestDock", shuttle, 3, 1, FRAC_PI_2);
    assert!(get_docking_config(&mut world, shuttle, station, None).is_none());
}

#[test]
fn docking_links_airlocks_until_undocked() {
    let mut world = world();
    let station = station(&mut world);
    let station_dock = dock_at(&mut world, "TestDock", station, 0, 8, -FRAC_PI_2);
    let shuttle = shuttle(&mut world);
    let shuttle_dock = dock_at(&mut world, "TestDock", shuttle, 3, 1, FRAC_PI_2);
    dock(&mut world, shuttle_dock, station_dock);
    assert_eq!(
        world.get::<Docking>(station_dock).unwrap().docked_with,
        Some(shuttle_dock)
    );
    // Занятые шлюзы для новой стыковки не годятся.
    assert!(get_docking_config(&mut world, shuttle, station, None).is_none());
    undock_docks(&mut world, shuttle);
    assert_eq!(
        world.get::<Docking>(station_dock).unwrap().docked_with,
        None
    );
    assert_eq!(
        world.get::<Docking>(shuttle_dock).unwrap().docked_with,
        None
    );
    // Удалённый шлюз расстыковывает пару.
    dock(&mut world, shuttle_dock, station_dock);
    world.delete_entity(shuttle_dock);
    assert_eq!(
        world.get::<Docking>(station_dock).unwrap().docked_with,
        None
    );
}

#[test]
fn angle_points_from_station_to_shuttle() {
    let mut world = world();
    let station = station(&mut world);
    let map = world
        .get::<Transform>(station)
        .and_then(Transform::map)
        .unwrap();
    let shuttle = shuttle(&mut world);
    world
        .set_coordinates(
            shuttle,
            EntityCoordinates::new(map, Vec2::new(-20.0, 7.0)),
            None,
        )
        .unwrap();
    let angle = get_angle(&mut world, shuttle, station).unwrap();
    assert_eq!(angle.direction(), astrocore_math::Direction::West);
}
