//! Команды администратора: `kick`, `ban`, `pardon`, `banlist`, `spawn`, `delete`,
//! `rejuvenate`; поиск игрока по имени или идентификатору среди играющих и в базе; новый
//! бан выгоняет попавших под него игроков и оповещает администраторов. Ответы команд,
//! ждущих базу, приходят в консоль игрока отдельно. Образец — `KickCommand`, `SpawnCommand`
//! и `DeleteCommand` Robust, `BanCommand`, `PardonCommand`, `BanListCommand`,
//! `RejuvenateCommand`, `BanManager.CreateServerBan`, `KickMatchingConnectedPlayers`,
//! `PlayerLocator.LookupIdByNameOrIdAsync`, `ChatManager.SendAdminAlert` SS14 @ 2f4a2aad.
//!
//! Нет пока: банов на роли (`roleban`) и окна банов (`BanPanelEui`, `BanListEui`), банов по
//! железу, заметок администратора.

use std::net::IpAddr;

use astrocore_console::{
    CVarDef, CVarFlags, ConfigWorldExt, ConsoleCommand, ConsoleShell, ConsoleWorldExt,
};
use astrocore_ecs::LifecycleWorldExt;
use astrocore_loc::LocArg;
use astrocore_map::{EntityCoordinates, MapCoordinates, SpawnWorldExt, TransformWorldExt};
use astrocore_math::Vec2;
use astrocore_net::{
    NetEventWorldExt, NetTarget, NetUserId, RemoteCommandOutput, ServerNet, Session,
    reject_connection,
};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::Side;
use crate::bans::{BanDef, NoteSeverity, PlayerInfo, UnbanDef, ban_matches, format_ban_message};
use crate::database::now_ms;
use crate::examine::loc;

pub const SERVER_BAN_DEFAULT_SEVERITY: CVarDef<&str> = CVarDef::new(
    "admin.server_ban_default_severity",
    "High",
    CVarFlags::ARCHIVE
        .union(CVarFlags::SERVER)
        .union(CVarFlags::REPLICATED),
    "Тяжесть бана по умолчанию.",
);

/// Найденный игрок (`LocatedPlayerData`).
#[derive(Debug, Clone, PartialEq)]
pub struct LocatedPlayer {
    pub user_id: NetUserId,
    pub user_name: String,
    pub last_address: Option<IpAddr>,
}

/// Ответ команды позже: игроку — в консоль, серверу — в журнал.
pub fn reply_later(world: &mut World, session: Option<Entity>, line: String, error: bool) {
    match session.filter(|s| world.get::<Session>(*s).is_some()) {
        Some(session) => {
            let reply = if error {
                RemoteCommandOutput {
                    output: Vec::new(),
                    errors: vec![line],
                }
            } else {
                RemoteCommandOutput {
                    output: vec![line],
                    errors: Vec::new(),
                }
            };
            world.send_net_event(NetTarget::Session(session), &reply);
        }
        None => eprintln!("{line}"),
    }
}

fn address_of(world: &World, session: Entity) -> Option<IpAddr> {
    let conn = world.get::<Session>(session)?.conn;
    world
        .get_resource::<ServerNet>()?
        .address(conn)
        .map(|a| a.ip())
}

/// Играющий игрок по имени (без учёта регистра).
pub fn session_by_name(world: &mut World, name: &str) -> Option<Entity> {
    world
        .query::<(Entity, &Session)>()
        .iter(world)
        .find(|(_, s)| s.name.eq_ignore_ascii_case(name))
        .map(|(e, _)| e)
}

/// Найти игрока (`LookupIdByNameOrIdAsync`): среди играющих, по идентификатору или по
/// последнему имени в базе. Результат — в `then`.
pub fn locate_player(
    world: &mut World,
    text: &str,
    then: impl FnOnce(&mut World, Option<LocatedPlayer>) + Send + 'static,
) {
    if let Some(session) = session_by_name(world, text) {
        let located = world.get::<Session>(session).map(|s| LocatedPlayer {
            user_id: s.user_id,
            user_name: s.name.clone(),
            last_address: address_of(world, session),
        });
        then(world, located);
        return;
    }
    let by_id = uuid::Uuid::parse_str(text).ok().map(NetUserId);
    let only_id = |id: NetUserId| LocatedPlayer {
        user_id: id,
        user_name: id.to_string(),
        last_address: None,
    };
    if world.get_resource::<crate::database::Database>().is_none() {
        // Без базы — только по идентификатору.
        then(world, by_id.map(only_id));
        return;
    }
    let name = text.to_owned();
    crate::database::spawn_db(
        world,
        move |db| async move {
            let record = match by_id {
                Some(id) => db.get_player_record_by_user_id(id).await,
                None => db.get_player_record_by_user_name(&name).await,
            };
            record.ok().flatten()
        },
        move |world, record| {
            let located = match (record, by_id) {
                (Some(record), _) => Some(LocatedPlayer {
                    user_id: record.user_id,
                    user_name: record.last_seen_user_name,
                    last_address: record.last_seen_address.parse().ok(),
                }),
                // Идентификатор без записи — всё равно годится для бана.
                (None, Some(id)) => Some(only_id(id)),
                (None, None) => None,
            };
            then(world, located);
        },
    );
}

// Кик.

/// Выгнать игрока с причиной (`DisconnectChannel`).
pub fn kick(world: &mut World, session: Entity, reason: &str) {
    if let Some(conn) = world.get::<Session>(session).map(|s| s.conn) {
        reject_connection(world, conn, reason);
    }
}

fn kick_command(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let Some((name, rest)) = args.split_first() else {
        shell.write_line("kick <игрок> [причина]");
        return;
    };
    let Some(target) = session_by_name(world, name) else {
        shell.write_error(loc(world, "shell-target-player-does-not-exist", &[]));
        return;
    };
    let reason = if rest.is_empty() {
        "Kicked by console".to_owned()
    } else {
        format!("Kicked by console: {}", rest.join(" "))
    };
    kick(world, target, &reason);
}

// Баны.

fn parse_severity(text: &str) -> Option<NoteSeverity> {
    Some(match text.trim().to_ascii_lowercase().as_str() {
        "none" => NoteSeverity::None,
        "minor" => NoteSeverity::Minor,
        "medium" => NoteSeverity::Medium,
        "high" => NoteSeverity::High,
        _ => return None,
    })
}

fn severity_name(severity: NoteSeverity) -> &'static str {
    match severity {
        NoteSeverity::None => "None",
        NoteSeverity::Minor => "Minor",
        NoteSeverity::Medium => "Medium",
        NoteSeverity::High => "High",
    }
}

/// Выгнать играющих, попавших под бан (`KickMatchingConnectedPlayers`).
pub fn kick_matching_connected_players(world: &mut World, ban: &BanDef) {
    let sessions: Vec<(Entity, NetUserId)> = world
        .query::<(Entity, &Session)>()
        .iter(world)
        .map(|(e, s)| (e, s.user_id))
        .collect();
    let message = format_ban_message(world, ban);
    for (session, user) in sessions {
        let player = PlayerInfo {
            user_id: Some(user),
            address: address_of(world, session),
            exempt_flags: 0,
            is_new_player: false,
        };
        if ban_matches(ban, &player) {
            kick(world, session, &message);
        }
    }
}

/// Бан на сервер (`CreateServerBan`): в базу, оповещение администраторов, игроков — вон.
pub fn create_server_ban(
    world: &mut World,
    admin: Option<Entity>,
    target: LocatedPlayer,
    reason: &str,
    minutes: u32,
    severity: NoteSeverity,
) {
    let expiration = (minutes > 0).then(|| now_ms() + i64::from(minutes) * 60_000);
    let mut ban = BanDef::server(reason, expiration);
    ban.user_ids.push(target.user_id);
    ban.severity = severity;
    let admin_state = admin
        .and_then(|a| world.get::<Session>(a))
        .map(|s| (s.user_id, s.name.clone()));
    ban.banning_admin = admin_state.as_ref().map(|(user, _)| *user);
    let admin_name = admin_state
        .map(|(_, name)| name)
        .unwrap_or_else(|| loc(world, "system-user", &[]));
    let started = crate::database::spawn_db(
        world,
        {
            let ban = ban.clone();
            move |db| async move { db.add_ban(&ban).await }
        },
        move |world, result| {
            let ban = match result {
                Ok(ban) => ban,
                Err(error) => {
                    reply_later(world, admin, format!("бан не сохранён: {error}"), true);
                    return;
                }
            };
            announce_ban(world, &ban, &admin_name, &target, reason_of(&ban));
            kick_matching_connected_players(world, &ban);
        },
    );
    if !started {
        reply_later(world, admin, "нет базы данных".into(), true);
    }
}

fn reason_of(ban: &BanDef) -> String {
    ban.reason.clone()
}

fn announce_ban(
    world: &mut World,
    ban: &BanDef,
    admin: &str,
    target: &LocatedPlayer,
    reason: String,
) {
    let expires = match ban.expiration_time {
        Some(end) => crate::bans::format_utc(end),
        None => loc(world, "server-ban-string-never", &[]),
    };
    let text = loc(
        world,
        "server-ban-string-no-pii",
        &[
            ("admin", LocArg::from(admin.to_owned())),
            ("severity", LocArg::from(severity_name(ban.severity))),
            ("expires", LocArg::from(expires)),
            (
                "name",
                LocArg::from(format!("{} ({})", target.user_name, target.user_id)),
            ),
            ("reason", LocArg::from(reason)),
        ],
    );
    crate::chat::send_admin_alert(world, &text);
}

fn ban_command(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let default = world
        .resource::<astrocore_console::Config>()
        .get_text(&SERVER_BAN_DEFAULT_SEVERITY);
    let mut severity = parse_severity(&default).unwrap_or(NoteSeverity::High);
    let (target, reason, minutes) = match args {
        [target, reason] => (target, reason, 0),
        [target, reason, minutes] | [target, reason, minutes, _] => {
            let Ok(minutes) = minutes.parse::<u32>() else {
                shell.write_line(loc(
                    world,
                    "cmd-ban-invalid-minutes",
                    &[("minutes", LocArg::from(minutes.clone()))],
                ));
                return;
            };
            if let [.., text] = args
                && args.len() == 4
            {
                match parse_severity(text) {
                    Some(value) => severity = value,
                    None => {
                        shell.write_line(loc(
                            world,
                            "cmd-ban-invalid-severity",
                            &[("severity", LocArg::from(text.clone()))],
                        ));
                        return;
                    }
                }
            }
            (target, reason, minutes)
        }
        _ => {
            shell.write_line(loc(world, "cmd-ban-invalid-arguments", &[]));
            shell.write_line(loc(world, "cmd-ban-help", &[]));
            return;
        }
    };
    let admin = shell.player;
    let reason = reason.clone();
    locate_player(world, target, move |world, located| match located {
        Some(target) => create_server_ban(world, admin, target, &reason, minutes, severity),
        None => {
            let text = loc(world, "cmd-ban-player", &[]);
            reply_later(world, admin, text, true);
        }
    });
}

fn pardon_command(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let [id] = args else {
        shell.write_line(loc(world, "cmd-pardon-help", &[]));
        return;
    };
    let Ok(ban_id) = id.parse::<i64>() else {
        shell.write_line(loc(
            world,
            "cmd-pardon-unable-to-parse",
            &[
                ("id", LocArg::from(id.clone())),
                ("help", LocArg::from(loc(world, "cmd-pardon-help", &[]))),
            ],
        ));
        return;
    };
    let admin = shell.player;
    let admin_user = admin
        .and_then(|a| world.get::<Session>(a))
        .map(|s| s.user_id);
    let started = crate::database::spawn_db(
        world,
        move |db| async move {
            let Some(ban) = db.get_ban(ban_id).await.ok().flatten() else {
                return Err(None);
            };
            if let Some(unban) = ban.unban {
                return Err(Some(unban));
            }
            db.add_unban(&UnbanDef {
                ban_id,
                unbanning_admin: admin_user,
                unban_time: now_ms(),
            })
            .await
            .map_err(|_| None)
        },
        move |world, result| {
            let line = match result {
                Ok(()) => loc(world, "cmd-pardon-success", &[("id", LocArg::from(ban_id))]),
                Err(Some(unban)) => match unban.unbanning_admin {
                    Some(admin) => loc(
                        world,
                        "cmd-pardon-already-pardoned-specific",
                        &[
                            ("admin", LocArg::from(admin.to_string())),
                            (
                                "time",
                                LocArg::from(crate::bans::format_utc(unban.unban_time)),
                            ),
                        ],
                    ),
                    None => loc(world, "cmd-pardon-already-pardoned", &[]),
                },
                Err(None) => format!("No ban found with id {ban_id}"),
            };
            reply_later(world, admin, line, false);
        },
    );
    if !started {
        shell.write_error("нет базы данных");
    }
}

fn banlist_command(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let [target] = args else {
        shell.write_error(loc(world, "cmd-banlist-help", &[]));
        return;
    };
    let admin = shell.player;
    locate_player(world, target, move |world, located| {
        let Some(target) = located else {
            let text = loc(world, "cmd-ban-player", &[]);
            reply_later(world, admin, text, true);
            return;
        };
        let started = crate::database::spawn_db(
            world,
            move |db| async move {
                db.get_bans(
                    target.last_address,
                    Some(target.user_id),
                    false,
                    crate::bans::BanType::Server,
                )
                .await
                .map(|bans| (bans, target.user_name))
            },
            move |world, result| match result {
                Ok((bans, _)) if !bans.is_empty() => {
                    for ban in bans {
                        let line = format!("{}: {}", ban.id.unwrap_or_default(), ban.reason);
                        reply_later(world, admin, line, false);
                    }
                }
                Ok((_, name)) => {
                    let line = loc(world, "cmd-banlist-empty", &[("user", LocArg::from(name))]);
                    reply_later(world, admin, line, false);
                }
                Err(error) => reply_later(world, admin, error.to_string(), true),
            },
        );
        if !started {
            reply_later(world, admin, "нет базы данных".into(), true);
        }
    });
}

// Сущности.

fn attached(world: &World, shell: &ConsoleShell) -> Option<Entity> {
    world.get::<Session>(shell.player?)?.attached
}

/// `spawn <прототип>` у себя, `spawn <прототип> <сущность>` у сущности,
/// `spawn <прототип> <x> <y>` на своей карте (`SpawnCommand`).
fn spawn_command(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let at: Option<EntityCoordinates> = match args {
        [_] => attached(world, shell).and_then(|e| world.coordinates(e)),
        [_, target] => {
            crate::map_commands::find_target(world, target).and_then(|e| world.coordinates(e))
        }
        [_, x, y] => {
            let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>()) else {
                shell.write_error("x и y — числа");
                return;
            };
            attached(world, shell)
                .and_then(|e| world.map_coordinates(e))
                .and_then(|own| {
                    use astrocore_map::MapWorldExt;
                    world.snap_to_grid_or_map(MapCoordinates {
                        position: Vec2::new(x, y),
                        ..own
                    })
                })
        }
        _ => {
            shell.write_error("spawn <прототип> [сущность | x y]");
            return;
        }
    };
    let Some(at) = at else {
        shell.write_error("некуда поставить: нет своей сущности или цели");
        return;
    };
    match world.spawn_attached(&args[0], at) {
        Ok(entity) => {
            let net = world
                .get::<astrocore_net::NetEntity>(entity)
                .map(|n| n.0.to_string())
                .unwrap_or_default();
            shell.write_line(format!("{} {net}", args[0]));
        }
        Err(error) => shell.write_error(format!("{}: {error}", args[0])),
    }
}

/// `delete <сущность>` (`DeleteCommand`).
fn delete_command(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let [target] = args else {
        shell.write_error("delete <сущность>");
        return;
    };
    match crate::map_commands::find_target(world, target) {
        Some(entity) => world.delete_entity(entity),
        None => shell.write_error(format!("«{target}» — нет такой сущности")),
    }
}

/// `rejuvenate [сущность...]` (`RejuvenateCommand`): без целей — себя.
fn rejuvenate_command(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let targets: Vec<Entity> = if args.is_empty() {
        attached(world, shell).into_iter().collect()
    } else {
        args.iter()
            .filter_map(|a| crate::map_commands::find_target(world, a))
            .collect()
    };
    if targets.is_empty() {
        shell.write_error(loc(
            world,
            "rejuvenate-command-no-entity-attached-message",
            &[],
        ));
        return;
    }
    for target in targets {
        crate::damage::DamageableWorldExt::rejuvenate(world, target);
    }
    shell.write_line(loc(world, "rejuvenate-command-self-heal-message", &[]));
}

pub fn install(world: &mut World, side: Side) {
    if !side.is_server() {
        return;
    }
    world.register_cvar(&SERVER_BAN_DEFAULT_SEVERITY);
    for (name, description, help, execute) in [
        (
            "kick",
            "Выгнать игрока.",
            "kick <игрок> [причина]",
            kick_command as astrocore_console::CommandFn,
        ),
        (
            "ban",
            "Забанить игрока.",
            "ban <игрок или идентификатор> <причина> [минуты, 0 — навсегда] [тяжесть]",
            ban_command,
        ),
        (
            "pardon",
            "Снять бан.",
            "pardon <номер бана>",
            pardon_command,
        ),
        (
            "banlist",
            "Действующие баны игрока.",
            "banlist <игрок или идентификатор>",
            banlist_command,
        ),
        (
            "spawn",
            "Создать сущность.",
            "spawn <прототип> [сущность | x y]",
            spawn_command,
        ),
        (
            "delete",
            "Удалить сущность.",
            "delete <сущность>",
            delete_command,
        ),
        (
            "rejuvenate",
            "Полностью вылечить.",
            "rejuvenate [сущность...]",
            rejuvenate_command,
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
}

#[cfg(test)]
mod tests;
