use std::collections::BTreeSet;

use super::*;

/// Карта SS14: комната 5×3 с кольцом стен 7×5 и дверью в нижней стене (SS14 y = 0).
fn room_yaml(version: i64) -> String {
    let stride = if version >= 7 { 7 } else { 6 };
    let mut bytes = vec![0u8; 16 * 16 * stride];
    for y in 0..=4usize {
        for x in 0..=6usize {
            let at = (y * 16 + x) * stride;
            bytes[at..at + 4].copy_from_slice(&1i32.to_le_bytes());
        }
    }
    let tiles = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut ents = String::new();
    let mut wall = |x: i32, y: i32| {
        ents.push_str(&format!(
            "      - uid: 1\n        components:\n        - type: Transform\n          pos: {x}.5,{y}.5\n"
        ));
    };
    for x in 0..=6 {
        for y in [0, 4] {
            if !(y == 0 && x == 3) {
                wall(x, y);
            }
        }
    }
    for y in 1..=3 {
        for x in [0, 6] {
            wall(x, y);
        }
    }
    format!(
        "meta:\n  format: {version}\ntilemap:\n  0: Space\n  1: FloorSteel\nentities:\n\
         - proto: \"\"\n  entities:\n  - uid: 100\n    components:\n    - type: MapGrid\n      chunks:\n        0,0:\n          ind: 0,0\n          version: {version}\n          size: 16\n          tiles: {tiles}\n    - type: Transform\n      pos: 0,0\n\
         - proto: WallSolid\n  entities:\n{ents}\
         - proto: AirlockGlass\n  entities:\n  - uid: 2\n    components:\n    - type: Transform\n      pos: 3.5,0.5\n\
         - proto: SpawnPointLatejoin\n  entities:\n  - uid: 3\n    components:\n    - type: Transform\n      pos: 2.5,2.5\n\
         - proto: Crowbar\n  entities:\n  - uid: 4\n    components:\n    - type: Transform\n      pos: 1.5,1.5\n\
         - proto: Lamp\n  entities:\n  - uid: 5\n    components:\n    - type: Transform\n      pos: 1.5,1.5\n"
    )
}

fn known() -> BTreeSet<String> {
    ["Crowbar".to_string()].into()
}

#[test]
fn a_room_with_a_door_becomes_walls_floor_and_a_door_edge() {
    let (map, r) = import(&room_yaml(7), "room", &known(), &Materials::default()).unwrap();
    assert_eq!(r.floor_tiles, 16, "комната 5×3 и клетка двери");
    assert_eq!(r.wall_cells, 19, "кольцо 7×5 без клетки двери");
    assert_eq!(
        r.wall_edges, 17,
        "периметр комнаты без двери и две боковые стороны двери"
    );
    assert_eq!((r.doors, r.doors_unplaced), (1, 0));
    assert_eq!((r.spawns, r.items, r.skipped_entities), (1, 1, 1));
    assert_eq!(map.items[0].prototype, "Crowbar");
    // Ось Y карты SS14 уходит в −Z: спавн (2.5, 2.5) → z = −2.5.
    assert_eq!((map.spawn[0], map.spawn[2]), (2.5, -2.5));
}

#[test]
fn the_imported_map_builds_a_valid_world() {
    let (map, _) = import(&room_yaml(7), "room", &known(), &Materials::default()).unwrap();
    let world = map.build_world("room").unwrap();
    assert_eq!(world.elements().count(), map.elements.len());
    let text = map.to_ron().unwrap();
    assert_eq!(maps::MapFile::parse(&text).unwrap(), map);
}

#[test]
fn chunk_versions_six_and_seven_decode_the_same_tiles() {
    let a = import(&room_yaml(7), "r", &known(), &Materials::default())
        .unwrap()
        .1;
    let b = import(&room_yaml(6), "r", &known(), &Materials::default())
        .unwrap()
        .1;
    assert_eq!(a, b);
}

#[test]
fn a_map_without_floor_is_an_error() {
    let yaml = "tilemap:\n  0: Space\nentities: []\n";
    assert!(matches!(
        import(yaml, "x", &known(), &Materials::default()),
        Err(ImportError::NoFloor)
    ));
}
