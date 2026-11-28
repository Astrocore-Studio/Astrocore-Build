use astrocore_ecs::GameWorldExt;
use astrocore_map::TransformWorldExt;

use super::*;
use crate::hands::HandsWorldExt;
use crate::item_toggle::try_activate;
use crate::testing::{spawn_at, station, world_with};
use crate::tools::ToolWorldExt;

const PROTOTYPES: &str = r#"
- type: reagent
  id: WeldingFuel
- type: entity
  id: Solution
  components:
  - type: Solution
- type: entity
  parent: Solution
  id: SolutionWelder
  components:
  - type: Solution
    id: welder
    solution:
      maxVol: 10
      reagents:
      - ReagentId: WeldingFuel
        Quantity: 3
- type: entity
  parent: Solution
  id: SolutionTank
  components:
  - type: Solution
    id: tank
    solution:
      maxVol: 100
      reagents:
      - ReagentId: WeldingFuel
        Quantity: 100
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: DoAfter
  - type: Hands
    startingHands:
      hand_right:
        location: Right
- type: entity
  id: Welder
  components:
  - type: Item
  - type: SolutionManager
    solutions: [ SolutionWelder ]
  - type: RefillableSolution
    solution: welder
  - type: Tool
    qualities: [ Welding ]
  - type: ItemToggle
  - type: ItemToggleHot
  - type: IgnitionSource
  - type: Welder
- type: entity
  id: FuelTank
  components:
  - type: SolutionManager
    solutions: [ SolutionTank ]
  - type: DrainableSolution
    solution: tank
  - type: ReagentTank
    tankType: Fuel
"#;

fn n(value: i32) -> FixedPoint2 {
    FixedPoint2::new(value)
}

fn run_seconds(world: &mut World, seconds: f32) {
    let rate = f32::from(world.resource::<GameTime>().tick_rate);
    for _ in 0..(seconds * rate).ceil() as u32 {
        world.run_tick();
    }
}

#[test]
fn lit_welder_burns_fuel_and_goes_out_when_empty() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let welder = spawn_at(&mut world, "Welder", grid, 2, 2);
    world.run_tick();
    assert!(!crate::flammable::is_hot(&mut world, welder));
    assert!(try_activate(&mut world, welder, None, false));
    // Зажигание — 0,5.
    assert_eq!(
        fuel_and_capacity(&mut world, welder),
        (FixedPoint2::from_cents(250), n(10))
    );
    assert!(crate::flammable::is_hot(&mut world, welder));
    assert!(world.get::<IgnitionSource>(welder).unwrap().ignited);
    // По 1 в секунду; на нуле гаснет.
    run_seconds(&mut world, 3.1);
    assert_eq!(fuel_and_capacity(&mut world, welder).0, FixedPoint2::ZERO);
    assert!(!is_activated(&world, welder));
    assert!(!world.get::<Welder>(welder).unwrap().enabled);
    // Без топлива не зажечь.
    assert!(!try_activate(&mut world, welder, None, false));
}

#[test]
fn welding_needs_a_lit_welder_with_fuel_and_spends_it() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let welder = spawn_at(&mut world, "Welder", grid, 2, 2);
    let target = spawn_at(&mut world, "FuelTank", grid, 3, 2);
    assert!(world.try_pickup(user, welder, None, true));
    world.run_tick();
    // Не зажжён — нельзя.
    assert!(!world.use_tool(welder, user, Some(target), 1.0, &["Welding"], (), 1.0));
    assert!(try_activate(&mut world, welder, Some(user), false));
    // Топлива 2,5 — на 3 не хватит.
    assert!(!world.use_tool(welder, user, Some(target), 1.0, &["Welding"], (), 3.0));
    crate::tools::register_tool_event::<()>(&mut world);
    assert!(world.use_tool(welder, user, Some(target), 0.5, &["Welding"], (), 2.0));
    run_seconds(&mut world, 0.6);
    // 2,5 − 2 на сварку (горение ещё не списалось).
    assert_eq!(
        fuel_and_capacity(&mut world, welder).0,
        FixedPoint2::from_cents(50)
    );
}

#[test]
fn fuel_tank_refills_the_welder() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let welder = spawn_at(&mut world, "Welder", grid, 2, 2);
    let tank = spawn_at(&mut world, "FuelTank", grid, 3, 2);
    world.run_tick();
    let mut event = AfterInteract {
        user,
        used: welder,
        target: Some(tank),
        click: world.map_coordinates(tank).unwrap(),
        can_reach: true,
        handled: false,
    };
    world.raise(welder, &mut event);
    assert!(event.handled);
    assert_eq!(fuel_and_capacity(&mut world, welder).0, n(10));
    let drained = containers::drainable_solution(&mut world, tank).unwrap();
    assert_eq!(containers::solution(&world, drained).unwrap().volume, n(93));
}
