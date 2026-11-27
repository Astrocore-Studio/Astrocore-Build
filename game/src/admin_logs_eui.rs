//! Окно журнала администратора: команда `adminlogs` открывает его, клиент просит записи по
//! отбору (раунд, текст, виды, важность, игроки), сервер отдаёт их пачками из базы, «Далее» —
//! следующая пачка. Образец — `AdminLogsEui`, `AdminLogsEuiState`, `AdminLogsEuiMsg`,
//! `OpenAdminLogsCommand` SS14 @ 2f4a2aad.
//!
//! Нет пока: выгрузки в файл, отбора «все игроки сразу» и «без игроков», окна вне игры.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use astrocore_console::{
    CVarDef, CVarFlags, ConfigWorldExt, ConsoleCommand, ConsoleShell, ConsoleWorldExt,
};
use astrocore_ecs::EventWorldExt;
use astrocore_net::{NetEventWorldExt, NetTarget, NetUserId, PlayerDisconnected};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{AdminLogEntry, AdminLogs, LogFilter, LogImpact, LogType};
use crate::admin_manager::{self, flags};
use crate::database::{DbResult, spawn_db};

pub const ADMIN_LOGS_CLIENT_BATCH_SIZE: CVarDef<i32> = CVarDef::new(
    "adminlogs.client_batch_size",
    1000,
    CVarFlags::SERVERONLY,
    "Сколько записей журнала отдаётся окну за раз.",
);

/// Запись для окна (`SharedAdminLog`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedAdminLog {
    pub id: i64,
    pub log_type: LogType,
    pub impact: LogImpact,
    /// Время, мс UTC.
    pub date: i64,
    pub message: String,
    pub players: Vec<NetUserId>,
}

impl From<&AdminLogEntry> for SharedAdminLog {
    fn from(log: &AdminLogEntry) -> Self {
        Self {
            id: log.id,
            log_type: log.log_type,
            impact: log.impact,
            date: log.date,
            message: log.message.clone(),
            players: log.players.clone(),
        }
    }
}

/// Отбор, который просит окно (`LogsRequest`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LogsRequest {
    pub round_id: Option<i64>,
    pub search: Option<String>,
    pub types: Option<Vec<LogType>>,
    pub impacts: Option<Vec<LogImpact>>,
    pub any_players: Option<Vec<NetUserId>>,
    pub ascending: bool,
}

/// Сообщения окна журнала (`AdminLogsEuiState`, `AdminLogsEuiMsg`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AdminLogsMessage {
    /// Сервер: окно открыто — текущий раунд, известные игроки, записей в раунде.
    State {
        round_id: i64,
        players: BTreeMap<NetUserId, String>,
        round_logs: i64,
    },
    /// Сервер: пачка записей (`NewLogs`).
    NewLogs {
        logs: Vec<SharedAdminLog>,
        replace: bool,
        has_next: bool,
    },
    /// Клиент: записи по отбору.
    Request(LogsRequest),
    /// Клиент: следующая пачка (`NextLogsRequest`).
    Next,
    /// Клиент: окно закрыто.
    Close,
}

struct OpenWindow {
    filter: LogFilter,
    /// Номер запроса: ответ на устаревший запрос не отправляется (`_logSendCancellation`).
    generation: u64,
}

/// Открытые окна журнала по сессиям. Ресурс сервера.
#[derive(Resource, Default)]
pub struct AdminLogsWindows {
    open: HashMap<Entity, OpenWindow>,
}

impl AdminLogsWindows {
    pub fn is_open(&self, session: Entity) -> bool {
        self.open.contains_key(&session)
    }
}

fn current_round(world: &World) -> i64 {
    world
        .get_resource::<AdminLogs>()
        .and_then(|l| l.round)
        .unwrap_or(0)
}

/// Открыть окно администратору (`OpenAdminLogsCommand`).
pub fn open(world: &mut World, session: Entity) {
    let round_id = current_round(world);
    world
        .get_resource_or_insert_with(AdminLogsWindows::default)
        .open
        .insert(
            session,
            OpenWindow {
                filter: LogFilter::default(),
                generation: 0,
            },
        );
    let players = world
        .get_resource::<crate::admin_system::AdminPlayerList>()
        .map(|l| {
            l.players
                .values()
                .map(|p| (p.user_id, p.username.clone()))
                .collect()
        })
        .unwrap_or_default();
    let round_logs = world
        .get_resource::<AdminLogs>()
        .map_or(0, AdminLogs::round_logs);
    world.send_net_event(
        NetTarget::Session(session),
        &AdminLogsMessage::State {
            round_id,
            players,
            round_logs,
        },
    );
}

/// Отправить пачку по сохранённому отбору (`SendLogs`).
fn send_logs(world: &mut World, session: Entity, replace: bool) {
    let Some(window) = world
        .get_resource_mut::<AdminLogsWindows>()
        .and_then(|w| w.into_inner().open.get_mut(&session))
    else {
        return;
    };
    window.generation += 1;
    let generation = window.generation;
    let filter = window.filter.clone();
    let limit = filter.limit.unwrap_or(0);
    let started = spawn_db(
        world,
        move |db| async move { db.get_admin_logs(&filter).await },
        move |world, result: DbResult<Vec<AdminLogEntry>>| {
            let logs = match result {
                Ok(logs) => logs,
                Err(error) => {
                    eprintln!("журнал администратора: {error}");
                    Vec::new()
                }
            };
            let Some(window) = world
                .get_resource_mut::<AdminLogsWindows>()
                .and_then(|w| w.into_inner().open.get_mut(&session))
            else {
                return;
            };
            if window.generation != generation {
                return;
            }
            // Следующая пачка — после последней отданной записи.
            if let Some(last) = logs.last() {
                window.filter.last_log_id = Some(last.id);
            }
            let has_next = limit > 0 && logs.len() as i64 >= limit;
            let message = AdminLogsMessage::NewLogs {
                logs: logs.iter().map(SharedAdminLog::from).collect(),
                replace,
                has_next,
            };
            world.send_net_event(NetTarget::Session(session), &message);
        },
    );
    if !started {
        let message = AdminLogsMessage::NewLogs {
            logs: Vec::new(),
            replace,
            has_next: false,
        };
        world.send_net_event(NetTarget::Session(session), &message);
    }
}

/// Сообщение окна от клиента (`HandleMessage`): только с правом `LOGS` и открытым окном.
pub fn handle_message(world: &mut World, session: Entity, message: &AdminLogsMessage) {
    if matches!(message, AdminLogsMessage::Close) {
        if let Some(mut windows) = world.get_resource_mut::<AdminLogsWindows>() {
            windows.open.remove(&session);
        }
        return;
    }
    let open = world
        .get_resource::<AdminLogsWindows>()
        .is_some_and(|w| w.is_open(session));
    if !open || !admin_manager::has_flag(world, session, flags::LOGS) {
        return;
    }
    match message {
        AdminLogsMessage::Request(request) => {
            let limit = i64::from(world.cvar(&ADMIN_LOGS_CLIENT_BATCH_SIZE).max(1));
            let round = request.round_id.unwrap_or_else(|| current_round(world));
            let filter = LogFilter {
                round: Some(round),
                search: request.search.clone().filter(|s| !s.is_empty()),
                types: request.types.clone(),
                impacts: request.impacts.clone(),
                any_players: request.any_players.clone().filter(|p| !p.is_empty()),
                limit: Some(limit),
                ascending: request.ascending,
                last_log_id: None,
                ..LogFilter::default()
            };
            if let Some(window) = world
                .resource_mut::<AdminLogsWindows>()
                .open
                .get_mut(&session)
            {
                window.filter = filter;
            }
            send_logs(world, session, true);
        }
        AdminLogsMessage::Next => send_logs(world, session, false),
        _ => {}
    }
}

/// Окно журнала. Ресурс мира клиента.
#[derive(Resource, Debug, Clone)]
pub struct AdminLogsWindow {
    pub open: bool,
    pub current_round: i64,
    pub round: i64,
    pub round_logs: i64,
    pub logs: Vec<SharedAdminLog>,
    pub has_next: bool,
    pub search: String,
    pub search_focused: bool,
    /// Показываемая важность (`SelectedImpacts`): отбор на клиенте, как в SS14.
    pub impacts: BTreeSet<LogImpact>,
    pub page: usize,
    pub version: u64,
}

impl Default for AdminLogsWindow {
    fn default() -> Self {
        Self {
            open: false,
            current_round: 0,
            round: 0,
            round_logs: 0,
            logs: Vec::new(),
            has_next: false,
            search: String::new(),
            search_focused: false,
            impacts: IMPACTS.into_iter().collect(),
            page: 0,
            version: 0,
        }
    }
}

pub const IMPACTS: [LogImpact; 4] = [
    LogImpact::Low,
    LogImpact::Medium,
    LogImpact::High,
    LogImpact::Extreme,
];

impl AdminLogsWindow {
    /// Записи, прошедшие отбор по важности.
    pub fn visible(&self) -> Vec<&SharedAdminLog> {
        self.logs
            .iter()
            .filter(|l| self.impacts.contains(&l.impact))
            .collect()
    }
}

/// Пришло сообщение окна журнала (`AdminLogsEui.HandleState`, `HandleMessage`).
pub fn receive_logs_message(world: &mut World, message: &AdminLogsMessage) {
    match message {
        AdminLogsMessage::State {
            round_id,
            round_logs,
            ..
        } => {
            let first = {
                let mut window = world.get_resource_or_insert_with(AdminLogsWindow::default);
                let first = !window.open;
                window.open = true;
                window.current_round = *round_id;
                window.round_logs = *round_logs;
                if first {
                    window.round = *round_id;
                }
                window.version += 1;
                first
            };
            // Открытое окно сразу просит записи текущего раунда.
            if first {
                request_logs(world);
            }
        }
        AdminLogsMessage::NewLogs {
            logs,
            replace,
            has_next,
        } => {
            let mut window = world.get_resource_or_insert_with(AdminLogsWindow::default);
            if *replace {
                window.logs.clone_from(logs);
                window.page = 0;
            } else {
                window.logs.extend(logs.iter().cloned());
            }
            window.has_next = *has_next;
            window.version += 1;
        }
        _ => {}
    }
}

/// Запросить записи по отбору окна (`RequestLogs`).
pub fn request_logs(world: &mut World) {
    let window = world
        .get_resource_or_insert_with(AdminLogsWindow::default)
        .clone();
    let search = window.search.trim().to_owned();
    world.send_net_event_to_server(&AdminLogsMessage::Request(LogsRequest {
        round_id: Some(window.round),
        search: (!search.is_empty()).then_some(search),
        ..LogsRequest::default()
    }));
}

/// Закрыть окно журнала.
pub fn close_logs(world: &mut World) {
    let mut window = world.get_resource_or_insert_with(AdminLogsWindow::default);
    window.open = false;
    window.search_focused = false;
    window.version += 1;
    world.send_net_event_to_server(&AdminLogsMessage::Close);
}

fn adminlogs_command(world: &mut World, shell: &mut ConsoleShell, _args: &[String]) {
    let Some(session) = shell.player else {
        shell.write_error("Команда только для игрока.");
        return;
    };
    open(world, session);
}

pub fn install(world: &mut World, side: Side) {
    world.register_net_event::<AdminLogsMessage>();
    if !side.is_server() {
        world.init_resource::<AdminLogsWindow>();
        world.subscribe_net_event::<AdminLogsMessage>(|world, event| {
            let message = event.event.clone();
            receive_logs_message(world, &message);
        });
        return;
    }
    world.register_cvar(&ADMIN_LOGS_CLIENT_BATCH_SIZE);
    world.init_resource::<AdminLogsWindows>();
    world.register_command(ConsoleCommand {
        name: "adminlogs",
        description: "Открыть журнал администратора.",
        help: "adminlogs",
        execute: adminlogs_command,
        complete: None,
    });
    world.subscribe_net_event::<AdminLogsMessage>(|world, event| {
        let Some(session) = event.session else {
            return;
        };
        let message = event.event.clone();
        handle_message(world, session, &message);
    });
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        let session = event.session;
        world
            .resource_mut::<AdminLogsWindows>()
            .open
            .remove(&session);
    });
}

#[cfg(test)]
mod tests;
