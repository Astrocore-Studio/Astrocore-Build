use astrocore_ecs::GameWorldExt;
use astrocore_map::TileCoord;

use super::*;
use crate::atmos::gas::CELL_VOLUME;
use crate::atmos::{GasMixture, ensure_grid_atmosphere, with_tile_mixture};
use crate::atmos_alarms::tests::PROTOTYPES;
use crate::testing::{spawn_at, station, world_with};

#[test]
fn closed_firelock_holding_pressure_lights_up_and_refuses_to_open() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    ensure_grid_atmosphere(&mut world, grid);
    for _ in 0..10 {
        world.run_tick();
    }
    let firelock = spawn_at(&mut world, "TestFirelock", grid, 5, 5);
    world.run_tick();
    emergency_pressure_stop(&mut world, firelock);
    for _ in 0..30 {
        world.run_tick();
    }
    assert_eq!(
        world.get::<Door>(firelock).unwrap().state,
        DoorState::Closed
    );
    // Север — воздух, юг — вакуум.
    for _ in 0..3 {
        with_tile_mixture(&mut world, grid, TileCoord::new(5, 6, 0), |tile| {
            tile.copy_from(&GasMixture::standard_air(CELL_VOLUME))
        });
        with_tile_mixture(&mut world, grid, TileCoord::new(5, 4, 0), |tile| {
            tile.clear()
        });
        assert_eq!(check_pressure_and_fire(&world, firelock), (true, false));
    }
    for _ in 0..31 {
        with_tile_mixture(&mut world, grid, TileCoord::new(5, 6, 0), |tile| {
            tile.copy_from(&GasMixture::standard_air(CELL_VOLUME))
        });
        world.run_tick();
    }
    assert!(world.get::<Firelock>(firelock).unwrap().pressure);
    assert_eq!(
        world.appearance(firelock, PRESSURE_WARNING),
        Some(&astrocore_net::AppearanceValue::Bool(true))
    );
    let mut before = BeforeDoorOpened {
        user: None,
        cancelled: false,
    };
    world.raise(firelock, &mut before);
    assert!(before.cancelled);
}
