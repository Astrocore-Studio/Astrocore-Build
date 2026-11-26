use super::*;
use crate::atmos::{ensure_grid_atmosphere, tile_mixture};
use crate::pipes::tests::{PIPES, run};
use crate::pipes::{net_of, pipe_air};
use crate::testing::{spawn_at, station, world_with};

const DEVICES: &str = r#"
- type: entity
  id: TestWall
  components:
  - type: Transform
    anchored: true
  - type: Airtight
- type: entity
  id: TestVent
  components:
  - type: Transform
    anchored: true
  - type: AtmosDevice
  - type: ApcPowerReceiver
    needsPower: false
  - type: GasVentPump
  - type: NodeContainer
    nodes:
      pipe: !type:PipeNode
        nodeGroupID: Pipe
        pipeDirection: West
- type: entity
  id: TestScrubber
  components:
  - type: Transform
    anchored: true
  - type: AtmosDevice
  - type: ApcPowerReceiver
    needsPower: false
  - type: GasVentScrubber
  - type: NodeContainer
    nodes:
      pipe: !type:PipeNode
        nodeGroupID: Pipe
        pipeDirection: East
- type: entity
  id: TestPump
  components:
  - type: Transform
    anchored: true
  - type: AtmosDevice
  - type: GasPressurePump
    targetPressure: 500
  - type: NodeContainer
    nodes:
      inlet: !type:PipeNode
        nodeGroupID: Pipe
        pipeDirection: West
      outlet: !type:PipeNode
        nodeGroupID: Pipe
        pipeDirection: East
- type: entity
  id: TestValve
  parent: TestPump
  components:
  - type: GasValve
    open: false
"#;

struct Scene {
    world: World,
    grid: Entity,
}

/// Комната 1..=8 в стенах; воздух пустой (вакуум).
fn scene() -> Scene {
    let mut world = world_with(&format!("{PIPES}{DEVICES}"));
    let grid = station(&mut world);
    for i in 0..10 {
        for (x, y) in [(i, 0), (i, 9), (0, i), (9, i)] {
            if (x, y) != (0, 0) || i == 0 {
                spawn_at(&mut world, "TestWall", grid, x, y);
            }
        }
    }
    ensure_grid_atmosphere(&mut world, grid);
    let mut scene = Scene { world, grid };
    run(&mut scene.world, 0.3);
    scene
}

fn pressure(scene: &Scene, x: i32, y: i32) -> f32 {
    tile_mixture(&scene.world, scene.grid, TileCoord::new(x, y, 0))
        .map_or(0.0, GasMixture::pressure)
}

#[test]
fn vent_refills_a_room_once_the_lockout_is_released() {
    let mut scene = scene();
    let pipe = spawn_at(&mut scene.world, "PipeLateral", scene.grid, 3, 4);
    let vent = spawn_at(&mut scene.world, "TestVent", scene.grid, 4, 4);
    run(&mut scene.world, 0.1);
    crate::pipes::with_pipe_air(&mut scene.world, pipe, "pipe", |air| {
        air.moles[OXYGEN] = 10000.0;
        air.set_temperature(T20C);
    });
    // Вакуум ниже 80 кПа: вентиляция почти закрыта.
    run(&mut scene.world, 3.0);
    assert!(pressure(&scene, 4, 4) < 5.0, "{}", pressure(&scene, 4, 4));
    assert!(
        scene
            .world
            .get::<GasVentPump>(vent)
            .unwrap()
            .under_pressure_lockout
    );
    // Снять блокировку отвёрткой (на 30 секунд).
    // Комната 8×8 наполняется дольше 30 секунд: блокировку снимают трижды.
    for _ in 0..3 {
        let now = scene.world.resource::<GameTime>().cur_time().as_secs_f64();
        scene
            .world
            .get_mut::<GasVentPump>(vent)
            .unwrap()
            .manual_lockout_until = Some(now + 30.0);
        run(&mut scene.world, 30.0);
    }
    // Выше 80 кПа блокировки нет: вентиляция дополняет до 101,3 сама.
    run(&mut scene.world, 20.0);
    let p = pressure(&scene, 2, 7);
    assert!(p > 90.0 && p < 102.0, "{p}");
}

#[test]
fn scrubber_takes_carbon_dioxide_into_the_pipe() {
    let mut scene = scene();
    let scrubber = spawn_at(&mut scene.world, "TestScrubber", scene.grid, 4, 4);
    spawn_at(&mut scene.world, "PipeLateral", scene.grid, 5, 4);
    run(&mut scene.world, 0.1);
    crate::atmos::with_tile_mixture(
        &mut scene.world,
        scene.grid,
        TileCoord::new(4, 4, 0),
        |air| {
            air.copy_from(&GasMixture::standard_air(CELL_VOLUME));
            air.moles[CARBON_DIOXIDE] = 10.0;
        },
    );
    run(&mut scene.world, 3.0);
    let piped = pipe_air(&scene.world, scrubber, "pipe").unwrap().moles[CARBON_DIOXIDE];
    assert!(piped > 1.0, "{piped}");
    assert_eq!(
        pipe_air(&scene.world, scrubber, "pipe").unwrap().moles[OXYGEN],
        0.0
    );
}

#[test]
fn pump_fills_the_outlet_to_target_and_valve_joins_nets() {
    let mut scene = scene();
    let left = spawn_at(&mut scene.world, "PipeLateral", scene.grid, 3, 4);
    let pump = spawn_at(&mut scene.world, "TestPump", scene.grid, 4, 4);
    let right = spawn_at(&mut scene.world, "PipeLateral", scene.grid, 5, 4);
    let valve = spawn_at(&mut scene.world, "TestValve", scene.grid, 6, 4);
    let far = spawn_at(&mut scene.world, "PipeLateral", scene.grid, 7, 4);
    run(&mut scene.world, 0.1);
    crate::pipes::with_pipe_air(&mut scene.world, left, "pipe", |air| {
        air.moles[NITROGEN] = 500.0
    });
    run(&mut scene.world, 3.0);
    let out = pipe_air(&scene.world, right, "pipe").unwrap().pressure();
    assert!((out - 500.0).abs() < 5.0, "{out}");
    // Закрытый клапан делит сети, открытый соединяет.
    assert_ne!(
        net_of(&scene.world, right, "pipe"),
        net_of(&scene.world, far, "pipe")
    );
    set_valve(&mut scene.world, valve, true);
    run(&mut scene.world, 0.1);
    assert_eq!(
        net_of(&scene.world, right, "pipe"),
        net_of(&scene.world, far, "pipe")
    );
    let _ = pump;
}
