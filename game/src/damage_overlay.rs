//! Уровни затемнения экрана от урона у сущности игрока: боль (доля урона групп боли от порога
//! крита, меньше 5 % — нет), удушье (урон группы `Airloss` от порога крита), крит (доля урона
//! от порога смерти) и смерть. Клиент рисует по ним красную пульсацию, темноту и белую дымку
//! (`damage_overlay_view`). Образец — `DamageOverlayComponent`, `SharedDamageOverlaySystem`
//! SS14 @ 2f4a2aad.
//!
//! Онемение от боли (`PainNumbnessStatusEffect`) не учитывается: статус-эффектов пока нет.

use astrocore_ecs::{ComponentStartup, EventWorldExt, RegistryWorldExt};
use astrocore_net::NetRegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::damage::{DamageChanged, Damageable, Injurable};
use crate::fixed_point::FixedPoint2;
use crate::mobs::{MobState, MobStateChanged, MobStateWorldExt, MobThresholds};

/// Уровни затемнения (`DamageOverlayComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DamageOverlay {
    pub current_state: MobState,
    pub pain_level: f32,
    pub crit_level: f32,
    pub dead_level: f32,
    pub oxygen_level: f32,
}

/// Пересчитать уровни (`UpdateOverlays`).
pub fn update_overlays(world: &mut World, entity: Entity) {
    let (Some(state), Some(thresholds), Some(damageable), Some(injurable)) = (
        world.mob_state(entity),
        world.get::<MobThresholds>(entity).cloned(),
        world.get::<Damageable>(entity).cloned(),
        world.get::<Injurable>(entity).cloned(),
    ) else {
        return;
    };
    // Не умирает и не падает в крит — затемнения нет.
    let Some(crit_threshold) = world.incap_threshold(entity) else {
        return;
    };
    if !thresholds.show_overlays {
        if let Some(mut overlay) = world.get_mut::<DamageOverlay>(entity) {
            *overlay = DamageOverlay::default();
        }
        return;
    }
    let crit = crit_threshold.float().max(f32::EPSILON);
    let dead_threshold = world.threshold_for_state(entity, MobState::Dead);
    let per_group = &damageable.damage_per_group;
    let Some(mut overlay) = world.get_mut::<DamageOverlay>(entity) else {
        return;
    };
    overlay.current_state = state;
    match state {
        MobState::Alive => {
            let pain: f32 = injurable
                .pain_damage_groups
                .iter()
                .filter_map(|group| per_group.get(group))
                .map(|v| v.float())
                .sum();
            overlay.pain_level = (pain / crit).min(1.0);
            // Почти без урона — без пульсации.
            if overlay.pain_level < 0.05 {
                overlay.pain_level = 0.0;
            }
            if let Some(oxygen) = per_group.get("Airloss") {
                overlay.oxygen_level = (oxygen.float() / crit).min(1.0);
            }
            overlay.crit_level = 0.0;
            overlay.dead_level = 0.0;
        }
        MobState::Critical => {
            let Some(dead) = dead_threshold else {
                return;
            };
            let total = damageable.damage.total().max(FixedPoint2::default());
            overlay.crit_level = if total == FixedPoint2::default() {
                0.0
            } else {
                (total.float() / dead.float().max(f32::EPSILON)).min(1.0)
            };
            overlay.pain_level = 0.0;
            overlay.dead_level = 0.0;
        }
        MobState::Dead => {
            overlay.pain_level = 0.0;
            overlay.crit_level = 0.0;
        }
        MobState::Invalid => {}
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<DamageOverlay>();
    world.register_net_serde::<DamageOverlay>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<DamageOverlay, ComponentStartup>(|world, entity, _| {
        update_overlays(world, entity);
    });
    world.subscribe::<DamageOverlay, MobStateChanged>(|world, entity, _| {
        update_overlays(world, entity);
    });
    // После пересчёта состояния по порогам (`MobThresholdChecked`).
    world.subscribe::<DamageOverlay, DamageChanged>(|world, entity, _| {
        update_overlays(world, entity);
    });
}

#[cfg(test)]
mod tests;
