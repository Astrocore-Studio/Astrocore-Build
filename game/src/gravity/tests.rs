use astrocore_ecs::GameWorldExt;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: TestGravityGenerator
  components:
  - type: Transform
    anchored: true
  - type: Appearance
  - type: ApcPowerReceiver
    needsPower: false
  - type: PowerCharge
    chargeRate: 0.5
    charge: 0
    idlePower: 50
    activePower: 2500
  - type: GravityGenerator
    lightRadiusMin: 0.75
    lightRadiusMax: 2.5
  - type: PointLight
- type: entity
  id: Mob
  components:
  - type: Physics
    bodyType: KinematicController
  - type: GravityAffected
- type: entity
  id: Ghost
  components:
  - type: Physics
    bodyType: KinematicController
  - type: MovementIgnoreGravity
"#;

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

#[test]
fn generator_charges_turns_gravity_on_and_off() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    world.entity_mut(grid).insert(Gravity {
        gravity_shake_sound: SoundSpecifier::path("/Audio/Effects/alert.ogg"),
        enabled: false,
        inherent: false,
    });
    let mob = spawn_at(&mut world, "Mob", grid, 3, 3);
    let ghost = spawn_at(&mut world, "Ghost", grid, 4, 3);
    assert!(is_weightless(&world, mob));
    assert!(!is_weightless(&world, ghost));

    let generator = spawn_at(&mut world, "TestGravityGenerator", grid, 8, 8);
    assert_eq!(
        world.get::<ApcPowerReceiver>(generator).unwrap().power_load,
        2500.0
    );
    run(&mut world, 1.0);
    assert!(is_weightless(&world, mob), "заряд ещё не полный");
    run(&mut world, 1.2);
    assert!(
        world
            .get::<GravityGenerator>(generator)
            .unwrap()
            .gravity_active
    );
    assert!(world.get::<Gravity>(grid).unwrap().enabled);
    assert!(!is_weightless(&world, mob));
    assert!(world.get::<PointLight>(generator).unwrap().radius > 2.4);

    // Выключили: разряд за две секунды, гравитация пропадает.
    set_switched_on(&mut world, generator, false);
    assert_eq!(
        world.get::<ApcPowerReceiver>(generator).unwrap().power_load,
        50.0
    );
    run(&mut world, 2.2);
    assert!(!world.get::<Gravity>(grid).unwrap().enabled);
    assert!(is_weightless(&world, mob));
}

#[test]
fn ui_state_reports_charging() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let generator = spawn_at(&mut world, "TestGravityGenerator", grid, 8, 8);
    world.get_mut::<PowerCharge>(generator).unwrap().charge = 0.5;
    let state = ui_state(&world, generator, 0.5).unwrap();
    assert_eq!(state.status, PowerChargeStatus::Charging);
    assert_eq!(state.eta_seconds, 1);
    assert_eq!(state.charge, 127);
    let state = ui_state(&world, generator, -0.5).unwrap();
    assert_eq!(state.status, PowerChargeStatus::Discharging);
}
