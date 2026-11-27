use astrocore_map::TileCoord;

use super::*;
use crate::atmos::{ensure_grid_atmosphere, tile_mixture};
use crate::internals::tests::{PROTOTYPES, ticks};
use crate::testing::{spawn_at, station, world_with};

const EXTRA: &str = r#"
- type: entity
  id: OverfilledTank
  parent: OxygenTank
  components:
  - type: Explosive
    explosionType: Default
  - type: GasTank
    air:
      volume: 5
      moles:
        Oxygen: 30
"#;

fn setup() -> (World, Entity) {
    let mut world = world_with(&format!("{PROTOTYPES}{EXTRA}"));
    let grid = station(&mut world);
    ensure_grid_atmosphere(&mut world, grid);
    ticks(&mut world, 10);
    (world, grid)
}

fn pressure(world: &World, tank: Entity) -> f32 {
    tank_air(world, tank).unwrap().pressure()
}

#[test]
fn filled_tank_pressure_matches_ss14() {
    let (mut world, grid) = setup();
    let tank = spawn_at(&mut world, "OxygenTank", grid, 3, 3);
    // 2,05 моля при 20 °C в 5 л ≈ 1000 кПа.
    assert!((pressure(&world, tank) - 1000.0).abs() < 1.0);
    assert!(!world.get::<GasTank>(tank).unwrap().is_low_pressure());
}

#[test]
fn open_valve_releases_gas() {
    let (mut world, grid) = setup();
    let tank = spawn_at(&mut world, "OxygenTank", grid, 3, 3);
    let before = pressure(&world, tank);
    set_valve(&mut world, tank, true, None);
    ticks(&mut world, 60);
    assert!(pressure(&world, tank) < before);
    let tile = tile_mixture(&world, grid, TileCoord::new(3, 3, 0)).unwrap();
    assert!(tile.moles[gas::OXYGEN] > 0.0);
}

#[test]
fn release_pressure_limits_breath() {
    let (mut world, grid) = setup();
    let tank = spawn_at(&mut world, "OxygenTank", grid, 3, 3);
    // 0,5 л при 21,3 кПа и 20 °C: 0,00437 моля.
    let breath = remove_air_output(&mut world, tank, 0.5).unwrap();
    assert!((breath.total_moles() - 0.004_37).abs() < 0.000_05);
    assert_eq!(breath.volume, 0.5);
}

#[test]
fn overpressure_opens_valve_then_ruptures() {
    let (mut world, grid) = setup();
    // 30 молей — ≈ 14,6 МПа: выше предохранителя (1,5 МПа) и предела разрыва
    // 20 атм × (прочность 3 + 1) ≈ 8,1 МПа.
    let tank = spawn_at(&mut world, "OverfilledTank", grid, 3, 3);
    ticks(&mut world, 30);
    assert!(crate::destructible::is_destroyed(&world, tank));
    // Газ баллона (30 молей) вышел наружу (`MergeContainingMixture`) и расходится.
    let around: f32 = (2..5)
        .flat_map(|x| (2..5).map(move |y| TileCoord::new(x, y, 0)))
        .filter_map(|c| tile_mixture(&world, grid, c).map(|m| m.moles[gas::OXYGEN]))
        .sum();
    assert!(around > 10.0, "{around}");
}

#[test]
fn safety_valve_opens_above_safety_pressure() {
    let (mut world, grid) = setup();
    let tank = spawn_at(&mut world, "OxygenTank", grid, 3, 3);
    // 2000 кПа: выше предохранителя, ниже предела прочности.
    with_tank_air(&mut world, tank, |air| air.moles[gas::OXYGEN] = 4.1);
    ticks(&mut world, 15);
    assert!(world.get::<GasTank>(tank).unwrap().release_valve_open);
}
