//! Импорт карт SS14 (YAML карты формата 6/7) в формат `maps::MapFile`.
//!
//! Правила переноса:
//! * клетка с полом (не космос и не решётка) становится плитой пола;
//! * стена — это клетка, а не ребро: каждое ребро между проходимой клеткой и клеткой со стеной
//!   (прототипы `Wall*`, окна и решётки) становится стеной движка;
//! * дверь (`Airlock*`, `Door*`, `Windoor*`) занимает клетку: в проёме стены на её сторону
//!   ставится дверь на ребро между ней и проходимой клеткой по оси прохода;
//! * `SpawnPoint*` задаёт точку появления, предметы переносятся, если такой прототип есть в
//!   содержимом Build;
//! * ось Y карты SS14 смотрит «на север», у нас север — это −Z, поэтому `z = −y`.
//!
//! Формат клеток чанка (по `MapChunkSerializer` движка SS14): версия 7 — по 7 байт на клетку,
//! версия 6 — 6, ранее — 4; идентификатор тайла в первых 4 (или 2) байтах, порядок построчный.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use astrocore_core::{ObjectId, PrototypeId};
use astrocore_math::{GridCell, GridEdge};
use astrocore_world::{FloorDef, FloorId, OpeningKind, Placement, StructureElement, StructureKind};
use base64::Engine as _;
use maps::{MapFile, MapItem};
use serde_yaml::Value;

/// Высота верхней поверхности плиты пола, м (как у полигона).
const FLOOR_TOP_M: f32 = 0.2;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("YAML карты не разбирается: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("в карте нет ни одной клетки с полом")]
    NoFloor,
}

/// Материалы движка для конструкций импортируемой карты.
#[derive(Debug, Clone)]
pub struct Materials {
    pub wall: PrototypeId,
    pub floor: PrototypeId,
    pub door: PrototypeId,
}

impl Default for Materials {
    fn default() -> Self {
        let id = |s: &str| PrototypeId::new(s).expect("встроенный идентификатор валиден");
        Self {
            wall: id("ss14:wall_solid"),
            floor: id("ss14:floor_dark"),
            door: id("ss14:airlock"),
        }
    }
}

/// Что получилось при импорте.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub floor_tiles: usize,
    pub wall_cells: usize,
    pub wall_edges: usize,
    pub doors: usize,
    pub doors_unplaced: usize,
    pub spawns: usize,
    pub items: usize,
    pub skipped_entities: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Wall,
    Door,
    Spawn,
    Other,
}

fn classify(proto: &str) -> Kind {
    if proto.starts_with("Wall")
        || proto.contains("Window") && !proto.contains("Windoor")
        || proto.contains("Grille")
    {
        Kind::Wall
    } else if proto.contains("Airlock") || proto.starts_with("Door") || proto.contains("Windoor") {
        Kind::Door
    } else if proto.starts_with("SpawnPoint") {
        Kind::Spawn
    } else {
        Kind::Other
    }
}

fn parse_pair(text: &str) -> Option<(f32, f32)> {
    let (a, b) = text.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn is_floor_tile(name: &str) -> bool {
    !(name.starts_with("Space") || name.starts_with("Lattice") || name == "Empty")
}

/// Тайлы чанка: (x, y) мировые клетки → имя тайла SS14.
fn decode_chunk(
    chunk: &Value,
    offset: (i32, i32),
    tilemap: &HashMap<i64, String>,
    out: &mut BTreeMap<(i32, i32), String>,
) {
    let Some(b64) = chunk.get("tiles").and_then(Value::as_str) else {
        return;
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
        return;
    };
    let version = chunk.get("version").and_then(Value::as_i64).unwrap_or(1);
    let size = chunk.get("size").and_then(Value::as_i64).unwrap_or(16) as i32;
    let (cx, cy) = chunk
        .get("ind")
        .and_then(Value::as_str)
        .and_then(parse_pair)
        .map_or((0, 0), |(x, y)| (x as i32, y as i32));
    let stride = match version {
        7.. => 7,
        6 => 6,
        _ => 4,
    };
    let mut at = 0usize;
    for y in 0..size {
        for x in 0..size {
            if at + stride > bytes.len() {
                return;
            }
            let id = if version >= 6 {
                i64::from(i32::from_le_bytes(
                    bytes[at..at + 4].try_into().unwrap_or([0; 4]),
                ))
            } else {
                i64::from(u16::from_le_bytes(
                    bytes[at..at + 2].try_into().unwrap_or([0; 2]),
                ))
            };
            at += stride;
            if let Some(name) = tilemap.get(&id) {
                out.insert(
                    (cx * size + x + offset.0, cy * size + y + offset.1),
                    name.clone(),
                );
            }
        }
    }
}

fn entity_pos(entity: &Value) -> Option<(f32, f32)> {
    entity
        .get("components")?
        .as_sequence()?
        .iter()
        .find(|c| c.get("type").and_then(Value::as_str) == Some("Transform"))?
        .get("pos")?
        .as_str()
        .and_then(parse_pair)
}

/// Импортировать карту SS14 из текста YAML. `known_items` — имена прототипов Build, которые можно
/// класть на пол (совпадают с идентификаторами SS14 дословно).
pub fn import(
    yaml: &str,
    name: &str,
    known_items: &BTreeSet<String>,
    mats: &Materials,
) -> Result<(MapFile, Report), ImportError> {
    let root: Value = serde_yaml::from_str(yaml)?;
    let tilemap: HashMap<i64, String> = root
        .get("tilemap")
        .and_then(Value::as_mapping)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.as_i64()?, v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let mut report = Report::default();
    let mut tiles: BTreeMap<(i32, i32), String> = BTreeMap::new();
    let groups = root
        .get("entities")
        .and_then(Value::as_sequence)
        .cloned()
        .unwrap_or_default();
    for group in &groups {
        let Some(list) = group.get("entities").and_then(Value::as_sequence) else {
            continue;
        };
        for entity in list {
            let Some(comps) = entity.get("components").and_then(Value::as_sequence) else {
                continue;
            };
            let Some(grid) = comps
                .iter()
                .find(|c| c.get("type").and_then(Value::as_str) == Some("MapGrid"))
            else {
                continue;
            };
            let off =
                entity_pos(entity).map_or((0, 0), |(x, y)| (x.floor() as i32, y.floor() as i32));
            if let Some(chunks) = grid.get("chunks").and_then(Value::as_mapping) {
                for (_, chunk) in chunks {
                    decode_chunk(chunk, off, &tilemap, &mut tiles);
                }
            }
        }
    }

    let mut solid: BTreeSet<(i32, i32)> = BTreeSet::new();
    let mut doors: BTreeSet<(i32, i32)> = BTreeSet::new();
    let mut spawn: Option<(f32, f32)> = None;
    let mut items: Vec<MapItem> = Vec::new();
    for group in &groups {
        let proto = group
            .get("proto")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(list) = group.get("entities").and_then(Value::as_sequence) else {
            continue;
        };
        if proto.is_empty() {
            // Грид и карта — служебные сущности без прототипа.
            continue;
        }
        let kind = classify(proto);
        for entity in list {
            let Some((x, y)) = entity_pos(entity) else {
                continue;
            };
            let cell = (x.floor() as i32, -(y.floor() as i32) - 1);
            match kind {
                Kind::Wall => {
                    solid.insert(cell);
                }
                Kind::Door => {
                    doors.insert(cell);
                }
                Kind::Spawn => {
                    report.spawns += 1;
                    spawn.get_or_insert((x, -y));
                }
                Kind::Other if known_items.contains(proto) => {
                    items.push(MapItem {
                        prototype: proto.to_string(),
                        x,
                        z: -y,
                    });
                }
                Kind::Other => report.skipped_entities += 1,
            }
        }
    }
    // Клетки пола в наших координатах: z = −y − 1, чтобы клетка (x, y) → [−y−1, −y) по оси Z.
    let mut walk: BTreeSet<(i32, i32)> = tiles
        .iter()
        .filter(|(_, n)| is_floor_tile(n))
        .map(|(&(x, y), _)| (x, -y - 1))
        .collect();
    walk.extend(doors.iter().copied());
    for s in &solid {
        walk.remove(s);
    }
    if walk.is_empty() {
        return Err(ImportError::NoFloor);
    }
    report.floor_tiles = walk.len();
    report.wall_cells = solid.len();
    report.items = items.len();

    let mut next = 0x9000_0000u128;
    let mut elements = Vec::new();
    let mut element = |kind, placement, material: &PrototypeId, thickness| {
        next += 1;
        StructureElement {
            id: ObjectId::from_u128(next),
            kind,
            placement,
            floor: FloorId(0),
            material: material.clone(),
            thickness,
        }
    };
    let dirs = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    let mut door_edges: BTreeSet<GridEdge> = BTreeSet::new();
    for &d in &doors {
        // Ось, вдоль которой идёт стена: оба соседа на ней — стены; проход — поперёк.
        let along_x = solid.contains(&(d.0 - 1, d.1)) && solid.contains(&(d.0 + 1, d.1));
        let along_z = solid.contains(&(d.0, d.1 - 1)) && solid.contains(&(d.0, d.1 + 1));
        let candidates = if along_x && !along_z {
            [(d.0, d.1 + 1), (d.0, d.1 - 1)]
        } else if along_z && !along_x {
            [(d.0 + 1, d.1), (d.0 - 1, d.1)]
        } else {
            report.doors_unplaced += 1;
            continue;
        };
        let edge = candidates
            .into_iter()
            .find(|c| walk.contains(c) && !doors.contains(c))
            .and_then(|t| GridEdge::between(GridCell::new(d.0, d.1), GridCell::new(t.0, t.1)));
        match edge {
            Some(e) => {
                door_edges.insert(e);
            }
            None => report.doors_unplaced += 1,
        }
    }
    let mut wall_edges: BTreeSet<GridEdge> = BTreeSet::new();
    for &c in &walk {
        for (dx, dz) in dirs {
            let n = (c.0 + dx, c.1 + dz);
            if solid.contains(&n)
                && let Some(e) = GridEdge::between(GridCell::new(c.0, c.1), GridCell::new(n.0, n.1))
            {
                wall_edges.insert(e);
            }
        }
    }
    for e in &wall_edges {
        elements.push(element(
            StructureKind::Wall,
            Placement::Edge(*e),
            &mats.wall,
            0.2,
        ));
    }
    for e in &door_edges {
        elements.push(element(
            StructureKind::Opening(OpeningKind::Door),
            Placement::Edge(*e),
            &mats.door,
            0.1,
        ));
    }
    for &c in &walk {
        elements.push(element(
            StructureKind::Floor,
            Placement::Cell(GridCell::new(c.0, c.1)),
            &mats.floor,
            0.2,
        ));
    }
    report.wall_edges = wall_edges.len();
    report.doors = door_edges.len();

    let (sx, sz) = spawn.unwrap_or_else(|| {
        let c = walk.iter().next().copied().unwrap_or((0, 0));
        (c.0 as f32 + 0.5, c.1 as f32 + 0.5)
    });
    let map = MapFile {
        name: name.into(),
        spawn: [sx, FLOOR_TOP_M + 0.05, sz],
        floors: vec![FloorDef {
            id: FloorId(0),
            name: "Этаж 0".into(),
            base_y: 0.0,
            height: 3.0,
        }],
        elements,
        items,
    };
    Ok((map, report))
}

#[cfg(test)]
mod tests;
