use astrocore_ecs::GameWorldExt;

use super::*;
use crate::database::Database;

fn world_with_db() -> (World, i64) {
    let mut world = crate::testing::world_with("");
    let database = Database::in_memory().unwrap();
    let db = database.db().clone();
    let round = database.block_on(async {
        let server = db.server_id().await.unwrap();
        db.add_new_round(server, &[]).await.unwrap()
    });
    world.insert_resource(database);
    (world, round)
}

fn wait_saved(world: &mut World) {
    for _ in 0..400 {
        world.run_tick();
        let logs = world.resource::<AdminLogs>();
        if logs.queued().0 == 0 && !logs.saving {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("журнал не ушёл в базу");
}

fn stored(world: &World, filter: &LogFilter) -> Vec<AdminLogEntry> {
    let database = world.resource::<Database>();
    let db = database.db().clone();
    let filter = filter.clone();
    database.block_on(async move { db.get_admin_logs(&filter).await.unwrap() })
}

#[test]
fn pre_round_logs_wait_for_round_then_batch_to_database() {
    let (mut world, round) = world_with_db();
    world.set_cvar(&ADMIN_LOGS_QUEUE_SEND_DELAY, 0.0).unwrap();
    let player = NetUserId::random();
    let stranger = NetUserId::random();
    {
        let database = world.resource::<Database>();
        let db = database.db().clone();
        database.block_on(async {
            db.update_player_record(player, "Urist", "127.0.0.1")
                .await
                .unwrap()
        });
    }
    add_admin_log(
        &mut world,
        LogType::Connection,
        LogImpact::Low,
        "в лобби",
        &[],
        &[player],
    );
    world.run_tick();
    // До раунда записи ждут его номера.
    assert_eq!(world.resource::<AdminLogs>().queued(), (0, 1));
    round_starting(&mut world, round);
    add_admin_log(
        &mut world,
        LogType::Damaged,
        LogImpact::High,
        "Urist ударил\0 Bob",
        &[("damage", "10".into())],
        &[player, stranger, player],
    );
    wait_saved(&mut world);
    let all = stored(&world, &LogFilter::default());
    assert_eq!(all.len(), 2);
    // Новые первыми; номера по раунду с единицы.
    assert_eq!(all[0].message, "Urist ударил Bob");
    assert_eq!((all[0].round_id, all[0].id), (round, 2));
    assert_eq!((all[1].round_id, all[1].id), (round, 1));
    // Игрок без записи в базе к записи не привязан.
    assert_eq!(all[0].players, [player]);
    assert_eq!(all[0].json, r#"{"damage":"10"}"#);

    let high = stored(
        &world,
        &LogFilter {
            impacts: Some(vec![LogImpact::High]),
            ..Default::default()
        },
    );
    assert_eq!(high.len(), 1);
    let searched = stored(
        &world,
        &LogFilter {
            search: Some("лобби".into()),
            ascending: true,
            ..Default::default()
        },
    );
    assert_eq!(searched.len(), 1);
    let by_player = stored(
        &world,
        &LogFilter {
            any_players: Some(vec![stranger]),
            ..Default::default()
        },
    );
    assert!(by_player.is_empty());
    let other_round = stored(
        &world,
        &LogFilter {
            round: Some(round + 1),
            ..Default::default()
        },
    );
    assert!(other_round.is_empty());
}

#[test]
fn logs_wait_for_delay_and_overflow_is_dropped() {
    let (mut world, round) = world_with_db();
    round_starting(&mut world, round);
    world
        .set_cvar(&ADMIN_LOGS_QUEUE_SEND_DELAY, 3600.0)
        .unwrap();
    world.set_cvar(&ADMIN_LOGS_DROP_THRESHOLD, 3).unwrap();
    // Первая отправка — сразу, следующие ждут задержку.
    add_admin_log(&mut world, LogType::Verb, LogImpact::Low, "1", &[], &[]);
    wait_saved(&mut world);
    for i in 0..5 {
        add_admin_log(
            &mut world,
            LogType::Verb,
            LogImpact::Low,
            &i.to_string(),
            &[],
            &[],
        );
    }
    world.run_tick();
    let logs = world.resource::<AdminLogs>();
    assert_eq!(logs.queued(), (3, 0));
    assert_eq!(logs.dropped, 2);
    // Заполненная очередь уходит раньше срока.
    world.set_cvar(&ADMIN_LOGS_QUEUE_MAX, 3).unwrap();
    wait_saved(&mut world);
    assert_eq!(stored(&world, &LogFilter::default()).len(), 4);
}

#[test]
fn players_entering_and_leaving_are_logged() {
    let (mut world, round) = world_with_db();
    round_starting(&mut world, round);
    let session = world
        .spawn(Session {
            conn: astrocore_net::ConnId(7),
            user_id: NetUserId::random(),
            name: "Urist".into(),
            attached: None,
            last_input: 0,
            sync: Default::default(),
        })
        .id();
    world.raise_broadcast(&mut PlayerConnected { session });
    world.raise_broadcast(&mut PlayerDisconnected { session });
    let logs = world.resource::<AdminLogs>();
    assert_eq!(logs.queued(), (2, 0));
    assert!(logs.queue.iter().all(|l| l.log_type == LogType::Connection));
    assert_eq!(LogType::from_i64(104), Some(LogType::Connection));
    assert_eq!(LogType::Connection.name(), "Connection");
}

#[test]
fn without_database_or_when_disabled_nothing_piles_up() {
    let mut world = crate::testing::world_with("");
    round_starting(&mut world, 1);
    add_admin_log(&mut world, LogType::Verb, LogImpact::Low, "x", &[], &[]);
    world.run_tick();
    assert_eq!(world.resource::<AdminLogs>().queued(), (0, 0));
    world.set_cvar(&ADMIN_LOGS_ENABLED, false).unwrap();
    world.run_tick();
    add_admin_log(&mut world, LogType::Verb, LogImpact::Low, "y", &[], &[]);
    assert_eq!(world.resource::<AdminLogs>().queued(), (0, 0));
}
