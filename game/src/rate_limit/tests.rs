use astrocore_console::{CVarFlags, ConfigWorldExt};
use astrocore_ecs::EventWorldExt;
use astrocore_net::PlayerDisconnected;
use bevy_ecs::resource::Resource;

use super::*;
use crate::mind::tests::{PROTOTYPES, scene};

const PERIOD: CVarDef<f32> = CVarDef::new("test.rate_period", 0.3, CVarFlags::SERVERONLY, "");
const COUNT: CVarDef<i32> = CVarDef::new("test.rate_count", 2, CVarFlags::SERVERONLY, "");
const DELAY: CVarDef<i32> = CVarDef::new("test.rate_delay", 0, CVarFlags::SERVERONLY, "");

#[derive(Resource, Default)]
struct Calls {
    limited: usize,
    admins: usize,
}

fn limited(world: &mut World, _: Entity) {
    world.resource_mut::<Calls>().limited += 1;
}

fn admins(world: &mut World, _: Entity) {
    world.resource_mut::<Calls>().admins += 1;
}

fn world() -> (crate::mind::tests::Scene, Entity, Entity) {
    let mut scene = scene(PROTOTYPES);
    scene.world.register_cvar(&PERIOD);
    scene.world.register_cvar(&COUNT);
    scene.world.register_cvar(&DELAY);
    scene.world.init_resource::<Calls>();
    register_rate_limit(
        &mut scene.world,
        "Test",
        RateLimitRegistration {
            period: PERIOD,
            count: COUNT,
            player_limited: Some(limited),
            admin_announce_delay: Some(DELAY),
            admin_announce: Some(admins),
            log_type: LogType::RateLimited,
        },
    );
    let (first, _) = scene.session(1);
    let (second, _) = scene.session(2);
    (scene, first, second)
}

#[test]
fn actions_over_the_limit_are_blocked_and_reported_once_per_period() {
    let (mut scene, first, second) = world();
    let world = &mut scene.world;
    assert_eq!(count_action(world, first, "Test"), RateLimitStatus::Allowed);
    assert_eq!(count_action(world, first, "Test"), RateLimitStatus::Allowed);
    assert_eq!(count_action(world, first, "Test"), RateLimitStatus::Blocked);
    assert_eq!(count_action(world, first, "Test"), RateLimitStatus::Blocked);
    // Другой игрок считается отдельно.
    assert_eq!(
        count_action(world, second, "Test"),
        RateLimitStatus::Allowed
    );
    // Игроку — один раз за период; администраторам — не чаще задержки (0 с: при каждом
    // превышении позже прошлого оповещения).
    let calls = world.resource::<Calls>();
    assert_eq!(calls.limited, 1);
    assert!(calls.admins >= 1);
    // Новый период — счёт заново (время настоящее).
    std::thread::sleep(std::time::Duration::from_millis(350));
    assert_eq!(count_action(world, first, "Test"), RateLimitStatus::Allowed);
}

#[test]
fn negative_delay_silences_admins_and_leaving_forgets_counts() {
    let (mut scene, first, _) = world();
    let world = &mut scene.world;
    world.set_cvar(&DELAY, -1).unwrap();
    world.set_cvar(&COUNT, 0).unwrap();
    assert_eq!(count_action(world, first, "Test"), RateLimitStatus::Blocked);
    assert_eq!(world.resource::<Calls>().admins, 0);
    assert_eq!(world.resource::<Calls>().limited, 1);
    world.raise_broadcast(&mut PlayerDisconnected { session: first });
    assert!(world.resource::<RateLimits>().data.is_empty());
}

#[test]
fn chat_ahelp_and_interactions_are_registered() {
    let (scene, _, _) = world();
    let limits = scene.world.resource::<RateLimits>();
    for key in ["Chat", "AdminHelp", "Interaction"] {
        assert!(limits.registrations.contains_key(key), "{key}");
    }
}

#[test]
fn clients_spamming_interactions_are_cut_off() {
    let (mut scene, first, _) = world();
    let args = |session| astrocore_input::InputArgs {
        function: "Use".into(),
        state: astrocore_input::BoundKeyState::Down,
        session,
        player: None,
        coordinates: None,
        target: None,
    };
    let allowed = (0..8)
        .filter(|_| {
            crate::interaction::interaction_rate_allows(&mut scene.world, &args(Some(first)))
        })
        .count();
    assert_eq!(allowed, 5);
    // Ввод без сессии (сервер) не ограничен.
    assert!(crate::interaction::interaction_rate_allows(
        &mut scene.world,
        &args(None)
    ));
}
