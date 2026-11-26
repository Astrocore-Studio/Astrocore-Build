use astrocore_ecs::GameWorldExt;
use astrocore_map::TransformWorldExt;

use super::*;
use crate::hands::HandsWorldExt;
use crate::interaction::InteractionWorldExt;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Human
  components:
  - type: Hands
  - type: ComplexInteraction
  - type: Physics
    bodyType: KinematicController
- type: entity
  id: Multitool
  components:
  - type: Item
  - type: Tool
    qualities: [ Pulsing ]
- type: entity
  id: AmeShielding
  components:
  - type: Transform
    anchored: true
  - type: AmeShield
  - type: Appearance
  - type: NodeContainer
    nodes:
      ame: !type:AdjacentNode
        nodeGroupID: AMEngine
- type: entity
  id: AmePartFlatpack
  components:
  - type: Item
  - type: Flatpack
    entity: AmeShielding
- type: entity
  id: AmeJar
  components:
  - type: Item
  - type: AmeFuelContainer
- type: entity
  id: AmeController
  components:
  - type: Transform
    anchored: true
  - type: Appearance
  - type: AmeController
    fuelSlot:
      whitelist:
        components:
        - AmeFuelContainer
  - type: ItemSlots
  - type: NodeContainer
    nodes:
      ame: !type:AdjacentNode
        nodeGroupID: AMEngine
  - type: PowerSupplier
    supplyRate: 0
"#;

fn human(world: &mut World, grid: Entity) -> Entity {
    let user = spawn_at(world, "Human", grid, 1, 1);
    world.add_hand(user, "right", crate::hands::Hand::default());
    user
}

#[test]
fn power_follows_the_ss14_formula() {
    assert!((calculate_power(2, 1) - 200_000.0 * 4f32.log10()).abs() < 1.0);
    assert_eq!(calculate_power(0, 1), 0.0);
    assert_eq!(calculate_power(2, 0), 0.0);
}

#[test]
fn unpacked_shielding_makes_a_core_and_the_controller_powers_up() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = human(&mut world, grid);
    let tool = spawn_at(&mut world, "Multitool", grid, 1, 1);
    assert!(world.try_pickup(user, tool, None, false));
    // Девять упаковок распаковываются мультитулом в квадрат 3×3 рядом с игроком.
    for x in 2..5 {
        for y in 1..4 {
            let pack = spawn_at(&mut world, "AmePartFlatpack", grid, x, y);
            let at = world.map_coordinates(pack).unwrap();
            world.interact_using(user, tool, pack, at);
        }
    }
    world.run_tick();
    let controller = spawn_at(&mut world, "AmeController", grid, 5, 2);
    world.run_tick();
    let engine = engine_of(&world, controller).unwrap();
    assert_eq!(engine.cores.len(), 1);
    assert_eq!(engine.master, Some(controller));

    // Банка — в слот, впрыск включён.
    world.try_drop(user, "right", None, false);
    let jar = spawn_at(&mut world, "AmeJar", grid, 1, 1);
    assert!(world.try_pickup(user, jar, None, false));
    let at = world.map_coordinates(controller).unwrap();
    world.interact_using(user, jar, controller, at);
    assert_eq!(slot_item(&world, controller, FUEL_SLOT), Some(jar));
    set_injecting(&mut world, controller, true);
    // Запертый на время впрыска слот банку не отдаёт.
    assert!(!try_eject_to_hands(
        &mut world,
        controller,
        FUEL_SLOT,
        Some(user)
    ));
    // Контроллер обновляется раз в 10 секунд: за 11 секунд после включения — один впрыск.
    for _ in 0..(11 * 30) {
        world.run_tick();
    }
    let supply = world.get::<PowerSupplier>(controller).unwrap().supply_rate;
    assert!((supply - calculate_power(2, 1)).abs() < 1.0, "{supply}");
    assert_eq!(world.get::<AmeFuelContainer>(jar).unwrap().fuel_amount, 498);
    let state = ui_state(&world, controller).unwrap();
    assert!(state.injecting && state.is_master);
    assert_eq!(state.core_count, 1);
    assert_eq!(
        world.appearance(controller, "enum.AmeControllerVisuals.DisplayState"),
        Some(&astrocore_net::AppearanceValue::Text("On".into()))
    );

    // Выключение снимает мощность.
    set_injecting(&mut world, controller, false);
    assert_eq!(
        world.get::<PowerSupplier>(controller).unwrap().supply_rate,
        0.0
    );
}

#[test]
fn overinjection_wears_the_cores_down() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    for x in 2..5 {
        for y in 1..4 {
            spawn_at(&mut world, "AmeShielding", grid, x, y);
        }
    }
    let controller = spawn_at(&mut world, "AmeController", grid, 5, 2);
    let jar = spawn_at(&mut world, "AmeJar", grid, 1, 1);
    crate::item_slots::insert(&mut world, controller, FUEL_SLOT, jar);
    world.run_tick();
    world
        .get_mut::<AmeController>(controller)
        .unwrap()
        .injection_amount = 8;
    set_injecting(&mut world, controller, true);
    for _ in 0..(21 * 30) {
        world.run_tick();
    }
    let stability = world.get::<AmeController>(controller).unwrap().stability;
    assert!(stability < 100, "{stability}");
}
