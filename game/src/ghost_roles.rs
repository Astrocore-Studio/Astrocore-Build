//! Роли призраков: свободные тела (мыши, дроны, животные) и «спавнеры», которые призрак
//! может занять; роль сразу достаётся первому или разыгрывается среди желающих (лотерея
//! продлевается с каждым новым участником до предела); список ролей у призраков, их число
//! на кнопке; команды `ghostroles` и `makeghostrole`. Образец — `GhostRoleSystem`,
//! `GhostRoleComponent`, `GhostTakeoverAvailableComponent`, `GhostRoleMobSpawnerComponent`,
//! `GhostRoleRaffleComponent`, `GhostRoleRaffleConfig`, `GhostRoleRaffleSettingsPrototype`,
//! `RngGhostRoleRaffleDecider`, `GhostRolesEuiMessages`, `MakeGhostRoleCommand`,
//! `CCVars.GhostQuickLottery` SS14 @ 2f4a2aad.
//!
//! Нет пока: ролей разума и должности роли (фаза 7), правил роли с задержкой
//! подтверждения, запретов на роли и требований по времени, выбора вида у спавнера
//! (`SelectablePrototypes`), радио ролей, следования за ролью.

use std::collections::{BTreeMap, BTreeSet};

use astrocore_console::{
    CVarDef, CVarFlags, ConfigWorldExt, ConsoleCommand, ConsoleShell, ConsoleWorldExt,
};
use astrocore_ecs::{
    ComponentShutdown, ComponentStartup, EventWorldExt, GameWorldExt, LifecycleWorldExt, MapInit,
    MetaData, Paused, RegistryWorldExt, Update,
};
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::SpawnWorldExt;
use astrocore_net::{
    NetEntities, NetEntity, NetEvent, NetEventWorldExt, NetTarget, PlayerAttached,
    PlayerDisconnected, Session,
};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::examine::loc;
use crate::ghost::Ghost;
use crate::mind::{self, MindAdded, MindContainer, MindRemoved};
use crate::mobs::{MobState, MobStateChanged};

pub const GHOST_QUICK_LOTTERY: CVarDef<bool> = CVarDef::new(
    "ghost.quick_lottery",
    false,
    CVarFlags::SERVER,
    "Розыгрыш ролей призраков за секунду (для отладки).",
);

/// Команды ролей, доступные всем игрокам.
pub const GHOST_ROLE_COMMANDS: [&str; 1] = ["ghostroles"];

/// Настройки розыгрыша (`GhostRoleRaffleConfig`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RaffleConfig {
    pub settings: String,
    pub decider: String,
}

impl Default for RaffleConfig {
    fn default() -> Self {
        Self {
            settings: "default".into(),
            decider: "default".into(),
        }
    }
}

/// Роль призрака (`GhostRoleComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GhostRole {
    pub name: String,
    pub description: String,
    pub rules: String,
    pub make_sentient: bool,
    #[serde(rename = "prob")]
    pub probability: f32,
    pub allow_speech: bool,
    pub allow_movement: bool,
    #[serde(rename = "reregister")]
    pub reregister_on_ghost: bool,
    pub raffle: Option<RaffleConfig>,
    pub job: Option<String>,
    #[serde(skip)]
    pub taken: bool,
    #[serde(skip)]
    pub identifier: u32,
}

impl Default for GhostRole {
    fn default() -> Self {
        Self {
            name: "Unknown".into(),
            description: "Unknown".into(),
            rules: "ghost-role-component-default-rules".into(),
            make_sentient: true,
            probability: 1.0,
            allow_speech: true,
            allow_movement: false,
            reregister_on_ghost: true,
            raffle: None,
            job: None,
            taken: false,
            identifier: 0,
        }
    }
}

/// Тело можно занять как роль (`GhostTakeoverAvailableComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GhostTakeoverAvailable {}

/// Спавнер роли: занятие создаёт существо (`GhostRoleMobSpawnerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GhostRoleMobSpawner {
    pub delete_on_spawn: bool,
    pub available_takeovers: i32,
    #[serde(skip)]
    pub current_takeovers: i32,
    pub prototype: Option<String>,
}

impl Default for GhostRoleMobSpawner {
    fn default() -> Self {
        Self {
            delete_on_spawn: true,
            available_takeovers: 1,
            current_takeovers: 0,
            prototype: None,
        }
    }
}

/// Идущий розыгрыш (`GhostRoleRaffleComponent`). Времена — секунды.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct GhostRoleRaffle {
    pub identifier: u32,
    /// Сессии участников.
    pub current_members: BTreeSet<Entity>,
    pub all_members: BTreeSet<Entity>,
    pub countdown: f64,
    pub cumulative_time: f64,
    pub join_extends_duration_by: f64,
    pub max_duration: f64,
}

/// Роль занимают (`TakeGhostRoleEvent`). На сущности роли.
#[derive(Debug, Clone, Copy)]
pub struct TakeGhostRole {
    pub player: Entity,
    pub took_role: bool,
}

/// Вид роли для списка (`GhostRoleKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GhostRoleKind {
    FirstComeFirstServe,
    RaffleReady,
    RaffleInProgress,
    RaffleJoined,
}

/// Роль в списке (`GhostRoleInfo`). Время розыгрыша — оставшиеся секунды: часы клиента с
/// сервером не сверены (как отсчёт лобби, ADR-0012).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GhostRoleInfo {
    pub identifier: u32,
    pub name: String,
    pub description: String,
    pub rules: String,
    pub kind: GhostRoleKind,
    pub raffle_player_count: u32,
    pub raffle_seconds_left: f32,
}

/// Сообщения ролей (`GhostRolesEuiMessages`, `GhostUpdateGhostRoleCountEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GhostRolesMessage {
    /// Клиент: открыть список.
    Open,
    Close,
    Request(u32),
    LeaveRaffle(u32),
    /// Сервер: список ролей.
    Roles(Vec<GhostRoleInfo>),
    /// Сервер: сколько ролей.
    Count(u32),
}

/// Роли и розыгрыши. Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct GhostRoles {
    pub roles: BTreeMap<u32, Entity>,
    pub raffles: BTreeMap<u32, Entity>,
    next_identifier: u32,
    /// Сессии с открытым списком.
    pub open: BTreeSet<Entity>,
    count_dirty: bool,
}

/// Состояние ролей на клиенте. Ресурс мира клиента.
#[derive(Resource, Debug, Clone, Default)]
pub struct ClientGhostRoles {
    pub count: u32,
    pub roles: Vec<GhostRoleInfo>,
    pub open: bool,
    /// Когда пришёл список, по часам клиента: от него отсчитываются розыгрыши.
    pub received_at: std::time::Duration,
    pub version: u64,
}

fn roles(world: &mut World) -> bevy_ecs::world::Mut<'_, GhostRoles> {
    world.get_resource_or_insert_with(GhostRoles::default)
}

fn paused(world: &World, entity: Entity) -> bool {
    world.get::<Paused>(entity).is_some()
}

// Регистр.

/// Зарегистрировать роль (`RegisterGhostRole`).
pub fn register_ghost_role(world: &mut World, entity: Entity) {
    if roles(world).roles.values().any(|e| *e == entity) {
        return;
    }
    let id = {
        let mut state = roles(world);
        state.next_identifier += 1;
        let id = state.next_identifier;
        state.roles.insert(id, entity);
        state.count_dirty = true;
        id
    };
    if let Some(mut role) = world.get_mut::<GhostRole>(entity) {
        role.identifier = id;
    }
    update_all_lists(world);
}

/// Снять роль с регистра (`UnregisterGhostRole`).
pub fn unregister_ghost_role(world: &mut World, entity: Entity) {
    let Some(id) = world.get::<GhostRole>(entity).map(|r| r.identifier) else {
        return;
    };
    if roles(world).roles.get(&id) != Some(&entity) {
        return;
    }
    {
        let mut state = roles(world);
        state.roles.remove(&id);
        state.count_dirty = true;
    }
    if world.get::<GhostRoleRaffle>(entity).is_some() {
        remove_raffle(world, entity);
    }
    update_all_lists(world);
}

/// Сколько ролей сейчас (`GetGhostRoleCount`).
pub fn ghost_role_count(world: &mut World) -> u32 {
    let entities: Vec<Entity> = roles(world).roles.values().copied().collect();
    entities.into_iter().filter(|e| !paused(world, *e)).count() as u32
}

/// Список ролей для игрока (`GetGhostRolesInfo`).
pub fn ghost_roles_info(world: &mut World, player: Option<Entity>) -> Vec<GhostRoleInfo> {
    let entries: Vec<(u32, Entity)> = roles(world).roles.iter().map(|(i, e)| (*i, *e)).collect();
    let mut infos = Vec::new();
    for (id, entity) in entries {
        if paused(world, entity) {
            continue;
        }
        let Some(role) = world.get::<GhostRole>(entity).cloned() else {
            continue;
        };
        let raffle = world.get::<GhostRoleRaffle>(entity).cloned();
        let kind = match (&role.raffle, &raffle) {
            (None, _) => GhostRoleKind::FirstComeFirstServe,
            (Some(_), None) => GhostRoleKind::RaffleReady,
            (Some(_), Some(r)) if player.is_some_and(|p| r.current_members.contains(&p)) => {
                GhostRoleKind::RaffleJoined
            }
            (Some(_), Some(_)) => GhostRoleKind::RaffleInProgress,
        };
        infos.push(GhostRoleInfo {
            identifier: id,
            name: loc(world, &role.name, &[]),
            description: loc(world, &role.description, &[]),
            rules: loc(world, &role.rules, &[]),
            kind,
            raffle_player_count: raffle
                .as_ref()
                .map_or(0, |r| r.current_members.len() as u32),
            raffle_seconds_left: raffle.as_ref().map_or(0.0, |r| r.countdown as f32),
        });
    }
    infos
}

/// Обновить открытые списки (`UpdateAllEui`).
pub fn update_all_lists(world: &mut World) {
    let open: Vec<Entity> = roles(world).open.iter().copied().collect();
    for session in open {
        if world.get::<Session>(session).is_none() {
            roles(world).open.remove(&session);
            continue;
        }
        let infos = ghost_roles_info(world, Some(session));
        world.send_net_event(
            NetTarget::Session(session),
            &GhostRolesMessage::Roles(infos),
        );
    }
}

// Розыгрыш.

/// Начать розыгрыш (`OnRaffleInit`).
fn start_raffle(world: &mut World, entity: Entity, identifier: u32) -> bool {
    let Some(config) = world
        .get::<GhostRole>(entity)
        .and_then(|r| r.raffle.clone())
    else {
        return false;
    };
    let settings = world
        .get_resource::<Prototypes>()
        .and_then(|p| p.resolved_node("ghostRoleRaffleSettings", &config.settings))
        .and_then(|n| n.get("settings").cloned());
    let number = |key: &str| {
        settings
            .as_ref()
            .and_then(|s| s.get(key))
            .and_then(|n| n.as_str())
            .and_then(|t| t.trim().parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let (initial, extend, max) = (
        number("initialDuration"),
        number("joinExtendsDurationBy"),
        number("maxDuration"),
    );
    if max < initial {
        eprintln!("роль призрака: неверный розыгрыш (предел меньше начала)");
        if let Some(mut role) = world.get_mut::<GhostRole>(entity) {
            role.raffle = None;
        }
        return false;
    }
    let countdown = if world.cvar(&GHOST_QUICK_LOTTERY) {
        1.0
    } else {
        initial
    };
    world.entity_mut(entity).insert(GhostRoleRaffle {
        identifier,
        current_members: BTreeSet::new(),
        all_members: BTreeSet::new(),
        countdown,
        cumulative_time: initial,
        join_extends_duration_by: extend,
        max_duration: max,
    });
    roles(world).raffles.insert(identifier, entity);
    true
}

fn remove_raffle(world: &mut World, entity: Entity) {
    if let Some(raffle) = world.get::<GhostRoleRaffle>(entity).cloned() {
        roles(world).raffles.remove(&raffle.identifier);
    }
    world.entity_mut(entity).remove::<GhostRoleRaffle>();
    update_all_lists(world);
}

/// Вступить в розыгрыш или начать его (`JoinRaffle`).
fn join_raffle(world: &mut World, player: Entity, identifier: u32) {
    let Some(entity) = roles(world).roles.get(&identifier).copied() else {
        return;
    };
    if world.get::<GhostRoleRaffle>(entity).is_none() && !start_raffle(world, entity, identifier) {
        return;
    }
    let Some(mut raffle) = world.get_mut::<GhostRoleRaffle>(entity) else {
        return;
    };
    if !raffle.current_members.insert(player) {
        return;
    }
    // Новый участник продлевает розыгрыш, но не дольше предела.
    if raffle.all_members.insert(player)
        && raffle.all_members.len() > 1
        && raffle.cumulative_time + raffle.join_extends_duration_by <= raffle.max_duration
    {
        raffle.countdown += raffle.join_extends_duration_by;
        raffle.cumulative_time += raffle.join_extends_duration_by;
    }
    update_all_lists(world);
}

/// Выйти из розыгрыша (`LeaveRaffle`).
pub fn leave_raffle(world: &mut World, player: Entity, identifier: u32) {
    let Some(entity) = roles(world).raffles.get(&identifier).copied() else {
        return;
    };
    let removed = world
        .get_mut::<GhostRoleRaffle>(entity)
        .is_some_and(|mut r| r.current_members.remove(&player));
    if removed {
        update_all_lists(world);
    }
}

/// Выйти из всех розыгрышей (`LeaveAllRaffles`).
pub fn leave_all_raffles(world: &mut World, player: Entity) {
    let raffles: Vec<Entity> = roles(world).raffles.values().copied().collect();
    let mut changed = false;
    for entity in raffles {
        changed |= world
            .get_mut::<GhostRoleRaffle>(entity)
            .is_some_and(|mut r| r.current_members.remove(&player));
    }
    if changed {
        update_all_lists(world);
    }
}

/// Попросить роль (`Request`): розыгрыш или сразу занять.
pub fn request(world: &mut World, player: Entity, identifier: u32) {
    let Some(entity) = roles(world).roles.get(&identifier).copied() else {
        return;
    };
    if world
        .get::<GhostRole>(entity)
        .is_some_and(|r| r.raffle.is_some())
    {
        join_raffle(world, player, identifier);
    } else {
        takeover(world, player, identifier);
    }
}

/// Занять роль (`Takeover`).
pub fn takeover(world: &mut World, player: Entity, identifier: u32) -> bool {
    let Some(entity) = roles(world).roles.get(&identifier).copied() else {
        return false;
    };
    let mut event = TakeGhostRole {
        player,
        took_role: false,
    };
    world.raise(entity, &mut event);
    if !event.took_role {
        return false;
    }
    let name = world
        .get::<Session>(player)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let role = world
        .get::<GhostRole>(entity)
        .map(|r| r.name.clone())
        .unwrap_or_default();
    let role = loc(world, &role, &[]);
    let users: Vec<_> = world
        .get::<Session>(player)
        .map(|s| s.user_id)
        .into_iter()
        .collect();
    add_admin_log(
        world,
        LogType::GhostRoleTaken,
        LogImpact::Low,
        &format!("{name} took the {role} ghost role"),
        &[],
        &users,
    );
    roles(world).open.remove(&player);
    world.send_net_event(NetTarget::Session(player), &GhostRolesMessage::Close);
    true
}

/// Победитель розыгрыша (`TryTakeover`): в игре и всё ещё призрак.
fn try_takeover(world: &mut World, player: Entity, identifier: u32) -> bool {
    let ghost = world
        .get::<Session>(player)
        .and_then(|s| s.attached)
        .is_some_and(|e| world.get::<Ghost>(e).is_some());
    if !ghost {
        return false;
    }
    if takeover(world, player, identifier) {
        leave_all_raffles(world, player);
        return true;
    }
    false
}

/// Розыгрыши идут (`UpdateRaffles`): пустые отменяются, законченные выбирают победителя
/// случайно среди участников (`RngGhostRoleRaffleDecider`).
pub fn update_raffles(world: &mut World, dt: f64) {
    let raffles: Vec<Entity> = roles(world).raffles.values().copied().collect();
    for entity in raffles {
        if paused(world, entity) {
            continue;
        }
        let Some(raffle) = world.get::<GhostRoleRaffle>(entity).cloned() else {
            continue;
        };
        if raffle.current_members.is_empty() {
            remove_raffle(world, entity);
            continue;
        }
        let countdown = raffle.countdown - dt;
        if let Some(mut stored) = world.get_mut::<GhostRoleRaffle>(entity) {
            stored.countdown = countdown;
        }
        if countdown > 0.0 {
            continue;
        }
        let (identifier, mut members) = (
            raffle.identifier,
            raffle.current_members.iter().copied().collect::<Vec<_>>(),
        );
        // Случайный порядок; первый, кто может занять роль, побеждает.
        for i in (1..members.len()).rev() {
            let j = crate::random::random(world).range(0, i as i32 + 1) as usize;
            members.swap(i, j);
        }
        for member in members {
            if try_takeover(world, member, identifier) {
                break;
            }
        }
        remove_raffle(world, entity);
    }
}

// Занятие тел.

/// Сделать существо разумным (`MakeSentient`): разум, движение, речь.
pub fn make_sentient(world: &mut World, entity: Entity, allow_movement: bool, allow_speech: bool) {
    if world.get::<MindContainer>(entity).is_none() {
        world.entity_mut(entity).insert(MindContainer::default());
    }
    if allow_movement
        && world
            .get::<astrocore_movement::InputMover>(entity)
            .is_none()
    {
        world.add_comp(entity, astrocore_movement::InputMover::default());
    }
    if allow_speech && world.get::<crate::chat::Speech>(entity).is_none() {
        world.add_comp(entity, crate::chat::Speech::default());
    }
}

/// Новый разум игрока в теле роли (`GhostRoleInternalCreateMindAndTransfer`): прежний
/// разум стирается, если игрок не просто посещает призрака.
pub fn create_mind_and_transfer(world: &mut World, player: Entity, mob: Entity) {
    let Some(user) = world.get::<Session>(player).map(|s| s.user_id) else {
        return;
    };
    if let Some(old) = mind::mind_of_user(world, user)
        && world
            .get::<mind::Mind>(old)
            .is_some_and(|m| m.visiting_entity.is_none())
    {
        mind::wipe_mind(world, old);
    }
    let name = world.entity_name(mob);
    let new_mind = mind::create_mind(world, Some(user), Some(&name));
    mind::transfer_to(world, new_mind, Some(mob), false, true);
}

fn can_take(world: &World, entity: Entity) -> bool {
    world.get::<GhostRole>(entity).is_some_and(|r| !r.taken) && !paused(world, entity)
}

/// Занять свободное тело (`OnTakeoverTakeRole`).
fn on_takeover(world: &mut World, entity: Entity, event: &mut TakeGhostRole) {
    if !can_take(world, entity) {
        event.took_role = false;
        return;
    }
    let Some(role) = world.get::<GhostRole>(entity).cloned() else {
        return;
    };
    if let Some(mut stored) = world.get_mut::<GhostRole>(entity) {
        stored.taken = true;
    }
    if mind::mind_of(world, entity).is_some() {
        event.took_role = false;
        return;
    }
    if role.make_sentient {
        make_sentient(world, entity, role.allow_movement, role.allow_speech);
    }
    create_mind_and_transfer(world, event.player, entity);
    unregister_ghost_role(world, entity);
    event.took_role = true;
}

/// Занять спавнер (`OnSpawnerTakeRole`): создаётся существо.
fn on_spawner(world: &mut World, entity: Entity, event: &mut TakeGhostRole) {
    if !can_take(world, entity) {
        event.took_role = false;
        return;
    }
    let (Some(role), Some(spawner)) = (
        world.get::<GhostRole>(entity).cloned(),
        world.get::<GhostRoleMobSpawner>(entity).cloned(),
    ) else {
        return;
    };
    let Some(prototype) = spawner.prototype.filter(|p| !p.is_empty()) else {
        eprintln!("спавнер роли без прототипа");
        return;
    };
    let Some(at) = mind::grid_or_map_coordinates(world, entity) else {
        return;
    };
    let Ok(mob) = world.spawn_attached(&prototype, at) else {
        return;
    };
    if role.make_sentient {
        make_sentient(world, mob, role.allow_movement, role.allow_speech);
    }
    if world.get::<MindContainer>(mob).is_none() {
        world.entity_mut(mob).insert(MindContainer::default());
    }
    create_mind_and_transfer(world, event.player, mob);
    let takeovers = {
        let Some(mut stored) = world.get_mut::<GhostRoleMobSpawner>(entity) else {
            return;
        };
        stored.current_takeovers += 1;
        stored.current_takeovers
    };
    event.took_role = true;
    if takeovers < spawner.available_takeovers {
        return;
    }
    if let Some(mut stored) = world.get_mut::<GhostRole>(entity) {
        stored.taken = true;
    }
    if spawner.delete_on_spawn {
        world.queue_delete(entity);
    } else {
        unregister_ghost_role(world, entity);
    }
}

// Команды и сообщения.

fn open_list(world: &mut World, session: Entity) {
    roles(world).open.insert(session);
    let infos = ghost_roles_info(world, Some(session));
    world.send_net_event(
        NetTarget::Session(session),
        &GhostRolesMessage::Roles(infos),
    );
}

fn receive(world: &mut World, event: &mut NetEvent<GhostRolesMessage>) {
    let Some(session) = event.session else {
        return;
    };
    match event.event.clone() {
        GhostRolesMessage::Open => open_list(world, session),
        GhostRolesMessage::Close => {
            roles(world).open.remove(&session);
        }
        GhostRolesMessage::Request(id) => request(world, session, id),
        GhostRolesMessage::LeaveRaffle(id) => leave_raffle(world, session, id),
        GhostRolesMessage::Roles(_) | GhostRolesMessage::Count(_) => {}
    }
}

fn ghostroles_command(world: &mut World, shell: &mut ConsoleShell, _args: &[String]) {
    match shell.player {
        Some(session) => open_list(world, session),
        None => shell.write_line("Только для игроков."),
    }
}

/// `makeghostrole <сущность> <имя> <описание> [правила]` (`MakeGhostRoleCommand`).
fn makeghostrole_command(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    if !(3..=4).contains(&args.len()) {
        shell.write_line("makeghostrole <сущность> <имя> <описание> [правила]");
        return;
    }
    let Some(entity) = args[0]
        .parse::<u32>()
        .ok()
        .and_then(|n| world.get_resource::<NetEntities>()?.entity(NetEntity(n)))
        .filter(|e| world.is_alive(*e))
    else {
        shell.write_line(format!("Нет сущности {}", args[0]));
        return;
    };
    if mind::mind_of(world, entity).is_some_and(|m| {
        world
            .get::<mind::Mind>(m)
            .is_some_and(|m| m.user_id.is_some())
    }) {
        shell.write_line("У сущности уже есть игрок.");
        return;
    }
    if world.get::<GhostRole>(entity).is_some() {
        shell.write_line("Сущность уже роль призрака.");
        return;
    }
    let mut role = GhostRole {
        name: args[1].clone(),
        description: args[2].clone(),
        ..GhostRole::default()
    };
    if let Some(rules) = args.get(3) {
        role.rules.clone_from(rules);
    }
    world.entity_mut(entity).insert(GhostTakeoverAvailable {});
    world.add_comp(entity, role);
    shell.write_line(format!("{} теперь роль призрака «{}».", args[0], args[1]));
}

fn update(world: &mut World) {
    let dt = world
        .get_resource::<astrocore_ecs::GameTime>()
        .map_or(0.0, |t| 1.0 / f64::from(t.tick_rate.max(1)));
    update_raffles(world, dt);
    // Число ролей — всем, когда изменилось (`UpdateGhostRoleCount`).
    if std::mem::take(&mut roles(world).count_dirty) {
        let count = ghost_role_count(world);
        world.send_net_event(NetTarget::All, &GhostRolesMessage::Count(count));
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_cvar(&GHOST_QUICK_LOTTERY);
    world.register_data_component::<GhostRole>();
    world.register_data_component::<GhostTakeoverAvailable>();
    world.register_data_component::<GhostRoleMobSpawner>();
    world.register_net_event::<GhostRolesMessage>();
    if !side.is_server() {
        world.init_resource::<ClientGhostRoles>();
        world.subscribe_net_event::<GhostRolesMessage>(|world, event| {
            let now = world
                .get_resource::<astrocore_ecs::GameTime>()
                .map(astrocore_ecs::GameTime::cur_time)
                .unwrap_or_default();
            let mut state = world.resource_mut::<ClientGhostRoles>();
            match &event.event {
                GhostRolesMessage::Roles(roles) => {
                    state.roles.clone_from(roles);
                    state.count = roles.len() as u32;
                    state.open = true;
                    state.received_at = now;
                }
                GhostRolesMessage::Count(count) => state.count = *count,
                GhostRolesMessage::Close => state.open = false,
                _ => return,
            }
            state.version += 1;
        });
        return;
    }
    world.init_resource::<GhostRoles>();
    world.register_command(ConsoleCommand {
        name: "ghostroles",
        description: "Открыть список ролей призраков.",
        help: "ghostroles",
        execute: ghostroles_command,
        complete: None,
    });
    world.register_command(ConsoleCommand {
        name: "makeghostrole",
        description: "Сделать сущность ролью призрака.",
        help: "makeghostrole <сущность> <имя> <описание> [правила]",
        execute: makeghostrole_command,
        complete: None,
    });
    world.subscribe_net_event::<GhostRolesMessage>(receive);
    // Роль с вероятностью (`OnMapInit`), регистрация (`OnRoleStartup`, `OnRoleShutdown`).
    world.subscribe::<GhostRole, MapInit>(|world, entity, _| {
        let probability = world
            .get::<GhostRole>(entity)
            .map_or(1.0, |r| r.probability);
        if probability < 1.0 && !crate::random::random(world).prob(probability) {
            world.remove_comp::<GhostRole>(entity);
        }
    });
    world.subscribe::<GhostRole, ComponentStartup>(|world, entity, _| {
        register_ghost_role(world, entity);
    });
    world.subscribe::<GhostRole, ComponentShutdown>(|world, entity, _| {
        unregister_ghost_role(world, entity);
    });
    world.subscribe::<GhostTakeoverAvailable, TakeGhostRole>(on_takeover);
    world.subscribe::<GhostRoleMobSpawner, TakeGhostRole>(on_spawner);
    // Тело занято или освобождено (`OnMindAdded`, `OnMindRemoved`).
    world.subscribe::<GhostTakeoverAvailable, MindAdded>(|world, entity, _| {
        if let Some(mut role) = world.get_mut::<GhostRole>(entity) {
            role.taken = true;
        }
        unregister_ghost_role(world, entity);
    });
    world.subscribe::<GhostTakeoverAvailable, MindRemoved>(|world, entity, _| {
        let terminating = world
            .get::<MetaData>(entity)
            .is_some_and(|m| m.stage >= astrocore_ecs::LifeStage::Terminating);
        let Some(mut role) = world.get_mut::<GhostRole>(entity) else {
            return;
        };
        if !role.reregister_on_ghost || terminating {
            return;
        }
        role.taken = false;
        register_ghost_role(world, entity);
    });
    // Живое тело — роль, в крите или мёртвое — нет (`OnMobStateChanged`).
    world.subscribe::<GhostTakeoverAvailable, MobStateChanged>(|world, entity, event| match event
        .new_state
    {
        MobState::Alive => {
            if world.get::<GhostRole>(entity).is_some_and(|r| !r.taken) {
                register_ghost_role(world, entity);
            }
        }
        MobState::Critical | MobState::Dead => unregister_ghost_role(world, entity),
        _ => {}
    });
    // Не призрак — из розыгрышей и списка (`OnPlayerAttached`); вышел — из розыгрышей.
    world.subscribe_broadcast::<PlayerAttached>(|world, event| {
        if world.get::<Ghost>(event.entity).is_some() {
            let count = ghost_role_count(world);
            world.send_net_event(
                NetTarget::Session(event.session),
                &GhostRolesMessage::Count(count),
            );
            return;
        }
        if roles(world).open.remove(&event.session) {
            world.send_net_event(NetTarget::Session(event.session), &GhostRolesMessage::Close);
        }
        leave_all_raffles(world, event.session);
    });
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        roles(world).open.remove(&event.session);
        leave_all_raffles(world, event.session);
    });
    world.subscribe_broadcast::<crate::game_ticker::RoundRestartCleanup>(|world, _| {
        world.insert_resource(GhostRoles::default());
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
