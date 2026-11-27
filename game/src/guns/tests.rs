use astrocore_ecs::GameWorldExt;
use astrocore_net::{AppearanceValue, AppearanceWorldExt};

use super::*;
use crate::testing::{spawn_at, station, world_with};

fn prototypes() -> String {
    format!(
        "{}{}",
        crate::projectiles::tests::PROTOTYPES,
        r#"
- type: entity
  id: TestEmitter
  components:
  - type: Transform
    anchored: true
  - type: Physics
    bodyType: Static
  - type: Appearance
  - type: ApcPowerReceiver
    needsPower: false
  - type: PowerState
    idlePowerDraw: 5
    workingPowerDraw: 600
  - type: Gun
    burstCooldown: 1.4
    burstFireRate: 0.5
    selectedMode: Burst
  - type: NetworkPoweredAmmoProvider
    prototype: TestBolt
  - type: AutoShootGun
  - type: Emitter
- type: entity
  id: Engineer
  components:
  - type: Hands
  - type: ComplexInteraction
  - type: Physics
    bodyType: KinematicController
"#
    )
}

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

#[derive(bevy_ecs::resource::Resource, Default)]
struct Shots(u32);

fn activate(world: &mut World, user: Entity, target: Entity) {
    world.raise(
        target,
        &mut ActivateInWorld {
            user,
            target,
            complex: true,
            handled: false,
        },
    );
}

#[test]
fn emitter_fires_bursts_while_on() {
    let mut world = world_with(&prototypes());
    let grid = station(&mut world);
    world.init_resource::<Shots>();
    world.subscribe::<Gun, GunShot>(|world, _, event| {
        world.resource_mut::<Shots>().0 += event.projectiles.len() as u32;
    });
    let emitter = spawn_at(&mut world, "TestEmitter", grid, 8, 12);
    let user = spawn_at(&mut world, "Engineer", grid, 7, 12);
    assert_eq!(
        world.get::<ApcPowerReceiver>(emitter).unwrap().power_load,
        5.0
    );
    run(&mut world, 1.0);
    assert_eq!(world.resource::<Shots>().0, 0);

    activate(&mut world, user, emitter);
    assert!(working_state(&world, emitter));
    assert!(world.get::<AutoShootGun>(emitter).unwrap().enabled);
    assert_eq!(
        world.get::<ApcPowerReceiver>(emitter).unwrap().power_load,
        600.0
    );
    assert_eq!(
        world.appearance(emitter, POWER_STATE_VISUALS),
        Some(&AppearanceValue::Text("On".into()))
    );
    // Очередь: три выстрела через две секунды.
    run(&mut world, 4.1);
    assert_eq!(world.resource::<Shots>().0, 3);
    // Следующая очередь: через 2 с после третьего выстрела и ещё 1.4 с.
    run(&mut world, 3.0);
    assert_eq!(world.resource::<Shots>().0, 3);
    run(&mut world, 0.5);
    assert_eq!(world.resource::<Shots>().0, 4);

    // Выключили — стрельба прекратилась.
    activate(&mut world, user, emitter);
    assert!(!world.get::<AutoShootGun>(emitter).unwrap().enabled);
    let before = world.resource::<Shots>().0;
    run(&mut world, 4.0);
    assert_eq!(world.resource::<Shots>().0, before);
}
