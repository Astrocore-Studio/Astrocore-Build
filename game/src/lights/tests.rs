use astrocore_ecs::GameWorldExt;
use astrocore_net::AppearanceValue;

use super::*;
use crate::hands::Hand;
use crate::interaction::InteractionWorldExt;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r##"
- type: entity
  id: LightTube
  components:
  - type: Item
  - type: Appearance
  - type: LightBulb
    bulb: Tube
    color: "#FFE4CE"
    lightRadius: 7
    PowerUse: 25
- type: entity
  id: LightBulb
  components:
  - type: Item
  - type: LightBulb
    bulb: Bulb
- type: entity
  id: TestLight
  components:
  - type: Transform
    anchored: true
  - type: Appearance
  - type: PointLight
    enabled: true
  - type: ApcPowerReceiver
    needsPower: false
  - type: PoweredLight
    bulb: Tube
    hasLampOnSpawn: LightTube
- type: entity
  id: Engineer
  components:
  - type: Hands
  - type: ComplexInteraction
  - type: DoAfter
  - type: Physics
    bodyType: KinematicController
"##;

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

fn state(world: &World, light: Entity) -> Option<String> {
    match world.appearance(light, BULB_STATE) {
        Some(AppearanceValue::Text(text)) => Some(text.clone()),
        _ => None,
    }
}

fn lit(world: &World, light: Entity) -> bool {
    world.get::<PointLight>(light).is_some_and(|l| l.enabled)
}

#[test]
fn spawned_tube_lights_up_and_follows_power() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let light = spawn_at(&mut world, "TestLight", grid, 2, 2);
    assert!(bulb_of(&world, light).is_some());
    assert!(lit(&world, light));
    assert_eq!(state(&world, light).as_deref(), Some("On"));
    let point = world.get::<PointLight>(light).unwrap();
    assert_eq!(point.radius, 7.0);
    assert_eq!(
        world.get::<ApcPowerReceiver>(light).unwrap().power_load,
        25.0
    );

    // Без питания гаснет, лампа цела.
    world
        .get_mut::<ApcPowerReceiver>(light)
        .unwrap()
        .needs_power = true;
    update_light(&mut world, light);
    assert!(!lit(&world, light));
    assert_eq!(state(&world, light).as_deref(), Some("Off"));
    world
        .get_mut::<ApcPowerReceiver>(light)
        .unwrap()
        .needs_power = false;

    // Выключатель.
    set_light_on(&mut world, light, false);
    assert!(!lit(&world, light));
    assert_eq!(
        world.get::<ApcPowerReceiver>(light).unwrap().power_load,
        0.0
    );
    set_light_on(&mut world, light, true);
    assert!(lit(&world, light));
}

#[test]
fn broken_tube_comes_out_at_once_and_a_new_one_goes_in() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let light = spawn_at(&mut world, "TestLight", grid, 2, 2);
    let user = spawn_at(&mut world, "Engineer", grid, 1, 2);
    world.add_hand(user, "right", Hand::default());
    assert!(try_destroy_bulb(&mut world, light));
    assert!(!lit(&world, light));
    assert_eq!(state(&world, light).as_deref(), Some("Broken"));
    assert!(!try_destroy_bulb(&mut world, light));

    world.interact_hand(user, light);
    let broken = world.active_item(user).expect("лампа в руке");
    assert_eq!(world.get::<LightBulb>(broken).unwrap().state, "Broken");
    assert_eq!(state(&world, light).as_deref(), Some("Empty"));
    assert!(world.try_drop(user, "right", None, false));

    // Лампа не того вида не встаёт.
    let bulb = spawn_at(&mut world, "LightBulb", grid, 1, 2);
    assert!(!insert_bulb(&mut world, light, bulb));
    let tube = spawn_at(&mut world, "LightTube", grid, 1, 2);
    assert!(world.try_pickup(user, tube, None, false));
    let click = world.map_coordinates(light).unwrap();
    assert!(world.interact_using(user, tube, light, click));
    assert!(lit(&world, light));

    // Рабочую лампу вынимают с ожиданием.
    world.interact_hand(user, light);
    assert_eq!(bulb_of(&world, light), Some(tube));
    run(&mut world, 2.2);
    assert_eq!(bulb_of(&world, light), None);
    assert_eq!(world.active_item(user), Some(tube));
}
