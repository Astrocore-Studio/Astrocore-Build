//! Вид существа по состоянию: спрайт мёртвого или в критическом состоянии и поворот
//! лежащего. Образец — `DamageStateVisualizerSystem` и `RotationVisualizerSystem` клиента
//! SS14 @ 2f4a2aad. Поворот ставится сразу, без анимации.

use std::collections::BTreeMap;

use astrocore_client::{DrawDepth, Sprite};
use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_math::Angle;
use astrocore_net::{AppearanceChange, AppearanceValue, AppearanceWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::mobs::{MobState, VISUALS_STATE};
use crate::standing::VISUALS_ROTATION;

/// Глубина рисования мёртвых (`DrawDepth.DeadMobs`).
const DEAD_MOBS: i32 = -10;

/// Слои по состояниям. В данных — компонент `DamageStateVisuals`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DamageStateVisuals {
    #[serde(default)]
    pub states: BTreeMap<MobState, BTreeMap<String, String>>,
    #[serde(skip)]
    pub original_draw_depth: Option<DrawDepth>,
}

fn quarter() -> Angle {
    Angle::degrees(90.0)
}

fn animation_time() -> f32 {
    0.125
}

/// Поворот спрайта стоя и лёжа. В данных — компонент `RotationVisuals`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationVisuals {
    #[serde(default = "quarter")]
    pub default_rotation: Angle,
    #[serde(default)]
    pub vertical_rotation: Angle,
    #[serde(default = "quarter")]
    pub horizontal_rotation: Angle,
    #[serde(default = "animation_time")]
    pub animation_time: f32,
}

fn layer_key(key: &str) -> String {
    format!("enum.DamageStateVisualLayers.{key}")
}

fn state_of(world: &World, entity: Entity) -> Option<MobState> {
    match world.appearance(entity, VISUALS_STATE)? {
        AppearanceValue::Text(text) => match text.as_str() {
            "Alive" => Some(MobState::Alive),
            "Critical" => Some(MobState::Critical),
            "Dead" => Some(MobState::Dead),
            _ => None,
        },
        _ => None,
    }
}

fn apply_damage_state(world: &mut World, entity: Entity) {
    let (Some(visuals), Some(state)) = (
        world.get::<DamageStateVisuals>(entity).cloned(),
        state_of(world, entity),
    ) else {
        return;
    };
    let Some(layers) = visuals.states.get(&state) else {
        return;
    };
    let Some(sprite) = world.get::<Sprite>(entity).cloned() else {
        return;
    };
    let mut updated = sprite.clone();
    for key in ["Base", "BaseUnshaded"] {
        if let Some(index) = updated.layer_index(&layer_key(key)) {
            updated.layers[index].visible = false;
        }
    }
    for (key, rsi_state) in layers {
        if let Some(index) = updated.layer_index(&layer_key(key)) {
            updated.layers[index].visible = true;
            updated.layers[index].state = Some(rsi_state.clone());
        }
    }
    let mut original = visuals.original_draw_depth;
    if state == MobState::Dead {
        if updated.drawdepth.0 > DEAD_MOBS {
            original = Some(updated.drawdepth);
            updated.drawdepth = DrawDepth(DEAD_MOBS);
        }
    } else if let Some(depth) = original.take() {
        updated.drawdepth = depth;
    }
    if let Some(mut visuals) = world.get_mut::<DamageStateVisuals>(entity) {
        visuals.original_draw_depth = original;
    }
    if updated != sprite
        && let Some(mut sprite) = world.get_mut::<Sprite>(entity)
    {
        *sprite = updated;
    }
}

fn apply_rotation(world: &mut World, entity: Entity) {
    let Some(visuals) = world.get::<RotationVisuals>(entity).cloned() else {
        return;
    };
    let horizontal = matches!(
        world.appearance(entity, VISUALS_ROTATION),
        Some(AppearanceValue::Text(text)) if text == "Horizontal"
    );
    let rotation = if horizontal {
        visuals.horizontal_rotation
    } else {
        visuals.vertical_rotation
    };
    if let Some(mut sprite) = world.get_mut::<Sprite>(entity)
        && sprite.rotation != rotation
    {
        sprite.rotation = rotation;
    }
}

pub(crate) fn install(world: &mut World) {
    world.register_data_component::<DamageStateVisuals>();
    world.register_data_component::<RotationVisuals>();
    world.subscribe::<DamageStateVisuals, AppearanceChange>(|world, entity, _| {
        apply_damage_state(world, entity);
    });
    world.subscribe::<RotationVisuals, AppearanceChange>(|world, entity, _| {
        apply_rotation(world, entity);
    });
}
