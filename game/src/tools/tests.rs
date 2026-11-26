use astrocore_ecs::{EventWorldExt, GameWorldExt};
use bevy_ecs::resource::Resource;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: tool
  id: Prying
  name: tool-quality-prying-name
  toolName: tool-quality-prying-tool-name
  spawn: Crowbar
- type: tool
  id: Screwing
  name: tool-quality-screwing-name
  toolName: tool-quality-screwing-tool-name
  spawn: Screwdriver
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
  id: Crowbar
  components:
  - type: Item
  - type: Tool
    qualities: [ Prying ]
    speedModifier: 2
- type: entity
  id: Multitool
  components:
  - type: Item
  - type: Tool
  - type: MultipleTool
    entries:
    - behavior: [ Prying ]
    - behavior: [ Screwing ]
- type: entity
  id: Crate
  components:
  - type: Item
"#;

#[derive(Debug, Clone)]
struct Pried;

#[derive(Resource, Default)]
struct Done(Vec<Entity>);

#[test]
fn tool_use_waits_scaled_by_speed_and_reaches_the_target() {
    let mut world = world_with(PROTOTYPES);
    world.init_resource::<Done>();
    register_tool_event::<Pried>(&mut world);
    world.subscribe::<crate::item::Item, DoAfterDone<Pried>>(|world, target, event| {
        if !event.cancelled {
            world.resource_mut::<Done>().0.push(target);
        }
    });
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 2, 2);
    let crate_ = spawn_at(&mut world, "Crate", grid, 3, 2);
    world.run_tick();
    assert!(crate::hands::HandsWorldExt::try_pickup_any_hand(
        &mut world, user, crowbar, true
    ));

    // Отвёрткой лом не работает.
    assert!(!world.use_tool(crowbar, user, Some(crate_), 1.0, &["Screwing"], Pried, 0.0));
    // Две секунды при скорости 2 — одна.
    assert!(world.use_tool(crowbar, user, Some(crate_), 2.0, &["Prying"], Pried, 0.0));
    let rate = usize::from(astrocore_ecs::DEFAULT_TICK_RATE);
    for _ in 0..rate / 2 {
        world.run_tick();
    }
    assert!(world.resource::<Done>().0.is_empty());
    for _ in 0..rate / 2 + 3 {
        world.run_tick();
    }
    assert_eq!(world.resource::<Done>().0, vec![crate_]);
}

#[test]
fn multitool_cycles_its_qualities() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let tool = spawn_at(&mut world, "Multitool", grid, 2, 2);
    assert!(world.has_quality(tool, "Prying"));
    assert!(world.cycle_multiple_tool(tool, None));
    assert!(world.has_quality(tool, "Screwing") && !world.has_quality(tool, "Prying"));
    assert!(world.cycle_multiple_tool(tool, None));
    assert!(world.has_quality(tool, "Prying"));
    assert_eq!(
        format_list(&world, &["a".into(), "b".into(), "c".into()]),
        "a, b, generic-and c"
    );
}
