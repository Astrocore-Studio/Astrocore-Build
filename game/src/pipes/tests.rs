use astrocore_ecs::{GameWorldExt, LifecycleWorldExt};

use super::*;
use crate::testing::{spawn_at, station, world_with};

pub(crate) const PIPES: &str = r#"
- type: entity
  id: PipeLateral
  components:
  - type: Transform
    anchored: true
  - type: NodeContainer
    nodes:
      pipe: !type:PipeNode
        nodeGroupID: Pipe
        pipeDirection: Lateral
        volume: 200
- type: entity
  id: PipeLateralLayer
  parent: PipeLateral
  components:
  - type: AtmosPipeLayers
    pipeLayer: Secondary
"#;

pub(crate) fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

#[test]
fn connected_pipes_share_air_and_split_by_volume() {
    let mut world = world_with(PIPES);
    let grid = station(&mut world);
    let pipes: Vec<Entity> = (2..6)
        .map(|x| spawn_at(&mut world, "PipeLateral", grid, x, 5))
        .collect();
    run(&mut world, 0.1);
    assert_eq!(
        net_of(&world, pipes[0], "pipe"),
        net_of(&world, pipes[3], "pipe")
    );
    assert_eq!(pipe_air(&world, pipes[0], "pipe").unwrap().volume, 800.0);
    with_pipe_air(&mut world, pipes[0], "pipe", |air| {
        air.moles[gas::NITROGEN] = 100.0
    });
    // Убрать трубу посередине: две сети по 200 и 400 литров, газ делится по объёму, доля
    // убранной трубы теряется.
    world.delete_entity(pipes[1]);
    run(&mut world, 0.1);
    let left = pipe_air(&world, pipes[0], "pipe").unwrap();
    let right = pipe_air(&world, pipes[3], "pipe").unwrap();
    assert_eq!(left.volume, 200.0);
    assert_eq!(right.volume, 400.0);
    assert!((left.moles[gas::NITROGEN] - 25.0).abs() < 0.01);
    assert!((right.moles[gas::NITROGEN] - 50.0).abs() < 0.01);
    // Снова соединить: газ собирается.
    spawn_at(&mut world, "PipeLateral", grid, 3, 5);
    run(&mut world, 0.1);
    let all = pipe_air(&world, pipes[0], "pipe").unwrap();
    assert!((all.moles[gas::NITROGEN] - 75.0).abs() < 0.01);
}

#[test]
fn different_layers_do_not_connect() {
    let mut world = world_with(PIPES);
    let grid = station(&mut world);
    let a = spawn_at(&mut world, "PipeLateral", grid, 2, 5);
    let b = spawn_at(&mut world, "PipeLateralLayer", grid, 3, 5);
    run(&mut world, 0.1);
    assert_ne!(net_of(&world, a, "pipe"), net_of(&world, b, "pipe"));
}
