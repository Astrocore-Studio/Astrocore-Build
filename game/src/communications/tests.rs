use astrocore_console::ConfigWorldExt;
use astrocore_ecs::GameTime;

use super::*;
use crate::game_ticker::{self, GameRunLevel};
use crate::testing::{spawn_at, station, world_with};

pub(crate) const PROTOTYPES: &str = r#"
- type: entity
  id: TestComms
  components:
  - type: CommunicationsConsole
    title: comms-console-announcement-title-station
  - type: AccessReader
    access: [ [ Command ] ]
- type: entity
  id: TestEvacConsole
  components:
  - type: EmergencyShuttleConsole
  - type: AccessReader
    access: [ [ Command ] ]
- type: entity
  id: TestCaptain
  components:
  - type: IdCard
    fullName: Ada Lovelace
    jobTitle: head of personnel
  - type: Access
    tags: [ Command, EmergencyShuttleRepealAll ]
- type: entity
  id: TestCommandCard
  components:
  - type: IdCard
    fullName: Bob
    jobTitle: captain
  - type: Access
    tags: [ Command ]
- type: entity
  id: TestAssistant
  components:
  - type: IdCard
    fullName: Carl
    jobTitle: assistant
  - type: Access
    tags: [ Maintenance ]
"#;

pub(crate) struct Scene {
    pub world: World,
    pub grid: Entity,
}

/// Станция-грид с владеющей станцией, раунд идёт.
pub(crate) fn scene() -> Scene {
    let mut world = world_with(PROTOTYPES);
    world.resource_mut::<game_ticker::GameTicker>().run_level = GameRunLevel::InRound;
    let grid = station(&mut world);
    let station = world.spawn(stations::StationData::default()).id();
    stations::add_grid_to_station(&mut world, station, grid, None);
    Scene { world, grid }
}

fn advance(world: &mut World, seconds: u32) {
    let rate = u32::from(world.resource::<GameTime>().tick_rate);
    world.resource_mut::<GameTime>().tick.0 += rate * seconds;
}

fn cooldown(world: &World, console: Entity) -> f32 {
    world
        .get::<CommunicationsConsole>(console)
        .unwrap()
        .announcement_cooldown_remaining
}

#[test]
fn announcements_wait_for_the_cooldown_and_need_access() {
    let Scene { mut world, grid } = scene();
    let console = spawn_at(&mut world, "TestComms", grid, 1, 1);
    let captain = spawn_at(&mut world, "TestCaptain", grid, 2, 1);
    let assistant = spawn_at(&mut world, "TestAssistant", grid, 3, 1);
    // После появления — 30 с перерыва.
    assert_eq!(cooldown(&world, console), 30.0);
    assert!(!can_announce(&world, console));
    announce(&mut world, console, Some(captain), "Привет");
    assert_eq!(cooldown(&world, console), 30.0);
    world
        .get_mut::<CommunicationsConsole>(console)
        .unwrap()
        .announcement_cooldown_remaining = 0.0;
    // Без доступа — отказ, перерыв не начинается.
    announce(&mut world, console, Some(assistant), "Привет");
    assert_eq!(cooldown(&world, console), 0.0);
    announce(&mut world, console, Some(captain), "Привет");
    assert_eq!(cooldown(&world, console), 90.0);
    // Перерыв уходит со временем.
    world.resource_mut::<GameTime>().tick.0 += 1;
    update(&mut world);
    assert!(cooldown(&world, console) < 90.0);
}

#[test]
fn signature_is_name_and_title_cased_job() {
    let Scene { mut world, grid } = scene();
    let captain = spawn_at(&mut world, "TestCaptain", grid, 2, 1);
    assert_eq!(
        access::identity_short_info(&world, captain).as_deref(),
        Some("Ada Lovelace (Head Of Personnel)")
    );
}

#[test]
fn announcement_text_is_trimmed_cut_and_has_few_newlines() {
    assert_eq!(chat::sanitize_announcement("  hi  ", 256, 2), "hi");
    assert_eq!(chat::sanitize_announcement("abcdef", 3, 2), "abc...");
    assert_eq!(chat::sanitize_announcement("a\nb\nc\nd", 0, 2), "a\nb\nc d");
}

#[test]
fn shuttle_is_called_then_recalled_until_the_turning_point() {
    let Scene { mut world, grid } = scene();
    let console = spawn_at(&mut world, "TestComms", grid, 1, 1);
    let captain = spawn_at(&mut world, "TestCaptain", grid, 2, 1);
    let assistant = spawn_at(&mut world, "TestAssistant", grid, 3, 1);
    assert!(can_call_or_recall(&world, console));
    call_shuttle(&mut world, console, assistant);
    assert!(!round_end::is_round_end_requested(&world));
    call_shuttle(&mut world, console, captain);
    assert!(round_end::is_round_end_requested(&world));
    // Перерыв после вызова.
    assert!(!can_call_or_recall(&world, console));
    advance(&mut world, 31);
    round_end::update(&mut world);
    assert!(can_call_or_recall(&world, console));
    recall_shuttle(&mut world, console, captain);
    assert!(!round_end::is_round_end_requested(&world));
    // После половины отсчёта отозвать нельзя.
    advance(&mut world, 31);
    round_end::update(&mut world);
    call_shuttle(&mut world, console, captain);
    advance(&mut world, 301);
    round_end::update(&mut world);
    assert!(!can_call_or_recall(&world, console));
    world
        .set_cvar(&EMERGENCY_RECALL_TURNING_POINT, 0.1)
        .unwrap();
    assert!(can_call_or_recall(&world, console));
    // Шаттл прибыл — ни вызова, ни отзыва.
    world
        .resource_mut::<emergency_shuttle::Evacuation>()
        .arrived = true;
    assert!(!can_call_or_recall(&world, console));
}

#[test]
fn consoles_that_cannot_shuttle_never_call() {
    let Scene { mut world, grid } = scene();
    let console = spawn_at(&mut world, "TestComms", grid, 1, 1);
    world
        .get_mut::<CommunicationsConsole>(console)
        .unwrap()
        .can_shuttle = false;
    assert!(!can_call_or_recall(&world, console));
}
