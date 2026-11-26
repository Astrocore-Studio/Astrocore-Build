use astrocore_ecs::{GameWorldExt, MetaData};
use astrocore_map::{MapWorldExt, TileCoord, TileDefinitions, TransformWorldExt};

use super::*;
use crate::hands::{Hand, HandsWorldExt};
use crate::stack::Stack;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: tile
  id: Lattice
  baseTurf: Space
  deconstructTools: [ Cutting ]
  itemDrop: PartRodMetal1
- type: tile
  id: Plating
  baseTurf: Lattice
- type: tile
  id: FloorSteel
  baseTurf: Plating
  deconstructTools: [ Prying ]
  itemDrop: FloorTileItemSteel1
- type: tile
  id: FloorWood
  baseTurf: Plating
  deconstructTools: [ Prying ]
- type: stack
  id: Rods
  spawn: PartRodMetal1
  maxCount: 30
- type: stack
  id: FloorTileSteel
  spawn: FloorTileItemSteel1
  maxCount: 30
- type: entity
  id: PartRodMetal
  components:
  - type: Item
  - type: Stack
    stackType: Rods
    count: 30
  - type: FloorTile
    outputs: [ Lattice ]
- type: entity
  id: PartRodMetal1
  parent: PartRodMetal
  components:
  - type: Stack
    stackType: Rods
    count: 1
- type: entity
  id: FloorTileItemSteel
  components:
  - type: Item
  - type: Stack
    stackType: FloorTileSteel
    count: 30
  - type: FloorTile
    outputs: [ Plating, FloorSteel ]
- type: entity
  id: FloorTileItemSteel1
  parent: FloorTileItemSteel
  components:
  - type: Stack
    stackType: FloorTileSteel
    count: 1
- type: entity
  id: Crowbar
  components:
  - type: Item
  - type: Tool
    qualities: [ Prying ]
  - type: ToolTileCompatible
- type: entity
  id: Engineer
  components:
  - type: Hands
  - type: ComplexInteraction
  - type: DoAfter
  - type: Physics
    bodyType: KinematicController
"#;

fn setup() -> (World, Entity, Entity) {
    let mut world = world_with(PROTOTYPES);
    for tile in ["Lattice", "Plating", "FloorSteel", "FloorWood"] {
        world
            .resource_mut::<TileDefinitions>()
            .register(tile, None, 1);
    }
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Engineer", grid, 14, 3);
    world.add_hand(user, "right", Hand::default());
    (world, grid, user)
}

fn tile(world: &World, grid: Entity, x: i32, y: i32) -> String {
    tile_name(world, grid, TileCoord::new(x, y, 0))
}

fn click(world: &mut World, user: Entity, grid: Entity, x: i32, y: i32) {
    let at = world
        .to_map_coordinates(EntityCoordinates::new(
            grid,
            TileCoord::new(x, y, 0).center(),
        ))
        .unwrap();
    world.user_interaction(user, at, None, false);
}

fn run(world: &mut World, seconds: f32) {
    for _ in 0..(seconds * 30.0).ceil() as u32 {
        world.run_tick();
    }
}

#[test]
fn tiles_go_on_plating_and_come_off_with_a_crowbar() {
    let (mut world, grid, user) = setup();
    let tiles = spawn_at(&mut world, "FloorTileItemSteel", grid, 14, 3);
    assert!(world.try_pickup(user, tiles, None, false));
    click(&mut world, user, grid, 15, 3);
    assert_eq!(tile(&world, grid, 15, 3), "FloorSteel");
    assert_eq!(world.get::<Stack>(tiles).unwrap().count, 29);

    assert!(world.try_drop(user, "right", None, false));
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 14, 3);
    assert!(world.try_pickup(user, crowbar, None, false));
    click(&mut world, user, grid, 15, 3);
    run(&mut world, 1.2);
    assert_eq!(tile(&world, grid, 15, 3), "Plating");
    // Выпала плитка.
    let dropped = world
        .query::<&MetaData>()
        .iter(&world)
        .filter(|m| m.prototype.as_deref() == Some("FloorTileItemSteel1"))
        .count();
    assert_eq!(dropped, 1);
    // Обшивку ломом не снять.
    click(&mut world, user, grid, 15, 3);
    run(&mut world, 1.2);
    assert_eq!(tile(&world, grid, 15, 3), "Plating");
}

#[test]
fn rods_make_lattice_over_space_next_to_the_station() {
    let (mut world, grid, user) = setup();
    world.set_tile(grid, TileCoord::new(15, 3, 0), Tile::default());
    let rods = spawn_at(&mut world, "PartRodMetal", grid, 14, 3);
    assert!(world.try_pickup(user, rods, None, false));
    // Пробой у края: под клеткой нет тайла, решётка встаёт на ближайший грид.
    click(&mut world, user, grid, 15, 3);
    assert_eq!(tile(&world, grid, 15, 3), "Lattice");
    // За краем станции.
    let edge = EntityCoordinates::new(grid, TileCoord::new(15, 3, 0).center());
    world.set_coordinates(user, edge, None).unwrap();
    click(&mut world, user, grid, 16, 3);
    assert_eq!(tile(&world, grid, 16, 3), "Lattice");
    assert!(world.try_drop(user, "right", None, false));
    let tiles = spawn_at(&mut world, "FloorTileItemSteel", grid, 14, 3);
    assert!(world.try_pickup(user, tiles, None, false));
    click(&mut world, user, grid, 15, 3);
    assert_eq!(tile(&world, grid, 15, 3), "Plating");
    click(&mut world, user, grid, 15, 3);
    assert_eq!(tile(&world, grid, 15, 3), "FloorSteel");
    // Одного грида хватает: новый не появился.
    let map = world
        .map_entity(world.map_coordinates(user).unwrap().map)
        .unwrap();
    assert_eq!(
        world.get::<astrocore_map::Map>(map).unwrap().grids().len(),
        1
    );
}

#[test]
fn history_remembers_the_layer_under_a_replaced_tile() {
    let (mut world, grid, _) = setup();
    let cell = TileCoord::new(3, 3, 0);
    let steel = tile_def(&world, "FloorSteel").unwrap();
    let wood = tile_def(&world, "FloorWood").unwrap();
    assert!(replace_tile(&mut world, grid, cell, &steel));
    assert!(
        world
            .get::<TileHistory>(grid)
            .is_none_or(|h| h.stacks.is_empty())
    );
    // Дерево поверх стали: сталь уходит в историю и возвращается при снятии.
    assert!(replace_tile(&mut world, grid, cell, &wood));
    assert!(deconstruct_tile(&mut world, grid, cell, false));
    assert_eq!(tile_name(&world, grid, cell), "FloorSteel");
    assert!(deconstruct_tile(&mut world, grid, cell, false));
    assert_eq!(tile_name(&world, grid, cell), "Plating");
}
