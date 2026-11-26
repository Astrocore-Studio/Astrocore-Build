//! Спрайт стопки по числу: состояние слоя или видимые слои. Образец — `StackSystem`
//! клиента и `ItemCounterSystem` SS14 @ 2f4a2aad, `ContentHelpers` (округление уровней).

use astrocore_client::Sprite;
use astrocore_ecs::EventWorldExt;
use astrocore_net::{AppearanceChange, AppearanceValue, AppearanceWorldExt};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::stack::{
    Stack, StackLayerFunction, StackLayerThreshold, VISUALS_ACTUAL, VISUALS_HIDE, VISUALS_MAX_COUNT,
};

/// Уровень от 0 до `levels − 1`, ровные доли с отбрасыванием (`RoundToEqualLevels`).
pub fn round_to_equal_levels(actual: f64, max: f64, levels: usize) -> usize {
    if actual >= max {
        return levels - 1;
    }
    if actual <= 0.0 {
        return 0;
    }
    (actual / max * levels as f64).trunc() as usize
}

/// Сколько слоёв видно, ближайший уровень (`RoundToNearestLevels`).
pub fn round_to_nearest_levels(actual: f64, max: f64, levels: usize) -> usize {
    if actual >= max {
        return levels;
    }
    if actual <= 0.0 {
        return 0;
    }
    (actual / max * levels as f64).round() as usize
}

fn int(world: &World, entity: Entity, key: &str) -> Option<i64> {
    match world.appearance(entity, key)? {
        AppearanceValue::Int(value) => Some(*value),
        _ => None,
    }
}

fn apply(world: &mut World, entity: Entity) {
    let Some(stack) = world.get::<Stack>(entity).cloned() else {
        return;
    };
    if stack.layer_states.is_empty() {
        return;
    }
    let Some(mut actual) = int(world, entity, VISUALS_ACTUAL) else {
        return;
    };
    let mut max = int(world, entity, VISUALS_MAX_COUNT).unwrap_or(stack.layer_states.len() as i64);
    let hidden = matches!(
        world.appearance(entity, VISUALS_HIDE),
        Some(AppearanceValue::Bool(true))
    );
    if stack.layer_function == StackLayerFunction::Threshold
        && let Some(threshold) = world.get::<StackLayerThreshold>(entity)
    {
        max = max.min(threshold.thresholds.len() as i64 + 1);
        let mut level = 0;
        for limit in &threshold.thresholds {
            if actual >= i64::from(*limit) && level < max {
                level += 1;
            } else {
                break;
            }
        }
        actual = level;
    }
    let Some(sprite) = world.get::<Sprite>(entity).cloned() else {
        return;
    };
    let mut updated = sprite.clone();
    if stack.is_composite {
        let shown = round_to_nearest_levels(actual as f64, max as f64, stack.layer_states.len());
        for (index, key) in stack.layer_states.iter().enumerate() {
            if let Some(layer) = updated.layer_index(key) {
                updated.layers[layer].visible = !hidden && index < shown;
            }
        }
    } else if let Some(layer) = updated.layer_index(&stack.base_layer) {
        let level = round_to_equal_levels(actual as f64, max as f64, stack.layer_states.len());
        updated.layers[layer].state = Some(stack.layer_states[level].clone());
        updated.layers[layer].visible = !hidden;
    }
    if updated != sprite
        && let Some(mut sprite) = world.get_mut::<Sprite>(entity)
    {
        *sprite = updated;
    }
}

pub(crate) fn install(world: &mut World) {
    world.subscribe::<Stack, AppearanceChange>(|world, entity, _| apply(world, entity));
}
