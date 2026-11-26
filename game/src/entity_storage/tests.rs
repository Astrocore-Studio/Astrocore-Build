use astrocore_ecs::GameWorldExt;
use astrocore_map::ContainerWorldExt;
use astrocore_movement::{InputMover, MoveButtons};
use astrocore_net::AppearanceValue;

use super::*;
use crate::interaction::InteractionWorldExt;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: InputMover
  - type: Hands
    startingHands:
      hand_right:
        location: Right
- type: entity
  id: Locker
  components:
  - type: Physics
    bodyType: Dynamic
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.25,-0.48,0.25,0.48"
        density: 75
        layer: [ MachineLayer ]
  - type: Appearance
  - type: EntityStorage
- type: entity
  id: SmallLocker
  parent: Locker
  components:
  - type: EntityStorage
    capacity: 1
- type: entity
  id: Pen
  components:
  - type: Item
  - type: Physics
    bodyType: Dynamic
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.1,-0.1,0.1,0.1"
- type: entity
  id: Statue
  parent: Pen
  components:
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.8,-0.8,0.8,0.8"
- type: entity
  id: Lamp
  components:
  - type: Physics
    bodyType: Dynamic
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.1,-0.1,0.1,0.1"
"#;

fn layer(world: &World, entity: Entity) -> i32 {
    world.get::<Fixtures>(entity).unwrap().fixtures["fix1"]
        .layer
        .0
}

#[test]
fn closing_takes_nearby_items_and_opening_drops_them() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let locker = spawn_at(&mut world, "Locker", grid, 3, 3);
    let pen = spawn_at(&mut world, "Pen", grid, 3, 3);
    let statue = spawn_at(&mut world, "Statue", grid, 3, 3);
    let lamp = spawn_at(&mut world, "Lamp", grid, 3, 3);
    let far = spawn_at(&mut world, "Pen", grid, 5, 5);
    world.run_tick();
    let closed_layer = layer(&world, locker);
    assert_ne!(closed_layer & groups::MID, 0);

    // Шкаф закрыт с начала: «использовать» открывает его, проход освобождается.
    assert!(world.interaction_activate(user, locker, true));
    assert!(world.is_open(locker));
    assert_eq!(
        layer(&world, locker) & (groups::MID | groups::HIGH | groups::LOW),
        0
    );
    assert_eq!(
        world.appearance(locker, VISUALS_OPEN),
        Some(&AppearanceValue::Bool(true))
    );

    // Закрытие забирает только предмет рядом: статуя велика, лампа не в белом списке.
    world.toggle_open(user, locker);
    assert!(!world.is_open(locker));
    let inside = world.container(locker, ENTITY_STORAGE_CONTAINER).unwrap();
    assert!(inside.contains(pen));
    assert!(!inside.contains(statue) && !inside.contains(lamp) && !inside.contains(far));
    assert_eq!(
        world.get::<InsideEntityStorage>(pen).unwrap().storage,
        locker
    );
    assert_eq!(layer(&world, locker), closed_layer);
    assert_eq!(
        world.appearance(locker, VISUALS_HAS_CONTENTS),
        Some(&AppearanceValue::Bool(true))
    );

    world.toggle_open(user, locker);
    assert!(world.containing_container(pen).is_none());
    assert!(world.get::<InsideEntityStorage>(pen).is_none());
    let at = world.coordinates(pen).unwrap();
    assert_eq!(at.parent, grid);
    assert!((at.position - Vec2::new(3.5, 3.5)).length() < 1e-3);
}

#[test]
fn capacity_limits_what_gets_in() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let locker = spawn_at(&mut world, "SmallLocker", grid, 3, 3);
    let first = spawn_at(&mut world, "Pen", grid, 3, 3);
    let second = spawn_at(&mut world, "Pen", grid, 3, 3);
    world.run_tick();
    world.open_storage(locker, None);
    world.close_storage(locker, None);
    let inside = world.container(locker, ENTITY_STORAGE_CONTAINER).unwrap();
    assert_eq!(inside.entities().len(), 1);
    assert!(inside.contains(first) != inside.contains(second));
    let outside = if inside.contains(first) {
        second
    } else {
        first
    };
    assert!(!world.can_insert_entity_storage(outside, locker));
}

#[test]
fn pressing_movement_inside_opens_the_locker_once_per_delay() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 3, 3);
    let locker = spawn_at(&mut world, "Locker", grid, 3, 3);
    world.run_tick();
    assert!(world.entity_storage_insert(user, locker));
    assert!(world.get::<InsideEntityStorage>(user).is_some());

    world.get_mut::<InputMover>(user).unwrap().held = MoveButtons::UP;
    world.run_tick();
    assert!(world.is_open(locker));
    assert!(world.containing_container(user).is_none());
}
