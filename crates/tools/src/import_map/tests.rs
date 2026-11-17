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
fn repairable_items_keep_authored_damage_and_a_vendors_broken_state() {
    let yaml = room_yaml(7)
        .replace("proto: Crowbar", "proto: VendingMachineCola")
        .replacen("  - uid: 4\n    components:\n", "  - uid: 4\n    components:\n    - type: Damageable\n      damage:\n        types: {Blunt: 78.25, Heat: 12.5}\n    - type: VendingMachine\n      broken: true\n", 1);
    let (map, _) = import(
        &yaml,
        "repair",
        &["VendingMachineCola".into()].into(),
        &Materials::default(),
    )
    .unwrap();
    let item = map
        .items
        .iter()
        .find(|item| item.prototype == "VendingMachineCola")
        .unwrap();
    let damage: serde_json::Value = serde_json::from_str(&item.meta["damage"]).unwrap();
    assert_eq!(damage, serde_json::json!({"Blunt":78.25,"Heat":12.5}));
    assert_eq!(item.meta["broken"], "1");
    // build_world validates structure; the game places MapItems later.
    let saved = maps::MapFile::parse(&map.to_ron().unwrap()).unwrap();
    let saved = saved
        .items
        .iter()
        .find(|item| item.prototype == "VendingMachineCola")
        .unwrap();
    assert_eq!(saved.meta, item.meta);
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
fn a_directional_window_can_share_its_tile_with_a_windoor_on_another_side() {
    let yaml = room_yaml(7)
        .replace("proto: AirlockGlass", "proto: Windoor")
        .replace("pos: 3.5,0.5", "pos: 3.5,1.5")
        + "- proto: WindowReinforcedDirectional\n  entities:\n  - uid: 1000\n    components:\n    - type: Transform\n      pos: 3.5,1.5\n      rot: 1.5707963267948966 rad\n";
    let (map, report) = import(&yaml, "glazing", &known(), &Materials::default()).unwrap();
    assert_eq!((report.doors, report.doors_unplaced), (1, 0));
    assert!(
        map.elements
            .iter()
            .any(|e| matches!(e.kind, StructureKind::Opening(OpeningKind::Window)))
    );
    assert!(map.build_world("glazing").is_ok());
}

#[test]
fn station_airlock_keeps_its_board_skin_and_access_and_loose_electronics_remain_items() {
    let proto = "AirlockArmoryGlassLocked";
    let yaml = room_yaml(7)
        .replace("proto: AirlockGlass", &format!("proto: {proto}"))
        .replace("proto: Crowbar", "proto: DoorElectronics");
    let mut materials = Materials::default();
    materials
        .door_access
        .insert(proto.into(), vec![vec!["Armory".into()]]);
    let (map, report) = import(
        &yaml,
        "armory",
        &["DoorElectronics".into()].into(),
        &materials,
    )
    .unwrap();
    assert_eq!(report.doors, 1);
    assert!(map.items.iter().any(|i| i.prototype == "DoorElectronics"));
    let zone = map
        .zones
        .iter()
        .find(|z| z.metadata.contains_key("door_element"))
        .unwrap();
    assert_eq!(zone.metadata["frame_board"], "DoorElectronicsArmory");
    assert_eq!(zone.metadata["installed_access"], "Armory");
    assert_eq!(zone.metadata["construction_node"], "glassAirlock");
    assert!(
        map.elements
            .iter()
            .any(|e| e.material.as_str() == "ss14:airlock_armory_glass_locked")
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
fn a_window_becomes_a_thin_glass_pane_instead_of_a_solid_block() {
    let yaml = room_yaml(7).replace("proto: WallSolid", "proto: Window");
    let (map, r) = import(&yaml, "room", &known(), &Materials::default()).unwrap();
    assert_eq!(r.wall_blocks, 0, "сплошных блоков у окон нет");
    assert_eq!(r.wall_edges, 17, "рёбра-панели те же, что у стен");
    let text = format!("{:?}", map.elements);
    assert!(text.contains("Window"), "ребро — Opening(Window)");
    assert!(!text.contains("Freeform"), "блоков нет");
}

#[test]
fn the_game_map_prototype_gives_the_station_name_and_job_seats_without_map_names_in_code() {
    let dir = std::env::temp_dir().join(format!("gamemap_{}", std::process::id()));
    let maps = dir.join("Prototypes").join("Maps").join("Deep");
    std::fs::create_dir_all(&maps).unwrap();
    std::fs::write(
        maps.join("any.yml"),
        "- type: gameMap\n  id: Other\n  mapName: Other\n  mapPath: /Maps/other.yml\n  stations: {}\n\
         - type: gameMap\n  id: Sandbox\n  mapName: Sandbox\n  mapPath: /Maps/Test/sandbox.yml\n  stations:\n    S:\n      components:\n      - type: StationNameSetup\n      - type: StationJobs\n        availableJobs:\n          Captain: [ -1, -1 ]\n          Engineer: [ 2, 4 ]\n",
    )
    .unwrap();
    let proto = dir.join("Prototypes");
    let info = game_map_info(&proto, Path::new("/x/Resources/Maps/Test/sandbox.yml")).unwrap();
    assert_eq!(info.name, "Sandbox");
    assert_eq!(
        info.jobs,
        vec![("Captain".into(), -1, -1), ("Engineer".into(), 2, 4)]
    );
    assert!(game_map_info(&proto, Path::new("/x/Resources/Maps/nope.yml")).is_none());
    // Должности попадают в зону карты записями `job_slot_<n>`.
    let mats = Materials {
        station_jobs: info.jobs,
        ..Materials::default()
    };
    let (map, _) = import(&room_yaml(7), "room", &known(), &mats).unwrap();
    let all: Vec<(&String, &String)> = map.zones.iter().flat_map(|z| z.metadata.iter()).collect();
    assert!(
        all.iter()
            .any(|(k, v)| k.as_str() == "job_slot_1" && v.as_str() == "Engineer:2:4")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn entities_and_tiles_of_a_shifted_and_rotated_grid_share_one_world_frame() {
    // Сетка 7 сдвинута на (10, 4) и повёрнута на 180°; её сущность лежит в её координатах.
    let mut root: Value = serde_yaml::from_str(
        "entities:\n\
         - proto: ''\n  entities:\n  - uid: 7\n    components:\n    - type: MapGrid\n      chunks: {}\n    - type: Transform\n      pos: 10,4\n      rot: 3.141592653589793 rad\n\
         - proto: Crowbar\n  entities:\n  - uid: 8\n    components:\n    - type: Transform\n      parent: 7\n      pos: 2.5,1.5\n      rot: 1.5707963267948966 rad\n\
         - proto: Pen\n  entities:\n  - uid: 9\n    components:\n    - type: Transform\n      parent: 99\n      pos: 2.5,1.5\n",
    )
    .unwrap();
    let frames = to_world_frame(&mut root);
    let f = frames[&7];
    assert_eq!((f.x, f.y, f.q), (10.0, 4.0, 2));
    // Клетка (2, 1) сетки → центр (2.5, 1.5) → поворот на 180° (−2.5, −1.5) → +(10, 4) = (7.5, 2.5).
    assert_eq!(f.cell_to_world((2, 1)), (7, 2));
    let at = |uid: i64| {
        let groups = root["entities"].as_sequence().unwrap();
        let e = groups
            .iter()
            .flat_map(|g| g["entities"].as_sequence().unwrap().iter())
            .find(|e| e["uid"].as_i64() == Some(uid))
            .unwrap();
        let t = e["components"][0].clone();
        (
            t["pos"].as_str().map(str::to_string),
            t["rot"].as_str().map(str::to_string),
        )
    };
    let (pos, rot) = at(8);
    assert_eq!(pos.as_deref(), Some("7.5,2.5"), "позиция в мире");
    let rad: f64 = rot.unwrap().trim_end_matches(" rad").parse().unwrap();
    assert!(
        (rad - 1.5 * std::f64::consts::PI).abs() < 1e-9,
        "повороты складываются"
    );
    // Сущность с неизвестным родителем (карта, контейнер) не трогается.
    assert_eq!(at(9).0.as_deref(), Some("2.5,1.5"));
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
    assert_eq!(pipe_base_dirs("GasPipeStraightAlt1"), Some(1 | 2));
    assert_eq!(pipe_base_dirs("GasPipeTJunction"), Some(2 | 4 | 8));
}
