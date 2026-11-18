//! `import-map <карта.yml> <выход.ron> [имя] [--proto-root <Resources/Prototypes>]` — карта SS14 →
//! формат карты Build. С `--proto-root` из прототипа `gameMap` берутся должности станции.

use std::collections::BTreeSet;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("использование: import-map <карта.yml> <выход.ron> [имя]");
        std::process::exit(2);
    }
    let proto_root = args
        .iter()
        .position(|a| a == "--proto-root")
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from);
    let name = args
        .get(2)
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| {
            Path::new(&args[0])
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("map")
                .to_string()
        });
    let yaml = std::fs::read_to_string(&args[0]).unwrap_or_else(|e| {
        eprintln!("{}: {e}", args[0]);
        std::process::exit(1);
    });
    let registry = polygon::load_registry(&polygon::content_dir()).ok();
    // Карта может ставить только то, что у Build есть чем рисовать и обрабатывать: у узла дерева
    // прототипов SS14 без компонентов (ни своих, ни унаследованных) механик пока нет.
    let known: BTreeSet<String> = registry
        .as_ref()
        .map(|r| {
            r.ids()
                .filter(|id| r.resolve(id).is_ok_and(|p| !p.components.is_empty()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut materials = tools::import_map::Materials::default();
    if let Some(registry) = &registry
        && let Ok(data) = registry.resolve("AtmosAlertsData")
        && let Some(data) = data.component("AtmosAlerts")
    {
        for row in data
            .get("devices")
            .and_then(prototypes::Data::as_list)
            .unwrap_or_default()
        {
            if row.get("wallMount").and_then(prototypes::Data::as_bool) == Some(true)
                && let Some(name) = row.get("prototype").and_then(prototypes::Data::as_str)
            {
                materials.wallmounts.insert(name.into());
            }
        }
    }
    for t in polygon::load_tiles(&polygon::content_dir()).unwrap_or_default() {
        if let Ok(id) = astrocore_core::PrototypeId::new(polygon::tile_material_id(&t.tile)) {
            materials.tiles.insert(t.tile, id);
        }
    }
    for a in polygon::load_door_access(&polygon::content_dir()).unwrap_or_default() {
        materials.door_access.insert(a.proto, a.access);
    }
    for w in polygon::load_wall_hp(&polygon::content_dir()).unwrap_or_default() {
        materials.wall_hp.insert(
            w.proto,
            tools::import_map::WallClass {
                hp: w.hp,
                coefficients: w.coefficients,
                flat: w.flat,
            },
        );
    }
    if let Some(registry) = &registry
        && let Ok(volumes) = polygon::volumes_of(registry)
    {
        for (name, v) in volumes {
            materials.volume_m.insert(name, v.height_px / 32.0);
        }
    }
    if let Some(registry) = &registry {
        for name in registry.ids().map(str::to_string).collect::<Vec<_>>() {
            let Ok(r) = registry.spawnable(&name) else {
                continue;
            };
            let Some(air) = r.component("Airtight") else {
                continue;
            };
            let mask = air
                .get("directions")
                .and_then(prototypes::Data::as_list)
                .unwrap_or_default()
                .iter()
                .filter_map(|d| match d.as_str()? {
                    "South" => Some(1u8),
                    "East" => Some(2),
                    "North" => Some(4),
                    "West" => Some(8),
                    _ => None,
                })
                .fold(0, |a, b| a | b);
            if mask != 0 {
                materials.airtight.insert(name, mask);
            }
        }
    }
    if let Some(root) = &proto_root {
        match tools::import_map::game_map_info(root, Path::new(&args[0])) {
            Some(info) => {
                println!(
                    "прототип карты «{}»: должностей станции {}",
                    info.name,
                    info.jobs.len()
                );
                materials.station_jobs = info.jobs;
            }
            None => eprintln!(
                "прототип gameMap для этой карты не найден в {}",
                root.display()
            ),
        }
    }
    match tools::import_map::import(&yaml, &name, &known, &materials) {
        Ok((map, r)) => {
            if let Err(error) = map.build_world("import-validation") {
                eprintln!("Импорт создаёт некорректный мир: {error}");
                std::process::exit(1);
            }
            if let Err(e) = map.save(Path::new(&args[1])) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            println!(
                "карта «{name}» → {}\n  пол: {} клеток, стен-клеток: {} (рёбер {}, блоков {}), дверей: {} (не поставлено {}), огнезапоров: {} (не поставлено {}), сигнализаций со связями: {},\n  спавнов: {}, предметов: {}, труб: {}, труб диспозала: {}, дверей с доступом: {}, клеток с кабелями: {}, мебели-ящиков: {}, стульев: {}, пропущено сущностей: {}",
                args[1],
                r.floor_tiles,
                r.wall_cells,
                r.wall_edges,
                r.wall_blocks,
                r.doors,
                r.doors_unplaced,
                r.firelocks,
                r.firelocks_unplaced,
                r.alarms,
                r.spawns,
                r.items,
                r.pipes,
                r.disposal_tubes,
                r.door_access,
                r.cables,
                r.furniture,
                r.chairs,
                r.skipped_entities
            );
            println!("  запертых для воздуха рёбер: {}", r.airtight_edges);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
