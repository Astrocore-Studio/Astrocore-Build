use astrocore_map::TransformWorldExt;
use bevy_ecs::world::World;

use super::*;
use crate::testing::{spawn_at, station, world_with};

pub(crate) const CABLES: &str = r#"
- type: entity
  id: CableBase
  abstract: true
  components:
  - type: Transform
    anchored: true
- type: entity
  parent: CableBase
  id: CableHV
  components:
  - type: NodeContainer
    nodes:
      power:
        !type:CableNode
        nodeGroupID: HVPower
- type: entity
  parent: CableBase
  id: CableMV
  components:
  - type: NodeContainer
    nodes:
      power:
        !type:CableNode
        nodeGroupID: MVPower
- type: entity
  parent: CableBase
  id: CableTerminal
  components:
  - type: NodeContainer
    nodes:
      power:
        !type:CableTerminalNode
        nodeGroupID: HVPower
- type: entity
  parent: CableBase
  id: Smes
  components:
  - type: NodeContainer
    nodes:
      input:
        !type:CableTerminalPortNode
        nodeGroupID: HVPower
      output:
        !type:CableDeviceNode
        nodeGroupID: HVPower
"#;

fn groups(world: &mut World) -> Vec<NodeGroup> {
    rebuild(world);
    world.resource::<NodeGroups>().groups.clone()
}

#[test]
fn cables_of_one_kind_join_along_sides() {
    let mut world = world_with(CABLES);
    let grid = station(&mut world);
    let a = spawn_at(&mut world, "CableHV", grid, 1, 1);
    let b = spawn_at(&mut world, "CableHV", grid, 2, 1);
    // По диагонали кабели не соединяются; кабель другого напряжения — в своей группе.
    let c = spawn_at(&mut world, "CableHV", grid, 3, 2);
    let mv = spawn_at(&mut world, "CableMV", grid, 2, 1);
    let groups = groups(&mut world);
    assert_eq!(groups.len(), 3);
    let state = world.resource::<NodeGroups>();
    assert_eq!(state.group_of(a, "power"), state.group_of(b, "power"));
    assert_ne!(state.group_of(b, "power"), state.group_of(c, "power"));
    let mv_group = state.group_of(mv, "power").unwrap();
    assert_eq!(groups[mv_group].kind, "MVPower");
}

#[test]
fn terminal_links_smes_input_and_blocks_the_cable_across() {
    let mut world = world_with(CABLES);
    let grid = station(&mut world);
    // Сеть входа: кабели (1..3, 1) и клемма в (3, 1), смотрит на восток на СМЭС в (4, 1).
    let input_left = spawn_at(&mut world, "CableHV", grid, 2, 1);
    spawn_at(&mut world, "CableHV", grid, 3, 1);
    let terminal = spawn_at(&mut world, "CableTerminal", grid, 3, 1);
    world
        .set_local_rotation(terminal, astrocore_math::Angle(std::f32::consts::FRAC_PI_2))
        .unwrap();
    let smes = spawn_at(&mut world, "Smes", grid, 4, 1);
    // Выход: кабель под СМЭС и дальше на восток.
    spawn_at(&mut world, "CableHV", grid, 4, 1);
    let output_right = spawn_at(&mut world, "CableHV", grid, 5, 1);
    rebuild(&mut world);
    let state = world.resource::<NodeGroups>();
    let input = state.group_of(smes, "input").unwrap();
    let output = state.group_of(smes, "output").unwrap();
    assert_ne!(input, output, "клемма разделяет вход и выход");
    assert_eq!(state.group_of(input_left, "power"), Some(input));
    assert_eq!(state.group_of(output_right, "power"), Some(output));
}

#[test]
fn deleting_a_cable_splits_the_group_next_tick() {
    use astrocore_ecs::{GameWorldExt, LifecycleWorldExt};

    let mut world = world_with(CABLES);
    let grid = station(&mut world);
    let a = spawn_at(&mut world, "CableHV", grid, 1, 1);
    let middle = spawn_at(&mut world, "CableHV", grid, 2, 1);
    let b = spawn_at(&mut world, "CableHV", grid, 3, 1);
    world.run_tick();
    {
        let state = world.resource::<NodeGroups>();
        assert_eq!(state.group_of(a, "power"), state.group_of(b, "power"));
    }
    world.delete_entity(middle);
    world.run_tick();
    let state = world.resource::<NodeGroups>();
    assert_ne!(state.group_of(a, "power"), state.group_of(b, "power"));
}
