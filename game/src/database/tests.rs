use astrocore_ecs::GameWorldExt;

use super::*;

fn db() -> Database {
    Database::in_memory().expect("SQLite в памяти")
}

#[test]
fn guests_get_stable_assigned_ids() {
    let database = db();
    let db = database.db().clone();
    database.block_on(async {
        assert_eq!(db.get_assigned_user_id("Urist").await.unwrap(), None);
        let id = NetUserId::random();
        db.assign_user_id("Urist", id).await.unwrap();
        assert_eq!(db.get_assigned_user_id("Urist").await.unwrap(), Some(id));
        // Имя и идентификатор уникальны.
        assert!(
            db.assign_user_id("Urist", NetUserId::random())
                .await
                .is_err()
        );
        assert!(db.assign_user_id("Other", id).await.is_err());
    });
}

#[test]
fn player_record_is_created_then_updated() {
    let database = db();
    let db = database.db().clone();
    database.block_on(async {
        let id = NetUserId::random();
        db.update_player_record(id, "Urist", "127.0.0.1")
            .await
            .unwrap();
        let first = db.get_player_record_by_user_id(id).await.unwrap().unwrap();
        assert_eq!(first.last_seen_user_name, "Urist");
        assert_eq!(first.first_seen_time, first.last_seen_time);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        db.update_player_record(id, "Urist McHands", "10.0.0.2")
            .await
            .unwrap();
        let second = db.get_player_record_by_user_id(id).await.unwrap().unwrap();
        assert_eq!(second.first_seen_time, first.first_seen_time);
        assert!(second.last_seen_time > first.last_seen_time);
        assert_eq!(second.last_seen_address, "10.0.0.2");
        assert_eq!(
            db.get_player_record_by_user_name("Urist McHands")
                .await
                .unwrap()
                .map(|r| r.user_id),
            Some(id)
        );
        assert_eq!(
            db.get_player_record_by_user_name("Urist").await.unwrap(),
            None
        );
    });
}

#[test]
fn player_found_by_name_is_the_most_recent() {
    let database = db();
    let db = database.db().clone();
    database.block_on(async {
        let old = NetUserId::random();
        let new = NetUserId::random();
        db.update_player_record(old, "Urist", "127.0.0.1")
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        db.update_player_record(new, "Urist", "127.0.0.1")
            .await
            .unwrap();
        let found = db.get_player_record_by_user_name("Urist").await.unwrap();
        assert_eq!(found.map(|r| r.user_id), Some(new));
    });
}

#[test]
fn servers_rounds_and_connection_logs() {
    let database = db();
    let db = database.db().clone();
    database.block_on(async {
        let (server, existed) = db.add_or_get_server("main").await.unwrap();
        assert!(!existed);
        assert_eq!(db.add_or_get_server("main").await.unwrap(), (server, true));
        // Номер этого сервера создаётся по имени из настроек один раз.
        let own = db.server_id().await.unwrap();
        assert_eq!(db.server_id().await.unwrap(), own);
        assert_ne!(own, server);

        let a = NetUserId::random();
        let b = NetUserId::random();
        let stranger = NetUserId::random();
        db.update_player_record(a, "A", "127.0.0.1").await.unwrap();
        db.update_player_record(b, "B", "127.0.0.1").await.unwrap();
        let round = db.add_new_round(server, &[a, stranger]).await.unwrap();
        db.add_round_players(round, &[a, b]).await.unwrap();
        let record = db.get_round(round).await.unwrap().unwrap();
        assert_eq!(record.server_id, server);
        assert_eq!(record.players, vec![a, b]);
        assert!(record.start_date.is_some());
        assert_eq!(db.get_round(round + 100).await.unwrap(), None);

        db.add_connection_log(a, "A", "127.0.0.1", None, server)
            .await
            .unwrap();
        db.add_connection_log(a, "A", "127.0.0.1", Some(ConnectionDenyReason::Ban), server)
            .await
            .unwrap();
        let logs = db.connection_logs(a).await.unwrap();
        assert_eq!(logs.len(), 2);
        assert!(
            logs.iter()
                .any(|l| l.denied == Some(ConnectionDenyReason::Ban))
        );
        assert!(logs.iter().any(|l| l.denied.is_none()));
        assert!(db.connection_logs(b).await.unwrap().is_empty());
    });
}

#[test]
fn continuations_run_in_world_at_tick_start() {
    let mut world = crate::testing::world_with("");
    world.insert_resource(db());
    #[derive(Resource, Default)]
    struct Got(Option<i64>);
    world.init_resource::<Got>();
    assert!(spawn_db(
        &world,
        |db| async move { db.add_or_get_server("x").await.map(|s| s.0) },
        |world, result| world.resource_mut::<Got>().0 = result.ok(),
    ));
    for _ in 0..200 {
        world.run_tick();
        if world.resource::<Got>().0.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(world.resource::<Got>().0.is_some());
}

#[test]
fn config_selects_engine_and_builds_urls() {
    assert_eq!(
        postgres_url("db.local", 5433, "astro core", "admin", "p@ss:word"),
        "postgres://admin:p%40ss%3Aword@db.local:5433/astro%20core"
    );
    assert_eq!(
        postgres_url("localhost", 5432, "astrocore", "postgres", ""),
        "postgres://postgres@localhost:5432/astrocore"
    );
    let mut world = crate::testing::world_with("");
    world.set_cvar_text("database.engine", "mysql").unwrap();
    assert!(Database::from_config(world.resource::<Config>()).is_err());
    // SQLite по умолчанию: файл в каталоге данных, миграции применяются.
    let dir = std::env::temp_dir().join(format!("astrocore-db-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    world.set_cvar_text("database.engine", "sqlite").unwrap();
    world
        .set_cvar_text("game.user_data", &dir.display().to_string())
        .unwrap();
    let database = Database::from_config(world.resource::<Config>()).unwrap();
    let db = database.db().clone();
    database.block_on(async { db.add_or_get_server("file").await.unwrap() });
    drop(database);
    assert!(dir.join("preferences.db").exists());
    // Повторное открытие не применяет миграции заново и видит данные.
    let database = Database::from_config(world.resource::<Config>()).unwrap();
    let db = database.db().clone();
    assert!(database.block_on(async { db.add_or_get_server("file").await.unwrap().1 }));
    let _ = std::fs::remove_dir_all(&dir);
}
