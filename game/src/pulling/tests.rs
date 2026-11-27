use astrocore_ecs::{GameWorldExt, LifecycleWorldExt};
use astrocore_map::{MapCoordinates, TransformWorldExt};
use astrocore_movement::{InputMover, MoveButtons, MovementSpeedModifier};
use astrocore_physics::JointWorldExt;

use super::*;
use crate::testing::{spawn_at, station, world_with};
use crate::virtual_item::VirtualItem;

const PROTOTYPES: &str = r#"
- type: entity
  id: VirtualItem
  components:
  - type: Item
    size: Ginormous
  - type: VirtualItem
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: InputMover
  - type: MovementSpeedModifier
  - type: Puller
  - type: Pullable
  - type: Hands
    startingHands:
      hand_right:
        location: Right
      hand_left:
        location: Left
  - type: Physics
    bodyType: KinematicController
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeCircle
          radius: 0.35
        density: 185
        layer: [ MobLayer ]
        mask: [ MobMask ]
- type: entity
  id: Crate
  components:
  - type: Pullable
  - type: Physics
    bodyType: Dynamic
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.4,-0.4,0.4,0.4"
        density: 50
        layer: [ MachineLayer ]
        mask: [ MachineMask ]
"#;

fn walk(world: &mut World, mob: Entity, buttons: MoveButtons, ticks: usize) {
    world.get_mut::<InputMover>(mob).unwrap().held = buttons;
    for _ in 0..ticks {
        world.run_tick();
    }
    world.get_mut::<InputMover>(mob).unwrap().held = MoveButtons::NONE;
}

fn position(world: &World, entity: Entity) -> Vec2 {
    world.map_coordinates(entity).unwrap().position
}

#[test]
fn pulled_crate_follows_on_a_rope_and_takes_a_hand() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let human = spawn_at(&mut world, "Human", grid, 3, 5);
    let crate_ = spawn_at(&mut world, "Crate", grid, 2, 5);
    world.run_tick();

    assert!(world.try_start_pull(human, crate_));
    assert_eq!(world.pulling_of(human), Some(crate_));
    assert_eq!(world.puller_of(crate_), Some(human));
    let virtual_items: Vec<Entity> = world
        .held_items(human)
        .into_iter()
        .filter(|held| world.get::<VirtualItem>(*held).is_some())
        .collect();
    assert_eq!(virtual_items.len(), 1);
    assert_eq!(
        world
            .get::<MovementSpeedModifier>(human)
            .unwrap()
            .walk_speed_modifier,
        PULLING_SPEED_MODIFIER
    );

    walk(&mut world, human, MoveButtons::RIGHT, 45);
    let (h, c) = (position(&world, human), position(&world, crate_));
    assert!(h.x > 6.5, "{h}");
    assert!(c.x > 5.0, "{c}");
    assert!(h.distance(c) < 1.3, "человек {h}, ящик {c}");

    // Выпустил виртуальный предмет — отпустил ящик.
    let hand = world.hand_holding(human, virtual_items[0]).unwrap();
    assert!(world.try_drop(human, &hand, None, true));
    assert!(!world.is_pulled(crate_));
    let joint = world.get::<Pullable>(crate_).unwrap().pull_joint_id.clone();
    assert!(joint.is_none());
    assert_eq!(
        world
            .get::<MovementSpeedModifier>(human)
            .unwrap()
            .walk_speed_modifier,
        1.0
    );
    world.run_tick();
    assert!(!world.is_alive(virtual_items[0]));
}

#[test]
fn pulled_mob_breaks_free_by_moving_and_toggle_releases() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let puller = spawn_at(&mut world, "Human", grid, 3, 5);
    let victim = spawn_at(&mut world, "Human", grid, 4, 5);
    let crate_ = spawn_at(&mut world, "Crate", grid, 2, 5);
    world.run_tick();

    assert!(world.toggle_pull(crate_, puller));
    assert!(world.toggle_pull(crate_, puller));
    assert!(!world.is_pulled(crate_));
    world.run_tick();
    assert!(world.held_items(puller).is_empty());

    // Тянуть другого: старый груз отпускается.
    assert!(world.try_start_pull(puller, crate_));
    assert!(world.try_start_pull(puller, victim));
    assert!(!world.is_pulled(crate_));
    assert_eq!(world.pulling_of(puller), Some(victim));
    let id = world
        .get::<Pullable>(victim)
        .unwrap()
        .pull_joint_id
        .clone()
        .unwrap();
    assert!(world.joint(puller, &id).is_some());

    world.get_mut::<InputMover>(victim).unwrap().held = MoveButtons::RIGHT;
    world.run_tick();
    assert!(!world.is_pulled(victim));
    assert!(world.joint(puller, &id).is_none());
}

#[test]
fn moving_the_pulled_object_to_a_point() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let human = spawn_at(&mut world, "Human", grid, 5, 5);
    let crate_ = spawn_at(&mut world, "Crate", grid, 4, 5);
    world.run_tick();
    assert!(world.try_start_pull(human, crate_));
    let target = MapCoordinates {
        position: position(&world, human) + Vec2::new(0.0, 1.0),
        ..world.map_coordinates(human).unwrap()
    };
    move_pulled(&mut world, human, target);
    for _ in 0..60 {
        world.run_tick();
    }
    let at = position(&world, crate_);
    assert!(
        at.distance(target.position) < 0.3,
        "{at} → {:?}",
        target.position
    );
    assert!(world.get::<PullMoving>(crate_).is_none());
}

#[test]
fn pull_alerts_show_on_both_sides_and_clicking_releases() {
    let prototypes = format!(
        "{PROTOTYPES}{}",
        r#"
- type: alert
  id: Pulling
  icons: [ /Textures/Interface/Alerts/Pull/pulling.png ]
  clickEvent: !type:StopPullingAlertEvent
- type: alert
  id: Pulled
  icons: [ /Textures/Interface/Alerts/Pull/pulled.png ]
  clickEvent: !type:StopBeingPulledAlertEvent
"#
    );
    let mut world = world_with(&prototypes);
    let grid = station(&mut world);
    let human = spawn_at(&mut world, "Human", grid, 3, 5);
    let other = spawn_at(&mut world, "Human", grid, 2, 5);
    for entity in [human, other] {
        world
            .entity_mut(entity)
            .insert(crate::alerts::Alerts::default());
    }
    world.run_tick();
    assert!(world.try_start_pull(human, other));
    assert!(crate::alerts::is_showing_alert(&world, human, "Pulling"));
    assert!(crate::alerts::is_showing_alert(&world, other, "Pulled"));
    // Тот, кого тащат, вырывается нажатием на значок.
    assert!(crate::alerts::activate_alert(&mut world, other, "Pulled"));
    assert!(!world.is_pulled(other));
    assert!(!crate::alerts::is_showing_alert(&world, human, "Pulling"));
    assert!(!crate::alerts::is_showing_alert(&world, other, "Pulled"));
    // Тянущий отпускает нажатием на свой значок.
    assert!(world.try_start_pull(human, other));
    assert!(crate::alerts::activate_alert(&mut world, human, "Pulling"));
    assert!(!world.is_pulled(other));
}
