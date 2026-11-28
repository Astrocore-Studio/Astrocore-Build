//! Переливание: щелчок ёмкостью в руке по другой ёмкости — из своей в доливаемую цель
//! (если своя сливается), иначе в свою из сливаемой цели. Количество за раз — настройка
//! ёмкости (глаголы «1u» … по вместимости и «Своё» с окном ввода), ограниченное наибольшим
//! доливом цели; с задержкой долива и слива — действие с задержкой. Пустой источник и полная
//! цель — сообщения, перелитое — в журнал администратора. Образец — `SolutionTransferSystem`,
//! `SolutionTransferComponent`, `TransferAmountSetValueMessage`,
//! `TransferAmountBoundUserInterface` SS14 @ 2f4a2aad.

use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_loc::EntityNameWorldExt;
use astrocore_net::NetRegistryWorldExt;
use astrocore_ui::{BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterWorldExt};
use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::interaction::AfterInteract;
use crate::popups::{PopupType, PopupWorldExt};
use crate::solution::Solution;
use crate::solution_container::{
    self as containers, DrainableSolution, RefillableSolution, drainable_solution,
    refillable_solution,
};
use crate::verbs::{GetVerbs, Verb, VerbKind};

/// Ключ окна (`TransferAmountUiKey.Key`).
pub const TRANSFER_AMOUNT_UI_KEY: &str = "enum.TransferAmountUiKey.Key";

/// Количества глаголов (`DefaultTransferAmounts`).
pub const DEFAULT_TRANSFER_AMOUNTS: [i32; 10] = [1, 5, 10, 15, 30, 60, 120, 240, 480, 960];

fn five() -> FixedPoint2 {
    FixedPoint2::new(5)
}
fn hundred_twenty() -> FixedPoint2 {
    FixedPoint2::new(120)
}
fn yes() -> bool {
    true
}

/// Ёмкость переливает (`SolutionTransferComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionTransfer {
    #[serde(default = "five")]
    pub transfer_amount: FixedPoint2,
    #[serde(default = "five", rename = "minTransferAmount")]
    pub minimum_transfer_amount: FixedPoint2,
    #[serde(default = "hundred_twenty", rename = "maxTransferAmount")]
    pub maximum_transfer_amount: FixedPoint2,
    #[serde(default = "yes")]
    pub can_receive: bool,
    #[serde(default = "yes")]
    pub can_send: bool,
    #[serde(default)]
    pub can_change_transfer_amount: bool,
}

/// Задать количество за раз (`TransferAmountSetValueMessage`). Клиент → сервер.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TransferAmountSetValue {
    pub value: FixedPoint2,
}

/// Можно ли перелить (`SolutionTransferAttemptEvent`): на источнике, затем на цели.
#[derive(Debug, Clone)]
pub struct SolutionTransferAttempt {
    pub from: Entity,
    pub to: Entity,
    pub cancel_reason: Option<String>,
}

/// Перелито (`SolutionTransferredEvent`). На цели.
#[derive(Debug, Clone, Copy)]
pub struct SolutionTransferred {
    pub from: Entity,
    pub to: Entity,
    pub user: Entity,
    pub amount: FixedPoint2,
}

/// Слив в цель по задержке (`SolutionDrainTransferDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
pub struct SolutionDrainTransferDoAfter {
    pub amount: FixedPoint2,
}

/// Долив в свою из цели по задержке (`SolutionRefillTransferDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
pub struct SolutionRefillTransferDoAfter {
    pub amount: FixedPoint2,
}

/// Кто кому сколько (`SolutionTransferData`).
#[derive(Debug, Clone, Copy)]
pub struct SolutionTransferData {
    pub user: Entity,
    pub source_entity: Entity,
    pub source: Entity,
    pub target_entity: Entity,
    pub target: Entity,
    pub amount: FixedPoint2,
}

/// Растворы источника и цели (`TryGetTransferrableSolutions`).
fn transferrable(world: &mut World, source: Entity, target: Entity) -> Option<(Entity, Entity)> {
    if world.get::<DrainableSolution>(source).is_none()
        || world.get::<RefillableSolution>(target).is_none()
    {
        return None;
    }
    Some((
        drainable_solution(world, source)?,
        refillable_solution(world, target)?,
    ))
}

fn popup(
    world: &mut World,
    key: &str,
    args: &[(&str, astrocore_loc::LocArg)],
    on: Entity,
    user: Entity,
) {
    let text = loc(world, key, args);
    world.popup_entity_for(&text, on, user, PopupType::Small);
}

/// Можно ли перелить (`CanTransfer`).
fn can_transfer(world: &mut World, data: &SolutionTransferData) -> bool {
    let mut attempt = SolutionTransferAttempt {
        from: data.source_entity,
        to: data.target_entity,
        cancel_reason: None,
    };
    world.raise(data.source_entity, &mut attempt);
    if let Some(reason) = attempt.cancel_reason.clone() {
        world.popup_entity_for(&reason, data.source_entity, data.user, PopupType::Small);
        return false;
    }
    if containers::solution(world, data.source).is_none_or(Solution::is_empty) {
        let name = world.entity_arg(data.source_entity);
        popup(
            world,
            "comp-solution-transfer-is-empty",
            &[("target", name.into())],
            data.source_entity,
            data.user,
        );
        return false;
    }
    world.raise(data.target_entity, &mut attempt);
    if let Some(reason) = attempt.cancel_reason {
        world.popup_entity_for(&reason, data.target_entity, data.user, PopupType::Small);
        return false;
    }
    if containers::solution(world, data.target)
        .is_none_or(|s| s.available_volume() == FixedPoint2::ZERO)
    {
        let name = world.entity_arg(data.target_entity);
        popup(
            world,
            "comp-solution-transfer-is-full",
            &[("target", name.into())],
            data.target_entity,
            data.user,
        );
        return false;
    }
    true
}

/// Перелить (`Transfer`); возвращает перелитое.
pub fn transfer(world: &mut World, data: SolutionTransferData) -> FixedPoint2 {
    if !can_transfer(world, &data) {
        return FixedPoint2::ZERO;
    }
    let (Some(source), Some(target)) = (
        containers::solution(world, data.source).cloned(),
        containers::solution(world, data.target).cloned(),
    ) else {
        return FixedPoint2::ZERO;
    };
    let amount = data
        .amount
        .min(source.volume.min(target.available_volume()));
    let taken = containers::split_solution(world, data.source, amount);
    containers::add_solution(world, data.target, &taken);
    world.raise(
        data.target_entity,
        &mut SolutionTransferred {
            from: data.source_entity,
            to: data.target_entity,
            user: data.user,
            amount,
        },
    );
    let (user, target_name) = (
        world.entity_name(data.user),
        world.entity_name(data.target_entity),
    );
    crate::admin_logs::add_admin_log(
        world,
        crate::admin_logs::LogType::Action,
        crate::admin_logs::LogImpact::Medium,
        &format!("{user} transferred {amount}u to {target_name}"),
        &[],
        &[],
    );
    amount
}

/// Слить своё в цель (`DrainTransfer`).
fn drain_transfer(world: &mut World, data: SolutionTransferData) {
    let transferred = transfer(world, data);
    if transferred <= FixedPoint2::ZERO {
        return;
    }
    let target = world.entity_arg(data.target_entity);
    popup(
        world,
        "comp-solution-transfer-transfer-solution",
        &[
            ("amount", transferred.to_string().into()),
            ("target", target.into()),
        ],
        data.source_entity,
        data.user,
    );
}

/// Долить свою из цели (`RefillTransfer`).
fn refill_transfer(world: &mut World, data: SolutionTransferData) {
    let transferred = transfer(world, data);
    if transferred <= FixedPoint2::ZERO {
        return;
    }
    let brim = containers::solution(world, data.target)
        .is_some_and(|s| s.available_volume() == FixedPoint2::ZERO);
    let key = if brim {
        "comp-solution-transfer-fill-fully"
    } else {
        "comp-solution-transfer-fill-normal"
    };
    let (owner, target) = (
        world.entity_arg(data.source_entity),
        world.entity_arg(data.target_entity),
    );
    popup(
        world,
        key,
        &[
            ("owner", owner.into()),
            ("amount", transferred.to_string().into()),
            ("target", target.into()),
        ],
        data.target_entity,
        data.user,
    );
}

fn seconds(value: f32) -> f64 {
    f64::from(value)
}

/// Щелчок ёмкостью (`OnAfterInteract`).
fn on_after_interact(world: &mut World, held: Entity, event: &mut AfterInteract) {
    let Some(target) = event.target.filter(|_| event.can_reach) else {
        return;
    };
    let Some(component) = world.get::<SolutionTransfer>(held).cloned() else {
        return;
    };
    let user = event.user;
    // Из своей в цель — в первую очередь.
    if component.can_send
        && let Some((own, theirs)) = transferrable(world, held, target)
    {
        event.handled = true;
        let refillable = world.get::<RefillableSolution>(target).cloned();
        let drainable = world.get::<DrainableSolution>(held).cloned();
        let mut amount = component.transfer_amount;
        if let Some(max) = refillable.as_ref().and_then(|r| r.max_refill) {
            amount = amount.min(max);
        }
        let data = SolutionTransferData {
            user,
            source_entity: held,
            source: own,
            target_entity: target,
            target: theirs,
            amount,
        };
        let time =
            refillable.map_or(0.0, |r| r.refill_time) + drainable.map_or(0.0, |d| d.drain_time);
        if time > 0.0 {
            if !can_transfer(world, &data) {
                return;
            }
            start_do_after(
                world,
                user,
                held,
                target,
                time,
                SolutionDrainTransferDoAfter { amount },
            );
        } else {
            drain_transfer(world, data);
        }
        return;
    }
    if component.can_receive
        && let Some((theirs, own)) = transferrable(world, target, held)
    {
        event.handled = true;
        let refillable = world.get::<RefillableSolution>(held).cloned();
        let drainable = world.get::<DrainableSolution>(target).cloned();
        let mut amount = component.transfer_amount;
        if let Some(max) = refillable.as_ref().and_then(|r| r.max_refill) {
            amount = amount.min(max);
        }
        let data = SolutionTransferData {
            user,
            source_entity: target,
            source: theirs,
            target_entity: held,
            target: own,
            amount,
        };
        let time =
            refillable.map_or(0.0, |r| r.refill_time) + drainable.map_or(0.0, |d| d.drain_time);
        if time > 0.0 {
            if !can_transfer(world, &data) {
                return;
            }
            start_do_after(
                world,
                user,
                held,
                target,
                time,
                SolutionRefillTransferDoAfter { amount },
            );
        } else {
            refill_transfer(world, data);
        }
    }
}

fn start_do_after<E: Clone + Send + Sync + 'static>(
    world: &mut World,
    user: Entity,
    held: Entity,
    target: Entity,
    time: f32,
    event: E,
) {
    let mut args = DoAfterArgs::new(user, seconds(time), Some(target), Some(held), Some(held));
    args.break_on_damage = true;
    args.break_on_move = true;
    args.need_hand = true;
    args.hidden = true;
    world.try_start_do_after(args, event);
}

/// Глаголы количества (`AddSetTransferVerbs`).
fn transfer_verbs(world: &mut World, held: Entity, event: &mut GetVerbs) {
    if event.kind != VerbKind::Alternative || !event.can_access || !event.can_interact {
        return;
    }
    let Some(component) = world.get::<SolutionTransfer>(held).cloned() else {
        return;
    };
    if !component.can_change_transfer_amount
        || world.get::<crate::hands::Hands>(event.user).is_none()
    {
        return;
    }
    let user = event.user;
    let category = Some("verb-categories-transfer".to_owned());
    let mut custom = Verb::new(
        VerbKind::Alternative,
        loc(world, "comp-solution-transfer-verb-custom-amount", &[]),
        move |world| {
            world.open_ui(held, TRANSFER_AMOUNT_UI_KEY, user);
        },
    );
    custom.category = category.clone();
    custom.priority = 1;
    event.verbs.push(custom);
    let mut priority = 0;
    for amount in DEFAULT_TRANSFER_AMOUNTS.map(FixedPoint2::new) {
        if amount < component.minimum_transfer_amount || amount > component.maximum_transfer_amount
        {
            continue;
        }
        let mut verb = Verb::new(
            VerbKind::Alternative,
            loc(
                world,
                "comp-solution-transfer-verb-amount",
                &[("amount", amount.to_string().into())],
            ),
            move |world| set_transfer_amount(world, held, amount, Some(user)),
        );
        verb.category = category.clone();
        verb.priority = priority;
        priority -= 1;
        event.verbs.push(verb);
    }
}

/// Задать количество за раз с сообщением.
pub fn set_transfer_amount(
    world: &mut World,
    held: Entity,
    amount: FixedPoint2,
    user: Option<Entity>,
) {
    let Some(mut component) = world.get_mut::<SolutionTransfer>(held) else {
        return;
    };
    let amount = amount.clamp(
        component.minimum_transfer_amount,
        component.maximum_transfer_amount,
    );
    component.transfer_amount = amount;
    if let Some(user) = user {
        popup(
            world,
            "comp-solution-transfer-set-amount",
            &[("amount", amount.to_string().into())],
            held,
            user,
        );
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<SolutionTransfer>();
    world.register_net_serde::<SolutionTransfer>();
    world.register_ui_message::<TransferAmountSetValue>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<SolutionTransfer, AfterInteract>(on_after_interact);
    world.subscribe::<SolutionTransfer, GetVerbs>(transfer_verbs);
    world.subscribe::<SolutionTransfer, BoundUiMessage<TransferAmountSetValue>>(
        |world, held, event| {
            set_transfer_amount(world, held, event.message.value, Some(event.actor));
        },
    );
    world.subscribe::<SolutionTransfer, DoAfterDone<SolutionDrainTransferDoAfter>>(
        |world, held, event| {
            let Some(target) = event.target.filter(|_| !event.cancelled) else {
                return;
            };
            // Заново: за время задержки всё могло измениться. В SS14 здесь проверяется
            // `CanReceive`, а не `CanSend`.
            let can = world
                .get::<SolutionTransfer>(held)
                .is_some_and(|c| c.can_receive);
            if let (true, Some((own, theirs))) = (can, transferrable(world, held, target)) {
                drain_transfer(
                    world,
                    SolutionTransferData {
                        user: event.user,
                        source_entity: held,
                        source: own,
                        target_entity: target,
                        target: theirs,
                        amount: event.event.amount,
                    },
                );
            }
        },
    );
    world.subscribe::<SolutionTransfer, DoAfterDone<SolutionRefillTransferDoAfter>>(
        |world, held, event| {
            let Some(target) = event.target.filter(|_| !event.cancelled) else {
                return;
            };
            let can = world
                .get::<SolutionTransfer>(held)
                .is_some_and(|c| c.can_receive);
            if let (true, Some((theirs, own))) = (can, transferrable(world, target, held)) {
                refill_transfer(
                    world,
                    SolutionTransferData {
                        user: event.user,
                        source_entity: target,
                        source: theirs,
                        target_entity: held,
                        target: own,
                        amount: event.event.amount,
                    },
                );
            }
        },
    );
}

#[cfg(test)]
mod tests;
