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
//! Не сделано: декали, стены и проёмы.

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
        &[CMD_SPAWN, CMD_DELETE, CMD_TILE]
    }

    fn command(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        kind: &str,
        data: &[u8],
    ) {
        let flag = if kind == CMD_TILE { TILE_FLAG } else { FLAG };
        if !st.allowed(actor, Some(flag)) {
            st.popup(actor, format!("Нет прав на маппинг (нужен флаг {flag})"));
            return;
        }
        let result = match kind {
            CMD_SPAWN => self.spawn(st, ctx, data),
            CMD_TILE => self.tiles(st, ctx, data),
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
