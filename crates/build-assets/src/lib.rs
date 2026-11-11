use std::path::{Path, PathBuf};

use astrocore_sprites::rsi::{RsiError, RsiMeta};

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("ввод-вывод {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Rsi(PathBuf, RsiError),
    #[error("{0}: у набора нет лицензии или авторства в meta.json")]
    NoLicense(PathBuf),
    #[error("{0}: нет файла состояния {1}")]
    MissingImage(PathBuf, String),
}

/// Загруженный набор RSI: путь и разобранные метаданные.
#[derive(Debug)]
pub struct RsiSet {
    pub dir: PathBuf,
    pub meta: RsiMeta,
}

/// Корень SS14-ассетов внутри репозитория билда.
pub fn ss14_textures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ss14/Textures")
}

/// Найти все каталоги `*.rsi` под `root`, по возрастанию пути.
pub fn find_rsi(root: &Path) -> Result<Vec<PathBuf>, AssetError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = std::fs::read_dir(&dir).map_err(|e| AssetError::Io(dir.clone(), e))?;
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.extension().is_some_and(|e| e == "rsi") {
                    out.push(p);
                } else {
                    stack.push(p);
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Загрузить набор: движковый разбор `meta.json` с лимитами, лицензия и авторство обязательны,
/// у каждого состояния должен быть файл `<имя>.png`.
pub fn load_rsi(dir: &Path) -> Result<RsiSet, AssetError> {
    let meta_path = dir.join("meta.json");
    let bytes = std::fs::read(&meta_path).map_err(|e| AssetError::Io(meta_path.clone(), e))?;
    let meta = RsiMeta::parse(&bytes).map_err(|e| AssetError::Rsi(dir.to_path_buf(), e))?;
    if !meta.has_license() {
        return Err(AssetError::NoLicense(dir.to_path_buf()));
    }
    for state in &meta.states {
        let file = format!("{}.png", state.name);
        if !dir.join(&file).is_file() {
            return Err(AssetError::MissingImage(dir.to_path_buf(), file));
        }
    }
    Ok(RsiSet {
        dir: dir.to_path_buf(),
        meta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_ss14_set_parses_and_carries_license_and_all_images() {
        let sets = find_rsi(&ss14_textures_dir()).unwrap();
        assert!(sets.len() >= 6, "найдено наборов: {}", sets.len());
        for dir in sets {
            let set = load_rsi(&dir).unwrap_or_else(|e| panic!("{e}"));
            assert!(!set.meta.states.is_empty(), "{}", dir.display());
        }
    }

    #[test]
    fn tools_expose_the_expected_states() {
        let crowbar = load_rsi(&ss14_textures_dir().join("Objects/Tools/crowbar.rsi")).unwrap();
        assert!(crowbar.meta.state("icon").is_some());
        assert!(crowbar.meta.state("inhand-left").is_some());
        assert!(crowbar.meta.state("нет-такого").is_none());
    }

    #[test]
    fn a_set_without_license_or_images_is_refused() {
        let dir = std::env::temp_dir().join(format!("build-assets-{}", std::process::id()));
        let rsi = dir.join("bad.rsi");
        std::fs::create_dir_all(&rsi).unwrap();
        std::fs::write(
            rsi.join("meta.json"),
            br#"{"version":1,"size":{"x":32,"y":32},"states":[{"name":"icon"}]}"#,
        )
        .unwrap();
        assert!(matches!(
            load_rsi(&rsi),
            Err(AssetError::NoLicense(_) | AssetError::Rsi(..))
        ));
        std::fs::write(
            rsi.join("meta.json"),
            br#"{"version":1,"license":"CC-BY-SA-3.0","copyright":"x","size":{"x":32,"y":32},"states":[{"name":"icon"}]}"#,
        )
        .unwrap();
        assert!(matches!(load_rsi(&rsi), Err(AssetError::MissingImage(..))));
        let _ = std::fs::remove_dir_all(dir);
    }
}
