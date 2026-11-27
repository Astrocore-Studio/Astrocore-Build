use astrocore_ecs::{GameWorldExt, LifecycleWorldExt};
use astrocore_map::TransformWorldExt;
use astrocore_math::Vec2;

use super::*;
use crate::testing::{spawn_at, station, world_with};

pub(crate) const PROTOTYPES: &str = r#"
- type: entity
  id: TestBolt
  components:
  - type: Physics
  - type: Fixtures
    fixtures:
      projectile:
        shape: !type:PhysShapeAabb
          bounds: "-0.2,-0.2,0.2,0.2"
        hard: false
        mask: [ Opaque ]
  - type: Projectile
    damage:
      types:
        Heat: 14
- type: entity
  id: Target
  components:
  - type: Transform
    anchored: true
  - type: Physics
    bodyType: Static
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb {}
        layer: [ Opaque ]
  - type: Damageable
  - type: Injurable
"#;

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

#[test]
fn bolt_hits_a_wall_deals_damage_and_disappears() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let target = spawn_at(&mut world, "Target", grid, 10, 3);
    let bolt = spawn_at(&mut world, "TestBolt", grid, 2, 3);
    shoot_projectile(
        &mut world,
        bolt,
        Vec2::new(1.0, 0.0),
        Vec2::ZERO,
        None,
        None,
        PROJECTILE_SPEED,
    );
    run(&mut world, 0.5);
    assert!(!world.is_alive(bolt));
    assert_eq!(world.total_damage(target).to_string(), "14");
    // Скорость 40 м/с: за такт больше клетки, но снаряд не проходит насквозь.
    let behind = world.map_coordinates(target).unwrap();
    assert!(behind.position.x > 10.0);
}

#[test]
fn bolt_ignores_its_shooter_at_first() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let shooter = spawn_at(&mut world, "Target", grid, 3, 3);
    let bolt = spawn_at(&mut world, "TestBolt", grid, 3, 3);
    shoot_projectile(
        &mut world,
        bolt,
        Vec2::new(0.0, 1.0),
        Vec2::ZERO,
        Some(shooter),
        None,
        10.0,
    );
    run(&mut world, 0.1);
    assert!(world.is_alive(bolt));
    assert_eq!(world.total_damage(shooter).to_string(), "0");
}
