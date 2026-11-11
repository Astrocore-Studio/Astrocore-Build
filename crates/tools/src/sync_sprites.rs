//! Копирование спрайтов из каталога `Resources/Textures` сборки SS14 в `assets/ss14/Textures`.
//!
//! Нужные наборы собираются из содержимого Build (прототипы предметов, двери, материалы) и
//! фиксированного списка (тело персонажа, интерфейс). Набор RSI копируется целиком, только если
//! есть читаемый `meta.json`. Набор без лицензии или авторства копируется, но попадает в отчёт
//! (`unlicensed`), чтобы его можно было проверить позже.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use prototypes::Data;

/// Наборы, которые нужны клиенту независимо от прототипов.
pub const FIXED_RSI: &[&str] = &[
    "Mobs/Species/Human/parts.rsi",
    // Значки состояния справа вверху (панель алертов SS14).
    "Interface/Alerts/human_alive.rsi",
    "Interface/Alerts/human_critical.rsi",
    "Interface/Alerts/human_dead.rsi",
    "Interface/Alerts/bleed.rsi",
    "Interface/Alerts/breathing.rsi",
];
/// Каталоги интерфейса (худ): копируются только картинки верхнего уровня с нужным началом имени
/// (слоты; плитки и части рамки хранилища).
pub const FIXED_DIRS: &[(&str, &[&str])] = &[
    ("Interface/Default/Slots", &[""]),
    ("Interface/Default/Storage", &["tile_", "piece_"]),
];
pub const FIXED_FILES: &[&str] = &[
    "Interface/Default/meta.json",
    "Interface/Default/SlotBackground.png",
    "Interface/Default/blocked.png",
    "Interface/Default/slot_highlight.png",
    "Interface/Inventory/hand_active.png",
    "Interface/Inventory/hand_slot_highlight.png",
    "Interface/Inventory/inv_slot_background.png",
];

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("содержимое: {0}")]
    Content(String),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub copied: Vec<String>,
    pub missing: Vec<String>,
    /// (набор, причина отказа): нет читаемого `meta.json`.
    pub rejected: Vec<(String, String)>,
    /// (набор, чего не хватает): скопированы, но без лицензии или авторства.
    pub unlicensed: Vec<(String, String)>,
    pub bytes: u64,
}

/// Наборы RSI и отдельные картинки, на которые ссылается содержимое Build.
pub fn referenced(content: &Path) -> Result<(BTreeSet<String>, BTreeSet<String>), SyncError> {
    let registry =
        polygon::load_registry(content).map_err(|e| SyncError::Content(e.to_string()))?;
    let mut rsi: BTreeSet<String> = FIXED_RSI.iter().map(|s| (*s).to_string()).collect();
    let mut files = BTreeSet::new();
    let ids: Vec<String> = registry.ids().map(str::to_string).collect();
    for id in ids {
        let Ok(r) = registry.spawnable(&id) else {
            continue;
        };
        if let Some(s) = r
            .component("Sprite")
            .and_then(|c| c.get("sprite"))
            .and_then(Data::as_str)
        {
            rsi.insert(s.to_string());
        }
    }
    for d in polygon::load_doors(content).map_err(|e| SyncError::Content(e.to_string()))? {
        rsi.insert(d.rsi);
    }
    for m in polygon::load_materials(content).map_err(|e| SyncError::Content(e.to_string()))? {
        files.insert(m.image);
    }
    Ok((rsi, files))
}

/// Ошибка `meta.json`, из-за которой набор нельзя считать RSI.
fn meta_problem(dir: &Path) -> Option<String> {
    let Ok(text) = std::fs::read_to_string(dir.join("meta.json")) else {
        return Some("нет meta.json".into());
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .err()
        .map(|_| "meta.json не разбирается".into())
}

/// Чего не хватает по лицензии: `None`, если лицензия и авторство указаны.
fn license_gap(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("meta.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let has = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .is_some_and(|s| !s.trim().is_empty())
    };
    match (has("license"), has("copyright")) {
        (true, true) => None,
        (false, false) => Some("нет license и copyright".into()),
        (false, true) => Some("нет license".into()),
        (true, false) => Some("нет copyright".into()),
    }
}

fn copy_dir(src: &Path, dst: &Path, bytes: &mut u64) -> Result<(), SyncError> {
    std::fs::create_dir_all(dst).map_err(|e| SyncError::Io(dst.to_path_buf(), e))?;
    for entry in walkdir::WalkDir::new(src).min_depth(1) {
        let entry = entry.map_err(|e| SyncError::Content(e.to_string()))?;
        let rel = entry.path().strip_prefix(src).unwrap_or(entry.path());
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| SyncError::Io(target.clone(), e))?;
        } else {
            *bytes += std::fs::copy(entry.path(), &target)
                .map_err(|e| SyncError::Io(target.clone(), e))?;
        }
    }
    Ok(())
}

/// Скопировать наборы и файлы из `source` в `dest` (оба — каталоги `Textures`).
pub fn sync(
    source: &Path,
    dest: &Path,
    rsi: &BTreeSet<String>,
    files: &BTreeSet<String>,
) -> Result<SyncReport, SyncError> {
    let mut report = SyncReport::default();
    for name in rsi {
        let src = source.join(name);
        if !src.is_dir() {
            report.missing.push(name.clone());
            continue;
        }
        if let Some(why) = meta_problem(&src) {
            report.rejected.push((name.clone(), why));
            continue;
        }
        if let Some(gap) = license_gap(&src) {
            report.unlicensed.push((name.clone(), gap));
        }
        copy_dir(&src, &dest.join(name), &mut report.bytes)?;
        report.copied.push(name.clone());
    }
    let all_files = files
        .iter()
        .map(String::as_str)
        .chain(FIXED_FILES.iter().copied());
    for name in all_files {
        let src = source.join(name);
        if !src.is_file() {
            report.missing.push(name.to_string());
            continue;
        }
        let dst = dest.join(name);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SyncError::Io(parent.to_path_buf(), e))?;
        }
        report.bytes += std::fs::copy(&src, &dst).map_err(|e| SyncError::Io(dst.clone(), e))?;
        report.copied.push(name.to_string());
    }
    for (name, prefixes) in FIXED_DIRS {
        let src = source.join(name);
        if !src.is_dir() {
            report.missing.push((*name).to_string());
            continue;
        }
        let dst = dest.join(name);
        std::fs::create_dir_all(&dst).map_err(|e| SyncError::Io(dst.clone(), e))?;
        let entries = std::fs::read_dir(&src).map_err(|e| SyncError::Io(src.clone(), e))?;
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().to_string();
            let wanted = file.ends_with(".png") && prefixes.iter().any(|p| file.starts_with(p));
            if wanted && entry.path().is_file() {
                report.bytes += std::fs::copy(entry.path(), dst.join(&file))
                    .map_err(|e| SyncError::Io(dst.join(&file), e))?;
            }
        }
        report.copied.push((*name).to_string());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rsi(dir: &Path, meta: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("meta.json"), meta).unwrap();
        std::fs::write(dir.join("icon.png"), b"png").unwrap();
    }

    #[test]
    fn a_set_without_license_is_copied_with_a_warning_and_a_broken_one_is_rejected() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        rsi(
            &src.path().join("A/good.rsi"),
            r#"{"license":"CC-BY-SA-3.0","copyright":"x"}"#,
        );
        rsi(&src.path().join("A/nolicense.rsi"), r#"{"copyright":"x"}"#);
        rsi(&src.path().join("A/broken.rsi"), "не json");
        let names: BTreeSet<String> = [
            "A/good.rsi",
            "A/nolicense.rsi",
            "A/broken.rsi",
            "A/absent.rsi",
        ]
        .map(String::from)
        .into();
        let r = sync(src.path(), dst.path(), &names, &BTreeSet::new()).unwrap();
        assert_eq!(
            r.copied,
            vec!["A/good.rsi".to_string(), "A/nolicense.rsi".to_string()]
        );
        assert!(dst.path().join("A/nolicense.rsi/icon.png").is_file());
        assert_eq!(
            r.unlicensed,
            vec![("A/nolicense.rsi".to_string(), "нет license".to_string())]
        );
        assert!(!dst.path().join("A/broken.rsi").exists());
        assert_eq!(r.rejected.len(), 1);
        assert!(r.missing.contains(&"A/absent.rsi".to_string()));
    }
}
