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
use maps::{MapFile, MapItem, MapZone};
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
    /// WallMount prototypes from the source-derived metadata, projected onto 3D wall faces.
    pub wallmounts: BTreeSet<String>,
    pub wall: PrototypeId,
    pub floor: PrototypeId,
    pub door: PrototypeId,
    pub firelock: PrototypeId,
    /// Материал по имени плитки SS14 (`FloorSteel` → `ss14:tile_floorsteel`); неизвестные плитки — `floor`.
    pub tiles: BTreeMap<String, PrototypeId>,
    /// Списки доступа считывателей по имени прототипа SS14 (`door_access.ron`).
    pub door_access: BTreeMap<String, Vec<Vec<String>>>,
}

impl Default for Materials {
    fn default() -> Self {
        let id = |s: &str| PrototypeId::new(s).expect("встроенный идентификатор валиден");
        Self {
            wallmounts: BTreeSet::new(),
            wall: id("ss14:wall_solid"),
            floor: id("ss14:floor_dark"),
            door: id("ss14:airlock"),
            firelock: id("ss14:firelock"),
            tiles: BTreeMap::new(),
            door_access: BTreeMap::new(),
        }
    }
}

/// Что получилось при импорте.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub floor_tiles: usize,
    pub wall_cells: usize,
    pub wall_edges: usize,
    /// Сплошные блоки стен на клетку.
    pub wall_blocks: usize,
    pub doors: usize,
    pub doors_unplaced: usize,
    /// Огнезапоры поставлены / не поставлены.
    pub firelocks: usize,
    pub firelocks_unplaced: usize,
    /// Сигнализации со списками устройств.
    pub alarms: usize,
    pub spawns: usize,
    pub items: usize,
    /// Трубы газовой сети (зоны `gas_pipe`).
    pub pipes: usize,
    /// Двери с ограничением доступа (зоны `door_access`).
    pub door_access: usize,
    /// Трубы диспозала (зоны `disposal_tube`).
    pub disposal_tubes: usize,
    /// Клетки с кабелями (зоны `power_cable`).
    pub cables: usize,
    /// Столы, стойки и шкафы (ящики-конструкции) и стулья.
    pub furniture: usize,
    pub chairs: usize,
    pub skipped_entities: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Wall,
    Door,
    /// Огнезапор (`Firelock`, `FirelockGlass`): дверь со своими спрайтами.
    Firelock,
    Spawn,
    Other,
}

fn classify(proto: &str) -> Kind {
    if proto.starts_with("Wall")
        || proto.contains("Window") && !proto.contains("Windoor")
        || proto.contains("Grille")
    {
        Kind::Wall
    } else if proto == "Firelock" || proto == "FirelockGlass" {
        Kind::Firelock
    } else if proto.contains("Airlock") || proto.starts_with("Door") || proto.contains("Windoor") {
        Kind::Door
    } else if proto.starts_with("SpawnPoint") {
        Kind::Spawn
    } else {
        Kind::Other
    }
}

/// Список устройств сигнализации (`DeviceList.devices`): `uid` связанных сущностей.
/// `AccessReader.access` самой сущности карты: списки уровней доступа.
fn reader_access(entity: &Value) -> Option<Vec<Vec<String>>> {
    let comp = entity
        .get("components")
        .and_then(Value::as_sequence)?
        .iter()
        .find(|c| c.get("type").and_then(Value::as_str) == Some("AccessReader"))?;
    let lists = comp.get("access")?.as_sequence()?;
    Some(
        lists
            .iter()
            .filter_map(|l| {
                Some(
                    l.as_sequence()?
                        .iter()
                        .filter_map(|t| t.as_str().map(String::from))
                        .collect(),
                )
            })
            .collect(),
    )
}

/// Булево поле компонента сущности карты (`EntityStorage.open`, `Lock.locked`).
fn component_bool(entity: &Value, component: &str, field: &str) -> Option<bool> {
    entity
        .get("components")?
        .as_sequence()?
        .iter()
        .find(|c| c.get("type").and_then(Value::as_str) == Some(component))?
        .get(field)?
        .as_bool()
}

fn component_field<'a>(entity: &'a Value, component: &str, field: &str) -> Option<&'a Value> {
    entity
        .get("components")?
        .as_sequence()?
        .iter()
        .find(|c| c.get("type").and_then(Value::as_str) == Some(component))?
        .get(field)
}

fn component_number(entity: &Value, component: &str, field: &str) -> Option<f64> {
    component_field(entity, component, field)?.as_f64()
}

fn component_text(entity: &Value, component: &str, field: &str) -> Option<String> {
    component_field(entity, component, field)?
        .as_str()
        .map(String::from)
}

fn device_list(entity: &Value) -> Vec<i64> {
    let comps = entity.get("components").and_then(Value::as_sequence);
    let find = |name: &str| {
        comps.and_then(|c| {
            c.iter()
                .find(|c| c.get("type").and_then(Value::as_str) == Some(name))
        })
    };
    let mut out: Vec<i64> = find("DeviceList")
        .and_then(|c| c.get("devices"))
        .and_then(Value::as_sequence)
        .map(|l| l.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    // Выключатели и кнопки: `DeviceLinkSource.linkedPorts` — цели сигнала по uid.
    if let Some(ports) = find("DeviceLinkSource")
        .and_then(|c| c.get("linkedPorts"))
        .and_then(Value::as_mapping)
    {
        out.extend(ports.keys().filter_map(Value::as_i64));
    }
    out
}

/// Поворот сущности в четвертях оборота против часовой стрелки (`rot: 1.57… rad`).
fn entity_rot(entity: &Value) -> u8 {
    let Some(rad) = entity
        .get("components")
        .and_then(Value::as_sequence)
        .and_then(|c| {
            c.iter()
                .find(|c| c.get("type").and_then(Value::as_str) == Some("Transform"))
        })
        .and_then(|c| c.get("rot"))
        .and_then(Value::as_str)
        .and_then(|t| t.trim().trim_end_matches("rad").trim().parse::<f64>().ok())
    else {
        return 0;
    };
    ((rad / std::f64::consts::FRAC_PI_2).round() as i64).rem_euclid(4) as u8
}

/// Толщина ребра-стены: рёбра нужны логике, а видимый объём даёт блок на всю клетку.
const WALL_EDGE_THICKNESS_M: f32 = 0.02;
/// Высота блока стены над полом, мм: одна стена — один тайл, куб 1×1×1 м.
const WALL_HEIGHT_MM: u32 = 1000;

/// Стороны трубы SS14 без поворота (`pipeDirection` прототипа): N, S, E, W битами 1, 2, 4, 8.
fn pipe_base_dirs(proto: &str) -> Option<u8> {
    const N: u8 = 1;
    const S: u8 = 2;
    const E: u8 = 4;
    const W: u8 = 8;
    // Дополнительные слои труб (`Alt1`, `Alt2`) — отдельные сети, их пока не переносим.
    if proto.ends_with("Alt1") || proto.ends_with("Alt2") {
        return None;
    }
    Some(match proto {
        "GasPipeStraight" => N | S,
        "GasPipeBend" => S | W,
        "GasPipeTJunction" => S | E | W,
        "GasPipeFourway" => N | S | E | W,
        "GasPipeHalf" => S,
        _ => return None,
    })
}

/// Мебель-ящик по прототипу SS14: материал и высота, м (`Table*`, `Rack`, шкафы с книгами).
fn furniture_box(proto: &str) -> Option<(&'static str, f32)> {
    if proto.starts_with("Table") {
        let material = if proto.contains("Wood") || proto.contains("Carpet") {
            "ss14:table_wood"
        } else if proto.contains("Reinforced") && !proto.contains("Glass") {
            "ss14:table_reinforced"
        } else if proto.contains("Glass") {
            "ss14:table_glass"
        } else {
            "ss14:table_generic"
        };
        // Стол ниже пояса: 0,6 м (было 0,78, выглядело сплошным кубом).
        return Some((material, 0.6));
    }
    match proto {
        "Rack" => Some(("ss14:rack", 0.9)),
        "BookshelfFilled" | "Bookshelf" => Some(("ss14:bookshelf", 1.6)),
        _ => None,
    }
}

/// Стул SS14 → прототип Build из `furniture.ron`.
fn chair_proto(proto: &str) -> Option<&'static str> {
    Some(match proto {
        "Chair" | "ChairFolding" | "ChairPilotSeat" | "ChairRitual" => "Chair",
        p if p.starts_with("ChairOffice") => "ChairOffice",
        "ComfyChair" => "ChairComfy",
        "ChairWood" => "ChairWooden",
        "Stool" | "StoolBar" => "StoolBar",
        _ => return None,
    })
}

/// Группа кабеля SS14 (`CableHV`, `CableMV`, `CableApcExtension`): значение `power_cable`.
fn cable_group(proto: &str) -> Option<&'static str> {
    match proto {
        p if p.starts_with("CableHV") && !p.contains("Stack") => Some("hv"),
        p if p.starts_with("CableMV") && !p.contains("Stack") => Some("mv"),
        p if p.starts_with("CableApcExtension") => Some("apc"),
        _ => None,
    }
}

/// Повернуть стороны против часовой стрелки: юг → восток → север → запад (`RotatePipeDirection`).
fn rotate_pipe_dirs(dirs: u8, quarters: u8) -> u8 {
    let mut d = dirs;
    for _ in 0..quarters % 4 {
        let mut n = 0;
        for (from, to) in [(2u8, 4u8), (4, 1), (1, 8), (8, 2)] {
            if d & from != 0 {
                n |= to;
            }
        }
        d = n;
    }
    d
}

/// Выходы трубы диспозала по порядку прототипа SS14 (`DisposalTube.exits`, поворот 0).
fn disposal_exits(proto: &str) -> Option<&'static str> {
    Some(match proto {
        "DisposalPipe" => "SN",
        "DisposalTrunk" => "S",
        "DisposalJunction" => "SWN",
        "DisposalJunctionFlipped" => "SEN",
        "DisposalYJunction" => "SWE",
        "DisposalXJunction" => "SENW",
        "DisposalBend" => "SW",
        _ => return None,
    })
}

/// Сторона света, повёрнутая на `quarters` четвертей против часовой: юг → восток → север → запад.
fn rotate_exit(c: char, quarters: u8) -> char {
    let order = ['S', 'E', 'N', 'W'];
    let i = order.iter().position(|o| *o == c).unwrap_or(0);
    order[(i + usize::from(quarters)) % 4]
}

fn dirs_text(d: u8) -> String {
    [(1u8, 'N'), (2, 'S'), (4, 'E'), (8, 'W')]
        .iter()
        .filter(|(b, _)| d & b != 0)
        .map(|(_, c)| *c)
        .collect()
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
    let mut zones: Vec<MapZone> = Vec::new();
    // Клетка каждой сущности по её `uid`: по ним читаются списки устройств сигнализаций.
    let mut uid_cell: HashMap<i64, (i32, i32)> = HashMap::new();
    for group in &groups {
        for entity in group
            .get("entities")
            .and_then(Value::as_sequence)
            .into_iter()
            .flatten()
        {
            if let (Some(uid), Some((x, y))) = (
                entity.get("uid").and_then(Value::as_i64),
                entity_pos(entity),
            ) {
                uid_cell.insert(uid, (x.floor() as i32, -(y.floor() as i32) - 1));
            }
        }
    }
    let mut fire_cells: BTreeSet<(i32, i32)> = BTreeSet::new();
    let mut fire_uids: BTreeMap<(i32, i32), Vec<i64>> = BTreeMap::new();
    let mut door_lists: BTreeMap<(i32, i32), Vec<Vec<String>>> = BTreeMap::new();
    let mut edge_access: BTreeMap<GridEdge, Vec<Vec<String>>> = BTreeMap::new();
    let mut cables_seen: BTreeSet<((i32, i32), &str)> = BTreeSet::new();
    // Ящики мебели: клетка, материал, высота.
    let mut boxes: Vec<((i32, i32), &'static str, f32)> = Vec::new();
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
                    // Доступ: свой `AccessReader` на карте или плата прототипа.
                    let lists =
                        reader_access(entity).or_else(|| mats.door_access.get(proto).cloned());
                    if let Some(l) = lists.filter(|l| !l.is_empty()) {
                        door_lists.insert(cell, l);
                    }
                }
                Kind::Firelock => {
                    fire_cells.insert(cell);
                    if let Some(uid) = entity.get("uid").and_then(Value::as_i64) {
                        fire_uids.entry(cell).or_default().push(uid);
                    }
                }
                Kind::Spawn => {
                    report.spawns += 1;
                    let job = proto.trim_start_matches("SpawnPoint");
                    // Появление новичков: пассажир и поздний вход (запасная должность SS14).
                    if matches!(job, "Passenger" | "Latejoin") {
                        spawn.get_or_insert((x, -y));
                    }
                    zones.push(MapZone {
                        min: [x - 0.4, FLOOR_TOP_M, -y - 0.4],
                        max: [x + 0.4, FLOOR_TOP_M + 2.0, -y + 0.4],
                        floor: 0,
                        metadata: [("spawn_job".to_string(), job.to_string())].into(),
                    });
                }
                Kind::Other if pipe_base_dirs(proto).is_some() => {
                    let dirs =
                        rotate_pipe_dirs(pipe_base_dirs(proto).unwrap_or(0), entity_rot(entity));
                    let (cx, cz) = (cell.0 as f32, cell.1 as f32);
                    zones.push(MapZone {
                        min: [cx + 0.01, 0.0, cz + 0.01],
                        max: [cx + 0.99, 3.0, cz + 0.99],
                        floor: 0,
                        metadata: [("gas_pipe".to_string(), dirs_text(dirs))].into(),
                    });
                    report.pipes += 1;
                }
                Kind::Other if disposal_exits(proto).is_some() => {
                    let rot = entity_rot(entity);
                    let exits: String = disposal_exits(proto)
                        .unwrap_or("")
                        .chars()
                        .map(|c| rotate_exit(c, rot))
                        .collect();
                    let (cx, cz) = (cell.0 as f32, cell.1 as f32);
                    let mut metadata: std::collections::BTreeMap<String, String> =
                        [("disposal_tube".to_string(), exits)].into();
                    if proto == "DisposalTrunk" {
                        metadata.insert("disposal_entry".into(), "1".into());
                    }
                    zones.push(MapZone {
                        min: [cx + 0.01, 0.0, cz + 0.01],
                        max: [cx + 0.99, 3.0, cz + 0.99],
                        floor: 0,
                        metadata,
                    });
                    report.disposal_tubes += 1;
                }
                Kind::Other if furniture_box(proto).is_some() => {
                    if let Some((m, h)) = furniture_box(proto) {
                        boxes.push((cell, m, h));
                    }
                }
                Kind::Other if chair_proto(proto).is_some() => {
                    if let Some(c) = chair_proto(proto).filter(|c| known_items.contains(*c)) {
                        items.push(MapItem {
                            prototype: c.to_string(),
                            x,
                            z: -y,
                            rot: 0,
                            links: Vec::new(),
                            meta: BTreeMap::new(),
                        });
                        report.chairs += 1;
                    }
                }
                Kind::Other if cable_group(proto).is_some() => {
                    let group = cable_group(proto).unwrap_or("hv");
                    let key = (cell, group);
                    if cables_seen.insert(key) {
                        let (cx, cz) = (cell.0 as f32, cell.1 as f32);
                        zones.push(MapZone {
                            min: [cx + 0.01, 0.0, cz + 0.01],
                            max: [cx + 0.99, 3.0, cz + 0.99],
                            floor: 0,
                            metadata: [("power_cable".to_string(), group.to_string())].into(),
                        });
                        report.cables += 1;
                    }
                }
                Kind::Other if known_items.contains(proto) => {
                    let links = device_list(entity)
                        .iter()
                        .filter_map(|u| uid_cell.get(u).map(|&(a, b)| [a, b]))
                        .collect::<BTreeSet<[i32; 2]>>()
                        .into_iter()
                        .collect::<Vec<_>>();
                    if !links.is_empty() {
                        report.alarms += 1;
                    }
                    // Шкаф, открытый на карте: берётся прототип открытого вида; замок — как записано на карте.
                    let open_name = format!("{proto}Open");
                    let prototype = if component_bool(entity, "EntityStorage", "open") == Some(true)
                        && known_items.contains(&open_name)
                    {
                        open_name
                    } else {
                        proto.to_string()
                    };
                    let mut meta = BTreeMap::new();
                    if let Some(uid) = entity.get("uid").and_then(Value::as_i64) {
                        meta.insert("ss14_uid".into(), uid.to_string());
                    }
                    // DeviceList references entities, not cells. Retain exact links as well
                    // as the old cell list used by firelocks and piping.
                    let device_ids = device_list(entity);
                    if !device_ids.is_empty() {
                        meta.insert(
                            "device_ids".into(),
                            device_ids
                                .iter()
                                .map(i64::to_string)
                                .collect::<Vec<_>>()
                                .join(";"),
                        );
                    }
                    if let Some(name) = component_text(entity, "MetaData", "name") {
                        meta.insert("name".into(), name);
                    }
                    if let Some(address) = component_text(entity, "DeviceNetwork", "address") {
                        meta.insert("device_address".into(), address);
                    }
                    if let Some(locked) = component_bool(entity, "Lock", "locked") {
                        meta.insert("locked".to_string(), u8::from(locked).to_string());
                    }
                    // Настройки газовых устройств, заданные на карте.
                    if let Some(t) =
                        component_number(entity, "GasThermoMachine", "targetTemperature")
                    {
                        meta.insert("thermo_target".to_string(), t.to_string());
                    }
                    if let Some(g) = component_text(entity, "GasFilter", "filteredGas") {
                        meta.insert("filter_gas".to_string(), g);
                    }
                    items.push(MapItem {
                        prototype,
                        x,
                        z: -y,
                        rot: entity_rot(entity),
                        links,
                        meta,
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
    // SS14 stores wall-mounted entities at the centre of a solid tile. In a 3D
    // edge-wall world that puts both the sprite and its interaction point behind
    // the wall. Keep the source anchor, and place the default 0.3 m object against
    // the source-facing face: half tile + half 0.2 m wall + half object extent.
    for item in items
        .iter_mut()
        .filter(|item| mats.wallmounts.contains(&item.prototype))
    {
        let cell = (item.x.floor() as i32, item.z.floor() as i32);
        let (dx, dz) = [(0, 1), (1, 0), (0, -1), (-1, 0)][item.rot as usize % 4];
        if solid.contains(&cell) && walk.contains(&(cell.0 + dx, cell.1 + dz)) {
            item.meta
                .insert("wallmount_origin".into(), format!("{},{}", item.x, item.z));
            let offset = 0.5 + 0.2 * 0.5 + 0.3 * 0.5;
            item.x = cell.0 as f32 + 0.5 + dx as f32 * offset;
            item.z = cell.1 as f32 + 0.5 + dz as f32 * offset;
        }
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
    let mut fire_edges: BTreeSet<GridEdge> = BTreeSet::new();
    let mut fire_sources: BTreeMap<GridEdge, Vec<i64>> = BTreeMap::new();
    // Огнезапоры стоят в проходах и перекрывают его вместе со стенами и соседними огнезапорами.
    let mut barrier = solid.clone();
    barrier.extend(fire_cells.iter().copied());
    barrier.extend(doors.iter().copied());
    walk.extend(fire_cells.iter().copied());
    let all: Vec<((i32, i32), bool)> = doors
        .iter()
        .map(|d| (*d, false))
        .chain(fire_cells.iter().map(|d| (*d, true)))
        .collect();
    for (d, fire) in all {
        // Огнезапор на клетке двери: проход уже закрыт дверью.
        if fire && doors.contains(&d) {
            continue;
        }
        // Ось, вдоль которой идёт стена: оба соседа на ней — стены; проход — поперёк.
        let wall: &BTreeSet<(i32, i32)> = &barrier;
        let along_x = wall.contains(&(d.0 - 1, d.1)) && wall.contains(&(d.0 + 1, d.1));
        let along_z = wall.contains(&(d.0, d.1 - 1)) && wall.contains(&(d.0, d.1 + 1));
        let candidates = if along_x && !along_z {
            [(d.0, d.1 + 1), (d.0, d.1 - 1)]
        } else if along_z && !along_x {
            [(d.0 + 1, d.1), (d.0 - 1, d.1)]
        } else {
            if fire {
                report.firelocks_unplaced += 1;
            } else {
                report.doors_unplaced += 1;
            }
            continue;
        };
        let edge = candidates
            .into_iter()
            .find(|c| walk.contains(c) && !doors.contains(c) && !fire_cells.contains(c))
            .and_then(|t| GridEdge::between(GridCell::new(d.0, d.1), GridCell::new(t.0, t.1)));
        match (edge, fire) {
            (Some(e), false) => {
                door_edges.insert(e);
                if let Some(l) = door_lists.get(&d) {
                    edge_access.insert(e, l.clone());
                }
            }
            (Some(e), true) if !door_edges.contains(&e) => {
                fire_edges.insert(e);
                fire_sources
                    .entry(e)
                    .or_default()
                    .extend(fire_uids.get(&d).into_iter().flatten().copied());
            }
            (_, true) => report.firelocks_unplaced += 1,
            (None, false) => report.doors_unplaced += 1,
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
    // Стена на карте SS14 занимает всю клетку. Рёбра остаются для логики (воздух, проходимость,
    // двери), но тонкие, а вид и объём даёт сплошной блок на клетку (`Freeform`).
    for e in &wall_edges {
        elements.push(element(
            StructureKind::Wall,
            Placement::Edge(*e),
            &mats.wall,
            WALL_EDGE_THICKNESS_M,
        ));
    }
    let mut wall_blocks = 0usize;
    for &c in &solid {
        let touches_floor = (-1..=1).any(|dx| {
            (-1..=1).any(|dz| (dx, dz) != (0, 0) && walk.contains(&(c.0 + dx, c.1 + dz)))
        });
        if !touches_floor {
            continue;
        }
        elements.push(element(
            StructureKind::Freeform,
            Placement::Free(astrocore_world::FreePlacement {
                cell: GridCell::new(c.0, c.1),
                x_mm: 500,
                z_mm: 500,
                y_mm: (FLOOR_TOP_M * 1000.0) as i32,
                yaw_cdeg: 0,
                half_mm: [500, WALL_HEIGHT_MM, 500],
            }),
            &mats.wall,
            0.1,
        ));
        wall_blocks += 1;
    }
    report.wall_blocks = wall_blocks;
    for e in &door_edges {
        elements.push(element(
            StructureKind::Opening(OpeningKind::Door),
            Placement::Edge(*e),
            &mats.door,
            0.1,
        ));
    }
    for e in &fire_edges {
        let door = element(
            StructureKind::Opening(OpeningKind::Door),
            Placement::Edge(*e),
            &mats.firelock,
            0.1,
        );
        for uid in fire_sources.get(e).into_iter().flatten() {
            zones.push(MapZone {
                min: [e.x as f32 + 0.45, 0.0, e.z as f32 + 0.45],
                max: [e.x as f32 + 0.55, 3.0, e.z as f32 + 0.55],
                floor: 0,
                metadata: [
                    ("ss14_uid".into(), uid.to_string()),
                    (
                        "alarm_firelock".into(),
                        format!("{:x}", door.id.as_uuid().as_u128()),
                    ),
                ]
                .into(),
            });
        }
        elements.push(door);
    }
    report.firelocks = fire_edges.len();
    // Плитка каждой клетки: имя SS14 → материал (в наших координатах z = −y − 1).
    let tile_at: BTreeMap<(i32, i32), &String> =
        tiles.iter().map(|(&(x, y), n)| ((x, -y - 1), n)).collect();
    for &c in &walk {
        let material = tile_at
            .get(&c)
            .and_then(|n| mats.tiles.get(n.as_str()))
            .unwrap_or(&mats.floor);
        elements.push(element(
            StructureKind::Floor,
            Placement::Cell(GridCell::new(c.0, c.1)),
            material,
            0.2,
        ));
    }
    // Мебель-ящики стоят на полу и мешают пройти; на одной клетке ящик один.
    let mut boxed: BTreeSet<(i32, i32)> = BTreeSet::new();
    for (c, m, h) in boxes {
        if !walk.contains(&c) || !boxed.insert(c) {
            continue;
        }
        let Ok(material) = PrototypeId::new(m) else {
            continue;
        };
        elements.push(element(
            StructureKind::Freeform,
            Placement::Free(astrocore_world::FreePlacement {
                cell: GridCell::new(c.0, c.1),
                x_mm: 500,
                z_mm: 500,
                y_mm: (FLOOR_TOP_M * 1000.0) as i32,
                yaw_cdeg: 0,
                half_mm: [430, (h * 1000.0) as u32, 430],
            }),
            &material,
            0.1,
        ));
        report.furniture += 1;
    }
    report.wall_edges = wall_edges.len();
    report.doors = door_edges.len();
    for (e, lists) in &edge_access {
        let text: Vec<String> = lists.iter().map(|l| l.join(",")).collect();
        let dir = match e.dir {
            astrocore_math::EdgeDir::AlongX => "X",
            astrocore_math::EdgeDir::AlongZ => "Z",
        };
        zones.push(MapZone {
            min: [e.x as f32 + 0.45, 0.0, e.z as f32 + 0.45],
            max: [e.x as f32 + 0.55, 3.0, e.z as f32 + 0.55],
            floor: 0,
            metadata: [
                ("door_edge".to_string(), format!("{},{},{dir}", e.x, e.z)),
                ("door_access".to_string(), text.join(";")),
            ]
            .into(),
        });
        report.door_access += 1;
    }

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
        zones,
    };
    Ok((map, report))
}

#[cfg(test)]
mod tests;
