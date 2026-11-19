//! Маппинг (админ-инструменты): поставить любую сущность в точку мира и удалить объект. Как
//! окно спавна сущностей SS14 (`EntitySpawnWindow`): клиент выбирает прототип в каталоге и
//! щёлкает по миру, сервер проверяет права (флаг `Spawn`), прототип и точку.
//!
//! Предмет (у прототипа есть описание предмета) создаётся как вещь на полу — его можно взять;
//! остальное (мебель, устройства) — размещение мира по прототипу, повёрнутое по `rot`; системы,
//! которые следят за размещениями (автоматы, капсулы, шкафы…), подхватывают его сами.
//!
//! Плитки (`mapping.tile`, флаг `Mapping`, как `TileSpawnWindow` SS14): пол — и, по флажку,
//! потолок из той же плитки — на прямоугольник клеток текущего уровня; пустая плитка убирает пол
//! и потолок. Уровень, которого ещё нет, создаётся сам (ADR-0003).
//!
//! Стены, окна, решётки и шлюзы (`mapping.structure`, флаг `Mapping`) — сущности каталога, как в
//! SS14: сервер переводит их в конструкции движка. Стена, окно, решётка — два куба 1 × 1 × 1 м на
//! клетку и рёбра вокруг (как постройка в игре); ставятся линией протяжкой. Шлюз — проём двери на
//! ребре клетки по повороту и служебная зона шлюза (плата, провода, доступ), как у импортёра.
//!
//! Не сделано: декали.

use astrocore_api::HookContext;
use astrocore_core::{NetworkEntityId, ObjectId, PrototypeId};
use astrocore_math::GridCell;
use astrocore_math::glam::Vec3;
use astrocore_world::{
    AuthoredKind, AuthoredObject, FloorDef, FloorId, Placement, SlotKey, StructureElement,
    StructureKind, WorldCommand,
};
use serde::{Deserialize, Serialize};

use crate::module::State;
use crate::systems::GameSystem;

pub const CMD_SPAWN: &str = "mapping.spawn";
pub const CMD_DELETE: &str = "mapping.delete";
pub const CMD_TILE: &str = "mapping.tile";
pub const CMD_STRUCTURE: &str = "mapping.structure";
/// Ластик: всё, кроме пола и потолка, в прямоугольнике клеток (флаг `Mapping`).
pub const CMD_ERASE: &str = "mapping.erase";
/// Самая длинная линия стен за одну протяжку, клеток.
pub const MAX_LINE_CELLS: usize = 64;
/// Флаг прав для плиток (и других правок конструкции карты).
pub const TILE_FLAG: &str = "Mapping";
/// Самый большой прямоугольник плиток за один запрос, клеток.
pub const MAX_TILE_CELLS: i64 = 32 * 32;
/// Толщина плиты пола, м (верх пола — `maps::FLOOR_TOP_M`), и потолка.
const FLOOR_THICKNESS_M: f32 = maps::FLOOR_TOP_M;
const CEILING_THICKNESS_M: f32 = 0.1;
/// Флаг прав, нужный для маппинга (как у окна спавна SS14).
pub const FLAG: &str = "Spawn";
/// Половина стороны размещения не-предмета, м (занимает почти весь тайл).
const HALF_M: f32 = 0.45;
/// Высота размещения не-предмета, м.
const HEIGHT_M: f32 = 1.0;
const MAX_REQUEST_BYTES: usize = 256;

/// Запрос «поставить»: прототип (идентификатор движка, `ss14:…`), точка ступней и поворот
/// четвертями оборота.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnRequest {
    pub proto: String,
    pub at: [f32; 3],
    #[serde(default)]
    pub rot: u8,
}

pub fn spawn_data(r: &SpawnRequest) -> Vec<u8> {
    serde_json::to_vec(r).unwrap_or_default()
}

/// Запрос плиток: углы прямоугольника (клетки X/Z, включительно), уровень и плитка —
/// материал движка (`ss14:tile_…`) или `None`, чтобы убрать пол и потолок.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileRequest {
    pub tile: Option<String>,
    pub from: [i32; 2],
    pub to: [i32; 2],
    #[serde(default)]
    pub level: i32,
    /// Положить и потолок из той же плитки.
    #[serde(default)]
    pub ceiling: bool,
}

pub fn tile_data(r: &TileRequest) -> Vec<u8> {
    serde_json::to_vec(r).unwrap_or_default()
}

/// Запрос постройки: прототип SS14 (`WallSolid`, `Window`, `AirlockEngineering`…), линия клеток
/// (для шлюза — первая клетка), поворот шлюза и уровень.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructureRequest {
    pub proto: String,
    pub from: [i32; 2],
    pub to: [i32; 2],
    #[serde(default)]
    pub rot: u8,
    #[serde(default)]
    pub level: i32,
}

/// Запрос ластика: углы прямоугольника клеток (включительно) и уровень.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EraseRequest {
    pub from: [i32; 2],
    pub to: [i32; 2],
    #[serde(default)]
    pub level: i32,
}

pub fn erase_data(r: &EraseRequest) -> Vec<u8> {
    serde_json::to_vec(r).unwrap_or_default()
}

pub fn structure_data(r: &StructureRequest) -> Vec<u8> {
    serde_json::to_vec(r).unwrap_or_default()
}

/// Конструкция каталога маппинга: прототип SS14, материал движка (по нему — значок) и шлюз ли.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureEntry {
    pub proto: String,
    pub material: String,
    pub airlock: bool,
}

/// Описания шлюзов: прототип → материал, узел постройки, плата, провода.
fn airlocks() -> &'static serde_json::Value {
    static AIRLOCKS: std::sync::LazyLock<serde_json::Value> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!(
            "../../../../content/engineering-airlocks.json"
        ))
        .expect("описания шлюзов")
    });
    &AIRLOCKS
}

/// Что можно строить: стены, окна и решётки из графов постройки SS14 и шлюзы, по имени.
pub fn structure_catalog() -> Vec<StructureEntry> {
    let mut out: Vec<StructureEntry> = crate::crafting::CATALOG["barriers"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| name.as_str() != "InflatableWall")
        .filter_map(|(name, _)| {
            Some(StructureEntry {
                proto: name.clone(),
                material: prototypes::engine_id("ss14", name)?.as_str().to_string(),
                airlock: false,
            })
        })
        .collect();
    out.extend(
        airlocks()
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(name, def)| {
                Some(StructureEntry {
                    proto: name.clone(),
                    material: def["material"].as_str()?.to_string(),
                    airlock: true,
                })
            }),
    );
    out.sort_by(|a, b| a.proto.cmp(&b.proto));
    out
}

/// Клетки линии протяжки: от `from` вдоль оси, по которой сдвиг больше (как линия в SS14).
pub fn line_cells(from: [i32; 2], to: [i32; 2]) -> Vec<[i32; 2]> {
    let (dx, dz) = (to[0] - from[0], to[1] - from[1]);
    let (axis, len) = if dx.abs() >= dz.abs() {
        (0, dx)
    } else {
        (1, dz)
    };
    let step = len.signum();
    (0..=len.abs())
        .take(MAX_LINE_CELLS)
        .map(|i| {
            let mut c = from;
            c[axis] += i * step;
            c
        })
        .collect()
}

pub fn delete_data(id: ObjectId) -> Vec<u8> {
    id.as_uuid().as_u128().to_le_bytes().to_vec()
}

#[derive(Default)]
pub struct MappingSystem {
    serial: u128,
}

impl GameSystem for MappingSystem {
    fn id(&self) -> &'static str {
        "mapping"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[CMD_SPAWN, CMD_DELETE, CMD_TILE, CMD_STRUCTURE, CMD_ERASE]
    }

    fn command(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        kind: &str,
        data: &[u8],
    ) {
        let flag = if matches!(kind, CMD_TILE | CMD_STRUCTURE | CMD_ERASE) {
            TILE_FLAG
        } else {
            FLAG
        };
        if !st.allowed(actor, Some(flag)) {
            st.popup(actor, format!("Нет прав на маппинг (нужен флаг {flag})"));
            return;
        }
        let result = match kind {
            CMD_SPAWN => self.spawn(st, ctx, data),
            CMD_TILE => self.tiles(st, ctx, data),
            CMD_STRUCTURE => self.structure(st, ctx, data),
            CMD_ERASE => Self::erase(st, ctx, data),
            _ => Self::delete(ctx, data),
        };
        if let Err(text) = result {
            st.popup(actor, text);
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl MappingSystem {
    fn spawn(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        data: &[u8],
    ) -> Result<(), String> {
        if data.len() > MAX_REQUEST_BYTES {
            return Err("Слишком длинный запрос".into());
        }
        let r: SpawnRequest =
            serde_json::from_slice(data).map_err(|_| "Неверный запрос".to_string())?;
        if r.at.iter().any(|v| !v.is_finite()) {
            return Err("Неверная точка".into());
        }
        let proto = PrototypeId::new(r.proto.as_str()).map_err(|_| "Неверный прототип")?;
        let name = st
            .ss14_names
            .get(&proto)
            .cloned()
            .ok_or_else(|| format!("Нет прототипа {}", r.proto))?;
        if !st.spawnable.contains(&proto) {
            return Err(format!("{name} нельзя создать (абстрактный)"));
        }
        let at = Vec3::from_array(r.at);
        let floor = crate::module::floor_at(ctx, at.y).ok_or("Здесь нет этажа")?;
        // Предмет — вещью на полу (её можно взять).
        if st.info(&proto).is_some() {
            let item = st
                .create_item(&name)
                .ok_or_else(|| format!("{name} не создаётся"))?;
            if !st.place(ctx, &item, at) {
                return Err("Не удалось положить".into());
            }
            return Ok(());
        }
        let id = self.fresh_id(ctx);
        let mut obj = AuthoredObject::new(
            id,
            AuthoredKind::Placement { prototype: proto },
            floor,
            Vec3::new(at.x - HALF_M, at.y, at.z - HALF_M),
            Vec3::new(at.x + HALF_M, at.y + HEIGHT_M, at.z + HALF_M),
        );
        if !r.rot.is_multiple_of(4) {
            obj.metadata.insert("rot".into(), (r.rot % 4).to_string());
        }
        ctx.request(WorldCommand::PlaceAuthored(obj));
        Ok(())
    }

    /// Новый идентификатор, которого нет в мире: после `loadmap` счётчик начинается заново, а
    /// карта уже может содержать такие идентификаторы.
    fn fresh_id(&mut self, ctx: &HookContext<'_>) -> ObjectId {
        loop {
            self.serial += 1;
            let id = ObjectId::from_u128(0xAD31_0000_0000_0000 + self.serial);
            if ctx.world.element(id).is_none() && ctx.world.authored_object(id).is_none() {
                return id;
            }
        }
    }

    fn tiles(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        data: &[u8],
    ) -> Result<(), String> {
        if data.len() > MAX_REQUEST_BYTES {
            return Err("Слишком длинный запрос".into());
        }
        let r: TileRequest =
            serde_json::from_slice(data).map_err(|_| "Неверный запрос".to_string())?;
        let material = match &r.tile {
            Some(t) => {
                let id = PrototypeId::new(t.as_str()).map_err(|_| "Неверная плитка")?;
                if !st.tiles.contains(&id) {
                    return Err(format!("Нет плитки {t}"));
                }
                Some(id)
            }
            None => None,
        };
        let (x0, x1) = (r.from[0].min(r.to[0]), r.from[0].max(r.to[0]));
        let (z0, z1) = (r.from[1].min(r.to[1]), r.from[1].max(r.to[1]));
        let cells = (i64::from(x1) - i64::from(x0) + 1) * (i64::from(z1) - i64::from(z0) + 1);
        if cells > MAX_TILE_CELLS {
            return Err(format!(
                "Слишком большой прямоугольник: не больше {MAX_TILE_CELLS} клеток"
            ));
        }
        // Плитки ложатся в мир карты, где стоит маппер (хук вызван в её рамках, ADR-0013).
        if !(-50..50).contains(&r.level) {
            return Err("Нет такого уровня".into());
        }
        let floor = FloorId(r.level);
        if material.is_some() && ctx.world.floor(floor).is_none() {
            ctx.request(WorldCommand::DefineFloor(level_def(ctx, r.level)));
        }
        for x in x0..=x1 {
            for z in z0..=z1 {
                let cell = GridCell::new(x, z);
                for key in [SlotKey::FloorSlab(cell), SlotKey::CeilingSlab(cell)] {
                    if let Some(old) = ctx.world.element_at(floor, key) {
                        let id = old.id;
                        ctx.request(WorldCommand::RemoveElement { id });
                    }
                }
                let Some(material) = &material else {
                    continue;
                };
                let mut slabs = vec![(StructureKind::Floor, FLOOR_THICKNESS_M)];
                if r.ceiling {
                    slabs.push((StructureKind::Ceiling, CEILING_THICKNESS_M));
                }
                for (kind, thickness) in slabs {
                    let id = self.fresh_id(ctx);
                    ctx.request(WorldCommand::PlaceElement(StructureElement {
                        id,
                        kind,
                        placement: Placement::Cell(cell),
                        floor,
                        material: material.clone(),
                        thickness,
                    }));
                }
            }
        }
        Ok(())
    }

    fn structure(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        data: &[u8],
    ) -> Result<(), String> {
        if data.len() > MAX_REQUEST_BYTES {
            return Err("Слишком длинный запрос".into());
        }
        let r: StructureRequest =
            serde_json::from_slice(data).map_err(|_| "Неверный запрос".to_string())?;
        if !(-50..50).contains(&r.level) {
            return Err("Нет такого уровня".into());
        }
        let floor = FloorId(r.level);
        if ctx.world.floor(floor).is_none() {
            ctx.request(WorldCommand::DefineFloor(level_def(ctx, r.level)));
        }
        if let Some(def) = airlocks().get(r.proto.as_str()) {
            return self.airlock(ctx, &r, floor, def);
        }
        if crate::crafting::CATALOG["barriers"]
            .get(r.proto.as_str())
            .is_none()
            || r.proto == "InflatableWall"
        {
            return Err(format!("{} не строится маппингом", r.proto));
        }
        let material = prototypes::engine_id("ss14", &r.proto).ok_or("Нет материала")?;
        for [x, z] in line_cells(r.from, r.to) {
            let cell = (x, z);
            let walls = st.take_walls(ctx);
            let taken = walls.contains_key(&cell);
            st.walls = Some(walls);
            if !taken {
                st.build_barrier(ctx, floor, cell, &r.proto, material.clone());
            }
        }
        Ok(())
    }

    /// Шлюз на клетке: проём двери на ребре по повороту (`rot`: 0 — к +Z, 1 — к +X, 2 — к −Z,
    /// 3 — к −X) и служебная зона шлюза, как у импортёра карт.
    fn airlock(
        &mut self,
        ctx: &mut HookContext<'_>,
        r: &StructureRequest,
        floor: FloorId,
        def: &serde_json::Value,
    ) -> Result<(), String> {
        let [x, z] = r.from;
        let (dx, dz) = [(0, 1), (1, 0), (0, -1), (-1, 0)][usize::from(r.rot % 4)];
        let edge =
            astrocore_math::GridEdge::between(GridCell::new(x, z), GridCell::new(x + dx, z + dz))
                .ok_or("Нет ребра")?;
        if ctx.world.edge_element(floor, edge).is_some() {
            return Err("Здесь уже стена или дверь".into());
        }
        let material = def["material"]
            .as_str()
            .and_then(|m| PrototypeId::new(m).ok())
            .ok_or("У шлюза нет материала")?;
        let door = self.fresh_id(ctx);
        ctx.request(WorldCommand::PlaceElement(StructureElement {
            id: door,
            kind: StructureKind::Opening(astrocore_world::OpeningKind::Door),
            placement: Placement::Edge(edge),
            floor,
            material,
            thickness: 0.1,
        }));
        let base = ctx
            .world
            .floor(floor)
            .map_or(floor.0 as f32 * maps::LEVEL_HEIGHT_M, |f| f.base_y)
            + maps::FLOOR_TOP_M;
        let (cx, cz) = (x as f32 + 0.5, z as f32 + 0.5);
        let text = |k: &str| def[k].as_str().unwrap_or_default().to_string();
        let mut zone = AuthoredObject::new(
            self.fresh_id(ctx),
            AuthoredKind::Zone,
            floor,
            Vec3::new(cx - 0.35, base, cz - 0.35),
            Vec3::new(cx + 0.35, base + 0.8, cz + 0.35),
        );
        zone.metadata = [
            ("construction_graph".to_string(), "Airlock".to_string()),
            ("construction_node".into(), text("node")),
            ("airlock_proto".into(), r.proto.clone()),
            ("wire_layout".into(), text("wires")),
            ("installed_access".into(), String::new()),
            ("frame_board".into(), text("board")),
            ("rot".into(), (r.rot % 4).to_string()),
            (
                "door_element".into(),
                format!("{:x}", door.as_uuid().as_u128()),
            ),
            ("welded".into(), "0".into()),
            ("bolted".into(), "0".into()),
            ("panel_open".into(), "0".into()),
        ]
        .into();
        ctx.request(WorldCommand::PlaceAuthored(zone));
        Ok(())
    }

    /// Ластик (`Eraser` окна спавна SS14): в прямоугольнике убираются сущности и предметы,
    /// стены, окна и решётки (их рёбра — если по другую сторону не стена), шлюзы с их зонами,
    /// огнезапоры и маркеры появления. Пол и потолок остаются — для них «пустая плитка».
    fn erase(st: &mut State, ctx: &mut HookContext<'_>, data: &[u8]) -> Result<(), String> {
        if data.len() > MAX_REQUEST_BYTES {
            return Err("Слишком длинный запрос".into());
        }
        let r: EraseRequest =
            serde_json::from_slice(data).map_err(|_| "Неверный запрос".to_string())?;
        let (x0, x1) = (r.from[0].min(r.to[0]), r.from[0].max(r.to[0]));
        let (z0, z1) = (r.from[1].min(r.to[1]), r.from[1].max(r.to[1]));
        if (i64::from(x1) - i64::from(x0) + 1) * (i64::from(z1) - i64::from(z0) + 1)
            > MAX_TILE_CELLS
        {
            return Err(format!(
                "Слишком большой прямоугольник: не больше {MAX_TILE_CELLS} клеток"
            ));
        }
        let floor = FloorId(r.level);
        let inside = |x: i32, z: i32| (x0..=x1).contains(&x) && (z0..=z1).contains(&z);
        let cell_of = |p: Vec3| (p.x.floor() as i32, p.z.floor() as i32);
        let mut gone: Vec<WorldCommand> = Vec::new();
        // Сущности и предметы, шлюзы (зона и её дверь), маркеры появления.
        for o in ctx.world.authored().filter(|o| o.floor == floor) {
            let (x, z) = cell_of((o.min() + o.max()) * 0.5);
            if !inside(x, z) {
                continue;
            }
            match &o.kind {
                AuthoredKind::Placement { .. } => {}
                AuthoredKind::Zone => {
                    let door = o
                        .metadata
                        .get("door_element")
                        .and_then(|h| u128::from_str_radix(h, 16).ok())
                        .map(ObjectId::from_u128);
                    if door.is_none() && !o.metadata.contains_key("spawn_job") {
                        continue;
                    }
                    if let Some(id) = door.filter(|d| ctx.world.element(*d).is_some()) {
                        gone.push(WorldCommand::RemoveElement { id });
                    }
                }
                _ => continue,
            }
            gone.push(WorldCommand::RemoveAuthored { id: o.id });
        }
        // Стены, окна, решётки: блоки клеток.
        let mut walls = st.take_walls(ctx);
        let mut erased: std::collections::BTreeSet<(i32, i32)> = Default::default();
        for e in ctx.world.elements().filter(|e| e.floor == floor) {
            if let (StructureKind::Freeform, Placement::Free(p)) = (&e.kind, &e.placement)
                && inside(p.cell.x, p.cell.z)
            {
                erased.insert((p.cell.x, p.cell.z));
                gone.push(WorldCommand::RemoveElement { id: e.id });
            }
        }
        for c in &erased {
            walls.remove(c);
            st.window_grilles.remove(c);
            st.barrier_progress.remove(c);
            st.reinforced_work.remove(c);
        }
        // Рёбра стёртых стен — если по другую сторону не осталась стена; огнезапоры у клеток.
        for e in ctx.world.elements().filter(|e| e.floor == floor) {
            let Placement::Edge(g) = e.placement else {
                continue;
            };
            let (a, b) = match g.dir {
                astrocore_math::EdgeDir::AlongX => ((g.x, g.z - 1), (g.x, g.z)),
                astrocore_math::EdgeDir::AlongZ => ((g.x - 1, g.z), (g.x, g.z)),
            };
            let remove = match e.kind {
                StructureKind::Wall
                | StructureKind::Opening(astrocore_world::OpeningKind::Window) => {
                    (erased.contains(&a) && !walls.contains_key(&b))
                        || (erased.contains(&b) && !walls.contains_key(&a))
                }
                StructureKind::Opening(astrocore_world::OpeningKind::Door) => {
                    e.material.as_str().ends_with("firelock")
                        && (inside(a.0, a.1) || inside(b.0, b.1))
                }
                _ => false,
            };
            if remove
                && !gone
                    .iter()
                    .any(|c| matches!(c, WorldCommand::RemoveElement { id } if *id == e.id))
            {
                gone.push(WorldCommand::RemoveElement { id: e.id });
            }
        }
        st.walls = Some(walls);
        for c in gone {
            ctx.request(c);
        }
        Ok(())
    }

    fn delete(ctx: &mut HookContext<'_>, data: &[u8]) -> Result<(), String> {
        let bytes: [u8; 16] = data.try_into().map_err(|_| "Неверный запрос".to_string())?;
        let id = ObjectId::from_u128(u128::from_le_bytes(bytes));
        match ctx.world.authored_object(id).map(|o| &o.kind) {
            Some(AuthoredKind::Placement { .. }) => {
                ctx.request(WorldCommand::RemoveAuthored { id });
                Ok(())
            }
            Some(_) => Err("Это не сущность (зону и маркеры так не удалить)".into()),
            None => Err("Такого объекта нет".into()),
        }
    }
}

/// Неявный уровень (ADR-0003): над нижним, если он есть, под верхним — если есть он, иначе на
/// высоте номера уровня.
fn level_def(ctx: &HookContext<'_>, level: i32) -> FloorDef {
    let base_y = if let Some(below) = ctx.world.floor(FloorId(level - 1)) {
        below.top_y()
    } else if let Some(above) = ctx.world.floor(FloorId(level + 1)) {
        above.base_y - maps::LEVEL_HEIGHT_M
    } else {
        level as f32 * maps::LEVEL_HEIGHT_M
    };
    FloorDef {
        id: FloorId(level),
        name: format!("Уровень {level}"),
        base_y,
        height: maps::LEVEL_HEIGHT_M,
    }
}
