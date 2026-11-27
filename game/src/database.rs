//! База данных сервера: игроки, выданные гостям идентификаторы, серверы, раунды, журнал
//! подключений. SQLite (по умолчанию, файл в каталоге данных) или PostgreSQL — настройка
//! `database.engine`; схема — миграции `migrations/sqlite` и `migrations/postgres` (ADR-0011).
//! Запросы идут в фоновом рантайме: игра передаёт продолжение, оно выполняется в начале такта.
//! Образец — `Content.Server.Database` (`Model.cs`), `ServerDbManager`, `ServerDbBase`,
//! `ServerDbSqlite`, `ServerDbPostgres`, `ServerDbEntryManager` SS14 @ 2f4a2aad.
//!
//! Нет пока: профилей (6.2), банов и журнала действий администраторов (следующая часть 6.1),
//! заметок администраторов, белых списков, времени игры, искусственной задержки запросов
//! (`database.sqlite_delay`).

use std::future::Future;
use std::net::IpAddr;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use astrocore_console::{CVarDef, CVarFlags, Config, ConfigWorldExt};
use astrocore_ecs::{GameWorldExt, PreUpdate};
use astrocore_net::NetUserId;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use sqlx::AnyPool;
use sqlx::Row;
use sqlx::any::{AnyPoolOptions, AnyRow};
use tokio::runtime::Runtime;
use tokio::sync::OnceCell;

use crate::Side;

/// Движок базы: `sqlite` или `postgres` (`database.engine`).
pub const DATABASE_ENGINE: CVarDef<&str> = CVarDef::new(
    "database.engine",
    "sqlite",
    CVarFlags::SERVERONLY,
    "Движок базы данных: sqlite или postgres.",
);
/// Файл SQLite относительно каталога данных (`database.sqlite_dbpath`).
pub const DATABASE_SQLITE_PATH: CVarDef<&str> = CVarDef::new(
    "database.sqlite_dbpath",
    "preferences.db",
    CVarFlags::SERVERONLY,
    "Файл базы SQLite относительно каталога данных.",
);
/// Сколько запросов к SQLite одновременно (`database.sqlite_concurrency`).
pub const DATABASE_SQLITE_CONCURRENCY: CVarDef<i32> = CVarDef::new(
    "database.sqlite_concurrency",
    3,
    CVarFlags::SERVERONLY,
    "Сколько запросов к SQLite выполняется одновременно.",
);
pub const DATABASE_PG_HOST: CVarDef<&str> = CVarDef::new(
    "database.pg_host",
    "localhost",
    CVarFlags::SERVERONLY,
    "Адрес сервера PostgreSQL.",
);
pub const DATABASE_PG_PORT: CVarDef<i32> = CVarDef::new(
    "database.pg_port",
    5432,
    CVarFlags::SERVERONLY,
    "Порт сервера PostgreSQL.",
);
pub const DATABASE_PG_DATABASE: CVarDef<&str> = CVarDef::new(
    "database.pg_database",
    "astrocore",
    CVarFlags::SERVERONLY,
    "Имя базы PostgreSQL.",
);
pub const DATABASE_PG_USERNAME: CVarDef<&str> = CVarDef::new(
    "database.pg_username",
    "postgres",
    CVarFlags::SERVERONLY,
    "Пользователь PostgreSQL.",
);
pub const DATABASE_PG_PASSWORD: CVarDef<&str> = CVarDef::new(
    "database.pg_password",
    "",
    CVarFlags::SERVERONLY.union(CVarFlags::CONFIDENTIAL),
    "Пароль PostgreSQL.",
);
/// Сколько запросов к PostgreSQL одновременно (`database.pg_concurrency`).
pub const DATABASE_PG_CONCURRENCY: CVarDef<i32> = CVarDef::new(
    "database.pg_concurrency",
    8,
    CVarFlags::SERVERONLY,
    "Сколько запросов к PostgreSQL выполняется одновременно.",
);
/// Гость получает постоянный идентификатор по имени (`game.persistguests`).
pub const GAME_PERSIST_GUESTS: CVarDef<bool> = CVarDef::new(
    "game.persistguests",
    true,
    CVarFlags::SERVERONLY.union(CVarFlags::ARCHIVE),
    "Гость получает постоянный идентификатор по имени и его данные сохраняются.",
);
/// Имя сервера в базе (`adminlogs.server_name`).
pub const SERVER_NAME: CVarDef<&str> = CVarDef::new(
    "adminlogs.server_name",
    "unknown",
    CVarFlags::SERVERONLY,
    "Имя этого сервера в базе данных.",
);

/// Ошибка базы текстом.
pub type DbResult<T> = Result<T, String>;

fn db_error(error: impl std::fmt::Display) -> String {
    format!("база данных: {error}")
}

/// Время сейчас: миллисекунды UTC.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Движок базы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbEngine {
    Sqlite,
    Postgres,
}

/// Почему подключение отклонено (`ConnectionDenyReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionDenyReason {
    Ban = 0,
    Whitelist = 1,
    Full = 2,
    Panic = 3,
    BabyJail = 4,
    IpChecks = 5,
}

impl ConnectionDenyReason {
    fn from_i64(value: i64) -> Option<Self> {
        Some(match value {
            0 => Self::Ban,
            1 => Self::Whitelist,
            2 => Self::Full,
            3 => Self::Panic,
            4 => Self::BabyJail,
            5 => Self::IpChecks,
            _ => return None,
        })
    }
}

/// Игрок в базе (`PlayerRecord`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerRecord {
    pub user_id: NetUserId,
    pub first_seen_time: i64,
    pub last_seen_user_name: String,
    pub last_seen_time: i64,
    pub last_seen_address: String,
    pub last_read_rules: Option<i64>,
}

/// Раунд в базе (`RoundRecord`).
#[derive(Debug, Clone, PartialEq)]
pub struct RoundRecord {
    pub id: i64,
    pub start_date: Option<i64>,
    pub server_id: i64,
    pub players: Vec<NetUserId>,
}

/// Запись журнала подключений (`ConnectionLog`).
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionLogRecord {
    pub id: i64,
    pub user_id: NetUserId,
    pub user_name: String,
    pub time: i64,
    pub address: String,
    pub denied: Option<ConnectionDenyReason>,
    pub server_id: i64,
}

fn parse_user_id(text: &str) -> DbResult<NetUserId> {
    uuid::Uuid::parse_str(text).map(NetUserId).map_err(db_error)
}

/// Доступ к базе: пул соединений. Дёшево клонируется в задачи.
#[derive(Clone)]
pub struct Db {
    pool: AnyPool,
    pub engine: DbEngine,
    /// Запись этого сервера (`ServerDbEntryManager.ServerEntity`).
    server: Arc<OnceCell<i64>>,
    server_name: Arc<str>,
}

impl Db {
    /// Номер этого сервера в базе: создаётся при первом обращении.
    pub async fn server_id(&self) -> DbResult<i64> {
        self.server
            .get_or_try_init(|| async {
                self.add_or_get_server(&self.server_name).await.map(|s| s.0)
            })
            .await
            .copied()
    }

    /// Выданный гостю идентификатор (`GetAssignedUserIdAsync`).
    pub async fn get_assigned_user_id(&self, name: &str) -> DbResult<Option<NetUserId>> {
        let row = sqlx::query("SELECT user_id FROM assigned_user_id WHERE user_name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?;
        row.map(|r| parse_user_id(&r.try_get::<String, _>("user_id").map_err(db_error)?))
            .transpose()
    }

    /// Запомнить идентификатор гостя (`AssignUserIdAsync`).
    pub async fn assign_user_id(&self, name: &str, user_id: NetUserId) -> DbResult<()> {
        sqlx::query("INSERT INTO assigned_user_id (user_name, user_id) VALUES ($1, $2)")
            .bind(name)
            .bind(user_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(())
    }

    /// Обновить запись игрока при входе, создать при первом (`UpdatePlayerRecordAsync`).
    pub async fn update_player_record(
        &self,
        user_id: NetUserId,
        user_name: &str,
        address: &str,
    ) -> DbResult<()> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO player (user_id, first_seen_time, last_seen_user_name, last_seen_time, \
             last_seen_address) VALUES ($1, $2, $3, $2, $4) \
             ON CONFLICT (user_id) DO UPDATE SET last_seen_user_name = excluded.last_seen_user_name, \
             last_seen_time = excluded.last_seen_time, last_seen_address = excluded.last_seen_address",
        )
        .bind(user_id.to_string())
        .bind(now)
        .bind(user_name)
        .bind(address)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    fn player_record(row: &AnyRow) -> DbResult<PlayerRecord> {
        Ok(PlayerRecord {
            user_id: parse_user_id(&row.try_get::<String, _>("user_id").map_err(db_error)?)?,
            first_seen_time: row.try_get("first_seen_time").map_err(db_error)?,
            last_seen_user_name: row.try_get("last_seen_user_name").map_err(db_error)?,
            last_seen_time: row.try_get("last_seen_time").map_err(db_error)?,
            last_seen_address: row.try_get("last_seen_address").map_err(db_error)?,
            last_read_rules: row.try_get("last_read_rules").map_err(db_error)?,
        })
    }

    /// Игрок по идентификатору (`GetPlayerRecordByUserId`).
    pub async fn get_player_record_by_user_id(
        &self,
        user_id: NetUserId,
    ) -> DbResult<Option<PlayerRecord>> {
        let row = sqlx::query(
            "SELECT user_id, first_seen_time, last_seen_user_name, last_seen_time, \
             last_seen_address, last_read_rules FROM player WHERE user_id = $1",
        )
        .bind(user_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.as_ref().map(Self::player_record).transpose()
    }

    /// Игрок по последнему имени; при совпадении — тот, кто заходил позже
    /// (`GetPlayerRecordByUserName`).
    pub async fn get_player_record_by_user_name(
        &self,
        user_name: &str,
    ) -> DbResult<Option<PlayerRecord>> {
        let row = sqlx::query(
            "SELECT user_id, first_seen_time, last_seen_user_name, last_seen_time, \
             last_seen_address, last_read_rules FROM player WHERE last_seen_user_name = $1 \
             ORDER BY last_seen_time DESC LIMIT 1",
        )
        .bind(user_name)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.as_ref().map(Self::player_record).transpose()
    }

    /// Запись журнала подключений (`AddConnectionLogAsync`): её номер.
    pub async fn add_connection_log(
        &self,
        user_id: NetUserId,
        user_name: &str,
        address: &str,
        denied: Option<ConnectionDenyReason>,
        server_id: i64,
    ) -> DbResult<i64> {
        let row = sqlx::query(
            "INSERT INTO connection_log (user_id, user_name, time, address, denied, server_id, trust) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
        )
        .bind(user_id.to_string())
        .bind(user_name)
        .bind(now_ms())
        .bind(address)
        .bind(denied.map(|d| d as i64))
        .bind(server_id)
        .bind(0.0f64)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        row.try_get("id").map_err(db_error)
    }

    /// Журнал подключений игрока, новые первыми.
    pub async fn connection_logs(&self, user_id: NetUserId) -> DbResult<Vec<ConnectionLogRecord>> {
        let rows = sqlx::query(
            "SELECT id, user_id, user_name, time, address, denied, server_id FROM connection_log \
             WHERE user_id = $1 ORDER BY time DESC, id DESC",
        )
        .bind(user_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.iter()
            .map(|row| {
                Ok(ConnectionLogRecord {
                    id: row.try_get("id").map_err(db_error)?,
                    user_id: parse_user_id(
                        &row.try_get::<String, _>("user_id").map_err(db_error)?,
                    )?,
                    user_name: row.try_get("user_name").map_err(db_error)?,
                    time: row.try_get("time").map_err(db_error)?,
                    address: row.try_get("address").map_err(db_error)?,
                    denied: row
                        .try_get::<Option<i64>, _>("denied")
                        .map_err(db_error)?
                        .and_then(ConnectionDenyReason::from_i64),
                    server_id: row.try_get("server_id").map_err(db_error)?,
                })
            })
            .collect()
    }

    /// Сервер по имени, созданный при первом обращении (`AddOrGetServer`): номер и был ли.
    pub async fn add_or_get_server(&self, name: &str) -> DbResult<(i64, bool)> {
        let existing = sqlx::query("SELECT id FROM server WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?;
        if let Some(row) = existing {
            return Ok((row.try_get("id").map_err(db_error)?, true));
        }
        let row = sqlx::query("INSERT INTO server (name) VALUES ($1) RETURNING id")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?;
        Ok((row.try_get("id").map_err(db_error)?, false))
    }

    /// Новый раунд с игроками, которые есть в базе (`AddNewRound`): его номер.
    pub async fn add_new_round(&self, server_id: i64, players: &[NetUserId]) -> DbResult<i64> {
        let row =
            sqlx::query("INSERT INTO round (start_date, server_id) VALUES ($1, $2) RETURNING id")
                .bind(now_ms())
                .bind(server_id)
                .fetch_one(&self.pool)
                .await
                .map_err(db_error)?;
        let id: i64 = row.try_get("id").map_err(db_error)?;
        self.add_round_players(id, players).await?;
        Ok(id)
    }

    /// Добавить игроков в раунд (`AddRoundPlayers`); неизвестные базе пропускаются.
    pub async fn add_round_players(&self, round: i64, players: &[NetUserId]) -> DbResult<()> {
        for player in players {
            sqlx::query(
                "INSERT INTO player_round (players_id, rounds_id) \
                 SELECT id, $2 FROM player WHERE user_id = $1 ON CONFLICT DO NOTHING",
            )
            .bind(player.to_string())
            .bind(round)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        }
        Ok(())
    }

    /// Раунд с игроками (`GetRound`).
    pub async fn get_round(&self, id: i64) -> DbResult<Option<RoundRecord>> {
        let Some(row) = sqlx::query("SELECT id, start_date, server_id FROM round WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?
        else {
            return Ok(None);
        };
        let players = sqlx::query(
            "SELECT player.user_id FROM player_round JOIN player ON player.id = player_round.players_id \
             WHERE player_round.rounds_id = $1 ORDER BY player.id",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?
        .iter()
        .map(|r| parse_user_id(&r.try_get::<String, _>("user_id").map_err(db_error)?))
        .collect::<DbResult<Vec<_>>>()?;
        Ok(Some(RoundRecord {
            id: row.try_get("id").map_err(db_error)?,
            start_date: row.try_get("start_date").map_err(db_error)?,
            server_id: row.try_get("server_id").map_err(db_error)?,
            players,
        }))
    }
}

/// Продолжение запроса: выполняется в мире в начале такта.
type Continuation = Box<dyn FnOnce(&mut World) + Send>;

/// База сервера: пул, фоновый рантайм и очередь готовых продолжений. Ресурс сервера; без
/// него игра работает без базы (ADR-0011).
#[derive(Resource)]
pub struct Database {
    db: Db,
    runtime: Arc<Runtime>,
    done: Sender<Continuation>,
    ready: Mutex<Receiver<Continuation>>,
}

/// Строка подключения PostgreSQL: имя и пароль — с экранированием.
fn postgres_url(host: &str, port: i32, database: &str, user: &str, password: &str) -> String {
    fn escape(text: &str) -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }
    let auth = if password.is_empty() {
        escape(user)
    } else {
        format!("{}:{}", escape(user), escape(password))
    };
    format!("postgres://{auth}@{host}:{port}/{}", escape(database))
}

impl Database {
    /// Подключиться по URL, применить миграции движка.
    pub fn connect(
        url: &str,
        engine: DbEngine,
        concurrency: u32,
        server_name: &str,
    ) -> DbResult<Self> {
        sqlx::any::install_default_drivers();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("database")
            .enable_all()
            .build()
            .map_err(db_error)?;
        let in_memory = url.contains(":memory:");
        let pool = runtime.block_on(async {
            let mut options = AnyPoolOptions::new().max_connections(if in_memory {
                1
            } else {
                concurrency.max(1)
            });
            if in_memory {
                // База в памяти живёт, пока открыто её единственное соединение.
                options = options
                    .idle_timeout(None)
                    .max_lifetime(None)
                    .min_connections(1);
            }
            let pool = options.connect(url).await.map_err(db_error)?;
            match engine {
                DbEngine::Sqlite => sqlx::migrate!("./migrations/sqlite").run(&pool).await,
                DbEngine::Postgres => sqlx::migrate!("./migrations/postgres").run(&pool).await,
            }
            .map_err(db_error)?;
            Ok::<_, String>(pool)
        })?;
        let (done, ready) = channel();
        Ok(Self {
            db: Db {
                pool,
                engine,
                server: Arc::new(OnceCell::new()),
                server_name: server_name.into(),
            },
            runtime: Arc::new(runtime),
            done,
            ready: Mutex::new(ready),
        })
    }

    /// SQLite в памяти — для тестов.
    pub fn in_memory() -> DbResult<Self> {
        Self::connect("sqlite::memory:", DbEngine::Sqlite, 1, "test")
    }

    /// Подключиться по настройкам `database.*` (`ServerDbManager.Init`).
    pub fn from_config(config: &Config) -> DbResult<Self> {
        let server_name = config.get_text(&SERVER_NAME);
        match config.get_text(&DATABASE_ENGINE).to_lowercase().as_str() {
            "sqlite" => {
                let path = std::path::Path::new(&config.get_text(&crate::map_commands::USER_DATA))
                    .join(config.get_text(&DATABASE_SQLITE_PATH));
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(db_error)?;
                }
                let url = format!("sqlite://{}?mode=rwc", path.display());
                let concurrency = config.get(&DATABASE_SQLITE_CONCURRENCY).max(1) as u32;
                Self::connect(&url, DbEngine::Sqlite, concurrency, &server_name)
            }
            "postgres" => {
                let url = postgres_url(
                    &config.get_text(&DATABASE_PG_HOST),
                    config.get(&DATABASE_PG_PORT),
                    &config.get_text(&DATABASE_PG_DATABASE),
                    &config.get_text(&DATABASE_PG_USERNAME),
                    &config.get_text(&DATABASE_PG_PASSWORD),
                );
                let concurrency = config.get(&DATABASE_PG_CONCURRENCY).max(1) as u32;
                Self::connect(&url, DbEngine::Postgres, concurrency, &server_name)
            }
            other => Err(format!("database.engine: неизвестный движок «{other}»")),
        }
    }

    /// Доступ к запросам.
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Дождаться задачи (тесты, запуск сервера).
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// Запустить задачу в фоне; `then` получит её результат в мире в начале такта.
    pub fn spawn<T, F, C>(&self, task: impl FnOnce(Db) -> F, then: C)
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
        C: FnOnce(&mut World, T) + Send + 'static,
    {
        let future = task(self.db.clone());
        let done = self.done.clone();
        self.runtime.spawn(async move {
            let result = future.await;
            let continuation: Continuation = Box::new(move |world| then(world, result));
            let _ = done.send(continuation);
        });
    }

    fn take_ready(&self) -> Vec<Continuation> {
        self.ready
            .lock()
            .map(|r| r.try_iter().collect())
            .unwrap_or_default()
    }
}

/// Запустить задачу базы из мира; без базы — ничего.
pub fn spawn_db<T, F, C>(world: &World, task: impl FnOnce(Db) -> F, then: C) -> bool
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
    C: FnOnce(&mut World, T) + Send + 'static,
{
    match world.get_resource::<Database>() {
        Some(database) => {
            database.spawn(task, then);
            true
        }
        None => false,
    }
}

/// Готовые продолжения запросов выполняются в мире (`PreUpdate`).
pub fn apply_results(world: &mut World) {
    let Some(ready) = world.get_resource::<Database>().map(Database::take_ready) else {
        return;
    };
    for continuation in ready {
        continuation(world);
    }
}

/// Адрес игрока для базы: только IP, как `IPAddress` SS14.
pub fn address_text(address: Option<std::net::SocketAddr>) -> String {
    address.map_or_else(
        || IpAddr::from([0, 0, 0, 0]).to_string(),
        |a| a.ip().to_string(),
    )
}

pub fn install(world: &mut World, side: Side) {
    for def in [
        &DATABASE_ENGINE,
        &DATABASE_SQLITE_PATH,
        &DATABASE_PG_HOST,
        &DATABASE_PG_DATABASE,
        &DATABASE_PG_USERNAME,
        &DATABASE_PG_PASSWORD,
        &SERVER_NAME,
    ] {
        world.register_cvar(def);
    }
    for def in [
        &DATABASE_SQLITE_CONCURRENCY,
        &DATABASE_PG_PORT,
        &DATABASE_PG_CONCURRENCY,
    ] {
        world.register_cvar(def);
    }
    world.register_cvar(&GAME_PERSIST_GUESTS);
    if !side.is_server() {
        return;
    }
    world.add_game_systems(PreUpdate, apply_results);
}

#[cfg(test)]
mod tests;
