use astrocore_ecs::GameWorldExt;
use astrocore_map::TileCoord;

use super::*;
use crate::atmos::{ensure_grid_atmosphere, with_tile_mixture};
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: damageType
  id: Blunt
  name: damage-type-blunt
- type: entity
  id: Spaceman
  components:
  - type: Damageable
  - type: Injurable
  - type: Alerts
  - type: Barotrauma
    damage:
      types:
        Blunt: 0.5
- type: entity
  id: Suited
  parent: Spaceman
  components:
  - type: PressureProtection
    lowPressureMultiplier: 1000
- type: alert
  id: LowPressure
  category: Pressure
  icons:
  - { sprite: /Textures/Interface/Alerts/pressure.rsi, state: lowpressure1 }
  - { sprite: /Textures/Interface/Alerts/pressure.rsi, state: lowpressure2 }
  maxSeverity: 2
- type: alert
  id: HighPressure
  category: Pressure
  icons:
  - { sprite: /Textures/Interface/Alerts/pressure.rsi, state: highpressure1 }
  - { sprite: /Textures/Interface/Alerts/pressure.rsi, state: highpressure2 }
  maxSeverity: 2
"#;

fn blunt(world: &World, entity: Entity) -> f32 {
    world.total_damage(entity).float()
}

#[test]
fn vacuum_hurts_and_air_does_not() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    ensure_grid_atmosphere(&mut world, grid);
    for _ in 0..10 {
        world.run_tick();
    }
    let inside = spawn_at(&mut world, "Spaceman", grid, 3, 3);
    let outside = spawn_at(&mut world, "Spaceman", grid, 5, 5);
    let suited = spawn_at(&mut world, "Suited", grid, 5, 5);
    // Клетка 3,3 с воздухом, остальное — вакуум без газа.
    with_tile_mixture(&mut world, grid, TileCoord::new(3, 3, 0), |air| {
        air.copy_from(&GasMixture::standard_air(CELL_VOLUME));
    });
    // Секунда: проверка раз в секунду, воздух не успевает разойтись далеко.
    for _ in 0..31 {
        world.run_tick();
    }
    assert_eq!(blunt(&world, inside), 0.0);
    // Вакуум: 0,5 × 4 = 2 за секунду.
    assert_eq!(blunt(&world, outside), 2.0);
    assert_eq!(blunt(&world, suited), 0.0);
    // Значки: в вакууме — низкое давление уровня 2; воздух клетки уже расходится в вакуум —
    // давление падает, и там лишь предупреждение без урона.
    let pressure = |entity| {
        crate::alerts::active_alerts(&world, entity)
            .and_then(|a| a.get("category:Pressure").cloned())
            .map(|s| (s.alert_type, s.severity))
    };
    assert_eq!(pressure(outside), Some(("LowPressure".into(), Some(2))));
    assert_ne!(pressure(inside).and_then(|(_, level)| level), Some(2));
}

#[test]
fn felt_pressure_respects_protection() {
    let mut barotrauma: Barotrauma = astrocore_data::from_node(
        &astrocore_data::parse_yaml("damage:\n  types:\n    Blunt: 1\n").unwrap(),
    )
    .unwrap();
    assert_eq!(felt_low_pressure(&barotrauma, 10.0), 10.0);
    barotrauma.low_pressure_multiplier = 1000.0;
    assert_eq!(felt_low_pressure(&barotrauma, 10.0), ONE_ATMOSPHERE);
    barotrauma.has_immunity = true;
    assert_eq!(felt_high_pressure(&barotrauma, 9000.0), ONE_ATMOSPHERE);
}
