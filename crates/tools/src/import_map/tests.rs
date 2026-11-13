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
fn device_links_preserve_uids_and_names_for_colocated_entities() {
    let yaml=room_yaml(7).replacen("  - uid: 4\n    components:\n", "  - uid: 4\n    components:\n    - type: DeviceList\n      devices: [5]\n    - type: MetaData\n      name: Room control\n",1);
    let known = ["Crowbar".into(), "Lamp".into()].into();
    let (map, _) = import(&yaml, "room", &known, &Materials::default()).unwrap();
    let control = map.items.iter().find(|i| i.prototype == "Crowbar").unwrap();
    let linked = map.items.iter().find(|i| i.prototype == "Lamp").unwrap();
    assert_eq!(control.meta["ss14_uid"], "4");
    assert_eq!(control.meta["device_ids"], "5");
    assert_eq!(control.meta["name"], "Room control");
    assert_eq!(linked.meta["ss14_uid"], "5");
    assert_eq!((control.x, control.z), (linked.x, linked.z));
}

#[test]
fn wall_mount_uses_the_source_facing_and_retains_its_map_anchor() {
    let yaml = room_yaml(7)
        + "- proto: FireAlarm\n  entities:\n  - uid: 6\n    components:\n    - type: Transform\n      pos: 3.5,4.5\n";
    let mut known = known();
    known.insert("FireAlarm".into());
    let mut mats = Materials::default();
    mats.wallmounts.insert("FireAlarm".into());
    let (map, _) = import(&yaml, "room", &known, &mats).unwrap();
    let alarm = map
        .items
        .iter()
        .find(|i| i.prototype == "FireAlarm")
        .unwrap();
    assert_eq!((alarm.x, alarm.z), (3.5, -3.75));
    assert_eq!(alarm.meta["wallmount_origin"], "3.5,-4.5");
}

#[test]
fn firelock_uid_resolves_to_the_imported_structure_element() {
    let yaml = room_yaml(7).replace("proto: AirlockGlass", "proto: Firelock");
    let (map, report) = import(&yaml, "room", &known(), &Materials::default()).unwrap();
    assert_eq!(report.firelocks, 1);
    let link = map
        .zones
        .iter()
        .find(|z| z.metadata.get("ss14_uid").is_some_and(|uid| uid == "2"))
        .unwrap();
    let id = u128::from_str_radix(&link.metadata["alarm_firelock"], 16).unwrap();
    assert!(
        map.elements
            .iter()
            .any(|element| element.id == ObjectId::from_u128(id)
                && element.material.as_str() == "ss14:firelock")
    );
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
fn walls_carry_their_strength_from_the_prototype_table() {
    let mut mats = Materials::default();
    mats.wall_hp.insert(
        "WallSolid".into(),
        WallClass {
            hp: 250.0,
            coefficients: [("Piercing".to_string(), 0.7)].into(),
            flat: [("Structural".to_string(), 3.0)].into(),
        },
    );
    let (map, _) = import(&room_yaml(7), "room", &known(), &mats).unwrap();
    let meta: Vec<(&String, &String)> = map.zones.iter().flat_map(|z| z.metadata.iter()).collect();
    let class = meta
        .iter()
        .find(|(k, _)| k.as_str() == "wall_class_0")
        .expect("класс стены");
    assert_eq!(class.1, "hp=250;coef=Piercing:0.7;flat=Structural:3");
    let cells = meta
        .iter()
        .find(|(k, _)| k.starts_with("wall_cells_0_"))
        .expect("клетки стен");
    assert!(cells.1.contains("0:-1"), "угловая стена: {}", cells.1);
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

#[test]
fn pipe_directions_rotate_counter_clockwise() {
    // Юг → восток → север → запад (RotatePipeDirection), труба «прямая» (N+S) на 90° становится W+E.
    assert_eq!(rotate_pipe_dirs(2, 1), 4);
    assert_eq!(rotate_pipe_dirs(2, 2), 1);
    assert_eq!(rotate_pipe_dirs(2, 3), 8);
    assert_eq!(dirs_text(rotate_pipe_dirs(1 | 2, 1)), "EW");
    assert_eq!(
        dirs_text(rotate_pipe_dirs(2 | 8, 1)),
        "SE",
        "изгиб юг+запад на 90° — юг+восток"
    );
    assert_eq!(dirs_text(rotate_pipe_dirs(2 | 8, 2)), "NE");
    assert_eq!(pipe_base_dirs("GasPipeStraightAlt1"), None);
    assert_eq!(pipe_base_dirs("GasPipeTJunction"), Some(2 | 4 | 8));
}
