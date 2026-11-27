//! Голосования: у голосования заголовок, кто вызвал, варианты и срок; игроки голосуют и
//! меняют голос, по истечении срока побеждает вариант с наибольшим числом голосов (ничья —
//! все лидеры). Стандартные голосования — перезапуск раунда, следующий режим, следующая
//! карта; администраторы вызывают и свои (`customvote`) и отменяют любые. Игрок не может
//! вызвать голосование, пока идёт другое, пока не прошёл его личный перерыв и перерыв этого
//! вида; администратор — всегда. Клиенту рассылаются голосования и то, может ли он вызвать.
//! Образец — `VoteManager` (с `.DefaultVotes`), `VoteOptions`, `VoteCommands`, `MsgVoteData`,
//! `MsgVoteCanCall`, `MsgVoteMenu`, `StandardVoteType`, `CCVars.Vote` SS14 @ 2f4a2aad.
//!
//! Время — настоящее (`RealTime`). Нет голосования за кик (`Votekick`: нужны учёт наигранного
//! времени и антагонисты), веб-хуков Discord, записи своих голосований в базу.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use astrocore_console::{
    CVarChanged, CVarDef, CVarFlags, ConfigWorldExt, ConsoleCommand, ConsoleShell, ConsoleWorldExt,
};
use astrocore_ecs::{EventWorldExt, GameWorldExt, Update};
use astrocore_loc::LocArg;
use astrocore_net::{
    NetEventWorldExt, NetTarget, NetUserId, PlayerConnected, PlayerDisconnected, Session,
};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::admin_manager::{self, AdminPermsChanged, flags};
use crate::examine::loc;
use crate::game_ticker::{self, GameRunLevel};

macro_rules! cvar_bool {
    ($name:ident, $key:literal, $default:expr, $doc:literal) => {
        pub const $name: CVarDef<bool> = CVarDef::new($key, $default, CVarFlags::SERVERONLY, $doc);
    };
}
macro_rules! cvar_i32 {
    ($name:ident, $key:literal, $default:expr, $doc:literal) => {
        pub const $name: CVarDef<i32> = CVarDef::new($key, $default, CVarFlags::SERVERONLY, $doc);
    };
}

cvar_bool!(
    VOTE_ENABLED,
    "vote.enabled",
    true,
    "Можно ли игрокам вызывать голосования."
);
cvar_bool!(
    VOTE_RESTART_ENABLED,
    "vote.restart_enabled",
    true,
    "Голосование за перезапуск раунда."
);
cvar_i32!(
    VOTE_RESTART_MAX_PLAYERS,
    "vote.restart_max_players",
    20,
    "До скольки игроков перезапуск можно вызвать без доли призраков."
);
cvar_i32!(
    VOTE_RESTART_GHOST_PERCENTAGE,
    "vote.restart_ghost_percentage",
    55,
    "Доля призраков, %, нужная для перезапуска на большом сервере."
);
cvar_bool!(
    VOTE_PRESET_ENABLED,
    "vote.preset_enabled",
    true,
    "Голосование за режим."
);
cvar_bool!(
    VOTE_MAP_ENABLED,
    "vote.map_enabled",
    false,
    "Голосование за карту."
);
pub const VOTE_RESTART_REQUIRED_RATIO: CVarDef<f32> = CVarDef::new(
    "vote.restart_required_ratio",
    0.85,
    CVarFlags::SERVERONLY,
    "Доля «за» среди «за» и «против», нужная для перезапуска.",
);
cvar_bool!(
    VOTE_RESTART_NOT_ALLOWED_WHEN_ADMIN_ONLINE,
    "vote.restart_not_allowed_when_admin_online",
    true,
    "Перезапуск не проходит, пока есть администратор."
);
pub const VOTE_SAME_TYPE_TIMEOUT: CVarDef<f32> = CVarDef::new(
    "vote.same_type_timeout",
    240.0,
    CVarFlags::SERVERONLY,
    "Перерыв между голосованиями одного вида, секунды.",
);
cvar_i32!(
    VOTE_TIMER_MAP,
    "vote.timermap",
    90,
    "Длительность голосования за карту, с."
);
cvar_i32!(
    VOTE_TIMER_RESTART,
    "vote.timerrestart",
    60,
    "Длительность голосования за перезапуск, с."
);
cvar_i32!(
    VOTE_TIMER_PRESET,
    "vote.timerpreset",
    30,
    "Длительность голосования за режим, с."
);
cvar_i32!(
    VOTE_TIMER_ALONE,
    "vote.timeralone",
    10,
    "Длительность голосования, когда игрок один, с."
);

/// Стандартное голосование (`StandardVoteType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum StandardVoteType {
    Restart,
    Preset,
    Map,
}

impl StandardVoteType {
    pub const ALL: [StandardVoteType; 3] = [Self::Restart, Self::Preset, Self::Map];

    /// Имя без учёта регистра (`Enum.TryParse(ignoreCase)`).
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|t| format!("{t:?}").eq_ignore_ascii_case(text))
    }

    fn enabled(self) -> CVarDef<bool> {
        match self {
            Self::Restart => VOTE_RESTART_ENABLED,
            Self::Preset => VOTE_PRESET_ENABLED,
            Self::Map => VOTE_MAP_ENABLED,
        }
    }
}

/// Кто может голосовать (`VoterEligibility`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoterEligibility {
    All,
    /// Только призраки.
    Ghost,
}

/// Итог (`VoteFinishedEventArgs`): победитель, если один, лидеры и голоса по вариантам.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteOutcome {
    pub winner: Option<String>,
    pub winners: Vec<String>,
    pub votes: Vec<usize>,
}

pub type VoteFinished = Box<dyn FnOnce(&mut World, &VoteOutcome) + Send + Sync>;

/// Настройки голосования (`VoteOptions`): варианты — (текст, данные).
pub struct VoteOptions {
    pub title: String,
    pub initiator_text: String,
    pub initiator: Option<Entity>,
    pub duration: Duration,
    /// Перерыв вызвавшему; по умолчанию — двойная длительность.
    pub initiator_timeout: Option<Duration>,
    pub options: Vec<(String, String)>,
    pub eligibility: VoterEligibility,
    pub display_votes: bool,
}

impl Default for VoteOptions {
    fn default() -> Self {
        Self {
            title: String::new(),
            initiator_text: String::new(),
            initiator: None,
            duration: Duration::from_secs(60),
            initiator_timeout: None,
            options: Vec::new(),
            eligibility: VoterEligibility::All,
            display_votes: true,
        }
    }
}

struct VoteEntry {
    data: String,
    text: String,
    votes: usize,
}

struct VoteReg {
    title: String,
    initiator_text: String,
    start: Instant,
    end: Instant,
    entries: Vec<VoteEntry>,
    cast: BTreeMap<Entity, usize>,
    votes_dirty: BTreeSet<Entity>,
    eligibility: VoterEligibility,
    display_votes: bool,
    finished: bool,
    cancelled: bool,
    dirty: bool,
    on_finished: Vec<VoteFinished>,
}

/// Голосования сервера. Ресурс сервера.
#[derive(Resource, Default)]
pub struct Votes {
    next_id: i32,
    votes: BTreeMap<i32, VoteReg>,
    standard_timeout: BTreeMap<StandardVoteType, Instant>,
    vote_timeout: BTreeMap<NetUserId, Instant>,
    can_call_dirty: BTreeSet<Entity>,
}

/// Голосование для клиента (`MsgVoteData`). Время — от начала и полная длительность, с.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoteData {
    pub vote_id: i32,
    pub active: bool,
    pub title: String,
    pub initiator: String,
    pub elapsed: f32,
    pub duration: f32,
    /// (голоса, текст); голоса нулевые, если их не показывают.
    pub options: Vec<(u16, String)>,
    /// Свой голос — только если он изменился (`IsYourVoteDirty`).
    pub your_vote: Option<Option<u8>>,
    pub display_votes: bool,
}

/// Может ли игрок вызвать голосование (`MsgVoteCanCall`): через сколько секунд и какие
/// виды недоступны (на сколько секунд; 0 — выключены).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoteCanCall {
    pub can_call: bool,
    pub when_can_call: f32,
    pub unavailable: Vec<(StandardVoteType, f32)>,
}

/// Игрок открыл меню голосований (`MsgVoteMenu`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoteMenuOpened;

/// Голосование у клиента (`ActiveVote`): время — по часам клиента.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientVote {
    pub title: String,
    pub initiator: String,
    pub start: Instant,
    pub end: Instant,
    /// (текст, голоса).
    pub entries: Vec<(String, u16)>,
    pub our_vote: Option<u8>,
    pub display_votes: bool,
}

/// Голосования клиента (`VoteManager` клиента). Ресурс мира клиента.
#[derive(Resource, Debug, Clone, Default)]
pub struct ClientVotes {
    pub votes: BTreeMap<i32, ClientVote>,
    pub can_call: bool,
    /// Недоступные виды: до какого времени; `None` — выключены.
    pub timeouts: BTreeMap<StandardVoteType, Option<Instant>>,
    pub version: u64,
}

impl ClientVotes {
    /// Можно ли вызвать вид (`CanCallStandardVote`): ждать ли и сколько.
    pub fn can_call_standard(&self, kind: StandardVoteType) -> Result<(), Option<Duration>> {
        match self.timeouts.get(&kind) {
            None => Ok(()),
            Some(None) => Err(None),
            Some(Some(until)) => Err(Some(until.saturating_duration_since(Instant::now()))),
        }
    }
}

/// Пришло голосование (`ReceiveVoteData`): новое, изменённое или законченное. `true` —
/// голосование новое (звук).
pub fn client_receive_vote(world: &mut World, data: &VoteData) -> bool {
    let mut client = world.get_resource_or_insert_with(ClientVotes::default);
    client.version += 1;
    if !data.active {
        client.votes.remove(&data.vote_id);
        return false;
    }
    let now = Instant::now();
    let end = now + Duration::from_secs_f32((data.duration - data.elapsed).max(0.0));
    let start = end
        .checked_sub(Duration::from_secs_f32(data.duration.max(0.0)))
        .unwrap_or(now);
    let new = !client.votes.contains_key(&data.vote_id);
    let vote = client
        .votes
        .entry(data.vote_id)
        .or_insert_with(|| ClientVote {
            title: String::new(),
            initiator: String::new(),
            start,
            end,
            entries: Vec::new(),
            our_vote: None,
            display_votes: false,
        });
    vote.title.clone_from(&data.title);
    vote.initiator.clone_from(&data.initiator);
    vote.start = start;
    vote.end = end;
    vote.display_votes = data.display_votes;
    vote.entries = data.options.iter().map(|(v, t)| (t.clone(), *v)).collect();
    if let Some(ours) = data.your_vote {
        vote.our_vote = ours;
    }
    new
}

/// Право вызова (`ReceiveVoteCanCall`).
pub fn client_receive_can_call(world: &mut World, data: &VoteCanCall) {
    let now = Instant::now();
    let mut client = world.get_resource_or_insert_with(ClientVotes::default);
    client.can_call = data.can_call;
    client.timeouts = data
        .unavailable
        .iter()
        .map(|(kind, wait)| {
            let until = (*wait > 0.0).then(|| now + Duration::from_secs_f32(*wait));
            (*kind, until)
        })
        .collect();
    client.version += 1;
}

/// Голос с клиента (`SendCastVote`): сразу у себя и командой `vote` на сервер.
pub fn send_cast_vote(world: &mut World, id: i32, option: u8) {
    if let Some(mut client) = world.get_resource_mut::<ClientVotes>()
        && let Some(vote) = client.votes.get_mut(&id)
    {
        vote.our_vote = Some(option);
        client.version += 1;
    }
    astrocore_net::RemoteConsoleWorldExt::execute_remote_command(
        world,
        &format!("vote {id} {option}"),
    );
}

fn votes(world: &mut World) -> bevy_ecs::world::Mut<'_, Votes> {
    world.get_resource_or_insert_with(Votes::default)
}

fn sessions(world: &mut World) -> Vec<Entity> {
    let mut list: Vec<Entity> = world
        .query_filtered::<Entity, bevy_ecs::query::With<Session>>()
        .iter(world)
        .collect();
    list.sort();
    list
}

fn session_name(world: &World, session: Entity) -> String {
    world
        .get::<Session>(session)
        .map(|s| s.name.clone())
        .unwrap_or_default()
}

fn log(world: &mut World, impact: LogImpact, message: String) {
    add_admin_log(world, LogType::Vote, impact, &message, &[], &[]);
}

fn announce(world: &mut World, key: &str, args: &[(&str, LocArg)]) {
    let text = loc(world, key, args);
    crate::chat::dispatch_server_announcement(world, &text);
}

fn dirty_can_call_all(world: &mut World) {
    let all = sessions(world);
    votes(world).can_call_dirty.extend(all);
}

/// Может ли голосовать (`CheckVoterEligibility`).
pub fn check_voter_eligibility(
    world: &World,
    session: Entity,
    eligibility: VoterEligibility,
) -> bool {
    match eligibility {
        VoterEligibility::All => true,
        VoterEligibility::Ghost => world
            .get::<Session>(session)
            .and_then(|s| s.attached)
            .is_some_and(|body| world.get::<crate::ghost::Ghost>(body).is_some()),
    }
}

/// Доля подходящих игроков, округлённая до процента (`CalculateEligibleVoterPercentage`).
pub fn eligible_voter_percentage(world: &mut World, eligibility: VoterEligibility) -> i32 {
    let all = sessions(world);
    if all.is_empty() {
        return 0;
    }
    let eligible = all
        .iter()
        .filter(|s| check_voter_eligibility(world, **s, eligibility))
        .count();
    (eligible as f64 / all.len() as f64 * 100.0).round() as i32
}

/// Режимы для голосования (`GetGamePresets`): показываемые и подходящие по числу игроков.
pub fn game_presets(world: &mut World) -> Vec<(String, String)> {
    let players = sessions(world).len() as i32;
    let Some(prototypes) = world.get_resource::<astrocore_prototypes::Prototypes>() else {
        return Vec::new();
    };
    let mut ids: Vec<&str> = prototypes.ids("gamePreset").collect();
    ids.sort_unstable();
    ids.into_iter()
        .filter_map(|id| game_ticker::game_preset(prototypes, id))
        .filter(|p| p.show_in_vote)
        .filter(|p| players >= p.min_players.unwrap_or(i32::MIN))
        .filter(|p| players <= p.max_players.unwrap_or(i32::MAX))
        .map(|p| (p.id, p.name))
        .collect()
}

/// Можно ли вызвать (`CanCallVote`): (можно, администратор, сколько ждать).
pub fn can_call_vote(
    world: &mut World,
    session: Entity,
    vote_type: Option<StandardVoteType>,
) -> (bool, bool, Duration) {
    if admin_manager::has_flag(world, session, flags::MODERATOR) {
        return (true, true, Duration::ZERO);
    }
    if !world.cvar(&VOTE_ENABLED) {
        return (false, false, Duration::ZERO);
    }
    if let Some(kind) = vote_type
        && !world.cvar(&kind.enabled())
    {
        return (false, false, Duration::ZERO);
    }
    if !votes(world).votes.is_empty() {
        return (false, false, Duration::ZERO);
    }
    let now = Instant::now();
    if let Some(kind) = vote_type
        && let Some(until) = votes(world).standard_timeout.get(&kind).copied()
    {
        return (false, false, until.saturating_duration_since(now));
    }
    // Один режим, и он уже текущий, — не голосование.
    if vote_type == Some(StandardVoteType::Preset) {
        let presets = game_presets(world);
        let current = world
            .get_resource::<game_ticker::GameTicker>()
            .and_then(|t| t.preset.clone());
        if presets.len() == 1 && Some(&presets[0].0) == current.as_ref() {
            return (false, false, Duration::ZERO);
        }
    }
    let Some(user) = world.get::<Session>(session).map(|s| s.user_id) else {
        return (false, false, Duration::ZERO);
    };
    match votes(world).vote_timeout.get(&user).copied() {
        Some(until) => (false, false, until.saturating_duration_since(now)),
        None => (true, false, Duration::ZERO),
    }
}

/// Создать голосование (`CreateVote`): номер.
pub fn create_vote(world: &mut World, options: VoteOptions) -> i32 {
    let now = Instant::now();
    let initiator_user = options
        .initiator
        .and_then(|s| world.get::<Session>(s))
        .map(|s| s.user_id);
    let mut state = votes(world);
    state.next_id += 1;
    let id = state.next_id;
    state.votes.insert(
        id,
        VoteReg {
            title: options.title,
            initiator_text: options.initiator_text,
            start: now,
            end: now + options.duration,
            entries: options
                .options
                .into_iter()
                .map(|(text, data)| VoteEntry {
                    data,
                    text,
                    votes: 0,
                })
                .collect(),
            cast: BTreeMap::new(),
            votes_dirty: BTreeSet::new(),
            eligibility: options.eligibility,
            display_votes: options.display_votes,
            finished: false,
            cancelled: false,
            dirty: true,
            on_finished: Vec::new(),
        },
    );
    if let Some(user) = initiator_user {
        let timeout = options.initiator_timeout.unwrap_or(options.duration * 2);
        state.vote_timeout.insert(user, now + timeout);
    }
    dirty_can_call_all(world);
    id
}

/// Что сделать по итогу (`OnFinished`).
pub fn on_finished(world: &mut World, id: i32, action: VoteFinished) {
    if let Some(vote) = votes(world).votes.get_mut(&id) {
        vote.on_finished.push(action);
    }
}

/// Голос игрока (`CastVote`); `None` — снять голос. `false` — нет такого голосования или
/// варианта.
pub fn cast_vote(world: &mut World, id: i32, session: Entity, option: Option<usize>) -> bool {
    let mut state = votes(world);
    let Some(vote) = state.votes.get_mut(&id) else {
        return false;
    };
    if option.is_some_and(|o| o >= vote.entries.len()) {
        return false;
    }
    if let Some(previous) = vote.cast.get(&session).copied() {
        vote.entries[previous].votes -= 1;
    }
    match option {
        Some(option) => {
            vote.entries[option].votes += 1;
            vote.cast.insert(session, option);
        }
        None => {
            vote.cast.remove(&session);
        }
    }
    vote.votes_dirty.insert(session);
    vote.dirty = true;
    true
}

/// Отменить (`CancelVote`).
pub fn cancel_vote(world: &mut World, id: i32) -> bool {
    let mut state = votes(world);
    let Some(vote) = state.votes.get_mut(&id) else {
        return false;
    };
    if vote.cancelled {
        return false;
    }
    vote.cancelled = true;
    vote.finished = true;
    vote.dirty = true;
    vote.on_finished.clear();
    dirty_can_call_all(world);
    true
}

/// Подвести итог (`EndVote`): неподходящие голоса снимаются, лидеры — победители.
fn end_vote(world: &mut World, id: i32) {
    let eligibility = match votes(world).votes.get(&id) {
        Some(vote) if !vote.finished => vote.eligibility,
        _ => return,
    };
    let voters: Vec<Entity> = votes(world).votes[&id].cast.keys().copied().collect();
    for voter in voters {
        if !check_voter_eligibility(world, voter, eligibility) {
            cast_vote(world, id, voter, None);
        }
    }
    let (outcome, actions) = {
        let mut state = votes(world);
        let vote = state.votes.get_mut(&id).expect("голосование есть");
        let best = vote.entries.iter().map(|e| e.votes).max().unwrap_or(0);
        let winners: Vec<String> = vote
            .entries
            .iter()
            .filter(|e| e.votes == best)
            .map(|e| e.data.clone())
            .collect();
        vote.finished = true;
        vote.dirty = true;
        let outcome = VoteOutcome {
            winner: (winners.len() == 1).then(|| winners[0].clone()),
            winners,
            votes: vote.entries.iter().map(|e| e.votes).collect(),
        };
        (outcome, std::mem::take(&mut vote.on_finished))
    };
    for action in actions {
        action(world, &outcome);
    }
    dirty_can_call_all(world);
}

/// Голоса по вариантам (`VotesPerOption`).
pub fn votes_per_option(world: &World, id: i32) -> BTreeMap<String, usize> {
    world
        .get_resource::<Votes>()
        .and_then(|v| v.votes.get(&id))
        .map(|vote| {
            vote.entries
                .iter()
                .map(|e| (e.data.clone(), e.votes))
                .collect()
        })
        .unwrap_or_default()
}

/// Идущие голосования: (номер, кто вызвал, заголовок).
pub fn active_votes(world: &World) -> Vec<(i32, String, String)> {
    world
        .get_resource::<Votes>()
        .map(|v| {
            v.votes
                .iter()
                .filter(|(_, vote)| !vote.finished)
                .map(|(id, vote)| (*id, vote.initiator_text.clone(), vote.title.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Сообщение одному игроку (`SendSingleUpdate`).
fn vote_data(world: &World, id: i32, vote: &VoteReg, session: Entity) -> VoteData {
    let mut data = VoteData {
        vote_id: id,
        active: !vote.finished,
        title: String::new(),
        initiator: String::new(),
        elapsed: 0.0,
        duration: 0.0,
        options: Vec::new(),
        your_vote: None,
        display_votes: false,
    };
    if !check_voter_eligibility(world, session, vote.eligibility) {
        data.active = false;
        return data;
    }
    if !vote.finished {
        data.title.clone_from(&vote.title);
        data.initiator.clone_from(&vote.initiator_text);
        data.elapsed = vote.start.elapsed().as_secs_f32();
        data.duration = vote.end.duration_since(vote.start).as_secs_f32();
    }
    if let Some(cast) = vote.cast.get(&session)
        && vote.votes_dirty.contains(&session)
    {
        data.your_vote = Some(Some(*cast as u8));
    } else if vote.votes_dirty.contains(&session) {
        data.your_vote = Some(None);
    }
    // Администратор видит голоса всегда.
    data.display_votes =
        vote.display_votes || admin_manager::has_flag(world, session, flags::MODERATOR);
    data.options = vote
        .entries
        .iter()
        .map(|e| {
            let count = if data.display_votes {
                e.votes as u16
            } else {
                0
            };
            (count, e.text.clone())
        })
        .collect();
    data
}

fn send_vote_to(world: &mut World, id: i32, session: Entity) {
    let Some(state) = world.get_resource::<Votes>() else {
        return;
    };
    let Some(vote) = state.votes.get(&id) else {
        return;
    };
    let data = vote_data(world, id, vote, session);
    world.send_net_event(NetTarget::Session(session), &data);
}

fn send_can_call(world: &mut World, session: Entity) {
    let (can_call, is_admin, wait) = can_call_vote(world, session, None);
    let unavailable = if is_admin {
        Vec::new()
    } else {
        StandardVoteType::ALL
            .into_iter()
            .filter_map(|kind| {
                let (ok, _, wait) = can_call_vote(world, session, Some(kind));
                (!ok).then_some((kind, wait.as_secs_f32()))
            })
            .collect()
    };
    let message = VoteCanCall {
        can_call,
        when_can_call: wait.as_secs_f32(),
        unavailable,
    };
    world.send_net_event(NetTarget::Session(session), &message);
}

/// Такт (`Update`): итоги по сроку, рассылка, перерывы, право вызова.
pub fn update(world: &mut World) {
    let now = Instant::now();
    let ids: Vec<i32> = votes(world).votes.keys().copied().collect();
    for id in ids {
        let due = votes(world).votes.get(&id).is_some_and(|v| now >= v.end);
        if due {
            end_vote(world, id);
        }
        let dirty = votes(world).votes.get(&id).is_some_and(|v| v.dirty);
        if dirty {
            for session in sessions(world) {
                send_vote_to(world, id, session);
            }
            if let Some(vote) = votes(world).votes.get_mut(&id) {
                vote.votes_dirty.clear();
                vote.dirty = false;
            }
        }
        if votes(world).votes.get(&id).is_some_and(|v| v.finished) {
            votes(world).votes.remove(&id);
        }
    }
    let expired_users: Vec<NetUserId> = votes(world)
        .vote_timeout
        .iter()
        .filter(|(_, until)| **until < now)
        .map(|(user, _)| *user)
        .collect();
    for user in expired_users {
        votes(world).vote_timeout.remove(&user);
        if let Some(session) = crate::mind::session_of_user(world, user) {
            votes(world).can_call_dirty.insert(session);
        }
    }
    let expired_types: Vec<StandardVoteType> = votes(world)
        .standard_timeout
        .iter()
        .filter(|(_, until)| **until < now)
        .map(|(kind, _)| *kind)
        .collect();
    if !expired_types.is_empty() {
        for kind in expired_types {
            votes(world).standard_timeout.remove(&kind);
        }
        dirty_can_call_all(world);
    }
    let dirty: Vec<Entity> = std::mem::take(&mut votes(world).can_call_dirty)
        .into_iter()
        .collect();
    for session in dirty {
        if world.get::<Session>(session).is_some() {
            send_can_call(world, session);
        }
    }
}

fn initiator_text(world: &World, initiator: Option<Entity>) -> String {
    match initiator {
        Some(session) => session_name(world, session),
        None => loc(world, "ui-vote-initiator-server", &[]),
    }
}

/// Длительность и перерыв вызвавшему; один на сервере — короткие (`alone`).
fn timing(
    world: &mut World,
    initiator: Option<Entity>,
    timer: CVarDef<i32>,
) -> (Duration, Option<Duration>) {
    let alone = sessions(world).len() == 1 && initiator.is_some();
    let seconds =
        |world: &World, def: &CVarDef<i32>| Duration::from_secs(world.cvar(def).max(0) as u64);
    if alone {
        (
            seconds(world, &VOTE_TIMER_ALONE),
            Some(Duration::from_secs(10)),
        )
    } else {
        (seconds(world, &timer), None)
    }
}

/// Перерыв вида (`TimeoutStandardVote`).
fn timeout_standard_vote(world: &mut World, kind: StandardVoteType) {
    let seconds = world.cvar(&VOTE_SAME_TYPE_TIMEOUT).max(0.0);
    votes(world)
        .standard_timeout
        .insert(kind, Instant::now() + Duration::from_secs_f32(seconds));
    dirty_can_call_all(world);
}

/// Стандартное голосование (`CreateStandardVote`).
pub fn create_standard_vote(world: &mut World, initiator: Option<Entity>, kind: StandardVoteType) {
    match initiator {
        Some(session) => {
            let name = session_name(world, session);
            log(
                world,
                LogImpact::Medium,
                format!("{name} initiated a {kind:?} vote"),
            );
        }
        None => log(
            world,
            LogImpact::Medium,
            format!("Initiated a {kind:?} vote"),
        ),
    }
    match kind {
        StandardVoteType::Restart => {
            // В лобби перезапуск не голосуется.
            if game_ticker::run_level(world) != GameRunLevel::InRound {
                return;
            }
            create_restart_vote(world, initiator);
        }
        StandardVoteType::Preset => create_preset_vote(world, initiator),
        StandardVoteType::Map => create_map_vote(world, initiator),
    }
    game_ticker::update_info_text(world);
    timeout_standard_vote(world, kind);
}

/// Перезапуск (`CreateRestartVote`): на большом сервере нужна доля призраков.
fn create_restart_vote(world: &mut World, initiator: Option<Entity>) {
    let max_players = world.cvar(&VOTE_RESTART_MAX_PLAYERS);
    let total = sessions(world).len() as i32;
    let required = world.cvar(&VOTE_RESTART_GHOST_PERCENTAGE);
    let ghosts = eligible_voter_percentage(world, VoterEligibility::Ghost);
    if total <= max_players || ghosts >= required {
        start_restart_vote(world, initiator);
    } else {
        log(
            world,
            LogImpact::Medium,
            format!(
                "Restart vote failed: Current Ghost player percentage:{ghosts}% does not meet {required}%"
            ),
        );
        announce(
            world,
            "ui-vote-restart-fail-not-enough-ghost-players",
            &[("ghostPlayerRequirement", LocArg::from(i64::from(required)))],
        );
    }
}

fn start_restart_vote(world: &mut World, initiator: Option<Entity>) {
    let (duration, alone_timeout) = timing(world, initiator, VOTE_TIMER_RESTART);
    let options = VoteOptions {
        title: loc(world, "ui-vote-restart-title", &[]),
        initiator_text: initiator_text(world, initiator),
        initiator,
        duration,
        initiator_timeout: alone_timeout.or(Some(Duration::from_secs(300))),
        options: [
            ("ui-vote-restart-yes", "yes"),
            ("ui-vote-restart-no", "no"),
            ("ui-vote-restart-abstain", "abstain"),
        ]
        .into_iter()
        .map(|(key, data)| (loc(world, key, &[]), data.to_owned()))
        .collect(),
        ..VoteOptions::default()
    };
    let id = create_vote(world, options);
    on_finished(
        world,
        id,
        Box::new(|world, outcome| {
            let yes = outcome.votes[0];
            let no = outcome.votes[1];
            let total = yes + no;
            let ratio = world.cvar(&VOTE_RESTART_REQUIRED_RATIO);
            if total > 0 && yes as f32 / total as f32 >= ratio {
                if world.cvar(&VOTE_RESTART_NOT_ALLOWED_WHEN_ADMIN_ONLINE)
                    && !admin_manager::active_admins(world).is_empty()
                {
                    log(
                        world,
                        LogImpact::Medium,
                        format!(
                            "Restart vote attempted to pass, but an admin was online. {yes}/{no}"
                        ),
                    );
                } else {
                    log(
                        world,
                        LogImpact::Medium,
                        format!("Restart vote succeeded: {yes}/{no}"),
                    );
                    announce(world, "ui-vote-restart-succeeded", &[]);
                    crate::round_end::end_round(world, None);
                }
            } else {
                log(
                    world,
                    LogImpact::Medium,
                    format!("Restart vote failed: {yes}/{no}"),
                );
                announce(
                    world,
                    "ui-vote-restart-failed",
                    &[("ratio", LocArg::from(f64::from(ratio)))],
                );
            }
        }),
    );
    // Вызвавший — «за», остальные — «воздержались».
    if let Some(initiator) = initiator {
        cast_vote(world, id, initiator, Some(0));
    }
    for session in sessions(world) {
        if Some(session) != initiator {
            cast_vote(world, id, session, Some(2));
        }
    }
}

/// Победитель или случайный из лидеров при ничьей.
fn pick(world: &mut World, outcome: &VoteOutcome) -> (String, bool) {
    match &outcome.winner {
        Some(winner) => (winner.clone(), false),
        None => {
            let index =
                crate::random::random(world).range(0, outcome.winners.len() as i32) as usize;
            (outcome.winners[index].clone(), true)
        }
    }
}

/// Режим (`CreatePresetVote`).
fn create_preset_vote(world: &mut World, initiator: Option<Entity>) {
    let presets = game_presets(world);
    let (duration, alone_timeout) = timing(world, initiator, VOTE_TIMER_PRESET);
    let names: BTreeMap<String, String> = presets
        .iter()
        .map(|(id, name)| (id.clone(), loc(world, name, &[])))
        .collect();
    let options = VoteOptions {
        title: loc(world, "ui-vote-gamemode-title", &[]),
        initiator_text: initiator_text(world, initiator),
        initiator,
        duration,
        initiator_timeout: alone_timeout,
        options: presets
            .iter()
            .map(|(id, _)| (names[id].clone(), id.clone()))
            .collect(),
        ..VoteOptions::default()
    };
    let id = create_vote(world, options);
    on_finished(
        world,
        id,
        Box::new(move |world, outcome| {
            let (picked, tie) = pick(world, outcome);
            let name = names
                .get(&picked)
                .cloned()
                .unwrap_or_else(|| picked.clone());
            if tie {
                announce(
                    world,
                    "ui-vote-gamemode-tie",
                    &[("picked", LocArg::from(name))],
                );
            } else {
                announce(
                    world,
                    "ui-vote-gamemode-win",
                    &[("winner", LocArg::from(name))],
                );
            }
            log(
                world,
                LogImpact::Medium,
                format!("Preset vote finished: {picked}"),
            );
            game_ticker::set_game_preset(world, &picked, false, None);
        }),
    );
}

/// Карта (`CreateMapVote`): итог применяется, только пока карту ещё можно сменить.
fn create_map_vote(world: &mut World, initiator: Option<Entity>) {
    let maps = crate::game_maps::currently_eligible_maps(world);
    let (duration, alone_timeout) = timing(world, initiator, VOTE_TIMER_MAP);
    let names: BTreeMap<String, String> = maps
        .iter()
        .map(|m| (m.id.clone(), m.map_name.clone()))
        .collect();
    let options = VoteOptions {
        title: loc(world, "ui-vote-map-title", &[]),
        initiator_text: initiator_text(world, initiator),
        initiator,
        duration,
        initiator_timeout: alone_timeout,
        options: maps
            .iter()
            .map(|m| (m.map_name.clone(), m.id.clone()))
            .collect(),
        ..VoteOptions::default()
    };
    let id = create_vote(world, options);
    on_finished(
        world,
        id,
        Box::new(move |world, outcome| {
            let (picked, tie) = pick(world, outcome);
            let name = names
                .get(&picked)
                .cloned()
                .unwrap_or_else(|| picked.clone());
            if tie {
                announce(
                    world,
                    "ui-vote-map-tie",
                    &[("picked", LocArg::from(name.clone()))],
                );
            } else {
                announce(
                    world,
                    "ui-vote-map-win",
                    &[("winner", LocArg::from(name.clone()))],
                );
            }
            log(
                world,
                LogImpact::Medium,
                format!("Map vote finished: {name}"),
            );
            if game_ticker::can_update_map(world) {
                if crate::game_maps::select_map(world, &picked) {
                    game_ticker::update_info_text(world);
                } else {
                    announce(
                        world,
                        "ui-vote-map-invalid",
                        &[("winner", LocArg::from(name))],
                    );
                }
            } else if game_ticker::run_level(world) != GameRunLevel::PreRoundLobby {
                announce(world, "ui-vote-map-notlobby", &[]);
            } else {
                let preload = game_ticker::ROUND_PRELOAD_TIME.as_secs();
                let time = format!("{}:{:02}", preload / 60, preload % 60);
                announce(
                    world,
                    "ui-vote-map-notlobby-time",
                    &[("time", LocArg::from(time))],
                );
            }
        }),
    );
}

// Команды.

fn createvote(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let [kind] = args else {
        shell.write_error(loc(world, "shell-need-exactly-one-argument", &[]));
        return;
    };
    let Some(kind) = StandardVoteType::parse(kind) else {
        shell.write_error(loc(world, "cmd-createvote-invalid-vote-type", &[]));
        return;
    };
    if let Some(session) = shell.player
        && !can_call_vote(world, session, Some(kind)).0
    {
        let name = session_name(world, session);
        log(
            world,
            LogImpact::Medium,
            format!("{name} failed to start {kind:?} vote"),
        );
        shell.write_error(loc(world, "cmd-createvote-cannot-call-vote-now", &[]));
        return;
    }
    create_standard_vote(world, shell.player, kind);
}

fn customvote(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    if args.len() < 3 || args.len() > 10 {
        shell.write_error(loc(
            world,
            "shell-need-between-arguments",
            &[("lower", LocArg::from(3)), ("upper", LocArg::from(10))],
        ));
        return;
    }
    let title = args[0].clone();
    let texts: Vec<String> = args[1..].to_vec();
    let options = VoteOptions {
        title: title.clone(),
        initiator_text: match shell.player {
            Some(session) => session_name(world, session),
            None => loc(world, "vote-options-server-initiator-text", &[]),
        },
        initiator: shell.player,
        duration: Duration::from_secs(30),
        options: texts
            .iter()
            .enumerate()
            .map(|(i, text)| (text.clone(), i.to_string()))
            .collect(),
        ..VoteOptions::default()
    };
    let who = shell
        .player
        .map(|s| format!("{} initiated", session_name(world, s)))
        .unwrap_or_else(|| "Initiated".to_owned());
    log(
        world,
        LogImpact::Medium,
        format!("{who} a custom vote: {title} - {}", texts.join("; ")),
    );
    let id = create_vote(world, options);
    on_finished(
        world,
        id,
        Box::new(move |world, outcome| {
            let text = |data: &str| {
                data.parse::<usize>()
                    .ok()
                    .and_then(|i| texts.get(i).cloned())
                    .unwrap_or_default()
            };
            match &outcome.winner {
                Some(winner) => {
                    let winner = text(winner);
                    log(
                        world,
                        LogImpact::Medium,
                        format!("Custom vote {title} finished: {winner}"),
                    );
                    announce(
                        world,
                        "cmd-customvote-on-finished-win",
                        &[
                            ("title", LocArg::from(title.clone())),
                            ("winner", LocArg::from(winner)),
                        ],
                    );
                }
                None => {
                    let ties: Vec<String> = outcome.winners.iter().map(|w| text(w)).collect();
                    let ties = ties.join(", ");
                    log(
                        world,
                        LogImpact::Medium,
                        format!("Custom vote {title} finished as tie: {ties}"),
                    );
                    announce(
                        world,
                        "cmd-customvote-on-finished-tie",
                        &[
                            ("title", LocArg::from(title.clone())),
                            ("ties", LocArg::from(ties)),
                        ],
                    );
                }
            }
        }),
    );
}

fn vote_command(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let Some(session) = shell.player else {
        shell.write_error(loc(world, "cmd-vote-on-execute-error-must-be-player", &[]));
        return;
    };
    let [id, option] = args else {
        shell.write_error(loc(
            world,
            "shell-wrong-arguments-number-need-specific",
            &[
                ("properAmount", LocArg::from(2)),
                ("currentAmount", LocArg::from(args.len() as i64)),
            ],
        ));
        return;
    };
    let Ok(id) = id.parse::<i32>() else {
        shell.write_error(loc(world, "cmd-vote-on-execute-error-invalid-vote-id", &[]));
        return;
    };
    let Ok(option) = option.parse::<i32>() else {
        shell.write_error(loc(
            world,
            "cmd-vote-on-execute-error-invalid-vote-options",
            &[],
        ));
        return;
    };
    let entries = world
        .get_resource::<Votes>()
        .and_then(|v| v.votes.get(&id))
        .filter(|v| !v.finished)
        .map(|v| v.entries.len());
    let Some(entries) = entries else {
        shell.write_error(loc(world, "cmd-vote-on-execute-error-invalid-vote", &[]));
        return;
    };
    let option = match option {
        -1 => None,
        n if n >= 0 && (n as usize) < entries => Some(n as usize),
        _ => {
            shell.write_error(loc(world, "cmd-vote-on-execute-error-invalid-option", &[]));
            return;
        }
    };
    cast_vote(world, id, session, option);
}

fn listvotes(world: &mut World, shell: &mut ConsoleShell, _: &[String]) {
    for (id, initiator, title) in active_votes(world) {
        shell.write_line(format!("[{id}] {initiator}: {title}"));
    }
}

fn cancelvote(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let Some(id) = args.first() else {
        shell.write_error(loc(world, "cmd-cancelvote-error-missing-vote-id", &[]));
        return;
    };
    let title = id.parse::<i32>().ok().and_then(|id| {
        world
            .get_resource::<Votes>()?
            .votes
            .get(&id)
            .map(|v| (id, v.title.clone()))
    });
    let Some((id, title)) = title else {
        shell.write_error(loc(world, "cmd-cancelvote-error-invalid-vote-id", &[]));
        return;
    };
    let who = shell
        .player
        .map(|s| format!("{} canceled", session_name(world, s)))
        .unwrap_or_else(|| "Canceled".to_owned());
    log(world, LogImpact::Medium, format!("{who} vote: {title}"));
    cancel_vote(world, id);
}

/// Команды голосований: имена для таблицы прав.
pub const VOTE_COMMANDS: [&str; 3] = ["createvote", "vote", "listvotes"];

pub fn install(world: &mut World, side: Side) {
    world.register_net_event::<VoteData>();
    world.register_net_event::<VoteCanCall>();
    world.register_net_event::<VoteMenuOpened>();
    if !side.is_server() {
        world.init_resource::<ClientVotes>();
        world.subscribe_net_event::<VoteData>(|world, event| {
            let data = event.event.clone();
            if client_receive_vote(world, &data) {
                // Звук нового голосования (`voteding.ogg`).
                astrocore_audio::AudioWorldExt::play_local(
                    world,
                    &astrocore_audio::SoundSpecifier::path("/Audio/Effects/voteding.ogg"),
                    astrocore_audio::SoundSource::Global,
                );
            }
        });
        world.subscribe_net_event::<VoteCanCall>(|world, event| {
            let data = event.event.clone();
            client_receive_can_call(world, &data);
        });
        return;
    }
    for def in [
        VOTE_ENABLED,
        VOTE_RESTART_ENABLED,
        VOTE_PRESET_ENABLED,
        VOTE_MAP_ENABLED,
        VOTE_RESTART_NOT_ALLOWED_WHEN_ADMIN_ONLINE,
    ] {
        world.register_cvar(&def);
    }
    for def in [
        VOTE_RESTART_MAX_PLAYERS,
        VOTE_RESTART_GHOST_PERCENTAGE,
        VOTE_TIMER_MAP,
        VOTE_TIMER_RESTART,
        VOTE_TIMER_PRESET,
        VOTE_TIMER_ALONE,
    ] {
        world.register_cvar(&def);
    }
    world.register_cvar(&VOTE_RESTART_REQUIRED_RATIO);
    world.register_cvar(&VOTE_SAME_TYPE_TIMEOUT);
    world.init_resource::<Votes>();
    for (name, description, help, execute) in [
        (
            "createvote",
            "Вызывает голосование.",
            "createvote <restart|preset|map>",
            createvote as astrocore_console::CommandFn,
        ),
        (
            "customvote",
            "Своё голосование.",
            "customvote <заголовок> <вариант1> <вариант2> [вариант3...]",
            customvote,
        ),
        (
            "vote",
            "Голос в идущем голосовании.",
            "vote <номер> <вариант>",
            vote_command,
        ),
        ("listvotes", "Идущие голосования.", "listvotes", listvotes),
        (
            "cancelvote",
            "Отменяет голосование.",
            "cancelvote <номер>",
            cancelvote,
        ),
    ] {
        world.register_command(ConsoleCommand {
            name,
            description,
            help,
            execute,
            complete: None,
        });
    }
    // Вошедший получает идущие голосования; вышедший снимает свои голоса.
    world.subscribe_broadcast::<PlayerConnected>(|world, event| {
        let session = event.session;
        let ids: Vec<i32> = votes(world).votes.keys().copied().collect();
        for id in ids {
            send_vote_to(world, id, session);
        }
        votes(world).can_call_dirty.insert(session);
    });
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        let session = event.session;
        let ids: Vec<i32> = votes(world).votes.keys().copied().collect();
        for id in ids {
            cast_vote(world, id, session, None);
        }
    });
    world.subscribe_broadcast::<AdminPermsChanged>(|world, event| {
        let session = event.session;
        votes(world).can_call_dirty.insert(session);
    });
    world.subscribe_broadcast::<CVarChanged>(|world, event| {
        let name = event.name.as_str();
        if name == VOTE_ENABLED.name
            || StandardVoteType::ALL
                .into_iter()
                .any(|kind| kind.enabled().name == name)
        {
            dirty_can_call_all(world);
        }
    });
    world.subscribe_net_event::<VoteMenuOpened>(|world, event| {
        if let Some(session) = event.session {
            let name = session_name(world, session);
            log(world, LogImpact::Low, format!("{name} opened vote menu"));
        }
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
