use astrocore_ecs::GameWorldExt;
use astrocore_map::TileCoord;

use super::*;
use crate::atmos::gas::CELL_VOLUME;
use crate::atmos::{GasMixture, ensure_grid_atmosphere, tile_mixture, with_tile_mixture};
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: damageType
  id: Heat
  name: damage-type-heat
- type: damageType
  id: Cold
  name: damage-type-cold
- type: entity
  id: Body
  components:
  - type: Damageable
  - type: Injurable
  - type: Temperature
  - type: AtmosExposed
  - type: TemperatureDamage
    heatDamageThreshold: 316.15
    coldDamageThreshold: 273.15
    coldDamage:
      types:
        Cold: 0.1
    heatDamage:
      types:
        Heat: 1.5
  - type: TemperatureSpeed
    thresholds:
      293: 0.9
      280: 0.8
      260: 0.7
  - type: MovementSpeedModifier
- type: entity
  id: Living
  parent: Body
  components:
  - type: ThermalRegulator
    metabolismHeat: 800
    radiatedHeat: 100
    implicitHeatRegulation: 500
    sweatHeatRegulation: 2000
    shiveringHeatRegulation: 2000
    normalBodyTemperature: 310.15
    thermalRegulationTemperatureThreshold: 2
- type: entity
  id: Hat
  components:
  - type: TemperatureProtection
    coefficient: 0.5
"#;

fn ticks(world: &mut World, count: usize) {
    for _ in 0..count {
        world.run_tick();
    }
}

fn hot_tile(world: &mut World, grid: Entity, temperature: f32) {
    with_tile_mixture(world, grid, TileCoord::new(3, 3, 0), |air| {
        air.copy_from(&GasMixture::standard_air(CELL_VOLUME).with_temperature(temperature));
    });
}

#[test]
fn body_and_air_exchange_heat() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    ensure_grid_atmosphere(&mut world, grid);
    ticks(&mut world, 5);
    hot_tile(&mut world, grid, 500.0);
    let body = spawn_at(&mut world, "Body", grid, 3, 3);
    assert_eq!(temperature_of(&world, body), Some(T20C));
    // Обмен раз в секунду: 150 Вт/К × площадь 2 × (500 − 293,15) К = 62 070 Дж при
    // теплоёмкости 3000 Дж/К (масса 1 кг) — плюс 20,69 К. Горячий воздух за секунду
    // немного расходится по соседним клеткам, но почти не остывает.
    ticks(&mut world, 31);
    let after = temperature_of(&world, body).unwrap();
    assert!((after - (T20C + 20.69)).abs() < 1.5, "{after}");
    let air = tile_mixture(&world, grid, TileCoord::new(3, 3, 0))
        .unwrap()
        .temperature();
    assert!(air < 500.0, "воздух остыл: {air}");
}

#[test]
fn heat_damage_follows_logistic_curve() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let body = spawn_at(&mut world, "Body", grid, 3, 3);
    world.entity_mut(body).remove::<AtmosExposed>();
    world.run_tick();
    set_temperature(&mut world, body, 416.15);
    // Первый пересчёт — с прошедшим временем 0; дальше раз в секунду.
    ticks(&mut world, 1);
    let mut last = world.total_damage(body).float();
    for _ in 0..3 {
        set_temperature(&mut world, body, 416.15);
        ticks(&mut world, 30);
    }
    last = world.total_damage(body).float() - last;
    // Пересчёт — при смене температуры и не чаще раза в секунду: первая смена в цикле
    // приходит через такт после предыдущего пересчёта и пропускается, дальше — два раза
    // примерно по секунде. 16 / (1 + e^−0,5) − 8 = 1,959 в секунду × 1,5 ≈ 2,94.
    assert!(last > 5.7 && last < 6.3, "{last}");
}

#[test]
fn cold_slows_after_delay() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let body = spawn_at(&mut world, "Body", grid, 3, 3);
    world.entity_mut(body).remove::<AtmosExposed>();
    world.run_tick();
    set_temperature(&mut world, body, 285.0);
    let speed = |world: &World| {
        world
            .get::<astrocore_movement::MovementSpeedModifier>(body)
            .unwrap()
            .walk_speed_modifier
    };
    assert_eq!(speed(&world), 1.0);
    ticks(&mut world, 31);
    assert_eq!(speed(&world), 0.9);
    set_temperature(&mut world, body, 300.0);
    ticks(&mut world, 31);
    assert_eq!(speed(&world), 1.0);
}

#[test]
fn regulator_warms_cold_body_and_protection_halves_exchange() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let living = spawn_at(&mut world, "Living", grid, 3, 3);
    world.entity_mut(living).remove::<AtmosExposed>();
    world.run_tick();
    assert_eq!(temperature_of(&world, living), Some(310.15));
    set_temperature(&mut world, living, 300.0);
    ticks(&mut world, 31);
    // Обмен веществ +700 Дж, неявная регуляция +500, дрожь до 2000 — тело теплеет.
    let warmed = temperature_of(&world, living).unwrap();
    assert!(warmed > 300.0, "{warmed}");

    let body = spawn_at(&mut world, "Body", grid, 4, 4);
    let plain = conduct_heat(&mut world, body, 400.0, 1.0e9, 1.0, 1.0, false)
        .unwrap()
        .0;
    set_temperature(&mut world, body, T20C);
    let hat = spawn_at(&mut world, "Hat", grid, 4, 4);
    world.subscribe::<Temperature, BeforeHeatExchange>(move |world, _, event| {
        if let Some(p) = world.get::<TemperatureProtection>(hat) {
            event.heat_transfer_modifier *= p.coefficient;
        }
    });
    let protected = conduct_heat(&mut world, body, 400.0, 1.0e9, 1.0, 1.0, false)
        .unwrap()
        .0;
    assert!((protected - plain / 2.0).abs() < 1.0, "{plain} {protected}");
}
