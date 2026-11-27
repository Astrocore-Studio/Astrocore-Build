use astrocore_ecs::{GameTime, GameWorldExt};

use super::*;
use crate::item_toggle::{is_activated, toggle};
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Cell
  components:
  - type: Battery
    maxCharge: 300
    startingCharge: 300
  - type: PowerCell
- type: entity
  id: Rock
  components:
  - type: Item
- type: entity
  id: Lamp
  components:
  - type: ItemToggle
  - type: ToggleCellDraw
  - type: PowerCellDraw
    drawRate: 100
    useCharge: 50
  - type: PowerCellSlot
    cellSlotId: cell_slot
  - type: ItemSlots
    slots:
      cell_slot:
        startingItem: Cell
"#;

fn seconds(world: &mut World, seconds: f32) {
    let rate = f32::from(world.resource::<GameTime>().tick_rate);
    for _ in 0..(rate * seconds).round() as usize {
        world.run_tick();
    }
}

#[test]
fn device_draws_from_its_cell_while_on() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let lamp = spawn_at(&mut world, "Lamp", grid, 1, 1);
    let cell = battery_in_slot(&world, lamp).expect("батарейка в слоте");
    assert_eq!(device_of_cell(&world, cell), Some(lamp));
    // Выключенная лампа не тратит.
    seconds(&mut world, 1.0);
    assert_eq!(battery::charge(&world, cell), 300.0);
    assert!(toggle(&mut world, lamp, None, false));
    assert!(is_activated(&world, lamp));
    seconds(&mut world, 1.0);
    assert!((battery::charge(&world, cell) - 200.0).abs() < 5.0);
    // Действие — 50 Дж.
    assert!(try_use_activatable_charge(&mut world, lamp, None));
    // Разрядилась — лампа выключается сама.
    seconds(&mut world, 2.0);
    assert_eq!(battery::charge(&world, cell), 0.0);
    assert!(!is_activated(&world, lamp));
    // Без заряда не включить.
    assert!(!toggle(&mut world, lamp, None, false));
    assert!(!has_activatable_charge(&mut world, lamp, None));
}

#[test]
fn only_cells_fit_and_removing_turns_off() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let lamp = spawn_at(&mut world, "Lamp", grid, 1, 1);
    let cell = battery_in_slot(&world, lamp).unwrap();
    toggle(&mut world, lamp, None, false);
    assert!(is_activated(&world, lamp));
    world.remove_from_container(cell, None).unwrap();
    assert!(battery_in_slot(&world, lamp).is_none());
    assert!(!is_activated(&world, lamp));
    let rock = spawn_at(&mut world, "Rock", grid, 2, 2);
    assert!(
        world
            .insert_into_container(rock, lamp, "cell_slot")
            .is_err()
    );
    assert!(world.insert_into_container(cell, lamp, "cell_slot").is_ok());
    assert_eq!(battery_in_slot(&world, lamp), Some(cell));
}
