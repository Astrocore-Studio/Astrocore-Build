use astrocore_ecs::LifecycleWorldExt;

use super::*;
use crate::game_ticker::{ActiveGameRule, GameRule};
use crate::mind::tests::{PROTOTYPES, scene};
use crate::mobs::MobStateWorldExt;

#[derive(bevy_ecs::resource::Resource, Default)]
struct Sent(Vec<String>);

fn rule(world: &mut World, always: bool, delay: f64) -> Entity {
    world
        .spawn((
            GameRule::default(),
            ActiveGameRule,
            RespawnDeadRule {
                always_respawn_dead: always,
            },
            RespawnTracker {
                respawn_delay: delay,
                ..RespawnTracker::default()
            },
        ))
        .id()
}

#[test]
fn dead_players_on_the_list_wait_for_respawn() {
    let mut scene = scene(PROTOTYPES);
    scene.world.init_resource::<Sent>();
    scene
        .world
        .subscribe_broadcast::<crate::chat::ChatMessageSent>(|world, event| {
            let text = event.message.message.clone();
            world.resource_mut::<Sent>().0.push(text);
        });
    let tracker = rule(&mut scene.world, false, 5.0);
    let (session, user, body, _) = scene.player(1);
    let (_, other_user, other_body, _) = scene.player(2);
    add_to_tracker(&mut scene.world, user, tracker);
    scene.world.change_mob_state(body, MobState::Dead, None);
    scene
        .world
        .change_mob_state(other_body, MobState::Dead, None);
    let state = scene.world.get::<RespawnTracker>(tracker).unwrap().clone();
    // В очереди только игрок из списка; ему сказано, через сколько.
    assert!(state.respawn_queue.contains_key(&user));
    assert!(!state.respawn_queue.contains_key(&other_user));
    assert_eq!(
        scene.world.resource::<Sent>().0,
        ["rule-respawn-in-seconds"]
    );
    assert!(scene.world.get::<Session>(session).is_some());
}

#[test]
fn always_respawn_rule_takes_everyone_and_needs_a_station() {
    let mut scene = scene(PROTOTYPES);
    let tracker = rule(&mut scene.world, true, 0.0);
    let (_, user, body, _) = scene.player(1);
    scene.world.change_mob_state(body, MobState::Dead, None);
    assert!(
        scene
            .world
            .get::<RespawnTracker>(tracker)
            .unwrap()
            .players
            .contains(&user)
    );
    scene.world.flush_deletions();
    // Без станции возрождать некуда — тело остаётся.
    assert!(scene.world.is_alive(body));
}
