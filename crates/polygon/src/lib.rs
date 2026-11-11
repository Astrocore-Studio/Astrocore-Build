use std::path::{Path, PathBuf};

use assets::{DoorSource, MaterialSource, SpriteSource, build_manifest};
use astrocore_core::{ObjectId, PrototypeId};
use astrocore_editor::tools::{self, SequentialIds};
use astrocore_math::glam::Vec3;
use astrocore_math::{Facing, GridCell, GridEdge};
use astrocore_world::{
    AuthoredKind, AuthoredObject, FloorId, OpeningKind, Placement, StructureElement, StructureKind,
    World, WorldCommand, WorldConfig, WorldError,
};
use prototypes::{Data, PrototypeRegistry};

pub const WALL: &str = "ss14:wall_solid";
pub const FLOOR: &str = "ss14:floor_dark";
pub const DOOR: &str = "ss14:airlock";
pub const NAMESPACE: &str = "ss14";

/// Точка появления игроков: комната A, у южной стены.
/// Высота верхней поверхности пола, м: плита толщиной 0,2 м; на неё встают игроки и кладутся предметы.
pub const FLOOR_TOP_M: f32 = 0.2;

pub const SPAWN: [f32; 3] = [2.5, FLOOR_TOP_M + 0.05, 4.5];

#[derive(Debug, thiserror::Error)]
pub enum PolygonError {
    #[error("мир: {0}")]
    World(#[from] WorldError),
    #[error("инструмент карты: {0}")]
    Tool(#[from] tools::ToolError),
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

fn pid(s: &str) -> PrototypeId {
    PrototypeId::new(s).expect("встроенный идентификатор валиден")
}

/// Загрузить и проверить прототипы из каталога содержимого.
pub fn load_registry(content: &Path) -> Result<PrototypeRegistry, PolygonError> {
    let mut r = PrototypeRegistry::new();
    r.load_dir(&content.join("prototypes"))?;
    r.validate()?;
    Ok(r)
}

/// Предметы, разложенные на карте: (имя прототипа, клетка x, z, смещение внутри клетки).
pub const ITEM_SPAWNS: &[(&str, f32, f32)] = &[
    ("Crowbar", 1.5, 1.5),
    ("Wrench", 2.5, 1.5),
    ("Screwdriver", 3.5, 1.5),
    ("Pen", 1.5, 3.5),
    ("Box", 4.0, 3.0),
    ("ClothingBackpack", 3.0, 3.5),
    // Предмет в другой комнате: за дверью.
    ("Pen", 8.5, 2.5),
];

/// Строит мир полигона: два этажа, комнаты A и B с дверью между ними и лестницей наверх.
///
/// Этаж 0: комната A (клетки 0..5 × 0..5) и комната B (6..11 × 0..5), дверь в общей стене.
/// Этаж 1: комната над B, лестница из B ведёт к ней.
pub fn build_world() -> Result<World, PolygonError> {
    let mut world = World::new("polygon", WorldConfig::default())?;
    let mut ids = SequentialIds(0x9000_0000);
    let wall = pid(WALL);
    let floor = pid(FLOOR);
    let door = pid(DOOR);

    let (floor0, floor1) = (FloorId(0), FloorId(1));
    world.apply(WorldCommand::DefineFloor(astrocore_world::FloorDef {
        id: floor0,
        name: "Этаж 0".into(),
        base_y: 0.0,
        height: 3.0,
    }))?;
    let (_, define_up) = tools::add_floor_above(&world, "Этаж 1", 3.0);
    world.apply(define_up)?;

    // Стены обеих комнат; общее ребро между ними занимает первая комната, повтор пропускается.
    for (a, b) in [((0, 0), (5, 5)), ((6, 0), (11, 5))] {
        let cmds = tools::room_rect(
            &world,
            &mut ids,
            floor0,
            GridCell::new(a.0, a.1),
            GridCell::new(b.0, b.1),
            &wall,
            0.2,
            false,
            false,
        )?;
        for c in cmds {
            match world.apply(c) {
                Ok(_) | Err(WorldError::SlotOccupied { .. }) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    // Дверь: стена на общем ребре заменяется проёмом.
    let door_edge = GridEdge::along_z(6, 2);
    if let Some(w) = world.edge_element(floor0, door_edge).map(|e| e.id) {
        world.apply(WorldCommand::RemoveElement { id: w })?;
    }
    world.apply(WorldCommand::PlaceElement(StructureElement {
        id: ids_next(&mut ids),
        kind: StructureKind::Opening(OpeningKind::Door),
        placement: Placement::Edge(door_edge),
        floor: floor0,
        material: door,
        thickness: 0.1,
    }))?;
    // Полы по контурам комнат.
    for c in tools::auto_slabs(&world, &mut ids, floor0, StructureKind::Floor, &floor, 0.2)? {
        world.apply(c)?;
    }
    // Верхний этаж над комнатой B и лестница к нему.
    for c in tools::room_rect(
        &world,
        &mut ids,
        floor1,
        GridCell::new(6, 0),
        GridCell::new(11, 5),
        &wall,
        0.2,
        false,
        false,
    )? {
        world.apply(c)?;
    }
    for c in tools::auto_slabs(&world, &mut ids, floor1, StructureKind::Floor, &floor, 0.2)? {
        match world.apply(c) {
            Ok(_) | Err(WorldError::SlotOccupied { .. }) => {}
            Err(e) => return Err(e.into()),
        }
    }
    for c in tools::stair_between(
        &world,
        &mut ids,
        floor0,
        floor1,
        GridCell::new(7, 3),
        Facing::East,
        3,
        false,
        &floor,
        true,
    )? {
        world.apply(c)?;
    }
    Ok(world)
}

fn ids_next(ids: &mut SequentialIds) -> ObjectId {
    use astrocore_editor::tools::IdGen;
    ids.next_id()
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
    let index = registry.engine_index(NAMESPACE)?;
    let mut placed = 0;
    for (i, (name, x, z)) in ITEM_SPAWNS.iter().enumerate() {
        let id = index
            .iter()
            .find(|(_, n)| n.as_str() == *name)
            .map(|(id, _)| id.clone())
            .ok_or_else(|| PolygonError::UnknownItem((*name).to_string()))?;
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
