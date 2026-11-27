//! Множители скорости ходьбы: каждая система вносит свой множитель в событие, итог
//! записывается в `MovementSpeedModifier`. Образец — `MovementSpeedModifierSystem.
//! RefreshMovementSpeedModifiers` и `RefreshMovementSpeedModifiersEvent` SS14 @ 2f4a2aad.

use astrocore_ecs::EventWorldExt;
use astrocore_movement::MovementSpeedModifier;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

/// Сбор множителей скорости (`RefreshMovementSpeedModifiersEvent`). На сущности.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefreshMovementSpeedModifiers {
    pub walk: f32,
    pub sprint: f32,
}

impl Default for RefreshMovementSpeedModifiers {
    fn default() -> Self {
        Self {
            walk: 1.0,
            sprint: 1.0,
        }
    }
}

impl RefreshMovementSpeedModifiers {
    /// Умножить скорость (`ModifySpeed`).
    pub fn modify_speed(&mut self, walk: f32, sprint: f32) {
        self.walk *= walk;
        self.sprint *= sprint;
    }
}

/// Пересчитать множители (`RefreshMovementSpeedModifiers`).
pub fn refresh_movement_speed_modifiers(world: &mut World, entity: Entity) {
    if world.get::<MovementSpeedModifier>(entity).is_none() {
        return;
    }
    let mut event = RefreshMovementSpeedModifiers::default();
    world.raise(entity, &mut event);
    if let Some(mut speed) = world.get_mut::<MovementSpeedModifier>(entity)
        && (speed.walk_speed_modifier != event.walk || speed.sprint_speed_modifier != event.sprint)
    {
        speed.walk_speed_modifier = event.walk;
        speed.sprint_speed_modifier = event.sprint;
    }
}
