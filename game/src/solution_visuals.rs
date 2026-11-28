//! Вид заполнения ёмкости на клиенте: слой заполнения получает состояние `<основа><уровень>`
//! по доле заполнения и цвет раствора; пустая ёмкость прячет слой или показывает «пустое»
//! состояние. Образец — `SolutionContainerVisualsSystem` клиента SS14 @ 2f4a2aad.
//!
//! Нет пока: смены вида ёмкости по реагенту (`Metamorphic`: стаканы с напитками), заполнения
//! в руках и на одежде.

use astrocore_client::Sprite;
use astrocore_ecs::EventWorldExt;
use astrocore_math::Color;
use astrocore_net::{AppearanceChange, AppearanceValue, AppearanceWorldExt};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::solution_container::{
    SolutionContainerVisuals, VISUALS_COLOR, VISUALS_FILL_FRACTION, VISUALS_SOLUTION_NAME,
};

/// Ближайший уровень от 0 до `levels − 1` с округлением вверх (`ContentHelpers.RoundToLevels`).
pub fn round_to_levels(actual: f64, max: f64, levels: i32) -> i32 {
    if actual >= max {
        return levels - 1;
    }
    if actual <= 0.0 {
        return 0;
    }
    (actual / max * f64::from(levels - 2)).ceil() as i32
}

fn apply(world: &mut World, entity: Entity) {
    let Some(visuals) = world.get::<SolutionContainerVisuals>(entity).cloned() else {
        return;
    };
    // Обновился не тот раствор, который показывается.
    if let (Some(shown), Some(AppearanceValue::Text(name))) = (
        visuals.solution_name.as_ref(),
        world.appearance(entity, VISUALS_SOLUTION_NAME),
    ) && !shown.is_empty()
        && name != shown
    {
        return;
    }
    let Some(AppearanceValue::Float(fraction)) =
        world.appearance(entity, VISUALS_FILL_FRACTION).cloned()
    else {
        return;
    };
    let color = match world.appearance(entity, VISUALS_COLOR) {
        Some(AppearanceValue::Text(hex)) => Color::parse(hex),
        _ => None,
    };
    let Some(sprite) = world.get::<Sprite>(entity).cloned() else {
        return;
    };
    let Some(layer) = sprite.layer_index(&visuals.layer) else {
        return;
    };
    let mut updated = sprite.clone();
    let fill = &mut updated.layers[layer];
    fill.visible = true;
    let level = round_to_levels(
        f64::from(fraction.min(1.0)),
        1.0,
        visuals.max_fill_levels + 1,
    );
    if level > 0 {
        let Some(base) = &visuals.fill_base_name else {
            return;
        };
        fill.state = Some(format!("{base}{level}"));
        fill.color = match (visuals.change_color, color) {
            (true, Some(color)) => color,
            _ => Color::WHITE,
        };
    } else {
        match &visuals.empty_sprite_name {
            None => fill.visible = false,
            Some(empty) => {
                fill.state = Some(empty.clone());
                fill.color = if visuals.change_color {
                    Color::parse(&visuals.empty_sprite_color).unwrap_or(Color::WHITE)
                } else {
                    Color::WHITE
                };
            }
        }
    }
    if updated != sprite
        && let Some(mut sprite) = world.get_mut::<Sprite>(entity)
    {
        *sprite = updated;
    }
}

pub(crate) fn install(world: &mut World) {
    world.subscribe::<SolutionContainerVisuals, AppearanceChange>(|world, entity, _| {
        apply(world, entity);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_levels_round_up() {
        // Мензурка: 6 уровней, слои 1–6.
        assert_eq!(round_to_levels(0.0, 1.0, 7), 0);
        assert_eq!(round_to_levels(0.01, 1.0, 7), 1);
        assert_eq!(round_to_levels(0.5, 1.0, 7), 3);
        assert_eq!(round_to_levels(0.99, 1.0, 7), 5);
        assert_eq!(round_to_levels(1.0, 1.0, 7), 6);
    }
}
