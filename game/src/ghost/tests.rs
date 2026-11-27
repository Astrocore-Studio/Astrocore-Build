use astrocore_console::{ConsoleShell, ConsoleWorldExt};
use astrocore_ecs::{GameWorldExt, LifecycleWorldExt};
use astrocore_map::TransformWorldExt;
use astrocore_movement::{InputMover, MoveButtons};
use astrocore_net::NetEvent;

use super::*;
use crate::action_blocker::PickupAttempt;
use crate::game_ticker::{GameTicker, PlayerGameStatus};
use crate::mind::tests::{PROTOTYPES as MIND_PROTOTYPES, scene};
use crate::testing::spawn_at;

const EXTRA: &str = r#"
- type: localizedDataset
  id: Spooky
  values:
    prefix: spooky-line-
    count: 2
- type: entity
  id: Vendor
  name: vendor
  components:
  - type: Transform
    anchored: true
  - type: Speech
  - type: SpookySpeaker
    messageSet: Spooky
    speakChance: 1
- type: entity
  id: Beacon
  name: beacon
  components:
  - type: WarpPoint
    location: beacon-bridge
- type: entity
  id: StationBeacon
  name: station beacon
  components:
  - type: WarpPoint
  - type: NavMapBeacon
    defaultText: station-beacon-bridge
  - type: ConfigurableNavMapBeacon
- type: entity
  id: Corpse
  components:
  - type: MindContainer
  - type: MobState
  - type: MobThresholds
    thresholds:
      0: Alive
      200: Dead
  - type: InputMover
  - type: GhostOnMove
    mustBeDead: true
"#;

fn prototypes() -> String {
    format!("{MIND_PROTOTYPES}{EXTRA}")
}

fn message(world: &mut World, session: Entity, event: GhostMessage) {
    world.raise_broadcast(&mut NetEvent {
        session: Some(session),
        event,
    });
}

#[test]
fn ghost_command_needs_a_joined_player() {
    let mut scene = scene(&prototypes());
    let (session, user, body, mind_id) = scene.player(1);
    let mut shell = ConsoleShell::player(session);
    scene.world.execute_command(&mut shell, "ghost").ok();
    assert_eq!(shell.output, ["ghost-command-error-lobby"]);
    assert_eq!(scene.attached(session), Some(body));
    scene
        .world
        .resource_mut::<GameTicker>()
        .statuses
        .insert(user, PlayerGameStatus::JoinedGame);
    let mut shell = ConsoleShell::player(session);
    scene.world.execute_command(&mut shell, "ghost").ok();
    let ghost = scene.attached(session).unwrap();
    assert!(scene.world.get::<Ghost>(ghost).is_some());
    // Живое тело брошено навсегда.
    assert!(!can_return_to_body(&scene.world, ghost));
    assert_eq!(scene.state(mind_id).owned_entity, Some(ghost));
}

#[test]
fn return_to_body_and_warps() {
    let mut scene = scene(&prototypes());
    let (session, _, body, mind_id) = scene.player(1);
    let (_, _, other, _) = scene.player(2);
    scene.world.change_mob_state(body, MobState::Dead, None);
    assert!(on_ghost_attempt(
        &mut scene.world,
        mind_id,
        true,
        false,
        false
    ));
    let ghost = scene.attached(session).unwrap();
    // Телепорт: живой игрок и точка карты.
    let beacon = spawn_at(&mut scene.world, "Beacon", scene.grid, 9, 9);
    // Сетевые номера выдаются на такте.
    scene.world.run_tick();
    let players = player_warps(&mut scene.world, Some(ghost));
    assert_eq!(players.len(), 1);
    assert!(players[0].display_name.starts_with("Urist ("));
    let points = location_warps(&mut scene.world);
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].display_name, "beacon-bridge");
    let target = *scene.world.get::<NetEntity>(beacon).unwrap();
    message(
        &mut scene.world,
        session,
        GhostMessage::WarpToTarget(target),
    );
    let at = scene.world.coordinates(ghost).unwrap().position;
    assert!((at.x - 9.5).abs() < 0.01 && (at.y - 9.5).abs() < 0.01);
    assert_ne!(Some(other), Some(ghost));
    // Обратно в мёртвое тело.
    message(&mut scene.world, session, GhostMessage::ReturnToBody);
    scene.world.flush_deletions();
    assert_eq!(scene.attached(session), Some(body));
    assert!(!scene.world.is_alive(ghost));
}

#[test]
fn dead_bodies_let_go_when_the_player_moves() {
    let mut scene = scene(&prototypes());
    let (session, user) = scene.session(1);
    let corpse = spawn_at(&mut scene.world, "Corpse", scene.grid, 3, 3);
    let mind_id = mind::create_mind(&mut scene.world, Some(user), Some("Urist"));
    mind::transfer_to(&mut scene.world, mind_id, Some(corpse), false, true);
    scene.world.get_mut::<InputMover>(corpse).unwrap().held = MoveButtons::UP;
    // Живой — идёт сам.
    ghost_on_move(&mut scene.world);
    assert_eq!(scene.attached(session), Some(corpse));
    scene.world.change_mob_state(corpse, MobState::Dead, None);
    ghost_on_move(&mut scene.world);
    let ghost = scene.attached(session).unwrap();
    assert_ne!(ghost, corpse);
    assert!(can_return_to_body(&scene.world, ghost));
}

#[test]
fn ghosts_do_not_touch_the_world_and_tell_their_death_time() {
    let mut scene = scene(&prototypes());
    let (session, _, _, mind_id) = scene.player(1);
    on_ghost_attempt(&mut scene.world, mind_id, true, false, false);
    let ghost = scene.attached(session).unwrap();
    let mut attempt = PickupAttempt {
        user: ghost,
        item: ghost,
        cancelled: false,
    };
    scene.world.raise(ghost, &mut attempt);
    assert!(attempt.cancelled);
    // Призрак в слое призраков и видит их.
    assert_eq!(
        scene
            .world
            .get::<astrocore_net::Visibility>(ghost)
            .unwrap()
            .layer
            .0,
        VISIBILITY_GHOST
    );
    assert_ne!(
        scene
            .world
            .get::<astrocore_net::Eye>(ghost)
            .unwrap()
            .vis_mask
            .0
            & VISIBILITY_GHOST,
        0
    );
    let lines = crate::examine::examine_text(&mut scene.world, ghost, ghost);
    assert!(lines.iter().any(|l| l == "comp-ghost-examine-time-seconds"));
}

#[test]
fn boo_spooks_speakers_once_per_cooldown() {
    let mut scene = scene(&prototypes());
    scene
        .world
        .insert_resource(crate::random::GameRandom::seeded(3));
    let (session, _, _, mind_id) = scene.player(1);
    on_ghost_attempt(&mut scene.world, mind_id, true, false, false);
    let ghost = scene.attached(session).unwrap();
    // Никого рядом — не вышло.
    assert!(boo(&mut scene.world, ghost));
    let vendor = spawn_at(&mut scene.world, "Vendor", scene.grid, 3, 2);
    boo(&mut scene.world, ghost);
    let next = scene
        .world
        .get::<SpookySpeaker>(vendor)
        .unwrap()
        .next_speak_time;
    assert!(next >= Duration::from_secs(30));
    // Пока не прошла задержка — молчит.
    let mut event = GhostBoo {
        allowed: GhostBooIntensity::Extreme,
        response: GhostBooIntensity::None,
        handled: false,
    };
    scene.world.raise(vendor, &mut event);
    assert!(!event.handled);
    assert_eq!(
        GhostBooIntensity::from_remaining(6),
        GhostBooIntensity::Extreme
    );
    assert_eq!(
        GhostBooIntensity::from_remaining(3),
        GhostBooIntensity::Normal
    );
    assert_eq!(
        GhostBooIntensity::from_remaining(0),
        GhostBooIntensity::None
    );
}

#[test]
fn everyone_sees_ghosts_after_the_round() {
    let mut scene = scene(&format!(
        "{}{}",
        prototypes().replace(
            "  - type: Ghost\n  - type: Eye\n",
            "  - type: Ghost\n  - type: Eye\n  - type: Tag\n    tags: [ AllowGhostShownByEvent ]\n"
        ),
        ""
    ));
    let (session, _, _, mind_id) = scene.player(1);
    on_ghost_attempt(&mut scene.world, mind_id, true, false, false);
    let ghost = scene.attached(session).unwrap();
    scene
        .world
        .raise_broadcast(&mut crate::game_ticker::GameRunLevelChanged {
            old: crate::game_ticker::GameRunLevel::InRound,
            new: crate::game_ticker::GameRunLevel::PostRound,
        });
    assert_eq!(
        scene
            .world
            .get::<astrocore_net::Visibility>(ghost)
            .unwrap()
            .layer
            .0,
        VISIBILITY_NORMAL
    );
}

#[test]
fn station_beacons_name_their_warp_points() {
    let mut scene = scene(&prototypes());
    spawn_at(&mut scene.world, "StationBeacon", scene.grid, 1, 1);
    scene.world.run_tick();
    let warps = location_warps(&mut scene.world);
    assert_eq!(warps.len(), 1);
    assert_eq!(warps[0].display_name, "station-beacon-bridge");
}

#[test]
fn ghosts_toggle_their_field_of_view() {
    let mut scene = scene(&prototypes());
    let (session, _, _, mind_id) = scene.player(1);
    on_ghost_attempt(&mut scene.world, mind_id, true, false, false);
    let ghost = scene.attached(session).unwrap();
    let before = scene.world.get::<Eye>(ghost).unwrap().draw_fov;
    message(&mut scene.world, session, GhostMessage::ToggleFov);
    assert_eq!(scene.world.get::<Eye>(ghost).unwrap().draw_fov, !before);
}

#[test]
fn location_is_described_by_the_nearest_enabled_beacon() {
    use astrocore_map::{MapCoordinates, TransformWorldExt};
    use astrocore_math::Vec2;
    let mut world = crate::testing::world_with(
        r#"
- type: entity
  id: TestBeacon
  components:
  - type: ConfigurableNavMapBeacon
  - type: NavMapBeacon
    text: Bridge
"#,
    );
    let grid = crate::testing::station(&mut world);
    let map = world.map_coordinates(grid).unwrap().map;
    let here = MapCoordinates::new(map, Vec2::new(2.0, 2.0));
    assert_eq!(
        nearest_beacon_string(&mut world, here),
        "nav-beacon-pos-no-beacons"
    );
    crate::testing::spawn_at(&mut world, "TestBeacon", grid, 1, 1);
    assert_eq!(
        nearest_beacon_string(&mut world, here),
        "nav-beacon-pos-format"
    );
    let far = MapCoordinates::new(map, Vec2::new(60.0, 2.0));
    assert_eq!(
        nearest_beacon_string(&mut world, far),
        "nav-beacon-pos-format-direction"
    );
}
