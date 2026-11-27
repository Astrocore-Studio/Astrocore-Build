use astrocore_ecs::GameWorldExt;
use astrocore_map::{TileCoord, TransformWorldExt};

use super::*;
use crate::atmos::{ensure_grid_atmosphere, tile_mixture, with_tile_mixture};
use crate::gas_tank::tank_air;
use crate::pipes::{pipe_air, with_pipe_air};
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: itemSize
  id: Normal
  weight: 4
- type: entity
  id: TestCanister
  components:
  - type: AtmosDevice
    requireAnchored: false
    joinSystem: true
  - type: NodeContainer
    nodes:
      port:
        !type:PortablePipeNode
        nodeGroupID: Pipe
        rotationsEnabled: false
        volume: 1
  - type: GasPortable
  - type: Appearance
  - type: Lock
    locked: false
  - type: UIRequiresLock
    userInterfaceKeys:
    - enum.GasCanisterUiKey.Key
  - type: ActivatableUI
    key: enum.GasCanisterUiKey.Key
  - type: UserInterface
    interfaces:
      enum.GasCanisterUiKey.Key:
        type: GasCanisterBoundUserInterface
  - type: Anchorable
  - type: GasCanister
    maxIntegrity: 15
    integrity: 15
    maxReleasePressure: 1013.25
    safetyPressure: 10132.5
    overpressure: 20265
    gasTankSlot:
      whitelist:
        components:
        - GasTank
    air:
      volume: 1000
      temperature: 293.15
      moles:
        Nitrogen: 1871.71051
- type: entity
  id: TestPort
  components:
  - type: Transform
    anchored: true
  - type: NodeContainer
    nodes:
      connected:
        !type:PortPipeNode
        nodeGroupID: Pipe
        pipeDirection: South
  - type: GasPort
- type: entity
  id: TestPipe
  components:
  - type: Transform
    anchored: true
  - type: NodeContainer
    nodes:
      pipe: !type:PipeNode
        nodeGroupID: Pipe
        pipeDirection: Longitudinal
        volume: 200
- type: entity
  id: TestTank
  components:
  - type: Item
    size: Normal
  - type: GasTank
    air:
      volume: 5
      temperature: 293.15
- type: entity
  id: TestHeater
  components:
  - type: Transform
    anchored: true
  - type: AtmosDevice
  - type: ApcPowerReceiver
    needsPower: false
  - type: NodeContainer
    nodes:
      pipe: !type:PipeNode
        nodeGroupID: Pipe
        pipeDirection: South
  - type: GasThermoMachine
    targetTemperature: 373.15
    coefficientOfPerformance: 0.9
    heatCapacity: 5000
"#;

fn ticks(world: &mut World, count: usize) {
    for _ in 0..count {
        world.run_tick();
    }
}

fn setup() -> (World, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    ensure_grid_atmosphere(&mut world, grid);
    ticks(&mut world, 10);
    (world, grid)
}

fn pressure(world: &World, canister: Entity) -> f32 {
    canister_mixture(world, canister).unwrap().pressure()
}

#[test]
fn canister_holds_ss14_pressure_and_releases_to_tile() {
    let (mut world, grid) = setup();
    let canister = spawn_at(&mut world, "TestCanister", grid, 8, 8);
    // 1871,7 моля азота в 1000 л при 20 °C — 45 атм.
    assert!((pressure(&world, canister) - 4562.1).abs() < 1.0);
    set_canister_valve(&mut world, canister, true, None);
    ticks(&mut world, 30);
    assert!(pressure(&world, canister) < 4561.0);
    let tile = tile_mixture(&world, grid, TileCoord::new(8, 8, 0)).unwrap();
    assert!(tile.moles[gas::NITROGEN] > 0.0);
}

#[test]
fn canister_fills_inserted_tank() {
    let (mut world, grid) = setup();
    let canister = spawn_at(&mut world, "TestCanister", grid, 8, 8);
    let tank = spawn_at(&mut world, "TestTank", grid, 8, 8);
    assert!(crate::item_slots::insert(
        &mut world,
        canister,
        "tank_slot",
        tank
    ));
    assert_eq!(inserted_tank(&world, canister), Some(tank));
    assert_eq!(
        world.appearance(canister, TANK_INSERTED),
        Some(&astrocore_net::AppearanceValue::Bool(true))
    );
    set_canister_valve(&mut world, canister, true, None);
    ticks(&mut world, 60);
    // Выпуск до давления выпуска — 1 атм.
    let tank_pressure = tank_air(&world, tank).unwrap().pressure();
    assert!(
        (tank_pressure - gas::ONE_ATMOSPHERE).abs() < 5.0,
        "{tank_pressure}"
    );
    let tile = tile_mixture(&world, grid, TileCoord::new(8, 8, 0)).unwrap();
    assert_eq!(tile.moles[gas::NITROGEN], 0.0);
}

#[test]
fn canister_anchors_only_on_port_and_shares_air() {
    let (mut world, grid) = setup();
    let canister = spawn_at(&mut world, "TestCanister", grid, 5, 5);
    // Без порта не крепится.
    let mut attempt = crate::anchorable::AnchorAttempt {
        user: canister,
        tool: canister,
        anchoring: true,
        cancelled: false,
        fail_message: None,
        delay: 1.0,
    };
    world.raise(canister, &mut attempt);
    assert!(attempt.cancelled);

    let port = spawn_at(&mut world, "TestPort", grid, 8, 8);
    let pipe = spawn_at(&mut world, "TestPipe", grid, 8, 7);
    let canister = spawn_at(&mut world, "TestCanister", grid, 8, 8);
    assert_eq!(gas_port_at(&world, canister), Some(port));
    world.anchor(canister);
    ticks(&mut world, 3);
    assert!(port_connected(&world, canister));
    ticks(&mut world, 30);
    // Сеть труб и канистра выровнялись по давлению.
    let net = pipe_air(&world, pipe, "pipe").unwrap().pressure();
    let own = pressure(&world, canister);
    assert!((net - own).abs() / own < 0.01, "{net} {own}");
}

#[test]
fn safety_valve_vents_overpressure() {
    let (mut world, grid) = setup();
    let canister = spawn_at(&mut world, "TestCanister", grid, 8, 8);
    // Выше предохранителя (10 МПа), ниже разрыва (20 МПа × 16).
    canister_air(&mut world, canister, |air| {
        air.moles[gas::NITROGEN] = 5000.0
    });
    ticks(&mut world, 15);
    assert!(
        world
            .get::<GasCanister>(canister)
            .unwrap()
            .safety_valve_open
    );
    let tile = tile_mixture(&world, grid, TileCoord::new(8, 8, 0)).unwrap();
    assert!(tile.moles[gas::NITROGEN] > 0.0);
}

#[test]
fn locked_canister_ui_does_not_open() {
    let (mut world, grid) = setup();
    let canister = spawn_at(&mut world, "TestCanister", grid, 8, 8);
    world.get_mut::<crate::lock::Lock>(canister).unwrap().locked = true;
    let mut attempt = crate::activatable_ui::ActivatableUiOpenAttempt {
        user: canister,
        cancelled: false,
    };
    world.raise(canister, &mut attempt);
    assert!(attempt.cancelled);
}

#[test]
fn heater_warms_pipe_with_hysteresis() {
    let (mut world, grid) = setup();
    let heater = spawn_at(&mut world, "TestHeater", grid, 8, 8);
    let pipe = spawn_at(&mut world, "TestPipe", grid, 8, 7);
    ticks(&mut world, 3);
    with_pipe_air(&mut world, pipe, "pipe", |air| {
        air.moles[gas::NITROGEN] = 10.0;
        air.set_temperature(gas::T20C);
    });
    ticks(&mut world, 30 * 20);
    // Цель 100 °C, допуск 2 К: греется до 375 К и держится между 373 и 375 К.
    let temperature = pipe_air(&world, pipe, "pipe").unwrap().temperature();
    assert!((372.0..=375.2).contains(&temperature), "{temperature}");
    set_target_temperature(&mut world, heater, 10_000.0);
    assert_eq!(
        world
            .get::<GasThermoMachine>(heater)
            .unwrap()
            .target_temperature,
        593.15
    );
}

#[test]
fn tile_air_unchanged_by_closed_canister() {
    let (mut world, grid) = setup();
    with_tile_mixture(&mut world, grid, TileCoord::new(8, 8, 0), |air| air.clear());
    let canister = spawn_at(&mut world, "TestCanister", grid, 8, 8);
    ticks(&mut world, 30);
    assert!((pressure(&world, canister) - 4562.1).abs() < 1.0);
}
