//! Раунд (ADR-0012, ADR-0013): стадии `PreRoundLobby` → `InRound` → `PostRound`, лобби с готовностью
//! и отсчётом, пресет режима и правила раунда, загрузка карты и станций к старту,
//! распределение должностей, поздний вход и наблюдение, итоги раунда, перезапуск с удалением
//! всех сущностей и новым номером раунда в базе. Образец — `GameTicker` и `ServerGameTicker`
//! (`RoundFlow`, `Lobby`, `Player`, `Spawning`, `GamePreset`, `GameRule`, `CVars`),
//! `GamePresetPrototype`, `GameRuleComponent`, команды `Content.Server/GameTicking/Commands`
//! и перезапуск после итогов `RoundEndSystem` SS14 @ 2f4a2aad.
//!
//! Игрок связан с телом через разум (`mind`), наблюдатель — призрак (`ghost`).
//!
//! Нет пока: логики правил (антагонисты, события станции), выбора карты по числу игроков и пулам
//! карт, фона лобби, повторов, вебхуков Discord.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use astrocore_console::{
    CVarDef, CVarFlags, Config, ConfigWorldExt, ConsoleCommand, ConsoleWorldExt,
};
use astrocore_data::DataNode;
use astrocore_ecs::{
    EventWorldExt, GameTime, GameWorldExt, LifecycleWorldExt, MetaData, RegistryWorldExt, Update,
};
use astrocore_loc::{EntityNameWorldExt, LocArg};
use astrocore_map::{
    EntityCoordinates, Map, MapGrid, MapId, MapLoadWorldExt, MapWorldExt, TransformWorldExt,
};
use astrocore_net::{
    NetEvent, NetEventWorldExt, NetTarget, NetUserId, PlayerConnected, PlayerDisconnected, Session,
    SessionWorldExt,
};
use astrocore_prototypes::{PrototypeWorldExt, Prototypes};
use astrocore_resources::Resources;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::preferences::{PreferencesCache, PreferencesLoaded};
use crate::profiles::{HumanoidCharacterProfile, random_profile};
use crate::station_spawning::{SpawnPoint, SpawnPointType};
use crate::stations::{
    StationJobs, StationSpawning, assign_jobs, assign_overflow_jobs, calc_extended_access,
    entity_name, pick_best_available_job, setup_stations, station_from_bits,
};

// Настройки.

pub const GAME_LOBBY_ENABLED: CVarDef<bool> = CVarDef::new(
    "game.lobbyenabled",
    true,
    CVarFlags::ARCHIVE,
    "Лобби перед раундом; без него игроки сразу входят в раунд.",
);
pub const GAME_LOBBY_DURATION: CVarDef<i32> = CVarDef::new(
    "game.lobbyduration",
    150,
    CVarFlags::ARCHIVE,
    "Сколько секунд лобби до старта раунда.",
);
pub const GAME_DEFAULT_PRESET: CVarDef<&str> = CVarDef::new(
    "game.defaultpreset",
    "secret",
    CVarFlags::ARCHIVE,
    "Пресет режима по умолчанию.",
);
pub const GAME_FALLBACK_ENABLED: CVarDef<bool> = CVarDef::new(
    "game.fallbackenabled",
    true,
    CVarFlags::ARCHIVE,
    "Пробовать запасные пресеты, если пресет не стартовал.",
);
pub const GAME_FALLBACK_PRESET: CVarDef<&str> = CVarDef::new(
    "game.fallbackpreset",
    "Traitor,Extended",
    CVarFlags::ARCHIVE,
    "Запасные пресеты через запятую.",
);
pub const GAME_IGNORED_PRESETS: CVarDef<&str> = CVarDef::new(
    "game.ignoredpresets",
    "",
    CVarFlags::ARCHIVE,
    "Правила раунда, которые не добавляются, через запятую.",
);
pub const GAME_DUMMY_TICKER: CVarDef<bool> = CVarDef::new(
    "game.dummyticker",
    false,
    CVarFlags::ARCHIVE.union(CVarFlags::SERVERONLY),
    "Раундом никто не управляет (для отладки).",
);
pub const ROUND_RESTART_TIME: CVarDef<f32> = CVarDef::new(
    "game.round_restart_time",
    120.0,
    CVarFlags::SERVERONLY,
    "Через сколько секунд после итогов перезапускается раунд.",
);
pub const IC_RANDOM_CHARACTERS: CVarDef<bool> = CVarDef::new(
    "ic.random_characters",
    false,
    CVarFlags::SERVER,
    "Каждому игроку — случайный персонаж.",
);

/// За сколько до старта загружается карта (`RoundPreloadTime`).
pub const ROUND_PRELOAD_TIME: Duration = Duration::from_secs(15);
/// Запасная должность (`FallbackOverflowJob`).
pub const FALLBACK_OVERFLOW_JOB: &str = "Passenger";
/// Сущность наблюдателя (`ObserverPrototypeName`).
pub const OBSERVER_PROTOTYPE: &str = "MobObserver";
/// Команды, доступные любому игроку (`AnyCommand`).
pub const ANY_COMMANDS: [&str; 3] = ["toggleready", "joingame", "observe"];

// Состояние.

/// Стадия раунда (`GameRunLevel`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GameRunLevel {
    #[default]
    PreRoundLobby,
    InRound,
    PostRound,
}

/// Статус игрока (`PlayerGameStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerGameStatus {
    NotReadyToPlay,
    ReadyToPlay,
    JoinedGame,
}

/// Правило в списке раунда (`GameRule`): время, сущность, стадия.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleStage {
    Added,
    Started,
    Ended,
}

/// Управление раундом (`ServerGameTicker`). Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct GameTicker {
    pub run_level: GameRunLevel,
    /// Номер раунда в базе; 0 — ещё нет.
    pub round_id: i64,
    pub statuses: BTreeMap<NetUserId, PlayerGameStatus>,
    /// Время старта по часам сервера (`_roundStartTime`).
    pub round_start_time: Option<Duration>,
    pub paused: bool,
    pause_time: Option<Duration>,
    /// Отсчёт не начат: в лобби никого (`_roundStartCountdownHasNotStartedYetDueToNoPlayers`).
    pub waiting_for_players: bool,
    /// Выбранный пресет и пресет идущего раунда.
    pub preset: Option<String>,
    pub current_preset: Option<String>,
    pub reset_countdown: Option<i32>,
    /// Когда начался раунд (`RoundStartTimeSpan`).
    pub round_started_at: Duration,
    pub default_map: Option<MapId>,
    pub disallow_late_join: bool,
    starting: bool,
    /// Имена игроков для итогов (`ContentPlayerData.Name`): живут и после выхода.
    pub player_names: BTreeMap<NetUserId, String>,
    /// Когда перезапустить раунд после итогов.
    pub restart_at: Option<Duration>,
    pub rules: Vec<(Duration, Entity, RuleStage)>,
    pub players_joined_normally: i32,
}

fn now(world: &World) -> Duration {
    world
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default()
}

fn ticker(world: &mut World) -> bevy_ecs::world::Mut<'_, GameTicker> {
    world.get_resource_or_insert_with(GameTicker::default)
}

/// Стадия раунда.
pub fn run_level(world: &World) -> GameRunLevel {
    world
        .get_resource::<GameTicker>()
        .map_or(GameRunLevel::PreRoundLobby, |t| t.run_level)
}

/// Стадия сменилась (`GameRunLevelChangedEvent`). Широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct GameRunLevelChanged {
    pub old: GameRunLevel,
    pub new: GameRunLevel,
}

fn set_run_level(world: &mut World, level: GameRunLevel) {
    let old = std::mem::replace(&mut ticker(world).run_level, level);
    world.raise_broadcast(&mut GameRunLevelChanged { old, new: level });
}

/// Длительность раунда (`RoundDuration`).
pub fn round_duration(world: &World) -> Duration {
    let Some(ticker) = world.get_resource::<GameTicker>() else {
        return Duration::ZERO;
    };
    if ticker.run_level < GameRunLevel::InRound {
        return Duration::ZERO;
    }
    now(world).saturating_sub(ticker.round_started_at)
}

// События.

/// Раунд начинается (`RoundStartingEvent`). Широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct RoundStarting {
    pub round_id: i64,
}

/// Попытка старта (`RoundStartAttemptEvent`): правило может отменить. Широковещательно.
#[derive(Debug, Clone)]
pub struct RoundStartAttempt {
    pub players: Vec<Entity>,
    pub forced: bool,
    pub cancelled: bool,
}

/// Игроки сейчас получат должности (`RulePlayerSpawningEvent`). Широковещательно.
#[derive(Debug, Clone)]
pub struct RulePlayerSpawning {
    pub players: Vec<Entity>,
    pub profiles: BTreeMap<NetUserId, HumanoidCharacterProfile>,
    pub forced: bool,
}

/// Должности розданы (`RulePlayerJobsAssignedEvent`). Широковещательно.
#[derive(Debug, Clone)]
pub struct RulePlayerJobsAssigned {
    pub players: Vec<Entity>,
    pub forced: bool,
}

/// Игрок появился (`PlayerSpawnCompleteEvent`). Широковещательно.
#[derive(Debug, Clone)]
pub struct PlayerSpawnComplete {
    pub mob: Entity,
    pub session: Entity,
    pub job: Option<String>,
    pub late_join: bool,
    pub station: Option<Entity>,
    pub joined_player_count: i32,
}

/// Игрок вошёл в лобби (`PlayerJoinedLobbyEvent`). Широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct PlayerJoinedLobby {
    pub session: Entity,
}

/// Сущности раунда сейчас удалятся (`RoundRestartCleanupEvent`). Широковещательно.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RoundRestartCleanup;

/// Текст итогов дополняется правилами (`RoundEndTextAppendEvent`). Широковещательно.
#[derive(Debug, Clone, Default)]
pub struct RoundEndTextAppend {
    pub text: String,
}

impl RoundEndTextAppend {
    pub fn add_line(&mut self, line: &str) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        self.text.push_str(line);
    }
}

// Сообщения клиенту и от клиента.

/// Сообщения раунда сервер → клиент.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TickerMessage {
    /// В лобби (`TickerJoinLobbyEvent`).
    JoinLobby,
    /// В игре (`TickerJoinGameEvent`).
    JoinGame,
    /// Состояние лобби (`TickerLobbyStatusEvent`): секунды до старта считаются от получения.
    LobbyStatus {
        is_round_started: bool,
        you_are_ready: bool,
        seconds_until_start: Option<f32>,
        paused: bool,
        round_duration: f32,
    },
    /// Текст лобби (`TickerLobbyInfoEvent`).
    LobbyInfo(String),
    /// Отсчёт сдвинут или на паузе (`TickerLobbyCountdownEvent`).
    LobbyCountdown {
        seconds_until_start: Option<f32>,
        paused: bool,
    },
    /// Поздний вход запрещён или разрешён (`TickerLateJoinStatusEvent`).
    LateJoinStatus { disallowed: bool },
    /// Итоги (`RoundEndMessageEvent`).
    RoundEnd(Box<RoundEndMessage>),
    /// Сущности раунда удаляются (`RoundRestartCleanupEvent`).
    RestartCleanup,
}

/// Игрок в итогах (`RoundEndPlayerInfo`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundEndPlayerInfo {
    pub ooc_name: String,
    pub ic_name: Option<String>,
    pub role: String,
    pub antag: bool,
    pub observer: bool,
    pub connected: bool,
}

/// Итоги раунда.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundEndMessage {
    pub gamemode_title: String,
    pub round_end_text: String,
    pub round_duration: f32,
    pub round_id: i64,
    pub players: Vec<RoundEndPlayerInfo>,
}

/// Просьбы игрока клиент → сервер: готовность, вход в раунд, наблюдение.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LobbyRequest {
    ToggleReady(bool),
    JoinGame {
        station: Option<u64>,
        job: Option<String>,
    },
    Observe,
}

fn session_user(world: &World, session: Entity) -> Option<NetUserId> {
    world.get::<Session>(session).map(|s| s.user_id)
}

fn sessions(world: &mut World) -> Vec<(Entity, NetUserId)> {
    let mut list: Vec<(Entity, NetUserId)> = world
        .query::<(Entity, &Session)>()
        .iter(world)
        .map(|(e, s)| (e, s.user_id))
        .collect();
    list.sort_by_key(|(e, _)| *e);
    list
}

fn session_by_user(world: &mut World, user: NetUserId) -> Option<Entity> {
    sessions(world)
        .into_iter()
        .find(|(_, u)| *u == user)
        .map(|(e, _)| e)
}

fn send(world: &mut World, session: Entity, message: &TickerMessage) {
    world.send_net_event(NetTarget::Session(session), message);
}

fn lobby_enabled(world: &World) -> bool {
    world.cvar(&GAME_LOBBY_ENABLED)
}

// Пресеты.

/// Пресет режима (`GamePresetPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct GamePreset {
    pub id: String,
    pub alias: Vec<String>,
    pub name: String,
    pub description: String,
    pub min_players: Option<i32>,
    pub max_players: Option<i32>,
    pub rules: Vec<String>,
}

fn strings(node: Option<&DataNode>) -> Vec<String> {
    node.and_then(DataNode::as_seq)
        .map(|s| {
            s.iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn text(node: &DataNode, key: &str) -> Option<String> {
    node.get(key)?.as_str().map(str::to_owned)
}

/// Пресет по идентификатору прототипа `gamePreset`.
pub fn game_preset(prototypes: &Prototypes, id: &str) -> Option<GamePreset> {
    let node = prototypes.resolved_node("gamePreset", id)?;
    Some(GamePreset {
        id: id.to_owned(),
        alias: strings(node.get("alias")),
        name: text(node, "name").unwrap_or_else(|| "????".into()),
        description: text(node, "description").unwrap_or_default(),
        min_players: text(node, "minPlayers").and_then(|t| t.trim().parse().ok()),
        max_players: text(node, "maxPlayers").and_then(|t| t.trim().parse().ok()),
        rules: strings(node.get("rules")),
    })
}

/// Пресет по идентификатору или псевдониму без учёта регистра (`FindGamePreset`).
pub fn find_game_preset(prototypes: &Prototypes, name: &str) -> Option<GamePreset> {
    if let Some(preset) = game_preset(prototypes, name) {
        return Some(preset);
    }
    let mut ids: Vec<&str> = prototypes.ids("gamePreset").collect();
    ids.sort_unstable();
    ids.into_iter()
        .filter_map(|id| game_preset(prototypes, id))
        .find(|p| p.alias.iter().any(|a| a.eq_ignore_ascii_case(name)))
}

fn preset_of(world: &World, id: Option<&str>) -> Option<GamePreset> {
    game_preset(world.get_resource::<Prototypes>()?, id?)
}

/// Выбрать пресет (`SetGamePreset`); `force` — сразу начать раунд.
pub fn set_game_preset(
    world: &mut World,
    name: &str,
    force: bool,
    reset_delay: Option<i32>,
) -> bool {
    let Some(preset) = world
        .get_resource::<Prototypes>()
        .and_then(|p| find_game_preset(p, name))
    else {
        return false;
    };
    {
        let mut ticker = ticker(world);
        ticker.reset_countdown = reset_delay.map(|delay| {
            if ticker.current_preset.is_none() {
                delay - 1
            } else {
                delay
            }
        });
        ticker.preset = Some(preset.id);
    }
    update_info_text(world);
    if force {
        start_round(world, true);
    }
    true
}

fn initialize_game_preset(world: &mut World) {
    let name = if lobby_enabled(world) {
        world.resource::<Config>().get_text(&GAME_DEFAULT_PRESET)
    } else {
        "sandbox".to_owned()
    };
    set_game_preset(world, &name, false, None);
}

/// Сбросить пресет после заданного числа раундов (`TryResetPreset`).
fn try_reset_preset(world: &mut World) {
    let reset = {
        let mut ticker = ticker(world);
        match ticker.reset_countdown {
            None => false,
            Some(count) if count > 0 => {
                ticker.reset_countdown = Some(count - 1);
                false
            }
            Some(_) => true,
        }
    };
    if reset {
        initialize_game_preset(world);
        ticker(world).reset_countdown = None;
    }
}

// Правила раунда.

/// Правило раунда (`GameRuleComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GameRule {
    pub min_players: i32,
    pub cancel_preset_on_too_few_players: bool,
    pub silent: bool,
    pub delay: Option<MinMax>,
    #[serde(skip)]
    pub activated_at: Duration,
}

/// Промежуток задержки (`MinMax`), секунды.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct MinMax {
    pub min: i32,
    pub max: i32,
}

/// Правило запущено (`ActiveGameRuleComponent`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct ActiveGameRule;

/// Правило ждёт запуска (`DelayedStartRuleComponent`).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DelayedStartRule {
    pub rule_start_time: Duration,
}

/// Правило добавлено, запущено или завершено. Широковещательно.
#[derive(Debug, Clone, Copy)]
pub enum GameRuleEvent {
    Added(Entity),
    Started(Entity),
    Ended(Entity),
}

/// Добавить правило (`AddGameRule`): сущность вне карты; пропущенные настройкой — нет.
pub fn add_game_rule(world: &mut World, rule: &str, force: bool) -> Option<Entity> {
    let ignored = world.resource::<Config>().get_text(&GAME_IGNORED_PRESETS);
    if !force && ignored.split(',').any(|i| i.trim() == rule) {
        return None;
    }
    let entity = match world.spawn_prototype(rule) {
        Ok(entity) => entity,
        Err(error) => {
            eprintln!("правило {rule}: {error}");
            return None;
        }
    };
    world.initialize_and_start(entity, false);
    if world.get::<GameRule>(entity).is_none() {
        eprintln!("у правила {rule} нет GameRule");
        world.entity_mut(entity).insert(GameRule::default());
    }
    let silent = world.get::<GameRule>(entity).is_some_and(|r| r.silent);
    if !silent {
        let time = round_time(world);
        ticker(world).rules.push((time, entity, RuleStage::Added));
    }
    add_admin_log(
        world,
        LogType::EventStarted,
        LogImpact::Medium,
        &format!("Added game rule {rule}"),
        &[],
        &[],
    );
    world.raise_broadcast(&mut GameRuleEvent::Added(entity));
    Some(entity)
}

/// Время раунда (`GetRoundTime`): в лобби — ноль.
pub fn round_time(world: &World) -> Duration {
    if run_level(world) == GameRunLevel::PreRoundLobby {
        Duration::ZERO
    } else {
        round_duration(world)
    }
}

fn set_rule_stage(world: &mut World, rule: Entity, stage: RuleStage) {
    let time = round_time(world);
    let mut ticker = ticker(world);
    match ticker.rules.iter_mut().rev().find(|(_, e, _)| *e == rule) {
        Some(entry) => {
            entry.2 = stage;
            if stage == RuleStage::Started {
                entry.0 = time;
            }
        }
        None if stage == RuleStage::Started => ticker.rules.push((time, rule, stage)),
        None => {}
    }
}

/// Запустить правило (`StartGameRule`): с задержкой — позже.
pub fn start_game_rule(world: &mut World, rule: Entity) -> bool {
    let Some(component) = world.get::<GameRule>(rule).cloned() else {
        return false;
    };
    if world.get::<ActiveGameRule>(rule).is_some() {
        return false;
    }
    let delayed = world.entity_mut(rule).take::<DelayedStartRule>().is_some();
    if let (Some(delay), false) = (component.delay, delayed) {
        let seconds = crate::random::random(world).range(delay.min, delay.max + 1);
        if seconds <= 0 {
            return true;
        }
        let at = now(world) + Duration::from_secs(seconds as u64);
        world.entity_mut(rule).insert(DelayedStartRule {
            rule_start_time: at,
        });
        return true;
    }
    world.entity_mut(rule).insert(ActiveGameRule);
    let time = round_time(world);
    if let Some(mut data) = world.get_mut::<GameRule>(rule) {
        data.activated_at = time;
    }
    if !component.silent {
        set_rule_stage(world, rule, RuleStage::Started);
    }
    add_admin_log(
        world,
        LogType::EventStarted,
        LogImpact::Medium,
        &format!("Started game rule {}", prototype_name(world, rule)),
        &[],
        &[],
    );
    world.raise_broadcast(&mut GameRuleEvent::Started(rule));
    if component.silent {
        end_game_rule(world, rule);
    }
    true
}

/// Завершить правило (`EndGameRule`).
pub fn end_game_rule(world: &mut World, rule: Entity) -> bool {
    let Some(component) = world.entity_mut(rule).take::<GameRule>() else {
        return false;
    };
    world.entity_mut(rule).remove::<ActiveGameRule>();
    if !component.silent {
        set_rule_stage(world, rule, RuleStage::Ended);
    }
    add_admin_log(
        world,
        LogType::EventStopped,
        LogImpact::Medium,
        &format!("Ended game rule {}", prototype_name(world, rule)),
        &[],
        &[],
    );
    world.raise_broadcast(&mut GameRuleEvent::Ended(rule));
    true
}

fn prototype_name(world: &World, entity: Entity) -> String {
    world
        .get::<MetaData>(entity)
        .and_then(|m| m.prototype.as_deref().map(str::to_owned))
        .unwrap_or_default()
}

/// Добавленные правила (`GetAddedGameRules`).
pub fn added_game_rules(world: &mut World) -> Vec<Entity> {
    let mut list: Vec<Entity> = world
        .query_filtered::<Entity, bevy_ecs::query::With<GameRule>>()
        .iter(world)
        .collect();
    list.sort();
    list
}

/// Запущенные правила (`GetActiveGameRules`).
pub fn active_game_rules(world: &mut World) -> Vec<Entity> {
    let mut list: Vec<Entity> = world
        .query_filtered::<Entity, (
            bevy_ecs::query::With<GameRule>,
            bevy_ecs::query::With<ActiveGameRule>,
        )>()
        .iter(world)
        .collect();
    list.sort();
    list
}

/// Удалить все правила (`ClearGameRules`).
pub fn clear_game_rules(world: &mut World) {
    for rule in added_game_rules(world) {
        end_game_rule(world, rule);
        world.delete_entity(rule);
    }
}

fn add_game_preset_rules(world: &mut World) -> bool {
    if world.cvar(&GAME_DUMMY_TICKER) {
        return false;
    }
    let preset_id = ticker(world).preset.clone();
    let Some(preset) = preset_of(world, preset_id.as_deref()) else {
        return false;
    };
    ticker(world).current_preset = Some(preset.id.clone());
    for rule in &preset.rules {
        add_game_rule(world, rule, false);
    }
    true
}

/// Запустить правила пресета (`StartGamePresetRules`).
pub fn start_game_preset_rules(world: &mut World) {
    for rule in added_game_rules(world) {
        start_game_rule(world, rule);
    }
}

/// Отложенные правила, чьё время пришло (`UpdateGameRules`).
fn update_game_rules(world: &mut World) {
    let time = now(world);
    let due: Vec<Entity> = world
        .query::<(Entity, &DelayedStartRule)>()
        .iter(world)
        .filter(|(_, d)| d.rule_start_time <= time)
        .map(|(e, _)| e)
        .collect();
    for rule in due {
        start_game_rule(world, rule);
    }
}

/// Сообщение сервера всем (`SendServerMessage`).
fn send_server_message(world: &mut World, key: &str, args: &[(&str, LocArg)]) {
    let text = crate::examine::loc(world, key, args);
    crate::chat::dispatch_server_announcement(world, &text);
}

/// Сообщение сервера игроку (`DispatchServerMessage`).
fn server_message_to(world: &mut World, session: Entity, key: &str, args: &[(&str, LocArg)]) {
    let text = crate::examine::loc(world, key, args);
    crate::chat::dispatch_server_message(world, session, &text);
}

/// Каждое слово с заглавной (`TextInfo.ToTitleCase`).
fn title_case(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// Лобби.

/// Текст лобби (`GetInfoText`).
pub fn info_text(world: &mut World) -> String {
    let (preset_id, round_id, level) = {
        let ticker = ticker(world);
        (
            ticker
                .current_preset
                .clone()
                .or_else(|| ticker.preset.clone()),
            ticker.round_id,
            ticker.run_level,
        )
    };
    let Some(preset) = preset_of(world, preset_id.as_deref()) else {
        return String::new();
    };
    let player_count = sessions(world).len();
    let ready_count = ticker(world)
        .statuses
        .values()
        .filter(|s| **s == PlayerGameStatus::ReadyToPlay)
        .count();
    let mut names: Vec<String> = world
        .query_filtered::<Entity, (
            bevy_ecs::query::With<StationJobs>,
            bevy_ecs::query::With<StationSpawning>,
        )>()
        .iter(world)
        .collect::<Vec<_>>()
        .into_iter()
        .map(|station| entity_name(world, station))
        .collect();
    names.sort();
    let map_name = if names.is_empty() {
        selected_map_name(world)
            .unwrap_or_else(|| crate::examine::loc(world, "game-ticker-no-map-selected", &[]))
    } else {
        names.join("\n")
    };
    let gm_title = crate::examine::loc(world, &preset.name, &[]);
    let desc = crate::examine::loc(world, &preset.description, &[]);
    let key = if level == GameRunLevel::PreRoundLobby {
        "game-ticker-get-info-preround-text"
    } else {
        "game-ticker-get-info-text"
    };
    crate::examine::loc(
        world,
        key,
        &[
            ("roundId", LocArg::from(round_id)),
            (
                "playerCount",
                LocArg::from(player_count.to_string().as_str()),
            ),
            ("readyCount", LocArg::from(ready_count)),
            ("mapName", LocArg::from(map_name.as_str())),
            ("gmTitle", LocArg::from(gm_title.as_str())),
            ("desc", LocArg::from(desc.as_str())),
        ],
    )
}

fn selected_map_name(world: &World) -> Option<String> {
    let setting = world.resource::<Config>().get_text(&crate::content::MAP);
    crate::stations::game_map(world.get_resource::<Prototypes>()?, &setting).map(|m| m.map_name)
}

/// Разослать текст лобби (`UpdateInfoText`).
pub fn update_info_text(world: &mut World) {
    if !world.contains_resource::<astrocore_net::ServerNet>() {
        return;
    }
    let text = info_text(world);
    world.send_net_event(NetTarget::All, &TickerMessage::LobbyInfo(text));
}

fn seconds_until_start(world: &World) -> Option<f32> {
    let ticker = world.get_resource::<GameTicker>()?;
    let start = ticker.round_start_time?;
    let reference = if ticker.paused {
        ticker.pause_time.unwrap_or_else(|| now(world))
    } else {
        now(world)
    };
    Some(start.saturating_sub(reference).as_secs_f32())
}

/// Состояние лобби для игрока (`GetStatusMsg`).
pub fn status_message(world: &mut World, session: Entity) -> TickerMessage {
    let user = session_user(world, session);
    let seconds = seconds_until_start(world);
    let duration = round_duration(world).as_secs_f32();
    let ticker = ticker(world);
    let ready = user
        .and_then(|u| ticker.statuses.get(&u))
        .is_some_and(|s| *s == PlayerGameStatus::ReadyToPlay);
    TickerMessage::LobbyStatus {
        is_round_started: ticker.run_level != GameRunLevel::PreRoundLobby,
        you_are_ready: ready,
        seconds_until_start: seconds,
        paused: ticker.paused,
        round_duration: duration,
    }
}

fn send_status_to_all(world: &mut World) {
    for (session, _) in sessions(world) {
        let message = status_message(world, session);
        send(world, session, &message);
    }
}

fn update_late_join_status(world: &mut World) {
    let disallowed = ticker(world).disallow_late_join;
    world.send_net_event(
        NetTarget::All,
        &TickerMessage::LateJoinStatus { disallowed },
    );
}

fn send_countdown(world: &mut World) {
    let message = TickerMessage::LobbyCountdown {
        seconds_until_start: seconds_until_start(world),
        paused: ticker(world).paused,
    };
    world.send_net_event(NetTarget::All, &message);
}

/// Отложить старт (`DelayStart`).
pub fn delay_start(world: &mut World, time: Duration) -> bool {
    if run_level(world) != GameRunLevel::PreRoundLobby {
        return false;
    }
    let base = ticker(world).round_start_time.unwrap_or_else(|| now(world));
    ticker(world).round_start_time = Some(base + time);
    send_countdown(world);
    let text = crate::examine::loc(
        world,
        "game-ticker-delay-start",
        &[("seconds", LocArg::from(time.as_secs_f64()))],
    );
    crate::chat::dispatch_server_announcement(world, &text);
    true
}

/// Пауза отсчёта (`PauseStart`).
pub fn pause_start(world: &mut World, pause: bool) -> bool {
    if ticker(world).paused == pause {
        return false;
    }
    let time = now(world);
    let mut ticker = ticker(world);
    ticker.paused = pause;
    if pause {
        ticker.pause_time = Some(time);
    } else if let Some(paused_at) = ticker.pause_time.take()
        && let Some(start) = ticker.round_start_time.as_mut()
    {
        *start += time.saturating_sub(paused_at);
    }
    send_countdown(world);
    let key = if pause {
        "game-ticker-pause-start"
    } else {
        "game-ticker-pause-start-resumed"
    };
    send_server_message(world, key, &[]);
    true
}

/// Готовность игрока (`ToggleReady`): только в лобби и после загрузки настроек.
pub fn toggle_ready(world: &mut World, session: Entity, ready: bool) {
    let Some(user) = session_user(world, session) else {
        return;
    };
    if !ticker(world).statuses.contains_key(&user)
        || world.resource::<PreferencesCache>().get(user).is_none()
        || run_level(world) != GameRunLevel::PreRoundLobby
    {
        return;
    }
    ticker(world).statuses.insert(
        user,
        if ready {
            PlayerGameStatus::ReadyToPlay
        } else {
            PlayerGameStatus::NotReadyToPlay
        },
    );
    let message = status_message(world, session);
    send(world, session, &message);
    update_info_text(world);
}

/// Готовность всем (`ToggleReadyAll`).
pub fn toggle_ready_all(world: &mut World, ready: bool) {
    let status = if ready {
        PlayerGameStatus::ReadyToPlay
    } else {
        PlayerGameStatus::NotReadyToPlay
    };
    for value in ticker(world).statuses.values_mut() {
        *value = status;
    }
    send_status_to_all(world);
}

/// Готовые игроки (`ReadyPlayerCount`).
pub fn ready_player_count(world: &mut World) -> usize {
    let lobby = lobby_enabled(world);
    let online: BTreeSet<NetUserId> = sessions(world).into_iter().map(|(_, u)| u).collect();
    ticker(world)
        .statuses
        .iter()
        .filter(|(user, status)| {
            online.contains(user) && !(lobby && **status == PlayerGameStatus::NotReadyToPlay)
        })
        .count()
}

/// Игрок в игре (`UserHasJoinedGame`).
pub fn user_has_joined_game(world: &World, user: NetUserId) -> bool {
    world
        .get_resource::<GameTicker>()
        .and_then(|t| t.statuses.get(&user))
        .is_some_and(|s| *s == PlayerGameStatus::JoinedGame)
}

fn add_round_player(world: &mut World, user: NetUserId) {
    let round = ticker(world).round_id;
    if round == 0 {
        return;
    }
    crate::database::spawn_db(
        world,
        move |db| async move { db.add_round_players(round, &[user]).await },
        |_, result| {
            if let Err(error) = result {
                eprintln!("игрок раунда: {error}");
            }
        },
    );
}

/// В лобби (`PlayerJoinLobby`).
pub fn player_join_lobby(world: &mut World, session: Entity) {
    let Some(user) = session_user(world, session) else {
        return;
    };
    let status = if lobby_enabled(world) {
        PlayerGameStatus::NotReadyToPlay
    } else {
        PlayerGameStatus::ReadyToPlay
    };
    ticker(world).statuses.insert(user, status);
    add_round_player(world, user);
    send(world, session, &TickerMessage::JoinLobby);
    let status = status_message(world, session);
    send(world, session, &status);
    let info = TickerMessage::LobbyInfo(info_text(world));
    send(world, session, &info);
    let jobs = crate::stations::jobs_available(world);
    world.send_net_event(NetTarget::Session(session), &jobs);
    world.raise_broadcast(&mut PlayerJoinedLobby { session });
}

/// В игре (`PlayerJoinGame`).
pub fn player_join_game(world: &mut World, session: Entity) {
    let Some(user) = session_user(world, session) else {
        return;
    };
    ticker(world)
        .statuses
        .insert(user, PlayerGameStatus::JoinedGame);
    add_round_player(world, user);
    server_message_to(world, session, "game-ticker-player-join-game-message", &[]);
    send(world, session, &TickerMessage::JoinGame);
}

// Карта.

/// Загрузить карту раунда без инициализации (`LoadMaps`): прототип `gameMap` из `game.map`
/// или путь файла; станции — из её гридов.
pub fn load_maps(world: &mut World) {
    if let Some(map) = ticker(world).default_map
        && world.map_entity(map).is_some()
    {
        return;
    }
    add_game_preset_rules(world);
    let setting = world.resource::<Config>().get_text(&crate::content::MAP);
    let game_map = world
        .get_resource::<Prototypes>()
        .and_then(|p| crate::stations::game_map(p, &setting));
    let path = game_map
        .as_ref()
        .map_or(setting, |m| crate::stations::resource_path(&m.map_path));
    let Some(resources) = world.get_resource::<Resources>().cloned() else {
        eprintln!("карта {path}: нет ресурсов");
        return;
    };
    let loaded = resources
        .read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| astrocore_data::parse_yaml(&text).map_err(|e| e.to_string()))
        .and_then(|node| world.load_map(&node, false).map_err(|e| e.to_string()));
    let report = match loaded {
        Ok(report) => report,
        Err(error) => {
            eprintln!("карта {path}: {error}");
            return;
        }
    };
    for error in report.errors.iter().take(20) {
        eprintln!("карта: {error}");
    }
    let Some((map_entity, map_id)) = report.maps.first().copied() else {
        return;
    };
    ticker(world).default_map = Some(map_id);
    if let Some(game_map) = &game_map {
        if let Some(mut meta) = world.get_mut::<MetaData>(map_entity) {
            meta.name = Some(game_map.map_name.clone());
        }
        setup_stations(world, game_map, &report.grids, None);
    }
}

// Раунд.

fn increment_round_number(world: &mut World) {
    let players: Vec<NetUserId> = ticker(world).statuses.keys().copied().collect();
    let round = match world.get_resource::<crate::database::Database>() {
        Some(database) => {
            let db = database.db().clone();
            database.block_on(async {
                let server = db.server_id().await?;
                db.add_new_round(server, &players).await
            })
        }
        None => Err(crate::database::db_error("нет базы")),
    };
    let round = match round {
        Ok(round) => round,
        Err(error) => {
            eprintln!("новый раунд в базе: {error}");
            ticker(world).round_id + 1
        }
    };
    ticker(world).round_id = round;
    crate::admin_logs::round_starting(world, round);
}

/// Профиль игрока для раунда: выбранный или случайный (`StartRound`, `GetPlayerProfile`).
fn player_profile(world: &mut World, user: NetUserId) -> HumanoidCharacterProfile {
    let selected = world
        .get_resource::<PreferencesCache>()
        .and_then(|c| c.selected_profile(user).cloned());
    let mut profile = match selected {
        Some(profile) => profile,
        None => {
            let blacklist: BTreeSet<String> = world
                .resource::<Config>()
                .get_text(&crate::profiles::IC_NEW_ACCOUNT_SPECIES_BLACKLIST)
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            random_profile(world, &blacklist)
        }
    };
    if world.cvar(&IC_RANDOM_CHARACTERS) {
        let jobs = profile.job_priorities.clone();
        profile = random_profile(world, &BTreeSet::new());
        profile.job_priorities = jobs;
    }
    profile
}

/// Начать раунд (`StartRound`): готовые игроки, карта, правила пресета, инициализация карты,
/// должности и появление, стадия «в раунде».
pub fn start_round(world: &mut World, force: bool) {
    if world.cvar(&GAME_DUMMY_TICKER) || ticker(world).starting {
        return;
    }
    if run_level(world) != GameRunLevel::PreRoundLobby {
        return;
    }
    ticker(world).starting = true;
    if ticker(world).round_id == 0 {
        increment_round_number(world);
    }
    send_server_message(world, "game-ticker-start-round", &[]);
    let lobby = lobby_enabled(world);
    let mut ready = Vec::new();
    let mut profiles = BTreeMap::new();
    for (session, user) in sessions(world) {
        let status = ticker(world).statuses.get(&user).copied();
        if lobby && status != Some(PlayerGameStatus::ReadyToPlay) {
            continue;
        }
        if status.is_none() {
            continue;
        }
        ready.push(session);
        let profile = player_profile(world, user);
        profiles.insert(user, profile);
    }
    load_maps(world);
    update_info_text(world);
    start_game_preset_rules(world);
    let round_id = ticker(world).round_id;
    world.raise_broadcast(&mut RoundStarting { round_id });
    if !start_preset(world, &ready, force) {
        ticker(world).starting = false;
        return;
    }
    if let Some(map) = ticker(world).default_map.and_then(|m| world.map_entity(m)) {
        world.initialize_map(map);
    }
    spawn_players(world, &ready, profiles, force);
    set_run_level(world, GameRunLevel::InRound);
    let time = now(world);
    ticker(world).round_started_at = time;
    send_status_to_all(world);
    update_late_join_status(world);
    update_info_text(world);
    ticker(world).starting = false;
}

/// Проверка пресета (`StartPreset`): правило может отменить старт; тогда — запасные пресеты
/// или возврат в лобби.
fn start_preset(world: &mut World, ready: &[Entity], force: bool) -> bool {
    let mut attempt = RoundStartAttempt {
        players: ready.to_vec(),
        forced: force,
        cancelled: false,
    };
    world.raise_broadcast(&mut attempt);
    if !attempt.cancelled {
        return true;
    }
    if world.cvar(&GAME_FALLBACK_ENABLED) {
        let fallbacks = world.resource::<Config>().get_text(&GAME_FALLBACK_PRESET);
        for preset in fallbacks
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            clear_game_rules(world);
            set_game_preset(world, preset, false, Some(1));
            add_game_preset_rules(world);
            start_game_preset_rules(world);
            let mut retry = RoundStartAttempt {
                players: ready.to_vec(),
                forced: force,
                cancelled: false,
            };
            world.raise_broadcast(&mut retry);
            if !retry.cancelled {
                return true;
            }
        }
    }
    restart_round(world);
    delay_start(world, Duration::from_secs(30));
    false
}

/// Станции, где появляются игроки (`GetSpawnableStations`).
pub fn spawnable_stations(world: &mut World) -> Vec<Entity> {
    let mut list: Vec<Entity> = world
        .query_filtered::<Entity, (
            bevy_ecs::query::With<StationJobs>,
            bevy_ecs::query::With<StationSpawning>,
        )>()
        .iter(world)
        .collect();
    list.sort();
    list
}

/// Раздать должности и создать тела готовым игрокам (`SpawnPlayers`).
fn spawn_players(
    world: &mut World,
    ready: &[Entity],
    mut profiles: BTreeMap<NetUserId, HumanoidCharacterProfile>,
    force: bool,
) {
    world.raise_broadcast(&mut RulePlayerSpawning {
        players: ready.to_vec(),
        profiles: profiles.clone(),
        forced: force,
    });
    let users: Vec<NetUserId> = ready
        .iter()
        .filter_map(|s| session_user(world, *s))
        .collect();
    profiles.retain(|user, _| users.contains(user));
    let stations = spawnable_stations(world);
    let mut assigned = assign_jobs(world, &profiles, &stations, true);
    assign_overflow_jobs(world, &mut assigned, &users, &profiles, &stations);
    let mut counts: BTreeMap<Entity, i32> = stations.iter().map(|s| (*s, 0)).collect();
    for (job, station) in assigned.values() {
        if let (Some(_), Some(station)) = (job, station) {
            *counts.entry(*station).or_default() += 1;
        }
    }
    calc_extended_access(world, &counts);
    for (user, (job, station)) in &assigned {
        let Some(job) = job else {
            if let Some(session) = session_by_user(world, *user) {
                server_message_to(world, session, "job-not-available-wait-in-lobby", &[]);
            }
            continue;
        };
        let Some(session) = session_by_user(world, *user) else {
            continue;
        };
        let profile = profiles[user].clone();
        spawn_player_with(world, session, profile, *station, Some(job.clone()), false);
    }
    world.raise_broadcast(&mut RulePlayerJobsAssigned {
        players: ready.to_vec(),
        forced: force,
    });
}

/// Игрок входит в раунд (`SpawnPlayer`): выбранный персонаж, должность станции.
pub fn spawn_player(
    world: &mut World,
    session: Entity,
    station: Option<Entity>,
    job: Option<String>,
    late_join: bool,
) {
    let Some(user) = session_user(world, session) else {
        return;
    };
    let profile = player_profile(world, user);
    spawn_player_with(world, session, profile, station, job, late_join);
}

fn spawn_player_with(
    world: &mut World,
    session: Entity,
    profile: HumanoidCharacterProfile,
    station: Option<Entity>,
    job: Option<String>,
    late_join: bool,
) {
    if world.cvar(&GAME_DUMMY_TICKER) {
        return;
    }
    let Some(user) = session_user(world, session) else {
        return;
    };
    let station = station.or_else(|| {
        let mut stations = spawnable_stations(world);
        if stations.is_empty() {
            return None;
        }
        let index = crate::random::random(world).range(0, stations.len() as i32) as usize;
        Some(stations.swap_remove(index))
    });
    if late_join && ticker(world).disallow_late_join {
        join_as_observer(world, session);
        return;
    }
    let job = job.or_else(|| {
        station.and_then(|station| {
            pick_best_available_job(
                world,
                station,
                &profile.job_priorities,
                true,
                &BTreeSet::new(),
            )
        })
    });
    let Some(job) = job else {
        if !lobby_enabled(world) {
            join_as_observer(world, session);
        }
        server_message_to(
            world,
            session,
            "game-ticker-player-no-jobs-available-when-joining",
            &[],
        );
        return;
    };
    player_join_game(world, session);
    let in_round = run_level(world) == GameRunLevel::InRound;
    let mob = match world
        .get_resource::<crate::server::ArrivalPoint>()
        .copied()
        .and_then(|crate::server::ArrivalPoint(point)| world.snap_to_grid_or_map(point))
    {
        Some(at) => crate::station_spawning::spawn_player_mob(
            world,
            at,
            Some(&job),
            Some(&profile),
            station,
        ),
        None => crate::station_spawning::spawn_player_character_on_station(
            world,
            station,
            Some(&job),
            Some(&profile),
            in_round,
        ),
    };
    let Some(mob) = mob else {
        eprintln!("игрок {user} не получил тело");
        return;
    };
    // Новый разум с именем тела переносится в тело (`CreateMind`, `TransferTo`).
    let character = world.entity_name(mob);
    let new_mind = crate::mind::create_mind(world, Some(user), Some(&character));
    crate::mind::transfer_to(world, new_mind, Some(mob), false, true);
    let name = world
        .get::<Session>(session)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    {
        let mut ticker = ticker(world);
        ticker.player_names.insert(user, name.clone());
        ticker.players_joined_normally += 1;
    }
    if let Some(station) = station {
        crate::stations::try_assign_job(world, station, &job, user);
    }
    let (log, text) = if late_join {
        (LogType::LateJoin, "late joined")
    } else {
        (LogType::RoundStartJoin, "joined")
    };
    let station_name = station.map(|s| entity_name(world, s)).unwrap_or_default();
    add_admin_log(
        world,
        log,
        LogImpact::Medium,
        &format!(
            "Player {name} {text} as {} on station {station_name} as a {job}.",
            profile.name
        ),
        &[],
        &[user],
    );
    // Объявление о прибытии и приветствие (`SpawnPlayer`).
    let (job_name, notify_crew, has_extended) = {
        let node = world
            .get_resource::<Prototypes>()
            .and_then(|p| p.resolved_node("job", &job));
        let key = node
            .and_then(|n| n.get("name"))
            .and_then(DataNode::as_str)
            .map(str::to_owned);
        let notify = node
            .and_then(|n| n.get("joinNotifyCrew"))
            .and_then(DataNode::as_str)
            .is_some_and(|t| t == "true");
        let extended = node.is_some_and(|n| {
            !strings(n.get("extendedAccess")).is_empty()
                || !strings(n.get("extendedAccessGroups")).is_empty()
        });
        (key, notify, extended)
    };
    let job_name =
        title_case(&job_name.map_or_else(|| job.clone(), |k| crate::examine::loc(world, &k, &[])));
    if late_join && station.is_some() {
        let character = LocArg::from(astrocore_loc::EntityNameWorldExt::entity_arg(world, mob));
        let character_name = world
            .get::<MetaData>(mob)
            .and_then(|m| m.name.clone())
            .unwrap_or_default();
        let key = if notify_crew {
            "latejoin-arrival-announcement-special"
        } else {
            "latejoin-arrival-announcement"
        };
        let text = crate::examine::loc(
            world,
            key,
            &[
                ("character", LocArg::from(character_name.as_str())),
                ("entity", character),
                ("job", LocArg::from(job_name.as_str())),
            ],
        );
        let sender = crate::examine::loc(world, "latejoin-arrival-sender", &[]);
        let color = notify_crew.then(|| "#FFD700".to_owned());
        crate::chat::dispatch_station_announcement(world, mob, &text, Some(&sender), false, color);
    }
    if let Some(station) = station {
        let extended = world
            .get::<StationJobs>(station)
            .is_some_and(|j| j.extended_access);
        if extended && has_extended {
            server_message_to(world, session, "job-greet-crew-shortages", &[]);
        }
        let station_name = entity_name(world, station);
        server_message_to(
            world,
            session,
            "job-greet-station-name",
            &[("stationName", LocArg::from(station_name.as_str()))],
        );
    }
    let joined = ticker(world).players_joined_normally;
    world.raise_broadcast(&mut PlayerSpawnComplete {
        mob,
        session,
        job: Some(job),
        late_join,
        station,
        joined_player_count: joined,
    });
}

/// Вход в идущий раунд по просьбе игрока (`MakeJoinGame`).
pub fn make_join_game(
    world: &mut World,
    session: Entity,
    station: Option<Entity>,
    job: Option<String>,
) {
    let Some(user) = session_user(world, session) else {
        return;
    };
    if !ticker(world).statuses.contains_key(&user)
        || world.resource::<PreferencesCache>().get(user).is_none()
    {
        return;
    }
    spawn_player(world, session, station, job, true);
}

/// Точка появления наблюдателя (`GetObserverSpawnPoint`): точки наблюдателей, иначе центр
/// грида, иначе центр карты раунда.
pub fn observer_spawn_point(world: &mut World) -> Option<EntityCoordinates> {
    let points: Vec<Entity> = world
        .query::<(Entity, &SpawnPoint)>()
        .iter(world)
        .filter(|(_, p)| p.spawn_type == SpawnPointType::Observer)
        .map(|(e, _)| e)
        .collect();
    let mut positions: Vec<EntityCoordinates> = points
        .iter()
        .filter_map(|e| world.coordinates(*e))
        .collect();
    if positions.is_empty() {
        let grids: Vec<Entity> = world
            .query_filtered::<Entity, bevy_ecs::query::With<MapGrid>>()
            .iter(world)
            .collect();
        positions = grids
            .into_iter()
            .map(|g| EntityCoordinates::new(g, astrocore_math::Vec2::ZERO))
            .collect();
    }
    if !positions.is_empty() {
        positions.sort_by_key(|c| c.parent);
        let index = crate::random::random(world).range(0, positions.len() as i32) as usize;
        return Some(positions[index]);
    }
    let map = ticker(world)
        .default_map
        .and_then(|m| world.map_entity(m))
        .or_else(|| {
            world
                .query_filtered::<Entity, bevy_ecs::query::With<Map>>()
                .iter(world)
                .next()
        })?;
    Some(EntityCoordinates::new(map, astrocore_math::Vec2::ZERO))
}

/// Наблюдать (`JoinAsObserver`): игрок в игре, тело — наблюдатель.
pub fn join_as_observer(world: &mut World, session: Entity) {
    if world.cvar(&GAME_DUMMY_TICKER) {
        return;
    }
    let Some(user) = session_user(world, session) else {
        return;
    };
    player_join_game(world, session);
    spawn_observer(world, session, user);
}

/// Призрак-наблюдатель (`SpawnObserver`): без разума — новый разум с ролью наблюдателя,
/// иначе разум переносится в призрака.
pub fn spawn_observer(world: &mut World, session: Entity, user: NetUserId) {
    if world.cvar(&GAME_DUMMY_TICKER) {
        return;
    }
    let mind = match crate::mind::mind_of_user(world, user) {
        Some(mind) => mind,
        None => {
            let character = player_profile(world, user).name;
            let mind = crate::mind::create_mind(world, Some(user), Some(&character));
            if let Some(mut state) = world.get_mut::<crate::mind::Mind>(mind) {
                state.observer = true;
            }
            mind
        }
    };
    let at = observer_spawn_point(world);
    let ghost = crate::ghost::spawn_ghost(world, mind, at, false);
    let name = world
        .get::<Session>(session)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    ticker(world).player_names.insert(user, name.clone());
    let ghost = ghost.map(|g| world.entity_name(g)).unwrap_or_default();
    add_admin_log(
        world,
        LogType::LateJoin,
        LogImpact::Low,
        &format!("{name} late joined the round as an Observer with {ghost}."),
        &[],
        &[],
    );
}

/// Заново (`Respawn`): тело забыто, игрок — в лобби или сразу в раунд.
pub fn respawn(world: &mut World, session: Entity) {
    let Some(user) = session_user(world, session) else {
        return;
    };
    crate::mind::wipe_user_mind(world, user);
    add_admin_log(
        world,
        LogType::Respawn,
        LogImpact::Medium,
        &format!("Player {user} was respawned."),
        &[],
        &[],
    );
    if lobby_enabled(world) {
        player_join_lobby(world, session);
    } else {
        spawn_player(world, session, None, None, true);
    }
}

/// Закончить раунд (`EndRound`): итоги и перезапуск через `game.round_restart_time`.
pub fn end_round(world: &mut World, text: &str) {
    if world.cvar(&GAME_DUMMY_TICKER) || run_level(world) != GameRunLevel::InRound {
        return;
    }
    set_run_level(world, GameRunLevel::PostRound);
    show_round_end_scoreboard(world, text);
    let delay = world.cvar(&ROUND_RESTART_TIME).max(0.0);
    let at = now(world) + Duration::from_secs_f32(delay);
    ticker(world).restart_at = Some(at);
}

/// Должность игрока для итогов.
pub(crate) fn job_title(world: &mut World, user: NetUserId) -> Option<String> {
    let stations = spawnable_stations(world);
    let job = stations.iter().find_map(|s| {
        world
            .get::<StationJobs>(*s)?
            .player_jobs
            .get(&user)?
            .first()
            .cloned()
    })?;
    let key = world
        .get_resource::<Prototypes>()?
        .resolved_node("job", &job)?
        .get("name")?
        .as_str()?
        .to_owned();
    Some(crate::examine::loc(world, &key, &[]))
}

/// Итоги раунда (`ShowRoundEndScoreboard`).
pub fn show_round_end_scoreboard(world: &mut World, text: &str) {
    add_admin_log(
        world,
        LogType::EmergencyShuttle,
        LogImpact::High,
        "Round ended, showing summary",
        &[],
        &[],
    );
    let preset = {
        let ticker = ticker(world);
        ticker.current_preset.clone()
    };
    let gamemode_title = preset_of(world, preset.as_deref())
        .map(|p| crate::examine::loc(world, &p.name, &[]))
        .unwrap_or_default();
    let mut append = RoundEndTextAppend::default();
    world.raise_broadcast(&mut append);
    let round_end_text = format!("{text}\n{}", append.text);
    let online: BTreeSet<NetUserId> = sessions(world).into_iter().map(|(_, u)| u).collect();
    // Все разумы раунда, и вышедших игроков тоже.
    let minds: Vec<crate::mind::Mind> = world
        .query::<&crate::mind::Mind>()
        .iter(world)
        .cloned()
        .collect();
    let unknown = crate::examine::loc(world, "game-ticker-unknown-role", &[]);
    let mut players: Vec<RoundEndPlayerInfo> = Vec::new();
    for mind in minds {
        let Some(user) = mind.user_id.or(mind.original_owner_user_id) else {
            continue;
        };
        let ic_name = mind
            .character_name
            .clone()
            .or_else(|| mind.current_entity().map(|e| world.entity_name(e)))
            .or_else(|| Some("Unknown".to_owned()));
        let ooc_name = ticker(world)
            .player_names
            .get(&user)
            .cloned()
            .unwrap_or_else(|| "(IMPOSSIBLE: REGISTERED MIND WITH NO OWNER)".to_owned());
        players.push(RoundEndPlayerInfo {
            ooc_name,
            ic_name,
            role: job_title(world, user).unwrap_or_else(|| unknown.clone()),
            antag: false,
            observer: mind.observer,
            connected: online.contains(&user),
        });
    }
    players.sort_by(|a, b| a.ooc_name.cmp(&b.ooc_name));
    let message = RoundEndMessage {
        gamemode_title,
        round_end_text,
        round_duration: round_duration(world).as_secs_f32(),
        round_id: ticker(world).round_id,
        players,
    };
    world.send_net_event(NetTarget::All, &TickerMessage::RoundEnd(Box::new(message)));
}

/// Удалить все сущности раунда (`FlushEntities`): всё с `MetaData`, сессии остаются.
fn flush_entities(world: &mut World) {
    let entities: Vec<Entity> = world
        .query_filtered::<Entity, bevy_ecs::query::With<MetaData>>()
        .iter(world)
        .collect();
    for entity in entities {
        if world.get_entity(entity).is_ok() && world.get::<Session>(entity).is_none() {
            world.delete_entity(entity);
        }
    }
}

/// Очистка перед новым раундом (`ResettingCleanup`).
fn resetting_cleanup(world: &mut World) {
    for (session, _) in sessions(world) {
        world.detach_player(session);
        player_join_lobby(world, session);
    }
    world.raise_broadcast(&mut RoundRestartCleanup);
    world.send_net_event(NetTarget::All, &TickerMessage::RestartCleanup);
    flush_entities(world);
    let lobby = lobby_enabled(world);
    let users: Vec<NetUserId> = sessions(world).into_iter().map(|(_, u)| u).collect();
    let mut ticker = ticker(world);
    ticker.default_map = None;
    ticker.current_preset = None;
    ticker.rules.clear();
    ticker.disallow_late_join = false;
    ticker.player_names.clear();
    ticker.statuses.clear();
    ticker.restart_at = None;
    for user in users {
        ticker.statuses.insert(
            user,
            if lobby {
                PlayerGameStatus::NotReadyToPlay
            } else {
                PlayerGameStatus::ReadyToPlay
            },
        );
    }
}

/// Новый раунд (`RestartRound`): лобби, очистка, новый номер; без лобби — сразу старт.
pub fn restart_round(world: &mut World) {
    if world.cvar(&GAME_DUMMY_TICKER) {
        return;
    }
    try_reset_preset(world);
    send_server_message(world, "game-ticker-restart-round", &[]);
    ticker(world).players_joined_normally = 0;
    set_run_level(world, GameRunLevel::PreRoundLobby);
    resetting_cleanup(world);
    increment_round_number(world);
    if !lobby_enabled(world) {
        start_round(world, false);
        return;
    }
    if sessions(world).is_empty() {
        ticker(world).waiting_for_players = true;
        ticker(world).round_start_time = None;
    } else {
        let duration = Duration::from_secs(world.cvar(&GAME_LOBBY_DURATION).max(0) as u64);
        let start = now(world) + duration;
        ticker(world).round_start_time = Some(start);
    }
    send_status_to_all(world);
    update_info_text(world);
}

/// Ход раунда каждый такт (`UpdateRoundFlow`, `UpdateGameRules`): старт по отсчёту,
/// заранее — карта, после итогов — перезапуск; свободные должности — игрокам.
fn update(world: &mut World) {
    if world.cvar(&GAME_DUMMY_TICKER) || !world.contains_resource::<GameTicker>() {
        return;
    }
    let time = now(world);
    let (level, start, paused, waiting, restart_at) = {
        let ticker = ticker(world);
        (
            ticker.run_level,
            ticker.round_start_time,
            ticker.paused,
            ticker.waiting_for_players,
            ticker.restart_at,
        )
    };
    if level == GameRunLevel::PreRoundLobby
        && !paused
        && !waiting
        && let Some(start) = start
    {
        if start <= time {
            start_round(world, false);
        } else if start.saturating_sub(ROUND_PRELOAD_TIME) <= time {
            load_maps(world);
        }
    }
    if level == GameRunLevel::PostRound && restart_at.is_some_and(|at| at <= time) {
        restart_round(world);
    }
    update_game_rules(world);
    send_jobs_available(world);
}

/// Свободные должности всем, если изменились (`ServerStationJobsSystem.Update`).
fn send_jobs_available(world: &mut World) {
    let dirty = world
        .get_resource::<crate::stations::JobsAvailableDirty>()
        .is_some_and(|d| d.0);
    if !dirty {
        return;
    }
    world
        .resource_mut::<crate::stations::JobsAvailableDirty>()
        .0 = false;
    let jobs = crate::stations::jobs_available(world);
    world.send_net_event(NetTarget::All, &jobs);
}

/// Игрок с загруженными настройками (`PlayerStatusChanged` → `InGame`): прежнее тело,
/// иначе лобби или сразу раунд.
fn player_ready_to_play(world: &mut World, session: Entity, user: NetUserId) {
    let mind = crate::mind::mind_of_user(world, user);
    let current = mind
        .and_then(|m| world.get::<crate::mind::Mind>(m)?.current_entity())
        .filter(|e| world.is_alive(*e));
    match (mind, current) {
        (Some(_), Some(entity)) => {
            world.attach_player(session, entity);
            player_join_game(world, session);
        }
        // Разум без сущности: тело удалено или игрок наблюдал — наблюдатель.
        (Some(_), None) => join_as_observer(world, session),
        (None, _) if lobby_enabled(world) => player_join_lobby(world, session),
        (None, _) => {
            if run_level(world) == GameRunLevel::PreRoundLobby {
                player_join_lobby(world, session);
            } else {
                spawn_player(world, session, None, None, true);
            }
        }
    }
    update_info_text(world);
}

fn receive_request(world: &mut World, event: &mut NetEvent<LobbyRequest>) {
    let Some(session) = event.session else {
        return;
    };
    match event.event.clone() {
        LobbyRequest::ToggleReady(ready) => toggle_ready(world, session, ready),
        LobbyRequest::JoinGame { station, job } => {
            if run_level(world) == GameRunLevel::PreRoundLobby {
                return;
            }
            let station = station.and_then(|bits| station_from_bits(world, bits));
            make_join_game(world, session, station, job);
        }
        LobbyRequest::Observe => {
            let Some(user) = session_user(world, session) else {
                return;
            };
            if run_level(world) != GameRunLevel::PreRoundLobby && !user_has_joined_game(world, user)
            {
                join_as_observer(world, session);
            }
        }
    }
}

// Команды.

fn command(
    name: &'static str,
    description: &'static str,
    help: &'static str,
    execute: astrocore_console::CommandFn,
) -> ConsoleCommand {
    ConsoleCommand {
        name,
        description,
        help,
        execute,
        complete: None,
    }
}

fn parse_bool(text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn commands() -> Vec<ConsoleCommand> {
    vec![
        command(
            "startround",
            "Начинает раунд сейчас.",
            "startround",
            |world, shell, _| {
                if run_level(world) != GameRunLevel::PreRoundLobby {
                    shell.write_error("раунд уже идёт");
                    return;
                }
                start_round(world, false);
            },
        ),
        command(
            "endround",
            "Заканчивает раунд.",
            "endround",
            |world, shell, _| {
                if run_level(world) != GameRunLevel::InRound {
                    shell.write_error("раунд не идёт");
                    return;
                }
                end_round(world, "");
            },
        ),
        command(
            "restartround",
            "Заканчивает раунд и перезапускает его.",
            "restartround",
            |world, shell, _| {
                if run_level(world) != GameRunLevel::InRound {
                    shell.write_error("раунд не идёт");
                    return;
                }
                end_round(world, "");
            },
        ),
        command(
            "restartroundnow",
            "Сразу перезапускает раунд.",
            "restartroundnow",
            |world, _, _| restart_round(world),
        ),
        command(
            "delaystart",
            "Откладывает старт раунда или ставит отсчёт на паузу.",
            "delaystart [секунды]",
            |world, shell, args| {
                if run_level(world) != GameRunLevel::PreRoundLobby {
                    shell.write_error("раунд уже начался");
                    return;
                }
                match args.first() {
                    None => {
                        let paused = !ticker(world).paused;
                        pause_start(world, paused);
                        shell.write_line(if paused {
                            "отсчёт на паузе"
                        } else {
                            "отсчёт продолжен"
                        });
                    }
                    Some(text) => match text.parse::<u64>() {
                        Ok(seconds) if seconds > 0 => {
                            delay_start(world, Duration::from_secs(seconds));
                            shell.write_line(format!("старт отложен на {seconds} с"));
                        }
                        _ => shell.write_error("нужно положительное число секунд"),
                    },
                }
            },
        ),
        command(
            "golobby",
            "Возвращает всех в лобби, можно с новым пресетом.",
            "golobby [пресет]",
            |world, shell, args| {
                if let Some(preset) = args.first()
                    && !set_game_preset(world, preset, false, None)
                {
                    shell.write_error(format!("нет пресета {preset}"));
                    return;
                }
                world.set_cvar(&GAME_LOBBY_ENABLED, true).ok();
                restart_round(world);
            },
        ),
        command(
            "setgamepreset",
            "Выбирает пресет режима на следующие раунды.",
            "setgamepreset <пресет> [раундов]",
            |world, shell, args| {
                let Some(preset) = args.first() else {
                    shell.write_error("setgamepreset <пресет> [раундов]");
                    return;
                };
                let delay = args.get(1).and_then(|t| t.parse().ok());
                if !set_game_preset(world, preset, false, delay) {
                    shell.write_error(format!("нет пресета {preset}"));
                }
            },
        ),
        command(
            "forcepreset",
            "Сразу начинает раунд с пресетом.",
            "forcepreset <пресет>",
            |world, shell, args| {
                if run_level(world) != GameRunLevel::PreRoundLobby {
                    shell.write_error("раунд уже идёт");
                    return;
                }
                let Some(preset) = args.first() else {
                    shell.write_error("forcepreset <пресет>");
                    return;
                };
                if !set_game_preset(world, preset, true, None) {
                    shell.write_error(format!("нет пресета {preset}"));
                }
            },
        ),
        command(
            "toggledisallowlatejoin",
            "Запрещает или разрешает поздний вход.",
            "toggledisallowlatejoin <true|false>",
            |world, shell, args| {
                let Some(value) = args.first().and_then(|t| parse_bool(t)) else {
                    shell.write_error("toggledisallowlatejoin <true|false>");
                    return;
                };
                world
                    .set_cvar(&crate::stations::GAME_DISALLOW_LATE_JOINS, value)
                    .ok();
                ticker(world).disallow_late_join = value;
                update_late_join_status(world);
            },
        ),
        command(
            "toggleready",
            "Готов или не готов к раунду.",
            "toggleready <true|false>",
            |world, shell, args| {
                let Some(session) = shell.player else {
                    shell.write_error("команду выполняет игрок");
                    return;
                };
                let Some(ready) = args.first().and_then(|t| parse_bool(t)) else {
                    shell.write_error("toggleready <true|false>");
                    return;
                };
                toggle_ready(world, session, ready);
            },
        ),
        command(
            "joingame",
            "Войти в идущий раунд на должность.",
            "joingame <должность> [станция]",
            |world, shell, args| {
                let Some(session) = shell.player else {
                    shell.write_error("команду выполняет игрок");
                    return;
                };
                if run_level(world) == GameRunLevel::PreRoundLobby {
                    shell.write_error("раунд ещё не начался");
                    return;
                }
                let job = args.first().cloned();
                let station = args
                    .get(1)
                    .and_then(|t| t.parse().ok())
                    .and_then(|bits| station_from_bits(world, bits));
                make_join_game(world, session, station, job);
            },
        ),
        command(
            "observe",
            "Наблюдать за раундом.",
            "observe",
            |world, shell, _| {
                let Some(session) = shell.player else {
                    shell.write_error("команду выполняет игрок");
                    return;
                };
                if run_level(world) == GameRunLevel::PreRoundLobby {
                    shell.write_error("раунд ещё не начался");
                    return;
                }
                join_as_observer(world, session);
            },
        ),
        command(
            "respawn",
            "Вернуть игрока в лобби заново.",
            "respawn [игрок]",
            |world, shell, args| {
                let session = match args.first() {
                    Some(name) => sessions(world)
                        .into_iter()
                        .map(|(e, _)| e)
                        .find(|e| world.get::<Session>(*e).is_some_and(|s| &s.name == name)),
                    None => shell.player,
                };
                match session {
                    Some(session) => respawn(world, session),
                    None => shell.write_error("нет такого игрока"),
                }
            },
        ),
    ]
}

// Клиент.

/// Состояние лобби на клиенте: из сообщений раунда.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct ClientLobby {
    /// Игрок в лобби (`TickerJoinLobbyEvent`), а не в игре.
    pub in_lobby: bool,
    pub round_started: bool,
    pub ready: bool,
    /// Когда начнётся раунд по часам клиента.
    pub start_at: Option<Duration>,
    pub paused: bool,
    /// Длительность раунда на момент `round_duration_at`.
    pub round_duration: f32,
    pub round_duration_at: Duration,
    pub info: String,
    pub late_join_disallowed: bool,
    /// Последние итоги и показывать ли их.
    pub round_end: Option<RoundEndMessage>,
    pub show_round_end: bool,
    pub version: u64,
}

impl ClientLobby {
    /// Секунды до старта сейчас; `None` — отсчёта нет.
    pub fn seconds_left(&self, now: Duration) -> Option<f32> {
        self.start_at.map(|at| at.saturating_sub(now).as_secs_f32())
    }

    /// Длительность раунда сейчас.
    pub fn round_time(&self, now: Duration) -> f32 {
        self.round_duration + now.saturating_sub(self.round_duration_at).as_secs_f32()
    }
}

/// Клиент применяет сообщение раунда.
pub fn apply_ticker_message(world: &mut World, message: &TickerMessage) {
    let time = now(world);
    let mut lobby = world.get_resource_or_insert_with(ClientLobby::default);
    match message {
        TickerMessage::JoinLobby => lobby.in_lobby = true,
        TickerMessage::JoinGame => lobby.in_lobby = false,
        TickerMessage::LobbyStatus {
            is_round_started,
            you_are_ready,
            seconds_until_start,
            paused,
            round_duration,
        } => {
            lobby.round_started = *is_round_started;
            lobby.ready = *you_are_ready;
            lobby.start_at = seconds_until_start.map(|s| time + Duration::from_secs_f32(s));
            lobby.paused = *paused;
            lobby.round_duration = *round_duration;
            lobby.round_duration_at = time;
        }
        TickerMessage::LobbyInfo(text) => lobby.info = text.clone(),
        TickerMessage::LobbyCountdown {
            seconds_until_start,
            paused,
        } => {
            lobby.start_at = seconds_until_start.map(|s| time + Duration::from_secs_f32(s));
            lobby.paused = *paused;
        }
        TickerMessage::LateJoinStatus { disallowed } => lobby.late_join_disallowed = *disallowed,
        TickerMessage::RoundEnd(end) => {
            lobby.round_end = Some((**end).clone());
            lobby.show_round_end = true;
        }
        TickerMessage::RestartCleanup => {
            lobby.round_started = false;
            lobby.ready = false;
        }
    }
    lobby.version += 1;
}

pub fn install(world: &mut World, side: Side) {
    world.register_cvar(&GAME_LOBBY_ENABLED);
    world.register_cvar(&GAME_LOBBY_DURATION);
    world.register_cvar(&GAME_DEFAULT_PRESET);
    world.register_cvar(&GAME_FALLBACK_ENABLED);
    world.register_cvar(&GAME_FALLBACK_PRESET);
    world.register_cvar(&GAME_IGNORED_PRESETS);
    world.register_cvar(&GAME_DUMMY_TICKER);
    world.register_cvar(&ROUND_RESTART_TIME);
    world.register_cvar(&IC_RANDOM_CHARACTERS);
    world.register_data_component::<GameRule>();
    world.register_net_event::<TickerMessage>();
    world.register_net_event::<LobbyRequest>();
    if !side.is_server() {
        world.init_resource::<ClientLobby>();
        world.subscribe_broadcast::<NetEvent<TickerMessage>>(|world, event| {
            let message = event.event.clone();
            apply_ticker_message(world, &message);
        });
        return;
    }
    world.init_resource::<GameTicker>();
    world.add_game_systems(Update, update);
    world.subscribe_net_event::<LobbyRequest>(receive_request);
    world.subscribe_broadcast::<PreferencesLoaded>(|world, event| {
        player_ready_to_play(world, event.session, event.user_id);
    });
    world.subscribe_broadcast::<PlayerConnected>(|world, event| {
        if let Some(name) = world.get::<Session>(event.session).map(|s| s.name.clone()) {
            let text = crate::examine::loc(
                world,
                "player-join-message",
                &[("name", LocArg::from(name.as_str()))],
            );
            crate::chat::send_admin_announcement(world, &text);
        }
        let start = {
            let ticker = ticker(world);
            ticker.waiting_for_players
        };
        if lobby_enabled(world) && start {
            let duration = Duration::from_secs(world.cvar(&GAME_LOBBY_DURATION).max(0) as u64);
            let at = now(world) + duration;
            let mut ticker = ticker(world);
            ticker.waiting_for_players = false;
            ticker.round_start_time = Some(at);
        }
    });
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        if let Some(name) = world.get::<Session>(event.session).map(|s| s.name.clone()) {
            let text = crate::examine::loc(
                world,
                "player-leave-message",
                &[("name", LocArg::from(name.as_str()))],
            );
            crate::chat::send_admin_announcement(world, &text);
        }
        update_info_text(world);
    });
    world.subscribe_broadcast::<astrocore_console::CVarChanged>(|world, event| {
        if event.name == crate::stations::GAME_DISALLOW_LATE_JOINS.name {
            let value = world.cvar(&crate::stations::GAME_DISALLOW_LATE_JOINS);
            ticker(world).disallow_late_join = value;
            update_late_join_status(world);
        }
    });
    for command in commands() {
        world.register_command(command);
    }
}

/// Первый раунд при запуске сервера (`PostInitialize` → `RestartRound`).
pub fn post_initialize(world: &mut World) {
    initialize_game_preset(world);
    restart_round(world);
}

#[cfg(test)]
mod tests;
