//! Спрайт шкафа по состоянию: дверца открыта или закрыта. Образец —
//! `EntityStorageVisualizerSystem` клиента SS14 @ 2f4a2aad.

use astrocore_client::{DrawDepth, Sprite};
use astrocore_ecs::{ComponentInit, EventWorldExt, RegistryWorldExt};
use astrocore_net::{AppearanceChange, AppearanceValue, AppearanceWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::entity_storage::VISUALS_OPEN;

/// Слой основы (`StorageVisualLayers.Base`).
pub const LAYER_BASE: &str = "enum.StorageVisualLayers.Base";
/// Слой дверцы (`StorageVisualLayers.Door`).
pub const LAYER_DOOR: &str = "enum.StorageVisualLayers.Door";

/// Состояния спрайта шкафа. В данных — компонент `EntityStorageVisuals`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityStorageVisuals {
    #[serde(default)]
    pub state_base_closed: Option<String>,
    #[serde(default)]
    pub state_base_open: Option<String>,
    #[serde(default)]
    pub state_door_open: Option<String>,
    #[serde(default)]
    pub state_door_closed: Option<String>,
    #[serde(default)]
    pub open_draw_depth: Option<DrawDepth>,
    #[serde(default)]
    pub closed_draw_depth: Option<DrawDepth>,
}

fn set_state(sprite: &mut Sprite, key: &str, state: &str) {
    if let Some(index) = sprite.layer_index(key) {
        sprite.layers[index].state = Some(state.to_owned());
    }
}

fn set_visible(sprite: &mut Sprite, key: &str, visible: bool) {
    if let Some(index) = sprite.layer_index(key) {
        sprite.layers[index].visible = visible;
    }
}

fn apply(world: &mut World, entity: Entity) {
    let Some(visuals) = world.get::<EntityStorageVisuals>(entity).cloned() else {
        return;
    };
    let Some(AppearanceValue::Bool(open)) = world.appearance(entity, VISUALS_OPEN).cloned() else {
        return;
    };
    let Some(sprite) = world.get::<Sprite>(entity) else {
        return;
    };
    let mut updated = sprite.clone();
    if updated.layer_index(LAYER_DOOR).is_none() {
        return;
    }
    let (depth, door, base) = if open {
        (
            visuals.open_draw_depth,
            visuals.state_door_open,
            visuals.state_base_open,
        )
    } else {
        (
            visuals.closed_draw_depth,
            visuals.state_door_closed,
            visuals.state_base_closed,
        )
    };
    if let Some(depth) = depth {
        updated.drawdepth = depth;
    }
    match door {
        Some(state) => {
            set_state(&mut updated, LAYER_DOOR, &state);
            set_visible(&mut updated, LAYER_DOOR, true);
        }
        None => set_visible(&mut updated, LAYER_DOOR, false),
    }
    if let Some(state) = base {
        set_state(&mut updated, LAYER_BASE, &state);
    }
    if updated != *sprite
        && let Some(mut sprite) = world.get_mut::<Sprite>(entity)
    {
        *sprite = updated;
    }
}

pub(crate) fn install(world: &mut World) {
    world.register_data_component::<EntityStorageVisuals>();
    world.subscribe::<EntityStorageVisuals, ComponentInit>(|world, entity, _| {
        let Some(mut visuals) = world.get_mut::<EntityStorageVisuals>(entity) else {
            return;
        };
        let Some(closed) = visuals.state_base_closed.clone() else {
            return;
        };
        visuals.state_base_open.get_or_insert(closed.clone());
        if let Some(mut sprite) = world.get_mut::<Sprite>(entity) {
            set_state(&mut sprite, LAYER_BASE, &closed);
        }
    });
    world.subscribe::<EntityStorageVisuals, AppearanceChange>(|world, entity, _| {
        apply(world, entity);
    });
}
