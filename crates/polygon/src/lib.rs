use std::path::{Path, PathBuf};

use assets::{DoorSource, MaterialSource, SpriteSource, build_manifest};
use astrocore_core::ObjectId;
use astrocore_math::glam::Vec3;
use astrocore_world::{AuthoredKind, AuthoredObject, FloorId, World, WorldCommand, WorldError};
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

const POLYGON_MAP: &str = include_str!("../../../content/maps/polygon.ron");

/// Карта полигона из `content/maps/polygon.ron`: две комнаты, дверь между ними, лестница на второй
/// этаж и предметы на полу.
pub fn map() -> MapFile {
    MapFile::parse(POLYGON_MAP).expect("встроенная карта полигона разбирается")
}

/// Мир полигона: этажи и конструкции из файла карты (предметы ставит `place_items`).
pub fn build_world() -> Result<World, PolygonError> {
    Ok(map().build_world("polygon")?)
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

/// Расставить произвольный список предметов карты (идентификаторы — по порядку в списке).
pub fn place_items_from(
    world: &mut World,
    registry: &PrototypeRegistry,
    items: &[MapItem],
) -> Result<usize, PolygonError> {
    let index = registry.engine_index(NAMESPACE)?;
    let mut placed = 0;
    for (
        i,
        MapItem {
            prototype: name,
            x,
            z,
        },
    ) in items.iter().enumerate()
    {
        let id = index
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(id, _)| id.clone())
            .ok_or_else(|| PolygonError::UnknownItem(name.clone()))?;
        let size = registry
            .resolve(name)?
            .component("Item")
            .and_then(|c| c.get("size"))
            .and_then(Data::as_str)
            .map_or(0.3, dropped_extent);
        let h = size * 0.5;
        let obj = AuthoredObject::new(
            ObjectId::from_u128(0x1000_0000 + i as u128),
            AuthoredKind::Placement { prototype: id },
            FloorId(0),
            Vec3::new(x - h, FLOOR_TOP_M, z - h),
            Vec3::new(x + h, FLOOR_TOP_M + size, z + h),
        );
        world.apply(WorldCommand::PlaceAuthored(obj))?;
        placed += 1;
    }
    Ok(placed)
}

/// Материалы геометрии из `content/materials.ron`.
pub fn load_materials(content: &Path) -> Result<Vec<MaterialSource>, PolygonError> {
    let path = content.join("materials.ron");
    let text = std::fs::read_to_string(&path).map_err(|e| PolygonError::Io(path.clone(), e))?;
    ron::from_str(&text).map_err(|e| PolygonError::Ron(path, e.to_string()))
}

/// Двери из `content/doors.ron`: материал проёма → набор RSI со спрайтами состояний.
pub fn load_doors(content: &Path) -> Result<Vec<DoorSource>, PolygonError> {
    let path = content.join("doors.ron");
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
        let (Some(item), Some(sprite)) = (r.component("Item"), r.component("Sprite")) else {
            continue;
        };
        let (Some(rsi), Some(state)) = (
            sprite.get("sprite").and_then(Data::as_str),
            sprite.get("state").and_then(Data::as_str),
        ) else {
            continue;
        };
        let e = dropped_extent(item.get("size").and_then(Data::as_str).unwrap_or("small"));
        out.push(SpriteSource {
            id: id.to_string(),
            rsi: rsi.to_string(),
            state: state.to_string(),
            size: [e.max(0.25) * 1.2, e.max(0.25) * 1.2],
        });
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
