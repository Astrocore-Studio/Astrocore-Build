use astrocore_console::{ConsoleShell, ConsoleWorldExt};
use astrocore_ecs::GameWorldExt;

use super::*;
use crate::testing::world_with;

fn user_data(world: &mut World) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("astrocore-autosave-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    world
        .set_cvar_text("game.user_data", &dir.display().to_string())
        .unwrap();
    dir
}

#[test]
fn file_names_carry_the_date() {
    assert_eq!(autosave_file_name(0), "1970-1-01_00.00.00-AUTO.yml");
    // 2026-11-27 13:05:09 UTC.
    assert_eq!(
        autosave_file_name(1_795_784_709_000),
        "2026-11-27_13.05.09-AUTO.yml"
    );
}

#[test]
fn mapping_maps_are_saved_on_schedule_until_initialized() {
    let mut world = world_with("");
    let dir = user_data(&mut world);
    world.set_cvar(&AUTOSAVE_INTERVAL, 0.0).unwrap();
    world.create_map_with_id(MapId(7), false).unwrap();
    let map = world.map_entity(MapId(7)).unwrap();
    let mut shell = ConsoleShell::server();
    world
        .execute_command(&mut shell, "toggleautosave 7 Maps/station.yml")
        .unwrap();
    assert_eq!(
        world.resource::<MappingAutosaves>().maps[&map].1,
        "station.yml"
    );
    std::thread::sleep(std::time::Duration::from_millis(2));
    world.run_tick();
    let saved: Vec<_> = std::fs::read_dir(dir.join("Autosaves/station.yml"))
        .unwrap()
        .collect();
    assert!(!saved.is_empty());
    // Повторный вызов выключает.
    world
        .execute_command(&mut shell, "toggleautosave 7")
        .unwrap();
    assert!(world.resource::<MappingAutosaves>().maps.is_empty());
    // Инициализированная карта выбывает из автосохранения.
    toggle_autosave(&mut world, MapId(7), Some("again.yml"));
    world.initialize_map(map);
    std::thread::sleep(std::time::Duration::from_millis(2));
    world.run_tick();
    assert!(world.resource::<MappingAutosaves>().maps.is_empty());
    // Выключенная настройка забывает карты и не даёт включить.
    world.create_map_with_id(MapId(8), false).unwrap();
    toggle_autosave(&mut world, MapId(8), Some("x.yml"));
    world.set_cvar(&AUTOSAVE_ENABLED, false).unwrap();
    assert!(world.resource::<MappingAutosaves>().maps.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}
