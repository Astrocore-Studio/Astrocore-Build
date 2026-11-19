//! Карта как данные: этажи, элементы конструкции, предметы на земле и точка появления.
//! Хранится в RON; игра строит из неё мир движка, импортёр SS14 пишет в тот же формат.

use std::path::{Path, PathBuf};

use astrocore_core::{ObjectId, PrototypeId};
use astrocore_math::GridCell;
use astrocore_math::glam::Vec3;
use astrocore_world::{
    AuthoredKind, AuthoredObject, FloorDef, FloorId, Placement, StructureElement, StructureKind,
    World, WorldCommand, WorldConfig, WorldError,
};
use serde::{Deserialize, Serialize};

/// Предмет на земле: имя прототипа Build (`Crowbar`) и точка на полу, м.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapItem {
    pub prototype: String,
    pub x: f32,
    pub z: f32,
    /// Поворот в четвертях оборота против часовой стрелки (как у сущностей SS14), 0…3.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rot: u8,
    /// Клетки устройств, связанных с этим объектом (`DeviceList` сигнализации SS14).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<[i32; 2]>,
    /// Дополнительные метаданные объекта (например, `locked` у шкафа).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub meta: std::collections::BTreeMap<String, String>,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

/// Область карты с метаданными (например, `atmosphere = vacuum`): границы в метрах.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapZone {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub floor: i32,
    pub metadata: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapFile {
    pub name: String,
    /// Точка появления игроков (ноги), м.
    pub spawn: [f32; 3],
    /// Уровни с нестандартной высотой. Остальные уровни неявные (ADR-0003): уровень, на котором
    /// стоит хоть одна конструкция, создаётся сам — высотой [`LEVEL_HEIGHT_M`], над нижним.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub floors: Vec<FloorDef>,
    /// Стены, проёмы, полы и лестницы; применяются в порядке списка.
    pub elements: Vec<StructureElement>,
    #[serde(default)]
    pub items: Vec<MapItem>,
    #[serde(default)]
    pub zones: Vec<MapZone>,
}

/// Высота неявного уровня, м: пол 0,2 + два куба стены по 1 м + потолок 0,1 (ADR-0003).
pub const LEVEL_HEIGHT_M: f32 = 2.3;

/// Высота пола над основанием уровня, м: верх плиты пола, на нём стоят ноги и предметы.
pub const FLOOR_TOP_M: f32 = 0.2;

#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error("мир: {0}")]
    World(#[from] WorldError),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("карта: {0}")]
    Ron(String),
}

impl MapFile {
    pub fn parse(text: &str) -> Result<Self, MapError> {
        ron::from_str(text).map_err(|e| MapError::Ron(e.to_string()))
    }

    pub fn load(path: &Path) -> Result<Self, MapError> {
        let text =
            std::fs::read_to_string(path).map_err(|e| MapError::Io(path.to_path_buf(), e))?;
        Self::parse(&text)
    }

    pub fn to_ron(&self) -> Result<String, MapError> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default().depth_limit(3))
            .map_err(|e| MapError::Ron(e.to_string()))
    }

    pub fn save(&self, path: &Path) -> Result<(), MapError> {
        std::fs::write(path, self.to_ron()?).map_err(|e| MapError::Io(path.to_path_buf(), e))
    }

    /// Пустая карта для маппинга: площадка 3 × 3 из пола под точкой появления, чтобы было на чём
    /// стоять. Остальное строит маппер.
    pub fn empty(name: &str) -> Self {
        let elements = (-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| (x, z)))
            .enumerate()
            .map(|(i, (x, z))| StructureElement {
                id: ObjectId::from_u128(0x5EED_0000 + i as u128),
                kind: StructureKind::Floor,
                placement: Placement::Cell(GridCell::new(x, z)),
                floor: FloorId(0),
                material: PrototypeId::try_from("ss14:tile_plating".to_string())
                    .expect("имя материала корректно"),
                thickness: FLOOR_TOP_M,
            })
            .collect();
        Self {
            name: name.into(),
            spawn: [0.5, FLOOR_TOP_M + 0.05, 0.5],
            floors: Vec::new(),
            elements,
            items: Vec::new(),
            zones: Vec::new(),
        }
    }

    /// Все уровни карты: заданные явно и неявные — те, на которых что-то стоит (и уровень 0, где
    /// точка появления). Неявный уровень стоит над нижним (под верхним, если номер отрицательный).
    pub fn levels(&self) -> Vec<FloorDef> {
        let mut used: std::collections::BTreeSet<i32> = [0].into();
        for e in &self.elements {
            used.insert(e.floor.0);
            if let StructureKind::Stair { to } | StructureKind::Ramp { to } = e.kind {
                used.insert(to.0);
            }
        }
        used.extend(self.zones.iter().map(|z| z.floor));
        used.extend(self.floors.iter().map(|f| f.id.0));
        let (lo, hi) = (*used.first().unwrap_or(&0), *used.last().unwrap_or(&0));
        let explicit = |id: i32| self.floors.iter().find(|f| f.id.0 == id).cloned();
        let mut all: std::collections::BTreeMap<i32, FloorDef> = std::collections::BTreeMap::new();
        let level0 = explicit(0).unwrap_or_else(|| implicit_level(0, 0.0));
        all.insert(0, level0.clone());
        let mut below = level0.clone();
        for id in 1..=hi {
            let f = explicit(id).unwrap_or_else(|| implicit_level(id, below.top_y()));
            below = f.clone();
            all.insert(id, f);
        }
        let mut above = level0;
        for id in (lo..0).rev() {
            let f =
                explicit(id).unwrap_or_else(|| implicit_level(id, above.base_y - LEVEL_HEIGHT_M));
            above = f.clone();
            all.insert(id, f);
        }
        all.into_iter()
            .filter(|(id, _)| used.contains(id))
            .map(|(_, f)| f)
            .collect()
    }

    /// Мир движка из уровней и элементов карты (без предметов: их ставит игра).
    pub fn build_world(&self, world_id: &str) -> Result<World, MapError> {
        let mut world = World::new(world_id, WorldConfig::default())?;
        for f in &self.levels() {
            world.apply(WorldCommand::DefineFloor(f.clone()))?;
        }
        for e in &self.elements {
            world.apply(WorldCommand::PlaceElement(e.clone()))?;
        }
        for (i, z) in self.zones.iter().enumerate() {
            let mut obj = AuthoredObject::new(
                ObjectId::from_u128(0x2000_0000 + i as u128),
                AuthoredKind::Zone,
                FloorId(z.floor),
                Vec3::from_array(z.min),
                Vec3::from_array(z.max),
            );
            obj.metadata = z.metadata.clone();
            world.apply(WorldCommand::PlaceAuthored(obj))?;
        }
        Ok(world)
    }

    /// Снимок мира как карты: этажи, элементы, предметы-размещения и зоны (по возрастанию
    /// идентификатора, чтобы файл не менялся без причины). `name_of` даёт имя прототипа Build по
    /// идентификатору движка; размещения без имени пропускаются и возвращаются числом.
    pub fn from_world(
        name: &str,
        spawn: [f32; 3],
        world: &World,
        name_of: impl Fn(&PrototypeId) -> Option<String>,
    ) -> (Self, usize) {
        let mut elements: Vec<StructureElement> = world.elements().cloned().collect();
        elements.sort_by_key(|e| e.id);
        let mut floors: Vec<FloorDef> = world.floors().cloned().collect();
        let mut objects: Vec<&AuthoredObject> = world.authored().collect();
        objects.sort_by_key(|o| o.id);
        let (mut items, mut zones, mut skipped) = (Vec::new(), Vec::new(), 0);
        for o in objects {
            match &o.kind {
                AuthoredKind::Placement { prototype } => match name_of(prototype) {
                    Some(prototype) => items.push(item_of(o, prototype)),
                    None => skipped += 1,
                },
                AuthoredKind::Zone => zones.push(MapZone {
                    min: o.min().to_array(),
                    max: o.max().to_array(),
                    floor: o.floor.0,
                    metadata: o.metadata.clone(),
                }),
                _ => {}
            }
        }
        let mut map = Self {
            name: name.into(),
            spawn,
            floors: Vec::new(),
            elements,
            items,
            zones,
        };
        // В файл попадают только уровни, которые не вывести заново (другая высота или имя).
        // Уровень выводится из нижних (отрицательные — из верхних), поэтому сравниваем по порядку
        // от уровня 0 наружу, учитывая уже оставленные.
        floors.sort_by_key(|f| (f.id.0 < 0, f.id.0.abs()));
        for f in floors {
            if map.levels().iter().find(|i| i.id == f.id) != Some(&f) {
                map.floors.push(f);
            }
        }
        map.floors.sort_by_key(|f| f.id.0);
        (map, skipped)
    }
}

/// Неявный уровень `id` с основанием на высоте `base_y`.
fn implicit_level(id: i32, base_y: f32) -> FloorDef {
    FloorDef {
        id: FloorId(id),
        name: format!("Уровень {id}"),
        base_y,
        height: LEVEL_HEIGHT_M,
    }
}

/// Предмет карты из размещения: центр по X/Z, поворот и связи — из своих ключей метаданных,
/// остальные метаданные (`locked`, `bolted`, …) переходят в `meta` как есть.
fn item_of(o: &AuthoredObject, prototype: String) -> MapItem {
    let mut meta = o.metadata.clone();
    let rot = meta
        .remove("rot")
        .and_then(|r| r.parse::<u8>().ok())
        .map_or(0, |r| r % 4);
    let links = meta
        .remove("links")
        .map(|text| {
            text.split(';')
                .filter_map(|pair| {
                    let (x, z) = pair.split_once(',')?;
                    Some([x.trim().parse().ok()?, z.trim().parse().ok()?])
                })
                .collect()
        })
        .unwrap_or_default();
    MapItem {
        prototype,
        x: (o.min().x + o.max().x) * 0.5,
        z: (o.min().z + o.max().z) * 0.5,
        rot,
        links,
        meta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = r#"(
        name: "t",
        spawn: (0.5, 0.2, 0.5),
        floors: [(id: 0, name: "Этаж 0", base_y: 0.0, height: 2.3)],
        elements: [(
            id: "00000000-0000-0000-0000-000000000001",
            kind: Floor,
            placement: Cell((x: 0, z: 0)),
            floor: 0,
            material: "ss14:tile_steel",
            thickness: 0.2,
        )],
        zones: [(min: (0.0, 0.0, 0.0), max: (1.0, 2.0, 1.0), floor: 0, metadata: {"atmosphere": "vacuum"})],
    )"#;

    #[test]
    fn snapshot_keeps_structure_items_and_zones() {
        let map = MapFile::parse(MAP).unwrap();
        let mut world = map.build_world("t").unwrap();
        let mut obj = AuthoredObject::new(
            ObjectId::from_u128(0x1000_0000),
            AuthoredKind::Placement {
                prototype: PrototypeId::try_from("build:locker".to_string()).unwrap(),
            },
            FloorId(0),
            Vec3::new(0.25, 0.2, 0.25),
            Vec3::new(0.75, 0.7, 0.75),
        );
        obj.metadata.insert("rot".into(), "3".into());
        obj.metadata.insert("links".into(), "1,2;-3,4".into());
        obj.metadata.insert("locked".into(), "true".into());
        world.apply(WorldCommand::PlaceAuthored(obj)).unwrap();
        let unknown = AuthoredObject::new(
            ObjectId::from_u128(0x1000_0001),
            AuthoredKind::Placement {
                prototype: PrototypeId::try_from("build:ghost".to_string()).unwrap(),
            },
            FloorId(0),
            Vec3::ZERO,
            Vec3::ONE,
        );
        world.apply(WorldCommand::PlaceAuthored(unknown)).unwrap();
        let (saved, skipped) = MapFile::from_world("t", map.spawn, &world, |id| {
            (id.as_str() == "build:locker").then(|| "Locker".to_string())
        });
        assert_eq!(skipped, 1);
        assert_eq!(saved.elements, map.elements);
        assert_eq!(saved.zones, map.zones);
        assert_eq!(
            saved.items,
            vec![MapItem {
                prototype: "Locker".into(),
                x: 0.5,
                z: 0.5,
                rot: 3,
                links: vec![[1, 2], [-3, 4]],
                meta: [("locked".to_string(), "true".to_string())].into(),
            }]
        );
        // Сохранённый файл снова разбирается в ту же карту.
        assert_eq!(MapFile::parse(&saved.to_ron().unwrap()).unwrap(), saved);
    }

    #[test]
    fn levels_appear_by_themselves_from_what_stands_on_them() {
        // Без списка этажей: пол на уровне 2 — уровень 2 стоит над (неявным) уровнем 1.
        let mut map = MapFile::empty("t");
        let mut upper = map.elements[0].clone();
        upper.id = ObjectId::from_u128(77);
        upper.floor = FloorId(2);
        map.elements.push(upper);
        let levels = map.levels();
        let ids: Vec<i32> = levels.iter().map(|f| f.id.0).collect();
        assert_eq!(ids, vec![0, 2], "пустые уровни не создаются");
        assert!((levels[1].base_y - 2.0 * LEVEL_HEIGHT_M).abs() < 1e-5);
        // Явный высокий уровень 0 сдвигает всё, что выше.
        map.floors.push(FloorDef {
            id: FloorId(0),
            name: "Холл".into(),
            base_y: 0.0,
            height: 4.0,
        });
        let base2 = map.levels()[1].base_y;
        assert!((base2 - (4.0 + LEVEL_HEIGHT_M)).abs() < 1e-5, "{base2}");
        // Мир строится, а сохранение оставляет только нестандартный уровень.
        let world = map.build_world("t").unwrap();
        let (saved, _) = MapFile::from_world("t", map.spawn, &world, |_| None);
        assert_eq!(saved.floors.len(), 1);
        assert_eq!(saved.floors[0].name, "Холл");
        assert_eq!(saved.levels(), map.levels());
    }

    #[test]
    fn an_empty_map_has_ground_under_the_spawn() {
        let map = MapFile::empty("new");
        let world = map.build_world("new").unwrap();
        assert_eq!(world.floors().count(), 1);
        assert_eq!(map.elements.len(), 9);
        let text = map.to_ron().unwrap();
        assert!(
            !text.contains("floors"),
            "неявные уровни не пишутся: {text}"
        );
        assert_eq!(MapFile::parse(&text).unwrap(), map);
    }
}
