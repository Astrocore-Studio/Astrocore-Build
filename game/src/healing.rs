//! Лечение предметами: бинты, мази, аптечки. Действие с задержкой, себя лечить дольше при
//! сильных ранах, предмет повторяет лечение, пока есть что лечить. Образец —
//! `Content.Shared/Medical/Healing` SS14 @ 2f4a2aad (`HealingComponent`, `HealingSystem`).
//! Бинты и жгуты останавливают кровотечение (`bloodlossModifier`), часть средств возвращает
//! кровь (`modifyBloodLevel`).

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, LifecycleWorldExt, RegistryWorldExt};
use astrocore_loc::EntityNameWorldExt;
use astrocore_net::NetRegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::bloodstream::{self, Bloodstream};
use crate::damage::{DamageSpecifier, Damageable, DamageableWorldExt, Injurable};
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterWorldExt};
use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::interaction::{AfterInteract, INTERACTION_RANGE, InteractionWorldExt, UseInHand};
use crate::mobs::{MobState, MobStateWorldExt};
use crate::popups::{PopupType, PopupWorldExt};
use crate::stack::{Stack, StackWorldExt};

fn three() -> f64 {
    3.0
}

fn self_penalty() -> f32 {
    3.0
}

/// Лечащий предмет. В данных — компонент `Healing`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Healing {
    pub damage: DamageSpecifier,
    #[serde(default)]
    pub bloodloss_modifier: f32,
    #[serde(default)]
    pub modify_blood_level: f32,
    #[serde(default)]
    pub damage_containers: Option<Vec<String>>,
    /// Задержка, секунды.
    #[serde(default = "three")]
    pub delay: f64,
    #[serde(default = "self_penalty")]
    pub self_heal_penalty_multiplier: f32,
    #[serde(default)]
    pub healing_begin_sound: Option<SoundSpecifier>,
    #[serde(default)]
    pub healing_end_sound: Option<SoundSpecifier>,
}

/// Итог лечения (`HealingDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
pub struct HealingDoAfter;

/// Подходит ли предмет к модели урона цели.
fn container_allows(world: &World, healing: &Healing, target: Entity) -> bool {
    let Some(injurable) = world.get::<Injurable>(target) else {
        return false;
    };
    match (&healing.damage_containers, &injurable.damage_container) {
        (Some(allowed), Some(container)) => allowed.contains(container),
        _ => true,
    }
}

/// Есть ли у цели то, что предмет лечит: урон его типов, нехватка крови или кровотечение
/// (`HasDamage`).
pub(crate) fn has_damage(world: &World, healing: &Healing, target: Entity) -> bool {
    let Some(damageable) = world.get::<Damageable>(target) else {
        return false;
    };
    let wounded = healing.damage.types.keys().any(|damage_type| {
        damageable
            .damage
            .types
            .get(damage_type)
            .is_some_and(|v| *v > FixedPoint2::ZERO)
    });
    if wounded {
        return true;
    }
    let Some(bloodstream) = world.get::<Bloodstream>(target) else {
        return false;
    };
    (healing.modify_blood_level > 0.0 && crate::bloodstream::blood_level(world, target) < 1.0)
        || (healing.bloodloss_modifier < 0.0 && bloodstream.bleed_amount > 0.0)
}

/// Себя лечить дольше: от 1 до множителя по доле урона до крита (`GetScaledHealingPenalty`).
pub fn scaled_healing_penalty(world: &World, target: Entity, multiplier: f32) -> f32 {
    if world.get::<Damageable>(target).is_none()
        || world.get::<crate::mobs::MobThresholds>(target).is_none()
    {
        return multiplier;
    }
    let Some(critical) = world.threshold_for_state(target, MobState::Critical) else {
        return 1.0;
    };
    let percent = (world.total_damage(target) / critical).float();
    (percent * (multiplier - 1.0) + 1.0).max(1.0)
}

/// Начать лечение (`TryHeal`).
pub fn try_heal(world: &mut World, item: Entity, target: Entity, user: Entity) -> bool {
    let Some(healing) = world.get::<Healing>(item).cloned() else {
        return false;
    };
    if world.get::<Damageable>(target).is_none() || !container_allows(world, &healing, target) {
        return false;
    }
    if user != target && !world.in_range_unobstructed(user, target, INTERACTION_RANGE) {
        return false;
    }
    if world
        .get::<Stack>(item)
        .is_some_and(|stack| stack.count < 1)
    {
        return false;
    }
    if !has_damage(world, &healing, target) {
        let name = world.entity_arg(item);
        let text = loc(
            world,
            "medical-item-cant-use",
            &[("item", name.clone().into())],
        );
        world.popup_entity_for(&text, item, user, PopupType::Small);
        return false;
    }
    if let Some(sound) = &healing.healing_begin_sound {
        world.play_entity(sound, item, None);
    }
    let not_self = user != target;
    if not_self {
        let user_name = world.entity_arg(user);
        let item_name = world.entity_arg(item);
        let text = loc(
            world,
            "medical-item-popup-target",
            &[
                ("user", user_name.clone().into()),
                ("item", item_name.clone().into()),
            ],
        );
        world.popup_entity_for(&text, target, target, PopupType::Medium);
    }
    let delay = if not_self {
        healing.delay
    } else {
        healing.delay
            * f64::from(scaled_healing_penalty(
                world,
                target,
                healing.self_heal_penalty_multiplier,
            ))
    };
    let mut args = DoAfterArgs::new(user, delay, Some(target), Some(item), Some(target));
    args.need_hand = true;
    args.break_on_move = true;
    world.try_start_do_after(args, HealingDoAfter);
    true
}

/// Лечение завершено (`OnDoAfter`).
fn on_do_after(world: &mut World, target: Entity, event: &mut DoAfterDone<HealingDoAfter>) {
    if event.handled || event.cancelled {
        return;
    }
    let Some(item) = event.used else {
        return;
    };
    let Some(healing) = world.get::<Healing>(item).cloned() else {
        return;
    };
    if !container_allows(world, &healing, target) {
        return;
    }
    // Остановить кровотечение.
    if healing.bloodloss_modifier != 0.0
        && let Some(bleeding) = world
            .get::<Bloodstream>(target)
            .map(|b| b.bleed_amount > 0.0)
    {
        bloodstream::modify_bleed_amount(world, target, healing.bloodloss_modifier);
        let still = world
            .get::<Bloodstream>(target)
            .is_some_and(|b| b.bleed_amount > 0.0);
        if bleeding != still {
            let text = if event.user == target {
                loc(world, "medical-item-stop-bleeding-self", &[])
            } else {
                let name = world.entity_arg(target);
                loc(
                    world,
                    "medical-item-stop-bleeding",
                    &[("target", name.into())],
                )
            };
            world.popup_entity_for(&text, target, event.user, PopupType::Small);
        }
    }
    // Вернуть кровь.
    if healing.modify_blood_level != 0.0 {
        bloodstream::modify_blood_level(
            world,
            target,
            FixedPoint2::from_f32(healing.modify_blood_level),
        );
    }
    let healed = world.change_damage(target, &healing.damage, true, true, Some(event.user));
    if healed.is_empty() && healing.bloodloss_modifier != 0.0 {
        return;
    }
    // Из стопки уходит один предмет; без стопки предмет тратится целиком.
    let mut dont_repeat = false;
    if world.get::<Stack>(item).is_some() {
        world.reduce_stack(item, 1);
        if world.stack_count(item) <= 0 {
            dont_repeat = true;
        }
    } else {
        world.queue_delete(item);
        dont_repeat = true;
    }
    if let Some(sound) = &healing.healing_end_sound {
        world.play_entity(sound, target, None);
    }
    event.repeat = has_damage(world, &healing, target) && !dont_repeat;
    event.handled = true;
    if !event.repeat {
        return;
    }
    // Себя лечить тем быстрее, чем меньше осталось ран.
    if event.user == target {
        event.delay = healing.delay
            * f64::from(scaled_healing_penalty(
                world,
                target,
                healing.self_heal_penalty_multiplier,
            ));
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<Healing>();
    world.register_net_serde::<Healing>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Healing, UseInHand>(|world, item, event| {
        if !event.handled && try_heal(world, item, event.user, event.user) {
            event.handled = true;
        }
    });
    world.subscribe::<Healing, AfterInteract>(|world, item, event| {
        if event.handled || !event.can_reach {
            return;
        }
        if let Some(target) = event.target
            && try_heal(world, item, target, event.user)
        {
            event.handled = true;
        }
    });
    world.subscribe::<Damageable, DoAfterDone<HealingDoAfter>>(on_do_after);
}

#[cfg(test)]
mod tests;
