use astrocore_net::{AppearanceValue, AppearanceWorldExt};

use super::*;
use crate::testing::{spawn_at, station, world_with};

pub(crate) const PROTOTYPES: &str = r##"
- type: reagent
  id: Water
  name: reagent-name-water
  desc: reagent-desc-water
  physicalDesc: reagent-physical-desc-translucent
  color: "#75b1f0"
  recognizable: true
  specificHeat: 4
- type: reagent
  id: Ethanol
  name: reagent-name-ethanol
  desc: reagent-desc-ethanol
  physicalDesc: reagent-physical-desc-strong-smelling
  color: "#b05b3c"
- type: entity
  id: Solution
  components:
  - type: Solution
- type: entity
  abstract: true
  parent: Solution
  id: SolutionBeaker
  components:
  - type: Solution
    id: beaker
- type: entity
  abstract: true
  id: SolutionSmall
  components:
  - type: Solution
    solution:
      maxVol: 60
- type: entity
  parent: Solution
  id: SolutionStomach
  components:
  - type: Solution
    id: stomach
    solution:
      maxVol: 30
- type: entity
  parent: [SolutionBeaker, SolutionSmall]
  id: Beaker
  components:
  - type: Item
  - type: Appearance
  - type: SolutionContainerVisuals
    maxFillLevels: 6
    fillBaseName: beaker
  - type: ExaminableSolution
    solution: beaker
  - type: RefillableSolution
    solution: beaker
  - type: DrainableSolution
    solution: beaker
- type: entity
  parent: Beaker
  id: WaterBeaker
  components:
  - type: Solution
    solution:
      reagents:
      - ReagentId: Water
        Quantity: 50
- type: entity
  id: Body
  components:
  - type: SolutionManager
    solutions:
    - SolutionStomach
"##;

fn setup() -> (World, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    (world, grid)
}

fn n(value: i32) -> FixedPoint2 {
    FixedPoint2::new(value)
}

#[derive(Component, Default)]
struct Changes(u32);

#[test]
fn solution_fields_merge_from_several_parents() {
    let (mut world, grid) = setup();
    let beaker = spawn_at(&mut world, "Beaker", grid, 1, 1);
    let soln = try_get_solution(&mut world, beaker, "beaker").expect("раствор мензурки");
    assert_eq!(soln, beaker);
    assert_eq!(solution(&world, soln).unwrap().max_volume, n(60));
    let full = spawn_at(&mut world, "WaterBeaker", grid, 2, 1);
    let (_, water) = solution_named(&mut world, full, "beaker").unwrap();
    assert_eq!(water.prototype_quantity("Water"), n(50));
    assert_eq!(water.max_volume, n(60));
    assert_eq!(refillable_solution(&mut world, full), Some(full));
    assert!(try_get_solution(&mut world, full, "other").is_none());
}

#[test]
fn adding_reports_changes_overflow_and_appearance() {
    let (mut world, grid) = setup();
    let beaker = spawn_at(&mut world, "Beaker", grid, 1, 1);
    world.entity_mut(beaker).insert(Changes::default());
    world.subscribe::<Changes, SolutionChanged>(|world, entity, _| {
        world.get_mut::<Changes>(entity).unwrap().0 += 1;
    });
    let (all, accepted) = try_add_reagent(&mut world, beaker, ReagentId::new("Water"), n(45), None);
    assert!(all);
    assert_eq!(accepted, n(45));
    // Влезает только 15.
    let (all, accepted) =
        try_add_reagent(&mut world, beaker, ReagentId::new("Ethanol"), n(30), None);
    assert!(!all);
    assert_eq!(accepted, n(15));
    assert_eq!(world.get::<Changes>(beaker).unwrap().0, 2);
    assert_eq!(
        world.appearance(beaker, VISUALS_FILL_FRACTION),
        Some(&AppearanceValue::Float(1.0))
    );
    // Основной — вода.
    assert_eq!(
        world.appearance(beaker, VISUALS_BASE_OVERRIDE),
        Some(&AppearanceValue::Text("Water".into()))
    );
    let taken = split_solution(&mut world, beaker, n(30));
    assert_eq!(taken.volume, n(30));
    assert_eq!(solution(&world, beaker).unwrap().volume, n(30));
    // Сверх вместимости принудительно — перелив отрезается.
    force_add_solution(&mut world, beaker, &Solution::of("Water", n(40)));
    assert_eq!(solution(&world, beaker).unwrap().volume, n(60));
}

#[test]
fn managers_spawn_and_ensure_solutions() {
    let (mut world, grid) = setup();
    let body = spawn_at(&mut world, "Body", grid, 1, 1);
    let stomach = try_get_solution(&mut world, body, "stomach").expect("желудок");
    assert_ne!(stomach, body);
    assert_eq!(solution_owner(&world, stomach), body);
    assert_eq!(solution(&world, stomach).unwrap().max_volume, n(30));
    let (blood, existed) = ensure_solution(&mut world, body, "bloodstream").unwrap();
    assert!(!existed);
    assert_eq!(
        ensure_solution(&mut world, body, "bloodstream"),
        Some((blood, true))
    );
    let names: Vec<String> = enumerate_solutions(&world, body)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"stomach".to_owned()) && names.contains(&"bloodstream".to_owned()));
    // Новый раствор без вместимости: сначала задаём её.
    set_capacity(&mut world, blood, n(100));
    try_add_reagent(&mut world, blood, ReagentId::new("Water"), n(5), None);
    try_add_reagent(&mut world, stomach, ReagentId::new("Water"), n(3), None);
    assert_eq!(total_prototype_quantity(&world, body, "Water"), n(8));
}

#[test]
fn examine_tells_volume_look_and_recognized_reagents() {
    let (mut world, grid) = setup();
    let beaker = spawn_at(&mut world, "WaterBeaker", grid, 1, 1);
    try_add_reagent(&mut world, beaker, ReagentId::new("Ethanol"), n(5), None);
    let lines = crate::examine::examine_text(&mut world, beaker, beaker);
    assert!(lines.contains(&"examinable-solution-on-examine-volume".to_owned()));
    assert!(lines.contains(&"shared-solution-container-component-on-examine-main-text".to_owned()));
    // Узнаётся только вода.
    assert!(lines.contains(&"examinable-solution-has-recognizable-chemicals".to_owned()));
    let scan = solution_examine_lines(&world, solution(&world, beaker).unwrap());
    assert_eq!(scan.len(), 4);
}

#[test]
fn vague_volume_names_follow_fill() {
    let examinable: ExaminableSolution =
        astrocore_data::from_node(&astrocore_data::parse_yaml("solution: beaker").unwrap())
            .unwrap();
    let mut solution = Solution::of("Water", n(60));
    solution.max_volume = n(60);
    assert_eq!(examined_volume(&examinable, &solution, "Urist"), "full");
    solution.max_volume = n(100);
    assert_eq!(examined_volume(&examinable, &solution, "Urist"), "halffull");
    assert_eq!(
        examined_volume(&examinable, &solution, "Alice"),
        "halfempty"
    );
    solution.max_volume = n(1000);
    assert_eq!(
        examined_volume(&examinable, &solution, "Urist"),
        "mostlyempty"
    );
    let exact = ExaminableSolution {
        exact_volume: true,
        ..examinable
    };
    assert_eq!(examined_volume(&exact, &solution, "Urist"), "exact");
}
