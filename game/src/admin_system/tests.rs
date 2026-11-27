use astrocore_ecs::EventWorldExt;
use astrocore_net::{ConnId, PlayerConnected, PlayerDisconnected, Session};

use super::*;
use crate::mind::tests::{PROTOTYPES, scene};
use crate::testing::sent_net_events;

fn probe() -> PlayerInfoChanged {
    PlayerInfoChanged {
        info: PlayerInfo {
            username: String::new(),
            character_name: String::new(),
            identity_name: String::new(),
            starting_job: String::new(),
            antag: false,
            net_entity: None,
            user_id: NetUserId(uuid::Uuid::nil()),
            connected: false,
            active_this_round: false,
        },
    }
}

fn changes(world: &mut World, conn: ConnId) -> Vec<PlayerInfo> {
    sent_net_events(world, &probe())
        .into_iter()
        .filter(|(c, _)| *c == conn)
        .map(|(_, e)| e.info)
        .collect()
}

#[test]
fn admins_follow_players_joining_taking_bodies_and_leaving() {
    let mut scene = scene(PROTOTYPES);
    let (admin, _) = scene.session(1);
    let admin_conn = scene.world.get::<Session>(admin).unwrap().conn;
    // Получив права, администратор получает весь список.
    crate::admin_manager::promote_host(&mut scene.world, admin);
    let full = sent_net_events(
        &mut scene.world,
        &FullPlayerList {
            players: Vec::new(),
        },
    );
    assert!(full.iter().any(|(c, _)| *c == admin_conn));

    let (player, user, body, _) = scene.player(2);
    scene
        .world
        .raise_broadcast(&mut PlayerConnected { session: player });
    // Новое тело ещё без сетевого номера: сведения уходят со следующего такта.
    let now: Vec<_> = changes(&mut scene.world, admin_conn)
        .into_iter()
        .filter(|i| i.user_id == user)
        .collect();
    assert!(now.is_empty(), "{now:?}");
    scene.world.run_tick();
    scene.world.run_tick();
    let info = changes(&mut scene.world, admin_conn)
        .into_iter()
        .rfind(|i| i.user_id == user)
        .unwrap();
    assert_eq!(info.username, "player2");
    assert_eq!(info.character_name, "Urist");
    assert_eq!(info.net_entity, scene.world.get::<NetEntity>(body).copied());
    assert!(info.connected);
    assert!(info.active_this_round);

    scene
        .world
        .raise_broadcast(&mut PlayerDisconnected { session: player });
    let info = changes(&mut scene.world, admin_conn).pop().unwrap();
    assert!(!info.connected);
    assert_eq!(info.net_entity, None);
    let stored = &scene.world.resource::<AdminPlayerList>().players[&user];
    assert!(!stored.connected);
}

#[test]
fn round_restart_forgets_who_played() {
    let mut scene = scene(PROTOTYPES);
    let (admin, _) = scene.session(1);
    crate::admin_manager::promote_host(&mut scene.world, admin);
    let (_, user, _, _) = scene.player(2);
    scene.world.run_tick();
    scene.world.run_tick();
    assert!(scene.world.resource::<AdminPlayerList>().players[&user].active_this_round);
    scene
        .world
        .raise_broadcast(&mut crate::game_ticker::RoundRestartCleanup);
    let list = scene.world.resource::<AdminPlayerList>();
    assert!(list.round_active.is_empty());
    assert!(!list.players[&user].active_this_round);
    let full = sent_net_events(
        &mut scene.world,
        &FullPlayerList {
            players: Vec::new(),
        },
    );
    assert!(
        full.iter()
            .any(|(_, e)| e.players.iter().any(|p| p.user_id == user))
    );
}

#[test]
fn players_are_not_sent_the_list() {
    let mut scene = scene(PROTOTYPES);
    let (watcher, _) = scene.session(1);
    let conn = scene.world.get::<Session>(watcher).unwrap().conn;
    let (player, _, _, _) = scene.player(2);
    scene
        .world
        .raise_broadcast(&mut PlayerConnected { session: player });
    scene.world.run_tick();
    scene.world.run_tick();
    assert!(changes(&mut scene.world, conn).is_empty());
}
