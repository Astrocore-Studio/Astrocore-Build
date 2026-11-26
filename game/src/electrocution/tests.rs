use astrocore_ecs::GameWorldExt;

use super::*;
use crate::damage::DamageableWorldExt;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: damageType
  id: Shock
  name: damage-type-shock
- type: statusEffect
  id: Electrocution
- type: entity
  id: LiveFence
  components:
  - type: Appearance
  - type: Electrified
    requirePower: false
- type: entity
  id: DeadFence
  components:
  - type: Electrified
- type: entity
  id: Victim
  components:
  - type: Damageable
  - type: Injurable
  - type: StatusEffects
    allowed: [ Electrocution ]
- type: entity
  id: Robot
  components:
  - type: Damageable
  - type: Injurable
"#;

fn shock(world: &World, entity: Entity) -> f32 {
    world.total_damage(entity).float()
}

#[test]
fn touching_a_live_fence_hurts() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    world.run_tick();
    let fence = spawn_at(&mut world, "LiveFence", grid, 2, 2);
    let victim = spawn_at(&mut world, "Victim", grid, 1, 2);
    assert!(try_do_electrified_act(&mut world, fence, victim, 1.0));
    // 7.5 × 0.75 (первое звено цепочки), округлено вниз.
    assert_eq!(shock(&world, victim), 5.0);
    assert!(matches!(
        world.appearance(fence, SHOW_SPARKS),
        Some(astrocore_net::AppearanceValue::Bool(true))
    ));
    for _ in 0..40 {
        world.run_tick();
    }
    assert!(matches!(
        world.appearance(fence, SHOW_SPARKS),
        Some(astrocore_net::AppearanceValue::Bool(false))
    ));
}

#[test]
fn insulation_and_missing_power_protect() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    world.run_tick();
    let fence = spawn_at(&mut world, "LiveFence", grid, 2, 2);
    let dead = spawn_at(&mut world, "DeadFence", grid, 3, 2);
    let victim = spawn_at(&mut world, "Victim", grid, 1, 2);
    // Без кабеля под током решётка не бьёт.
    assert!(!try_do_electrified_act(&mut world, dead, victim, 1.0));
    // Изолированный инструмент или перчатки — тоже.
    assert!(!try_do_electrified_act(&mut world, fence, victim, 0.0));
    world
        .entity_mut(victim)
        .insert(Insulated { coefficient: 0.0 });
    assert!(!try_do_electrified_act(&mut world, fence, victim, 1.0));
    assert_eq!(shock(&world, victim), 0.0);
    // Без статус-эффекта «Электрошок» (роботы) тока нет.
    let robot = spawn_at(&mut world, "Robot", grid, 2, 3);
    assert!(!try_do_electrified_act(&mut world, fence, robot, 1.0));
}
