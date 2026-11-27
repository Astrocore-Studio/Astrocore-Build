use astrocore_console::{ConsoleShell, ConsoleWorldExt};
use astrocore_ecs::{EventWorldExt, GameWorldExt, LifecycleWorldExt};
use astrocore_net::ConnId;
use bevy_ecs::resource::Resource;

use super::*;
use crate::bans::BanType;
use crate::database::Database;
use crate::mind::tests::{PROTOTYPES, scene};

#[derive(Resource, Default)]
struct Alerts(Vec<String>);

fn wait_db(world: &mut World) {
    for _ in 0..40 {
        world.run_tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn sent_to(world: &World, conn: ConnId) -> usize {
    world
        .resource::<ServerNet>()
        .outgoing
        .iter()
        .filter(|o| o.conn == conn)
        .count()
}

#[test]
fn bans_are_stored_kick_the_player_and_can_be_pardoned() {
    let mut scene = scene(PROTOTYPES);
    scene.world.insert_resource(Database::in_memory().unwrap());
    scene.world.init_resource::<Alerts>();
    scene
        .world
        .subscribe_broadcast::<crate::chat::ChatMessageSent>(|world, event| {
            if event.message.channel == crate::chat::ChatChannel::ADMIN_ALERT {
                let text = event.message.message.clone();
                world.resource_mut::<Alerts>().0.push(text);
            }
        });
    let (admin, _, _, _) = scene.player(1);
    crate::admin_manager::promote_host(&mut scene.world, admin);
    let (victim, user, _, _) = scene.player(2);
    let conn = scene.world.get::<Session>(victim).unwrap().conn;
    scene.world.resource_mut::<ServerNet>().outgoing.clear();

    let mut shell = ConsoleShell::player(admin);
    scene
        .world
        .execute_command(&mut shell, "ban player2 griefing 60 minor")
        .unwrap();
    assert!(shell.errors.is_empty(), "{:?}", shell.errors);
    wait_db(&mut scene.world);
    // Бан в базе, администраторы оповещены, игрок выгнан.
    let database = scene.world.resource::<Database>();
    let bans = database
        .block_on(
            database
                .db()
                .get_bans(None, Some(user), false, BanType::Server),
        )
        .unwrap();
    assert_eq!(bans.len(), 1);
    assert_eq!(bans[0].reason, "griefing");
    assert_eq!(bans[0].severity, NoteSeverity::Minor);
    assert!(bans[0].expiration_time.is_some());
    // Оповещение уходит готовой строкой (`SendAdminAlertNoFormatOrEscape`).
    assert_eq!(scene.world.resource::<Alerts>().0.len(), 1);
    assert!(sent_to(&scene.world, conn) > 0);

    let id = bans[0].id.unwrap();
    let mut shell = ConsoleShell::player(admin);
    scene
        .world
        .execute_command(&mut shell, &format!("pardon {id}"))
        .unwrap();
    wait_db(&mut scene.world);
    let database = scene.world.resource::<Database>();
    let bans = database
        .block_on(
            database
                .db()
                .get_bans(None, Some(user), false, BanType::Server),
        )
        .unwrap();
    assert!(bans.is_empty());
}

#[test]
fn bad_ban_arguments_are_explained() {
    let mut scene = scene(PROTOTYPES);
    let (admin, _, _, _) = scene.player(1);
    crate::admin_manager::promote_host(&mut scene.world, admin);
    let mut shell = ConsoleShell::player(admin);
    scene
        .world
        .execute_command(&mut shell, "ban player2 reason soon")
        .unwrap();
    assert_eq!(shell.output, ["cmd-ban-invalid-minutes"]);
    let mut shell = ConsoleShell::player(admin);
    scene
        .world
        .execute_command(&mut shell, "ban player2 reason 5 dreadful")
        .unwrap();
    assert_eq!(shell.output, ["cmd-ban-invalid-severity"]);
    let mut shell = ConsoleShell::player(admin);
    scene
        .world
        .execute_command(&mut shell, "ban onlyone")
        .unwrap();
    assert_eq!(shell.output[0], "cmd-ban-invalid-arguments");
}

#[test]
fn players_cannot_ban_or_spawn() {
    let mut scene = scene(PROTOTYPES);
    let (player, _, _, _) = scene.player(1);
    let mut shell = ConsoleShell::player(player);
    assert!(scene.world.execute_command(&mut shell, "ban x y").is_err());
    let mut shell = ConsoleShell::player(player);
    assert!(
        scene
            .world
            .execute_command(&mut shell, "spawn Person")
            .is_err()
    );
}

#[test]
fn kick_spawn_delete_and_rejuvenate() {
    let mut scene = scene(PROTOTYPES);
    let (admin, _, body, _) = scene.player(1);
    crate::admin_manager::promote_host(&mut scene.world, admin);
    let (victim, _, victim_body, _) = scene.player(2);
    let conn = scene.world.get::<Session>(victim).unwrap().conn;
    scene.world.resource_mut::<ServerNet>().outgoing.clear();
    let mut shell = ConsoleShell::player(admin);
    scene
        .world
        .execute_command(&mut shell, "kick player2 spamming")
        .unwrap();
    assert!(sent_to(&scene.world, conn) > 0);

    let mut shell = ConsoleShell::player(admin);
    scene
        .world
        .execute_command(&mut shell, "spawn Person")
        .unwrap();
    assert!(shell.errors.is_empty(), "{:?}", shell.errors);
    assert!(shell.output[0].starts_with("Person"));
    let near = scene.world.coordinates(body).unwrap();
    let spawned: Vec<Entity> = scene
        .world
        .query::<(Entity, &astrocore_ecs::MetaData)>()
        .iter(&scene.world)
        .filter(|(e, m)| {
            m.prototype.as_deref() == Some("Person") && *e != body && *e != victim_body
        })
        .map(|(e, _)| e)
        .collect();
    assert_eq!(spawned.len(), 1);
    assert_eq!(scene.world.coordinates(spawned[0]).unwrap(), near);
    let mut shell = ConsoleShell::player(admin);
    scene
        .world
        .execute_command(&mut shell, "spawn Person 7.5 8.5")
        .unwrap();
    assert!(shell.errors.is_empty(), "{:?}", shell.errors);
    let mut shell = ConsoleShell::player(admin);
    assert!(
        scene
            .world
            .execute_command(&mut shell, "spawn NoSuchThing")
            .is_ok()
    );
    assert!(!shell.errors.is_empty());

    // Удаление по сетевому номеру; исцеление себя.
    scene.world.run_tick();
    let target = *scene
        .world
        .get::<astrocore_net::NetEntity>(spawned[0])
        .unwrap();
    let mut shell = ConsoleShell::player(admin);
    scene
        .world
        .execute_command(&mut shell, &format!("delete {}", target.0))
        .unwrap();
    assert!(!scene.world.is_alive(spawned[0]));
    crate::mobs::MobStateWorldExt::change_mob_state(
        &mut scene.world,
        body,
        crate::mobs::MobState::Dead,
        None,
    );
    let mut shell = ConsoleShell::player(admin);
    scene
        .world
        .execute_command(&mut shell, "rejuvenate")
        .unwrap();
    assert!(crate::mobs::MobStateWorldExt::is_mob_alive(
        &scene.world,
        body
    ));
}
