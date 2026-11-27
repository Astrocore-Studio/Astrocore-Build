//! Обращения к администраторам (ahelp, «bwoink»): у каждого игрока свой канал с
//! администраторами; игрок пишет только в свой канал, администратор с правом `ADMINHELP` —
//! в любой, в том числе «только администраторам» и без звука; сообщение получают все
//! администраторы с этим правом и сам игрок; если администраторов нет — игрок узнаёт об
//! этом; ограничение частоты; администраторы узнают, что собеседник вышел или вернулся.
//! Образец — `BwoinkSystem`, `SharedBwoinkSystem` (`BwoinkTextMessage`), `FormatName`,
//! `FormatFullMessageForRecipient`, `NotifyAdmins`, `CCVars.Ahelp*`,
//! `AHelpUIController` (звук и непрочитанное) SS14 @ 2f4a2aad.
//!
//! Нет пока: пересылки в Discord, индикатора набора, ссылок на имена, отметки о бане при
//! выходе собеседника.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use astrocore_console::{CVarDef, CVarFlags, Config, ConfigWorldExt};
use astrocore_ecs::{EventWorldExt, GameTime};
use astrocore_loc::escape_text;
use astrocore_net::{
    NetEvent, NetEventWorldExt, NetTarget, NetUserId, PlayerConnected, PlayerDisconnected, Session,
};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_manager::{self, AdminData, flags};
use crate::examine::loc;

pub const AHELP_RATE_LIMIT_PERIOD: CVarDef<f32> = CVarDef::new(
    "ahelp.rate_limit_period",
    2.0,
    CVarFlags::SERVERONLY,
    "Окно ограничения частоты ahelp, секунды.",
);
pub const AHELP_RATE_LIMIT_COUNT: CVarDef<i32> = CVarDef::new(
    "ahelp.rate_limit_count",
    10,
    CVarFlags::SERVERONLY,
    "Сколько сообщений ahelp можно за окно.",
);
pub const AHELP_ADMIN_PREFIX: CVarDef<bool> = CVarDef::new(
    "ahelp.admin_prefix",
    false,
    CVarFlags::SERVERONLY,
    "Показывать звание администратора в ahelp.",
);
pub const AHELP_SOUND: CVarDef<&str> = CVarDef::new(
    "audio.ahelp_sound",
    "/Audio/Effects/adminhelp.ogg",
    CVarFlags::SERVER
        .union(CVarFlags::REPLICATED)
        .union(CVarFlags::ARCHIVE),
    "Звук нового сообщения ahelp.",
);

/// Системный отправитель (`SystemUserId`).
pub const SYSTEM_USER: NetUserId = NetUserId(uuid::Uuid::nil());

/// Сообщение ahelp (`BwoinkTextMessage`). `user_id` — канал (игрок), `true_sender` —
/// кто на самом деле написал; от клиента сервер берёт только канал, текст и пометки.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BwoinkTextMessage {
    pub user_id: NetUserId,
    pub true_sender: NetUserId,
    pub text: String,
    pub play_sound: bool,
    pub admin_only: bool,
    /// Имя игрока канала: по нему администратор видит, с кем переписка.
    pub channel_name: String,
}

/// Беседы и частота. Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct Bwoink {
    /// Когда в канале писали последний раз (`_activeConversations`).
    pub active_conversations: BTreeMap<NetUserId, Duration>,
    rate: BTreeMap<NetUserId, VecDeque<Duration>>,
}

/// Переписка на клиенте. Ресурс мира клиента.
#[derive(Resource, Debug, Clone, Default)]
pub struct ClientBwoink {
    /// Канал → (имя, строки).
    pub channels: BTreeMap<NetUserId, (String, Vec<String>)>,
    pub unread: bool,
    pub version: u64,
}

fn now(world: &World) -> Duration {
    world
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default()
}

/// Администраторы, получающие ahelp (`GetTargetAdmins`).
pub fn target_admins(world: &World) -> Vec<Entity> {
    admin_manager::active_admins(world)
        .into_iter()
        .filter(|s| admin_manager::has_flag(world, *s, flags::ADMINHELP))
        .collect()
}

/// Имя отправителя (`FormatName`): помощник — фиолетовым, администратор — красным.
pub fn format_name(admin: Option<&AdminData>, prefix: &str, name: &str) -> String {
    match admin {
        Some(data) if data.flags == flags::ADMINHELP => {
            format!("[color=purple]{prefix}{name}[/color]")
        }
        Some(data) if data.has_flag(flags::ADMINHELP) => {
            format!("[color=red]{prefix}{name}[/color]")
        }
        _ => name.to_owned(),
    }
}

/// Строка для получателя (`FormatFullMessageForRecipient`).
pub fn format_message(
    world: &World,
    admin: Option<&AdminData>,
    sender_name: &str,
    text: &str,
    play_sound: bool,
    admin_only: bool,
) -> String {
    let prefix = match admin.and_then(|a| a.title.as_ref()) {
        Some(title) if world.resource::<Config>().get(&AHELP_ADMIN_PREFIX) => {
            format!("[bold]\\[{}\\][/bold] ", escape_text(title))
        }
        _ => String::new(),
    };
    let name = format_name(admin, &prefix, sender_name);
    let status = if admin_only {
        loc(world, "bwoink-message-admin-only", &[])
    } else if !play_sound {
        loc(world, "bwoink-message-silent", &[])
    } else {
        String::new()
    };
    format!("{status} {name}: {}", escape_text(text))
}

fn rate_allows(world: &mut World, user: NetUserId) -> bool {
    let period = Duration::from_secs_f32(world.cvar(&AHELP_RATE_LIMIT_PERIOD).max(0.0));
    let count = usize::try_from(world.cvar(&AHELP_RATE_LIMIT_COUNT)).unwrap_or(0);
    let time = now(world);
    let mut state = world.get_resource_or_insert_with(Bwoink::default);
    let queue = state.rate.entry(user).or_default();
    while queue
        .front()
        .is_some_and(|t| time.saturating_sub(*t) >= period)
    {
        queue.pop_front();
    }
    if queue.len() >= count {
        return false;
    }
    queue.push_back(time);
    true
}

fn session_name(world: &World, session: Entity) -> String {
    world
        .get::<Session>(session)
        .map(|s| s.name.clone())
        .unwrap_or_default()
}

/// Сообщение от игрока или администратора (`OnBwoinkTextMessage`).
pub fn receive_message(world: &mut World, sender: Entity, message: &BwoinkTextMessage) {
    let Some(sender_user) = world.get::<Session>(sender).map(|s| s.user_id) else {
        return;
    };
    let channel = message.user_id;
    let personal = sender_user == channel;
    let admin = admin_manager::admin_data(world, sender, false);
    let ahelp_admin = admin.as_ref().is_some_and(|a| a.has_flag(flags::ADMINHELP));
    if !(personal && !message.admin_only || ahelp_admin) {
        return;
    }
    if !rate_allows(world, sender_user) {
        let text = loc(world, "bwoink-system-rate-limited", &[]);
        let reply = BwoinkTextMessage {
            user_id: channel,
            true_sender: SYSTEM_USER,
            text,
            play_sound: false,
            admin_only: false,
            channel_name: String::new(),
        };
        world.send_net_event(NetTarget::Session(sender), &reply);
        return;
    }
    let time = now(world);
    world
        .get_resource_or_insert_with(Bwoink::default)
        .active_conversations
        .insert(channel, time);
    let play_sound = (!ahelp_admin || message.play_sound) && !message.admin_only;
    let channel_session = crate::mind::session_of_user(world, channel);
    let channel_name = channel_session
        .map(|s| session_name(world, s))
        .unwrap_or_else(|| message.channel_name.clone());
    let sender_name = session_name(world, sender);
    let text = format_message(
        world,
        admin.as_ref(),
        &sender_name,
        &message.text,
        play_sound,
        message.admin_only,
    );
    let outgoing = BwoinkTextMessage {
        user_id: channel,
        true_sender: sender_user,
        text,
        play_sound,
        admin_only: message.admin_only,
        channel_name,
    };
    let admins = target_admins(world);
    for admin in &admins {
        world.send_net_event(NetTarget::Session(*admin), &outgoing);
    }
    if let Some(player) = channel_session
        && !message.admin_only
        && !admins.contains(&player)
    {
        world.send_net_event(NetTarget::Session(player), &outgoing);
    }
    let users: Vec<NetUserId> = vec![channel];
    crate::admin_logs::add_admin_log(
        world,
        crate::admin_logs::LogType::AdminMessage,
        crate::admin_logs::LogImpact::Low,
        &format!(
            "Ahelp {sender_name} -> {}: {}",
            outgoing.channel_name, message.text
        ),
        &[],
        &users,
    );
    if admins.is_empty() {
        let text = loc(world, "bwoink-system-starmute-message-no-other-users", &[]);
        let reply = BwoinkTextMessage {
            user_id: channel,
            true_sender: SYSTEM_USER,
            text,
            play_sound: true,
            admin_only: false,
            channel_name: String::new(),
        };
        world.send_net_event(NetTarget::Session(sender), &reply);
    }
}

/// Собеседник вышел или вернулся (`NotifyAdmins`): только для недавних бесед (5 минут).
fn notify_admins(world: &mut World, session: Entity, key: &str, color: &str) {
    let Some(user) = world.get::<Session>(session).map(|s| s.user_id) else {
        return;
    };
    let last = world
        .get_resource::<Bwoink>()
        .and_then(|b| b.active_conversations.get(&user).copied());
    let Some(last) = last else {
        return;
    };
    if now(world).saturating_sub(last) > Duration::from_secs(300) {
        world
            .resource_mut::<Bwoink>()
            .active_conversations
            .remove(&user);
        return;
    }
    let name = session_name(world, session);
    let text = format!("[color={color}]{name} {}[/color]", loc(world, key, &[]));
    let message = BwoinkTextMessage {
        user_id: user,
        true_sender: SYSTEM_USER,
        text,
        play_sound: false,
        admin_only: false,
        channel_name: name,
    };
    for admin in target_admins(world) {
        world.send_net_event(NetTarget::Session(admin), &message);
    }
}

/// Сообщение пришло клиенту (`ReceivedBwoink`): в канал, звук от чужого.
pub fn client_receive(world: &mut World, message: &BwoinkTextMessage) {
    let local_user = world
        .get_resource::<crate::admin_manager::ClientAdmin>()
        .and_then(|a| a.user_id);
    let mut state = world.get_resource_or_insert_with(ClientBwoink::default);
    let entry = state
        .channels
        .entry(message.user_id)
        .or_insert_with(|| (message.channel_name.clone(), Vec::new()));
    if entry.0.is_empty() {
        entry.0.clone_from(&message.channel_name);
    }
    entry.1.push(message.text.clone());
    state.version += 1;
    let from_other = local_user != Some(message.true_sender);
    if from_other {
        state.unread = true;
    }
    if message.play_sound && from_other {
        let path = world
            .get_resource::<Config>()
            .map(|c| c.get_text(&AHELP_SOUND))
            .unwrap_or_default();
        if !path.is_empty() {
            astrocore_audio::AudioWorldExt::play_local(
                world,
                &astrocore_audio::SoundSpecifier::path(&path),
                astrocore_audio::SoundSource::Global,
            );
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_cvar(&AHELP_SOUND);
    world.register_net_event::<BwoinkTextMessage>();
    if !side.is_server() {
        world.init_resource::<ClientBwoink>();
        world.subscribe_net_event::<BwoinkTextMessage>(|world, event| {
            let message = event.event.clone();
            client_receive(world, &message);
        });
        return;
    }
    world.register_cvar(&AHELP_RATE_LIMIT_PERIOD);
    world.register_cvar(&AHELP_RATE_LIMIT_COUNT);
    world.register_cvar(&AHELP_ADMIN_PREFIX);
    world.init_resource::<Bwoink>();
    world.subscribe_net_event::<BwoinkTextMessage>(|world, event: &mut NetEvent<_>| {
        let Some(sender) = event.session else {
            return;
        };
        let message = event.event.clone();
        receive_message(world, sender, &message);
    });
    world.subscribe_broadcast::<PlayerConnected>(|world, event| {
        let session = event.session;
        notify_admins(
            world,
            session,
            "bwoink-system-player-reconnecting",
            "#00FF00",
        );
    });
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        let session = event.session;
        notify_admins(
            world,
            session,
            "bwoink-system-player-disconnecting",
            "#FFFF00",
        );
    });
    world.subscribe_broadcast::<crate::game_ticker::RoundRestartCleanup>(|world, _| {
        world.resource_mut::<Bwoink>().active_conversations.clear();
    });
}

#[cfg(test)]
mod tests;
