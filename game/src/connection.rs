//! Вход игроков на сервер с базой: гость получает постоянный идентификатор по имени, сервер
//! решает, пускать ли, пишет журнал подключений и запись игрока, затем впускает или
//! отказывает. Пока база отвечает, такт идёт (ADR-0011, ADR-0017 движка). Образец —
//! `ConnectionManager` (`NetMgrOnConnecting`, `ShouldDeny`, `AssignUserIdCallback`) SS14
//! @ 2f4a2aad.
//!
//! Баны на сервер проверяются по идентификатору и адресу (`bans.rs`).
//!
//! Нет пока: банов по железу, белого списка, заполненности сервера, «бункера» для новичков,
//! обхода проверок администраторами.

use std::net::IpAddr;

use astrocore_console::ConfigWorldExt;
use astrocore_ecs::EventWorldExt;
use astrocore_net::{ConnId, NetUserId, PlayerConnecting, complete_connection};
use bevy_ecs::world::World;

use crate::Side;
use crate::bans::{BanDef, BanType, format_ban_message};
use crate::database::{
    ConnectionDenyReason, Db, DbResult, GAME_PERSIST_GUESTS, address_text, spawn_db,
};

/// Отказ во входе (`ShouldDeny`): причина для журнала, баны, под которые попал игрок, и
/// текст для него (у бана текст собирается в мире — из перевода и настроек).
#[derive(Debug, Clone, PartialEq)]
pub enum Denial {
    Ban(Vec<BanDef>),
    Other(ConnectionDenyReason, String),
}

impl Denial {
    fn reason(&self) -> ConnectionDenyReason {
        match self {
            Denial::Ban(_) => ConnectionDenyReason::Ban,
            Denial::Other(reason, _) => *reason,
        }
    }
}

/// Итог входа: впустить с идентификатором или отказать.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Admit(NetUserId),
    Deny(Denial),
}

/// Выдать гостю постоянный идентификатор по имени (`AssignUserIdCallback`).
pub async fn assign_user_id(db: &Db, name: &str, persist: bool) -> DbResult<NetUserId> {
    if !persist {
        return Ok(NetUserId::random());
    }
    if let Some(user_id) = db.get_assigned_user_id(name).await? {
        return Ok(user_id);
    }
    let user_id = NetUserId::random();
    db.assign_user_id(name, user_id).await?;
    Ok(user_id)
}

/// Пускать ли (`ShouldDeny`): действующие баны на сервер по идентификатору и адресу.
pub async fn should_deny(
    db: &Db,
    user_id: NetUserId,
    address: Option<IpAddr>,
) -> DbResult<Option<Denial>> {
    let bans = db
        .get_bans(address, Some(user_id), false, BanType::Server)
        .await?;
    if !bans.is_empty() {
        return Ok(Some(Denial::Ban(bans)));
    }
    Ok(None)
}

/// Подключение целиком (`NetMgrOnConnecting`).
pub async fn on_connecting(
    db: &Db,
    name: &str,
    address: Option<IpAddr>,
    persist: bool,
) -> DbResult<Decision> {
    let user_id = assign_user_id(db, name, persist).await?;
    let server_id = db.server_id().await?;
    let text = address_text(address);
    let denial = should_deny(db, user_id, address).await?;
    let log = db
        .add_connection_log(
            user_id,
            name,
            &text,
            denial.as_ref().map(Denial::reason),
            server_id,
        )
        .await?;
    if let Some(denial) = denial {
        if let Denial::Ban(bans) = &denial {
            db.add_server_ban_hits(log, bans).await?;
        }
        return Ok(Decision::Deny(denial));
    }
    if persist {
        db.update_player_record(user_id, name, &text).await?;
    }
    Ok(Decision::Admit(user_id))
}

fn finish(world: &mut World, conn: ConnId, result: DbResult<Decision>) {
    let decision = match result {
        Ok(Decision::Admit(user_id)) => Ok(user_id),
        Ok(Decision::Deny(Denial::Ban(bans))) => Err(bans
            .first()
            .map(|ban| format_ban_message(world, ban))
            .unwrap_or_default()),
        Ok(Decision::Deny(Denial::Other(_, message))) => Err(message),
        Err(error) => {
            eprintln!("вход игрока: {error}");
            Err("Ошибка сервера при входе. Попробуйте позже.".into())
        }
    };
    complete_connection(world, conn, decision);
}

pub fn install(world: &mut World, side: Side) {
    if !side.is_server() {
        return;
    }
    world.subscribe_broadcast::<PlayerConnecting>(|world, event| {
        if event.denied.is_some() || event.deferred {
            return;
        }
        let persist = world.cvar(&GAME_PERSIST_GUESTS);
        let name = event.name.clone();
        let address = event.address.map(|a| a.ip());
        let conn = event.conn;
        let started = spawn_db(
            world,
            move |db| async move { on_connecting(&db, &name, address, persist).await },
            move |world, result| finish(world, conn, result),
        );
        event.deferred = started;
    });
}

#[cfg(test)]
mod tests;
