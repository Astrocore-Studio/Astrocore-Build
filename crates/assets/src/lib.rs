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
    #[error("{0}: {1}")]
    Attribution(PathBuf, String),
    #[error("неверный идентификатор {0:?}")]
    BadId(String),
    #[error("небезопасный путь {0:?}")]
    BadPath(String),
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

/// Запись `attributions.yml` каталога плиток SS14.
#[derive(Debug, serde::Deserialize)]
struct TileAttribution {
    files: Vec<String>,
    license: String,
    copyright: String,
}

/// Проверить каталог плиток: у каждой записи есть лицензия и авторство, перечисленные файлы
/// существуют. Возвращает число проверенных файлов.
pub fn check_tile_dir(dir: &Path) -> Result<usize, AssetError> {
    let path = dir.join("attributions.yml");
    let text = std::fs::read_to_string(&path).map_err(|e| AssetError::Io(path.clone(), e))?;
    let entries: Vec<TileAttribution> = serde_yaml::from_str(&text)
        .map_err(|e| AssetError::Attribution(path.clone(), e.to_string()))?;
    let mut count = 0;
    for e in &entries {
        if e.license.trim().is_empty() || e.copyright.trim().is_empty() {
            return Err(AssetError::NoLicense(path));
        }
        // Каталог содержит подмножество файлов SS14: проверяем только те, что скопированы.
        for f in &e.files {
            if dir.join(f).is_file() {
                count += 1;
            }
        }
    }
    if count == 0 {
        return Err(AssetError::Attribution(
            path,
            "нет ни одного перечисленного файла".into(),
        ));
    }
    Ok(count)
}

/// Материал геометрии: идентификатор движка и картинка (путь относительно `Textures/`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct MaterialSource {
    pub id: String,
    pub image: String,
}

/// Спрайт предмета: набор RSI, состояние и размер в метрах.
#[derive(Debug, Clone, PartialEq)]
pub struct SpriteSource {
    pub id: String,
    pub rsi: String,
    pub state: String,
    pub size: [f32; 2],
}

fn safe_rel(p: &str) -> Result<(), AssetError> {
    let ok = !p.is_empty()
        && Path::new(p)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    if ok {
        Ok(())
    } else {
        Err(AssetError::BadPath(p.to_string()))
    }
}

/// Манифест рендера для клиента движка (`RenderManifest`): идентификатор → PNG. Пути в нём
/// относительны каталогу манифеста (`manifest_dir`), все файлы проверяются на существование,
/// идентификаторы — на допустимость в движке.
pub fn build_manifest(
    assets_root: &Path,
    textures_subdir: &str,
    materials: &[MaterialSource],
    sprites: &[SpriteSource],
) -> Result<String, AssetError> {
    let textures = assets_root.join(textures_subdir);
    let check_id = |id: &str| {
        astrocore_core::PrototypeId::new(id).map_err(|_| AssetError::BadId(id.to_string()))
    };
    let mut mats = serde_json::Map::new();
    for m in materials {
        check_id(&m.id)?;
        safe_rel(&m.image)?;
        if !textures.join(&m.image).is_file() {
            return Err(AssetError::MissingImage(textures.clone(), m.image.clone()));
        }
        mats.insert(
            m.id.clone(),
            serde_json::json!({ "image": format!("{textures_subdir}/{}", m.image) }),
        );
    }
    let mut sprs = serde_json::Map::new();
    for s in sprites {
        check_id(&s.id)?;
        safe_rel(&s.rsi)?;
        let image = format!("{}/{}.png", s.rsi, s.state);
        safe_rel(&image)?;
        if !textures.join(&image).is_file() {
            return Err(AssetError::MissingImage(textures.clone(), image));
        }
        sprs.insert(
            s.id.clone(),
            serde_json::json!({ "image": format!("{textures_subdir}/{image}"), "size": s.size }),
        );
    }
    serde_json::to_string_pretty(&serde_json::json!({ "materials": mats, "sprites": sprs }))
        .map_err(|e| AssetError::Attribution(textures, e.to_string()))
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
    fn dark_floor_tile_is_attributed() {
        let dir = ss14_textures_dir().join("Tiles/Basic/Dark");
        assert!(check_tile_dir(&dir).unwrap() >= 1);
        assert!(
            check_tile_dir(&ss14_textures_dir()).is_err(),
            "без attributions.yml — ошибка"
        );
    }

    #[test]
    fn manifest_lists_existing_files_and_refuses_everything_else() {
        let root = ss14_textures_dir().join("..");
        let mats = [MaterialSource {
            id: "ss14:wall_solid".into(),
            image: "Structures/Walls/solid.rsi/full.png".into(),
        }];
        let sprite = |id: &str, rsi: &str, state: &str| SpriteSource {
            id: id.into(),
            rsi: rsi.into(),
            state: state.into(),
            size: [0.4, 0.4],
        };
        let json = build_manifest(
            &root,
            "Textures",
            &mats,
            &[sprite("ss14:crowbar", "Objects/Tools/crowbar.rsi", "icon")],
        )
        .unwrap();
        assert!(json.contains("Textures/Objects/Tools/crowbar.rsi/icon.png"));
        assert!(json.contains("Textures/Structures/Walls/solid.rsi/full.png"));
        let bad = |s: SpriteSource| build_manifest(&root, "Textures", &[], &[s]).is_err();
        assert!(bad(sprite(
            "ss14:crowbar",
            "Objects/Tools/crowbar.rsi",
            "no_such_state"
        )));
        assert!(bad(sprite("Плохо", "Objects/Tools/crowbar.rsi", "icon")));
        assert!(bad(sprite("ss14:x", "../outside.rsi", "icon")));
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
        let dir = std::env::temp_dir().join(format!("assets-{}", std::process::id()));
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
