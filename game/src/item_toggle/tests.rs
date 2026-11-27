use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Torch
  components:
  - type: ItemToggle
  - type: ItemToggleExaminableStatus
- type: entity
  id: Lit
  components:
  - type: ItemToggle
    activated: true
"#;

#[derive(bevy_ecs::component::Component, Default)]
struct Log(Vec<bool>);

#[derive(bevy_ecs::component::Component)]
struct Refuse;

#[test]
fn toggling_raises_events_and_attempts_can_refuse() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let torch = spawn_at(&mut world, "Torch", grid, 1, 1);
    world.entity_mut(torch).insert(Log::default());
    world.subscribe::<Log, ItemToggled>(|world, entity, event| {
        world
            .get_mut::<Log>(entity)
            .unwrap()
            .0
            .push(event.activated);
    });
    world.subscribe::<Refuse, ItemToggleActivateAttempt>(|_, _, event| {
        event.cancelled = true;
    });
    assert!(!is_activated(&world, torch));
    assert!(toggle(&mut world, torch, None, false));
    assert!(is_activated(&world, torch));
    assert!(toggle(&mut world, torch, None, false));
    assert!(!is_activated(&world, torch));
    assert_eq!(world.get::<Log>(torch).unwrap().0, vec![true, false]);
    world.entity_mut(torch).insert(Refuse);
    assert!(!try_activate(&mut world, torch, None, false));
    assert!(!is_activated(&world, torch));
    // Уже включённый из прототипа; без компонента — «включён».
    let lit = spawn_at(&mut world, "Lit", grid, 2, 2);
    assert!(is_activated(&world, lit));
    assert!(is_activated(&world, grid));
}
