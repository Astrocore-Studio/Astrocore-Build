//! Журнал действий для администраторов: запись — вид, важность, время, текст, значения (JSON)
//! и игроки, которых она касается. Записи копятся в очереди и уходят в базу пачкой раз в
//! несколько секунд или при заполнении очереди; до начала раунда ждут его номера; при
//! переполнении отбрасываются; если база не приняла — возвращаются в очередь.
//! Образец — `AdminLogManager` (`Add`, `Update`, `SaveLogs`, `RoundStarting`,
//! `RunLevelChanged`), `LogType`, `LogImpact`, `LogFilter`, `AdminLog`, `AdminLogPlayer`,
//! `ServerDbBase.AddAdminLogs`/`GetAdminLogs` SS14 @ 2f4a2aad.
//!
//! Нет пока: окна журнала (`AdminLogsEui`) и кэша последних записей, оповещений
//! администраторов о важных записях новичков, координат в записях, записи очереди при
//! остановке сервера.

use std::time::{Duration, Instant};

use astrocore_console::{CVarDef, CVarFlags, ConfigWorldExt};
use astrocore_ecs::{EventWorldExt, GameWorldExt, Update};
use astrocore_net::{NetUserId, PlayerConnected, PlayerDisconnected, Session};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use sqlx::Row;

use crate::Side;
use crate::database::{Db, DbResult, db_error, now_ms, parse_user_id, spawn_db};

pub const ADMIN_LOGS_ENABLED: CVarDef<bool> = CVarDef::new(
    "adminlogs.enabled",
    true,
    CVarFlags::SERVERONLY,
    "Вести журнал действий для администраторов.",
);
pub const ADMIN_LOGS_QUEUE_SEND_DELAY: CVarDef<f32> = CVarDef::new(
    "adminlogs.queue_send_delay_seconds",
    5.0,
    CVarFlags::SERVERONLY,
    "Раз в сколько секунд записи журнала уходят в базу.",
);
pub const ADMIN_LOGS_QUEUE_MAX: CVarDef<i32> = CVarDef::new(
    "adminlogs.queue_max",
    5000,
    CVarFlags::SERVERONLY,
    "Сколько записей в очереди отправляет их в базу раньше срока.",
);
pub const ADMIN_LOGS_DROP_THRESHOLD: CVarDef<i32> = CVarDef::new(
    "adminlogs.drop_threshold",
    20000,
    CVarFlags::SERVERONLY,
    "Сверх скольких записей в очереди новые отбрасываются.",
);

/// Вид записи журнала (`LogType`): номера как в SS14 — они хранятся в базе.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LogType {
    Unknown = 0,
    Damaged = 2,
    Healed = 3,
    Slip = 4,
    EventAnnounced = 5,
    EventStarted = 6,
    EventRan = 16,
    EventStopped = 7,
    Verb = 19,
    ShuttleCalled = 8,
    ShuttleRecalled = 9,
    ExplosiveDepressurization = 10,
    Respawn = 13,
    RoundStartJoin = 14,
    LateJoin = 15,
    ChemicalReaction = 17,
    EntityEffect = 18,
    CanisterValve = 20,
    CanisterPressure = 21,
    CanisterPurged = 22,
    CanisterTankEjected = 23,
    CanisterTankInserted = 24,
    DisarmedAction = 25,
    DisarmedKnockdown = 26,
    AttackArmedClick = 27,
    AttackArmedWide = 28,
    AttackUnarmedClick = 29,
    AttackUnarmedWide = 30,
    InteractHand = 31,
    InteractActivate = 32,
    Throw = 33,
    Landed = 34,
    ThrowHit = 35,
    Pickup = 36,
    Drop = 37,
    BulletHit = 38,
    ForceFeed = 40,
    Ingestion = 53,
    MeleeHit = 41,
    HitScanHit = 42,
    Mind = 43,
    Explosion = 44,
    Radiation = 45,
    Barotrauma = 46,
    Flammable = 47,
    Asphyxiation = 48,
    Temperature = 49,
    Hunger = 50,
    Thirst = 51,
    Electrocution = 52,
    CrayonDraw = 39,
    AtmosPressureChanged = 54,
    AtmosPowerChanged = 55,
    AtmosVolumeChanged = 56,
    AtmosFilterChanged = 57,
    AtmosRatioChanged = 58,
    FieldGeneration = 59,
    GhostRoleTaken = 60,
    Chat = 61,
    Action = 62,
    RCD = 63,
    Construction = 64,
    Trigger = 65,
    Anchor = 66,
    Unanchor = 67,
    EmergencyShuttle = 68,
    Emag = 69,
    Gib = 70,
    Identity = 71,
    CableCut = 72,
    StorePurchase = 73,
    LatticeCut = 74,
    Stripping = 75,
    Stamina = 76,
    EntitySpawn = 77,
    AdminMessage = 78,
    Anomaly = 79,
    WireHacking = 80,
    Teleport = 81,
    EntityDelete = 82,
    Vote = 83,
    ItemConfigure = 84,
    DeviceLinking = 85,
    Tile = 86,
    ChatRateLimited = 87,
    AtmosTemperatureChanged = 88,
    DeviceNetwork = 89,
    StoreRefund = 90,
    RateLimited = 91,
    InteractUsing = 92,
    Storage = 93,
    ExplosionHit = 94,
    GhostWarp = 95,
    PdaInteract = 96,
    AtmosDeviceSetting = 97,
    AdminCommands = 98,
    AntagSelection = 99,
    Botany = 100,
    ArtifactNode = 101,
    ShuttleImpact = 102,
    Instrument = 103,
    Connection = 104,
    SiliconLaw = 105,
}

impl LogType {
    /// Все виды.
    pub const ALL: &[LogType] = &[
        LogType::Unknown,
        LogType::Damaged,
        LogType::Healed,
        LogType::Slip,
        LogType::EventAnnounced,
        LogType::EventStarted,
        LogType::EventRan,
        LogType::EventStopped,
        LogType::Verb,
        LogType::ShuttleCalled,
        LogType::ShuttleRecalled,
        LogType::ExplosiveDepressurization,
        LogType::Respawn,
        LogType::RoundStartJoin,
        LogType::LateJoin,
        LogType::ChemicalReaction,
        LogType::EntityEffect,
        LogType::CanisterValve,
        LogType::CanisterPressure,
        LogType::CanisterPurged,
        LogType::CanisterTankEjected,
        LogType::CanisterTankInserted,
        LogType::DisarmedAction,
        LogType::DisarmedKnockdown,
        LogType::AttackArmedClick,
        LogType::AttackArmedWide,
        LogType::AttackUnarmedClick,
        LogType::AttackUnarmedWide,
        LogType::InteractHand,
        LogType::InteractActivate,
        LogType::Throw,
        LogType::Landed,
        LogType::ThrowHit,
        LogType::Pickup,
        LogType::Drop,
        LogType::BulletHit,
        LogType::ForceFeed,
        LogType::Ingestion,
        LogType::MeleeHit,
        LogType::HitScanHit,
        LogType::Mind,
        LogType::Explosion,
        LogType::Radiation,
        LogType::Barotrauma,
        LogType::Flammable,
        LogType::Asphyxiation,
        LogType::Temperature,
        LogType::Hunger,
        LogType::Thirst,
        LogType::Electrocution,
        LogType::CrayonDraw,
        LogType::AtmosPressureChanged,
        LogType::AtmosPowerChanged,
        LogType::AtmosVolumeChanged,
        LogType::AtmosFilterChanged,
        LogType::AtmosRatioChanged,
        LogType::FieldGeneration,
        LogType::GhostRoleTaken,
        LogType::Chat,
        LogType::Action,
        LogType::RCD,
        LogType::Construction,
        LogType::Trigger,
        LogType::Anchor,
        LogType::Unanchor,
        LogType::EmergencyShuttle,
        LogType::Emag,
        LogType::Gib,
        LogType::Identity,
        LogType::CableCut,
        LogType::StorePurchase,
        LogType::LatticeCut,
        LogType::Stripping,
        LogType::Stamina,
        LogType::EntitySpawn,
        LogType::AdminMessage,
        LogType::Anomaly,
        LogType::WireHacking,
        LogType::Teleport,
        LogType::EntityDelete,
        LogType::Vote,
        LogType::ItemConfigure,
        LogType::DeviceLinking,
        LogType::Tile,
        LogType::ChatRateLimited,
        LogType::AtmosTemperatureChanged,
        LogType::DeviceNetwork,
        LogType::StoreRefund,
        LogType::RateLimited,
        LogType::InteractUsing,
        LogType::Storage,
        LogType::ExplosionHit,
        LogType::GhostWarp,
        LogType::PdaInteract,
        LogType::AtmosDeviceSetting,
        LogType::AdminCommands,
        LogType::AntagSelection,
        LogType::Botany,
        LogType::ArtifactNode,
        LogType::ShuttleImpact,
        LogType::Instrument,
        LogType::Connection,
        LogType::SiliconLaw,
    ];

    /// Вид по номеру из базы.
    pub fn from_i64(value: i64) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| *t as i64 == value)
    }

    /// Имя вида, как в SS14.
    pub fn name(self) -> &'static str {
        match self {
            LogType::Unknown => "Unknown",
            LogType::Damaged => "Damaged",
            LogType::Healed => "Healed",
            LogType::Slip => "Slip",
            LogType::EventAnnounced => "EventAnnounced",
            LogType::EventStarted => "EventStarted",
            LogType::EventRan => "EventRan",
            LogType::EventStopped => "EventStopped",
            LogType::Verb => "Verb",
            LogType::ShuttleCalled => "ShuttleCalled",
            LogType::ShuttleRecalled => "ShuttleRecalled",
            LogType::ExplosiveDepressurization => "ExplosiveDepressurization",
            LogType::Respawn => "Respawn",
            LogType::RoundStartJoin => "RoundStartJoin",
            LogType::LateJoin => "LateJoin",
            LogType::ChemicalReaction => "ChemicalReaction",
            LogType::EntityEffect => "EntityEffect",
            LogType::CanisterValve => "CanisterValve",
            LogType::CanisterPressure => "CanisterPressure",
            LogType::CanisterPurged => "CanisterPurged",
            LogType::CanisterTankEjected => "CanisterTankEjected",
            LogType::CanisterTankInserted => "CanisterTankInserted",
            LogType::DisarmedAction => "DisarmedAction",
            LogType::DisarmedKnockdown => "DisarmedKnockdown",
            LogType::AttackArmedClick => "AttackArmedClick",
            LogType::AttackArmedWide => "AttackArmedWide",
            LogType::AttackUnarmedClick => "AttackUnarmedClick",
            LogType::AttackUnarmedWide => "AttackUnarmedWide",
            LogType::InteractHand => "InteractHand",
            LogType::InteractActivate => "InteractActivate",
            LogType::Throw => "Throw",
            LogType::Landed => "Landed",
            LogType::ThrowHit => "ThrowHit",
            LogType::Pickup => "Pickup",
            LogType::Drop => "Drop",
            LogType::BulletHit => "BulletHit",
            LogType::ForceFeed => "ForceFeed",
            LogType::Ingestion => "Ingestion",
            LogType::MeleeHit => "MeleeHit",
            LogType::HitScanHit => "HitScanHit",
            LogType::Mind => "Mind",
            LogType::Explosion => "Explosion",
            LogType::Radiation => "Radiation",
            LogType::Barotrauma => "Barotrauma",
            LogType::Flammable => "Flammable",
            LogType::Asphyxiation => "Asphyxiation",
            LogType::Temperature => "Temperature",
            LogType::Hunger => "Hunger",
            LogType::Thirst => "Thirst",
            LogType::Electrocution => "Electrocution",
            LogType::CrayonDraw => "CrayonDraw",
            LogType::AtmosPressureChanged => "AtmosPressureChanged",
            LogType::AtmosPowerChanged => "AtmosPowerChanged",
            LogType::AtmosVolumeChanged => "AtmosVolumeChanged",
            LogType::AtmosFilterChanged => "AtmosFilterChanged",
            LogType::AtmosRatioChanged => "AtmosRatioChanged",
            LogType::FieldGeneration => "FieldGeneration",
            LogType::GhostRoleTaken => "GhostRoleTaken",
            LogType::Chat => "Chat",
            LogType::Action => "Action",
            LogType::RCD => "RCD",
            LogType::Construction => "Construction",
            LogType::Trigger => "Trigger",
            LogType::Anchor => "Anchor",
            LogType::Unanchor => "Unanchor",
            LogType::EmergencyShuttle => "EmergencyShuttle",
            LogType::Emag => "Emag",
            LogType::Gib => "Gib",
            LogType::Identity => "Identity",
            LogType::CableCut => "CableCut",
            LogType::StorePurchase => "StorePurchase",
            LogType::LatticeCut => "LatticeCut",
            LogType::Stripping => "Stripping",
            LogType::Stamina => "Stamina",
            LogType::EntitySpawn => "EntitySpawn",
            LogType::AdminMessage => "AdminMessage",
            LogType::Anomaly => "Anomaly",
            LogType::WireHacking => "WireHacking",
            LogType::Teleport => "Teleport",
            LogType::EntityDelete => "EntityDelete",
            LogType::Vote => "Vote",
            LogType::ItemConfigure => "ItemConfigure",
            LogType::DeviceLinking => "DeviceLinking",
            LogType::Tile => "Tile",
            LogType::ChatRateLimited => "ChatRateLimited",
            LogType::AtmosTemperatureChanged => "AtmosTemperatureChanged",
            LogType::DeviceNetwork => "DeviceNetwork",
            LogType::StoreRefund => "StoreRefund",
            LogType::RateLimited => "RateLimited",
            LogType::InteractUsing => "InteractUsing",
            LogType::Storage => "Storage",
            LogType::ExplosionHit => "ExplosionHit",
            LogType::GhostWarp => "GhostWarp",
            LogType::PdaInteract => "PdaInteract",
            LogType::AtmosDeviceSetting => "AtmosDeviceSetting",
            LogType::AdminCommands => "AdminCommands",
            LogType::AntagSelection => "AntagSelection",
            LogType::Botany => "Botany",
            LogType::ArtifactNode => "ArtifactNode",
            LogType::ShuttleImpact => "ShuttleImpact",
            LogType::Instrument => "Instrument",
            LogType::Connection => "Connection",
            LogType::SiliconLaw => "SiliconLaw",
        }
    }
}

/// Важность записи (`LogImpact`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LogImpact {
    Low = -1,
    #[default]
    Medium = 0,
    High = 1,
    Extreme = 2,
}

impl LogImpact {
    pub fn from_i64(value: i64) -> Self {
        match value {
            -1 => Self::Low,
            1 => Self::High,
            2 => Self::Extreme,
            _ => Self::Medium,
        }
    }
}

/// Запись журнала (`AdminLog`).
#[derive(Debug, Clone, PartialEq)]
pub struct AdminLogEntry {
    pub round_id: i64,
    pub id: i64,
    pub log_type: LogType,
    pub impact: LogImpact,
    /// Время записи, мс UTC.
    pub date: i64,
    pub message: String,
    /// Значения записи объектом JSON.
    pub json: String,
    pub players: Vec<NetUserId>,
}

/// Очереди журнала. Ресурс сервера.
#[derive(Resource, Debug)]
pub struct AdminLogs {
    pub enabled: bool,
    /// Раунд идёт; `None` — до начала раунда (лобби).
    pub round: Option<i64>,
    next_id: i64,
    queue: Vec<AdminLogEntry>,
    pre_round_queue: Vec<AdminLogEntry>,
    next_save: Instant,
    saving: bool,
    pub dropped: u64,
}

impl Default for AdminLogs {
    fn default() -> Self {
        Self {
            enabled: true,
            round: None,
            next_id: 1,
            queue: Vec::new(),
            pre_round_queue: Vec::new(),
            next_save: Instant::now(),
            saving: false,
            dropped: 0,
        }
    }
}

impl AdminLogs {
    /// Записи, ждущие отправки: в раунде и до него.
    pub fn queued(&self) -> (usize, usize) {
        (self.queue.len(), self.pre_round_queue.len())
    }
}

/// Значение для записи: текст JSON-строкой.
pub fn json_object(values: &[(&str, String)]) -> String {
    let object: serde_json::Map<String, serde_json::Value> = values
        .iter()
        .map(|(k, v)| ((*k).to_owned(), serde_json::Value::String(v.clone())))
        .collect();
    serde_json::Value::Object(object).to_string()
}

/// Игрок, управляющий сущностью (`SerializablePlayer`, `EntityStringRepresentation.Session`).
pub fn player_of(world: &World, entity: Entity) -> Option<NetUserId> {
    let session = world.get::<astrocore_net::Actor>(entity)?.session;
    world.get::<Session>(session).map(|s| s.user_id)
}

/// Добавить запись (`IAdminLogManager.Add`).
pub fn add_admin_log(
    world: &mut World,
    log_type: LogType,
    impact: LogImpact,
    message: &str,
    values: &[(&str, String)],
    players: &[NetUserId],
) {
    let drop_threshold = world.cvar(&ADMIN_LOGS_DROP_THRESHOLD).max(0) as usize;
    let Some(mut logs) = world.get_resource_mut::<AdminLogs>() else {
        return;
    };
    if !logs.enabled {
        return;
    }
    let pre_round = logs.round.is_none();
    let count = if pre_round {
        logs.pre_round_queue.len()
    } else {
        logs.queue.len()
    };
    if count >= drop_threshold {
        logs.dropped += 1;
        return;
    }
    let id = logs.next_id;
    logs.next_id += 1;
    let mut unique = players.to_vec();
    unique.sort();
    unique.dedup();
    let entry = AdminLogEntry {
        round_id: logs.round.unwrap_or(0),
        id,
        log_type,
        impact,
        date: now_ms(),
        // PostgreSQL не хранит нулевые символы в тексте.
        message: message.replace('\0', ""),
        json: json_object(values),
        players: unique,
    };
    if pre_round {
        logs.pre_round_queue.push(entry);
    } else {
        logs.queue.push(entry);
    }
}

/// Раунд начался (`RoundStarting`): записи из лобби получают его номер и идут в очередь.
pub fn round_starting(world: &mut World, round: i64) {
    let Some(mut logs) = world.get_resource_mut::<AdminLogs>() else {
        return;
    };
    logs.round = Some(round);
    logs.next_id = 1;
    let waiting = std::mem::take(&mut logs.pre_round_queue);
    for mut entry in waiting {
        entry.round_id = round;
        entry.id = logs.next_id;
        logs.next_id += 1;
        logs.queue.push(entry);
    }
}

/// Отправка очереди в базу (`Update`, `TrySaveLogs`, `SaveLogs`).
fn update(world: &mut World) {
    let delay = Duration::from_secs_f32(world.cvar(&ADMIN_LOGS_QUEUE_SEND_DELAY).max(0.0));
    let queue_max = world.cvar(&ADMIN_LOGS_QUEUE_MAX).max(1) as usize;
    let enabled = world.cvar(&ADMIN_LOGS_ENABLED);
    let now = Instant::now();
    let batch = {
        let Some(mut logs) = world.get_resource_mut::<AdminLogs>() else {
            return;
        };
        logs.enabled = enabled;
        // До раунда записи ждут его номера.
        if logs.round.is_none() || logs.queue.is_empty() || logs.saving {
            return;
        }
        if now < logs.next_save && logs.queue.len() < queue_max {
            return;
        }
        logs.next_save = now + delay;
        logs.saving = true;
        std::mem::take(&mut logs.queue)
    };
    let sent = batch.clone();
    let started = spawn_db(
        world,
        move |db| async move { db.add_admin_logs(&sent).await },
        move |world, result: DbResult<()>| {
            let Some(mut logs) = world.get_resource_mut::<AdminLogs>() else {
                return;
            };
            logs.saving = false;
            if let Err(error) = result {
                eprintln!("журнал администраторов: {error}");
                // Не принятые базой записи возвращаются в начало очереди.
                let mut back = batch;
                back.append(&mut logs.queue);
                logs.queue = back;
            }
        },
    );
    if !started && let Some(mut logs) = world.get_resource_mut::<AdminLogs>() {
        // Без базы записи не копятся.
        logs.saving = false;
    }
}

/// Отбор записей (`LogFilter`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LogFilter {
    pub round: Option<i64>,
    /// Подстрока текста записи.
    pub search: Option<String>,
    pub types: Option<Vec<LogType>>,
    pub impacts: Option<Vec<LogImpact>>,
    /// Раньше этого времени, мс UTC.
    pub before: Option<i64>,
    /// Позже этого времени, мс UTC.
    pub after: Option<i64>,
    /// Касается хотя бы одного из игроков.
    pub any_players: Option<Vec<NetUserId>>,
    pub limit: Option<i64>,
    /// Сначала старые.
    pub ascending: bool,
}

impl Db {
    /// Сохранить записи с их игроками (`AddAdminLogs`). Игроки, которых нет в базе
    /// (гости без сохранения), к записи не привязываются.
    pub async fn add_admin_logs(&self, logs: &[AdminLogEntry]) -> DbResult<()> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        for log in logs {
            sqlx::query(
                "INSERT INTO admin_log (round_id, id, type, impact, date, message, json) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(log.round_id)
            .bind(log.id)
            .bind(log.log_type as i64)
            .bind(log.impact as i64)
            .bind(log.date)
            .bind(log.message.clone())
            .bind(log.json.clone())
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
            for player in &log.players {
                sqlx::query(
                    "INSERT INTO admin_log_player (round_id, log_id, player_user_id) \
                     SELECT $1, $2, $3 WHERE EXISTS (SELECT 1 FROM player WHERE user_id = $3)",
                )
                .bind(log.round_id)
                .bind(log.id)
                .bind(player.to_string())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            }
        }
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }

    /// Записи по отбору (`GetAdminLogs`), новые первыми, если не сказано иначе.
    pub async fn get_admin_logs(&self, filter: &LogFilter) -> DbResult<Vec<AdminLogEntry>> {
        // Отбор по раунду, времени и тексту — в базе; по видам, важности и игрокам — здесь.
        let rows = sqlx::query(
            "SELECT round_id, id, type, impact, date, message, json FROM admin_log \
             WHERE ($1 = 0 OR round_id = $2) AND ($3 = 0 OR date < $4) AND ($5 = 0 OR date > $6) \
             AND ($7 = '' OR message LIKE $8) ORDER BY date, id",
        )
        .bind(i64::from(filter.round.is_some()))
        .bind(filter.round.unwrap_or(0))
        .bind(i64::from(filter.before.is_some()))
        .bind(filter.before.unwrap_or(0))
        .bind(i64::from(filter.after.is_some()))
        .bind(filter.after.unwrap_or(0))
        .bind(filter.search.clone().unwrap_or_default())
        .bind(format!("%{}%", filter.search.clone().unwrap_or_default()))
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        let mut logs = Vec::new();
        for row in &rows {
            let round_id: i64 = row.try_get("round_id").map_err(db_error)?;
            let id: i64 = row.try_get("id").map_err(db_error)?;
            let Some(log_type) = LogType::from_i64(row.try_get("type").map_err(db_error)?) else {
                continue;
            };
            let impact = LogImpact::from_i64(row.try_get("impact").map_err(db_error)?);
            if filter
                .types
                .as_ref()
                .is_some_and(|t| !t.contains(&log_type))
                || filter
                    .impacts
                    .as_ref()
                    .is_some_and(|i| !i.contains(&impact))
            {
                continue;
            }
            let players = sqlx::query(
                "SELECT player_user_id FROM admin_log_player WHERE round_id = $1 AND log_id = $2 \
                 ORDER BY player_user_id",
            )
            .bind(round_id)
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?
            .iter()
            .map(|r| parse_user_id(&r.try_get::<String, _>("player_user_id").map_err(db_error)?))
            .collect::<DbResult<Vec<_>>>()?;
            if filter
                .any_players
                .as_ref()
                .is_some_and(|wanted| !wanted.iter().any(|p| players.contains(p)))
            {
                continue;
            }
            logs.push(AdminLogEntry {
                round_id,
                id,
                log_type,
                impact,
                date: row.try_get("date").map_err(db_error)?,
                message: row.try_get("message").map_err(db_error)?,
                json: row.try_get("json").map_err(db_error)?,
                players,
            });
        }
        if !filter.ascending {
            logs.reverse();
        }
        if let Some(limit) = filter.limit {
            logs.truncate(limit.max(0) as usize);
        }
        Ok(logs)
    }
}

fn session_entry(world: &World, session: Entity) -> Option<(NetUserId, String)> {
    world
        .get::<Session>(session)
        .map(|s| (s.user_id, s.name.clone()))
}

pub fn install(world: &mut World, side: Side) {
    world.register_cvar(&ADMIN_LOGS_ENABLED);
    world.register_cvar(&ADMIN_LOGS_QUEUE_SEND_DELAY);
    world.register_cvar(&ADMIN_LOGS_QUEUE_MAX);
    world.register_cvar(&ADMIN_LOGS_DROP_THRESHOLD);
    if !side.is_server() {
        return;
    }
    world.init_resource::<AdminLogs>();
    // Вход и выход игроков (`ServerGameTicker.Player`).
    world.subscribe_broadcast::<PlayerConnected>(|world, event| {
        let Some((user, name)) = session_entry(world, event.session) else {
            return;
        };
        let message = format!("Игрок {name} ({user}) вошёл в игру.");
        let values = [("player", user.to_string()), ("name", name)];
        add_admin_log(
            world,
            LogType::Connection,
            LogImpact::Low,
            &message,
            &values,
            &[user],
        );
    });
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        let Some((user, name)) = session_entry(world, event.session) else {
            return;
        };
        let message = format!("Игрок {name} ({user}) вышел из игры.");
        let values = [("player", user.to_string()), ("name", name)];
        add_admin_log(
            world,
            LogType::Connection,
            LogImpact::Low,
            &message,
            &values,
            &[user],
        );
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
