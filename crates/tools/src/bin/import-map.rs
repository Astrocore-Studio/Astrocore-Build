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
    let known: BTreeSet<String> = polygon::load_registry(&polygon::content_dir())
        .map(|r| r.ids().map(str::to_string).collect())
        .unwrap_or_default();
    match tools::import_map::import(
        &yaml,
        &name,
        &known,
        &tools::import_map::Materials::default(),
    ) {
        Ok((map, r)) => {
            if let Err(e) = map.save(Path::new(&args[1])) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            println!(
                "карта «{name}» → {}\n  пол: {} клеток, стен-клеток: {} (рёбер {}), дверей: {} (не поставлено {}),\n  спавнов: {}, предметов: {}, труб: {}, пропущено сущностей: {}",
                args[1],
                r.floor_tiles,
                r.wall_cells,
                r.wall_edges,
                r.doors,
                r.doors_unplaced,
                r.spawns,
                r.items,
                r.pipes,
                r.skipped_entities
            );
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
