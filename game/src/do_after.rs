//! Действия с задержкой (DoAfter): действие завершается через время, если за это время его
//! не прервали — движением, расстоянием до цели, сменой руки, уроном. Образец —
//! `Content.Shared/DoAfter` SS14 @ 2f4a2aad (`SharedDoAfterSystem`, `DoAfterArgs`).
//!
//! Система, начавшая действие, получает итог событием [`DoAfterDone<E>`] со своим `E`:
//! `cancelled` — прервано. Клиенту уходит только вид полосы прогресса
//! ([`DoAfterBars`]).

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::sync::Arc;

use astrocore_ecs::{
    EventWorldExt, GameTime, GameWorldExt, LifecycleWorldExt, RegistryWorldExt, Update,
};
use astrocore_map::{MapCoordinates, TransformWorldExt};
use astrocore_net::NetRegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::action_blocker::ActionBlockerWorldExt;
use crate::hands::{Hands, HandsWorldExt};
use crate::interaction::InteractionWorldExt;
use crate::tag::TagWorldExt;

/// Тег, с которым действия завершаются сразу (`InstantDoAfters`).
pub const INSTANT_DO_AFTERS_TAG: &str = "InstantDoAfters";

/// Сколько секунд завершённое или прерванное действие ещё показывается (`ExcessTime`).
pub const EXCESS_TIME: f64 = 0.5;

/// Как часто спрашивать системы, можно ли продолжать (`AttemptFrequency`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AttemptFrequency {
    #[default]
    Never,
    StartAndEnd,
    EveryTick,
}

/// Что делает два действия одинаковыми (`DuplicateConditions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuplicateConditions {
    pub same_target: bool,
    pub same_tool: bool,
    pub same_event: bool,
}

impl DuplicateConditions {
    pub const ALL: Self = Self {
        same_target: true,
        same_tool: true,
        same_event: true,
    };
}

/// Параметры действия (`DoAfterArgs`). Значения по умолчанию — как в SS14.
#[derive(Debug, Clone)]
pub struct DoAfterArgs {
    pub user: Entity,
    /// Длительность, секунды.
    pub delay: f64,
    pub target: Option<Entity>,
    pub used: Option<Entity>,
    /// На ком поднять итог; без неё — только широковещательно, если `broadcast`.
    pub event_target: Option<Entity>,
    pub broadcast: bool,
    /// Полоса прогресса не показывается.
    pub hidden: bool,
    pub attempt_frequency: AttemptFrequency,
    pub need_hand: bool,
    pub need_free_hand: bool,
    pub need_any_free_hand: bool,
    pub break_on_hand_change: bool,
    pub break_on_drop_item: bool,
    pub break_on_move: bool,
    pub movement_threshold: f32,
    /// Действующий и цель (инструмент) не дальше этого.
    pub distance_threshold: Option<f32>,
    pub break_on_damage: bool,
    pub damage_threshold: f32,
    pub require_can_interact: bool,
    pub block_duplicate: bool,
    pub cancel_duplicate: bool,
    pub duplicate_condition: DuplicateConditions,
}

impl DoAfterArgs {
    /// Действие пользователя с целью, итог поднимается на `event_target`.
    pub fn new(
        user: Entity,
        delay: f64,
        target: Option<Entity>,
        used: Option<Entity>,
        event_target: Option<Entity>,
    ) -> Self {
        Self {
            user,
            delay,
            target,
            used,
            event_target,
            broadcast: false,
            hidden: false,
            attempt_frequency: AttemptFrequency::Never,
            need_hand: false,
            need_free_hand: false,
            need_any_free_hand: false,
            break_on_hand_change: true,
            break_on_drop_item: true,
            break_on_move: false,
            movement_threshold: 0.3,
            distance_threshold: Some(1.5),
            break_on_damage: false,
            damage_threshold: 1.0,
            require_can_interact: true,
            block_duplicate: true,
            cancel_duplicate: true,
            duplicate_condition: DuplicateConditions::ALL,
        }
    }
}

/// Итог действия. Поднимается на `event_target` (и широковещательно при `broadcast`).
#[derive(Debug, Clone)]
pub struct DoAfterDone<E> {
    pub event: E,
    pub user: Entity,
    pub target: Option<Entity>,
    pub used: Option<Entity>,
    /// Прервано.
    pub cancelled: bool,
    pub handled: bool,
    /// Повторить действие с начала (`Repeat`).
    pub repeat: bool,
    /// Задержка повтора, секунды; обработчик может её поменять (`Args.Delay`).
    pub delay: f64,
}

/// Можно ли продолжать (`DoAfterAttemptEvent<T>`). Поднимается как итог.
#[derive(Debug, Clone)]
pub struct DoAfterAttempt<E> {
    pub event: E,
    pub user: Entity,
    pub target: Option<Entity>,
    pub cancelled: bool,
}

/// Итог в стёртом виде: поднимается со своим типом.
trait Raise: Send + Sync {
    /// Итог; при повторе — новая задержка.
    fn done(&self, world: &mut World, args: &DoAfterArgs, cancelled: bool) -> Option<f64>;
    fn attempt(&self, world: &mut World, args: &DoAfterArgs) -> bool;
    fn event_type(&self) -> TypeId;
}

struct Typed<E>(E);

impl<E: Clone + Send + Sync + 'static> Raise for Typed<E> {
    fn done(&self, world: &mut World, args: &DoAfterArgs, cancelled: bool) -> Option<f64> {
        let mut event = DoAfterDone {
            event: self.0.clone(),
            user: args.user,
            target: args.target,
            used: args.used,
            cancelled,
            handled: false,
            repeat: false,
            delay: args.delay,
        };
        match args.event_target.filter(|target| world.is_alive(*target)) {
            Some(target) if args.broadcast => world.raise_and_broadcast(target, &mut event),
            Some(target) => world.raise(target, &mut event),
            None if args.broadcast => world.raise_broadcast(&mut event),
            None => {}
        }
        event.repeat.then_some(event.delay)
    }

    fn attempt(&self, world: &mut World, args: &DoAfterArgs) -> bool {
        let mut event = DoAfterAttempt {
            event: self.0.clone(),
            user: args.user,
            target: args.target,
            cancelled: false,
        };
        match args.event_target.filter(|target| world.is_alive(*target)) {
            Some(target) if args.broadcast => world.raise_and_broadcast(target, &mut event),
            Some(target) => world.raise(target, &mut event),
            None => world.raise_broadcast(&mut event),
        }
        !event.cancelled
    }

    fn event_type(&self) -> TypeId {
        (&self.0 as &dyn Any).type_id()
    }
}

/// Одно действие (`DoAfter`).
#[derive(Clone)]
struct DoAfter {
    args: DoAfterArgs,
    event: Arc<dyn Raise>,
    start_time: f64,
    cancelled_time: Option<f64>,
    completed: bool,
    user_position: Option<MapCoordinates>,
    target_distance: f32,
    initial_hand: Option<String>,
    initial_item: Option<Entity>,
}

/// Действия сущности. В данных — компонент `DoAfter`.
#[derive(Component, Clone, Default, Serialize, Deserialize)]
pub struct DoAfterComponent {
    #[serde(skip)]
    do_afters: BTreeMap<u16, DoAfter>,
    #[serde(skip)]
    next_id: u16,
}

impl std::fmt::Debug for DoAfterComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoAfterComponent")
            .field("count", &self.do_afters.len())
            .finish()
    }
}

/// У сущности идут действия: только такие обходит такт (ADR-0005, п. 5).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ActiveDoAfter;

/// Полоса прогресса для клиента.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoAfterBar {
    pub index: u16,
    /// Начало и длительность, секунды игры.
    pub start_time: f64,
    pub delay: f64,
    pub cancelled_time: Option<f64>,
    pub completed: bool,
}

/// Полосы прогресса действий сущности: передаются клиенту.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DoAfterBars {
    pub bars: Vec<DoAfterBar>,
}

/// Идентификатор действия: сущность и номер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoAfterId {
    pub user: Entity,
    pub index: u16,
}

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

fn is_duplicate(a: &DoAfter, b: &DoAfterArgs, b_type: TypeId) -> bool {
    let conditions = b.duplicate_condition;
    (!conditions.same_target || a.args.target == b.target)
        && (!conditions.same_tool || a.args.used == b.used)
        && (!conditions.same_event || a.event.event_type() == b_type)
}

fn should_cancel(world: &mut World, do_after: &DoAfter) -> bool {
    let args = &do_after.args;
    let exists = |entity: Option<Entity>| entity.is_none_or(|entity| world.is_alive(entity));
    if !exists(args.used) || !exists(args.event_target) || !exists(args.target) {
        return true;
    }
    if args.break_on_move {
        let (Some(start), Some(now)) = (do_after.user_position, world.map_coordinates(args.user))
        else {
            return true;
        };
        if start.map != now.map || start.position.distance(now.position) > args.movement_threshold {
            return true;
        }
        if let Some(target) = args.target.and_then(|target| world.map_coordinates(target))
            && (target.position.distance(now.position) - do_after.target_distance).abs()
                > args.movement_threshold
        {
            return true;
        }
    }
    if let (Some(target), Some(range)) = (args.target, args.distance_threshold)
        && (!world.is_accessible(args.user, target)
            || !world.in_range_unobstructed(args.user, target, range))
    {
        return true;
    }
    if let (Some(used), Some(range)) = (args.used, args.distance_threshold)
        && !world.in_range_unobstructed(args.user, used, range)
    {
        return true;
    }
    if args.attempt_frequency == AttemptFrequency::EveryTick && !do_after.event.attempt(world, args)
    {
        return true;
    }
    if args.need_hand {
        let Some(hands) = world.get::<Hands>(args.user).cloned() else {
            return true;
        };
        if hands.hands.is_empty() {
            return true;
        }
        if args.break_on_drop_item
            && let Some(item) = do_after.initial_item
            && world.hand_holding(args.user, item).is_none()
        {
            return true;
        }
        if args.break_on_hand_change && hands.active_hand_id != do_after.initial_hand {
            return true;
        }
        if args.need_free_hand && world.active_item(args.user).is_some() {
            return true;
        }
        if args.need_any_free_hand && world.empty_hand(args.user).is_none() {
            return true;
        }
    }
    args.require_can_interact && !world.can_interact(args.user, args.target)
}

fn sync_bars(world: &mut World, user: Entity) {
    let Some(component) = world.get::<DoAfterComponent>(user) else {
        return;
    };
    let bars = DoAfterBars {
        bars: component
            .do_afters
            .iter()
            .filter(|(_, d)| !d.args.hidden)
            .map(|(index, d)| DoAfterBar {
                index: *index,
                start_time: d.start_time,
                delay: d.args.delay,
                cancelled_time: d.cancelled_time,
                completed: d.completed,
            })
            .collect(),
    };
    if world.get::<DoAfterBars>(user) != Some(&bars)
        && let Ok(mut target) = world.get_entity_mut(user)
    {
        target.insert(bars);
    }
}

/// Чем кончилась попытка начать действие.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoAfterStart {
    /// Действие идёт.
    Started(DoAfterId),
    /// Задержки нет: действие уже завершено.
    Instant,
    /// Не началось: нет рук, повтор, прервано сразу.
    Refused,
}

impl DoAfterStart {
    /// Действие началось или уже завершилось (`TryStartDoAfter` вернул `true`).
    pub fn ok(self) -> bool {
        self != DoAfterStart::Refused
    }
}

/// Действия с задержкой на `World`.
pub trait DoAfterWorldExt {
    /// Начать действие (`TryStartDoAfter`).
    fn try_start_do_after<E: Clone + Send + Sync + 'static>(
        &mut self,
        args: DoAfterArgs,
        event: E,
    ) -> DoAfterStart;

    /// Прервать действие.
    fn cancel_do_after(&mut self, id: DoAfterId);

    /// Прервать действия, которые прерываются уроном не меньше `amount`.
    fn cancel_do_afters_on_damage(&mut self, user: Entity, amount: f32);

    /// Идёт ли у сущности действие с этим событием.
    fn has_do_after<E: 'static>(&self, user: Entity) -> bool;
}

impl DoAfterWorldExt for World {
    fn try_start_do_after<E: Clone + Send + Sync + 'static>(
        &mut self,
        args: DoAfterArgs,
        event: E,
    ) -> DoAfterStart {
        let user = args.user;
        if self.get::<DoAfterComponent>(user).is_none() {
            return DoAfterStart::Refused;
        }
        let raise: Arc<dyn Raise> = Arc::new(Typed(event));
        // Повторы: прерываются старые и/или не начинается новое (`ProcessDuplicates`).
        let existing: Vec<(u16, bool, bool)> = self
            .get::<DoAfterComponent>(user)
            .map(|component| {
                component
                    .do_afters
                    .iter()
                    .filter(|(_, d)| !d.completed && d.cancelled_time.is_none())
                    .filter(|(_, d)| is_duplicate(d, &args, raise.event_type()))
                    .map(|(index, d)| (*index, d.args.block_duplicate, d.args.cancel_duplicate))
                    .collect()
            })
            .unwrap_or_default();
        let mut blocked = false;
        for (index, block, cancel) in existing {
            blocked |= args.block_duplicate || block;
            if args.cancel_duplicate || cancel {
                self.cancel_do_after(DoAfterId { user, index });
            }
        }
        if blocked {
            return DoAfterStart::Refused;
        }
        let user_position = self.map_coordinates(user);
        let target_distance = match (
            args.target.and_then(|t| self.map_coordinates(t)),
            user_position,
        ) {
            (Some(target), Some(own)) if args.break_on_move => {
                target.position.distance(own.position)
            }
            _ => 0.0,
        };
        let mut do_after = DoAfter {
            args: args.clone(),
            event: raise.clone(),
            start_time: now(self),
            cancelled_time: None,
            completed: false,
            user_position,
            target_distance,
            initial_hand: None,
            initial_item: None,
        };
        if args.need_hand {
            if self.get::<Hands>(user).is_none()
                || (args.need_free_hand && self.active_item(user).is_some())
                || (args.need_any_free_hand && self.empty_hand(user).is_none())
            {
                return DoAfterStart::Refused;
            }
            if args.break_on_hand_change || args.break_on_drop_item {
                do_after.initial_hand = self.active_hand(user);
                do_after.initial_item = self.active_item(user);
            }
        }
        if should_cancel(self, &do_after) {
            return DoAfterStart::Refused;
        }
        if args.attempt_frequency == AttemptFrequency::StartAndEnd && !raise.attempt(self, &args) {
            return DoAfterStart::Refused;
        }
        if args.delay <= 0.0 || self.has_tag(user, INSTANT_DO_AFTERS_TAG) {
            raise.done(self, &args, false);
            return DoAfterStart::Instant;
        }
        let Some(mut component) = self.get_mut::<DoAfterComponent>(user) else {
            return DoAfterStart::Refused;
        };
        let index = component.next_id;
        component.next_id = component.next_id.wrapping_add(1);
        component.do_afters.insert(index, do_after);
        self.entity_mut(user).insert(ActiveDoAfter);
        sync_bars(self, user);
        DoAfterStart::Started(DoAfterId { user, index })
    }

    fn cancel_do_after(&mut self, id: DoAfterId) {
        let time = now(self);
        let Some(mut component) = self.get_mut::<DoAfterComponent>(id.user) else {
            return;
        };
        let Some(do_after) = component.do_afters.get_mut(&id.index) else {
            return;
        };
        if do_after.cancelled_time.is_some() || do_after.completed {
            return;
        }
        do_after.cancelled_time = Some(time);
        let (event, args) = (do_after.event.clone(), do_after.args.clone());
        event.done(self, &args, true);
        sync_bars(self, id.user);
    }

    fn cancel_do_afters_on_damage(&mut self, user: Entity, amount: f32) {
        let breaking: Vec<u16> = self
            .get::<DoAfterComponent>(user)
            .map(|component| {
                component
                    .do_afters
                    .iter()
                    .filter(|(_, d)| d.args.break_on_damage && amount >= d.args.damage_threshold)
                    .map(|(index, _)| *index)
                    .collect()
            })
            .unwrap_or_default();
        for index in breaking {
            self.cancel_do_after(DoAfterId { user, index });
        }
    }

    fn has_do_after<E: 'static>(&self, user: Entity) -> bool {
        self.get::<DoAfterComponent>(user).is_some_and(|component| {
            component.do_afters.values().any(|d| {
                d.cancelled_time.is_none()
                    && !d.completed
                    && d.event.event_type() == TypeId::of::<E>()
            })
        })
    }
}

/// Такт: завершает, прерывает и убирает действия активных сущностей.
fn update(world: &mut World) {
    let time = now(world);
    let users: Vec<Entity> = world
        .query_filtered::<Entity, bevy_ecs::query::With<ActiveDoAfter>>()
        .iter(world)
        .collect();
    for user in users {
        let Some(component) = world.get::<DoAfterComponent>(user) else {
            if let Ok(mut target) = world.get_entity_mut(user) {
                target.remove::<ActiveDoAfter>();
            }
            continue;
        };
        let indices: Vec<u16> = component.do_afters.keys().copied().collect();
        for index in indices {
            let Some(do_after) = world
                .get::<DoAfterComponent>(user)
                .and_then(|component| component.do_afters.get(&index).cloned())
            else {
                continue;
            };
            if let Some(cancelled) = do_after.cancelled_time {
                if time - cancelled > EXCESS_TIME {
                    remove(world, user, index);
                }
                continue;
            }
            if do_after.completed {
                if time - do_after.start_time > do_after.args.delay + EXCESS_TIME {
                    remove(world, user, index);
                }
                continue;
            }
            if should_cancel(world, &do_after) {
                world.cancel_do_after(DoAfterId { user, index });
                continue;
            }
            if time - do_after.start_time >= do_after.args.delay {
                complete(world, user, index, &do_after, time);
            }
        }
        let empty = world
            .get::<DoAfterComponent>(user)
            .is_none_or(|component| component.do_afters.is_empty());
        if empty && let Ok(mut target) = world.get_entity_mut(user) {
            target.remove::<ActiveDoAfter>();
        }
        sync_bars(world, user);
    }
}

fn remove(world: &mut World, user: Entity, index: u16) {
    if let Some(mut component) = world.get_mut::<DoAfterComponent>(user) {
        component.do_afters.remove(&index);
    }
}

fn complete(world: &mut World, user: Entity, index: u16, do_after: &DoAfter, time: f64) {
    if do_after.args.attempt_frequency == AttemptFrequency::StartAndEnd
        && !do_after.event.attempt(world, &do_after.args)
    {
        world.cancel_do_after(DoAfterId { user, index });
        return;
    }
    if let Some(mut component) = world.get_mut::<DoAfterComponent>(user)
        && let Some(stored) = component.do_afters.get_mut(&index)
    {
        stored.completed = true;
    }
    let repeat = do_after.event.done(world, &do_after.args, false);
    if let Some(delay) = repeat
        && let Some(mut component) = world.get_mut::<DoAfterComponent>(user)
        && let Some(stored) = component.do_afters.get_mut(&index)
    {
        stored.start_time = time;
        stored.completed = false;
        stored.args.delay = delay;
        sync_bars(world, user);
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component_as::<DoAfterComponent>("DoAfter");
    world.register_net_serde::<DoAfterBars>();
    if side.is_server() {
        world.add_game_systems(Update, update);
    }
}

#[cfg(test)]
mod tests;
