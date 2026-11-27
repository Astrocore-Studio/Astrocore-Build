use astrocore_ecs::GameWorldExt;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Cell
  components:
  - type: Battery
    maxCharge: 100
    startingCharge: 100
- type: entity
  id: SelfCharging
  components:
  - type: Battery
    maxCharge: 100
    startingCharge: 50
  - type: BatterySelfRecharger
    autoRechargeRate: 10
    autoRechargePauseTime: 2
"#;

fn seconds(world: &mut World, seconds: f32) {
    let rate = f32::from(world.resource::<GameTime>().tick_rate);
    for _ in 0..(rate * seconds).round() as usize {
        world.run_tick();
    }
}

#[derive(bevy_ecs::component::Component, Default)]
struct States(Vec<BatteryState>);

#[test]
fn charge_is_spent_and_states_are_announced() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let cell = spawn_at(&mut world, "Cell", grid, 1, 1);
    world.entity_mut(cell).insert(States::default());
    world.subscribe::<States, BatteryStateChanged>(|world, entity, event| {
        world
            .get_mut::<States>(entity)
            .unwrap()
            .0
            .push(event.new_state);
    });
    assert_eq!(charge(&world, cell), 100.0);
    assert!(try_use_charge(&mut world, cell, 30.0));
    assert_eq!(charge(&world, cell), 70.0);
    assert!(!try_use_charge(&mut world, cell, 80.0));
    assert_eq!(use_charge(&mut world, cell, 80.0), -70.0);
    assert_eq!(charge_level(&world, cell), 0.0);
    world.rejuvenate_battery(cell);
    assert_eq!(charge(&world, cell), 100.0);
    assert_eq!(
        world.get::<States>(cell).unwrap().0,
        vec![
            BatteryState::Neither,
            BatteryState::Empty,
            BatteryState::Full
        ]
    );
}

trait RejuvenateBattery {
    fn rejuvenate_battery(&mut self, entity: Entity);
}

impl RejuvenateBattery for World {
    fn rejuvenate_battery(&mut self, entity: Entity) {
        self.raise(entity, &mut Rejuvenate);
    }
}

#[test]
fn self_recharge_pauses_after_use() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let cell = spawn_at(&mut world, "SelfCharging", grid, 1, 1);
    // 10 Дж/с от 50.
    seconds(&mut world, 1.0);
    assert!((charge(&world, cell) - 60.0).abs() < 0.5);
    // Трата — пауза 2 с без заряда.
    use_charge(&mut world, cell, 20.0);
    let after_use = charge(&world, cell);
    seconds(&mut world, 1.5);
    assert!((charge(&world, cell) - after_use).abs() < 0.01);
    seconds(&mut world, 1.5);
    assert!(charge(&world, cell) > after_use + 5.0);
    // Не больше ёмкости.
    seconds(&mut world, 10.0);
    assert_eq!(charge(&world, cell), 100.0);
}
