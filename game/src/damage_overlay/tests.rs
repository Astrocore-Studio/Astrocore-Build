use super::*;
use crate::damage::{DamageSpecifier, DamageableWorldExt};
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Mob
  components:
  - type: Damageable
  - type: Injurable
    damageContainer: Biological
  - type: MobState
  - type: MobThresholds
    thresholds:
      0: Alive
      100: Critical
      200: Dead
  - type: DamageOverlay
"#;

fn blunt(amount: i32) -> DamageSpecifier {
    DamageSpecifier::new([("Blunt", FixedPoint2::new(amount))])
}

fn overlay(world: &World, entity: Entity) -> DamageOverlay {
    world.get::<DamageOverlay>(entity).unwrap().clone()
}

#[test]
fn levels_follow_pain_crit_and_death() {
    let prototypes = format!("{}{PROTOTYPES}", crate::damage::tests::DAMAGE_PROTOTYPES);
    let mut world = world_with(&prototypes);
    let grid = station(&mut world);
    let mob = spawn_at(&mut world, "Mob", grid, 2, 2);
    // Чуть-чуть — без пульсации.
    world.change_damage(mob, &blunt(4), true, true, None);
    assert_eq!(overlay(&world, mob).pain_level, 0.0);
    // Половина до крита — боль 0,5.
    world.change_damage(mob, &blunt(46), true, true, None);
    assert!((overlay(&world, mob).pain_level - 0.5).abs() < 1e-4);
    // Крит: 150 из 200 до смерти.
    world.change_damage(mob, &blunt(100), true, true, None);
    let crit = overlay(&world, mob);
    assert_eq!(crit.current_state, MobState::Critical);
    assert!((crit.crit_level - 0.75).abs() < 1e-4);
    assert_eq!(crit.pain_level, 0.0);
    world.change_damage(mob, &blunt(60), true, true, None);
    let dead = overlay(&world, mob);
    assert_eq!(dead.current_state, MobState::Dead);
    assert_eq!(dead.crit_level, 0.0);
}
