use astrocore_console::{ConfigWorldExt, ConsoleShell, ConsoleWorldExt};
use astrocore_ecs::MetaData;
use astrocore_map::{Map, MapId, MapWorldExt, TransformWorldExt};
use astrocore_math::Vec2;
use astrocore_net::{ConnId, ServerNet, Session, SessionWorldExt};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Body
  components:
  - type: Physics
    bodyType: KinematicController
- type: entity
  id: AdminObserver
  components:
  - type: Physics
    bodyType: Kinematic
    ignorePaused: true
- type: entity
  id: Crate
  components:
  - type: Transform
    anchored: true
"#;

/// Выполняет команду консоли сервера; возвращает вывод и ошибки.
fn run(world: &mut World, line: &str) -> (Vec<String>, Vec<String>) {
    let mut shell = ConsoleShell::server();
    world.execute_command(&mut shell, line).unwrap();
    (shell.output, shell.errors)
}

/// Игрок с адреса `address`, управляющий телом `body`.
fn player(world: &mut World, address: &str, body: Entity) -> Entity {
    let conn = ConnId(1);
    world
        .resource_mut::<ServerNet>()
        .addresses
        .insert(conn, address.parse().unwrap());
    let session = world
        .spawn(Session {
            conn,
            name: "Mapper".into(),
            attached: None,
            last_input: 0,
            sync: Default::default(),
        })
        .id();
    world.attach_player(session, body);
    session
}

fn user_data(world: &mut World, name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("astrocore-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    world
        .set_cvar_text("game.user_data", &dir.display().to_string())
        .unwrap();
    dir
}

#[test]
fn maps_are_created_listed_paused_initialized_and_removed() {
    let mut world = world_with(PROTOTYPES);
    let (_, errors) = run(&mut world, "addmap 5 true");
    assert!(errors.is_empty(), "{errors:?}");
    let map = world.map_entity(MapId(5)).unwrap();
    assert!(!world.get::<Map>(map).unwrap().initialized);
    let (out, _) = run(&mut world, "lsmap");
    assert!(
        out.iter()
            .any(|line| line.starts_with("5: map 5, init: false, paused: true")),
        "{out:?}"
    );

    run(&mut world, "pausemap 5");
    assert_eq!(run(&mut world, "querymappaused 5").0, ["true"]);
    run(&mut world, "unpausemap 5");
    assert_eq!(run(&mut world, "querymappaused 5").0, ["false"]);
    run(&mut world, "mapinit 5");
    assert!(world.get::<Map>(map).unwrap().initialized);
    assert_eq!(
        run(&mut world, "mapinit 5").1,
        ["карта уже инициализирована"]
    );

    run(&mut world, "rmmap 5");
    assert!(world.map_entity(MapId(5)).is_none());
    assert_eq!(run(&mut world, "rmmap 5").1, ["карты 5 нет"]);
}

#[test]
fn saved_map_loads_back_under_another_id() {
    let mut world = world_with(PROTOTYPES);
    let dir = user_data(&mut world, "savemap");
    run(&mut world, "addmap 3 true");
    let map = world.map_entity(MapId(3)).unwrap();
    let grid = world.create_grid(map);
    world.set_tiles(
        grid,
        (0..4).map(|x| {
            (
                astrocore_map::TileCoord::new(x, 0, 0),
                astrocore_map::Tile::new(1),
            )
        }),
    );
    spawn_at(&mut world, "Crate", grid, 2, 0);

    let (out, errors) = run(&mut world, "savemap 3 Maps/test.yml");
    assert!(errors.is_empty(), "{errors:?}");
    assert!(out[0].contains("сущностей 3"), "{out:?}");
    assert!(dir.join("Maps/test.yml").is_file());
    // Инициализированную карту без force не сохранить.
    run(&mut world, "addmap 4");
    assert!(!run(&mut world, "savemap 4 Maps/played.yml").1.is_empty());
    assert!(
        run(&mut world, "savemap 4 Maps/played.yml true")
            .1
            .is_empty()
    );
    // Путь за пределы каталога данных отклоняется.
    assert!(!run(&mut world, "savemap 3 ../escape.yml").1.is_empty());

    let (out, errors) = run(&mut world, "loadmap 9 Maps/test.yml");
    assert!(errors.is_empty(), "{errors:?}");
    assert!(out[0].contains("сущностей 3"), "{out:?}");
    let loaded = world.map_entity(MapId(9)).unwrap();
    let state = world.get::<Map>(loaded).unwrap();
    assert!(!state.initialized && state.paused);
    assert_eq!(state.grids().len(), 1);
    assert_eq!(
        run(&mut world, "loadmap 9 Maps/test.yml").1,
        ["Maps/test.yml: карта 9 уже существует"]
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_players_on_the_server_machine_may_run_commands() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let body = spawn_at(&mut world, "Body", grid, 1, 1);
    let session = player(&mut world, "10.0.0.7:5000", body);
    let mut shell = ConsoleShell::player(session);
    assert!(world.execute_command(&mut shell, "lsmap").is_err());

    world
        .resource_mut::<ServerNet>()
        .addresses
        .insert(ConnId(1), "127.0.0.1:5000".parse().unwrap());
    let mut shell = ConsoleShell::player(session);
    assert!(world.execute_command(&mut shell, "lsmap").is_ok());
}

#[test]
fn mapping_makes_the_player_an_admin_ghost_on_a_paused_map() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let body = spawn_at(&mut world, "Body", grid, 1, 1);
    let session = player(&mut world, "127.0.0.1:5000", body);
    let mut shell = ConsoleShell::player(session);
    world.execute_command(&mut shell, "mapping").unwrap();
    assert!(shell.errors.is_empty(), "{:?}", shell.errors);

    let ghost = world.get::<Session>(session).unwrap().attached.unwrap();
    assert_ne!(ghost, body);
    assert_eq!(
        world.get::<MetaData>(ghost).unwrap().prototype.as_deref(),
        Some(ADMIN_OBSERVER)
    );
    assert_eq!(
        world.get::<MetaData>(ghost).unwrap().name.as_deref(),
        Some("Mapper")
    );
    let coordinates = world.map_coordinates(ghost).unwrap();
    let map = world.map_entity(coordinates.map).unwrap();
    let state = world.get::<Map>(map).unwrap();
    assert!(state.paused && !state.initialized);
    assert!(
        world
            .get::<astrocore_physics::Physics>(ghost)
            .unwrap()
            .ignore_paused
    );

    // Телепорт по карте и возвращение в тело.
    let mut shell = ConsoleShell::player(session);
    world.execute_command(&mut shell, "tp 3 4").unwrap();
    assert_eq!(
        world.map_coordinates(ghost).unwrap().position,
        Vec2::new(3.0, 4.0)
    );
    let mut shell = ConsoleShell::player(session);
    world.execute_command(&mut shell, "aghost").unwrap();
    assert_eq!(world.get::<Session>(session).unwrap().attached, Some(body));
}

#[test]
fn tp_lands_on_the_grid_under_the_point() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let body = spawn_at(&mut world, "Body", grid, 1, 1);
    let session = player(&mut world, "127.0.0.1:5000", body);
    let map = world.map_coordinates(body).unwrap().map;
    let mut shell = ConsoleShell::player(session);
    world
        .execute_command(&mut shell, &format!("tp 5.5 6.5 {}", map.0))
        .unwrap();
    assert!(shell.errors.is_empty(), "{:?}", shell.errors);
    assert_eq!(world.parent(body), Some(grid));
    assert_eq!(
        world.map_coordinates(body).unwrap().position,
        Vec2::new(5.5, 6.5)
    );
}

#[test]
fn addgas_adds_moles_to_a_tile() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    crate::atmos::ensure_grid_atmosphere(&mut world, grid);
    for _ in 0..10 {
        astrocore_ecs::GameWorldExt::run_tick(&mut world);
    }
    let id = net_id(&world, grid);
    let (out, errors) = run(&mut world, &format!("addgas 3 -4 {id} Plasma 30"));
    assert!(errors.is_empty(), "{errors:?}");
    assert!(out.iter().any(|l| l.contains("Нет клетки")), "{out:?}");
    let (out, errors) = run(&mut world, &format!("addgas 3 4 {id} Plasma 30"));
    assert!(errors.is_empty() && out.is_empty(), "{out:?} {errors:?}");
    let air =
        crate::atmos::tile_mixture(&world, grid, astrocore_map::TileCoord::new(3, 4, 0)).unwrap();
    assert_eq!(air.moles[crate::atmos::gas::PLASMA], 30.0);
}
