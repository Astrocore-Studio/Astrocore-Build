use astrocore_ecs::{GameWorldExt, LifecycleWorldExt};
use astrocore_map::TransformWorldExt;

use super::*;
use crate::interaction::InteractionWorldExt;
use crate::testing::{spawn_at, station, world_with};
use crate::verbs::VerbWorldExt;

const PROTOTYPES: &str = r#"
- type: stack
  id: Steel
  name: stack-steel
  spawn: SheetSteel1
  maxCount: 30
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: Hands
    startingHands:
      hand_right:
        location: Right
      hand_left:
        location: Left
- type: entity
  id: SheetSteel
  components:
  - type: Item
  - type: Appearance
  - type: Stack
    stackType: Steel
    count: 30
- type: entity
  id: SheetSteel1
  parent: SheetSteel
  components:
  - type: Stack
    stackType: Steel
    count: 1
"#;

fn scene() -> (World, Entity, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    (world, grid, user)
}

#[test]
fn counts_clamp_and_empty_stacks_disappear() {
    let (mut world, grid, _) = scene();
    let sheets = spawn_at(&mut world, "SheetSteel", grid, 2, 2);
    assert_eq!(world.stack_max_count(sheets), 30);
    world.set_stack_count(sheets, 50);
    assert_eq!(world.stack_count(sheets), 30);
    assert!(!world.try_use_stack(sheets, 31));
    assert!(world.try_use_stack(sheets, 30));
    world.run_tick();
    assert!(!world.is_alive(sheets));
}

#[test]
fn clicking_a_stack_with_a_stack_fills_the_one_in_hand() {
    let (mut world, grid, user) = scene();
    let floor = spawn_at(&mut world, "SheetSteel", grid, 3, 2);
    let held = spawn_at(&mut world, "SheetSteel", grid, 2, 2);
    world.set_stack_count(floor, 12);
    world.set_stack_count(held, 25);
    world.run_tick();
    assert!(world.try_pickup_any_hand(user, held, true));
    let click = world.map_coordinates(floor).unwrap();
    assert!(world.interact_using(user, held, floor, click));
    assert_eq!(world.stack_count(held), 30);
    assert_eq!(world.stack_count(floor), 7);
}

#[test]
fn splitting_puts_the_new_stack_in_hand() {
    let (mut world, grid, user) = scene();
    let floor = spawn_at(&mut world, "SheetSteel", grid, 3, 2);
    world.set_stack_count(floor, 12);
    world.run_tick();

    world.user_split(floor, user, 5);
    let held = world.active_item(user).expect("отделённая стопка в руке");
    assert_ne!(held, floor);
    assert_eq!(world.stack_count(held), 5);
    assert_eq!(world.stack_count(floor), 7);

    // С полной рукой той же стопкой отделённое добавляется к ней.
    world.user_split(floor, user, 2);
    assert_eq!(world.stack_count(held), 7);
    assert_eq!(world.stack_count(floor), 5);

    // Меню «Разделить»: 1 и 5 и «половина».
    let verbs = world.local_verbs(floor, user, &[VerbKind::Alternative]);
    let texts: Vec<&str> = verbs.iter().map(|v| v.text.as_str()).collect();
    assert!(
        texts.contains(&"1") && texts.contains(&"5") && !texts.contains(&"10"),
        "{texts:?}"
    );
    assert_eq!(verbs.len(), 3);
}
