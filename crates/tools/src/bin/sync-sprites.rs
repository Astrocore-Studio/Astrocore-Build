//! `sync-sprites <SS14/Resources/Textures> [<назначение>]` — копирует нужные наборы спрайтов.

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("использование: sync-sprites <SS14/Resources/Textures> [<assets/ss14/Textures>]");
        std::process::exit(2);
    }
    let source = PathBuf::from(&args[0]);
    let dest = args
        .get(1)
        .map_or_else(|| polygon::assets_dir().join("Textures"), PathBuf::from);
    let (rsi, files) = match tools::sync_sprites::referenced(&polygon::content_dir()) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    match tools::sync_sprites::sync(&source, &dest, &rsi, &files) {
        Ok(r) => {
            println!(
                "скопировано: {}, объём {:.2} МБ",
                r.copied.len(),
                r.bytes as f64 / 1048576.0
            );
            for m in &r.missing {
                println!("нет в источнике: {m}");
            }
            for (n, why) in &r.unlicensed {
                println!("без лицензии (скопировано): {n}: {why}");
            }
            for (n, why) in &r.rejected {
                println!("отклонено: {n}: {why}");
            }
            if !r.rejected.is_empty() {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
