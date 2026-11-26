use astrocore_ecs::GameWorldExt;
use astrocore_map::TransformWorldExt;

use super::*;
use crate::actions::{Action, RequestPerformAction};
use crate::interaction::InteractionWorldExt;
use crate::testing::{spawn_at, station, world_with};

const ENTITIES: &str = r#"
- type: entity
  id: ActionCombatModeToggle
  components:
  - type: Action
  - type: Appearance
  - type: InstantAction
    event: !type:ToggleCombatActionEvent
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: Hands
    startingHands:
      hand_right:
        location: Right
  - type: Physics
    bodyType: KinematicController
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeCircle
          radius: 0.35
        density: 185
        layer: [ MobLayer ]
        mask: [ MobMask ]
  - type: Damageable
  - type: Injurable
    damageContainer: Biological
  - type: CombatMode
  - type: MeleeWeapon
    hidden: true
    soundHit:
      collection: MetalThud
    damage:
      types:
        Blunt: 5
- type: entity
  id: Toolbox
  components:
  - type: Item
  - type: MeleeWeapon
    attackRate: 2
    damage:
      types:
        Blunt: 10
"#;

fn scene() -> (World, Entity, Entity, Entity) {
    let prototypes = format!("{}{ENTITIES}", crate::damage::tests::DAMAGE_PROTOTYPES);
    let mut world = world_with(&prototypes);
    let grid = station(&mut world);
    let fighter = spawn_at(&mut world, "Human", grid, 2, 2);
    let target = spawn_at(&mut world, "Human", grid, 3, 2);
    world.run_tick();
    (world, grid, fighter, target)
}

fn at(world: &World, entity: Entity) -> MapCoordinates {
    world.map_coordinates(entity).unwrap()
}

fn seconds(world: &mut World, seconds: f64) {
    for _ in 0..(seconds * f64::from(astrocore_ecs::DEFAULT_TICK_RATE)).ceil() as usize + 1 {
        world.run_tick();
    }
}

#[test]
fn combat_mode_is_an_action_and_fists_hit_at_their_rate() {
    let (mut world, _, fighter, target) = scene();
    // Без режима боя удара нет.
    assert!(!world.attempt_light_attack(fighter, fighter, Some(target), at(&world, target)));

    let action = world
        .get::<CombatMode>(fighter)
        .unwrap()
        .combat_toggle_action_entity
        .unwrap();
    let request = RequestPerformAction {
        action: *world.get::<NetEntity>(action).unwrap(),
        entity_target: None,
        coordinates_target: None,
    };
    assert!(world.try_perform_action(fighter, &request));
    assert!(world.is_in_combat_mode(fighter));
    assert!(world.get::<Action>(action).unwrap().toggled);

    assert_eq!(world.melee_weapon_of(fighter), Some(fighter));
    assert!(world.attempt_light_attack(fighter, fighter, Some(target), at(&world, target)));
    assert_eq!(world.total_damage(target), FixedPoint2::new(5));
    // Темп кулака — удар в секунду.
    assert!(!world.attempt_light_attack(fighter, fighter, Some(target), at(&world, target)));
    seconds(&mut world, 1.0);
    assert!(world.attempt_light_attack(fighter, fighter, Some(target), at(&world, target)));
    assert_eq!(world.total_damage(target), FixedPoint2::new(10));
}

#[test]
fn weapon_in_hand_replaces_fists_and_distance_misses() {
    let (mut world, grid, fighter, target) = scene();
    let toolbox = spawn_at(&mut world, "Toolbox", grid, 2, 2);
    world.run_tick();
    assert!(world.try_pickup_any_hand(fighter, toolbox, true));
    world.set_in_combat_mode(fighter, true);
    assert_eq!(world.melee_weapon_of(fighter), Some(toolbox));
    assert!(world.attempt_light_attack(fighter, toolbox, Some(target), at(&world, target)));
    assert_eq!(world.total_damage(target), FixedPoint2::new(10));

    // Далеко — взмах впустую.
    let far = spawn_at(&mut world, "Human", grid, 8, 2);
    world.run_tick();
    seconds(&mut world, 0.5);
    assert!(world.attempt_light_attack(fighter, toolbox, Some(far), at(&world, far)));
    assert_eq!(world.total_damage(far), FixedPoint2::ZERO);

    // В режиме боя клик предметом по человеку не взаимодействие; клик пустой рукой по
    // предмету — по-прежнему взять.
    let hand = world.hand_holding(fighter, toolbox).unwrap();
    assert!(world.try_drop(fighter, &hand, None, true));
    world.run_tick();
    world.user_interaction(fighter, at(&world, toolbox), Some(toolbox), false);
    assert_eq!(world.active_item(fighter), Some(toolbox));
}
