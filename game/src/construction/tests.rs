use astrocore_ecs::{GameWorldExt, MetaData};
use astrocore_map::{TileCoord, Transform, TransformWorldExt};

use super::*;
use crate::hands::{Hand, HandsWorldExt};
use crate::interaction::InteractionWorldExt;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: stack
  id: Steel
  name: stack-steel
  spawn: SheetSteel1
  maxCount: 30
- type: entity
  id: SheetSteel
  components:
  - type: Item
  - type: Stack
    stackType: Steel
    count: 30
- type: entity
  id: SheetSteel1
  parent: SheetSteel
  components:
  - type: Stack
    stackType: Steel
    count: 1
- type: entity
  id: Welder
  components:
  - type: Item
  - type: Tool
    qualities: [ Welding ]
- type: constructionGraph
  id: TestGirder
  start: start
  graph:
  - node: start
    edges:
    - to: girder
      steps:
      - material: Steel
        amount: 2
  - node: girder
    entity: TestGirderEntity
    edges:
    - to: wall
      conditions:
      - !type:EntityAnchored {}
      steps:
      - material: Steel
        amount: 2
        doAfter: 2
  - node: wall
    entity: TestWallEntity
    edges:
    - to: girder
      completed:
      - !type:GivePrototype
        prototype: SheetSteel1
        amount: 2
      steps:
      - tool: Welding
        doAfter: 3
- type: entity
  id: TestGirderEntity
  components:
  - type: Transform
    anchored: true
  - type: Construction
    graph: TestGirder
    node: girder
- type: entity
  id: TestWallEntity
  components:
  - type: Transform
    anchored: true
  - type: Airtight
  - type: Construction
    graph: TestGirder
    node: wall
- type: construction
  id: TestGirderRecipe
  graph: TestGirder
  startNode: start
  targetNode: wall
  category: construction-category-structures
  conditions:
  - !type:TileNotBlocked {}
- type: entity
  id: Blocker
  components:
  - type: Transform
    anchored: true
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.5,-0.5,0.5,0.5"
        hard: true
        layer: [ Impassable ]
- type: entity
  id: Engineer
  components:
  - type: Hands
  - type: ComplexInteraction
  - type: DoAfter
  - type: Physics
    bodyType: KinematicController
"#;

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

fn at(world: &World, grid: Entity, x: i32, y: i32, prototype: &str) -> Option<Entity> {
    let map_grid = world.get::<astrocore_map::MapGrid>(grid)?;
    map_grid
        .anchored_at(TileCoord::new(x, y, 0))
        .iter()
        .copied()
        .find(|e| {
            world
                .get::<MetaData>(*e)
                .and_then(|m| m.prototype.as_deref())
                == Some(prototype)
        })
}

#[test]
fn steel_turns_a_girder_into_a_wall_and_welding_takes_it_apart() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Engineer", grid, 1, 1);
    world.add_hand(user, "right", Hand::default());
    let girder = spawn_at(&mut world, "TestGirderEntity", grid, 2, 1);
    let steel = spawn_at(&mut world, "SheetSteel", grid, 1, 1);
    assert!(world.try_pickup(user, steel, None, false));
    let click = world.map_coordinates(girder).unwrap();
    assert!(world.interact_using(user, steel, girder, click));
    // Ожидание 2 секунды, затем на месте балки стена.
    run(&mut world, 2.2);
    let wall = at(&world, grid, 2, 1, "TestWallEntity").expect("стена");
    assert!(!world.is_alive(girder) || at(&world, grid, 2, 1, "TestGirderEntity").is_none());
    assert_eq!(world.get::<Stack>(steel).unwrap().count, 28);
    assert_eq!(world.get::<Construction>(wall).unwrap().node, "wall");
    assert!(world.get::<Transform>(wall).unwrap().anchored());

    // Без сварки сталь стену не меняет.
    assert!(!world.interact_using(user, steel, wall, click));
    assert!(world.try_drop(user, "right", None, false));
    let welder = spawn_at(&mut world, "Welder", grid, 1, 1);
    assert!(world.try_pickup(user, welder, None, false));
    assert!(world.interact_using(user, welder, wall, click));
    run(&mut world, 3.2);
    assert!(at(&world, grid, 2, 1, "TestGirderEntity").is_some());
    assert!(at(&world, grid, 2, 1, "TestWallEntity").is_none());
}

#[test]
fn graph_path_and_conditions() {
    let mut world = world_with(PROTOTYPES);
    let graph = graph(&mut world, "TestGirder").unwrap();
    assert_eq!(
        graph.path("start", "wall").unwrap(),
        ["start", "girder", "wall"].map(String::from)
    );
    let grid = station(&mut world);
    let girder = spawn_at(&mut world, "TestGirderEntity", grid, 2, 1);
    let anchored = astrocore_data::parse_yaml("!type:EntityAnchored {}").unwrap();
    assert!(check_condition(&world, girder, &anchored));
    let unknown = astrocore_data::parse_yaml("!type:SomethingNew {}").unwrap();
    assert!(!check_condition(&world, girder, &unknown));
}

#[test]
fn menu_builds_a_girder_from_steel_on_a_free_tile() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Engineer", grid, 1, 1);
    world.add_hand(user, "right", Hand::default());
    let steel = spawn_at(&mut world, "SheetSteel", grid, 1, 1);
    assert!(world.try_pickup(user, steel, None, false));
    assert_eq!(recipes(&world).len(), 1);

    let target = world
        .to_map_coordinates(astrocore_map::EntityCoordinates::new(
            grid,
            TileCoord::new(2, 1, 0).center(),
        ))
        .unwrap();
    let request = |ack| StartStructureConstruction {
        prototype: "TestGirderRecipe".into(),
        x: target.position.x,
        y: target.position.y,
        angle: 0.0,
        ack,
    };
    assert!(start_structure_construction(&mut world, user, &request(1)));
    world.run_tick();
    let girder = at(&world, grid, 2, 1, "TestGirderEntity").expect("балка");
    assert_eq!(world.get::<Stack>(steel).unwrap().count, 28);
    let construction = world.get::<Construction>(girder).unwrap();
    assert_eq!(construction.node, "girder");
    assert_eq!(construction.target_node.as_deref(), Some("wall"));

    // Клетка занята твёрдой балкой — нет. Без материалов — тоже нет.
    spawn_at(&mut world, "Blocker", grid, 3, 1);
    let blocked = world
        .to_map_coordinates(astrocore_map::EntityCoordinates::new(
            grid,
            TileCoord::new(3, 1, 0).center(),
        ))
        .unwrap();
    let mut on_blocked = request(2);
    on_blocked.x = blocked.position.x;
    on_blocked.y = blocked.position.y;
    assert!(!start_structure_construction(&mut world, user, &on_blocked));
    world.get_mut::<Stack>(steel).unwrap().count = 1;
    assert!(!start_structure_construction(&mut world, user, &request(3)));
    assert_eq!(world.get::<Stack>(steel).unwrap().count, 1);
}
