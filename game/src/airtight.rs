//! Непроницаемость для газа: стены, закрытые двери и окна перекрывают клетку. Образец —
//! `AirtightComponent` и `AirtightSystem.SetAirblocked` SS14 @ 2f4a2aad.
//!
//! Пока атмосферы нет (5.5), компонент только хранит состояние и сообщает об изменении
//! событием [`AirtightChanged`]; атмосфера подпишется на него.

use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;

fn yes() -> bool {
    true
}

/// Непроницаемость (`AirtightComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Airtight {
    /// Перекрытые направления (`airBlockedDirection`): имена флагов `AtmosDirection`.
    #[serde(default)]
    pub air_blocked_direction: Option<AirBlockedDirection>,
    #[serde(default = "yes")]
    pub air_blocked: bool,
    #[serde(default = "yes")]
    pub fix_vacuum: bool,
    #[serde(default = "yes")]
    pub rotate_air_blocked: bool,
    #[serde(default = "yes")]
    pub no_air_when_fully_air_blocked: bool,
}

/// Направления в данных: одно имя или список имён.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AirBlockedDirection {
    One(String),
    Many(Vec<String>),
}

/// Непроницаемость изменилась (`AirtightChanged`). Поднимается на сущности.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AirtightChanged {
    pub air_blocked: bool,
}

/// Перекрыть или открыть клетку (`SetAirblocked`).
pub fn set_airblocked(world: &mut World, entity: Entity, blocked: bool) {
    let Some(mut airtight) = world.get_mut::<Airtight>(entity) else {
        return;
    };
    if airtight.air_blocked == blocked {
        return;
    }
    airtight.air_blocked = blocked;
    world.raise_and_broadcast(
        entity,
        &mut AirtightChanged {
            air_blocked: blocked,
        },
    );
}

pub fn install(world: &mut World, _side: Side) {
    world.register_data_component::<Airtight>();
}
