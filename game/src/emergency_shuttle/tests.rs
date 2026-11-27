use std::f32::consts::FRAC_PI_2;

use astrocore_ecs::GameTime;
use astrocore_map::{Tile, TileCoord};

use super::*;
use crate::docking::Docking;
use crate::docking::tests::{PROTOTYPES, dock_at, shuttle};
use crate::testing::{station, world_with};

struct Scene {
    world: World,
    station: Entity,
    station_grid: Entity,
    shuttle: Entity,
    shuttle_dock: Entity,
    centcomm: Entity,
}

fn advance(world: &mut World, seconds: f32) {
    let rate = f32::from(world.resource::<GameTime>().tick_rate);
    world.resource_mut::<GameTime>().tick.0 += (rate * seconds).ceil() as u32;
}

fn map_of(world: &World, entity: Entity) -> Option<Entity> {
    world.get::<Transform>(entity).and_then(Transform::map)
}

/// Станция со шлюзом `dock` на западном краю, шаттл с одним шлюзом, ЦК со шлюзом.
fn scene(dock: Option<&str>) -> Scene {
    let mut world = world_with(PROTOTYPES);
    world.resource_mut::<game_ticker::GameTicker>().run_level = GameRunLevel::InRound;
    let station_grid = station(&mut world);
    if let Some(dock) = dock {
        dock_at(&mut world, dock, station_grid, 0, 8, -FRAC_PI_2);
    }
    let shuttle = shuttle(&mut world);
    let shuttle_dock = dock_at(&mut world, "TestDock", shuttle, 3, 1, FRAC_PI_2);
    let (centcomm_map, _) = world.create_map(true);
    let centcomm = world.create_grid(centcomm_map);
    world.set_tiles(
        centcomm,
        (0..8).flat_map(|x| (0..8).map(move |y| (TileCoord::new(x, y, 0), Tile::new(1)))),
    );
    dock_at(&mut world, "TestDock", centcomm, 0, 4, -FRAC_PI_2);
    let station = world.spawn_entity();
    world.entity_mut(station).insert((
        stations::StationData::default(),
        StationEmergencyShuttle {
            emergency_shuttle: Some(shuttle),
            ..StationEmergencyShuttle::default()
        },
        StationCentcomm {
            entity: Some(centcomm),
            ..StationCentcomm::default()
        },
    ));
    stations::add_grid_to_station(&mut world, station, station_grid, None);
    world.entity_mut(shuttle).insert(EmergencyShuttle);
    add_centcomm(&mut world, station);
    assert_eq!(
        world.get::<StationCentcomm>(station).unwrap().map_entity,
        Some(centcomm_map)
    );
    state(&mut world).transit_time = 20.0;
    Scene {
        world,
        station,
        station_grid,
        shuttle,
        shuttle_dock,
        centcomm,
    }
}

#[test]
fn countdown_end_docks_the_shuttle_then_it_leaves_and_the_round_ends() {
    let Scene {
        mut world,
        station_grid,
        shuttle,
        shuttle_dock,
        centcomm,
        ..
    } = scene(Some("TestPriorityDock"));
    let mut event = round_end::ShuttleCountdownFinished::default();
    world.raise_broadcast(&mut event);
    assert!(event.handled);
    assert!(emergency_shuttle_arrived(&world));
    assert!(world.resource::<round_end::RoundEnd>().shuttle_arrived);
    assert_eq!(map_of(&world, shuttle), map_of(&world, station_grid));
    assert!(
        world
            .get::<Docking>(shuttle_dock)
            .unwrap()
            .docked_with
            .is_some()
    );
    // У приоритетного шлюза стоянка обычная.
    assert_eq!(state(&mut world).console_accumulator, 180.0);
    // За 10 с до отлёта — объявление о запуске.
    update_emergency_console(&mut world, 171.0);
    assert!(state(&mut world).announced);
    assert!(!state(&mut world).launched);
    // За время разгона — прыжок к ЦК.
    update_emergency_console(&mut world, 4.0);
    assert!(state(&mut world).launched);
    assert!(world.get::<shuttles::Ftl>(shuttle).is_some());
    advance(&mut world, 5.0);
    shuttles::update(&mut world);
    update_emergency_console(&mut world, 5.0);
    assert!(state(&mut world).left);
    assert!(state(&mut world).round_end_at.is_some());
    // Переход — 20 с, затем стыковка с ЦК.
    advance(&mut world, 20.0);
    shuttles::update(&mut world);
    assert_eq!(map_of(&world, shuttle), map_of(&world, centcomm));
    assert!(
        world
            .get::<Docking>(shuttle_dock)
            .unwrap()
            .docked_with
            .is_some()
    );
    // Ещё 5 с — конец раунда.
    update(&mut world);
    assert_eq!(game_ticker::run_level(&world), GameRunLevel::InRound);
    advance(&mut world, 5.0);
    update(&mut world);
    assert_eq!(game_ticker::run_level(&world), GameRunLevel::PostRound);
    assert!(
        world
            .resource::<game_ticker::GameTicker>()
            .restart_at
            .is_some()
    );
}

#[test]
fn worse_docking_lengthens_the_wait() {
    let mut other = scene(Some("TestDock")).world;
    dock_emergency_shuttle(&mut other);
    let expected = 180.0 * other.cvar(&DOCK_TIME_MULTIPLIER_OTHER_DOCK);
    assert_eq!(state(&mut other).console_accumulator, expected);

    let Scene {
        mut world,
        shuttle,
        station_grid,
        ..
    } = scene(None);
    dock_emergency_shuttle(&mut world);
    assert_eq!(state(&mut world).console_accumulator, 360.0);
    // Шлюзов нет — шаттл рядом со станцией.
    assert_eq!(map_of(&world, shuttle), map_of(&world, station_grid));
}

#[test]
fn station_without_grids_gets_good_luck() {
    let Scene {
        mut world,
        station,
        shuttle,
        station_grid,
        ..
    } = scene(Some("TestDock"));
    world
        .get_mut::<stations::StationData>(station)
        .unwrap()
        .grids
        .clear();
    let result = dock_single_emergency_shuttle(&mut world, station).unwrap();
    assert_eq!(result.result_type, ShuttleDockResultType::GoodLuck);
    assert_ne!(map_of(&world, shuttle), map_of(&world, station_grid));
}

#[test]
fn early_launch_only_after_arrival_and_once() {
    let mut world = scene(Some("TestPriorityDock")).world;
    assert!(!early_launch(&mut world));
    dock_emergency_shuttle(&mut world);
    assert!(early_launch(&mut world));
    assert_eq!(state(&mut world).console_accumulator, 10.0);
    assert!(state(&mut world).announced);
    assert!(!early_launch(&mut world));
}

#[test]
fn round_end_after_departure_can_be_delayed() {
    let mut world = scene(Some("TestPriorityDock")).world;
    assert!(!delay_emergency_round_end(&mut world));
    dock_emergency_shuttle(&mut world);
    update_emergency_console(&mut world, 181.0);
    assert!(state(&mut world).left);
    assert!(delay_emergency_round_end(&mut world));
    assert!(!delay_emergency_round_end(&mut world));
    advance(&mut world, 60.0);
    update(&mut world);
    assert_eq!(game_ticker::run_level(&world), GameRunLevel::InRound);
}

#[test]
fn disabled_shuttle_leaves_the_round_end_to_round_end() {
    let mut world = scene(Some("TestPriorityDock")).world;
    world.set_cvar(&EMERGENCY_SHUTTLE_ENABLED, false).unwrap();
    let mut event = round_end::ShuttleCountdownFinished::default();
    world.raise_broadcast(&mut event);
    assert!(!event.handled);
    assert!(!emergency_shuttle_arrived(&world));
}

#[test]
fn transit_time_is_rolled_in_tens_each_round() {
    let mut world = world_with("");
    for _ in 0..20 {
        cleanup_emergency_console(&mut world);
        let transit = state(&mut world).transit_time;
        assert!((60.0..=180.0).contains(&transit));
        assert_eq!(transit % 10.0, 0.0);
        assert_eq!(state(&mut world).console_accumulator, f32::MIN);
    }
}
