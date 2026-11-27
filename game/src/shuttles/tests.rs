use std::f32::consts::FRAC_PI_2;

use astrocore_ecs::GameTime;
use astrocore_map::{MapGrid, TileCoord};

use super::*;
use crate::docking::Docking;
use crate::docking::tests::{PROTOTYPES, dock_at, shuttle};
use crate::testing::{station, world_with};

fn advance(world: &mut World, seconds: f32) {
    let rate = f32::from(world.resource::<GameTime>().tick_rate);
    world.resource_mut::<GameTime>().tick.0 += (rate * seconds).ceil() as u32;
}

fn map_of(world: &World, entity: Entity) -> Option<Entity> {
    world.get::<Transform>(entity).and_then(Transform::map)
}

#[test]
fn ftl_waits_for_startup_travels_then_docks() {
    let mut world = world_with(PROTOTYPES);
    let station = station(&mut world);
    let station_dock = dock_at(&mut world, "TestDock", station, 0, 8, -FRAC_PI_2);
    let shuttle = shuttle(&mut world);
    let shuttle_dock = dock_at(&mut world, "TestDock", shuttle, 3, 1, FRAC_PI_2);
    let home = map_of(&world, shuttle);
    ftl_to_dock(&mut world, shuttle, station, Some(1.0), Some(2.0), None);
    update(&mut world);
    assert_eq!(world.get::<Ftl>(shuttle).unwrap().state, FtlState::Starting);
    assert_eq!(map_of(&world, shuttle), home);
    // Разгон кончился — шаттл на карте перехода.
    advance(&mut world, 1.0);
    update(&mut world);
    assert_eq!(
        world.get::<Ftl>(shuttle).unwrap().state,
        FtlState::Travelling
    );
    let ftl_map = map_of(&world, shuttle).unwrap();
    assert!(world.get::<FtlMap>(ftl_map).is_some());
    // Переход кончился — шаттл у станции, шлюзы состыкованы.
    advance(&mut world, 2.0);
    update(&mut world);
    assert!(world.get::<Ftl>(shuttle).is_none());
    assert_eq!(map_of(&world, shuttle), map_of(&world, station));
    assert_eq!(
        world.get::<Docking>(shuttle_dock).unwrap().docked_with,
        Some(station_dock)
    );
    let placement = world.world_transform(shuttle).unwrap();
    assert!(placement.position.distance(Vec2::new(-4.0, 7.0)) < 1e-3);
    // Второй прыжок расстыковывает сразу.
    ftl_to_dock(&mut world, shuttle, station, Some(1.0), Some(1.0), None);
    assert_eq!(
        world.get::<Docking>(shuttle_dock).unwrap().docked_with,
        None
    );
}

#[test]
fn without_docks_the_shuttle_arrives_nearby_without_overlap() {
    let mut world = world_with(PROTOTYPES);
    let station = station(&mut world);
    let shuttle = shuttle(&mut world);
    assert!(try_ftl_dock(&mut world, shuttle, station, None).is_none());
    assert_eq!(map_of(&world, shuttle), map_of(&world, station));
    // Ни один тайл шаттла не лёг на станцию.
    let placement = world.world_transform(shuttle).unwrap();
    let station_grid = world.get::<MapGrid>(station).unwrap();
    for tile in crate::docking::grid_tiles(&world, shuttle) {
        let center = placement.to_world(Vec2::new(tile.x as f32 + 0.5, tile.y as f32 + 0.5));
        assert!(
            station_grid
                .tile(TileCoord::containing(center, 0))
                .is_empty()
        );
    }
}
