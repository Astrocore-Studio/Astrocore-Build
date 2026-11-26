use astrocore_ecs::GameWorldExt;
use astrocore_map::{MapCoordinates, TransformWorldExt};

use super::*;
use crate::damage::DamageSpecifier;
use crate::hands::HandsWorldExt;
use crate::testing::{spawn_at, station, world_with};

const ENTITIES: &str = r#"
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: DoAfter
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
  - type: MobState
  - type: MobThresholds
    thresholds:
      0: Alive
      100: Critical
      200: Dead
- type: entity
  id: Gauze
  components:
  - type: Item
  - type: Physics
    bodyType: Dynamic
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.1,-0.1,0.1,0.1"
        density: 20
        mask: [ ItemMask ]
  - type: Healing
    damageContainers: [ Biological ]
    damage:
      types:
        Blunt: -10
        Slash: -10
    delay: 1
- type: entity
  id: Brick
  parent: Gauze
  components:
  - type: DamageOtherOnHit
    damage:
      types:
        Blunt: 8
"#;

fn scene() -> (World, Entity, Entity, Entity) {
    let prototypes = format!("{}{ENTITIES}", crate::damage::tests::DAMAGE_PROTOTYPES);
    let mut world = world_with(&prototypes);
    let grid = station(&mut world);
    let medic = spawn_at(&mut world, "Human", grid, 2, 2);
    let patient = spawn_at(&mut world, "Human", grid, 3, 2);
    world.run_tick();
    (world, grid, medic, patient)
}

fn run_seconds(world: &mut World, seconds: f64) {
    let ticks = (seconds * f64::from(astrocore_ecs::DEFAULT_TICK_RATE)).ceil() as usize + 2;
    for _ in 0..ticks {
        world.run_tick();
    }
}

#[test]
fn gauze_heals_another_after_a_delay_and_is_used_up() {
    let (mut world, grid, medic, patient) = scene();
    let hurt = DamageSpecifier::new([
        ("Blunt", FixedPoint2::new(15)),
        ("Heat", FixedPoint2::new(5)),
    ]);
    world.change_damage(patient, &hurt, true, true, None);
    let gauze = spawn_at(&mut world, "Gauze", grid, 2, 2);
    world.run_tick();
    assert!(world.try_pickup_any_hand(medic, gauze, true));

    let click = world.map_coordinates(patient).unwrap();
    world.user_interaction(medic, click, Some(patient), false);
    // До конца задержки урон на месте.
    run_seconds(&mut world, 0.5);
    assert_eq!(world.total_damage(patient), FixedPoint2::new(20));
    run_seconds(&mut world, 1.0);
    let damage = world.get::<Damageable>(patient).unwrap().damage.clone();
    assert_eq!(damage.types["Blunt"], FixedPoint2::new(5));
    assert_eq!(damage.types["Heat"], FixedPoint2::new(5));
    assert!(!astrocore_ecs::LifecycleWorldExt::is_alive(&world, gauze));
}

#[test]
fn nothing_to_heal_and_self_heal_penalty() {
    let (mut world, grid, medic, _) = scene();
    let gauze = spawn_at(&mut world, "Gauze", grid, 2, 2);
    world.run_tick();
    // Ожог бинт не лечит: предмет остаётся, действия нет.
    world.change_damage(
        medic,
        &DamageSpecifier::new([("Heat", FixedPoint2::new(5))]),
        true,
        true,
        None,
    );
    assert!(!try_heal(&mut world, gauze, medic, medic));

    // Себя при половине урона до крита лечить вдвое дольше (множитель 3).
    world.change_damage(
        medic,
        &DamageSpecifier::new([("Blunt", FixedPoint2::new(45))]),
        true,
        true,
        None,
    );
    assert!((scaled_healing_penalty(&world, medic, 3.0) - 2.0).abs() < 1e-3);
    assert!(world.try_pickup_any_hand(medic, gauze, true));
    assert!(world.use_in_hand_interaction(medic, gauze));
    run_seconds(&mut world, 1.0);
    assert_eq!(world.total_damage(medic), FixedPoint2::new(50));
    run_seconds(&mut world, 1.1);
    assert_eq!(world.total_damage(medic), FixedPoint2::new(40));
}

#[test]
fn thrown_brick_hurts_whoever_it_hits() {
    // Бросает тот, кто стоит справа: путь до цели в двух тайлах свободен.
    let (mut world, grid, _, thrower) = scene();
    let target = spawn_at(&mut world, "Human", grid, 5, 2);
    let brick = spawn_at(&mut world, "Brick", grid, 3, 2);
    world.run_tick();
    assert!(world.try_pickup_any_hand(thrower, brick, true));
    // Предмет летит 80% пути до точки: целимся за центр цели, как игрок кликом.
    let mut aim: MapCoordinates = world.map_coordinates(target).unwrap();
    aim.position.x += 0.5;
    assert!(world.throw_held_item(thrower, aim));
    run_seconds(&mut world, 2.0);
    assert_eq!(world.total_damage(target), FixedPoint2::new(8));
}
