use astrocore_ecs::GameWorldExt;
use astrocore_map::ContainerKind;

use super::*;
use crate::fixed_point::FixedPoint2;
use crate::solution::ReagentId;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: reagent
  id: Water
  specificHeat: 4
- type: reagent
  id: Hydrogen
- type: reagent
  id: Oxygen
- type: reaction
  id: Electrolysis
  requiredMixerCategories: [ Electrolysis ]
  reactants:
    Water:
      amount: 2
  products:
    Hydrogen: 2
    Oxygen: 1
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
  id: Beaker
  components:
  - type: Item
  - type: SolutionManager
    solutions: [ SolutionBeaker ]
  - type: MixableSolution
    solution: beaker
  - type: FitsInDispenser
    solution: beaker
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
  id: Electrolyser
  components:
  - type: SolutionContainerMixer
    mixDuration: 5
  - type: ReactionMixer
    reactionTypes: [ Electrolysis ]
  - type: Appearance
- type: entity
  id: Rod
  components:
  - type: Item
  - type: ReactionMixer
    reactionTypes: [ Electrolysis ]
    timeToMix: 1
- type: entity
  id: Hotplate
  components:
  - type: SolutionHeater
    heatPerSecond: 160
  - type: ItemPlacer
  - type: Appearance
"#;

fn n(value: i32) -> FixedPoint2 {
    FixedPoint2::new(value)
}

fn beaker_with_water(world: &mut World, grid: Entity) -> Entity {
    let beaker = spawn_at(world, "Beaker", grid, 2, 2);
    let soln = containers::try_get_solution(world, beaker, "beaker").unwrap();
    containers::try_add_reagent(world, soln, ReagentId::new("Water"), n(10), None);
    beaker
}

fn quantity(world: &mut World, beaker: Entity, reagent: &str) -> FixedPoint2 {
    let soln = containers::try_get_solution(world, beaker, "beaker").unwrap();
    containers::solution(world, soln)
        .unwrap()
        .prototype_quantity(reagent)
}

fn run_seconds(world: &mut World, seconds: f32) {
    let rate = f32::from(world.resource::<GameTime>().tick_rate);
    for _ in 0..(seconds * rate).ceil() as u32 {
        world.run_tick();
    }
}

#[test]
fn electrolyser_mixes_its_container_when_done() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let machine = spawn_at(&mut world, "Electrolyser", grid, 3, 2);
    let beaker = beaker_with_water(&mut world, grid);
    // Без особого смешивания вода не разлагается.
    assert_eq!(quantity(&mut world, beaker, "Hydrogen"), FixedPoint2::ZERO);
    // Пустую — не включить.
    try_start_mix(&mut world, machine, Some(user));
    assert!(!world.get::<SolutionContainerMixer>(machine).unwrap().mixing);
    world.ensure_container(machine, "mixer", ContainerKind::Slot);
    world
        .force_insert_into_container(beaker, machine, "mixer")
        .unwrap();
    let mut activate = ActivateInWorld {
        user,
        target: machine,
        complex: true,
        handled: false,
    };
    world.raise(machine, &mut activate);
    assert!(world.get::<SolutionContainerMixer>(machine).unwrap().mixing);
    // Во время работы ёмкость не вынуть.
    assert!(world.remove_from_container(beaker, None).is_err());
    run_seconds(&mut world, 5.1);
    assert!(!world.get::<SolutionContainerMixer>(machine).unwrap().mixing);
    assert_eq!(quantity(&mut world, beaker, "Hydrogen"), n(10));
    assert_eq!(quantity(&mut world, beaker, "Oxygen"), n(5));
}

#[test]
fn machine_type_mixer_works_on_click_with_a_do_after() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let rod = spawn_at(&mut world, "Rod", grid, 2, 2);
    let beaker = beaker_with_water(&mut world, grid);
    let mut event = AfterInteract {
        user,
        used: rod,
        target: Some(beaker),
        click: astrocore_map::TransformWorldExt::map_coordinates(&world, beaker).unwrap(),
        can_reach: true,
        handled: false,
    };
    world.raise(rod, &mut event);
    assert!(event.handled);
    run_seconds(&mut world, 1.1);
    assert_eq!(quantity(&mut world, beaker, "Hydrogen"), n(10));
}

#[test]
fn hotplate_heats_what_lies_on_it() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let plate = spawn_at(&mut world, "Hotplate", grid, 3, 2);
    let beaker = beaker_with_water(&mut world, grid);
    let temperature = |world: &mut World| {
        let soln = containers::try_get_solution(world, beaker, "beaker").unwrap();
        containers::solution(world, soln).unwrap().temperature
    };
    let before = temperature(&mut world);
    let mut placed = astrocore_physics::StartCollide {
        our_entity: plate,
        other_entity: beaker,
        our_fixture: "itemcollide".into(),
        other_fixture: "fix1".into(),
    };
    world.raise(plate, &mut placed);
    assert!(world.get::<ActiveSolutionHeater>(plate).is_some());
    // 160 Дж/с на 10 воды с теплоёмкостью 4: 4 К в секунду.
    run_seconds(&mut world, 1.0);
    let heated = temperature(&mut world);
    assert!((heated - before - 4.0).abs() < 0.3, "{before} → {heated}");
    let mut removed = astrocore_physics::EndCollide {
        our_entity: plate,
        other_entity: beaker,
        our_fixture: "itemcollide".into(),
        other_fixture: "fix1".into(),
    };
    world.raise(plate, &mut removed);
    assert!(world.get::<ActiveSolutionHeater>(plate).is_none());
}
