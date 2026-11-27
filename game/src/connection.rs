//! Вход игроков на сервер с базой: гость получает постоянный идентификатор по имени, сервер
//! решает, пускать ли, пишет журнал подключений и запись игрока, затем впускает или
//! отказывает. Пока база отвечает, такт идёт (ADR-0011, ADR-0017 движка). Образец —
//! `ConnectionManager` (`NetMgrOnConnecting`, `ShouldDeny`, `AssignUserIdCallback`) SS14
//! @ 2f4a2aad.
//!
//! Нет пока: проверки банов (следующая часть 6.1), белого списка, заполненности сервера,
//! «бункера» для новичков, обхода проверок администраторами.

use astrocore_console::ConfigWorldExt;
use astrocore_ecs::EventWorldExt;
use astrocore_net::{ConnId, NetUserId, PlayerConnecting, complete_connection};
use bevy_ecs::world::World;

use crate::Side;
use crate::database::{
    ConnectionDenyReason, Db, DbResult, GAME_PERSIST_GUESTS, address_text, spawn_db,
};

/// Решение о подключении: отказ с причиной для журнала и текстом для игрока.
pub type Denial = (ConnectionDenyReason, String);

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

/// Пускать ли (`ShouldDeny`). Проверок пока нет.
pub async fn should_deny(
    _db: &Db,
    _user_id: NetUserId,
    _address: &str,
) -> DbResult<Option<Denial>> {
    Ok(None)
}

/// Подключение целиком (`NetMgrOnConnecting`): идентификатор игрока или отказ.
pub async fn on_connecting(
    db: &Db,
    name: &str,
    address: &str,
    persist: bool,
) -> DbResult<Result<NetUserId, String>> {
    let user_id = assign_user_id(db, name, persist).await?;
    let server_id = db.server_id().await?;
    let denial = should_deny(db, user_id, address).await?;
    let reason = denial.as_ref().map(|(reason, _)| *reason);
    db.add_connection_log(user_id, name, address, reason, server_id)
        .await?;
    if let Some((_, message)) = denial {
        return Ok(Err(message));
    }
    if persist {
        db.update_player_record(user_id, name, address).await?;
    }
    Ok(Ok(user_id))
}

fn finish(world: &mut World, conn: ConnId, result: DbResult<Result<NetUserId, String>>) {
    let decision = result.unwrap_or_else(|error| {
        eprintln!("вход игрока: {error}");
        Err("Ошибка сервера при входе. Попробуйте позже.".into())
    });
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
        let address = address_text(event.address);
        let conn = event.conn;
        let started = spawn_db(
            world,
            move |db| async move { on_connecting(&db, &name, &address, persist).await },
            move |world, result| finish(world, conn, result),
        );
        event.deferred = started;
    });
}

#[cfg(test)]
mod tests;
