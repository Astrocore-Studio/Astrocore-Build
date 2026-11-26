use std::collections::BTreeSet;

use super::*;
use crate::hands::HandsWorldExt;
use crate::testing::{spawn_at, station, world_with};

pub(crate) const PROTOTYPES: &str = r#"
- type: accessLevel
  id: Engineering
- type: accessLevel
  id: Security
- type: accessGroup
  id: AllAccess
  tags: [ Engineering, Security ]
- type: entity
  id: Human
  components:
  - type: Hands
  - type: ComplexInteraction
  - type: DoAfter
  - type: Physics
    bodyType: KinematicController
- type: entity
  id: EngineeringIDCard
  components:
  - type: Item
  - type: IdCard
    fullName: Urist
  - type: Access
    tags: [ Engineering ]
- type: entity
  id: CaptainIDCard
  components:
  - type: Item
  - type: IdCard
  - type: Access
    groups: [ AllAccess ]
- type: entity
  id: EngineerPDA
  components:
  - type: Item
  - type: Pda
    id: EngineeringIDCard
- type: entity
  id: EngineeringDoor
  components:
  - type: AccessReader
    access: [ [ Engineering ] ]
- type: entity
  id: SecurityOrEngineering
  components:
  - type: AccessReader
    access: [ [ Security ], [ Engineering ] ]
    denyTags: [ Banned ]
"#;

pub(crate) fn human(world: &mut World, grid: Entity) -> Entity {
    let user = spawn_at(world, "Human", grid, 1, 1);
    world.add_hand(user, "right", crate::hands::Hand::default());
    user
}

#[test]
fn readers_check_sets_of_levels() {
    let reader: AccessReader = astrocore_data::from_node(
        &astrocore_data::parse_yaml("access: [ [ A, B ], [ C ] ]\ndenyTags: [ X ]\n").unwrap(),
    )
    .unwrap();
    let tags = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
    assert!(tags_allowed(&tags(&["A", "B"]), &reader));
    assert!(tags_allowed(&tags(&["C"]), &reader));
    assert!(!tags_allowed(&tags(&["A"]), &reader));
    assert!(!tags_allowed(&tags(&["C", "X"]), &reader));
}

#[test]
fn cards_in_hand_and_in_pda_open_doors() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = human(&mut world, grid);
    let door = spawn_at(&mut world, "EngineeringDoor", grid, 2, 1);
    assert!(!is_allowed(&world, user, door));

    let pda = spawn_at(&mut world, "EngineerPDA", grid, 1, 1);
    // Стартовая карта уже в КПК.
    assert!(slot_item(&world, pda, PDA_ID_SLOT).is_some());
    assert!(world.try_pickup(user, pda, None, false));
    assert!(is_allowed(&world, user, door));

    // Группа AllAccess разворачивается в уровни.
    let captain = spawn_at(&mut world, "CaptainIDCard", grid, 3, 3);
    assert_eq!(
        world.get::<Access>(captain).unwrap().tags,
        ["Engineering", "Security"].map(String::from).into()
    );
    let either = spawn_at(&mut world, "SecurityOrEngineering", grid, 2, 2);
    assert!(is_allowed(&world, user, either));
}
