use astrocore_ecs::{EventWorldExt, GameWorldExt};
use astrocore_map::TransformWorldExt;
use bevy_ecs::resource::Resource;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: Hands
    startingHands:
      hand_right:
        location: Right
- type: entity
  id: Rock
  components:
  - type: Item
- type: entity
  abstract: true
  id: BaseAction
  components:
  - type: Action
  - type: Appearance
- type: entity
  id: ActionShout
  parent: BaseAction
  components:
  - type: Action
    useDelay: 2
  - type: InstantAction
    event: !type:ShoutActionEvent
      volume: 3
- type: entity
  id: ActionPoke
  parent: BaseAction
  components:
  - type: TargetAction
    range: 2
  - type: EntityTargetAction
    canTargetSelf: false
    whitelist:
      components:
      - Item
    event: !type:PokeActionEvent
"#;

#[derive(Resource, Default)]
struct Log(Vec<String>);

fn scene() -> (World, Entity, Entity) {
    let mut world = world_with(PROTOTYPES);
    world.init_resource::<Log>();
    world.subscribe::<Actions, ActionEvent>(|world, _, event| {
        let volume = event
            .data
            .get("volume")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .to_owned();
        world
            .resource_mut::<Log>()
            .0
            .push(format!("{} {volume}", event.kind));
        event.handled = true;
        event.toggle = event.kind == "ShoutActionEvent";
    });
    let grid = station(&mut world);
    let human = spawn_at(&mut world, "Human", grid, 2, 2);
    world.run_tick();
    (world, grid, human)
}

fn request(world: &World, action: Entity, target: Option<Entity>) -> RequestPerformAction {
    RequestPerformAction {
        action: *world.get::<NetEntity>(action).unwrap(),
        entity_target: target.map(|t| *world.get::<NetEntity>(t).unwrap()),
        coordinates_target: None,
    }
}

#[test]
fn instant_action_raises_its_event_toggles_and_cools_down() {
    let (mut world, _, human) = scene();
    let shout = world.add_action(human, "ActionShout", None).unwrap();
    world.run_tick();
    assert_eq!(world.actions_of(human), vec![shout]);
    assert!(
        world
            .container(human, ACTIONS_CONTAINER)
            .unwrap()
            .contains(shout)
    );

    assert!(world.try_perform_action(human, &request(&world, shout, None)));
    assert_eq!(
        world.resource::<Log>().0,
        vec!["ShoutActionEvent 3".to_owned()]
    );
    assert!(world.get::<Action>(shout).unwrap().toggled);
    // Пауза после использования.
    assert!(world.is_cooldown_active(shout));
    assert!(!world.try_perform_action(human, &request(&world, shout, None)));
    for _ in 0..(2 * usize::from(astrocore_ecs::DEFAULT_TICK_RATE) + 2) {
        world.run_tick();
    }
    assert!(world.try_perform_action(human, &request(&world, shout, None)));
    assert!(!world.get::<Action>(shout).unwrap().toggled);

    // Чужое или выключенное действие не выполняется.
    let grid = grid_of(&world, human);
    let other = spawn_at(&mut world, "Human", grid, 5, 5);
    world.run_tick();
    assert!(!world.try_perform_action(other, &request(&world, shout, None)));
    world.set_action_enabled(shout, false);
    for _ in 0..(2 * usize::from(astrocore_ecs::DEFAULT_TICK_RATE) + 2) {
        world.run_tick();
    }
    assert!(!world.try_perform_action(human, &request(&world, shout, None)));
}

fn grid_of(world: &World, entity: Entity) -> Entity {
    world.parent(entity).unwrap()
}

#[test]
fn entity_target_action_checks_whitelist_self_and_range() {
    let (mut world, grid, human) = scene();
    let poke = world.add_action(human, "ActionPoke", None).unwrap();
    let near = spawn_at(&mut world, "Rock", grid, 3, 2);
    let far = spawn_at(&mut world, "Rock", grid, 7, 2);
    let person = spawn_at(&mut world, "Human", grid, 2, 3);
    world.run_tick();

    assert!(!world.try_perform_action(human, &request(&world, poke, None)));
    assert!(!world.try_perform_action(human, &request(&world, poke, Some(human))));
    assert!(!world.try_perform_action(human, &request(&world, poke, Some(person))));
    assert!(!world.try_perform_action(human, &request(&world, poke, Some(far))));
    assert!(world.try_perform_action(human, &request(&world, poke, Some(near))));
    assert_eq!(
        world.resource::<Log>().0,
        vec!["PokeActionEvent -".to_owned()]
    );
}
