use astrocore_map::TransformWorldExt;

use super::*;
use crate::solution_container::tests::PROTOTYPES as SOLUTIONS;
use crate::testing::{spawn_at, station, world_with};

const EXTRA: &str = r#"
- type: entity
  parent: Beaker
  id: PourBeaker
  components:
  - type: SolutionTransfer
    canChangeTransferAmount: true
- type: entity
  parent: PourBeaker
  id: FullPourBeaker
  components:
  - type: Solution
    solution:
      reagents:
      - ReagentId: Water
        Quantity: 50
- type: entity
  id: Person
  components:
  - type: Hands
"#;

fn n(value: i32) -> FixedPoint2 {
    FixedPoint2::new(value)
}

fn volume(world: &World, entity: Entity) -> FixedPoint2 {
    containers::solution(world, entity).unwrap().volume
}

fn click(world: &mut World, user: Entity, held: Entity, target: Entity) -> bool {
    let mut event = AfterInteract {
        user,
        used: held,
        target: Some(target),
        click: world.map_coordinates(target).unwrap(),
        can_reach: true,
        handled: false,
    };
    on_after_interact(world, held, &mut event);
    event.handled
}

#[test]
fn held_container_pours_first_then_fills_from_target() {
    let mut world = world_with(&format!("{SOLUTIONS}{EXTRA}"));
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Person", grid, 1, 1);
    let full = spawn_at(&mut world, "FullPourBeaker", grid, 1, 1);
    let empty = spawn_at(&mut world, "PourBeaker", grid, 2, 1);
    // Своё сливается, цель доливается — 5 из полной в пустую.
    assert!(click(&mut world, user, full, empty));
    assert_eq!(volume(&world, full), n(45));
    assert_eq!(volume(&world, empty), n(5));
    // Пустая в руке: сливать нечего, но сначала проверяется слив — «пусто».
    let empty_two = spawn_at(&mut world, "PourBeaker", grid, 2, 2);
    assert!(click(&mut world, user, empty_two, full));
    assert_eq!(volume(&world, empty_two), FixedPoint2::ZERO);
    // Количество за раз — в пределах ёмкости.
    set_transfer_amount(&mut world, full, n(500), None);
    assert_eq!(
        world.get::<SolutionTransfer>(full).unwrap().transfer_amount,
        n(120)
    );
    assert!(click(&mut world, user, full, empty));
    // Мензурка на 60: долилась до краёв.
    assert_eq!(volume(&world, empty), n(50));
    assert_eq!(volume(&world, full), FixedPoint2::ZERO);
}

#[test]
fn amount_verbs_follow_bounds() {
    let mut world = world_with(&format!("{SOLUTIONS}{EXTRA}"));
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Person", grid, 1, 1);
    let beaker = spawn_at(&mut world, "PourBeaker", grid, 1, 1);
    let mut event = GetVerbs {
        kind: VerbKind::Alternative,
        user,
        target: beaker,
        using: None,
        can_interact: true,
        can_complex_interact: true,
        can_access: true,
        verbs: Vec::new(),
    };
    transfer_verbs(&mut world, beaker, &mut event);
    // «Своё» и 5, 10, 15, 30, 60, 120 по убыванию.
    let priorities: Vec<i32> = event.verbs.iter().map(|v| v.priority).collect();
    assert_eq!(priorities, vec![1, 0, -1, -2, -3, -4, -5]);
    // Без возможности менять — ничего.
    world
        .get_mut::<SolutionTransfer>(beaker)
        .unwrap()
        .can_change_transfer_amount = false;
    event.verbs.clear();
    transfer_verbs(&mut world, beaker, &mut event);
    assert!(event.verbs.is_empty());
}
