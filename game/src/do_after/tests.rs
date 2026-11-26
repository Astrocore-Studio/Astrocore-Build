use astrocore_ecs::GameWorldExt;
use astrocore_map::TransformWorldExt;
use astrocore_math::Vec2;
use bevy_ecs::resource::Resource;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: DoAfter
  - type: Hands
    startingHands:
      hand_right:
        location: Right
      hand_left:
        location: Left
- type: entity
  id: Lock
  components:
  - type: Transform
    anchored: true
- type: entity
  id: Crowbar
  components:
  - type: Item
"#;

/// Событие действия «взлом» для тестов.
#[derive(Debug, Clone, PartialEq)]
struct Pry;

#[derive(Resource, Default)]
struct Results(Vec<bool>);

fn scene() -> (World, Entity, Entity) {
    let mut world = world_with(PROTOTYPES);
    world.init_resource::<Results>();
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let lock = spawn_at(&mut world, "Lock", grid, 3, 2);
    world.subscribe::<astrocore_map::Transform, DoAfterDone<Pry>>(|world, _, event| {
        world.resource_mut::<Results>().0.push(event.cancelled);
    });
    (world, user, lock)
}

fn run_seconds(world: &mut World, seconds: f64) {
    let rate = f64::from(world.resource::<GameTime>().tick_rate);
    for _ in 0..(seconds * rate).ceil() as u32 {
        world.run_tick();
    }
}

#[test]
fn do_after_completes_after_the_delay_and_shows_a_bar() {
    let (mut world, user, lock) = scene();
    let started = world.try_start_do_after(
        DoAfterArgs::new(user, 1.0, Some(lock), None, Some(lock)),
        Pry,
    );
    assert!(matches!(started, DoAfterStart::Started(_)));
    assert!(world.has_do_after::<Pry>(user));
    assert_eq!(world.get::<DoAfterBars>(user).unwrap().bars.len(), 1);
    run_seconds(&mut world, 0.5);
    assert!(world.resource::<Results>().0.is_empty());
    run_seconds(&mut world, 0.6);
    assert_eq!(world.resource::<Results>().0, [false]);
    // Завершённое ещё полсекунды видно, потом убирается.
    run_seconds(&mut world, 0.6);
    assert!(world.get::<DoAfterBars>(user).unwrap().bars.is_empty());
    assert!(world.get::<ActiveDoAfter>(user).is_none());
}

#[test]
fn moving_away_or_duplicates_cancel_and_instant_ones_finish_at_once() {
    let (mut world, user, lock) = scene();
    let mut args = DoAfterArgs::new(user, 2.0, Some(lock), None, Some(lock));
    args.break_on_move = true;
    assert!(world.try_start_do_after(args.clone(), Pry).ok());
    // Повтор того же действия прерывает прежнее и сам не начинается.
    assert_eq!(
        world.try_start_do_after(args.clone(), Pry),
        DoAfterStart::Refused
    );
    assert_eq!(world.resource::<Results>().0, [true]);
    assert!(world.try_start_do_after(args.clone(), Pry).ok());
    run_seconds(&mut world, 0.2);
    let _ = world.set_world_position(user, Vec2::new(2.5, 1.5));
    run_seconds(&mut world, 0.2);
    assert_eq!(world.resource::<Results>().0, [true, true]);

    // Без задержки — сразу.
    world.resource_mut::<Results>().0.clear();
    let instant = DoAfterArgs::new(user, 0.0, Some(lock), None, Some(lock));
    assert_eq!(
        world.try_start_do_after(instant, Pry),
        DoAfterStart::Instant
    );
    assert_eq!(world.resource::<Results>().0, [false]);
}

#[test]
fn need_hand_breaks_on_hand_change_and_on_drop() {
    let (mut world, user, lock) = scene();
    let grid = world
        .get::<astrocore_map::Transform>(user)
        .unwrap()
        .grid()
        .unwrap();
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 2, 3);
    assert!(world.try_pickup(user, crowbar, None, true));
    let mut args = DoAfterArgs::new(user, 2.0, Some(lock), Some(crowbar), Some(lock));
    args.need_hand = true;
    assert!(world.try_start_do_after(args.clone(), Pry).ok());
    world.swap_hands(user, false);
    run_seconds(&mut world, 0.1);
    assert_eq!(world.resource::<Results>().0, [true]);
    // Свободная рука нужна, а она занята.
    world.swap_hands(user, false);
    let mut free = args.clone();
    free.need_free_hand = true;
    assert_eq!(world.try_start_do_after(free, Pry), DoAfterStart::Refused);
    // Предмет выпустили — действие прервано.
    assert!(world.try_start_do_after(args, Pry).ok());
    let hand = world.active_hand(user).unwrap();
    assert!(world.try_drop(user, &hand, None, true));
    run_seconds(&mut world, 0.1);
    assert_eq!(world.resource::<Results>().0, [true, true]);
}

#[test]
fn damage_cancels_when_asked() {
    let (mut world, user, lock) = scene();
    let mut args = DoAfterArgs::new(user, 2.0, Some(lock), None, Some(lock));
    args.break_on_damage = true;
    args.damage_threshold = 5.0;
    assert!(world.try_start_do_after(args, Pry).ok());
    world.cancel_do_afters_on_damage(user, 2.0);
    assert!(world.resource::<Results>().0.is_empty());
    world.cancel_do_afters_on_damage(user, 5.0);
    assert_eq!(world.resource::<Results>().0, [true]);
}
