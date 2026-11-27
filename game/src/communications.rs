//! Консоль связи: объявление по радио станции (или всем — у консоли `global`) с подписью
//! «имя (должность)» по ID-карте, перерыв между объявлениями (90 с, после появления — 30 с);
//! вызов и отзыв шаттла эвакуации — не после прибытия, не в перерыв конца раунда и не позже
//! точки невозврата (`shuttle.recall_turning_point` от отсчёта); доступ по считывателю
//! консоли; окно обновляется при изменении конца раунда и раз в 5 с. Образец —
//! `CommunicationsConsoleSystem`, `CommunicationsConsoleComponent`,
//! `SharedCommunicationsConsoleComponent`, `CommunicationConsoleAnnouncementEvent`,
//! `CommunicationConsoleCallShuttleAttemptEvent`, `CCVars.EmergencyRecallTurningPoint` SS14
//! @ 2f4a2aad.
//!
//! Нет пока: уровней тревоги в окне (уровней тревоги нет) и вывода текста на экраны станции
//! (`ScreenTextPayload`, экранов нет).

use std::time::Duration;

use astrocore_audio::SoundSpecifier;
use astrocore_console::{CVarDef, CVarFlags, ConfigWorldExt};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, Update};
use astrocore_loc::EntityNameWorldExt;
use astrocore_ui::{BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::examine::loc;
use crate::popups::{PopupType, PopupWorldExt};
use crate::{access, chat, emergency_shuttle, round_end, stations};

pub const COMMS_UI_KEY: &str = "enum.CommunicationsConsoleUiKey.Key";

pub const EMERGENCY_RECALL_TURNING_POINT: CVarDef<f32> = CVarDef::new(
    "shuttle.recall_turning_point",
    0.5,
    CVarFlags::SERVERONLY,
    "Доля отсчёта, после которой шаттл нельзя отозвать с консоли.",
);

/// Как часто обновляется открытое окно, секунды (`UIUpdateInterval`).
const UI_UPDATE_INTERVAL: f32 = 5.0;

fn default_title() -> String {
    "comms-console-announcement-title-station".into()
}

fn default_color() -> String {
    "#FFD700".into()
}

fn default_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Announcements/announce.ogg")
}

/// Консоль связи (`CommunicationsConsoleComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CommunicationsConsole {
    #[serde(skip)]
    pub ui_update_accumulator: f32,
    /// Сколько секунд до следующего объявления.
    pub announcement_cooldown_remaining: f32,
    pub broadcast_cooldown_remaining: f32,
    /// Ключ перевода или текст отправителя объявления.
    pub title: String,
    pub color: String,
    /// Перерыв между объявлениями, секунды.
    pub delay: i32,
    /// Перерыв после появления консоли, секунды.
    pub initial_delay: i32,
    /// Может вызывать и отзывать шаттл.
    pub can_shuttle: bool,
    /// Объявления всем, а не станции.
    pub global: bool,
    pub sound: SoundSpecifier,
    /// Подписывать объявление именем отправителя.
    pub announce_sent_by: bool,
}

impl Default for CommunicationsConsole {
    fn default() -> Self {
        Self {
            ui_update_accumulator: 0.0,
            announcement_cooldown_remaining: 0.0,
            broadcast_cooldown_remaining: 0.0,
            title: default_title(),
            color: default_color(),
            delay: 90,
            initial_delay: 30,
            can_shuttle: true,
            global: false,
            sound: default_sound(),
            announce_sent_by: true,
        }
    }
}

/// Состояние окна (`CommunicationsConsoleInterfaceState`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CommunicationsConsoleState {
    pub can_announce: bool,
    pub can_broadcast: bool,
    pub can_call: bool,
    /// Когда прибудет шаттл, по часам игры.
    pub expected_countdown_end: Option<Duration>,
    pub countdown_started: bool,
}

/// Действия в окне (`CommunicationsConsoleAnnounceMessage`,
/// `CommunicationsConsoleCallEmergencyShuttleMessage`,
/// `CommunicationsConsoleRecallEmergencyShuttleMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommunicationsConsoleMessage {
    Announce(String),
    CallEmergencyShuttle,
    RecallEmergencyShuttle,
}

/// Объявление с консоли сделано (`CommunicationConsoleAnnouncementEvent`). Широковещательно.
#[derive(Debug, Clone)]
pub struct CommunicationConsoleAnnouncement {
    pub console: Entity,
    pub text: String,
    pub sender: Option<Entity>,
}

/// Попытка вызвать шаттл (`CommunicationConsoleCallShuttleAttemptEvent`): можно отменить с
/// причиной. Широковещательно.
#[derive(Debug, Clone)]
pub struct CommunicationConsoleCallShuttleAttempt {
    pub console: Entity,
    pub sender: Option<Entity>,
    pub cancelled: bool,
    pub reason: Option<String>,
}

fn consoles(world: &mut World) -> Vec<Entity> {
    world
        .query_filtered::<Entity, bevy_ecs::query::With<CommunicationsConsole>>()
        .iter(world)
        .collect()
}

/// Можно ли делать объявление (`CanAnnounce`): консоль станции или всеобщая, перерыв прошёл.
pub fn can_announce(world: &World, console: Entity) -> bool {
    let Some(comp) = world.get::<CommunicationsConsole>(console) else {
        return false;
    };
    (stations::owning_station(world, console).is_some() || comp.global)
        && comp.announcement_cooldown_remaining <= 0.0
}

/// Пускает ли консоль (`CanUse`).
fn can_use(world: &World, user: Entity, console: Entity) -> bool {
    access::is_allowed(world, user, console)
}

/// Можно ли вызвать или отозвать шаттл с консоли (`CanCallOrRecall`).
pub fn can_call_or_recall(world: &World, console: Entity) -> bool {
    if emergency_shuttle::emergency_shuttle_arrived(world) || !round_end::can_call_or_recall(world)
    {
        return false;
    }
    if !world
        .get::<CommunicationsConsole>(console)
        .is_some_and(|c| c.can_shuttle)
    {
        return false;
    }
    let expected_end = world
        .get_resource::<round_end::RoundEnd>()
        .and_then(|s| s.expected_countdown_end);
    if expected_end.is_none() {
        return true;
    }
    // Отзыв: не позже точки невозврата.
    let threshold = world.cvar(&EMERGENCY_RECALL_TURNING_POINT);
    let (Some(left), Some(expected)) = (
        round_end::shuttle_time_left(world),
        round_end::expected_shuttle_length(world),
    ) else {
        return false;
    };
    left.as_secs_f64() / expected.as_secs_f64() >= f64::from(threshold)
}

/// Обновить окно консоли (`UpdateCommsConsoleInterface`).
pub fn update_interface(world: &mut World, console: Entity) {
    let expected_countdown_end = world
        .get_resource::<round_end::RoundEnd>()
        .and_then(|s| s.expected_countdown_end);
    let state = CommunicationsConsoleState {
        can_announce: can_announce(world, console),
        can_broadcast: true,
        can_call: can_call_or_recall(world, console),
        expected_countdown_end,
        countdown_started: expected_countdown_end.is_some(),
    };
    world.set_ui_state(console, COMMS_UI_KEY, &state);
}

/// Обновить окна всех консолей.
pub fn update_all_interfaces(world: &mut World) {
    for console in consoles(world) {
        update_interface(world, console);
    }
}

fn deny(world: &mut World, console: Entity, user: Entity) {
    let text = loc(world, "comms-console-permission-denied", &[]);
    world.popup_entity_for(&text, console, user, PopupType::Small);
}

/// Объявление (`OnAnnounceMessage`).
fn announce(world: &mut World, console: Entity, actor: Option<Entity>, message: &str) {
    let max = world.cvar(&chat::CHAT_MAX_ANNOUNCEMENT_LENGTH).max(0) as usize;
    let text = chat::sanitize_announcement(message, max, 2);
    let mut author = loc(world, "comms-console-announcement-unknown-sender", &[]);
    if let Some(user) = actor {
        if !can_announce(world, console) {
            return;
        }
        if !can_use(world, user, console) {
            deny(world, console, user);
            return;
        }
        if let Some(info) = access::identity_short_info(world, user) {
            author = info;
        }
    }
    let Some(comp) = world.get::<CommunicationsConsole>(console).cloned() else {
        return;
    };
    if let Some(mut stored) = world.get_mut::<CommunicationsConsole>(console) {
        stored.announcement_cooldown_remaining = comp.delay as f32;
    }
    update_interface(world, console);
    world.raise_broadcast(&mut CommunicationConsoleAnnouncement {
        console,
        text: text.clone(),
        sender: actor,
    });
    let title = loc(world, &comp.title, &[]);
    let signature = comp.announce_sent_by.then_some(author.as_str());
    let options = chat::Announcement {
        sender: Some(&title),
        play_sound: true,
        sound: comp.global.then(|| comp.sound.clone()),
        color: Some(comp.color.clone()),
        signature,
    };
    if comp.global {
        chat::announce_global(world, &text, &options);
    } else {
        chat::announce_station(world, console, &text, &options);
    }
    let player = actor.map(|a| world.entity_name(a)).unwrap_or_default();
    let scope = if comp.global { "global" } else { "station" };
    let log = match signature {
        Some(signature) => {
            format!("{player} has sent the following {scope} announcement as {signature}: {text}")
        }
        None => format!("{player} has sent the following {scope} announcement: {text}"),
    };
    add_admin_log(world, LogType::Chat, LogImpact::Low, &log, &[], &[]);
}

/// Вызов шаттла (`OnCallShuttleMessage`).
fn call_shuttle(world: &mut World, console: Entity, user: Entity) {
    if !can_call_or_recall(world, console) {
        return;
    }
    if !can_use(world, user, console) {
        deny(world, console, user);
        return;
    }
    let mut attempt = CommunicationConsoleCallShuttleAttempt {
        console,
        sender: Some(user),
        cancelled: false,
        reason: None,
    };
    world.raise_broadcast(&mut attempt);
    if attempt.cancelled {
        let text = attempt
            .reason
            .unwrap_or_else(|| loc(world, "comms-console-shuttle-unavailable", &[]));
        world.popup_entity_for(&text, console, user, PopupType::Small);
        return;
    }
    round_end::request_round_end(
        world,
        None,
        Some(user),
        Some(console),
        true,
        round_end::SHUTTLE_CALLED_TEXT,
        round_end::SHUTTLE_SENDER,
        false,
    );
    let name = world.entity_name(user);
    add_admin_log(
        world,
        LogType::Action,
        LogImpact::High,
        &format!("{name} has called the shuttle."),
        &[],
        &[],
    );
}

/// Отзыв шаттла (`OnRecallShuttleMessage`).
fn recall_shuttle(world: &mut World, console: Entity, user: Entity) {
    if !can_call_or_recall(world, console) {
        return;
    }
    if !can_use(world, user, console) {
        deny(world, console, user);
        return;
    }
    round_end::cancel_round_end_countdown(world, Some(user), Some(console), false);
    let name = world.entity_name(user);
    add_admin_log(
        world,
        LogType::Action,
        LogImpact::High,
        &format!("{name} has recalled the shuttle."),
        &[],
        &[],
    );
}

/// Такт: перерыв объявлений и обновление окна раз в 5 с (`Update`).
fn update(world: &mut World) {
    let frame_time = world
        .get_resource::<GameTime>()
        .map_or(0.0, GameTime::frame_time);
    let mut refresh = Vec::new();
    for (entity, mut comp) in world
        .query::<(Entity, &mut CommunicationsConsole)>()
        .iter_mut(world)
    {
        if comp.announcement_cooldown_remaining >= 0.0 {
            comp.announcement_cooldown_remaining -= frame_time;
        }
        comp.ui_update_accumulator += frame_time;
        if comp.ui_update_accumulator < UI_UPDATE_INTERVAL {
            continue;
        }
        comp.ui_update_accumulator -= UI_UPDATE_INTERVAL;
        refresh.push(entity);
    }
    for console in refresh {
        let open = world
            .get::<astrocore_ui::UserInterface>(console)
            .is_some_and(|ui| !ui.actors(COMMS_UI_KEY).is_empty());
        if open {
            update_interface(world, console);
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    use astrocore_ecs::RegistryWorldExt;
    world.register_data_component::<CommunicationsConsole>();
    if !side.is_server() {
        return;
    }
    world.register_cvar(&EMERGENCY_RECALL_TURNING_POINT);
    world.register_ui_message::<CommunicationsConsoleMessage>();
    world.subscribe::<CommunicationsConsole, MapInit>(|world, console, _| {
        if let Some(mut comp) = world.get_mut::<CommunicationsConsole>(console) {
            comp.announcement_cooldown_remaining = comp.initial_delay as f32;
        }
        update_interface(world, console);
    });
    world
        .subscribe_broadcast::<round_end::RoundEndChanged>(|world, _| update_all_interfaces(world));
    world.subscribe::<CommunicationsConsole, BoundUiMessage<CommunicationsConsoleMessage>>(
        |world, console, event| match &event.message {
            CommunicationsConsoleMessage::Announce(text) => {
                announce(world, console, Some(event.actor), text);
            }
            CommunicationsConsoleMessage::CallEmergencyShuttle => {
                call_shuttle(world, console, event.actor);
            }
            CommunicationsConsoleMessage::RecallEmergencyShuttle => {
                recall_shuttle(world, console, event.actor);
            }
        },
    );
    world.add_game_systems(Update, update);
}

#[cfg(test)]
pub(crate) mod tests;
