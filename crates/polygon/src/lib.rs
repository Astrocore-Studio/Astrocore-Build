use std::path::{Path, PathBuf};

use assets::{DoorSource, MaterialSource, SpriteSource, build_manifest};
use astrocore_core::PrototypeId;
use astrocore_world::{World, WorldError};
use maps::{MapFile, MapItem};
use prototypes::{Data, PrototypeRegistry};

pub const WALL: &str = "ss14:wall_solid";
pub const FLOOR: &str = "ss14:floor_dark";
pub const DOOR: &str = "ss14:airlock";
pub const NAMESPACE: &str = "ss14";

/// Высота верхней поверхности пола, м: плита толщиной 0,2 м; на неё встают игроки и кладутся предметы.
pub const FLOOR_TOP_M: f32 = 0.2;

#[derive(Debug, thiserror::Error)]
pub enum PolygonError {
    #[error("мир: {0}")]
    World(#[from] WorldError),
    #[error("карта: {0}")]
    Map(#[from] maps::MapError),
    #[error("прототипы: {0}")]
    Prototypes(#[from] prototypes::ProtoError),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Ron(PathBuf, String),
    #[error("ассеты: {0}")]
    Assets(#[from] assets::AssetError),
    #[error("предмет {0:?} из карты не найден в прототипах")]
    UnknownItem(String),
}

/// Каталог содержимого рядом с репозиторием (прототипы и материалы).
pub fn content_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
}

/// Корень ассетов SS14 (`assets/ss14`).
pub fn assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ss14")
}

/// Загрузить и проверить прототипы из каталога содержимого.
pub fn load_registry(content: &Path) -> Result<PrototypeRegistry, PolygonError> {
    let mut r = PrototypeRegistry::new();
    r.load_dir(&content.join("prototypes"))?;
    r.validate()?;
    Ok(r)
}

/// Описания прототипов для осмотра: `content/descriptions.ron` (список пар «id SS14, текст»).
/// Нет файла или он испорчен — осмотр просто без описаний.
pub fn load_descriptions(content: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(content.join("descriptions.ron"))
        .ok()
        .and_then(|t| ron::from_str::<Vec<(String, String)>>(&t).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

/// Идентификатор мира: клиент подключается с `--world-id polygon` к любой карте.
pub const WORLD_ID: &str = "polygon";

const POLYGON_MAP: &str = include_str!("../../../content/maps/polygon.ron");

/// Карта полигона из `content/maps/polygon.ron`: две комнаты, дверь между ними, лестница на второй
/// этаж и предметы на полу.
static MAP_OVERRIDE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Подменить встроенную карту файлом (сервер: `--map`), например импортом из SS14. Вызывается до
/// первого обращения к карте.
pub fn use_map_file(path: &Path) {
    let _ = MAP_OVERRIDE.set(path.to_path_buf());
}

pub fn map() -> MapFile {
    if let Some(path) = MAP_OVERRIDE
        .get()
        .cloned()
        .or_else(|| std::env::var_os("ASTRO_MAP").map(Into::into))
    {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("карта {}: {e}", path.display()));
        return MapFile::parse(&text).expect("подменённая карта разбирается");
    }
    MapFile::parse(POLYGON_MAP).expect("встроенная карта полигона разбирается")
}

/// Мир полигона: этажи и конструкции из файла карты (предметы ставит `place_items`).
pub fn build_world() -> Result<World, PolygonError> {
    Ok(map().build_world(WORLD_ID)?)
}

/// Точка появления игроков из файла карты.
pub fn spawn() -> [f32; 3] {
    map().spawn
}

/// Размеры предмета на земле в метрах по его размеру из прототипа.
fn dropped_extent(size: &str) -> f32 {
    match size.to_ascii_lowercase().as_str() {
        "tiny" => 0.2,
        "small" => 0.3,
        "normal" => 0.4,
        _ => 0.5,
    }
}

/// Расставить предметы полигона по миру как объекты-размещения. Идентификаторы стабильны
/// (одна и та же карта — одни и те же предметы), поэтому раунды воспроизводимы.
pub fn place_items(world: &mut World, registry: &PrototypeRegistry) -> Result<usize, PolygonError> {
    place_items_from(world, registry, &map().items)
}

/// Расставить произвольный список предметов карты (идентификаторы — по порядку в списке) картой 0.
pub fn place_items_from(
    world: &mut World,
    registry: &PrototypeRegistry,
    items: &[MapItem],
) -> Result<usize, PolygonError> {
    let map = MapFile {
        items: items.to_vec(),
        ..MapFile::empty("")
    };
    let resolve = item_resolver(registry)?;
    let commands = map
        .item_commands(astrocore_world::MapId(0), resolve)
        .map_err(PolygonError::UnknownItem)?;
    let placed = commands.len();
    for c in commands {
        world.apply(c)?;
    }
    Ok(placed)
}

/// Имя прототипа Build → идентификатор движка и размер предмета на земле, м (по размеру из
/// прототипа). Нужен для предметов карты (`MapFile::item_commands`).
pub fn item_resolver(
    registry: &PrototypeRegistry,
) -> Result<impl Fn(&str) -> Option<(PrototypeId, f32)> + '_, PolygonError> {
    let index = registry.engine_index(NAMESPACE)?;
    Ok(move |name: &str| {
        let id = index.iter().find(|(_, n)| n.as_str() == name)?.0.clone();
        let size = registry
            .resolve(name)
            .ok()?
            .component("Item")
            .and_then(|c| c.get("size"))
            .and_then(Data::as_str)
            .map_or(0.3, dropped_extent);
        Some((id, size))
    })
}

/// Материалы геометрии из `content/materials.ron`.
pub fn load_materials(content: &Path) -> Result<Vec<MaterialSource>, PolygonError> {
    let path = content.join("materials.ron");
    let text = std::fs::read_to_string(&path).map_err(|e| PolygonError::Io(path.clone(), e))?;
    let mut list: Vec<MaterialSource> =
        ron::from_str(&text).map_err(|e| PolygonError::Ron(path, e.to_string()))?;
    // Плитки пола SS14 (`content/tiles.ron`) тоже материалы: `ss14:tile_<имя строчными>`.
    for t in load_tiles(content)? {
        list.push(MaterialSource {
            id: tile_material_id(&t.tile),
            image: t.image,
            tile: Some(t.tile),
        });
    }
    Ok(list)
}

/// Плитка пола SS14 и её картинка.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TileDef {
    pub tile: String,
    pub image: String,
}

/// Плитки из `content/tiles.ron`; без файла — пусто.
pub fn load_tiles(content: &Path) -> Result<Vec<TileDef>, PolygonError> {
    let path = content.join("tiles.ron");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    ron::from_str(&text).map_err(|e| PolygonError::Ron(path, e.to_string()))
}

/// Идентификатор материала плитки.
pub fn tile_material_id(tile: &str) -> String {
    format!("ss14:tile_{}", tile.to_ascii_lowercase())
}

/// Двери из `content/doors.ron`: материал проёма → набор RSI со спрайтами состояний.
pub fn load_doors(content: &Path) -> Result<Vec<DoorSource>, PolygonError> {
    let path = content.join("doors.ron");
    let text = std::fs::read_to_string(&path).map_err(|e| PolygonError::Io(path.clone(), e))?;
    ron::from_str(&text).map_err(|e| PolygonError::Ron(path, e.to_string()))
}

/// Список доступа считывателя прототипа из `content/door_access.ron`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DoorAccess {
    pub proto: String,
    pub access: Vec<Vec<String>>,
}

pub fn load_door_access(content: &Path) -> Result<Vec<DoorAccess>, PolygonError> {
    let path = content.join("door_access.ron");
    let text = std::fs::read_to_string(&path).map_err(|e| PolygonError::Io(path.clone(), e))?;
    ron::from_str(&text).map_err(|e| PolygonError::Ron(path, e.to_string()))
}

/// Прочность стены, окна или решётки из `content/wall_hp.ron`: порог разрушения и защита
/// (`Destructible` и `damageModifierSet`).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WallHp {
    pub proto: String,
    pub hp: f32,
    pub coefficients: std::collections::BTreeMap<String, f32>,
    pub flat: std::collections::BTreeMap<String, f32>,
}

pub fn load_wall_hp(content: &Path) -> Result<Vec<WallHp>, PolygonError> {
    let path = content.join("wall_hp.ron");
    let text = std::fs::read_to_string(&path).map_err(|e| PolygonError::Io(path.clone(), e))?;
    ron::from_str(&text).map_err(|e| PolygonError::Ron(path, e.to_string()))
}

/// Спрайты предметов: у каждого прототипа с `Sprite` (набор и состояние) и `Item`.
pub fn sprites_of(registry: &PrototypeRegistry) -> Result<Vec<SpriteSource>, PolygonError> {
    let mut out = Vec::new();
    for (id, name) in registry.engine_index(NAMESPACE)? {
        let Ok(r) = registry.spawnable(&name) else {
            continue;
        };
        // Предметы и декорации (например, тело умершего): у декорации нет `Item`, размер — `size`
        // из `Sprite`, по умолчанию крупный.
        let Some(sprite) = r.component("Sprite") else {
            continue;
        };
        let item = r.component("Item");
        let (Some(rsi), Some(state)) = (
            sprite.get("sprite").and_then(Data::as_str),
            sprite.get("state").and_then(Data::as_str),
        ) else {
            continue;
        };
        let volume = volume_of(&r);
        let floor_object = item.is_none()
            && (name == "Catwalk"
                || r.component("Conveyor").is_some()
                || r.component("GasPipe").is_some());
        let e = dropped_extent(
            item.and_then(|i| i.get("size"))
                .and_then(Data::as_str)
                .unwrap_or("large"),
        );
        let base = SpriteSource {
            id: id.to_string(),
            rsi: rsi.to_string(),
            state: state.to_string(),
            size: [e.max(0.25) * 1.2, e.max(0.25) * 1.2],
            // Предметы лежат на полу плашмя, декорации и устройства стоят; конвейеры и трубные
            // устройства — объекты пола: лежат, повёрнуты по карте и имеют размер своей ячейки.
            flat: volume.map_or(item.is_some() || floor_object, |v| v.flat),
            depth_px: volume.map(|v| v.height_px),
            pixel_size: item.is_none(),
            oriented: floor_object,
            name: r.name.clone(),
        };
        let states: &[&str] = match name.as_str() {
            "ComputerFrame" => &["1", "2", "3", "4"],
            "MachineFrame" => &["box_2"],
            _ => &[],
        };
        for state in states {
            out.push(SpriteSource {
                id: format!("{}_{}", base.id, state),
                state: state.to_string(),
                ..base.clone()
            });
        }
        if let Some(state) = r
            .component("VendingMachineVisuals")
            .and_then(|visuals| visuals.get("brokenState"))
            .and_then(Data::as_str)
        {
            out.push(SpriteSource {
                id: format!("{}_broken", base.id),
                state: state.to_string(),
                ..base.clone()
            });
        }
        out.push(base);
    }
    Ok(out)
}

/// Объём спрайта из компонента прототипа `SpriteVolume` (как компоненты SS14): `layout`
/// (`"Flat"` — лежит горизонтально, `"Upright"` — стоит) и `height` — высота объёма в пикселях
/// (32 пикселя на метр). Нет компонента — вид по умолчанию, объём в один пиксель.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volume {
    pub flat: bool,
    pub height_px: f32,
}

fn volume_of(r: &prototypes::Resolved) -> Option<Volume> {
    let c = r.component("SpriteVolume")?;
    let flat = !c
        .get("layout")
        .and_then(Data::as_str)
        .is_some_and(|l| l.eq_ignore_ascii_case("upright"));
    let height_px = c
        .get("height")
        .and_then(Data::as_f64)
        .map_or(1.0, |h| h as f32)
        .clamp(1.0, 64.0);
    Some(Volume { flat, height_px })
}

/// Объём прототипов, у которых он задан: имя прототипа SS14 → объём.
pub fn volumes_of(
    registry: &PrototypeRegistry,
) -> Result<std::collections::BTreeMap<String, Volume>, PolygonError> {
    let mut out = std::collections::BTreeMap::new();
    for (_, name) in registry.engine_index(NAMESPACE)? {
        if let Ok(r) = registry.spawnable(&name)
            && let Some(v) = volume_of(&r)
        {
            out.insert(name, v);
        }
    }
    Ok(out)
}

/// Манифест рендера для клиента: текстуры материалов, спрайты предметов и двери.
pub fn render_manifest(
    content: &Path,
    registry: &PrototypeRegistry,
) -> Result<String, PolygonError> {
    let materials = load_materials(content)?;
    let sprites = sprites_of(registry)?;
    let doors = load_doors(content)?;
    Ok(build_manifest(
        &assets_dir(),
        "Textures",
        &materials,
        &sprites,
        &doors,
    )?)
}

#[cfg(test)]
mod tests;
