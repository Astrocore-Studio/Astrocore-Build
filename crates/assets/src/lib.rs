use std::path::{Path, PathBuf};

use astrocore_sprites::rsi::{RsiError, RsiMeta};

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("ввод-вывод {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Rsi(PathBuf, RsiError),
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

impl RsiSet {
    /// Указана ли в `meta.json` лицензия (для отчётов; на загрузку не влияет).
    pub fn has_license(&self) -> bool {
        self.meta.has_license()
    }
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

/// Загрузить набор: движковый разбор `meta.json` с лимитами, у каждого состояния должен быть файл
/// `<имя>.png`. Отсутствие лицензии не мешает загрузке: это видно по `RsiSet::has_license`.
pub fn load_rsi(dir: &Path) -> Result<RsiSet, AssetError> {
    let meta_path = dir.join("meta.json");
    let bytes = std::fs::read(&meta_path).map_err(|e| AssetError::Io(meta_path.clone(), e))?;
    let meta = RsiMeta::parse(&bytes).map_err(|e| AssetError::Rsi(dir.to_path_buf(), e))?;
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
    #[serde(default)]
    #[allow(dead_code)]
    license: String,
    #[serde(default)]
    #[allow(dead_code)]
    copyright: String,
}

/// Проверить каталог плиток: перечисленные в `attributions.yml` файлы существуют. Возвращает число проверенных файлов.
pub fn check_tile_dir(dir: &Path) -> Result<usize, AssetError> {
    let path = dir.join("attributions.yml");
    let text = std::fs::read_to_string(&path).map_err(|e| AssetError::Io(path.clone(), e))?;
    let entries: Vec<TileAttribution> = serde_yaml::from_str(&text)
        .map_err(|e| AssetError::Attribution(path.clone(), e.to_string()))?;
    let mut count = 0;
    for e in &entries {
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
    /// Лежит ли спрайт на полу плашмя (с толщиной) вместо стоячего билборда.
    pub flat: bool,
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

/// Дверь: идентификатор материала проёма и набор RSI со состояниями closed/open/opening/closing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct DoorSource {
    pub id: String,
    pub rsi: String,
}

/// Размер PNG из заголовка (без полной загрузки картинки).
fn png_size(path: &Path) -> Result<(u32, u32), AssetError> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .map_err(|e| AssetError::Io(path.to_path_buf(), e))?;
    if &head[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(AssetError::Attribution(path.to_path_buf(), "не PNG".into()));
    }
    let w = u32::from_be_bytes([head[16], head[17], head[18], head[19]]);
    let h = u32::from_be_bytes([head[20], head[21], head[22], head[23]]);
    Ok((w, h))
}

/// Описание листа состояния: ячейки размера RSI идут по строкам (направление × кадр), берём
/// первое направление. Кадров столько, сколько задержек в `meta.json`.
fn door_sheet(
    textures: &Path,
    rsi: &str,
    meta: &RsiMeta,
    state: &str,
    textures_subdir: &str,
) -> Result<serde_json::Value, AssetError> {
    let st = meta.state(state).ok_or_else(|| {
        AssetError::MissingImage(textures.join(rsi), format!("состояние {state} в meta.json"))
    })?;
    let file = format!("{rsi}/{state}.png");
    let path = textures.join(&file);
    if !path.is_file() {
        return Err(AssetError::MissingImage(textures.to_path_buf(), file));
    }
    let (w, h) = png_size(&path)?;
    let (cw, ch) = meta.size;
    if cw == 0 || ch == 0 || w % cw != 0 || h % ch != 0 {
        return Err(AssetError::Attribution(
            path,
            "размер листа не кратен размеру ячейки".into(),
        ));
    }
    let delays: Vec<f32> = if st.is_animated() {
        st.delays.first().cloned().unwrap_or_default()
    } else {
        Vec::new()
    };
    let (cols, rows) = (w / cw, h / ch);
    if delays.len() as u32 > cols * rows {
        return Err(AssetError::Attribution(
            path,
            "кадров больше, чем ячеек".into(),
        ));
    }
    Ok(serde_json::json!({
        "image": format!("{textures_subdir}/{file}"),
        "cols": cols,
        "rows": rows,
        "delays": delays,
    }))
}

/// Манифест рендера для клиента движка (`RenderManifest`): идентификатор → PNG. Пути в нём
/// относительны каталогу манифеста (`manifest_dir`), все файлы проверяются на существование,
/// идентификаторы — на допустимость в движке.
pub fn build_manifest(
    assets_root: &Path,
    textures_subdir: &str,
    materials: &[MaterialSource],
    sprites: &[SpriteSource],
    doors: &[DoorSource],
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
            serde_json::json!({
                "image": format!("{textures_subdir}/{image}"),
                "size": s.size,
                "layout": if s.flat { "flat" } else { "billboard" },
            }),
        );
    }
    let mut door_map = serde_json::Map::new();
    for d in doors {
        check_id(&d.id)?;
        safe_rel(&d.rsi)?;
        let set = load_rsi(&textures.join(&d.rsi))?;
        let sheet = |state: &str| door_sheet(&textures, &d.rsi, &set.meta, state, textures_subdir);
        door_map.insert(
            d.id.clone(),
            serde_json::json!({
                "closed": sheet("closed")?,
                "open": sheet("open")?,
                "opening": sheet("opening")?,
                "closing": sheet("closing")?,
            }),
        );
    }
    serde_json::to_string_pretty(
        &serde_json::json!({ "materials": mats, "sprites": sprs, "doors": door_map }),
    )
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
            flat: false,
        };
        let json = build_manifest(
            &root,
            "Textures",
            &mats,
            &[sprite("ss14:crowbar", "Objects/Tools/crowbar.rsi", "icon")],
            &[],
        )
        .unwrap();
        assert!(json.contains("Textures/Objects/Tools/crowbar.rsi/icon.png"));
        assert!(json.contains("Textures/Structures/Walls/solid.rsi/full.png"));
        let bad = |s: SpriteSource| build_manifest(&root, "Textures", &[], &[s], &[]).is_err();
        assert!(bad(sprite(
            "ss14:crowbar",
            "Objects/Tools/crowbar.rsi",
            "no_such_state"
        )));
        assert!(bad(sprite("Плохо", "Objects/Tools/crowbar.rsi", "icon")));
        assert!(bad(sprite("ss14:x", "../outside.rsi", "icon")));
    }

    #[test]
    fn a_door_manifest_reads_frame_grids_and_delays_from_the_rsi() {
        let root = ss14_textures_dir().join("..");
        let rsi = "Structures/Doors/Airlocks/Standard/airlock-base.rsi";
        let door = DoorSource {
            id: "ss14:airlock".into(),
            rsi: rsi.into(),
        };
        let json =
            build_manifest(&root, "Textures", &[], &[], std::slice::from_ref(&door)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let d = &v["doors"]["ss14:airlock"];
        // Открытие: лист 288×128 из ячеек 32×32 — 9 кадров в 4 строки (по направлению на строку).
        assert_eq!(d["opening"]["cols"], 9);
        assert_eq!(d["opening"]["rows"], 4);
        assert_eq!(d["opening"]["delays"].as_array().unwrap().len(), 9);
        assert_eq!(d["closed"]["cols"], 2, "статичный лист 64×64: 2×2 ячейки");
        assert_eq!(d["closed"]["delays"].as_array().unwrap().len(), 0);
        // Дверь без нужных состояний или с неверным набором — ошибка.
        let wrong = DoorSource {
            id: "ss14:x".into(),
            rsi: "Objects/Tools/crowbar.rsi".into(),
        };
        assert!(build_manifest(&root, "Textures", &[], &[], &[wrong]).is_err());
        let escape = DoorSource {
            id: "ss14:x".into(),
            rsi: "../x.rsi".into(),
        };
        assert!(build_manifest(&root, "Textures", &[], &[], &[escape]).is_err());
    }

    #[test]
    fn tools_expose_the_expected_states() {
        let crowbar = load_rsi(&ss14_textures_dir().join("Objects/Tools/crowbar.rsi")).unwrap();
        assert!(crowbar.meta.state("icon").is_some());
        assert!(crowbar.meta.state("inhand-left").is_some());
        assert!(crowbar.meta.state("нет-такого").is_none());
    }

    #[test]
    fn a_set_without_license_loads_but_one_without_images_is_refused() {
        let dir = std::env::temp_dir().join(format!("assets-{}", std::process::id()));
        let rsi = dir.join("bad.rsi");
        std::fs::create_dir_all(&rsi).unwrap();
        std::fs::write(
            rsi.join("meta.json"),
            br#"{"version":1,"size":{"x":32,"y":32},"states":[{"name":"icon"}]}"#,
        )
        .unwrap();
        assert!(matches!(load_rsi(&rsi), Err(AssetError::MissingImage(..))));
        std::fs::write(rsi.join("icon.png"), b"png").unwrap();
        assert!(matches!(load_rsi(&rsi), Ok(set) if !set.has_license()));
        std::fs::remove_file(rsi.join("icon.png")).unwrap();
        std::fs::write(
            rsi.join("meta.json"),
            br#"{"version":1,"license":"CC-BY-SA-3.0","copyright":"x","size":{"x":32,"y":32},"states":[{"name":"icon"}]}"#,
        )
        .unwrap();
        assert!(matches!(load_rsi(&rsi), Err(AssetError::MissingImage(..))));
        let _ = std::fs::remove_dir_all(dir);
    }
}
