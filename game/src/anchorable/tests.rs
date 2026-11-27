use astrocore_ecs::GameWorldExt;
use astrocore_map::{SpawnWorldExt, Transform, TransformWorldExt};

use super::*;
use crate::hands::{Hand, HandsWorldExt};
use crate::interaction::InteractionWorldExt;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Wrench
  components:
  - type: Item
  - type: Tool
    qualities: [ Anchoring ]
- type: entity
  id: Machine
  components:
  - type: Physics
    bodyType: Dynamic
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.25,-0.25,0.25,0.25"
        layer: [ MachineLayer ]
        mask: [ MachineMask ]
  - type: Anchorable
  - type: Appearance
  - type: Lock
    locked: false
  - type: LockedAnchorable
- type: entity
  id: Engineer
  components:
  - type: Hands
  - type: ComplexInteraction
  - type: DoAfter
  - type: Physics
    bodyType: KinematicController
"#;

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

fn anchored(world: &World, entity: Entity) -> bool {
    world
        .get::<Transform>(entity)
        .is_some_and(Transform::anchored)
}

#[test]
fn wrench_anchors_snaps_and_unanchors() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Engineer", grid, 2, 2);
    world.add_hand(user, "right", Hand::default());
    let wrench = spawn_at(&mut world, "Wrench", grid, 2, 2);
    assert!(world.try_pickup(user, wrench, None, false));
    let machine = world
        .spawn_attached(
            "Machine",
            EntityCoordinates::new(grid, astrocore_math::Vec2::new(3.3, 2.7)),
        )
        .unwrap();
    let click = world.map_coordinates(machine).unwrap();
    assert!(world.interact_using(user, wrench, machine, click));
    assert!(!anchored(&world, machine));
    run(&mut world, 1.2);
    assert!(anchored(&world, machine));
    let position = world.get::<Transform>(machine).unwrap().local_position();
    assert_eq!((position.x, position.y), (3.5, 2.5));

    // Второй такой же в ту же клетку не встаёт.
    let other = spawn_at(&mut world, "Machine", grid, 3, 2);
    assert!(!try_toggle_anchor(&mut world, other, user, wrench));

    // Запертое не открепить.
    world.get_mut::<crate::lock::Lock>(machine).unwrap().locked = true;
    assert!(!try_toggle_anchor(&mut world, machine, user, wrench));
    world.get_mut::<crate::lock::Lock>(machine).unwrap().locked = false;
    assert!(world.interact_using(user, wrench, machine, click));
    run(&mut world, 1.2);
    assert!(!anchored(&world, machine));
}
