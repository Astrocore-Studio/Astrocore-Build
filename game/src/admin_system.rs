//! Список игроков для администраторов: кто на сервере, кем играет, на какой должности,
//! в сети ли и играл ли в этом раунде. Сервер держит список и рассылает изменения активным
//! администраторам; клиент администратора хранит его для меню. Образец — `AdminSystem`
//! сервера и клиента, `PlayerInfo`, `FullPlayerListEvent`, `PlayerInfoChangedEvent`
//! SS14 @ 2f4a2aad.
//!
//! Отличие от SS14: сетевой номер новой сущности появляется только на стадии синхронизации,
//! поэтому изменение игрока, сменившего тело, рассылается со следующего такта. Нет пока
//! антагонистов, видов ролей и наигранного времени (нет их систем).

use std::collections::{BTreeMap, BTreeSet};

use astrocore_ecs::{EventWorldExt, GameWorldExt, Update};
use astrocore_loc::EntityNameWorldExt;
use astrocore_net::{
    NetEntity, NetEventWorldExt, NetTarget, NetUserId, PlayerAttached, PlayerConnected,
    PlayerDetached, PlayerDisconnected, Session,
};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_manager::{self, AdminPermsChanged};

/// Сведения об игроке (`PlayerInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub username: String,
    pub character_name: String,
    pub identity_name: String,
    pub starting_job: String,
    pub antag: bool,
    pub net_entity: Option<NetEntity>,
    pub user_id: NetUserId,
    pub connected: bool,
    pub active_this_round: bool,
}

/// Весь список (`FullPlayerListEvent`); пустой — права сняты.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullPlayerList {
    pub players: Vec<PlayerInfo>,
}

/// Изменился один игрок (`PlayerInfoChangedEvent`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerInfoChanged {
    pub info: PlayerInfo,
}

/// Список игроков на сервере. Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct AdminPlayerList {
    pub players: BTreeMap<NetUserId, PlayerInfo>,
    /// Игроки, получавшие тело в этом раунде (`_roundActivePlayers`).
    pub round_active: BTreeSet<NetUserId>,
    /// Ждут рассылки: их новое тело ещё без сетевого номера.
    dirty: BTreeSet<NetUserId>,
}

/// Список игроков у администратора. Ресурс мира клиента.
#[derive(Resource, Debug, Default, Clone)]
pub struct ClientPlayerList {
    pub players: BTreeMap<NetUserId, PlayerInfo>,
    pub version: u64,
}

/// Сведения об игроке сейчас (`GetPlayerInfo`). `None` — тело ещё без сетевого номера.
fn player_info(world: &mut World, session: Entity, connected: bool) -> Option<PlayerInfo> {
    let (user_id, username, attached) = {
        let s = world.get::<Session>(session)?;
        (s.user_id, s.name.clone(), s.attached)
    };
    let attached = attached.filter(|_| connected);
    let net_entity = match attached {
        Some(entity) => Some(*world.get::<NetEntity>(entity)?),
        None => None,
    };
    let character_name = attached.map(|e| world.entity_name(e)).unwrap_or_default();
    let starting_job = crate::game_ticker::job_title(world, user_id).unwrap_or_default();
    let active_this_round = world
        .resource::<AdminPlayerList>()
        .round_active
        .contains(&user_id);
    Some(PlayerInfo {
        username,
        identity_name: character_name.clone(),
        character_name,
        starting_job,
        antag: false,
        net_entity,
        user_id,
        connected,
        active_this_round,
    })
}

fn send_to_admins<E: Serialize + 'static>(world: &mut World, event: &E) {
    for admin in admin_manager::active_admins(world) {
        world.send_net_event(NetTarget::Session(admin), event);
    }
}

/// Обновить игрока и разослать (`UpdatePlayerList`).
pub fn update_player_list(world: &mut World, session: Entity, connected: bool) {
    let Some(user) = world.get::<Session>(session).map(|s| s.user_id) else {
        return;
    };
    let Some(info) = player_info(world, session, connected) else {
        world.resource_mut::<AdminPlayerList>().dirty.insert(user);
        return;
    };
    let mut list = world.resource_mut::<AdminPlayerList>();
    list.dirty.remove(&user);
    list.players.insert(user, info.clone());
    send_to_admins(world, &PlayerInfoChanged { info });
}

/// Весь список одному администратору (`SendFullPlayerList`).
pub fn send_full_player_list(world: &mut World, session: Entity) {
    let players = world
        .resource::<AdminPlayerList>()
        .players
        .values()
        .cloned()
        .collect();
    world.send_net_event(NetTarget::Session(session), &FullPlayerList { players });
}

fn flush_dirty(world: &mut World) {
    let dirty: Vec<NetUserId> = match world.get_resource::<AdminPlayerList>() {
        Some(list) if !list.dirty.is_empty() => list.dirty.iter().copied().collect(),
        _ => return,
    };
    for user in dirty {
        match crate::mind::session_of_user(world, user) {
            Some(session) => update_player_list(world, session, true),
            None => {
                world.resource_mut::<AdminPlayerList>().dirty.remove(&user);
            }
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_net_event::<FullPlayerList>();
    world.register_net_event::<PlayerInfoChanged>();
    if !side.is_server() {
        world.init_resource::<ClientPlayerList>();
        world.subscribe_net_event::<FullPlayerList>(|world, event| {
            let players = event
                .event
                .players
                .iter()
                .map(|p| (p.user_id, p.clone()))
                .collect();
            let mut list = world.resource_mut::<ClientPlayerList>();
            list.players = players;
            list.version += 1;
        });
        world.subscribe_net_event::<PlayerInfoChanged>(|world, event| {
            let info = event.event.info.clone();
            let mut list = world.resource_mut::<ClientPlayerList>();
            list.players.insert(info.user_id, info);
            list.version += 1;
        });
        return;
    }
    world.init_resource::<AdminPlayerList>();
    world.subscribe_broadcast::<PlayerConnected>(|world, event| {
        let session = event.session;
        update_player_list(world, session, true);
    });
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        let session = event.session;
        update_player_list(world, session, false);
    });
    world.subscribe_broadcast::<PlayerAttached>(|world, event| {
        let session = event.session;
        if let Some(user) = world.get::<Session>(session).map(|s| s.user_id) {
            world
                .resource_mut::<AdminPlayerList>()
                .round_active
                .insert(user);
        }
        update_player_list(world, session, true);
    });
    world.subscribe_broadcast::<PlayerDetached>(|world, event| {
        let session = event.session;
        update_player_list(world, session, true);
    });
    world.subscribe_broadcast::<AdminPermsChanged>(|world, event| {
        let session = event.session;
        if event.flags.is_some() {
            send_full_player_list(world, session);
        } else {
            world.send_net_event(
                NetTarget::Session(session),
                &FullPlayerList {
                    players: Vec::new(),
                },
            );
        }
    });
    world.subscribe_broadcast::<crate::game_ticker::RoundRestartCleanup>(|world, _| {
        world.resource_mut::<AdminPlayerList>().round_active.clear();
        let active: Vec<NetUserId> = world
            .resource::<AdminPlayerList>()
            .players
            .values()
            .filter(|p| p.active_this_round)
            .map(|p| p.user_id)
            .collect();
        for user in active {
            if let Some(info) = world
                .resource_mut::<AdminPlayerList>()
                .players
                .get_mut(&user)
            {
                info.active_this_round = false;
            }
        }
        let players = world
            .resource::<AdminPlayerList>()
            .players
            .values()
            .cloned()
            .collect();
        send_to_admins(world, &FullPlayerList { players });
    });
    world.add_game_systems(Update, flush_dirty);
}

#[cfg(test)]
mod tests;
