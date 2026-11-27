//! Декали: рисунки на полу грида (разметка, грязь, надписи). Образец — `Decal`,
//! `DecalChunkComponent`, `DecalChunkDecalsSerializer`, `DecalGridComponent` и `SharedDecalSystem`
//! SS14 @ 2f4a2aad, `ChunkEntityComponent` и `ChunkContainerComponent` Robust @ 1afd692.
//!
//! В игре декали грида лежат в одном сетевом компоненте [`GridDecals`]: клиент рисует их,
//! сервер меняет. В файле карты SS14 они живут в «чанк-сущностях» вне карт (`ChunkEntity` +
//! `DecalChunk`, чанк 16×16 клеток), а грид перечисляет их в `ChunkContainer`; старые карты
//! хранят их в компоненте грида `DecalGrid`. При загрузке декали читаются из обоих видов,
//! перед сохранением [`write_for_save`] переписывает чанк-сущности заново.

use std::collections::BTreeMap;

use astrocore_data::{DataMap, DataNode};
use astrocore_ecs::{EventWorldExt, LifecycleWorldExt, MetaData};
use astrocore_map::{
    Map, MapFileEntity, MapLoaded, MapPreserved, MapWorldExt, Transform, TransformWorldExt,
};
use astrocore_math::Vec2;
use astrocore_net::NetRegistryWorldExt;
use astrocore_prototypes::PrototypeWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;

/// Сторона чанка декалей в клетках (`ChunkEntitySystem.ChunkSize`).
pub const CHUNK_SIZE: i32 = 16;
/// Прототип чанк-сущности.
pub const CHUNK_ENTITY: &str = "ChunkEntity";

/// Декаль (`Decal` SS14): левый нижний угол в координатах грида, прототип декали, цвет,
/// поворот, порядок наложения.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decal {
    pub coordinates: Vec2,
    pub id: String,
    /// Цвет `#RRGGBBAA`; `None` — цвет рисунка.
    pub color: Option<String>,
    /// Поворот в радианах.
    pub angle: f32,
    pub z_index: i32,
    pub cleanable: bool,
}

/// Ключ декали: чанк и номер в чанке, как в SS14.
pub type DecalKey = (i32, i32, u16);

/// Декали грида. Сетевой компонент.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GridDecals {
    pub decals: BTreeMap<DecalKey, Decal>,
    /// Наибольший выданный номер в чанке (`MaxDecalId`).
    pub max_ids: BTreeMap<(i32, i32), u16>,
    /// Растёт с каждым изменением: клиент перерисовывает декали грида.
    pub revision: u64,
}

/// Чанк-сущности грида из файла карты: переписываются при сохранении. Только сервер.
#[derive(Component, Debug, Clone, Default)]
pub struct DecalChunkEntities(pub BTreeMap<(i32, i32), Entity>);

/// Чанк клетки грида.
pub fn chunk_of(coordinates: Vec2) -> (i32, i32) {
    (
        (coordinates.x / CHUNK_SIZE as f32).floor() as i32,
        (coordinates.y / CHUNK_SIZE as f32).floor() as i32,
    )
}

impl GridDecals {
    /// Добавляет декаль под следующим номером её чанка.
    pub fn add(&mut self, decal: Decal) -> DecalKey {
        let chunk = chunk_of(decal.coordinates);
        let used = self
            .decals
            .range((chunk.0, chunk.1, 0)..=(chunk.0, chunk.1, u16::MAX))
            .map(|(key, _)| key.2)
            .max();
        let max = self.max_ids.entry(chunk).or_default();
        let id = used.map_or(*max, |used| used.max(*max)).saturating_add(1);
        *max = id;
        let key = (chunk.0, chunk.1, id);
        self.decals.insert(key, decal);
        self.revision += 1;
        key
    }

    /// Удаляет декали, чей угол лежит в прямоугольнике грида; возвращает, сколько удалено.
    pub fn remove_in(&mut self, min: Vec2, max: Vec2) -> usize {
        let before = self.decals.len();
        self.decals.retain(|_, decal| {
            let c = decal.coordinates;
            !(c.x >= min.x && c.y >= min.y && c.x < max.x && c.y < max.y)
        });
        let removed = before - self.decals.len();
        if removed > 0 {
            self.revision += 1;
        }
        removed
    }
}

fn number(value: f32) -> String {
    let text = format!("{value}");
    if text == "-0" { "0".to_owned() } else { text }
}

fn read_vec(node: &DataNode) -> Option<Vec2> {
    let (x, y) = node.as_str()?.split_once(',')?;
    Some(Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
}

fn read_angle(node: Option<&DataNode>) -> f32 {
    let Some(text) = node.and_then(DataNode::as_str) else {
        return 0.0;
    };
    let text = text.trim();
    match text.strip_suffix("rad") {
        Some(radians) => radians.trim().parse().unwrap_or(0.0),
        None => text.parse::<f32>().map(f32::to_radians).unwrap_or(0.0),
    }
}

/// Общие поля группы декалей (`DecalData`).
fn read_data(node: &DataNode) -> Option<Decal> {
    Some(Decal {
        coordinates: Vec2::ZERO,
        id: node.get("id")?.as_str()?.to_owned(),
        color: node
            .get("color")
            .and_then(DataNode::as_str)
            .map(str::to_owned),
        angle: read_angle(node.get("angle")),
        z_index: node
            .get("zIndex")
            .and_then(DataNode::as_str)
            .and_then(|z| z.parse().ok())
            .unwrap_or(0),
        cleanable: node
            .get("cleanable")
            .and_then(DataNode::as_str)
            .is_some_and(|c| c.eq_ignore_ascii_case("true")),
    })
}

/// Группы `nodes` (`DecalChunkDecalsSerializer`, версия 1, и `DecalGrid` версии 2): номер →
/// декаль.
fn read_groups(nodes: &DataNode) -> Vec<(u16, Decal)> {
    let mut found = Vec::new();
    for group in nodes.as_seq().unwrap_or_default() {
        let Some(data) = group.get("node").and_then(read_data) else {
            continue;
        };
        let Some(decals) = group.get("decals").and_then(DataNode::as_map) else {
            continue;
        };
        for (id, coordinates) in decals.iter() {
            let (Ok(id), Some(coordinates)) = (id.parse::<u16>(), read_vec(coordinates)) else {
                continue;
            };
            found.push((
                id,
                Decal {
                    coordinates,
                    ..data.clone()
                },
            ));
        }
    }
    found
}

/// Узел `decals` компонента `DecalChunk` (`DecalChunkDecalsSerializer.Write`): группы по
/// общим полям в порядке SS14, внутри — по номеру.
fn write_groups(decals: &[(u16, &Decal)]) -> DataNode {
    type GroupKey = (String, String, i64, i32, bool);
    let mut groups: BTreeMap<GroupKey, (DataMap, Vec<(u16, Vec2)>)> = BTreeMap::new();
    for (id, decal) in decals {
        let key = (
            decal.id.clone(),
            decal.color.clone().unwrap_or_default(),
            (f64::from(decal.angle) * 1e9) as i64,
            decal.z_index,
            decal.cleanable,
        );
        let entry = groups.entry(key).or_insert_with(|| {
            let mut node = DataMap::new();
            if decal.z_index != 0 {
                node.insert("zIndex", DataNode::plain(decal.z_index.to_string()));
            }
            node.insert("id", DataNode::plain(decal.id.clone()));
            if let Some(color) = &decal.color {
                node.insert("color", DataNode::string(color.clone()));
            }
            if decal.angle != 0.0 {
                let text = astrocore_math::Angle(decal.angle).radians_text();
                node.insert("angle", DataNode::plain(format!("{text} rad")));
            }
            if decal.cleanable {
                node.insert("cleanable", DataNode::plain("True"));
            }
            (node, Vec::new())
        });
        entry.1.push((*id, decal.coordinates));
    }
    let mut nodes = Vec::new();
    for (_, (node, mut list)) in groups {
        list.sort_by_key(|(id, _)| *id);
        let mut decals = DataMap::new();
        for (id, at) in list {
            decals.insert(
                id.to_string(),
                DataNode::plain(format!("{},{}", number(at.x), number(at.y))),
            );
        }
        let mut group = DataMap::new();
        group.insert("node", DataNode::map(node));
        group.insert("decals", DataNode::map(decals));
        nodes.push(DataNode::map(group));
    }
    let mut root = DataMap::new();
    root.insert("version", DataNode::plain("1"));
    root.insert("nodes", DataNode::seq(nodes));
    DataNode::map(root)
}

/// Убирает из сохранённых как были компонентов записи с этими именами и возвращает их.
fn take_preserved(world: &mut World, entity: Entity, names: &[&str]) -> Vec<DataNode> {
    let Some(mut preserved) = world.get_mut::<MapPreserved>(entity) else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    preserved.components.retain(|node| {
        let hit = node.type_name().is_some_and(|name| names.contains(&name));
        if hit {
            taken.push(node.clone());
        }
        !hit
    });
    taken
}

/// Сохранённые как были компоненты с этим именем.
fn preserved(world: &World, entity: Entity, name: &str) -> Vec<DataNode> {
    world
        .get::<MapPreserved>(entity)
        .map(|p| {
            p.components
                .iter()
                .filter(|node| node.type_name() == Some(name))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Файл загружен: декали чанк-сущностей и старого `DecalGrid` переходят в [`GridDecals`].
fn adopt(world: &mut World, loaded: &MapLoaded) {
    // Данные остаются в `MapPreserved`, пока их не перепишет `write_for_save`: сохранение
    // без него не теряет декали.
    for &grid in &loaded.grids {
        for old in preserved(world, grid, "DecalGrid") {
            let Some(nodes) = old.get("chunkCollection").and_then(|c| c.get("nodes")) else {
                continue;
            };
            let mut decals = world.get::<GridDecals>(grid).cloned().unwrap_or_default();
            for (_, decal) in read_groups(nodes) {
                decals.add(decal);
            }
            world.entity_mut(grid).insert(decals);
        }
    }
    let mut chunk_entities: Vec<(u32, Entity)> = loaded
        .uids
        .iter()
        .filter(|(_, entity)| {
            world
                .get::<MetaData>(**entity)
                .and_then(|meta| meta.prototype.as_deref())
                == Some(CHUNK_ENTITY)
        })
        .map(|(uid, entity)| (*uid, *entity))
        .collect();
    chunk_entities.sort_unstable();
    for (_, chunk_entity) in chunk_entities {
        let Some(link) = world.get::<MapPreserved>(chunk_entity).and_then(|p| {
            p.components
                .iter()
                .find(|c| c.type_name() == Some(CHUNK_ENTITY))
                .cloned()
        }) else {
            continue;
        };
        let grid = link
            .get("root")
            .and_then(DataNode::as_str)
            .and_then(|uid| uid.parse::<u32>().ok())
            .and_then(|uid| loaded.uids.get(&uid).copied());
        let chunk = link.get("chunk").and_then(read_vec);
        let (Some(grid), Some(chunk)) = (grid, chunk) else {
            continue;
        };
        let chunk = (chunk.x as i32, chunk.y as i32);
        let data = preserved(world, chunk_entity, "DecalChunk");
        let mut decals = world.get::<GridDecals>(grid).cloned().unwrap_or_default();
        for node in &data {
            if let Some(max) = node
                .get("maxDecalId")
                .and_then(DataNode::as_str)
                .and_then(|m| m.parse::<u16>().ok())
            {
                let entry = decals.max_ids.entry(chunk).or_default();
                *entry = (*entry).max(max);
            }
            let groups = node.get("decals").and_then(|decals| decals.get("nodes"));
            for (id, decal) in groups.map(read_groups).unwrap_or_default() {
                decals.decals.insert((chunk.0, chunk.1, id), decal);
            }
        }
        world.entity_mut(grid).insert(decals);
        let mut links = world
            .get::<DecalChunkEntities>(grid)
            .cloned()
            .unwrap_or_default();
        links.0.insert(chunk, chunk_entity);
        world.entity_mut(grid).insert(links);
    }
}

/// Номера сущностей карты в файле: следующий свободный.
/// Новый номер сущности файла корня (карты или грида): больше номеров всего, что
/// сохраняется с корнем и с картой, на которой стоит грид, — номер не совпадёт ни в файле
/// грида, ни в файле карты.
fn next_file_uid(world: &mut World, root: Entity) -> u32 {
    let map = world
        .get::<Transform>(root)
        .and_then(Transform::map)
        .unwrap_or(root);
    let mut query = world.query::<(Entity, &MapFileEntity)>();
    let mut max = 0;
    for (entity, file) in query.iter(world) {
        let on_map = file.nullspace_of == Some(map)
            || file.nullspace_of == Some(root)
            || world
                .map_coordinates(entity)
                .and_then(|c| world.map_entity(c.map))
                == Some(map)
            || entity == map
            || entity == root;
        if on_map {
            max = max.max(file.uid);
        }
    }
    max + 1
}

/// Гриды, которые сохраняются с корнем: все гриды карты или сам грид.
pub fn saved_grids(world: &World, root: Entity) -> Vec<Entity> {
    match world.get::<Map>(root) {
        Some(map) => map.grids().to_vec(),
        None if world.get::<astrocore_map::MapGrid>(root).is_some() => vec![root],
        None => Vec::new(),
    }
}

/// Номер сущности в файле: прежний или новый.
fn file_uid(world: &mut World, entity: Entity, map: Entity, nullspace: bool) -> u32 {
    if let Some(file) = world.get::<MapFileEntity>(entity)
        && file.uid != 0
    {
        return file.uid;
    }
    let uid = next_file_uid(world, map);
    world.entity_mut(entity).insert(MapFileEntity {
        uid,
        nullspace_of: nullspace.then_some(map),
    });
    uid
}

fn set_preserved(world: &mut World, entity: Entity, node: DataNode) {
    let name = node.type_name().map(str::to_owned);
    let mut preserved = world
        .get::<MapPreserved>(entity)
        .cloned()
        .unwrap_or_default();
    preserved
        .components
        .retain(|existing| existing.type_name().map(str::to_owned) != name);
    preserved.components.push(node);
    world.entity_mut(entity).insert(preserved);
}

fn typed(name: &str, fields: DataMap) -> DataNode {
    let mut node = DataMap::new();
    node.insert("type", DataNode::plain(name));
    for (key, value) in fields.iter() {
        node.insert(key, value.clone());
    }
    DataNode::map(node)
}

/// Перед сохранением карты или грида: чанк-сущности гридов получают декали в формате SS14, грид —
/// их список в `ChunkContainer`; пустые чанк-сущности удаляются.
pub fn write_for_save(world: &mut World, root: Entity) {
    let grids = saved_grids(world, root);
    // Корень — карта или отдельный грид (ADR-0020 движка): чанки сохраняются с ним.
    let map = root;
    let map_entity = world
        .get::<Transform>(root)
        .and_then(Transform::map)
        .unwrap_or(root);
    let paused = world
        .get::<Map>(map_entity)
        .is_some_and(|m| m.paused || !m.initialized);
    for grid in grids {
        let decals = world.get::<GridDecals>(grid).cloned().unwrap_or_default();
        let mut links = world
            .get::<DecalChunkEntities>(grid)
            .cloned()
            .unwrap_or_default();
        if decals.decals.is_empty() && links.0.is_empty() {
            continue;
        }
        let mut by_chunk: BTreeMap<(i32, i32), Vec<(u16, &Decal)>> = BTreeMap::new();
        for ((x, y, id), decal) in &decals.decals {
            by_chunk.entry((*x, *y)).or_default().push((*id, decal));
        }
        // Пустые чанки уходят.
        let gone: Vec<(i32, i32)> = links
            .0
            .keys()
            .filter(|chunk| !by_chunk.contains_key(chunk))
            .copied()
            .collect();
        for chunk in gone {
            if let Some(entity) = links.0.remove(&chunk) {
                world.delete_entity(entity);
            }
        }
        // Старый вид хранения заменяется чанк-сущностями, как при загрузке в SS14.
        take_preserved(world, grid, &["DecalGrid"]);
        let grid_uid = file_uid(world, grid, map, false);
        let mut uids = Vec::new();
        for (chunk, list) in by_chunk {
            let entity = match links
                .0
                .get(&chunk)
                .copied()
                .filter(|e| world.get_entity(*e).is_ok())
            {
                Some(entity) => entity,
                None => {
                    let Ok(entity) = world.spawn_prototype(CHUNK_ENTITY) else {
                        continue;
                    };
                    world.initialize_and_start(entity, false);
                    if paused {
                        world.set_entity_paused(entity, true);
                    }
                    links.0.insert(chunk, entity);
                    entity
                }
            };
            let uid = file_uid(world, entity, map, true);
            uids.push(uid);
            let mut link = DataMap::new();
            link.insert("root", DataNode::plain(grid_uid.to_string()));
            link.insert("chunk", DataNode::plain(format!("{},{}", chunk.0, chunk.1)));
            set_preserved(world, entity, typed(CHUNK_ENTITY, link));
            let max = decals.max_ids.get(&chunk).copied().unwrap_or(0);
            let max = list.iter().map(|(id, _)| *id).max().unwrap_or(0).max(max);
            let mut data = DataMap::new();
            data.insert("maxDecalId", DataNode::plain(max.to_string()));
            data.insert("decals", write_groups(&list));
            set_preserved(world, entity, typed("DecalChunk", data));
        }
        uids.sort_unstable();
        let mut container = DataMap::new();
        container.insert(
            "chunkEntities",
            DataNode::seq(
                uids.iter()
                    .map(|uid| DataNode::plain(uid.to_string()))
                    .collect(),
            ),
        );
        set_preserved(world, grid, typed("ChunkContainer", container));
        world.entity_mut(grid).insert(links);
    }
}

/// Сервер: декаль на гриде под точкой карты.
pub fn place(world: &mut World, grid: Entity, local: Vec2, decal: Decal) -> DecalKey {
    let mut decals = world.get::<GridDecals>(grid).cloned().unwrap_or_default();
    let key = decals.add(Decal {
        coordinates: local,
        ..decal
    });
    world.entity_mut(grid).insert(decals);
    key
}

/// Сервер: удаляет декали грида в прямоугольнике его координат.
pub fn remove_in(world: &mut World, grid: Entity, min: Vec2, max: Vec2) -> usize {
    let Some(mut decals) = world.get::<GridDecals>(grid).cloned() else {
        return 0;
    };
    let removed = decals.remove_in(min, max);
    if removed > 0 {
        world.entity_mut(grid).insert(decals);
    }
    removed
}

pub fn install(world: &mut World, side: Side) {
    world.register_net_serde::<GridDecals>();
    if side.is_server() {
        world.subscribe_broadcast::<MapLoaded>(|world, event| adopt(world, event));
    }
}

#[cfg(test)]
mod tests;
