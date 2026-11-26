use super::*;
use crate::access::tests::PROTOTYPES as ACCESS;
use crate::hands::HandsWorldExt;
use crate::testing::{spawn_at, station, world_with};
use crate::weldable::{is_welded, set_welded};
use astrocore_ecs::GameWorldExt;

const DOORS: &str = r#"
- type: entity
  id: TestAirlock
  components:
  - type: Physics
    bodyType: Static
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.49,-0.49,0.49,0.49"
        density: 100
        mask: [ FullTileMask ]
        layer: [ AirlockLayer ]
  - type: Occluder
  - type: Airtight
  - type: Door
  - type: DoorBolt
  - type: Airlock
  - type: ApcPowerReceiver
    needsPower: false
  - type: Weldable
    time: 3
- type: entity
  id: EngineeringAirlock
  parent: TestAirlock
  components:
  - type: AccessReader
    access: [ [ Engineering ] ]
- type: entity
  id: UnpoweredAirlock
  parent: TestAirlock
  components:
  - type: ApcPowerReceiver
    needsPower: true
- type: entity
  id: Walker
  components:
  - type: Tag
    tags: [ DoorBumpOpener ]
  - type: Hands
  - type: ComplexInteraction
  - type: DoAfter
  - type: Physics
    bodyType: KinematicController
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeCircle
          radius: 0.35
        density: 185
        mask: [ MobMask ]
        layer: [ MobLayer ]
- type: entity
  id: Crowbar
  components:
  - type: Item
  - type: Prying
  - type: Tool
    qualities: [ Prying ]
- type: entity
  id: Welder
  components:
  - type: Item
  - type: Tool
    qualities: [ Welding ]
"#;

fn world() -> World {
    world_with(&format!("{ACCESS}{DOORS}"))
}

fn walker(world: &mut World, grid: Entity, x: i32, y: i32) -> Entity {
    let user = spawn_at(world, "Walker", grid, x, y);
    world.add_hand(user, "right", crate::hands::Hand::default());
    user
}

fn run_for(world: &mut World, seconds: f64) {
    let ticks = (seconds * 30.0).ceil() as u32;
    for _ in 0..ticks {
        world.run_tick();
    }
}

fn state(world: &World, door: Entity) -> DoorState {
    world.get::<Door>(door).unwrap().state
}

fn solid(world: &World, door: Entity) -> bool {
    world.get::<Physics>(door).unwrap().can_collide
}

#[test]
fn door_opens_in_two_phases_and_closes_by_itself() {
    let mut world = world();
    let grid = station(&mut world);
    let door = spawn_at(&mut world, "TestAirlock", grid, 5, 5);
    assert_eq!(state(&world, door), DoorState::Closed);
    assert!(solid(&world, door));
    assert!(world.get::<Airlock>(door).unwrap().powered);

    assert!(try_toggle(&mut world, door, None));
    assert_eq!(state(&world, door), DoorState::Opening);
    assert!(solid(&world, door));
    run_for(&mut world, 0.45);
    // Середина открытия: проход свободен, свет и газ проходят.
    assert_eq!(state(&world, door), DoorState::Opening);
    assert!(!solid(&world, door));
    assert!(!world.get::<Occluder>(door).unwrap().enabled);
    assert!(
        !world
            .get::<crate::airtight::Airtight>(door)
            .unwrap()
            .air_blocked
    );
    run_for(&mut world, 0.25);
    assert_eq!(state(&world, door), DoorState::Open);

    // Автозакрытие через 5 секунд.
    run_for(&mut world, 4.9);
    assert_eq!(state(&world, door), DoorState::Open);
    run_for(&mut world, 0.2);
    assert_eq!(state(&world, door), DoorState::Closing);
    run_for(&mut world, 0.7);
    assert_eq!(state(&world, door), DoorState::Closed);
    assert!(solid(&world, door));
    assert!(world.get::<Occluder>(door).unwrap().enabled);
}

#[test]
fn door_without_access_denies_and_card_opens_it() {
    let mut world = world();
    let grid = station(&mut world);
    let door = spawn_at(&mut world, "EngineeringAirlock", grid, 2, 1);
    let user = walker(&mut world, grid, 1, 1);
    assert!(!try_toggle(&mut world, door, Some(user)));
    assert_eq!(state(&world, door), DoorState::Denying);
    run_for(&mut world, 0.5);
    assert_eq!(state(&world, door), DoorState::Closed);

    let card = spawn_at(&mut world, "EngineeringIDCard", grid, 1, 1);
    assert!(world.try_pickup(user, card, None, false));
    assert!(try_toggle(&mut world, door, Some(user)));
    assert_eq!(state(&world, door), DoorState::Opening);
}

#[test]
fn bolts_and_missing_power_keep_an_airlock_shut() {
    let mut world = world();
    let grid = station(&mut world);
    let door = spawn_at(&mut world, "TestAirlock", grid, 5, 5);
    assert!(try_set_bolts_down(&mut world, door, true));
    assert!(is_bolted(&world, door));
    assert!(!try_toggle(&mut world, door, None));
    // Заболченная дверь не мигает отказом.
    deny(&mut world, door);
    assert_eq!(state(&world, door), DoorState::Closed);
    assert!(try_set_bolts_down(&mut world, door, false));
    assert!(try_toggle(&mut world, door, None));

    let unpowered = spawn_at(&mut world, "UnpoweredAirlock", grid, 7, 5);
    assert!(!world.get::<Airlock>(unpowered).unwrap().powered);
    assert!(!try_toggle(&mut world, unpowered, None));
    // Без питания болты не двигаются.
    assert!(!try_set_bolts_down(&mut world, unpowered, true));
}

#[test]
fn crowbar_pries_only_an_unpowered_airlock() {
    let mut world = world();
    let grid = station(&mut world);
    let powered = spawn_at(&mut world, "TestAirlock", grid, 2, 1);
    let unpowered = spawn_at(&mut world, "UnpoweredAirlock", grid, 1, 2);
    let user = walker(&mut world, grid, 1, 1);
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 1, 1);
    assert!(world.try_pickup(user, crowbar, None, false));

    assert!(try_pry_with(&mut world, powered, user, crowbar));
    run_for(&mut world, 2.0);
    assert_eq!(state(&world, powered), DoorState::Closed);

    assert!(try_pry_with(&mut world, unpowered, user, crowbar));
    run_for(&mut world, 1.0);
    assert_eq!(state(&world, unpowered), DoorState::Closed);
    run_for(&mut world, 0.6);
    assert_ne!(state(&world, unpowered), DoorState::Closed);
    run_for(&mut world, 1.0);
    // Без питания шлюз остаётся открытым: автозакрытие не назначается.
    assert_eq!(state(&world, unpowered), DoorState::Open);
    run_for(&mut world, 6.0);
    assert_eq!(state(&world, unpowered), DoorState::Open);
}

#[test]
fn welded_door_stays_shut_and_open_door_cannot_be_welded() {
    let mut world = world();
    let grid = station(&mut world);
    let door = spawn_at(&mut world, "TestAirlock", grid, 2, 1);
    let user = walker(&mut world, grid, 1, 1);
    let welder = spawn_at(&mut world, "Welder", grid, 1, 1);
    assert!(world.try_pickup(user, welder, None, false));

    assert!(crate::weldable::try_weld(&mut world, door, welder, user));
    run_for(&mut world, 3.2);
    assert!(is_welded(&world, door));
    assert!(!try_toggle(&mut world, door, None));
    assert_eq!(state(&world, door), DoorState::Closed);

    set_welded(&mut world, door, false);
    assert!(try_toggle(&mut world, door, None));
    assert!(!crate::weldable::try_weld(&mut world, door, welder, user));
}

#[test]
fn safety_keeps_the_door_open_over_someone_and_without_it_the_door_crushes() {
    let mut world = world();
    let grid = station(&mut world);
    let door = spawn_at(&mut world, "TestAirlock", grid, 5, 5);
    assert!(try_toggle(&mut world, door, None));
    run_for(&mut world, 0.7);
    assert_eq!(state(&world, door), DoorState::Open);
    let victim = walker(&mut world, grid, 5, 5);
    // Тег толчка мешает: дверь сразу откроется вновь, здесь проверяется только давка.
    world.remove_tag(victim, DOOR_BUMP_TAG);
    assert!(!try_close(&mut world, door, None));

    set_safety(&mut world, door, false);
    assert!(try_close(&mut world, door, None));
    run_for(&mut world, 0.45);
    // Середина закрытия: дверь твёрдая и давит стоящего.
    assert!(solid(&world, door));
    assert!(
        world
            .get::<Door>(door)
            .unwrap()
            .currently_crushing
            .contains(&victim)
    );
    // Через время оглушения дверь открывается сама.
    run_for(&mut world, 2.2);
    assert!(matches!(
        state(&world, door),
        DoorState::Opening | DoorState::Open
    ));
}

#[test]
fn emergency_access_lets_anyone_through() {
    let mut world = world();
    let grid = station(&mut world);
    let door = spawn_at(&mut world, "EngineeringAirlock", grid, 2, 1);
    let user = walker(&mut world, grid, 1, 1);
    set_emergency_access(&mut world, door, true);
    assert!(try_toggle(&mut world, door, Some(user)));
}
