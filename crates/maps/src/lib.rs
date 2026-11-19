//! Карта как данные: этажи, элементы конструкции, предметы на земле и точка появления.
//! Хранится в RON; игра строит из неё мир движка, импортёр SS14 пишет в тот же формат.

use std::path::{Path, PathBuf};

use astrocore_core::{ObjectId, PrototypeId};
use astrocore_math::GridCell;
use astrocore_math::glam::Vec3;
use astrocore_world::{
    AuthoredKind, AuthoredObject, FloorDef, FloorId, MapDef, MapId, Placement, StructureElement,
    StructureKind, World, WorldCommand, WorldConfig, WorldError,
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

/// Этажей на карту в номере этажа мира: этаж уровня `n` карты `k` — `k × 100 + n`
/// (уровни −50…49, ADR-0012 движка).
pub const MAP_FLOOR_STRIDE: i32 = 100;
/// Расстояние по высоте между картами мира, м: карты не касаются ни физикой, ни видимостью.
pub const MAP_SPACING_M: f32 = 1000.0;

/// Этаж мира для уровня `level` карты `map`.
pub fn floor_id(map: MapId, level: i32) -> FloorId {
    FloorId(map.0 as i32 * MAP_FLOOR_STRIDE + level)
}

/// Карта и уровень этажа мира (обратное к [`floor_id`]).
pub fn level_of(floor: FloorId) -> (MapId, i32) {
    let half = MAP_FLOOR_STRIDE / 2;
    let map = (floor.0 + half).div_euclid(MAP_FLOOR_STRIDE);
    (MapId(map.max(0) as u32), floor.0 - map * MAP_FLOOR_STRIDE)
}

/// Карта, к которой относится высота мира `y` (полоса ±500 м вокруг начала карты).
pub fn map_at_y(y: f32) -> MapId {
    MapId(((y + MAP_SPACING_M * 0.5) / MAP_SPACING_M).floor().max(0.0) as u32)
}

/// Высота начала карты в мире, м.
pub fn map_base_y(map: MapId) -> f32 {
    map.0 as f32 * MAP_SPACING_M
}

/// Идентификатор объекта карты в мире: одна и та же карта, загруженная под разными номерами,
/// получает разные идентификаторы (старшие 32 бита — номер карты). Для карты 0 — без изменений;
/// обратное преобразование — то же самое.
pub fn retag(id: ObjectId, map: MapId) -> ObjectId {
    ObjectId::from_u128(id.as_uuid().as_u128() ^ (u128::from(map.0) << 96))
}

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

    /// Мир движка из уровней и элементов карты (без предметов: их ставит игра). Карта ложится
    /// картой 0 мира.
    pub fn build_world(&self, world_id: &str) -> Result<World, MapError> {
        let mut world = World::new(world_id, WorldConfig::default())?;
        for c in self.structure_commands(MapId(0)) {
            world.apply(c)?;
        }
        Ok(world)
    }

    /// Команды мира, которые кладут конструкцию карты (уровни, элементы, зоны) картой `map`:
    /// этажи уровней — [`floor_id`], высоты сдвинуты на [`map_base_y`], идентификаторы —
    /// [`retag`]. Определение самой карты (`DefineMap`) — забота вызывающего.
    pub fn structure_commands(&self, map: MapId) -> Vec<WorldCommand> {
        let dy = map_base_y(map);
        let floor = |f: FloorId| floor_id(map, f.0);
        let mut out = Vec::new();
        for f in self.levels() {
            out.push(WorldCommand::DefineFloor(FloorDef {
                id: floor(f.id),
                base_y: f.base_y + dy,
                map,
                ..f
            }));
        }
        for e in &self.elements {
            let mut e = e.clone();
            e.id = retag(e.id, map);
            e.floor = floor(e.floor);
            match &mut e.kind {
                StructureKind::Stair { to } | StructureKind::Ramp { to } => *to = floor(*to),
                _ => {}
            }
            if let Placement::Free(p) = &mut e.placement {
                p.y_mm += (dy * 1000.0).round() as i32;
            }
            out.push(WorldCommand::PlaceElement(e));
        }
        for (i, z) in self.zones.iter().enumerate() {
            let shift = |v: [f32; 3]| Vec3::new(v[0], v[1] + dy, v[2]);
            let mut obj = AuthoredObject::new(
                retag(ObjectId::from_u128(0x2000_0000 + i as u128), map),
                AuthoredKind::Zone,
                floor(FloorId(z.floor)),
                shift(z.min),
                shift(z.max),
            );
            obj.metadata = z.metadata.clone();
            out.push(WorldCommand::PlaceAuthored(obj));
        }
        out
    }

    /// Команды мира, которые кладут предметы карты картой `map`. `resolve` даёт по имени прототипа
    /// Build идентификатор движка и размер предмета на земле, м; `Err` — имя, которого нет.
    pub fn item_commands(
        &self,
        map: MapId,
        resolve: impl Fn(&str) -> Option<(PrototypeId, f32)>,
    ) -> Result<Vec<WorldCommand>, String> {
        let floor = floor_id(map, 0);
        let base = self
            .levels()
            .iter()
            .find(|f| f.id == FloorId(0))
            .map_or(0.0, |f| f.base_y)
            + map_base_y(map)
            + FLOOR_TOP_M;
        let mut out = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            let (prototype, size) =
                resolve(&item.prototype).ok_or_else(|| item.prototype.clone())?;
            let h = size * 0.5;
            let mut obj = AuthoredObject::new(
                retag(ObjectId::from_u128(0x1000_0000 + i as u128), map),
                AuthoredKind::Placement { prototype },
                floor,
                Vec3::new(item.x - h, base, item.z - h),
                Vec3::new(item.x + h, base + size, item.z + h),
            );
            if item.rot != 0 {
                obj.metadata.insert("rot".into(), item.rot.to_string());
            }
            for (k, v) in &item.meta {
                obj.metadata.insert(k.clone(), v.clone());
            }
            if !item.links.is_empty() {
                let text: Vec<String> =
                    item.links.iter().map(|[x, z]| format!("{x},{z}")).collect();
                obj.metadata.insert("links".into(), text.join(";"));
            }
            out.push(WorldCommand::PlaceAuthored(obj));
        }
        Ok(out)
    }

    /// Снимок карты 0 мира (см. [`MapFile::from_map`]).
    pub fn from_world(
        name: &str,
        spawn: [f32; 3],
        world: &World,
        name_of: impl Fn(&PrototypeId) -> Option<String>,
    ) -> (Self, usize) {
        Self::from_map(name, spawn, world, MapId(0), name_of)
    }

    /// Снимок карты `map` мира как файла карты: этажи, элементы, предметы-размещения и зоны (по
    /// возрастанию идентификатора, чтобы файл не менялся без причины), обратно к уровням и высотам
    /// карты. `spawn` — в координатах мира. `name_of` даёт имя прототипа Build по идентификатору
    /// движка; размещения без имени пропускаются и возвращаются числом.
    pub fn from_map(
        name: &str,
        spawn: [f32; 3],
        world: &World,
        map: MapId,
        name_of: impl Fn(&PrototypeId) -> Option<String>,
    ) -> (Self, usize) {
        let dy = map_base_y(map);
        let ours = |f: FloorId| world.map_of_floor(f) == Some(map);
        let level = |f: FloorId| FloorId(level_of(f).1);
        let mut elements: Vec<StructureElement> = world
            .elements()
            .filter(|e| ours(e.floor))
            .cloned()
            .map(|mut e| {
                e.id = retag(e.id, map);
                e.floor = level(e.floor);
                match &mut e.kind {
                    StructureKind::Stair { to } | StructureKind::Ramp { to } => *to = level(*to),
                    _ => {}
                }
                if let Placement::Free(p) = &mut e.placement {
                    p.y_mm -= (dy * 1000.0).round() as i32;
                }
                e
            })
            .collect();
        elements.sort_by_key(|e| e.id);
        let mut floors: Vec<FloorDef> = world
            .floors()
            .filter(|f| f.map == map)
            .map(|f| FloorDef {
                id: level(f.id),
                base_y: f.base_y - dy,
                map: MapId(0),
                ..f.clone()
            })
            .collect();
        let mut objects: Vec<AuthoredObject> = world
            .authored()
            .filter(|o| ours(o.floor))
            .cloned()
            .map(|mut o| {
                o.id = retag(o.id, map);
                o
            })
            .collect();
        objects.sort_by_key(|o| o.id);
        let (mut items, mut zones, mut skipped) = (Vec::new(), Vec::new(), 0);
        for o in &objects {
            match &o.kind {
                AuthoredKind::Placement { prototype } => match name_of(prototype) {
                    Some(prototype) => items.push(item_of(o, prototype)),
                    None => skipped += 1,
                },
                AuthoredKind::Zone => {
                    let unshift = |v: Vec3| [v.x, v.y - dy, v.z];
                    zones.push(MapZone {
                        min: unshift(o.min()),
                        max: unshift(o.max()),
                        floor: level(o.floor).0,
                        metadata: o.metadata.clone(),
                    });
                }
                _ => {}
            }
        }
        let mut map_file = Self {
            name: name.into(),
            // До миллиметра, как координаты объектов мира: на высоте другой карты float теряет точность.
            spawn: [spawn[0], spawn[1] - dy, spawn[2]].map(|v| (v * 1000.0).round() / 1000.0),
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
            if map_file.levels().iter().find(|i| i.id == f.id) != Some(&f) {
                map_file.floors.push(f);
            }
        }
        map_file.floors.sort_by_key(|f| f.id.0);
        (map_file, skipped)
    }
}

/// Определение карты мира для загрузки: имя и флаги.
pub fn map_def(id: MapId, name: &str, paused: bool, initialized: bool) -> MapDef {
    MapDef {
        id,
        name: name.chars().take(astrocore_world::MAX_MAP_NAME).collect(),
        paused,
        initialized,
    }
}

/// Неявный уровень `id` с основанием на высоте `base_y`.
fn implicit_level(id: i32, base_y: f32) -> FloorDef {
    FloorDef {
        id: FloorId(id),
        name: format!("Уровень {id}"),
        base_y,
        height: LEVEL_HEIGHT_M,
        map: MapId(0),
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
            map: Default::default(),
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

    #[test]
    fn one_map_file_loads_as_several_maps_of_one_world_and_saves_back() {
        let mut map = MapFile::parse(MAP).unwrap();
        map.floors.clear();
        map.items.push(MapItem {
            prototype: "Locker".into(),
            x: 0.5,
            z: 0.5,
            rot: 1,
            links: vec![[1, 2]],
            meta: [("locked".to_string(), "true".to_string())].into(),
        });
        let locker = PrototypeId::try_from("build:locker".to_string()).unwrap();
        let resolve = |name: &str| (name == "Locker").then(|| (locker.clone(), 0.5));
        let mut world = map.build_world("t").unwrap();
        for c in map.item_commands(MapId(0), resolve).unwrap() {
            world.apply(c).unwrap();
        }
        // Та же карта картой 2: свои этажи, высота и идентификаторы — ничего не пересекается.
        let two = MapId(2);
        world
            .apply(WorldCommand::DefineMap(map_def(two, "копия", true, false)))
            .unwrap();
        for c in map.structure_commands(two) {
            world.apply(c).unwrap();
        }
        for c in map.item_commands(two, resolve).unwrap() {
            world.apply(c).unwrap();
        }
        let level0 = world.floor(floor_id(two, 0)).unwrap();
        assert!((level0.base_y - 2.0 * MAP_SPACING_M).abs() < 1e-3);
        assert_eq!(level0.map, two);
        assert!(world.floor_paused(floor_id(two, 0)));
        assert_eq!(world.element_count(), 2 * map.elements.len());
        let name_of = |id: &PrototypeId| (id == &locker).then(|| "Locker".to_string());
        let spawn = [map.spawn[0], map.spawn[1] + map_base_y(two), map.spawn[2]];
        let (saved, skipped) = MapFile::from_map("t", spawn, &world, two, name_of);
        assert_eq!(skipped, 0);
        assert_eq!(saved, map, "снимок карты 2 — исходный файл");
        let (saved0, _) = MapFile::from_world("t", map.spawn, &world, name_of);
        assert_eq!(saved0, map, "карта 0 не задета");
        // Удаление карты 2 оставляет карту 0 целой.
        world.apply(WorldCommand::RemoveMap { id: two }).unwrap();
        assert_eq!(world.element_count(), map.elements.len());
        assert_eq!(level_of(floor_id(two, -3)), (two, -3));
        assert_eq!(level_of(FloorId(7)), (MapId(0), 7));
    }
}
