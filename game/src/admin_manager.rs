//! Права администраторов: флаги, данные администратора сессии из базы (ранг, свои флаги,
//! «снятые» права), хост на машине сервера (`console.loginlocal`), снятие и возврат прав
//! (`deadmin`, `readmin`), невидимость, права на команды по флагам, статус прав клиенту,
//! объявления о входе и выходе администраторов. Образец — `AdminManager`, `AdminFlags`,
//! `AdminFlagsHelper`, `AdminData`, `MsgUpdateAdminStatus`, `engineCommandPerms.yml`,
//! атрибуты `[AdminCommand]`/`[AnyCommand]` команд, команды `deadmin`, `readmin`,
//! `adminwho`, `promotehost` SS14 @ 2f4a2aad.
//!
//! Нет пока: окна прав (`PermissionsEui`), toolshed, метрик.

use std::collections::{BTreeMap, BTreeSet};

use astrocore_console::{
    CVarDef, CVarFlags, Config, ConfigWorldExt, Console, ConsoleCommand, ConsoleShell,
    ConsoleWorldExt,
};
use astrocore_ecs::EventWorldExt;
use astrocore_loc::LocArg;
use astrocore_net::{
    NetEventWorldExt, NetTarget, NetUserId, PlayerConnected, PlayerDisconnected, ServerNet, Session,
};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::examine::loc;

/// Флаги прав (`AdminFlags`). Имена — как в базе SS14.
pub mod flags {
    pub const NONE: u32 = 0;
    pub const ADMIN: u32 = 1 << 0;
    pub const BAN: u32 = 1 << 1;
    pub const DEBUG: u32 = 1 << 2;
    pub const FUN: u32 = 1 << 3;
    pub const PERMISSIONS: u32 = 1 << 4;
    pub const SERVER: u32 = 1 << 5;
    pub const SPAWN: u32 = 1 << 6;
    pub const VAREDIT: u32 = 1 << 7;
    pub const MAPPING: u32 = 1 << 8;
    pub const LOGS: u32 = 1 << 9;
    pub const ROUND: u32 = 1 << 10;
    pub const QUERY: u32 = 1 << 11;
    pub const ADMINHELP: u32 = 1 << 12;
    pub const VIEWNOTES: u32 = 1 << 13;
    pub const EDITNOTES: u32 = 1 << 14;
    pub const MASSBAN: u32 = 1 << 15;
    pub const STEALTH: u32 = 1 << 16;
    pub const ADMINCHAT: u32 = 1 << 17;
    pub const PII: u32 = 1 << 18;
    pub const MODERATOR: u32 = 1 << 19;
    pub const ADMINWHO: u32 = 1 << 20;
    pub const NAMECOLOR: u32 = 1 << 21;
    pub const HOST: u32 = 1 << 31;

    /// Имя флага → флаг (`AdminFlagsHelper.NameToFlag`).
    pub const NAMES: [(&str, u32); 23] = [
        ("ADMIN", ADMIN),
        ("BAN", BAN),
        ("DEBUG", DEBUG),
        ("FUN", FUN),
        ("PERMISSIONS", PERMISSIONS),
        ("SERVER", SERVER),
        ("SPAWN", SPAWN),
        ("VAREDIT", VAREDIT),
        ("MAPPING", MAPPING),
        ("LOGS", LOGS),
        ("ROUND", ROUND),
        ("QUERY", QUERY),
        ("ADMINHELP", ADMINHELP),
        ("VIEWNOTES", VIEWNOTES),
        ("EDITNOTES", EDITNOTES),
        ("MASSBAN", MASSBAN),
        ("STEALTH", STEALTH),
        ("ADMINCHAT", ADMINCHAT),
        ("PII", PII),
        ("MODERATOR", MODERATOR),
        ("ADMINWHO", ADMINWHO),
        ("NAMECOLOR", NAMECOLOR),
        ("HOST", HOST),
    ];

    /// Все права (`AdminFlagsHelper.Everything`).
    pub fn everything() -> u32 {
        NAMES.iter().fold(0, |all, (_, flag)| all | flag)
    }

    /// Флаг по имени без учёта регистра.
    pub fn named(name: &str) -> Option<u32> {
        NAMES
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
            .map(|(_, f)| *f)
    }

    /// Флаги по именам (`NamesToFlags`); неизвестные пропускаются.
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> u32 {
        names
            .into_iter()
            .filter_map(named)
            .fold(0, |all, f| all | f)
    }

    /// Имена флагов.
    pub fn names(flags: u32) -> Vec<&'static str> {
        NAMES
            .iter()
            .filter(|(_, f)| flags & f != 0)
            .map(|(n, _)| *n)
            .collect()
    }
}

pub const ANNOUNCE_LOGIN: CVarDef<bool> = CVarDef::new(
    "admin.announce_login",
    true,
    CVarFlags::SERVERONLY,
    "Объявлять администраторам о входе администратора.",
);
pub const ANNOUNCE_LOGOUT: CVarDef<bool> = CVarDef::new(
    "admin.announce_logout",
    true,
    CVarFlags::SERVERONLY,
    "Объявлять администраторам о выходе администратора.",
);
pub const USE_CUSTOM_NAMES: CVarDef<bool> = CVarDef::new(
    "admin.use_custom_names_admin_rank",
    true,
    CVarFlags::SERVERONLY,
    "Своё звание администратора вместо имени ранга.",
);
pub const LOGIN_HOST_USER: CVarDef<&str> = CVarDef::new(
    "console.login_host_user",
    "",
    CVarFlags::SERVERONLY.union(CVarFlags::ARCHIVE),
    "Игрок с этим именем получает права хоста.",
);

/// Данные администратора (`AdminData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminData {
    pub flags: u32,
    pub title: Option<String>,
    /// Права действуют (не `deadmin`).
    pub active: bool,
    pub stealth: bool,
}

impl AdminData {
    /// `HasFlag`: все биты есть (пустой флаг — у любого).
    pub fn has_flag(&self, flag: u32) -> bool {
        self.flags & flag == flag
    }
}

struct AdminReg {
    data: AdminData,
    rank_id: Option<i64>,
    /// Хост или повышенный (`IsSpecialLogin`).
    special: bool,
}

/// Администраторы сессий. Ресурс сервера.
#[derive(Resource, Default)]
pub struct Admins {
    admins: BTreeMap<Entity, AdminReg>,
    promoted: BTreeSet<NetUserId>,
}

/// Статус прав игроку (`MsgUpdateAdminStatus`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminStatus {
    pub admin: Option<AdminData>,
    pub available_commands: Vec<String>,
    /// Идентификатор игрока: клиенту он нужен, чтобы узнавать свои сообщения (в Robust он
    /// приходит при рукопожатии).
    pub user_id: Option<NetUserId>,
}

/// Статус прав на клиенте. Ресурс мира клиента.
#[derive(Resource, Debug, Clone, Default)]
pub struct ClientAdmin {
    pub admin: Option<AdminData>,
    pub available_commands: Vec<String>,
    pub user_id: Option<NetUserId>,
    pub version: u64,
}

impl ClientAdmin {
    /// Активный администратор с флагом.
    pub fn has_flag(&self, flag: u32) -> bool {
        self.admin
            .as_ref()
            .is_some_and(|a| a.active && a.has_flag(flag))
    }
}

/// Права сменились (`OnPermsChanged`). Широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct AdminPermsChanged {
    pub session: Entity,
    /// Флаги активного администратора; `None` — не администратор.
    pub flags: Option<u32>,
}

// Права команд.

/// Команды для всех игроков (`[AnyCommand]`).
pub fn any_commands() -> Vec<&'static str> {
    let mut list: Vec<&str> = vec!["help", "readmin"];
    list.extend(crate::game_ticker::ANY_COMMANDS);
    list.extend(crate::chat::CHAT_COMMANDS);
    list.extend(crate::ghost::GHOST_COMMANDS);
    list.extend(crate::ghost_roles::GHOST_ROLE_COMMANDS);
    list
}

/// Команда → флаги, любого из которых хватает (`[AdminCommand]`, `engineCommandPerms.yml`).
pub fn command_flags(command: &str) -> Option<&'static [u32]> {
    use flags::*;
    Some(match command {
        "deadmin" => &[NONE],
        "adminwho" => &[ADMINWHO],
        "asay" => &[ADMINCHAT],
        "dsay" | "kick" | "respawn" => &[MODERATOR],
        "startround"
        | "endround"
        | "restartround"
        | "restartroundnow"
        | "delaystart"
        | "setgamepreset"
        | "forcepreset"
        | "golobby"
        | "toggledisallowlatejoin"
        | "forcemap" => &[ROUND],
        "tp" | "tpto" | "aghost" | "makeghostrole" | "rejuvenate" => &[ADMIN],
        "addmap" | "loadmap" | "loadgrid" | "mapinit" | "savemap" | "savegrid" | "tpgrid"
        | "rmmap" => &[MAPPING],
        "querymappaused" => &[DEBUG, MAPPING],
        "lsmap" => &[DEBUG],
        "mapping" | "toggleautosave" => &[SERVER, MAPPING],
        "pausemap" | "unpausemap" => &[SERVER, MAPPING],
        "explosion" => &[FUN],
        "addgas" | "echo" => &[DEBUG],
        "ban" | "pardon" | "banlist" => &[BAN],
        "adminlogs" => &[LOGS],
        "spawn" => &[SPAWN],
        "delete" => &[SPAWN, SERVER, DEBUG],
        "cvar" | "promotehost" => &[HOST],
        _ => return None,
    })
}

/// Может ли сессия выполнить команду (`CanCommand`).
pub fn can_command(world: &World, session: Entity, command: &str) -> bool {
    if any_commands().contains(&command) {
        return true;
    }
    let Some(required) = command_flags(command) else {
        return false;
    };
    let Some(data) = admin_data(world, session, false) else {
        return false;
    };
    required.iter().any(|flag| data.has_flag(*flag))
}

/// Проверка прав консоли: сервер может всё.
pub fn permission(world: &World, shell: &ConsoleShell, command: &str) -> bool {
    match shell.player {
        None => true,
        Some(session) => can_command(world, session, command),
    }
}

/// Команды, доступные сессии.
pub fn available_commands(world: &World, session: Entity) -> Vec<String> {
    let mut names: Vec<String> = world
        .get_resource::<Console>()
        .map(|c| c.iter().map(|command| command.name.to_owned()).collect())
        .unwrap_or_default();
    names.retain(|name| can_command(world, session, name));
    names
}

// Данные.

/// Данные администратора сессии (`GetAdminData`).
pub fn admin_data(world: &World, session: Entity, include_deadmin: bool) -> Option<AdminData> {
    let reg = world.get_resource::<Admins>()?.admins.get(&session)?;
    (reg.data.active || include_deadmin).then(|| reg.data.clone())
}

/// Активный администратор (`IsAdmin`).
pub fn is_admin(world: &World, session: Entity) -> bool {
    admin_data(world, session, false).is_some()
}

/// Активный администратор с флагом (`HasAdminFlag`).
pub fn has_flag(world: &World, session: Entity, flag: u32) -> bool {
    admin_data(world, session, false).is_some_and(|d| d.has_flag(flag))
}

/// Сессии активных администраторов (`ActiveAdmins`).
pub fn active_admins(world: &World) -> Vec<Entity> {
    world
        .get_resource::<Admins>()
        .map(|a| {
            a.admins
                .iter()
                .filter(|(_, r)| r.data.active)
                .map(|(s, _)| *s)
                .collect()
        })
        .unwrap_or_default()
}

fn is_local(world: &World, session: Entity) -> bool {
    let Some(conn) = world.get::<Session>(session).map(|s| s.conn) else {
        return false;
    };
    world
        .get_resource::<ServerNet>()
        .and_then(|net| net.address(conn))
        .is_some_and(|address| address.ip().is_loopback())
}

/// Хост (`promoteHost`): с машины сервера при `console.loginlocal`, повышенный командой или
/// с именем `console.login_host_user`.
fn is_host(world: &World, session: Entity) -> bool {
    let Some(state) = world.get::<Session>(session) else {
        return false;
    };
    let config = world.resource::<Config>();
    let host_user = config.get_text(&LOGIN_HOST_USER);
    (is_local(world, session) && config.get(&crate::map_commands::LOGIN_LOCAL))
        || world
            .get_resource::<Admins>()
            .is_some_and(|a| a.promoted.contains(&state.user_id))
        || (!host_user.is_empty() && state.name == host_user)
}

fn host_data(world: &World) -> AdminData {
    AdminData {
        flags: flags::everything(),
        title: Some(loc(world, "admin-manager-admin-data-host-title", &[])),
        active: true,
        stealth: false,
    }
}

/// Данные из записи базы (`LoadAdminDataCore`): приостановленный — не администратор.
pub fn data_from_record(
    world: &World,
    record: &crate::admins::AdminRecord,
) -> Option<(AdminData, Option<i64>)> {
    if record.suspended {
        return None;
    }
    let flags = flags::from_names(record.effective_flags().iter().map(String::as_str));
    let title = match (&record.title, &record.rank) {
        (Some(title), _) if world.resource::<Config>().get(&USE_CUSTOM_NAMES) => {
            Some(title.clone())
        }
        (_, Some(rank)) => Some(rank.name.clone()),
        _ => None,
    };
    Some((
        AdminData {
            flags,
            title,
            active: !record.deadminned,
            stealth: false,
        },
        record.rank.as_ref().map(|r| r.id),
    ))
}

fn announce(world: &mut World, key: &str, args: &[(&str, LocArg)]) {
    let text = loc(world, key, args);
    crate::chat::send_admin_announcement(world, &text);
}

fn server_message(world: &mut World, session: Entity, key: &str) {
    let text = loc(world, key, &[]);
    crate::chat::dispatch_server_message(world, session, &text);
}

/// Отправить статус прав игроку (`UpdateAdminStatus`).
pub fn update_admin_status(world: &mut World, session: Entity) {
    let status = AdminStatus {
        admin: admin_data(world, session, true),
        available_commands: available_commands(world, session),
        user_id: world.get::<Session>(session).map(|s| s.user_id),
    };
    world.send_net_event(NetTarget::Session(session), &status);
}

fn perms_changed(world: &mut World, session: Entity) {
    let flags = admin_data(world, session, false).map(|d| d.flags);
    world.raise_broadcast(&mut AdminPermsChanged { session, flags });
    update_admin_status(world, session);
}

/// Зарегистрировать администратора сессии (`LoginAdminMaybe`).
pub fn login_admin(
    world: &mut World,
    session: Entity,
    data: AdminData,
    rank_id: Option<i64>,
    special: bool,
) {
    if world.get::<Session>(session).is_none() {
        return;
    }
    let active = data.active;
    world
        .get_resource_or_insert_with(Admins::default)
        .admins
        .insert(
            session,
            AdminReg {
                data,
                rank_id,
                special,
            },
        );
    if active {
        if world.cvar(&ANNOUNCE_LOGIN) {
            let name = world
                .get::<Session>(session)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            announce(
                world,
                "admin-manager-admin-login-message",
                &[("name", LocArg::from(name))],
            );
        }
        perms_changed(world, session);
    } else {
        update_admin_status(world, session);
    }
}

/// Загрузить права игрока (`LoadAdminData`): хост — сразу, иначе из базы.
pub fn load_admin(world: &mut World, session: Entity) {
    if is_host(world, session) {
        let data = host_data(world);
        login_admin(world, session, data, None, true);
        return;
    }
    let Some(user) = world.get::<Session>(session).map(|s| s.user_id) else {
        return;
    };
    let started = crate::database::spawn_db(
        world,
        move |db| async move { db.get_admin_data_for(user).await },
        move |world, result| {
            let Ok(Some(record)) = result else {
                update_admin_status(world, session);
                return;
            };
            match data_from_record(world, &record) {
                Some((data, rank)) => login_admin(world, session, data, rank, false),
                None => update_admin_status(world, session),
            }
        },
    );
    if !started {
        update_admin_status(world, session);
    }
}

/// Снять права на время (`DeAdmin`).
pub fn deadmin(world: &mut World, session: Entity) -> bool {
    let Some(mut admins) = world.get_resource_mut::<Admins>() else {
        return false;
    };
    let Some(reg) = admins.admins.get_mut(&session) else {
        return false;
    };
    if !reg.data.active {
        return true;
    }
    reg.data.active = false;
    let (special, user) = (
        reg.special,
        world.get::<Session>(session).map(|s| s.user_id),
    );
    let name = world
        .get::<Session>(session)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    announce(
        world,
        "admin-manager-self-de-admin-message",
        &[("exAdminName", LocArg::from(name))],
    );
    server_message(world, session, "admin-manager-became-normal-player-message");
    if !special && let Some(user) = user {
        crate::database::spawn_db(
            world,
            move |db| async move { db.update_admin_deadminned(user, true).await },
            |_, _| {},
        );
    }
    perms_changed(world, session);
    true
}

/// Вернуть права (`ReAdmin`).
pub fn readmin(world: &mut World, session: Entity) -> bool {
    let Some(mut admins) = world.get_resource_mut::<Admins>() else {
        return false;
    };
    let Some(reg) = admins.admins.get_mut(&session) else {
        return false;
    };
    if reg.data.active {
        return true;
    }
    reg.data.active = true;
    let (special, user) = (
        reg.special,
        world.get::<Session>(session).map(|s| s.user_id),
    );
    server_message(world, session, "admin-manager-became-admin-message");
    let name = world
        .get::<Session>(session)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    announce(
        world,
        "admin-manager-self-re-admin-message",
        &[("newAdminName", LocArg::from(name))],
    );
    if !special && let Some(user) = user {
        crate::database::spawn_db(
            world,
            move |db| async move { db.update_admin_deadminned(user, false).await },
            |_, _| {},
        );
    }
    perms_changed(world, session);
    true
}

/// Сделать игрока хостом (`PromoteHost`).
pub fn promote_host(world: &mut World, session: Entity) {
    let Some(user) = world.get::<Session>(session).map(|s| s.user_id) else {
        return;
    };
    world
        .get_resource_or_insert_with(Admins::default)
        .promoted
        .insert(user);
    let data = host_data(world);
    let was_admin = world
        .get_resource::<Admins>()
        .is_some_and(|a| a.admins.contains_key(&session));
    world.resource_mut::<Admins>().admins.insert(
        session,
        AdminReg {
            data,
            rank_id: None,
            special: true,
        },
    );
    server_message(
        world,
        session,
        if was_admin {
            "admin-manager-admin-permissions-updated-message"
        } else {
            "admin-manager-became-admin-message"
        },
    );
    perms_changed(world, session);
}

/// Перечитать права игроков ранга (`ReloadAdminsWithRank`) — после правки ранга.
pub fn reload_admins_with_rank(world: &mut World, rank_id: i64) {
    let sessions: Vec<Entity> = world
        .get_resource::<Admins>()
        .map(|a| {
            a.admins
                .iter()
                .filter(|(_, r)| r.rank_id == Some(rank_id))
                .map(|(s, _)| *s)
                .collect()
        })
        .unwrap_or_default();
    for session in sessions {
        world.resource_mut::<Admins>().admins.remove(&session);
        load_admin(world, session);
    }
}

// Команды.

fn session_by_name(world: &mut World, name: &str) -> Option<Entity> {
    world
        .query::<(Entity, &Session)>()
        .iter(world)
        .find(|(_, s)| s.name.eq_ignore_ascii_case(name))
        .map(|(e, _)| e)
}

fn deadmin_command(world: &mut World, shell: &mut ConsoleShell, _args: &[String]) {
    let Some(session) = shell.player else {
        shell.write_line("Только для игроков.");
        return;
    };
    if !deadmin(world, session) {
        shell.write_line(loc(world, "cmd-deadmin-not-admin", &[]));
    }
}

fn readmin_command(world: &mut World, shell: &mut ConsoleShell, _args: &[String]) {
    let Some(session) = shell.player else {
        shell.write_line("Только для игроков.");
        return;
    };
    if !readmin(world, session) {
        shell.write_line(loc(world, "cmd-readmin-not-admin", &[]));
    }
}

/// Кто из администраторов в игре (`AdminWhoCommand`).
fn adminwho_command(world: &mut World, shell: &mut ConsoleShell, _args: &[String]) {
    let viewer_stealth = shell
        .player
        .is_some_and(|s| has_flag(world, s, flags::STEALTH));
    let entries: Vec<(String, AdminData)> = world
        .get_resource::<Admins>()
        .map(|a| {
            a.admins
                .iter()
                .filter_map(|(session, reg)| {
                    let name = world.get::<Session>(*session)?.name.clone();
                    Some((name, reg.data.clone()))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut lines = Vec::new();
    for (name, data) in entries {
        if data.stealth && !viewer_stealth {
            continue;
        }
        let mut line = name;
        if let Some(title) = &data.title {
            line.push_str(&format!(" - {title}"));
        }
        if !data.active {
            line.push_str(" [Deadminned]");
        }
        if data.stealth {
            line.push_str(" (S)");
        }
        lines.push(line);
    }
    lines.sort();
    for line in lines {
        shell.write_line(line);
    }
}

fn promotehost_command(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let [name] = args else {
        shell.write_line(loc(world, "shell-need-exactly-one-argument", &[]));
        return;
    };
    match session_by_name(world, name) {
        Some(session) => promote_host(world, session),
        None => shell.write_line(loc(world, "shell-target-player-does-not-exist", &[])),
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_net_event::<AdminStatus>();
    if !side.is_server() {
        world.init_resource::<ClientAdmin>();
        world.subscribe_net_event::<AdminStatus>(|world, event| {
            let status = event.event.clone();
            let mut admin = world.resource_mut::<ClientAdmin>();
            admin.admin = status.admin;
            admin.available_commands = status.available_commands;
            admin.user_id = status.user_id;
            admin.version += 1;
        });
        return;
    }
    world.register_cvar(&ANNOUNCE_LOGIN);
    world.register_cvar(&ANNOUNCE_LOGOUT);
    world.register_cvar(&USE_CUSTOM_NAMES);
    world.register_cvar(&LOGIN_HOST_USER);
    world.init_resource::<Admins>();
    world
        .get_resource_or_insert_with(Console::default)
        .permission = Some(permission);
    for (name, description, help, execute) in [
        (
            "deadmin",
            "Временно снять свои права администратора.",
            "deadmin",
            deadmin_command as astrocore_console::CommandFn,
        ),
        (
            "readmin",
            "Вернуть свои права администратора.",
            "readmin",
            readmin_command,
        ),
        (
            "adminwho",
            "Администраторы в игре.",
            "adminwho",
            adminwho_command,
        ),
        (
            "promotehost",
            "Дать игроку права хоста.",
            "promotehost <игрок>",
            promotehost_command,
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
    // Вход: права загружаются (`PlayerStatusChanged` → `InGame`).
    world.subscribe_broadcast::<PlayerConnected>(|world, event| {
        let session = event.session;
        load_admin(world, session);
    });
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        let session = event.session;
        let removed = world
            .get_resource_mut::<Admins>()
            .and_then(|mut a| a.admins.remove(&session));
        if let Some(reg) = removed
            && reg.data.active
            && !reg.data.stealth
            && world.cvar(&ANNOUNCE_LOGOUT)
        {
            let name = world
                .get::<Session>(session)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            announce(
                world,
                "admin-manager-admin-logout-message",
                &[("name", LocArg::from(name))],
            );
        }
    });
}

#[cfg(test)]
mod tests;
