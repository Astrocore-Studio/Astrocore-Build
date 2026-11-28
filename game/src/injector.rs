//! Инъекторы: шприцы, пипетки, гипоспреи, реактивные инъекторы — один компонент с режимами
//! из прототипов `injectorMode` (впрыск, забор, «динамический» — забирает из ёмкостей и
//! впрыскивает в тела). Щелчок по телу — действие с задержкой (время режима, плюс за объём
//! сверх порога, себе и лежащему — быстрее), по ёмкости — забор или долив; удар — тоже
//! впрыск; в руке — смена режима или впрыск себе (гипоспрей). Забор у тела — из крови.
//! Полный шприц сам переходит на впрыск, пустой — на забор. Количество за раз — глаголами.
//! Впрыснутое вызывает реакцию тела (`Injection`). Образец — `InjectorSystem`,
//! `InjectorComponent`, `InjectorModePrototype`, `InjectorDoAfterEvent`,
//! `SelfBeforeInjectEvent`, `TargetBeforeInjectEvent` SS14 @ 2f4a2aad.
//!
//! Как в SS14: при заборе с белым списком реагентов из тела или при неудаче временно
//! отделённые реагенты пропадают; сообщение «нельзя набрать» при полном шприце в момент
//! забора показывается ключом без перевода. Нет пока: закрытых ёмкостей (`Openable`, считаются
//! открытыми), следов ДНК (`TransferDna`), состояния в строке предмета на клиенте.

use std::collections::HashMap;
use std::sync::Arc;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_loc::{EntityNameWorldExt, LocArg};
use astrocore_net::NetRegistryWorldExt;
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::bloodstream::{self, Bloodstream};
use crate::combat::MeleeHit;
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterWorldExt};
use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::interaction::{AfterInteract, UseInHand};
use crate::popups::{PopupType, PopupWorldExt};
use crate::reactive::{ReactionMethod, do_entity_reaction};
use crate::solution::Solution;
use crate::solution_container::{self as containers, SolutionComponent};
use crate::standing::StandingWorldExt;
use crate::verbs::{GetVerbs, Verb, VerbKind};

fn injector_solution() -> String {
    "injector".into()
}
fn five() -> Option<FixedPoint2> {
    Some(FixedPoint2::new(5))
}
fn yes() -> bool {
    true
}
fn movement_threshold() -> f32 {
    0.1
}

/// Инъектор (`InjectorComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Injector {
    #[serde(default = "injector_solution")]
    pub solution_name: String,
    /// `None` — весь раствор за раз (автоинъекторы).
    #[serde(default = "five")]
    pub current_transfer_amount: Option<FixedPoint2>,
    pub active_mode_proto_id: String,
    pub allowed_modes: Vec<String>,
    #[serde(default)]
    pub ignore_mobs: bool,
    #[serde(default = "yes")]
    pub ignore_closed: bool,
    #[serde(default)]
    pub reagent_whitelist: Option<Vec<String>>,
    #[serde(default = "yes")]
    pub need_hand: bool,
    #[serde(default = "yes")]
    pub break_on_hand_change: bool,
    #[serde(default = "movement_threshold")]
    pub movement_threshold: f32,
}

/// Что делает режим (`InjectorBehavior`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InjectorBehavior {
    #[default]
    Inject,
    Draw,
    Dynamic,
}

fn amounts() -> Vec<FixedPoint2> {
    [1, 5, 10, 15].map(FixedPoint2::new).to_vec()
}
fn mob_time() -> f32 {
    5.0
}
fn half() -> f32 {
    0.5
}
fn delay_per_volume() -> f32 {
    0.1
}
fn ignore_delay() -> FixedPoint2 {
    FixedPoint2::new(5)
}
fn user_attempt() -> String {
    "injector-component-needle-injecting-user".into()
}
fn target_attempt() -> String {
    "injector-component-needle-injecting-target".into()
}

/// Режим инъектора (`InjectorModePrototype`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InjectorMode {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub inject_on_use: bool,
    #[serde(default = "amounts")]
    pub transfer_amounts: Vec<FixedPoint2>,
    /// С.
    #[serde(default = "mob_time")]
    pub mob_time: f32,
    #[serde(default)]
    pub container_draw_time: f32,
    #[serde(default = "half")]
    pub downed_modifier: f32,
    #[serde(default = "half")]
    pub self_modifier: f32,
    #[serde(default = "delay_per_volume")]
    pub delay_per_volume: f32,
    #[serde(default = "ignore_delay")]
    pub ignore_delay_for_volume: FixedPoint2,
    #[serde(default = "user_attempt")]
    pub popup_user_attempt: String,
    #[serde(default = "target_attempt")]
    pub popup_target_attempt: String,
    #[serde(default)]
    pub behavior: InjectorBehavior,
    #[serde(default)]
    pub inject_sound: Option<SoundSpecifier>,
    #[serde(default)]
    pub inject_popup_target: Option<String>,
}

/// Режимы по имени. Ресурс `World`.
#[derive(Resource, Debug, Default)]
pub struct InjectorModes(HashMap<String, Option<Arc<InjectorMode>>>);

/// Режим из прототипов (`ProtoMan.Resolve`).
pub fn mode(world: &mut World, id: &str) -> Option<Arc<InjectorMode>> {
    if let Some(found) = world
        .get_resource::<InjectorModes>()
        .and_then(|m| m.0.get(id).cloned())
    {
        return found;
    }
    let parsed = world
        .get_resource::<Prototypes>()
        .and_then(|p| p.resolved_node("injectorMode", id).cloned())
        .and_then(|node| astrocore_data::from_node::<InjectorMode>(&node).ok())
        .map(Arc::new);
    world
        .get_resource_or_insert_with(InjectorModes::default)
        .0
        .insert(id.to_owned(), parsed.clone());
    parsed
}

fn active_mode(world: &mut World, injector: Entity) -> Option<Arc<InjectorMode>> {
    let id = world
        .get::<Injector>(injector)?
        .active_mode_proto_id
        .clone();
    mode(world, &id)
}

/// Действие с задержкой инъектора (`InjectorDoAfterEvent`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InjectorDoAfter;

/// Перед впрыском — на вкалывающем (`SelfBeforeInjectEvent`): можно отменить, подменить
/// цель или сообщение.
#[derive(Debug, Clone, PartialEq)]
pub struct SelfBeforeInject {
    pub user: Entity,
    pub injector: Entity,
    pub target: Entity,
    pub cancelled: bool,
    pub override_message: Option<String>,
}

/// Перед впрыском — на цели (`TargetBeforeInjectEvent`).
#[derive(Debug, Clone, PartialEq)]
pub struct TargetBeforeInject {
    pub user: Entity,
    pub injector: Entity,
    pub target: Entity,
    pub cancelled: bool,
    pub override_message: Option<String>,
}

fn name(world: &World, entity: Entity) -> LocArg {
    world.entity_arg(entity).into()
}

fn popup(world: &mut World, key: &str, args: &[(&str, LocArg)], on: Entity, user: Entity) {
    let text = loc(world, key, args);
    world.popup_entity_for(&text, on, user, PopupType::Small);
}

fn injector_solution_entity(world: &mut World, injector: Entity) -> Option<Entity> {
    let name = world.get::<Injector>(injector)?.solution_name.clone();
    containers::try_get_solution(world, injector, &name)
}

fn solution_of(world: &World, soln: Entity) -> Solution {
    containers::solution(world, soln)
        .cloned()
        .unwrap_or_default()
}

fn log(world: &mut World, kind: LogType, message: String) {
    add_admin_log(world, kind, LogImpact::Medium, &message, &[], &[]);
}

fn pretty(world: &World, entity: Entity) -> String {
    world.entity_name(entity)
}

fn pretty_solution(solution: &Solution) -> String {
    solution
        .contents
        .iter()
        .map(|(id, quantity)| format!("{}: {quantity}", id.prototype))
        .collect::<Vec<_>>()
        .join(", ")
}

/// В руке: впрыск себе или смена режима (`OnInjectorUse`).
fn on_use_in_hand(world: &mut World, injector: Entity, event: &mut UseInHand) {
    if event.handled {
        return;
    }
    let Some(active) = active_mode(world, injector) else {
        return;
    };
    if active.inject_on_use {
        try_mobs_do_after(world, injector, event.user, event.user);
    } else {
        toggle_mode(world, injector, event.user);
    }
    event.handled = true;
    event.apply_delay = false;
}

/// Щелчок инъектором (`OnInjectorAfterInteract`).
fn on_after_interact(world: &mut World, injector: Entity, event: &mut AfterInteract) {
    if event.handled || !event.can_reach {
        return;
    }
    let Some(target) = event.target else {
        return;
    };
    if world.get::<Bloodstream>(target).is_some() {
        if world
            .get::<Injector>(injector)
            .is_some_and(|i| i.ignore_mobs)
        {
            popup(
                world,
                "injector-component-ignore-mobs",
                &[],
                target,
                event.user,
            );
            return;
        }
        event.handled |= try_mobs_do_after(world, injector, event.user, target);
        return;
    }
    event.handled |= try_container_do_after(world, injector, event.user, target);
}

fn start_do_after(
    world: &mut World,
    injector: Entity,
    user: Entity,
    target: Entity,
    delay: f32,
) -> bool {
    let Some(component) = world.get::<Injector>(injector).cloned() else {
        return false;
    };
    let mut args = DoAfterArgs::new(
        user,
        f64::from(delay),
        Some(target),
        Some(injector),
        Some(injector),
    );
    args.break_on_move = true;
    args.break_on_damage = true;
    args.need_hand = component.need_hand;
    args.break_on_hand_change = component.break_on_hand_change;
    args.movement_threshold = component.movement_threshold;
    world.try_start_do_after(args, InjectorDoAfter) != crate::do_after::DoAfterStart::Refused
}

/// Задержка для тела (`GetMobsDoAfterTime`); `None` — нечего впрыскивать.
fn mobs_do_after_time(
    world: &mut World,
    injector: Entity,
    user: Entity,
    target: Entity,
) -> Option<(f32, FixedPoint2)> {
    let soln = injector_solution_entity(world, injector)?;
    let active = active_mode(world, injector)?;
    let current = world.get::<Injector>(injector)?.current_transfer_amount;
    let solution = solution_of(world, soln);
    let mut time = active.mob_time;
    let amount = match (active.behavior, current) {
        (InjectorBehavior::Draw, Some(current)) => current.min(solution.available_volume()),
        _ => {
            if solution.volume == FixedPoint2::ZERO {
                popup(
                    world,
                    "injector-component-empty-message",
                    &[("injector", name(world, injector))],
                    target,
                    user,
                );
                return None;
            }
            current.unwrap_or(solution.volume).min(solution.volume)
        }
    };
    let over = (amount - active.ignore_delay_for_volume).max(FixedPoint2::ZERO);
    time += active.delay_per_volume * over.float();
    if user == target {
        time *= active.self_modifier;
    } else if world.is_down(target) {
        time *= active.downed_modifier;
    }
    Some((time, amount))
}

/// Начать впрыск или забор у тела с сообщениями (`TryMobsDoAfter`).
fn try_mobs_do_after(world: &mut World, injector: Entity, user: Entity, target: Entity) -> bool {
    if crate::use_delay::is_delayed(world, injector, crate::use_delay::DEFAULT_ID) {
        return false;
    }
    let Some((time, amount)) = mobs_do_after_time(world, injector, user, target) else {
        return false;
    };
    if !start_do_after(world, injector, user, target, time) {
        return false;
    }
    // Мгновенный — без сообщений и записей о попытке.
    if time == 0.0 {
        return true;
    }
    let Some(soln) = injector_solution_entity(world, injector) else {
        return false;
    };
    let Some(active) = active_mode(world, injector) else {
        return false;
    };
    let solution = pretty_solution(&solution_of(world, soln));
    popup(world, &active.popup_user_attempt, &[], target, user);
    let draw = active.behavior == InjectorBehavior::Draw;
    let (user_name, target_name) = (pretty(world, user), pretty(world, target));
    if user == target {
        if draw {
            log(
                world,
                LogType::ForceFeed,
                format!("{user_name} is attempting to draw {amount} units from themselves."),
            );
        } else {
            log(
                world,
                LogType::Ingestion,
                format!(
                    "{user_name} is attempting to inject themselves with a solution {solution}."
                ),
            );
        }
        return true;
    }
    popup(
        world,
        &active.popup_target_attempt,
        &[("user", name(world, user))],
        user,
        target,
    );
    let message = if draw {
        format!("{user_name} is attempting to draw {amount} units from {target_name}")
    } else {
        format!("{user_name} is attempting to inject {target_name} with a solution {solution}")
    };
    log(world, LogType::ForceFeed, message);
    true
}

/// Забор или долив ёмкости (`TryContainerDoAfter`, `GetContainerDoAfterTime`).
fn try_container_do_after(
    world: &mut World,
    injector: Entity,
    user: Entity,
    target: Entity,
) -> bool {
    let Some(active) = active_mode(world, injector) else {
        return false;
    };
    let mut time = 0.0;
    if matches!(
        active.behavior,
        InjectorBehavior::Draw | InjectorBehavior::Dynamic
    ) {
        let available = injector_solution_entity(world, injector).map_or(FixedPoint2::ZERO, |s| {
            solution_of(world, s).available_volume()
        });
        if available == FixedPoint2::ZERO {
            popup(
                world,
                "injector-component-cannot-toggle-draw-message",
                &[],
                user,
                user,
            );
            return false;
        }
        let Some(drawable) = containers::drawable_solution(world, target) else {
            popup(
                world,
                "injector-component-cannot-draw-message",
                &[("target", name(world, target))],
                injector,
                user,
            );
            return false;
        };
        if solution_of(world, drawable).volume == FixedPoint2::ZERO {
            popup(
                world,
                "injector-component-target-is-empty-message",
                &[("target", name(world, target))],
                injector,
                user,
            );
            return false;
        }
        time = active.container_draw_time;
    }
    start_do_after(world, injector, user, target, time)
}

/// Итог действия по режиму (`TryUseInjector`).
fn try_use_injector(world: &mut World, injector: Entity, user: Entity, target: Entity) -> bool {
    let Some(active) = active_mode(world, injector) else {
        return false;
    };
    // Закрытых ёмкостей пока нет (`Openable`).
    let open = true;
    let mut message = if target == user {
        "injector-component-cannot-transfer-message-self"
    } else {
        "injector-component-cannot-transfer-message"
    };
    match active.behavior {
        InjectorBehavior::Inject => {
            if open && let Some(soln) = containers::injectable_solution(world, target) {
                return try_inject(world, injector, user, target, soln);
            }
            if open && let Some(soln) = containers::refillable_solution(world, target) {
                return try_inject(world, injector, user, target, soln);
            }
        }
        InjectorBehavior::Draw => {
            if world.get::<Bloodstream>(target).is_some()
                && let Some(blood) = bloodstream::blood_solution(world, target)
            {
                return try_draw(world, injector, user, target, blood, true);
            }
            if open && let Some(soln) = containers::drawable_solution(world, target) {
                return try_draw(world, injector, user, target, soln, false);
            }
            message = if target == user {
                "injector-component-cannot-draw-message-self"
            } else {
                "injector-component-cannot-draw-message"
            };
            popup(
                world,
                message,
                &[("target", name(world, target))],
                injector,
                user,
            );
        }
        InjectorBehavior::Dynamic => {
            if world.get::<Bloodstream>(target).is_some()
                && let Some(soln) = containers::injectable_solution(world, target)
            {
                return try_inject(world, injector, user, target, soln);
            }
            if open && let Some(soln) = containers::drawable_solution(world, target) {
                return try_draw(world, injector, user, target, soln, false);
            }
        }
    }
    popup(
        world,
        message,
        &[("target", name(world, target))],
        injector,
        user,
    );
    false
}

/// Впрыснуть раствор инъектора в цель (`TryInject`).
fn try_inject(
    world: &mut World,
    injector: Entity,
    user: Entity,
    target: Entity,
    target_solution: Entity,
) -> bool {
    let soln = injector_solution_entity(world, injector);
    let volume = soln.map_or(FixedPoint2::ZERO, |s| solution_of(world, s).volume);
    let Some(soln) = soln.filter(|_| volume > FixedPoint2::ZERO) else {
        popup(
            world,
            "injector-component-empty-message",
            &[("injector", name(world, injector))],
            user,
            user,
        );
        return false;
    };
    let Some(active) = active_mode(world, injector) else {
        return false;
    };
    let mut self_event = SelfBeforeInject {
        user,
        injector,
        target,
        cancelled: false,
        override_message: None,
    };
    world.raise(user, &mut self_event);
    if self_event.cancelled {
        if let Some(text) = &self_event.override_message {
            world.popup_entity_for(text, user, user, PopupType::Small);
        }
        return true;
    }
    let target = self_event.target;
    let mut target_event = TargetBeforeInject {
        user,
        injector,
        target,
        cancelled: false,
        override_message: None,
    };
    world.raise(target, &mut target_event);
    if target_event.cancelled {
        let user_text = loc(world, "injector-component-blocked-user", &[]);
        let other_text = loc(
            world,
            "injector-component-blocked-other",
            &[("target", name(world, target)), ("user", name(world, user))],
        );
        world.popup_entity_split(
            &user_text,
            &other_text,
            target,
            user,
            PopupType::SmallCaution,
        );
        return true;
    }
    let current = world
        .get::<Injector>(injector)
        .and_then(|i| i.current_transfer_amount);
    let planned = current.unwrap_or(volume).min(volume);
    let available = solution_of(world, target_solution).available_volume();
    let real = planned.min(available);
    if real <= FixedPoint2::ZERO {
        let key = if target == user {
            "injector-component-target-already-full-message-self"
        } else {
            "injector-component-target-already-full-message"
        };
        popup(
            world,
            key,
            &[("target", name(world, target))],
            injector,
            user,
        );
        return false;
    }
    let stack = world.get::<crate::stack::Stack>(target).map(|s| s.count);
    let removed = match stack {
        Some(count) if count > 0 => split_stack_solution(world, soln, real, count),
        _ => containers::split_solution(world, soln, real),
    };
    do_entity_reaction(world, target, &removed, ReactionMethod::Injection);
    // `Inject` и `Refill` SS14 — добавление в найденный раствор.
    containers::add_solution(world, target_solution, &removed);
    let key = if target == user {
        "injector-component-inject-success-message-self"
    } else {
        "injector-component-inject-success-message"
    };
    let text = match self_event
        .override_message
        .or(target_event.override_message)
    {
        Some(text) => text,
        None => loc(
            world,
            key,
            &[
                ("amount", removed.volume.to_string().into()),
                ("target", name(world, target)),
            ],
        ),
    };
    world.popup_entity_for(&text, target, user, PopupType::Small);
    if let Some(key) = &active.inject_popup_target
        && target != user
    {
        popup(world, key, &[], target, target);
    }
    if let Some(sound) = &active.inject_sound {
        world.play_entity(sound, injector, None);
    }
    let message = format!(
        "{} injected {} with a solution {} using a {}",
        pretty(world, user),
        pretty(world, target),
        pretty_solution(&removed),
        pretty(world, injector)
    );
    log(world, LogType::ForceFeed, message);
    after_inject(world, injector, user);
    true
}

/// Отделить для стопки: на один предмет, остаток пропадает (`SplitStackSolution`).
fn split_stack_solution(
    world: &mut World,
    soln: Entity,
    quantity: FixedPoint2,
    count: i32,
) -> Solution {
    containers::modify(world, soln, |solution, _| {
        let split = solution.split(quantity / FixedPoint2::new(count));
        solution.split(quantity - split.volume);
        split
    })
    .unwrap_or_default()
}

/// Набрать из цели (`TryDraw`); у тела — из крови (`DrawFromBlood`).
fn try_draw(
    world: &mut World,
    injector: Entity,
    user: Entity,
    target: Entity,
    target_solution: Entity,
    from_blood: bool,
) -> bool {
    let soln = injector_solution_entity(world, injector);
    let available = soln.map_or(FixedPoint2::ZERO, |s| {
        solution_of(world, s).available_volume()
    });
    let Some(soln) = soln.filter(|_| available > FixedPoint2::ZERO) else {
        // Как в SS14: ключ без перевода.
        world.popup_entity_for(
            "injector-component-cannot-toggle-draw-message",
            user,
            user,
            PopupType::Small,
        );
        return false;
    };
    let Some(component) = world.get::<Injector>(injector).cloned() else {
        return false;
    };
    // С белым списком — остальные реагенты на время отделяются от раствора цели.
    let mut set_aside = Solution::default();
    if let Some(whitelist) = &component.reagent_whitelist
        && let Some(mut target_component) = world.get_mut::<SolutionComponent>(target_solution)
    {
        let names: Vec<&str> = whitelist.iter().map(String::as_str).collect();
        let volume = target_component.solution.volume;
        set_aside = target_component.solution.split_without(volume, &names);
    }
    let planned = component
        .current_transfer_amount
        .unwrap_or(FixedPoint2::new(5));
    let target_volume = solution_of(world, target_solution).volume;
    let real = planned.min(target_volume).min(available);
    if real <= FixedPoint2::ZERO {
        let key = if target == user {
            "injector-component-target-is-empty-message-self"
        } else {
            "injector-component-target-is-empty-message"
        };
        popup(
            world,
            key,
            &[("target", name(world, target))],
            injector,
            user,
        );
        return false;
    }
    if from_blood {
        let blood = containers::split_solution(world, target_solution, real);
        containers::try_add_solution(world, soln, &blood);
        draw_success(world, injector, user, target, real);
        return true;
    }
    let removed = containers::split_solution(world, target_solution, real);
    containers::try_add_solution(world, target_solution, &set_aside);
    if !containers::try_add_solution(world, soln, &removed) {
        return false;
    }
    draw_success(world, injector, user, target, removed.volume);
    true
}

fn draw_success(
    world: &mut World,
    injector: Entity,
    user: Entity,
    target: Entity,
    amount: FixedPoint2,
) {
    let key = if target == user {
        "injector-component-draw-success-message-self"
    } else {
        "injector-component-draw-success-message"
    };
    popup(
        world,
        key,
        &[
            ("amount", amount.to_string().into()),
            ("target", name(world, target)),
        ],
        target,
        user,
    );
    after_draw(world, injector, user);
}

/// Пауза использования и переход пустого на забор (`AfterInject`).
fn after_inject(world: &mut World, injector: Entity, user: Entity) {
    crate::use_delay::try_reset_delay(world, injector, false, crate::use_delay::DEFAULT_ID);
    let Some(soln) = injector_solution_entity(world, injector) else {
        return;
    };
    if solution_of(world, soln).volume != FixedPoint2::ZERO {
        return;
    }
    switch_to_behavior(world, injector, user, InjectorBehavior::Draw);
}

/// Переход полного на впрыск (`AfterDraw`).
fn after_draw(world: &mut World, injector: Entity, user: Entity) {
    let Some(soln) = injector_solution_entity(world, injector) else {
        return;
    };
    if solution_of(world, soln).available_volume() != FixedPoint2::ZERO {
        return;
    }
    switch_to_behavior(world, injector, user, InjectorBehavior::Inject);
}

fn switch_to_behavior(
    world: &mut World,
    injector: Entity,
    user: Entity,
    behavior: InjectorBehavior,
) {
    let Some(active) = active_mode(world, injector) else {
        return;
    };
    if active.behavior == InjectorBehavior::Dynamic {
        return;
    }
    let allowed = world
        .get::<Injector>(injector)
        .map(|i| i.allowed_modes.clone())
        .unwrap_or_default();
    for id in allowed {
        if mode(world, &id).is_some_and(|m| m.behavior == behavior) {
            set_mode(world, injector, user, &id, false);
            return;
        }
    }
}

/// Задать режим (`ToggleMode` с режимом).
pub fn set_mode(world: &mut World, injector: Entity, user: Entity, id: &str, show_popup: bool) {
    let Some(mut component) = world.get_mut::<Injector>(injector) else {
        return;
    };
    if !component.allowed_modes.iter().any(|m| m == id) {
        return;
    }
    component.active_mode_proto_id = id.to_owned();
    let Some(new_mode) = mode(world, id) else {
        return;
    };
    if !show_popup {
        return;
    }
    let mode_name = loc(world, &new_mode.name, &[]);
    popup(
        world,
        "injector-component-mode-changed-text",
        &[("mode", mode_name.into())],
        user,
        user,
    );
}

/// Следующий режим с другим поведением, если он возможен (`ToggleMode`).
pub fn toggle_mode(world: &mut World, injector: Entity, user: Entity) {
    let Some(active) = active_mode(world, injector) else {
        return;
    };
    let allowed = world
        .get::<Injector>(injector)
        .map(|i| i.allowed_modes.clone())
        .unwrap_or_default();
    let mut error = None;
    for id in allowed {
        let Some(proto) = mode(world, &id) else {
            continue;
        };
        if proto.behavior == active.behavior {
            continue;
        }
        let Some(soln) = injector_solution_entity(world, injector) else {
            continue;
        };
        let solution = solution_of(world, soln);
        if proto.behavior == InjectorBehavior::Inject && solution.volume == FixedPoint2::ZERO {
            error = Some("injector-component-cannot-toggle-inject-message");
            continue;
        }
        if proto.behavior == InjectorBehavior::Draw
            && solution.available_volume() == FixedPoint2::ZERO
        {
            error = Some("injector-component-cannot-toggle-draw-message");
            continue;
        }
        set_mode(world, injector, user, &id, true);
        return;
    }
    if let Some(key) = error {
        popup(world, key, &[], user, user);
    }
}

fn set_amount(world: &mut World, injector: Entity, user: Entity, amount: FixedPoint2) {
    if let Some(mut component) = world.get_mut::<Injector>(injector) {
        component.current_transfer_amount = Some(amount);
    }
    popup(
        world,
        "comp-solution-transfer-set-amount",
        &[("amount", amount.to_string().into())],
        user,
        user,
    );
}

/// Глаголы количества и смены режима (`AddVerbs`).
fn add_verbs(world: &mut World, injector: Entity, event: &mut GetVerbs) {
    if event.kind != VerbKind::Alternative
        || !event.can_access
        || !event.can_interact
        || world.get::<crate::hands::Hands>(event.user).is_none()
    {
        return;
    }
    let Some(active) = active_mode(world, injector) else {
        return;
    };
    let Some(component) = world.get::<Injector>(injector).cloned() else {
        return;
    };
    let user = event.user;
    let (Some(min), Some(max)) = (
        active.transfer_amounts.iter().min().copied(),
        active.transfer_amounts.iter().max().copied(),
    ) else {
        return;
    };
    let toggle = if component.current_transfer_amount == Some(max) {
        min
    } else {
        max
    };
    let category = Some("verb-categories-transfer".to_owned());
    let mut priority = 0;
    if active.transfer_amounts.len() > 1 {
        let mut verb = Verb::new(
            VerbKind::Alternative,
            loc(
                world,
                "comp-solution-transfer-verb-toggle",
                &[("amount", toggle.to_string().into())],
            ),
            move |world| set_amount(world, injector, user, toggle),
        );
        verb.category = category.clone();
        verb.priority = priority;
        event.verbs.push(verb);
        // Как в SS14: все глаголы количеств — с одним приоритетом.
        priority -= 1;
        for amount in active.transfer_amounts.iter().copied() {
            let mut verb = Verb::new(
                VerbKind::Alternative,
                loc(
                    world,
                    "comp-solution-transfer-verb-amount",
                    &[("amount", amount.to_string().into())],
                ),
                move |world| set_amount(world, injector, user, amount),
            );
            verb.category = category.clone();
            verb.priority = priority;
            event.verbs.push(verb);
        }
    }
    if !active.inject_on_use || component.allowed_modes.len() <= 1 {
        return;
    }
    let mut verb = Verb::new(
        VerbKind::Alternative,
        loc(world, "injector-toggle-verb-text", &[]),
        move |world| toggle_mode(world, injector, user),
    );
    verb.priority = priority;
    event.verbs.push(verb);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Injector>();
    world.register_net_serde::<Injector>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Injector, UseInHand>(on_use_in_hand);
    world.subscribe::<Injector, AfterInteract>(on_after_interact);
    world.subscribe::<Injector, DoAfterDone<InjectorDoAfter>>(|world, injector, event| {
        if event.cancelled || event.handled {
            return;
        }
        let Some(target) = event.target else {
            return;
        };
        event.handled |= try_use_injector(world, injector, event.user, target);
    });
    world.subscribe::<Injector, MeleeHit>(|world, injector, event| {
        if let Some(target) = event.hit_entities.first().copied() {
            try_mobs_do_after(world, injector, event.user, target);
        }
    });
    world.subscribe::<Injector, GetVerbs>(add_verbs);
}

#[cfg(test)]
mod tests;
