use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: itemSize
  id: Tiny
  weight: 1
  defaultShape:
  - 0,0,0,0
- type: reagent
  id: Water
- type: reagent
  id: Sugar
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
  id: SolutionBottle
  components:
  - type: Solution
    id: drink
    solution:
      maxVol: 30
- type: entity
  id: Beaker
  components:
  - type: Item
  - type: SolutionManager
    solutions: [ SolutionBeaker ]
  - type: FitsInDispenser
    solution: beaker
- type: entity
  id: Bottle
  name: bottle
  components:
  - type: Item
  - type: SolutionManager
    solutions: [ SolutionBottle ]
- type: entity
  id: PillCanister
  name: pill canister
  components:
  - type: Item
  - type: Storage
    grid:
    - 0,0,2,0
- type: entity
  id: Pill
  name: pill
  components:
  - type: Item
    size: Tiny
  - type: Pill
  - type: Edible
    edible: Pill
- type: entity
  id: Human
  components:
  - type: Hands
    startingHands:
      hand_right:
        location: Right
- type: entity
  id: ChemMasterTest
  components:
  - type: ItemSlots
    slots:
      beakerSlot: {}
      outputSlot: {}
  - type: Solution
    id: buffer
  - type: ChemMaster
    pillDosageLimit: 20
"#;

fn n(value: i32) -> FixedPoint2 {
    FixedPoint2::new(value)
}

struct Scene {
    world: World,
    grid: Entity,
    master: Entity,
    beaker: Entity,
    user: Entity,
}

fn scene() -> Scene {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let master = spawn_at(&mut world, "ChemMasterTest", grid, 3, 2);
    let beaker = spawn_at(&mut world, "Beaker", grid, 3, 2);
    let soln = containers::try_get_solution(&mut world, beaker, "beaker").unwrap();
    containers::try_add_reagent(&mut world, soln, ReagentId::new("Water"), n(20), None);
    containers::try_add_reagent(&mut world, soln, ReagentId::new("Sugar"), n(10), None);
    assert!(item_slots::insert(&mut world, master, INPUT_SLOT, beaker));
    Scene {
        world,
        grid,
        master,
        beaker,
        user,
    }
}

fn beaker_amount(s: &mut Scene, reagent: &str) -> FixedPoint2 {
    let soln = containers::try_get_solution(&mut s.world, s.beaker, "beaker").unwrap();
    containers::solution(&s.world, soln)
        .unwrap()
        .prototype_quantity(reagent)
}

fn buffer_of(s: &mut Scene) -> Solution {
    buffer_solution(&mut s.world, s.master).unwrap()
}

#[test]
fn reagents_move_between_beaker_and_buffer_or_are_discarded() {
    let mut s = scene();
    let water = ReagentId::new("Water");
    transfer_reagents(&mut s.world, s.master, &water, n(15), false);
    assert_eq!(beaker_amount(&mut s, "Water"), n(5));
    assert_eq!(buffer_of(&mut s).prototype_quantity("Water"), n(15));
    // «Всё» — сколько есть.
    let sugar = ReagentId::new("Sugar");
    transfer_reagents(&mut s.world, s.master, &sugar, FixedPoint2::MAX, false);
    assert_eq!(buffer_of(&mut s).prototype_quantity("Sugar"), n(10));
    // Порядок буфера сохраняется при полном убирании.
    transfer_reagents(&mut s.world, s.master, &water, FixedPoint2::MAX, true);
    assert_eq!(beaker_amount(&mut s, "Water"), n(20));
    assert_eq!(buffer_of(&mut s).contents.len(), 1);
    discard_reagents(&mut s.world, s.master, &sugar, n(4), true);
    assert_eq!(buffer_of(&mut s).prototype_quantity("Sugar"), n(6));
    discard_reagents(&mut s.world, s.master, &water, n(5), false);
    assert_eq!(beaker_amount(&mut s, "Water"), n(15));
}

#[test]
fn pills_are_made_from_the_buffer_into_the_canister() {
    let mut s = scene();
    let water = ReagentId::new("Water");
    transfer_reagents(&mut s.world, s.master, &water, n(20), false);
    let canister = spawn_at(&mut s.world, "PillCanister", s.grid, 3, 2);
    assert!(item_slots::insert(
        &mut s.world,
        s.master,
        OUTPUT_SLOT,
        canister
    ));
    s.world.get_mut::<ChemMaster>(s.master).unwrap().pill_type = 7;
    let message = ChemMasterCreatePills {
        dosage: 5,
        number: 3,
        label: "вода".into(),
    };
    let user = s.user;
    create_pills(&mut s.world, s.master, user, &message);
    let pills: Vec<Entity> = s
        .world
        .container(canister, STORAGE_CONTAINER)
        .unwrap()
        .entities()
        .to_vec();
    assert_eq!(pills.len(), 3);
    assert_eq!(s.world.get::<Pill>(pills[0]).unwrap().pill_type, 7);
    assert_eq!(
        crate::labels::label_text(&s.world, pills[0]).as_deref(),
        Some("вода")
    );
    assert_eq!(
        crate::labels::label_text(&s.world, canister).as_deref(),
        Some("вода")
    );
    let soln = containers::try_get_solution(&mut s.world, pills[0], PILL_SOLUTION).unwrap();
    let solution = containers::solution(&s.world, soln).unwrap();
    assert_eq!(solution.volume, n(5));
    assert_eq!(solution.max_volume, n(5));
    assert_eq!(buffer_of(&mut s).volume, n(5));
    // Места в баночке больше нет; доза выше предела не принимается.
    let info = output_container_info(&mut s.world, Some(canister)).unwrap();
    assert_eq!((info.current_volume, info.max_volume), (n(3), n(3)));
    create_pills(&mut s.world, s.master, user, &message);
    assert_eq!(buffer_of(&mut s).volume, n(5));
}

#[test]
fn bottles_are_filled_from_the_beaker_source() {
    let mut s = scene();
    let bottle = spawn_at(&mut s.world, "Bottle", s.grid, 3, 2);
    assert!(item_slots::insert(
        &mut s.world,
        s.master,
        OUTPUT_SLOT,
        bottle
    ));
    s.world.get_mut::<ChemMaster>(s.master).unwrap().draw_source = ChemMasterDrawSource::External;
    let user = s.user;
    let message = ChemMasterOutputToBottle {
        dosage: 12,
        label: String::new(),
    };
    output_to_bottle(&mut s.world, s.master, user, &message);
    let soln = containers::try_get_solution(&mut s.world, bottle, BOTTLE_SOLUTION).unwrap();
    assert_eq!(containers::solution(&s.world, soln).unwrap().volume, n(12));
    assert_eq!(
        beaker_amount(&mut s, "Water") + beaker_amount(&mut s, "Sugar"),
        n(18)
    );
    // Больше, чем влезет в бутылку, — отказ.
    let too_much = ChemMasterOutputToBottle {
        dosage: 20,
        label: String::new(),
    };
    output_to_bottle(&mut s.world, s.master, user, &too_much);
    assert_eq!(containers::solution(&s.world, soln).unwrap().volume, n(12));
}

#[test]
fn sorting_cycles_through_all_types() {
    let mut sort = ChemMasterSortingType::None;
    for expected in [
        ChemMasterSortingType::Alphabetical,
        ChemMasterSortingType::Quantity,
        ChemMasterSortingType::Latest,
        ChemMasterSortingType::None,
    ] {
        sort = sort.next();
        assert_eq!(sort, expected);
    }
}
