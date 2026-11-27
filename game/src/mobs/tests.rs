use astrocore_ecs::{EventWorldExt, GameWorldExt};
use astrocore_movement::InputMover;
use astrocore_net::{AppearanceValue, AppearanceWorldExt};
use bevy_ecs::resource::Resource;

use super::*;
use crate::action_blocker::ActionBlockerWorldExt;
use crate::damage::DamageSpecifier;
use crate::damage::DamageableWorldExt;
use crate::hands::HandsWorldExt;
use crate::standing::{STANDING_COLLISION_LAYER, StandingWorldExt};
use crate::testing::{spawn_at, station, world_with};

const ENTITIES: &str = r#"
- type: entity
  id: Mob
  components:
  - type: ComplexInteraction
  - type: InputMover
  - type: Appearance
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
  - type: Hands
    startingHands:
      hand_right:
        location: Right
  - type: Damageable
  - type: Injurable
    damageContainer: Biological
  - type: StandingState
  - type: MobState
  - type: MobThresholds
    thresholds:
      0: Alive
      100: Critical
      200: Dead
- type: entity
  id: Pen
  components:
  - type: Item
  - type: Physics
    bodyType: Dynamic
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.1,-0.1,0.1,0.1"
"#;

fn blunt(amount: i32) -> DamageSpecifier {
    DamageSpecifier::new([("Blunt", FixedPoint2::new(amount))])
}

#[derive(Resource, Default)]
struct Changes(Vec<(MobState, MobState)>);

#[test]
fn thresholds_move_the_mob_between_states() {
    let prototypes = format!("{}{ENTITIES}", crate::damage::tests::DAMAGE_PROTOTYPES);
    let mut world = world_with(&prototypes);
    world.init_resource::<Changes>();
    world.subscribe_broadcast::<MobStateChanged>(|world, event| {
        world
            .resource_mut::<Changes>()
            .0
            .push((event.old_state, event.new_state));
    });
    let grid = station(&mut world);
    let mob = spawn_at(&mut world, "Mob", grid, 2, 2);
    let pen = spawn_at(&mut world, "Pen", grid, 2, 2);
    world.run_tick();
    assert!(world.try_pickup_any_hand(mob, pen, true));
    assert!(world.is_mob_alive(mob));
    assert_eq!(world.incap_threshold(mob), Some(FixedPoint2::new(100)));
    let mask = |world: &World| {
        world
            .get::<astrocore_physics::Fixtures>(mob)
            .unwrap()
            .fixtures["fix1"]
            .mask
            .0
    };
    assert_ne!(mask(&world) & STANDING_COLLISION_LAYER, 0);

    // Крит: лежит, выронил вещь, не двигается и не действует.
    world.change_damage(mob, &blunt(120), true, true, None);
    assert!(world.is_mob_critical(mob));
    assert!(world.is_down(mob));
    assert_eq!(world.held_items(mob), Vec::<Entity>::new());
    assert!(!world.get::<InputMover>(mob).unwrap().can_move);
    assert!(!world.can_interact(mob, None));
    assert_eq!(mask(&world) & STANDING_COLLISION_LAYER, 0);
    assert_eq!(
        world.appearance(mob, VISUALS_STATE),
        Some(&AppearanceValue::from("Critical".to_owned()))
    );

    // Лечение ниже порога — снова на ногах.
    world.change_damage(mob, &blunt(-50), true, true, None);
    assert!(world.is_mob_alive(mob));
    assert!(!world.is_down(mob));
    assert!(world.get::<InputMover>(mob).unwrap().can_move);
    assert_ne!(mask(&world) & STANDING_COLLISION_LAYER, 0);

    // Смерть не отменяется лечением, только полным исцелением.
    world.change_damage(mob, &blunt(150), true, true, None);
    assert!(world.is_mob_dead(mob));
    world.change_damage(mob, &blunt(-200), true, true, None);
    assert!(world.is_mob_dead(mob));
    world.rejuvenate(mob);
    assert!(world.is_mob_alive(mob));
    assert_eq!(world.total_damage(mob), FixedPoint2::ZERO);

    assert_eq!(
        world.resource::<Changes>().0,
        vec![
            (MobState::Alive, MobState::Critical),
            (MobState::Critical, MobState::Alive),
            (MobState::Alive, MobState::Dead),
            (MobState::Dead, MobState::Alive),
        ]
    );
}

#[test]
fn moving_a_threshold_rechecks_the_state() {
    let prototypes = format!("{}{ENTITIES}", crate::damage::tests::DAMAGE_PROTOTYPES);
    let mut world = world_with(&prototypes);
    let grid = station(&mut world);
    let mob = spawn_at(&mut world, "Mob", grid, 2, 2);
    world.change_damage(mob, &blunt(60), true, true, None);
    assert!(world.is_mob_alive(mob));
    world.set_mob_state_threshold(mob, FixedPoint2::new(50), MobState::Critical);
    assert!(world.is_mob_critical(mob));
    assert_eq!(
        world.threshold_for_state(mob, MobState::Critical),
        Some(FixedPoint2::new(50))
    );
}

const HEALTH_ALERTS: &str = r#"
- type: alert
  id: HumanHealth
  category: Health
  icons:
  - { sprite: /Textures/Interface/Alerts/human_alive.rsi, state: health0 }
  - { sprite: /Textures/Interface/Alerts/human_alive.rsi, state: health1 }
  - { sprite: /Textures/Interface/Alerts/human_alive.rsi, state: health2 }
  - { sprite: /Textures/Interface/Alerts/human_alive.rsi, state: health3 }
  - { sprite: /Textures/Interface/Alerts/human_alive.rsi, state: health4 }
  minSeverity: 0
  maxSeverity: 4
- type: alert
  id: HumanCrit
  category: Health
  icons: [ { sprite: /Textures/Interface/Alerts/human_critical.rsi, state: critical } ]
- type: alert
  id: HumanDead
  category: Health
  icons: [ { sprite: /Textures/Interface/Alerts/human_dead.rsi, state: dead } ]
"#;

#[test]
fn health_alert_follows_damage_towards_the_next_threshold() {
    use crate::alerts::{Alerts, active_alerts};
    let prototypes = format!(
        "{}{ENTITIES}{HEALTH_ALERTS}",
        crate::damage::tests::DAMAGE_PROTOTYPES
    );
    let mut world = world_with(&prototypes);
    let grid = station(&mut world);
    let mob = spawn_at(&mut world, "Mob", grid, 2, 2);
    world.entity_mut(mob).insert(Alerts::default());
    let health = |world: &World| {
        active_alerts(world, mob)
            .and_then(|a| a.get("category:Health").cloned())
            .map(|s| (s.alert_type, s.severity))
    };
    world.change_damage(mob, &blunt(1), true, true, None);
    assert_eq!(health(&world), Some(("HumanHealth".into(), Some(0))));
    // 60 из 100 до крита — уровень round(0.6 × 4) = 2.
    world.change_damage(mob, &blunt(59), true, true, None);
    assert_eq!(health(&world), Some(("HumanHealth".into(), Some(2))));
    world.change_damage(mob, &blunt(60), true, true, None);
    assert_eq!(health(&world), Some(("HumanCrit".into(), None)));
    world.change_damage(mob, &blunt(100), true, true, None);
    assert_eq!(health(&world), Some(("HumanDead".into(), None)));
}
