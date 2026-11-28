use astrocore_ecs::{GameTime, GameWorldExt};

use super::*;
use crate::solution::ReagentId;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: reagent
  id: Nutriment
  flavor: food
- type: reagent
  id: Sugar
  flavor: sweet
- type: reagent
  id: Salt
  flavor: salty
  flavorMinimum: 2
- type: flavor
  id: food
  description: flavor-base-food
- type: flavor
  id: sweet
  description: flavor-base-sweet
- type: flavor
  id: salty
  flavorType: Complex
  description: flavor-base-salty
- type: edible
  id: Food
  message: edible-nom
  otherMessage: edible-nom-other
  verb: edible-verb-food
  noun: edible-noun-food
  verbName: ingestion-verb-food
- type: edible
  id: Pill
  message: edible-swallow
  otherMessage: edible-gulp-other
  verb: edible-verb-pill
  noun: edible-noun-pill
  verbName: ingestion-verb-food
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
      maxVol: 20
- type: entity
  parent: Solution
  id: SolutionFood
  components:
  - type: Solution
    id: food
    solution:
      maxVol: 30
- type: entity
  id: TestStomach
  components:
  - type: Organ
    category: Stomach
  - type: Stomach
  - type: SolutionManager
    solutions: [ SolutionStomach ]
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: DoAfter
  - type: Hands
    startingHands:
      hand_right:
        location: Right
      hand_left:
        location: Left
  - type: Body
  - type: InitialBody
    organs:
      Stomach: TestStomach
- type: entity
  id: Mask
  components:
  - type: IngestionBlocker
- type: entity
  id: Pizza
  components:
  - type: Item
  - type: SolutionManager
    solutions: [ SolutionFood ]
  - type: Edible
    trash: [ Plate ]
  - type: FlavorProfile
    flavors: [ sweet ]
- type: entity
  id: Soup
  components:
  - type: Item
  - type: SolutionManager
    solutions: [ SolutionFood ]
  - type: Edible
    utensil: Spoon
    utensilRequired: true
- type: entity
  id: Plate
  components:
  - type: Item
- type: entity
  id: Spoon
  components:
  - type: Item
  - type: Utensil
    types: [ Spoon ]
- type: entity
  id: TestPill
  components:
  - type: Item
  - type: SolutionManager
    solutions: [ SolutionFood ]
  - type: Pill
  - type: Edible
    delay: 0.6
    forceFeedDelay: 2
    transferAmount: null
    edible: Pill
"#;

fn n(value: i32) -> FixedPoint2 {
    FixedPoint2::new(value)
}

struct Scene {
    world: World,
    grid: Entity,
    user: Entity,
}

fn scene() -> Scene {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    world.run_tick();
    Scene { world, grid, user }
}

fn held(s: &mut Scene, prototype: &str, reagents: &[(&str, i32)]) -> Entity {
    let item = spawn_at(&mut s.world, prototype, s.grid, 2, 2);
    s.world.run_tick();
    let soln = containers::try_get_solution(&mut s.world, item, "food").unwrap();
    for (reagent, amount) in reagents {
        containers::force_add_solution(
            &mut s.world,
            soln,
            &Solution::from_reagents([(ReagentId::new(*reagent), n(*amount))]),
        );
    }
    assert!(s.world.try_pickup(s.user, item, None, true));
    item
}

fn stomach_volume(world: &mut World, body: Entity) -> FixedPoint2 {
    let stomach = stomachs_of(world, body)[0];
    let soln = containers::try_get_solution(world, stomach, STOMACH_SOLUTION).unwrap();
    containers::solution(world, soln).unwrap().volume
}

fn run_seconds(world: &mut World, seconds: f32) {
    let rate = f32::from(world.resource::<GameTime>().tick_rate);
    for _ in 0..(seconds * rate).ceil() as u32 {
        world.run_tick();
    }
}

fn use_in_hand(world: &mut World, user: Entity, item: Entity) -> bool {
    let mut event = UseInHand {
        user,
        used: item,
        handled: false,
        apply_delay: true,
    };
    world.raise(item, &mut event);
    event.handled
}

#[test]
fn pill_is_swallowed_whole_and_disappears() {
    let mut s = scene();
    let pill = held(&mut s, "TestPill", &[("Sugar", 10)]);
    let user = s.user;
    assert!(use_in_hand(&mut s.world, user, pill));
    run_seconds(&mut s.world, 0.7);
    assert_eq!(stomach_volume(&mut s.world, user), n(10));
    assert!(s.world.get_entity(pill).is_err());
    // Не влезает целиком — не глотается.
    let big = held(&mut s, "TestPill", &[("Sugar", 15)]);
    assert!(use_in_hand(&mut s.world, user, big));
    run_seconds(&mut s.world, 0.7);
    assert_eq!(stomach_volume(&mut s.world, user), n(10));
    assert!(s.world.get_entity(big).is_ok());
}

#[test]
fn food_is_eaten_in_bites_and_leaves_trash_in_hand() {
    let mut s = scene();
    let pizza = held(&mut s, "Pizza", &[("Nutriment", 12)]);
    let user = s.user;
    assert!(use_in_hand(&mut s.world, user, pizza));
    // Кусок в секунду по 5, повтор сам: 5, 10, 12.
    run_seconds(&mut s.world, 1.05);
    assert_eq!(stomach_volume(&mut s.world, user), n(5));
    run_seconds(&mut s.world, 2.1);
    assert_eq!(stomach_volume(&mut s.world, user), n(12));
    assert!(s.world.get_entity(pizza).is_err());
    let plate = s
        .world
        .held_items(user)
        .into_iter()
        .find(|e| s.world.entity_name(*e) == "Plate" || s.world.get::<Edible>(*e).is_none());
    assert!(plate.is_some());
}

#[test]
fn feeding_another_breaks_on_move_and_a_mask_blocks_it() {
    let mut s = scene();
    let target = spawn_at(&mut s.world, "Human", s.grid, 3, 2);
    s.world.run_tick();
    let pizza = held(&mut s, "Pizza", &[("Nutriment", 5)]);
    let user = s.user;
    let click = |world: &mut World| {
        let mut event = AfterInteract {
            user,
            used: pizza,
            target: Some(target),
            click: world.map_coordinates(target).unwrap(),
            can_reach: true,
            handled: false,
        };
        world.raise(pizza, &mut event);
        event.handled
    };
    // Рот закрыт.
    s.world
        .entity_mut(target)
        .insert(IngestionBlocker { enabled: true });
    assert!(!click(&mut s.world));
    s.world.entity_mut(target).remove::<IngestionBlocker>();
    assert!(click(&mut s.world));
    // Как в SS14: `forceFeedDelay` не используется — та же секунда, что и самому.
    run_seconds(&mut s.world, 0.5);
    assert_eq!(stomach_volume(&mut s.world, target), FixedPoint2::ZERO);
    run_seconds(&mut s.world, 0.6);
    assert_eq!(stomach_volume(&mut s.world, target), n(5));
    // Кормлёный не повторяет сам.
    assert!(s.world.get_entity(pizza).is_err());
}

#[test]
fn soup_needs_a_spoon() {
    let mut s = scene();
    let soup = held(&mut s, "Soup", &[("Nutriment", 5)]);
    let user = s.user;
    assert!(!use_in_hand(&mut s.world, user, soup));
    let spoon = spawn_at(&mut s.world, "Spoon", s.grid, 2, 2);
    assert!(s.world.try_pickup_any_hand(user, spoon, true));
    assert!(use_in_hand(&mut s.world, user, soup));
}

#[test]
fn flavors_join_food_and_reagents_by_type() {
    let mut s = scene();
    let pizza = held(&mut s, "Pizza", &[("Nutriment", 1)]);
    let user = s.user;
    // Соли меньше порога — не чувствуется; питательное вещество не считается.
    let split = Solution::from_reagents([
        (ReagentId::new("Nutriment"), n(3)),
        (ReagentId::new("Salt"), n(1)),
    ]);
    assert_eq!(
        localized_flavors_message(&mut s.world, pizza, user, Some(&split)),
        "flavor-profile"
    );
    let salty = Solution::from_reagents([(ReagentId::new("Salt"), n(3))]);
    assert_eq!(
        localized_flavors_message(&mut s.world, pizza, user, Some(&salty)),
        "flavor-profile-multiple"
    );
    // Как в SS14: вкус соли остался у блюда.
    assert_eq!(
        s.world.get::<FlavorProfile>(pizza).unwrap().flavors,
        ["sweet", "salty"]
    );
}

#[test]
fn utensil_types_parse_from_one_or_many() {
    let one: UtensilType =
        astrocore_data::from_node(&astrocore_data::parse_yaml("Spoon").unwrap()).unwrap();
    assert_eq!(one, UtensilType::SPOON);
    let many: UtensilType =
        astrocore_data::from_node(&astrocore_data::parse_yaml("[ Fork, Knife ]").unwrap()).unwrap();
    assert_eq!(many.names(), "Fork, Knife");
}
