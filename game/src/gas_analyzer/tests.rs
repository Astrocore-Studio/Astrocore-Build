use astrocore_ecs::GameWorldExt;
use astrocore_map::TileCoord;

use super::*;
use crate::atmos::gas::CELL_VOLUME;
use crate::atmos::{ensure_grid_atmosphere, with_tile_mixture};
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Analyzer
  components:
  - type: GasAnalyzer
- type: entity
  id: Tank
  name: tank
  components:
  - type: GasTank
    air:
      volume: 5
      temperature: 293.15
      moles:
        Oxygen: 2.051379050
- type: entity
  id: Pipe
  components:
  - type: Transform
    anchored: true
  - type: NodeContainer
    nodes:
      pipe: !type:PipeNode
        nodeGroupID: Pipe
        pipeDirection: Lateral
        volume: 200
"#;

fn setup() -> (World, Entity, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    ensure_grid_atmosphere(&mut world, grid);
    for _ in 0..10 {
        world.run_tick();
    }
    with_tile_mixture(&mut world, grid, TileCoord::new(3, 3, 0), |air| {
        air.copy_from(&GasMixture::standard_air(CELL_VOLUME))
    });
    let analyzer = spawn_at(&mut world, "Analyzer", grid, 3, 3);
    (world, grid, analyzer)
}

fn aim(world: &mut World, analyzer: Entity, user: Entity, target: Option<Entity>) {
    let mut component = world.get_mut::<GasAnalyzer>(analyzer).unwrap();
    component.user = Some(user);
    component.target = target;
    component.enabled = true;
}

#[test]
fn environment_is_always_first() {
    let (mut world, _, analyzer) = setup();
    aim(&mut world, analyzer, analyzer, None);
    let state = analyze(&mut world, analyzer).unwrap();
    assert_eq!(state.mixes.len(), 1);
    let air = &state.mixes[0];
    assert!((air.pressure - 101.325).abs() < 0.5, "{}", air.pressure);
    // Азот, потом кислород.
    assert_eq!(air.gases[0].gas, "Nitrogen");
    assert_eq!(air.gases[1].gas, "Oxygen");
}

#[test]
fn tank_answers_with_its_air() {
    let (mut world, grid, analyzer) = setup();
    let tank = spawn_at(&mut world, "Tank", grid, 3, 3);
    aim(&mut world, analyzer, analyzer, Some(tank));
    let state = analyze(&mut world, analyzer).unwrap();
    assert_eq!(state.mixes.len(), 2);
    assert_eq!(state.device_name, "tank");
    assert!((state.mixes[1].pressure - 1000.0).abs() < 1.0);
}

#[test]
fn pipe_shows_its_share_and_far_target_is_dropped() {
    let (mut world, grid, analyzer) = setup();
    let pipes: Vec<Entity> = (3..5)
        .map(|x| spawn_at(&mut world, "Pipe", grid, x, 3))
        .collect();
    for _ in 0..3 {
        world.run_tick();
    }
    crate::pipes::with_pipe_air(&mut world, pipes[0], "pipe", |air| {
        air.moles[crate::atmos::gas::NITROGEN] = 10.0
    });
    aim(&mut world, analyzer, analyzer, Some(pipes[0]));
    let state = analyze(&mut world, analyzer).unwrap();
    // Труба 200 л из сети 400 л — половина газа.
    assert_eq!(state.mixes[1].volume, 200.0);
    assert!((state.mixes[1].gases[0].amount - 5.0).abs() < 0.01);
    // Далеко — цель отпускается, остаётся воздух вокруг.
    let far = spawn_at(&mut world, "Tank", grid, 12, 12);
    aim(&mut world, analyzer, analyzer, Some(far));
    let state = analyze(&mut world, analyzer).unwrap();
    assert_eq!(state.mixes.len(), 1);
    assert!(world.get::<GasAnalyzer>(analyzer).unwrap().target.is_none());
}
