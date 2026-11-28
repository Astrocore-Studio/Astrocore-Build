//! Дефибриллятор: включённый, с зарядом и без паузы — щелчок по телу начинает разряд (3 с,
//! звук зарядки, сообщение рядом). Разряд тратит заряд батарейки, ставит паузу 5 с, лечит
//! удушье и бьёт током тело и всех, кто с ним взаимодействует (тянет, лечит); мёртвый, чей
//! урон ниже порога смерти, оживает в крит, а призраку приходит окно «вернуться в тело». Без
//! разума дефибриллятор говорит, почему не вышло. Не хватает заряда на следующий разряд —
//! выключается. Предметы `MultiHandedItem` требуют свободных рук и занимают их.
//! Образец — `SharedDefibrillatorSystem`, `DefibrillatorSystem` (сервер),
//! `DefibrillatorComponent`, `DefibrillatorEvents`, `MultiHandedItemSystem`,
//! `MultiHandedItemComponent` SS14 @ 2f4a2aad.
//!
//! Нет пока гниения тел (`Rotting`: «тело разложилось») и невоскрешаемых
//! (`Unrevivable`), судорог после удара (статус-эффекты, 7.3).

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_loc::EntityNameWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::action_blocker::PickupAttempt;
use crate::chat::{ChatTransmitRange, InGameIcChatType};
use crate::damage::{DamageSpecifier, DamageableWorldExt};
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterStart, DoAfterWorldExt};
use crate::examine::loc;
use crate::hands::{GotEquippedHand, GotUnequippedHand, HandsWorldExt};
use crate::interaction::{AfterInteract, entities_interacting_with};
use crate::mind::{Mind, MindContainer};
use crate::mobs::{MobState, MobStateWorldExt};
use crate::popups::{PopupType, PopupWorldExt};
use crate::virtual_item::{VirtualItemDeleted, VirtualItemWorldExt};

fn zap_damage() -> i32 {
    5
}
fn writhe() -> f32 {
    3.0
}
fn delay_id() -> String {
    "defib-delay".into()
}
fn zap_delay() -> f32 {
    5.0
}
fn do_after() -> f32 {
    3.0
}
fn yes() -> bool {
    true
}
fn sound(path: &str) -> Option<SoundSpecifier> {
    Some(SoundSpecifier::path(path))
}
fn zap_sound() -> Option<SoundSpecifier> {
    sound("/Audio/Items/Defib/defib_zap.ogg")
}
fn charge_sound() -> Option<SoundSpecifier> {
    sound("/Audio/Items/Defib/defib_charge.ogg")
}
fn failure_sound() -> Option<SoundSpecifier> {
    sound("/Audio/Items/Defib/defib_failed.ogg")
}
fn success_sound() -> Option<SoundSpecifier> {
    sound("/Audio/Items/Defib/defib_success.ogg")
}
fn ready_sound() -> Option<SoundSpecifier> {
    sound("/Audio/Items/Defib/defib_ready.ogg")
}

/// Дефибриллятор (`DefibrillatorComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Defibrillator {
    /// Лечение разрядом.
    pub zap_heal: DamageSpecifier,
    /// Урон током.
    #[serde(default = "zap_damage")]
    pub zap_damage: i32,
    /// Судороги после удара, с.
    #[serde(default = "writhe")]
    pub writhe_duration: f32,
    #[serde(default = "delay_id")]
    pub delay_id: String,
    /// Пауза после разряда, с.
    #[serde(default = "zap_delay")]
    pub zap_delay: f32,
    /// Разряд готовится, с.
    #[serde(default = "do_after")]
    pub do_after_duration: f32,
    /// Можно двигаться, пока готовится разряд.
    #[serde(default = "yes")]
    pub allow_do_after_movement: bool,
    #[serde(default = "zap_sound")]
    pub zap_sound: Option<SoundSpecifier>,
    #[serde(default = "charge_sound")]
    pub charge_sound: Option<SoundSpecifier>,
    #[serde(default = "failure_sound")]
    pub failure_sound: Option<SoundSpecifier>,
    #[serde(default = "success_sound")]
    pub success_sound: Option<SoundSpecifier>,
    #[serde(default = "ready_sound")]
    pub ready_sound: Option<SoundSpecifier>,
}

/// Разряд готов (`DefibrillatorZapDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
pub struct DefibrillatorZapDoAfter;

/// Перед разрядом — тому, кто разряжает (`SelfBeforeDefibrillatorZapsEvent`); можно сменить
/// цель или отменить.
#[derive(Debug, Clone, Copy)]
pub struct SelfBeforeDefibrillatorZaps {
    pub user: Entity,
    pub defib: Entity,
    pub defib_target: Entity,
    pub cancelled: bool,
}

/// Перед разрядом — цели (`TargetBeforeDefibrillatorZapsEvent`).
#[derive(Debug, Clone, Copy)]
pub struct TargetBeforeDefibrillatorZaps {
    pub user: Entity,
    pub defib: Entity,
    pub defib_target: Entity,
    pub cancelled: bool,
}

/// Цель получила разряд (`TargetDefibrillatedEvent`).
#[derive(Debug, Clone)]
pub struct TargetDefibrillated {
    pub user: Entity,
    pub target: Entity,
    pub defib: Entity,
    pub interactors: Vec<Entity>,
}

fn two() -> usize {
    2
}

/// Предмет держат несколькими руками (`MultiHandedItemComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiHandedItem {
    #[serde(default = "two")]
    pub hands_needed: usize,
}

/// Можно ли разрядить (`CanZap`): включён, без паузы, с зарядом.
pub fn can_zap(world: &mut World, defib: Entity, user: Option<Entity>) -> bool {
    let Some(component) = world.get::<Defibrillator>(defib).cloned() else {
        return false;
    };
    if !crate::item_toggle::is_activated(world, defib) {
        if let Some(user) = user {
            let text = loc(world, "defibrillator-not-on", &[]);
            world.popup_entity_for(&text, defib, user, PopupType::Small);
        }
        return false;
    }
    if world.get::<crate::use_delay::UseDelay>(defib).is_none()
        || crate::use_delay::is_delayed(world, defib, &component.delay_id)
    {
        return false;
    }
    crate::power_cell::has_activatable_charge(world, defib, user)
}

/// Начать готовить разряд (`TryStartZap`).
pub fn try_start_zap(world: &mut World, defib: Entity, target: Entity, user: Entity) -> bool {
    if !can_zap(world, defib, Some(user)) {
        return false;
    }
    let Some(component) = world.get::<Defibrillator>(defib).cloned() else {
        return false;
    };
    if let Some(sound) = &component.charge_sound {
        world.play_entity(sound, defib, None);
    }
    let (name, target_name) = (world.entity_arg(user), world.entity_arg(target));
    let text = loc(
        world,
        "defibrillator-begin",
        &[("name", name.into()), ("target", target_name.into())],
    );
    world.popup_entity(&text, target, PopupType::SmallCaution);
    let mut args = DoAfterArgs::new(
        user,
        f64::from(component.do_after_duration),
        Some(target),
        Some(defib),
        Some(defib),
    );
    args.need_hand = true;
    args.break_on_move = !component.allow_do_after_movement;
    world.try_start_do_after(args, DefibrillatorZapDoAfter) != DoAfterStart::Refused
}

/// Разряд (`Zap`).
pub fn zap(world: &mut World, defib: Entity, target: Entity, user: Entity) {
    let Some(component) = world.get::<Defibrillator>(defib).cloned() else {
        return;
    };
    if !crate::power_cell::try_use_activatable_charge(world, defib, Some(user)) {
        return;
    }
    let mut self_event = SelfBeforeDefibrillatorZaps {
        user,
        defib,
        defib_target: target,
        cancelled: false,
    };
    world.raise(user, &mut self_event);
    let target = self_event.defib_target;
    if self_event.cancelled || !can_zap(world, defib, Some(user)) {
        return;
    }
    let mut target_event = TargetBeforeDefibrillatorZaps {
        user,
        defib,
        defib_target: target,
        cancelled: false,
    };
    world.raise(target, &mut target_event);
    let target = target_event.defib_target;
    if target_event.cancelled || !can_zap(world, defib, Some(user)) {
        return;
    }
    let pause = std::time::Duration::from_secs_f32(component.zap_delay);
    crate::use_delay::set_length(world, defib, pause, &component.delay_id);
    crate::use_delay::try_reset_delay(world, defib, false, &component.delay_id);
    if let Some(sound) = &component.zap_sound {
        world.play_entity(sound, defib, None);
    }
    let failed = try_revive(world, defib, &component, user, target, true);
    let interactors = entities_interacting_with(world, target);
    for &interactor in &interactors {
        try_revive(world, defib, &component, user, interactor, false);
    }
    let sound = if failed {
        &component.failure_sound
    } else {
        &component.success_sound
    };
    if let Some(sound) = sound {
        world.play_entity(sound, defib, None);
    }
    world.raise(
        target,
        &mut TargetDefibrillated {
            user,
            target,
            defib,
            interactors,
        },
    );
    // Не хватит на следующий разряд — выключиться.
    if !crate::power_cell::has_activatable_charge(world, defib, None) {
        crate::item_toggle::try_deactivate(world, defib, None, true);
    }
}

/// Оживить разрядом (`TryRevive`); `true` — не оживил.
fn try_revive(
    world: &mut World,
    defib: Entity,
    component: &Defibrillator,
    user: Entity,
    target: Entity,
    is_original: bool,
) -> bool {
    let mut failed = true;
    world.change_damage(target, &component.zap_heal, true, true, Some(user));
    if world.is_mob_dead(target)
        && let Some(threshold) = world.threshold_for_state(target, MobState::Dead)
        && world.total_damage(target) < threshold
    {
        world.change_mob_state(target, MobState::Critical, Some(user));
        failed = false;
    }
    let mind = crate::mind::mind_of(world, target);
    let session = mind
        .and_then(|mind| world.get::<Mind>(mind).and_then(|m| m.user_id))
        .and_then(|user_id| crate::mind::session_of_user(world, user_id));
    let message = match (mind, session) {
        (Some(mind), Some(session)) => {
            // Игрок в призраке — предложить вернуться.
            let current = world.get::<Mind>(mind).and_then(Mind::current_entity);
            if current != Some(target) {
                crate::return_to_body::offer(world, session, mind);
            }
            None
        }
        _ if world.get::<MindContainer>(target).is_some() => Some("defibrillator-no-mind"),
        _ => Some("defibrillator-not-living"),
    };
    if let (true, Some(key)) = (is_original, message) {
        let text = loc(world, key, &[]);
        crate::chat::try_send_in_game_ic_message(
            world,
            defib,
            &text,
            InGameIcChatType::Speak,
            ChatTransmitRange::HideChat,
            false,
            None,
            None,
            false,
            true,
        );
    }
    crate::electrocution::try_do_electrocution(
        world,
        target,
        Some(defib),
        component.zap_damage,
        1.0,
        is_original,
    );
    failed
}

fn on_do_after(world: &mut World, defib: Entity, event: &mut DoAfterDone<DefibrillatorZapDoAfter>) {
    if event.handled || event.cancelled {
        return;
    }
    let Some(target) = event.target else {
        return;
    };
    if !can_zap(world, defib, Some(event.user)) {
        return;
    }
    event.handled = true;
    zap(world, defib, target, event.user);
}

/// Свободных рук не хватает — не поднять (`OnAttemptPickup`).
fn multi_handed_pickup(world: &mut World, item: Entity, event: &mut PickupAttempt) {
    if event.cancelled || event.item != item {
        return;
    }
    let Some(needed) = world.get::<MultiHandedItem>(item).map(|m| m.hands_needed) else {
        return;
    };
    let free = world
        .hands_in_order(event.user)
        .len()
        .saturating_sub(world.held_items(event.user).len());
    if free >= needed {
        return;
    }
    event.cancelled = true;
    let name = world.entity_arg(item);
    let text = loc(
        world,
        "multi-handed-item-pick-up-fail",
        &[
            ("number", (needed as i64 - 1).into()),
            ("item", name.into()),
        ],
    );
    world.popup_cursor(&text, event.user, PopupType::Small);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Defibrillator>();
    world.register_data_component::<MultiHandedItem>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Defibrillator, AfterInteract>(|world, defib, event| {
        if event.handled || !event.can_reach {
            return;
        }
        if let Some(target) = event.target {
            event.handled = try_start_zap(world, defib, target, event.user);
        }
    });
    world.subscribe::<Defibrillator, DoAfterDone<DefibrillatorZapDoAfter>>(on_do_after);
    world.subscribe::<MultiHandedItem, PickupAttempt>(multi_handed_pickup);
    // Занять остальные руки (`OnEquipped`).
    world.subscribe::<MultiHandedItem, GotEquippedHand>(|world, item, event| {
        let needed = world
            .get::<MultiHandedItem>(item)
            .map_or(1, |m| m.hands_needed);
        for _ in 1..needed {
            world.try_spawn_virtual_item_in_hand(item, event.user);
        }
    });
    world.subscribe::<MultiHandedItem, GotUnequippedHand>(|world, item, event| {
        world.delete_in_hands_matching(event.user, item);
    });
    // Заглушку руки выбросили — выпустить и сам предмет (`OnVirtualItemDeleted`).
    world.subscribe::<MultiHandedItem, VirtualItemDeleted>(|world, item, event| {
        if event.blocking_entity != item {
            return;
        }
        let dropped = world
            .hand_holding(event.user, item)
            .is_some_and(|hand| world.try_drop(event.user, &hand, None, true));
        if !dropped && world.held_items(event.user).contains(&item) {
            world.try_spawn_virtual_item_in_hand(item, event.user);
        }
    });
}

#[cfg(test)]
mod tests;
