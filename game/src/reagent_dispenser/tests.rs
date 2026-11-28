use super::*;
use crate::storage::StorageWorldExt;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r##"
- type: reagent
  id: Water
  color: "#0000ff"
- type: entity
  id: Solution
  components:
  - type: Solution
- type: entity
  parent: Solution
  id: SolutionBeaker
  components:
  - type: Solution
    id: beaker
    solution:
      maxVol: 50
- type: entity
  parent: Solution
  id: SolutionJug
  components:
  - type: Solution
    id: jug
    solution:
      maxVol: 200
      reagents:
      - ReagentId: Water
        Quantity: 200
- type: entity
  id: Beaker
  name: beaker
  components:
  - type: Item
  - type: SolutionManager
    solutions: [ SolutionBeaker ]
  - type: FitsInDispenser
    solution: beaker
  - type: RefillableSolution
    solution: beaker
- type: entity
  id: WaterJug
  name: water jug
  components:
  - type: Item
  - type: SolutionManager
    solutions: [ SolutionJug ]
  - type: DrainableSolution
    solution: jug
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: Hands
    startingHands:
      hand_right:
        location: Right
- type: entity
  id: Dispenser
  components:
  - type: Storage
    grid:
    - 0,0,4,2
  - type: ItemSlots
  - type: ReagentDispenser
"##;

#[test]
fn dispenses_from_stored_jugs_into_the_beaker() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let dispenser = spawn_at(&mut world, "Dispenser", grid, 3, 2);
    let jug = spawn_at(&mut world, "WaterJug", grid, 3, 2);
    let beaker = spawn_at(&mut world, "Beaker", grid, 3, 2);
    assert!(world.storage_insert(dispenser, jug, None, false));
    assert!(item_slots::insert(
        &mut world,
        dispenser,
        OUTPUT_SLOT,
        beaker
    ));
    let location = *world
        .get::<Storage>(dispenser)
        .unwrap()
        .stored_items
        .get(&jug)
        .unwrap();
    let list = inventory(&mut world, dispenser);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].reagent_label, "water jug");
    assert_eq!(list[0].quantity, FixedPoint2::new(200));
    // По умолчанию — 10 за раз.
    dispense(&mut world, dispenser, location);
    let info = build_container_info(&mut world, Some(beaker)).unwrap();
    assert_eq!(info.current_volume, FixedPoint2::new(10));
    assert_eq!(info.max_volume, FixedPoint2::new(50));
    world
        .get_mut::<ReagentDispenser>(dispenser)
        .unwrap()
        .dispense_amount = 120;
    dispense(&mut world, dispenser, location);
    let info = build_container_info(&mut world, Some(beaker)).unwrap();
    assert_eq!(info.current_volume, FixedPoint2::new(50));
    // Банку — в руку.
    let mut event = BoundUiMessage {
        entity: dispenser,
        key: UI_KEY.into(),
        actor: user,
        message: EjectContainer {
            storage_location: location,
        },
    };
    world.raise(dispenser, &mut event);
    assert_eq!(world.held_items(user), [jug]);
}
