use astrocore_console::{ConfigWorldExt, ConsoleShell, ConsoleWorldExt};
use astrocore_ecs::GameTime;

use super::*;
use crate::testing::world_with;

fn world() -> World {
    let mut world = world_with("");
    world.resource_mut::<game_ticker::GameTicker>().run_level = GameRunLevel::InRound;
    world
}

fn advance(world: &mut World, seconds: u32) {
    let rate = u32::from(world.resource::<GameTime>().tick_rate);
    world.resource_mut::<GameTime>().tick.0 += rate * seconds;
}

fn run(world: &mut World, line: &str) {
    let mut shell = ConsoleShell::server();
    world.execute_command(&mut shell, line).unwrap();
}

#[test]
fn minutes_parse_like_ss14() {
    assert_eq!(parse_minutes("5"), Some(Duration::from_secs(300)));
    assert_eq!(parse_minutes("1:30"), Some(Duration::from_secs(90)));
    assert_eq!(parse_minutes("1:75"), None);
    assert_eq!(parse_minutes("soon"), None);
}

#[test]
fn calling_counts_down_and_recall_waits_for_the_cooldown() {
    let mut world = world();
    request_round_end(
        &mut world,
        Some(Duration::from_secs(120)),
        None,
        None,
        true,
        SHUTTLE_CALLED_TEXT,
        SHUTTLE_SENDER,
        false,
    );
    assert!(is_round_end_requested(&world));
    assert_eq!(shuttle_time_left(&world), Some(Duration::from_secs(120)));
    assert_eq!(
        expected_shuttle_length(&world),
        Some(Duration::from_secs(120))
    );
    assert!(!can_call_or_recall(&world));
    // Отзыв во время перерыва не проходит, принудительный — проходит.
    cancel_round_end_countdown(&mut world, None, None, false);
    assert!(is_round_end_requested(&world));
    advance(&mut world, 31);
    update(&mut world);
    assert!(can_call_or_recall(&world));
    cancel_round_end_countdown(&mut world, None, None, false);
    assert!(!is_round_end_requested(&world));
    assert_eq!(shuttle_time_left(&world), None);
    // Повторный вызов в перерыве — только без проверки перерыва (команда).
    request_round_end(
        &mut world,
        None,
        None,
        None,
        true,
        SHUTTLE_CALLED_TEXT,
        SHUTTLE_SENDER,
        false,
    );
    assert!(!is_round_end_requested(&world));
    run(&mut world, "callshuttle 2:00");
    assert_eq!(shuttle_time_left(&world), Some(Duration::from_secs(120)));
    run(&mut world, "recallshuttle");
    assert!(!is_round_end_requested(&world));
}

#[test]
fn without_an_emergency_shuttle_the_round_ends_when_time_is_up() {
    let mut world = world();
    world
        .set_cvar(&game_ticker::ROUND_RESTART_TIME, 20.0)
        .unwrap();
    request_round_end(
        &mut world,
        Some(Duration::from_secs(60)),
        None,
        None,
        true,
        SHUTTLE_CALLED_TEXT,
        SHUTTLE_SENDER,
        true,
    );
    // Нельзя отозвать — так и было задано.
    advance(&mut world, 40);
    update(&mut world);
    cancel_round_end_countdown(&mut world, None, None, false);
    assert!(is_round_end_requested(&world));
    advance(&mut world, 21);
    update(&mut world);
    assert_eq!(game_ticker::run_level(&world), GameRunLevel::PostRound);
    let restart = world
        .resource::<game_ticker::GameTicker>()
        .restart_at
        .unwrap();
    assert_eq!(restart, now(&world) + Duration::from_secs(20));
}

#[test]
fn the_shuttle_calls_itself_after_the_shift_and_again_after_a_recall() {
    let mut world = world();
    world
        .set_cvar(&EMERGENCY_SHUTTLE_AUTO_CALL_TIME, 2)
        .unwrap();
    world
        .set_cvar(&EMERGENCY_SHUTTLE_AUTO_CALL_EXTENSION_TIME, 1)
        .unwrap();
    advance(&mut world, 100);
    update(&mut world);
    assert!(!is_round_end_requested(&world));
    advance(&mut world, 30);
    update(&mut world);
    assert!(is_round_end_requested(&world));
    assert!(state(&mut world).auto_called_before);
    // Отозвали — через продление снова.
    advance(&mut world, 31);
    update(&mut world);
    cancel_round_end_countdown(&mut world, None, None, false);
    advance(&mut world, 61);
    update(&mut world);
    assert!(is_round_end_requested(&world));
}

#[test]
fn outside_the_round_nothing_is_called_and_restart_resets() {
    let mut world = world_with("");
    request_round_end(
        &mut world,
        None,
        None,
        None,
        false,
        SHUTTLE_CALLED_TEXT,
        SHUTTLE_SENDER,
        false,
    );
    assert!(!is_round_end_requested(&world));
    world.resource_mut::<game_ticker::GameTicker>().run_level = GameRunLevel::InRound;
    request_round_end(
        &mut world,
        None,
        None,
        None,
        false,
        SHUTTLE_CALLED_TEXT,
        SHUTTLE_SENDER,
        false,
    );
    world.raise_broadcast(&mut game_ticker::RoundRestartCleanup);
    assert!(!is_round_end_requested(&world));
    assert!(can_call_or_recall(&world));
}
