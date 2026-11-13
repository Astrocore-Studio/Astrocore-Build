//! `import-map <карта.yml> <выход.ron> [имя]` — карта SS14 → формат карты Build.

use std::collections::BTreeSet;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("использование: import-map <карта.yml> <выход.ron> [имя]");
        std::process::exit(2);
    }
    let name = args.get(2).cloned().unwrap_or_else(|| {
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
    let known: BTreeSet<String> = registry
        .as_ref()
        .map(|r| r.ids().map(str::to_string).collect())
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
    match tools::import_map::import(&yaml, &name, &known, &materials) {
        Ok((map, r)) => {
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
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
