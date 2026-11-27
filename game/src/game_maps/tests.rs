use astrocore_console::{ConfigWorldExt, ConsoleShell, ConsoleWorldExt};
use astrocore_net::{ConnId, NetUserId};

use super::*;
use crate::testing::world_with;

const PROTOTYPES: &str = r#"
- type: gameMap
  id: Small
  mapName: Small Station
  mapPath: /Maps/small.yml
  minPlayers: 0
  maxPlayers: 2
- type: gameMap
  id: Big
  mapName: Big Station
  mapPath: /Maps/big.yml
  minPlayers: 3
  maxPlayers: 80
- type: gameMap
  id: Other
  mapName: Other Station
  mapPath: /Maps/other.yml
  minPlayers: 0
  maxPlayers: 80
- type: gameMap
  id: Spare
  mapName: Spare Station
  mapPath: /Maps/spare.yml
  minPlayers: 50
  maxPlayers: 60
  fallback: true
- type: gameMapPool
  id: DefaultMapPool
  maps: [ Small, Big, Other ]
- type: gameMapPool
  id: TinyPool
  maps: [ Small ]
- type: gamePreset
  id: Tiny
  name: tiny
  supportedMaps: TinyPool
"#;

fn world() -> World {
    let mut world = world_with(PROTOTYPES);
    world.set_cvar_text("game.map", "").unwrap();
    world
}

fn players(world: &mut World, count: u8) {
    for n in 0..count {
        world.spawn(Session {
            conn: ConnId(u64::from(n) + 1),
            user_id: NetUserId(uuid::Uuid::from_bytes([n + 1; 16])),
            name: format!("p{n}"),
            attached: None,
            last_input: 0,
            sync: Default::default(),
        });
    }
}

fn ids(maps: Vec<GameMap>) -> Vec<String> {
    maps.into_iter().map(|m| m.id).collect()
}

#[test]
fn eligible_maps_follow_player_count_with_fallback() {
    let mut world = world();
    players(&mut world, 1);
    assert_eq!(ids(currently_eligible_maps(&mut world)), ["Small", "Other"]);
    players(&mut world, 4);
    assert_eq!(ids(currently_eligible_maps(&mut world)), ["Big", "Other"]);
    // Никто из пула не подходит — запасные.
    world.set_cvar_text("game.map_pool", "TinyPool").unwrap();
    assert_eq!(ids(currently_eligible_maps(&mut world)), ["Spare"]);
}

#[test]
fn rotation_picks_the_least_recently_played_map() {
    let mut world = world();
    players(&mut world, 1);
    world.resource_mut::<GameMaps>().previous = ["Other", "Small"].map(String::from).into();
    // Обе сыграны: «Other» давнее.
    select_map_by_config_rules(&mut world);
    assert_eq!(selected_map(&world).unwrap().id, "Other");
    assert_eq!(
        world.resource::<GameMaps>().previous,
        ["Other", "Small", "Other"].map(String::from)
    );
    // Выбор держится до перезапуска раунда.
    clear_selected_map(&mut world);
    select_map_by_config_rules(&mut world);
    assert_eq!(selected_map(&world).unwrap().id, "Small");
    // Глубина памяти обрезает очередь.
    world.set_cvar(&GAME_MAP_MEMORY_DEPTH, 1).unwrap();
    assert_eq!(world.resource::<GameMaps>().previous.len(), 1);
}

#[test]
fn config_map_wins_and_forcemap_sets_or_clears_it() {
    let mut world = world();
    players(&mut world, 1);
    assert!(select_map(&mut world, "Other"));
    assert!(!select_map(&mut world, "Nowhere"));
    assert_eq!(selected_map(&world).unwrap().id, "Other");
    let mut shell = ConsoleShell::server();
    world.execute_command(&mut shell, "forcemap Big").unwrap();
    assert_eq!(shell.output, ["cmd-forcemap-success"]);
    assert_eq!(selected_map(&world).unwrap().id, "Big");
    let mut shell = ConsoleShell::server();
    world
        .execute_command(&mut shell, "forcemap Nowhere")
        .unwrap();
    assert_eq!(shell.output, ["cmd-forcemap-map-not-found"]);
    let mut shell = ConsoleShell::server();
    world.execute_command(&mut shell, "forcemap \"\"").unwrap();
    assert_eq!(shell.output, ["cmd-forcemap-cleared"]);
    assert_eq!(selected_map(&world).unwrap().id, "Other");
}

#[test]
fn preset_pool_limits_maps_and_revalidates_the_choice() {
    let mut world = world();
    players(&mut world, 1);
    assert!(select_map(&mut world, "Other"));
    crate::game_ticker::set_game_preset(&mut world, "Tiny", false, None);
    // «Other» не из пула режима — выбрана подходящая из его пула.
    assert_eq!(selected_map(&world).unwrap().id, "Small");
    assert_eq!(ids(all_votable_maps(&world)), ["Small"]);
}
