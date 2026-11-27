use astrocore_console::ConfigWorldExt;
use astrocore_ecs::GameWorldExt;
use astrocore_net::{ClientMessage, Incoming, NetRegistry, ServerNet, Session};

use super::*;
use crate::database::Database;

/// Приветствие клиента с верным набором компонентов.
fn hello(world: &mut World, conn: ConnId, name: &str) {
    let fingerprint = world.resource::<NetRegistry>().fingerprint();
    let bytes = postcard::to_allocvec(&ClientMessage::Hello {
        name: name.into(),
        fingerprint,
    })
    .unwrap();
    world
        .resource_mut::<ServerNet>()
        .incoming
        .push(Incoming::Packet(conn, bytes));
}

/// Такты, пока база не ответит и соединение не решится.
fn settle(world: &mut World, conn: ConnId) {
    for _ in 0..400 {
        world.run_tick();
        if !world.resource::<ServerNet>().is_pending(conn) {
            world.run_tick();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("база не ответила");
}

fn user_of(world: &World, conn: ConnId) -> Option<NetUserId> {
    let session = world.resource::<ServerNet>().session(conn)?;
    world.get::<Session>(session).map(|s| s.user_id)
}

#[test]
fn guest_keeps_user_id_between_visits_and_is_logged() {
    let mut world = crate::testing::world_with("");
    world.insert_resource(Database::in_memory().unwrap());
    hello(&mut world, ConnId(1), "Urist");
    world.run_tick();
    // Пока база думает, сессии нет.
    assert!(world.resource::<ServerNet>().is_pending(ConnId(1)));
    settle(&mut world, ConnId(1));
    let first = user_of(&world, ConnId(1)).expect("впущен");

    world
        .resource_mut::<ServerNet>()
        .incoming
        .push(Incoming::Disconnected(ConnId(1)));
    world.run_tick();
    hello(&mut world, ConnId(2), "Urist");
    settle(&mut world, ConnId(2));
    assert_eq!(user_of(&world, ConnId(2)), Some(first));

    let database = world.resource::<Database>();
    let db = database.db().clone();
    let (record, logs) = database.block_on(async {
        (
            db.get_player_record_by_user_id(first).await.unwrap(),
            db.connection_logs(first).await.unwrap(),
        )
    });
    assert_eq!(record.unwrap().last_seen_user_name, "Urist");
    assert_eq!(logs.len(), 2);
    assert!(logs.iter().all(|l| l.denied.is_none()));
}

#[test]
fn without_persistence_guests_are_new_each_time_and_not_stored() {
    let mut world = crate::testing::world_with("");
    world.insert_resource(Database::in_memory().unwrap());
    world.set_cvar(&GAME_PERSIST_GUESTS, false).unwrap();
    hello(&mut world, ConnId(1), "Urist");
    settle(&mut world, ConnId(1));
    let first = user_of(&world, ConnId(1)).unwrap();
    hello(&mut world, ConnId(2), "Urist");
    settle(&mut world, ConnId(2));
    let second = user_of(&world, ConnId(2)).unwrap();
    assert_ne!(first, second);
    let database = world.resource::<Database>();
    let db = database.db().clone();
    let stored = database.block_on(async { db.get_player_record_by_user_id(first).await.unwrap() });
    assert_eq!(stored, None);
}

#[test]
fn without_database_players_enter_at_once() {
    let mut world = crate::testing::world_with("");
    hello(&mut world, ConnId(1), "Urist");
    world.run_tick();
    assert!(user_of(&world, ConnId(1)).is_some());
}
