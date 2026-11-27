use astrocore_ecs::{GameWorldExt, LifecycleWorldExt};

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Generator
  components:
  - type: Transform
    anchored: true
  - type: Physics
    bodyType: Static
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeCircle
          radius: 0.4
        layer: [ WallLayer ]
        mask: [ FullTileMask ]
  - type: Appearance
  - type: PointLight
    enabled: false
  - type: ContainmentFieldGenerator
- type: entity
  id: ContainmentField
  components:
  - type: Transform
    anchored: true
  - type: Physics
    bodyType: Static
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb {}
        layer: [ GlassLayer ]
        mask: [ FullTileMask ]
  - type: ContainmentField
    throwForce: 50
"#;

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

fn fields(world: &mut World) -> usize {
    world.query::<&ContainmentField>().iter(world).count()
}

#[test]
fn charged_generators_connect_and_drop_the_field_when_drained() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let a = spawn_at(&mut world, "Generator", grid, 2, 5);
    let b = spawn_at(&mut world, "Generator", grid, 7, 5);
    for generator in [a, b] {
        world
            .get_mut::<ContainmentFieldGenerator>(generator)
            .unwrap()
            .enabled = true;
    }
    // Один болт — мало (3 из 6).
    receive_power(&mut world, a, 3);
    assert!(generator(&world, a).unwrap().connections.is_empty());
    // У соседа заряда нет, но связь ставит заряженный.
    receive_power(&mut world, a, 3);
    let component = generator(&world, a).unwrap();
    assert!(component.is_connected);
    assert_eq!(component.connections.len(), 1);
    assert!(generator(&world, b).unwrap().is_connected);
    assert_eq!(fields(&mut world), 4);
    assert!(world.get::<PointLight>(a).unwrap().enabled);

    // 20 секунд без болтов: заряд 6 → 4, связь с разряженным соседом снимается.
    run(&mut world, 20.2);
    assert!(!generator(&world, a).unwrap().is_connected);
    assert!(!generator(&world, b).unwrap().is_connected);
    run(&mut world, 0.1);
    assert_eq!(fields(&mut world), 0);
}

#[test]
fn generators_connect_only_within_range() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let a = spawn_at(&mut world, "Generator", grid, 2, 5);
    let far = spawn_at(&mut world, "Generator", grid, 2, 14);
    for generator in [a, far] {
        world
            .get_mut::<ContainmentFieldGenerator>(generator)
            .unwrap()
            .enabled = true;
    }
    receive_power(&mut world, a, 6);
    // Девять клеток — дальше восьми.
    assert!(!generator(&world, a).unwrap().is_connected);
    let near = spawn_at(&mut world, "Generator", grid, 2, 9);
    world
        .get_mut::<ContainmentFieldGenerator>(near)
        .unwrap()
        .enabled = true;
    receive_power(&mut world, a, 3);
    assert!(generator(&world, a).unwrap().is_connected);
    assert!(!generator(&world, far).unwrap().is_connected);
    assert!(world.is_alive(near));
}

#[test]
fn deleting_a_generator_removes_its_fields() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let a = spawn_at(&mut world, "Generator", grid, 2, 5);
    let b = spawn_at(&mut world, "Generator", grid, 5, 5);
    for generator in [a, b] {
        world
            .get_mut::<ContainmentFieldGenerator>(generator)
            .unwrap()
            .enabled = true;
    }
    receive_power(&mut world, a, 6);
    assert_eq!(fields(&mut world), 2);
    world.queue_delete(b);
    run(&mut world, 0.2);
    assert_eq!(fields(&mut world), 0);
    assert!(!generator(&world, a).unwrap().is_connected);
}

const EMITTER: &str = r#"
- type: entity
  id: EmitterBolt
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
  - type: Tag
    tags: [ EmitterBolt ]
- type: entity
  id: Emitter
  components:
  - type: Transform
    anchored: true
  - type: Physics
    bodyType: Static
  - type: Appearance
  - type: ApcPowerReceiver
    needsPower: false
  - type: PowerState
    isTurnedOn: true
  - type: Gun
    burstCooldown: 1.4
    burstFireRate: 0.5
    selectedMode: Burst
  - type: NetworkPoweredAmmoProvider
    prototype: EmitterBolt
  - type: AutoShootGun
    enabled: true
"#;

#[test]
fn emitter_bolts_charge_a_generator() {
    let mut world = world_with(&format!("{PROTOTYPES}{EMITTER}"));
    let grid = station(&mut world);
    let a = spawn_at(&mut world, "Generator", grid, 2, 5);
    let b = spawn_at(&mut world, "Generator", grid, 6, 5);
    for generator in [a, b] {
        world
            .get_mut::<ContainmentFieldGenerator>(generator)
            .unwrap()
            .enabled = true;
    }
    // Излучатель над генератором стреляет вниз.
    spawn_at(&mut world, "Emitter", grid, 2, 9);
    run(&mut world, 0.5);
    assert_eq!(generator(&world, a).unwrap().power_buffer, 3);
    run(&mut world, 2.0);
    assert_eq!(generator(&world, a).unwrap().power_buffer, 6);
    assert!(generator(&world, a).unwrap().is_connected);
    assert_eq!(fields(&mut world), 3);
}
