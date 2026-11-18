//! `voxelize [<assets/ss14>]` — воксельные модели для спрайтов манифеста рендера (ADR-0002).
//!
//! Для каждой записи `sprites` манифеста (кроме анимированных) строится модель и пишется в
//! `Models/<путь набора RSI>/<состояние>.avox` (`.flat.avox` — для лежащих вещей). Манифест
//! потом подхватывает модель (`server --write-manifest`). Существующие файлы перезаписываются.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn directions(rsi: &Path, state: &str) -> u32 {
    std::fs::read(rsi.join("meta.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|m| {
            m["states"].as_array()?.iter().find_map(|s| {
                (s["name"].as_str()? == state).then(|| s["directions"].as_u64().unwrap_or(1) as u32)
            })
        })
        .unwrap_or(1)
}

fn main() {
    let root = std::env::args()
        .nth(1)
        .map_or_else(polygon::assets_dir, PathBuf::from);
    let manifest: serde_json::Value = match std::fs::read(root.join("render-manifest.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
    {
        Some(m) => m,
        None => {
            eprintln!("нет манифеста рендера в {}", root.display());
            std::process::exit(1);
        }
    };
    let mut done: BTreeSet<String> = BTreeSet::new();
    let (mut carved, mut upright, mut flat, mut skipped) = (0, 0, 0, 0);
    for entry in manifest["sprites"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, e)| e)
    {
        let Some(image) = entry["image"].as_str() else {
            continue;
        };
        if entry["frames"].as_u64().unwrap_or(1) > 1 {
            skipped += 1;
            continue;
        }
        let is_flat = entry["layout"].as_str() == Some("flat");
        let Some(model_rel) = assets::model_path(image, is_flat) else {
            continue;
        };
        if !done.insert(model_rel.clone()) {
            continue;
        }
        let Ok(sheet) = image::open(root.join(image)).map(|i| i.to_rgba8()) else {
            skipped += 1;
            continue;
        };
        let grid = entry["grid"].as_array().map_or([1, 1], |g| {
            [
                g.first().and_then(|v| v.as_u64()).unwrap_or(1) as u32,
                g.get(1).and_then(|v| v.as_u64()).unwrap_or(1) as u32,
            ]
        });
        let (cw, ch) = (
            sheet.width() / grid[0].max(1),
            sheet.height() / grid[1].max(1),
        );
        if cw == 0 || ch == 0 || cw > 256 || ch > 256 {
            skipped += 1;
            continue;
        }
        let cell = |i: u32| {
            image::imageops::crop_imm(&sheet, (i % grid[0]) * cw, (i / grid[0]) * ch, cw, ch)
                .to_image()
        };
        let size = entry["size"][0].as_f64().unwrap_or(1.0).max(0.01) as f32;
        let vpm = (cw as f32 / size).round().clamp(8.0, 512.0) as u16;
        let path = Path::new(image);
        let state = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let dirs = directions(&root.join(path.parent().unwrap_or(Path::new(""))), state);
        let voxels = if is_flat {
            let thickness = entry["thickness"].as_f64().unwrap_or(0.03) as f32;
            flat += 1;
            tools::voxelize::extrude_flat(
                &cell(0),
                ((thickness * f32::from(vpm)).round() as usize).max(2),
            )
        } else if dirs >= 4 && grid[0] * grid[1] >= 4 {
            carved += 1;
            let views = [cell(0), cell(1), cell(2), cell(3)];
            tools::voxelize::carve([&views[0], &views[1], &views[2], &views[3]])
        } else {
            upright += 1;
            tools::voxelize::extrude_upright(&cell(0))
        };
        let Some(model) = tools::voxelize::to_model(&voxels, vpm) else {
            skipped += 1;
            continue;
        };
        let out = root.join(&model_rel);
        if let Some(dir) = out.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&out, astrocore_voxels::encode(&model)) {
            eprintln!("{}: {e}", out.display());
        }
    }
    println!(
        "моделей: {} (из четырёх видов {carved}, стоячих выдавленных {upright}, лежащих {flat}); пропущено {skipped}",
        done.len().min(carved + upright + flat)
    );
}
