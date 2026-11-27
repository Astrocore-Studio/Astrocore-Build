//! Конец раунда: вызов шаттла эвакуации с отсчётом (по умолчанию 10 минут), его отзыв,
//! перерыв между вызовом и отзывом (30 с), запрет отзыва; по окончании отсчёта шаттл
//! прибывает к станции. Автовызов: если шаттл не вызывали `shuttle.auto_call_time` минут —
//! «пересменка», после отзыва снова через `shuttle.auto_call_extension_time`. Конец раунда
//! с объявлением, через сколько перезапуск. Образец — `RoundEndSystem`,
//! `RoundEndSystemChangedEvent`, `CallShuttleCommand`, `RecallShuttleCommand`,
//! `CCVars.Shuttle` SS14 @ 2f4a2aad.
//!
//! Время отсчёта — по умолчанию: уровней тревоги станции (`AlertLevel.ShuttleTime`) пока нет.
//! Экраны с таймером шаттла (`ScreenShuttlePayload`) — позже, вместе с экранами.

use std::time::Duration;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_console::{
    CVarDef, CVarFlags, ConfigWorldExt, ConsoleCommand, ConsoleShell, ConsoleWorldExt,
};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, Update};
use astrocore_loc::LocArg;
use astrocore_net::NetTarget;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::examine::loc;
use crate::game_ticker::{self, GameRunLevel};

pub const EMERGENCY_SHUTTLE_AUTO_CALL_TIME: CVarDef<i32> = CVarDef::new(
    "shuttle.auto_call_time",
    90,
    CVarFlags::SERVERONLY,
    "Через сколько минут шаттл вызывается сам; 0 — никогда.",
);
pub const EMERGENCY_SHUTTLE_AUTO_CALL_EXTENSION_TIME: CVarDef<i32> = CVarDef::new(
    "shuttle.auto_call_extension_time",
    45,
    CVarFlags::SERVERONLY,
    "Через сколько минут после отзыва автовызова шаттл вызывается снова.",
);

/// Отсчёт по умолчанию (`DefaultCountdownDuration`).
pub const DEFAULT_COUNTDOWN: Duration = Duration::from_secs(600);
/// Перерыв между вызовом и отзывом (`DefaultCooldownDuration`).
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);

pub const SHUTTLE_CALLED_TEXT: &str = "round-end-system-shuttle-called-announcement";
pub const SHUTTLE_SENDER: &str = "round-end-system-shuttle-sender-announcement";
const GOLD: &str = "#FFD700";

/// Состояние конца раунда. Ресурс сервера.
#[derive(Resource, Debug, Clone, Default)]
pub struct RoundEnd {
    /// Шаттл вызван (`_countdownTokenSource`): до перезапуска или отзыва.
    pub requested: bool,
    /// Отсчёт уже закончился.
    pub countdown_fired: bool,
    pub last_countdown_start: Option<Duration>,
    pub expected_countdown_end: Option<Duration>,
    /// Перерыв между вызовом и отзывом (`_cooldownTokenSource`).
    pub cooldown_until: Option<Duration>,
    /// Шаттл нельзя отозвать (`CantRecall`).
    pub cant_recall: bool,
    pub auto_call_start: Duration,
    pub auto_called_before: bool,
    /// Шаттл эвакуации уже у станции (`EmergencyShuttleArrived`).
    pub shuttle_arrived: bool,
}

/// Состояние конца раунда изменилось (`RoundEndSystemChangedEvent`). Широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct RoundEndChanged;

/// Отсчёт закончился: шаттл должен прибыть (`DockEmergencyShuttle`). Широковещательно;
/// `handled` — шаттл эвакуации есть, иначе раунд кончается сразу.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShuttleCountdownFinished {
    pub handled: bool,
}

fn now(world: &World) -> Duration {
    world
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default()
}

fn state(world: &mut World) -> bevy_ecs::world::Mut<'_, RoundEnd> {
    world.get_resource_or_insert_with(RoundEnd::default)
}

fn changed(world: &mut World) {
    world.raise_broadcast(&mut RoundEndChanged);
}

/// Можно ли вызвать или отозвать (`CanCallOrRecall`).
pub fn can_call_or_recall(world: &World) -> bool {
    world
        .get_resource::<RoundEnd>()
        .is_none_or(|s| s.cooldown_until.is_none() && !s.cant_recall)
}

/// Шаттл вызван (`IsRoundEndRequested`).
pub fn is_round_end_requested(world: &World) -> bool {
    world
        .get_resource::<RoundEnd>()
        .is_some_and(|s| s.requested)
}

/// Сколько осталось до прибытия (`ShuttleTimeLeft`).
pub fn shuttle_time_left(world: &World) -> Option<Duration> {
    let end = world.get_resource::<RoundEnd>()?.expected_countdown_end?;
    Some(end.saturating_sub(now(world)))
}

/// Длительность отсчёта (`ExpectedShuttleLength`).
pub fn expected_shuttle_length(world: &World) -> Option<Duration> {
    let state = world.get_resource::<RoundEnd>()?;
    Some(
        state
            .expected_countdown_end?
            .saturating_sub(state.last_countdown_start?),
    )
}

/// Время и единицы для объявления: секунды до минуты, дальше минуты.
fn eta(world: &World, duration: Duration) -> (i64, String) {
    let (time, key) = if duration.as_secs() < 60 {
        (duration.as_secs() as i64, "eta-units-seconds")
    } else {
        ((duration.as_secs() / 60) as i64, "eta-units-minutes")
    };
    (time, loc(world, key, &[("amount", LocArg::from(time))]))
}

fn player_name(world: &World, entity: Entity) -> String {
    astrocore_loc::EntityNameWorldExt::entity_name(world, entity)
}

fn log(
    world: &mut World,
    kind: LogType,
    requester: Option<Entity>,
    machine: Option<Entity>,
    what: &str,
) {
    let with = machine
        .map(|m| format!(" with {} ", player_name(world, m)))
        .unwrap_or_default();
    let message = match requester {
        Some(r) => format!("{what} by {}{with}", player_name(world, r)),
        None => format!("{what}{with}"),
    };
    let players: Vec<_> = requester
        .and_then(|r| crate::admin_logs::player_of(world, r))
        .into_iter()
        .collect();
    add_admin_log(world, kind, LogImpact::High, &message, &[], &players);
}

/// Вызвать шаттл (`RequestRoundEnd`): `check_cooldown` — уважать перерыв.
#[allow(clippy::too_many_arguments)]
pub fn request_round_end(
    world: &mut World,
    countdown: Option<Duration>,
    requester: Option<Entity>,
    machine: Option<Entity>,
    check_cooldown: bool,
    text: &str,
    sender: &str,
    cant_recall: bool,
) {
    if game_ticker::run_level(world) != GameRunLevel::InRound {
        return;
    }
    if check_cooldown && state(world).cooldown_until.is_some() {
        return;
    }
    if state(world).requested {
        return;
    }
    let countdown = countdown.unwrap_or(DEFAULT_COUNTDOWN);
    {
        let mut state = state(world);
        state.requested = true;
        state.countdown_fired = false;
        state.cant_recall = cant_recall;
    }
    log(
        world,
        LogType::ShuttleCalled,
        requester,
        machine,
        "Shuttle called",
    );
    let (time, units) = eta(world, countdown);
    let message = loc(
        world,
        text,
        &[("time", LocArg::from(time)), ("units", LocArg::from(units))],
    );
    let sender = loc(world, sender, &[]);
    crate::chat::dispatch_global_announcement(
        world,
        &message,
        Some(&sender),
        false,
        Some(GOLD.into()),
    );
    world.play_global(
        &SoundSpecifier::path("/Audio/Announcements/shuttlecalled.ogg"),
        NetTarget::All,
    );
    let time_now = now(world);
    {
        let mut state = state(world);
        state.last_countdown_start = Some(time_now);
        state.expected_countdown_end = Some(time_now + countdown);
    }
    activate_cooldown(world);
    changed(world);
}

/// Отозвать шаттл (`CancelRoundEndCountdown`); `force` — несмотря на запрет и перерыв.
pub fn cancel_round_end_countdown(
    world: &mut World,
    requester: Option<Entity>,
    machine: Option<Entity>,
    force: bool,
) {
    if game_ticker::run_level(world) != GameRunLevel::InRound {
        return;
    }
    {
        let state = state(world);
        if !force && (state.cant_recall || state.cooldown_until.is_some()) {
            return;
        }
        if !state.requested {
            return;
        }
    }
    state(world).requested = false;
    log(
        world,
        LogType::ShuttleRecalled,
        requester,
        machine,
        "Shuttle recalled",
    );
    let message = loc(world, "round-end-system-shuttle-recalled-announcement", &[]);
    let sender = loc(world, SHUTTLE_SENDER, &[]);
    crate::chat::dispatch_global_announcement(
        world,
        &message,
        Some(&sender),
        false,
        Some(GOLD.into()),
    );
    world.play_global(
        &SoundSpecifier::path("/Audio/Announcements/shuttlerecalled.ogg"),
        NetTarget::All,
    );
    {
        let mut state = state(world);
        state.last_countdown_start = None;
        state.expected_countdown_end = None;
    }
    activate_cooldown(world);
    changed(world);
}

/// Закончить раунд (`EndRound`): итоги и объявление о перезапуске через `countdown` (по
/// умолчанию `game.round_restart_time`).
pub fn end_round(world: &mut World, countdown: Option<Duration>) {
    if game_ticker::run_level(world) != GameRunLevel::InRound {
        return;
    }
    {
        let mut state = state(world);
        state.last_countdown_start = None;
        state.expected_countdown_end = None;
        state.requested = true;
    }
    changed(world);
    game_ticker::end_round(world, "");
    let countdown = countdown.unwrap_or_else(|| {
        Duration::from_secs_f32(world.cvar(&game_ticker::ROUND_RESTART_TIME).max(0.0))
    });
    let (time, units) = eta(world, countdown);
    let message = loc(
        world,
        "round-end-system-round-restart-eta-announcement",
        &[("time", LocArg::from(time)), ("units", LocArg::from(units))],
    );
    crate::chat::dispatch_server_announcement(world, &message);
    let at = now(world) + countdown;
    if let Some(mut ticker) = world.get_resource_mut::<game_ticker::GameTicker>() {
        ticker.restart_at = Some(at);
    }
}

/// Перерыв (`ActivateCooldown`).
fn activate_cooldown(world: &mut World) {
    let until = now(world) + DEFAULT_COOLDOWN;
    state(world).cooldown_until = Some(until);
}

/// Сбросить (`Reset`) при перезапуске раунда.
fn reset(world: &mut World) {
    let time = now(world);
    world.insert_resource(RoundEnd {
        auto_call_start: time,
        ..RoundEnd::default()
    });
    changed(world);
}

/// Такт: отсчёт, перерыв, автовызов (`Update`, таймеры).
pub fn update(world: &mut World) {
    let time = now(world);
    let (fire, cooldown_over) = {
        let state = state(world);
        (
            state.requested
                && !state.countdown_fired
                && state.expected_countdown_end.is_some_and(|end| end <= time),
            state.cooldown_until.is_some_and(|until| until <= time),
        )
    };
    if cooldown_over {
        state(world).cooldown_until = None;
        changed(world);
    }
    if fire {
        state(world).countdown_fired = true;
        let mut event = ShuttleCountdownFinished::default();
        world.raise_broadcast(&mut event);
        // Шаттла эвакуации нет — раунд кончается сразу.
        if !event.handled {
            end_round(world, None);
        }
    }
    if game_ticker::run_level(world) != GameRunLevel::InRound {
        return;
    }
    let minutes = if state(world).auto_called_before {
        world.cvar(&EMERGENCY_SHUTTLE_AUTO_CALL_EXTENSION_TIME)
    } else {
        world.cvar(&EMERGENCY_SHUTTLE_AUTO_CALL_TIME)
    };
    let start = state(world).auto_call_start;
    if minutes != 0 && time.saturating_sub(start) > Duration::from_secs(minutes.max(0) as u64 * 60)
    {
        let arrived = state(world).shuttle_arrived;
        if !arrived && state(world).expected_countdown_end.is_none() {
            request_round_end(
                world,
                None,
                None,
                None,
                false,
                "round-end-system-shuttle-auto-called-announcement",
                SHUTTLE_SENDER,
                false,
            );
            state(world).auto_called_before = true;
        }
        state(world).auto_call_start = time;
    }
}

/// Время `callshuttle`: минуты или минуты:секунды (`TimeSpanMinutesFormats`).
pub fn parse_minutes(text: &str) -> Option<Duration> {
    match text.split_once(':') {
        Some((minutes, seconds)) => {
            let minutes: u64 = minutes.parse().ok()?;
            let seconds: u64 = seconds.parse().ok()?;
            (seconds < 60).then(|| Duration::from_secs(minutes * 60 + seconds))
        }
        None => Some(Duration::from_secs(text.parse::<u64>().ok()? * 60)),
    }
}

fn attached(world: &World, shell: &ConsoleShell) -> Option<Entity> {
    shell
        .player
        .and_then(|s| world.get::<astrocore_net::Session>(s))
        .and_then(|s| s.attached)
}

fn callshuttle(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let requester = attached(world, shell);
    match args {
        [time] => match parse_minutes(time) {
            Some(duration) => request_round_end(
                world,
                Some(duration),
                requester,
                None,
                false,
                SHUTTLE_CALLED_TEXT,
                SHUTTLE_SENDER,
                false,
            ),
            None => shell.write_line(loc(world, "shell-timespan-minutes-must-be-correct", &[])),
        },
        _ => request_round_end(
            world,
            None,
            requester,
            None,
            false,
            SHUTTLE_CALLED_TEXT,
            SHUTTLE_SENDER,
            false,
        ),
    }
}

fn recallshuttle(world: &mut World, shell: &mut ConsoleShell, _: &[String]) {
    let requester = attached(world, shell);
    cancel_round_end_countdown(world, requester, None, true);
}

pub fn install(world: &mut World, side: Side) {
    if !side.is_server() {
        return;
    }
    world.register_cvar(&EMERGENCY_SHUTTLE_AUTO_CALL_TIME);
    world.register_cvar(&EMERGENCY_SHUTTLE_AUTO_CALL_EXTENSION_TIME);
    world.init_resource::<RoundEnd>();
    world.register_command(ConsoleCommand {
        name: "callshuttle",
        description: "Вызывает шаттл эвакуации.",
        help: "callshuttle [минуты или м:сс]",
        execute: callshuttle,
        complete: None,
    });
    world.register_command(ConsoleCommand {
        name: "recallshuttle",
        description: "Отзывает шаттл эвакуации.",
        help: "recallshuttle",
        execute: recallshuttle,
        complete: None,
    });
    world.subscribe_broadcast::<game_ticker::RoundRestartCleanup>(|world, _| reset(world));
    // Автовызов отсчитывается от начала раунда (`SetAutoCallTime` при запуске).
    world.subscribe_broadcast::<game_ticker::RoundStarting>(|world, _| {
        let time = now(world);
        state(world).auto_call_start = time;
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
