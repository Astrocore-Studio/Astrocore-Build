//! Маппинг (админ-инструменты): поставить любую сущность в точку мира и удалить объект. Как
//! окно спавна сущностей SS14 (`EntitySpawnWindow`): клиент выбирает прототип в каталоге и
//! щёлкает по миру, сервер проверяет права (флаг `Spawn`), прототип и точку.
//!
//! Предмет (у прототипа есть описание предмета) создаётся как вещь на полу — его можно взять;
//! остальное (мебель, устройства) — размещение мира по прототипу, повёрнутое по `rot`; системы,
//! которые следят за размещениями (автоматы, капсулы, шкафы…), подхватывают его сами.
//!
//! Не сделано: тайлы, декали, стены и проёмы, сохранение карты.

use astrocore_api::HookContext;
use astrocore_core::{NetworkEntityId, ObjectId, PrototypeId};
use astrocore_math::glam::Vec3;
use astrocore_world::{AuthoredKind, AuthoredObject, WorldCommand};
use serde::{Deserialize, Serialize};

use crate::module::State;
use crate::systems::GameSystem;

pub const CMD_SPAWN: &str = "mapping.spawn";
pub const CMD_DELETE: &str = "mapping.delete";
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
        &[CMD_SPAWN, CMD_DELETE]
    }

    fn command(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        kind: &str,
        data: &[u8],
    ) {
        if !st.allowed(actor, Some(FLAG)) {
            st.popup(actor, "Нет прав на маппинг (нужен флаг Spawn)");
            return;
        }
        let result = if kind == CMD_SPAWN {
            self.spawn(st, ctx, data)
        } else {
            Self::delete(ctx, data)
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
        self.serial += 1;
        let id = ObjectId::from_u128(0xAD31_0000_0000_0000 + self.serial);
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
