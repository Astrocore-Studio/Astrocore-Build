//! Вид светильников и ламп на клиенте: рисунок основы по состоянию лампы, свечение только
//! у горящей лампы и цвета её света; рисунок и цвет лампы-предмета. Образец —
//! `PoweredLightVisualizerSystem` и `LightBulbSystem` клиента SS14 @ 2f4a2aad. Мигания нет.

use std::collections::BTreeMap;

use astrocore_client::Sprite;
use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_light::PointLight;
use astrocore_math::Color;
use astrocore_net::{AppearanceChange, AppearanceValue, AppearanceWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::lights::{BULB_STATE, LAMP_COLOR, LAMP_STATE, LightBulb};

/// Основа светильника (`PoweredLightLayers.Base`).
pub const LAYER_BASE: &str = "enum.PoweredLightLayers.Base";
/// Свечение (`PoweredLightLayers.Glow`).
pub const LAYER_GLOW: &str = "enum.PoweredLightLayers.Glow";
/// Основа лампы (`LightBulbVisualLayers.Base`).
pub const LAMP_LAYER: &str = "enum.LightBulbVisualLayers.Base";

/// Рисунки светильника по состоянию лампы (`PoweredLightVisualsComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoweredLightVisuals {
    /// Ключи — `empty`, `off`, `on`, `broken`, `burned`.
    #[serde(default)]
    pub sprite_state_map: BTreeMap<String, String>,
}

fn text(world: &World, entity: Entity, key: &str) -> Option<String> {
    match world.appearance(entity, key) {
        Some(AppearanceValue::Text(value)) => Some(value.clone()),
        _ => None,
    }
}

fn update_light(world: &mut World, entity: Entity) {
    let Some(visuals) = world.get::<PoweredLightVisuals>(entity).cloned() else {
        return;
    };
    let Some(state) = text(world, entity, BULB_STATE) else {
        return;
    };
    let color = world.get::<PointLight>(entity).map(|l| l.color);
    let Some(mut sprite) = world.get::<Sprite>(entity).cloned() else {
        return;
    };
    let before = sprite.clone();
    if let Some(rsi_state) = visuals.sprite_state_map.get(&state.to_lowercase())
        && let Some(index) = sprite.layer_index(LAYER_BASE)
    {
        sprite.layers[index].state = Some(rsi_state.clone());
    }
    if let Some(index) = sprite.layer_index(LAYER_GLOW) {
        if let Some(color) = color {
            sprite.layers[index].color = color;
        }
        sprite.layers[index].visible = state == "On";
    }
    if sprite != before
        && let Some(mut current) = world.get_mut::<Sprite>(entity)
    {
        *current = sprite;
    }
}

fn update_lamp(world: &mut World, entity: Entity) {
    let Some(lamp) = world.get::<LightBulb>(entity).cloned() else {
        return;
    };
    let state = text(world, entity, LAMP_STATE);
    let color = text(world, entity, LAMP_COLOR).and_then(|c| Color::parse(&c));
    let Some(mut sprite) = world.get::<Sprite>(entity).cloned() else {
        return;
    };
    let before = sprite.clone();
    if let Some(state) = state
        && let Some(index) = sprite.layer_index(LAMP_LAYER)
    {
        let rsi_state = match state.as_str() {
            "Broken" => &lamp.broken_sprite_state,
            "Burned" => &lamp.burned_sprite_state,
            _ => &lamp.normal_sprite_state,
        };
        sprite.layers[index].state = Some(rsi_state.clone());
    }
    if let Some(color) = color {
        sprite.color = color;
    }
    if sprite != before
        && let Some(mut current) = world.get_mut::<Sprite>(entity)
    {
        *current = sprite;
    }
}

pub(crate) fn install(world: &mut World) {
    world.register_data_component::<PoweredLightVisuals>();
    world.subscribe::<PoweredLightVisuals, AppearanceChange>(|world, entity, _| {
        update_light(world, entity)
    });
    world.subscribe::<LightBulb, AppearanceChange>(|world, entity, _| update_lamp(world, entity));
}
