use astrocore_ecs::GameWorldExt;

use super::*;
use crate::body::BodyWorldExt;
use crate::mobs::MobStateWorldExt;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r##"
- type: damageType
  id: Poison
  name: damage-type-poison
- type: damageType
  id: Blunt
  name: damage-type-blunt
- type: damageGroup
  id: Brute
  name: damage-group-brute
  damageTypes: [ Blunt ]
- type: reagent
  id: Water
- type: reagent
  id: Toxin
  metabolisms:
    Bloodstream:
      metabolismRate: 1
      effects:
      - !type:HealthChange
        damage:
          types:
            Poison: 2
      metabolites:
        Waste: 0.5
- type: reagent
  id: Waste
- type: reagent
  id: Salve
  worksOnTheDead: true
  metabolisms:
    Bloodstream:
      effects:
      - !type:EvenHealthChange
        damage:
          Brute: -1
- type: reagent
  id: Antidote
  metabolisms:
    Bloodstream:
      effects:
      - !type:HealthChange
        conditions:
        - !type:ReagentCondition
          reagent: Toxin
          min: 5
        damage:
          types:
            Blunt: 1
      - !type:AdjustReagent
        conditions:
        - !type:MobStateCondition
          mobstate: Alive
        reagent: Toxin
        amount: -2
- type: entity
  id: Solution
  components:
  - type: Solution
- type: entity
  parent: Solution
  id: SolutionStomach
  components:
  - type: Solution
    id: stomach
    solution:
      maxVol: 150
- type: entity
  parent: Solution
  id: SolutionBloodstream
  components:
  - type: Solution
    id: bloodstream
    solution:
      maxVol: 300
- type: entity
  parent: Solution
  id: SolutionMetabolites
  components:
  - type: Solution
    id: metabolites
    solution:
      maxVol: 300
- type: entity
  id: TestStomach
  components:
  - type: Organ
    category: Stomach
  - type: SolutionManager
    solutions: [ SolutionStomach ]
  - type: Metabolizer
    stages: [ Digestion ]
- type: entity
  id: TestHeart
  components:
  - type: Organ
    category: Heart
  - type: Metabolizer
    maxReagents: 2
    stages: [ Bloodstream ]
- type: entity
  id: Patient
  components:
  - type: Body
  - type: InitialBody
    organs:
      Stomach: TestStomach
      Heart: TestHeart
  - type: SolutionManager
    solutions: [ SolutionBloodstream, SolutionMetabolites ]
  - type: Damageable
  - type: Injurable
  - type: MobState
"##;

fn n(value: i32) -> FixedPoint2 {
    FixedPoint2::new(value)
}

fn setup() -> (World, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let patient = spawn_at(&mut world, "Patient", grid, 2, 2);
    // Срок первой переработки — через секунду после появления: дальше `second` доводит
    // до очередного срока.
    world.run_tick();
    (world, patient)
}

fn organ(world: &World, body: Entity, category: &str) -> Entity {
    world
        .organs(body)
        .into_iter()
        .find(|o| {
            world
                .get::<Organ>(*o)
                .is_some_and(|c| c.category.as_deref() == Some(category))
        })
        .unwrap()
}

fn soln(world: &mut World, owner: Entity, name: &str) -> Entity {
    containers::try_get_solution(world, owner, name).unwrap()
}

fn amount(world: &World, soln: Entity, reagent: &str) -> FixedPoint2 {
    containers::solution(world, soln)
        .unwrap()
        .prototype_quantity(reagent)
}

fn add(world: &mut World, soln: Entity, reagent: &str, quantity: FixedPoint2) {
    containers::try_add_reagent(world, soln, ReagentId::new(reagent), quantity, None);
}

/// Одна секунда: следующий срок переработки.
fn second(world: &mut World) {
    for _ in 0..30 {
        world.run_tick();
    }
}

fn damage(world: &World, entity: Entity, kind: &str) -> FixedPoint2 {
    world
        .get::<Damageable>(entity)
        .and_then(|d| d.damage.types.get(kind).copied())
        .unwrap_or_default()
}

#[test]
fn stomach_passes_reagents_into_blood_at_half_efficacy() {
    let (mut world, patient) = setup();
    let stomach = organ(&world, patient, "Stomach");
    let stomach_soln = soln(&mut world, stomach, "stomach");
    let blood = soln(&mut world, patient, "bloodstream");
    let metabolites = soln(&mut world, patient, "metabolites");
    add(&mut world, stomach_soln, "Water", n(10));
    second(&mut world);
    // У воды нет стадии пищеварения: 0,25 за раз, в кровь — половина. Сердце могло уже
    // перенести её дальше, в метаболиты.
    assert_eq!(
        amount(&world, stomach_soln, "Water"),
        FixedPoint2::from_cents(975)
    );
    let passed = amount(&world, blood, "Water") + amount(&world, metabolites, "Water");
    assert_eq!(passed, FixedPoint2::from_cents(12));
}

#[test]
fn blood_reagents_act_and_leave_metabolites() {
    let (mut world, patient) = setup();
    let blood = soln(&mut world, patient, "bloodstream");
    let metabolites = soln(&mut world, patient, "metabolites");
    add(&mut world, blood, "Toxin", n(3));
    second(&mut world);
    assert_eq!(amount(&world, blood, "Toxin"), n(2));
    assert_eq!(damage(&world, patient, "Poison"), n(2));
    assert_eq!(
        amount(&world, metabolites, "Waste"),
        FixedPoint2::from_cents(50)
    );
    second(&mut world);
    assert_eq!(damage(&world, patient, "Poison"), n(4));
}

#[test]
fn heart_processes_at_most_its_limit() {
    let (mut world, patient) = setup();
    let blood = soln(&mut world, patient, "bloodstream");
    add(&mut world, blood, "Toxin", n(5));
    add(&mut world, blood, "Salve", n(5));
    add(&mut world, blood, "Antidote", n(5));
    second(&mut world);
    let processed = [
        amount(&world, blood, "Toxin") < n(5),
        amount(&world, blood, "Salve") < n(5),
        amount(&world, blood, "Antidote") < n(5),
    ];
    // У сердца предел 2 реагента за раз, порядок — случайный.
    assert_eq!(processed.iter().filter(|p| **p).count(), 2, "{processed:?}");
}

#[test]
fn conditions_check_the_solution_and_the_body() {
    let (mut world, patient) = setup();
    let blood = soln(&mut world, patient, "bloodstream");
    add(&mut world, blood, "Toxin", n(4));
    add(&mut world, blood, "Antidote", n(1));
    // Мало токсина в крови — без ушиба; противоядие убирает 2 токсина из раствора.
    let heart = organ(&world, patient, "Heart");
    metabolize_stage(&mut world, heart, "Bloodstream");
    assert_eq!(damage(&world, patient, "Blunt"), FixedPoint2::ZERO);
    let toxin = amount(&world, blood, "Toxin");
    // 4 − 2 противоядием − 1 переработкой самого токсина.
    assert_eq!(toxin, n(1));
}

#[test]
fn the_dead_metabolize_only_what_works_on_the_dead() {
    let (mut world, patient) = setup();
    world.change_mob_state(patient, MobState::Dead, None);
    let blood = soln(&mut world, patient, "bloodstream");
    add(&mut world, blood, "Toxin", n(3));
    add(&mut world, blood, "Salve", n(3));
    second(&mut world);
    // Как в SS14: у мёртвых реагент без `worksOnTheDead` пропускается целиком и не убывает.
    assert_eq!(amount(&world, blood, "Toxin"), n(3));
    assert_eq!(damage(&world, patient, "Poison"), FixedPoint2::ZERO);
    assert_eq!(amount(&world, blood, "Salve"), FixedPoint2::from_cents(250));
}

#[test]
fn excluded_reagents_are_left_alone() {
    let (mut world, patient) = setup();
    world.subscribe::<Damageable, MetabolismExclusion>(|_, _, ev| {
        ev.reagents.push(ReagentId::new("Toxin"));
    });
    let blood = soln(&mut world, patient, "bloodstream");
    add(&mut world, blood, "Toxin", n(3));
    second(&mut world);
    assert_eq!(amount(&world, blood, "Toxin"), n(3));
}

#[test]
fn metabolism_data_parses() {
    let mut world = world_with(PROTOTYPES);
    let toxin = reagent_metabolisms(&mut world, "Toxin");
    let stage = &toxin.stages["Bloodstream"];
    assert_eq!(stage.metabolism_rate, n(1));
    assert_eq!(
        stage.metabolites,
        [("Waste".to_owned(), FixedPoint2::from_cents(50))]
    );
    assert!(reagent_metabolisms(&mut world, "Salve").works_on_the_dead);
    let antidote = reagent_metabolisms(&mut world, "Antidote");
    assert_eq!(
        antidote.stages["Bloodstream"].metabolism_rate,
        FixedPoint2::from_cents(50)
    );
    assert_eq!(antidote.stages["Bloodstream"].effects.len(), 2);
}
