use astrocore_console::{ConsoleShell, ConsoleWorldExt};
use astrocore_ecs::GameWorldExt;
use astrocore_net::Session;

use super::*;
use crate::admin_logs::add_admin_log;
use crate::database::Database;
use crate::mind::tests::{PROTOTYPES, scene};
use crate::testing::sent_net_events;

fn wait_db(world: &mut World) {
    for _ in 0..40 {
        world.run_tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn messages(world: &mut World, session: Entity) -> Vec<AdminLogsMessage> {
    let conn = world.get::<Session>(session).unwrap().conn;
    sent_net_events(world, &AdminLogsMessage::Next)
        .into_iter()
        .filter(|(c, _)| *c == conn)
        .map(|(_, m)| m)
        .collect()
}

fn new_logs(messages: &[AdminLogsMessage]) -> Vec<(Vec<String>, bool, bool)> {
    messages
        .iter()
        .filter_map(|m| match m {
            AdminLogsMessage::NewLogs {
                logs,
                replace,
                has_next,
            } => Some((
                logs.iter().map(|l| l.message.clone()).collect(),
                *replace,
                *has_next,
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn the_window_pages_through_saved_logs() {
    let mut scene = scene(PROTOTYPES);
    let database = Database::in_memory().unwrap();
    let db = database.db().clone();
    let round = database.block_on(async {
        let server = db.server_id().await.unwrap();
        db.add_new_round(server, &[]).await.unwrap()
    });
    scene.world.insert_resource(database);
    scene
        .world
        .set_cvar(&crate::admin_logs::ADMIN_LOGS_QUEUE_SEND_DELAY, 0.0)
        .unwrap();
    crate::admin_logs::round_starting(&mut scene.world, round);
    for text in ["first", "second", "third"] {
        add_admin_log(
            &mut scene.world,
            LogType::Unknown,
            LogImpact::High,
            text,
            &[],
            &[],
        );
    }
    wait_db(&mut scene.world);
    let (admin, _) = scene.session(1);
    crate::admin_manager::promote_host(&mut scene.world, admin);
    scene
        .world
        .set_cvar(&ADMIN_LOGS_CLIENT_BATCH_SIZE, 2)
        .unwrap();
    let mut shell = ConsoleShell::player(admin);
    scene
        .world
        .execute_command(&mut shell, "adminlogs")
        .unwrap();
    let sent = messages(&mut scene.world, admin);
    assert!(sent.iter().any(|m| matches!(
        m,
        AdminLogsMessage::State {
            round_id,
            round_logs: 3,
            ..
        } if *round_id == round
    )));

    handle_message(
        &mut scene.world,
        admin,
        &AdminLogsMessage::Request(LogsRequest::default()),
    );
    wait_db(&mut scene.world);
    // Новые первыми, по две за раз.
    assert_eq!(
        new_logs(&messages(&mut scene.world, admin)),
        [(vec!["third".to_owned(), "second".to_owned()], true, true)]
    );
    handle_message(&mut scene.world, admin, &AdminLogsMessage::Next);
    wait_db(&mut scene.world);
    assert_eq!(
        new_logs(&messages(&mut scene.world, admin)),
        [(vec!["first".to_owned()], false, false)]
    );

    // Поиск по тексту.
    handle_message(
        &mut scene.world,
        admin,
        &AdminLogsMessage::Request(LogsRequest {
            search: Some("sec".into()),
            ..LogsRequest::default()
        }),
    );
    wait_db(&mut scene.world);
    assert_eq!(
        new_logs(&messages(&mut scene.world, admin)),
        [(vec!["second".to_owned()], true, false)]
    );
}

#[test]
fn only_admins_with_logs_rights_get_logs() {
    let mut scene = scene(PROTOTYPES);
    scene.world.insert_resource(Database::in_memory().unwrap());
    let (player, _) = scene.session(2);
    let mut shell = ConsoleShell::player(player);
    assert!(
        scene
            .world
            .execute_command(&mut shell, "adminlogs")
            .is_err()
    );
    // Без открытого окна запрос не исполняется.
    handle_message(
        &mut scene.world,
        player,
        &AdminLogsMessage::Request(LogsRequest::default()),
    );
    wait_db(&mut scene.world);
    assert!(messages(&mut scene.world, player).is_empty());
}

#[test]
fn the_client_window_filters_by_impact_and_appends_batches() {
    let mut world = World::new();
    world.init_resource::<AdminLogsWindow>();
    let log = |id, impact| SharedAdminLog {
        id,
        log_type: LogType::Unknown,
        impact,
        date: 0,
        message: format!("log {id}"),
        players: Vec::new(),
    };
    receive_logs_message(
        &mut world,
        &AdminLogsMessage::State {
            round_id: 4,
            players: BTreeMap::new(),
            round_logs: 3,
        },
    );
    let window = world.resource::<AdminLogsWindow>();
    assert!(window.open);
    assert_eq!(window.round, 4);
    receive_logs_message(
        &mut world,
        &AdminLogsMessage::NewLogs {
            logs: vec![log(3, LogImpact::Low), log(2, LogImpact::Extreme)],
            replace: true,
            has_next: true,
        },
    );
    receive_logs_message(
        &mut world,
        &AdminLogsMessage::NewLogs {
            logs: vec![log(1, LogImpact::Low)],
            replace: false,
            has_next: false,
        },
    );
    let mut window = world.resource_mut::<AdminLogsWindow>();
    assert_eq!(window.logs.len(), 3);
    assert!(!window.has_next);
    window.impacts.remove(&LogImpact::Low);
    let visible: Vec<i64> = window.visible().iter().map(|l| l.id).collect();
    assert_eq!(visible, [2]);
}
