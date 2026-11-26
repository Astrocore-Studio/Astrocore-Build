use astrocore_ecs::{GameWorldExt, MetaData};
use astrocore_map::{MapWorldExt, Tile, TileCoord, TileDefinitions, TransformWorldExt};
use astrocore_net::{AppearanceValue, AppearanceWorldExt};

use super::*;
use crate::hands::HandsWorldExt;
use crate::interaction::InteractionWorldExt;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Human
  components:
  - type: DoAfter
  - type: Hands
  - type: ComplexInteraction
  - type: Physics
    bodyType: KinematicController
- type: tool
  id: Cutting
  name: cutting
- type: entity
  id: Wirecutter
  components:
  - type: Item
  - type: Tool
    qualities: [ Cutting ]
- type: entity
  id: CableHV
  components:
  - type: Transform
    anchored: true
  - type: Appearance
  - type: Cable
    cableDroppedOnCutPrototype: CableHVStack1
    cableType: HighVoltage
  - type: CableVis
    node: power
  - type: NodeContainer
    nodes:
      power: !type:CableNode
        nodeGroupID: HVPower
- type: entity
  id: CableHVStack1
  components:
  - type: Item
  - type: Stack
    stackType: CableHV
    count: 1
  - type: CablePlacer
    cablePrototypeID: CableHV
    blockingWireType: HighVoltage
- type: entity
  id: CableHVStack
  parent: CableHVStack1
  components:
  - type: Stack
    stackType: CableHV
    count: 10
- type: stack
  id: CableHV
  name: HV cable
  spawn: CableHVStack1
  maxCount: 30
"#;

fn world_with_tiles() -> (World, Entity) {
    let mut world = world_with(PROTOTYPES);
    world
        .resource_mut::<TileDefinitions>()
        .register("Plating", None, 1);
    let floor = world
        .resource_mut::<TileDefinitions>()
        .register("FloorSteel", None, 1);
    let mut properties = TileProperties::default();
    properties.set("Plating", true, true);
    properties.set("FloorSteel", false, true);
    world.insert_resource(properties);
    let grid = station(&mut world);
    // Полоса пола по y = 5.
    for x in 0..16 {
        world.set_tile(grid, TileCoord::new(x, 5, 0), Tile::new(floor));
    }
    (world, grid)
}

fn human(world: &mut World, grid: Entity) -> Entity {
    let user = spawn_at(world, "Human", grid, 1, 1);
    world.add_hand(user, "right", crate::hands::Hand::default());
    user
}

fn cables(world: &mut World) -> Vec<Entity> {
    world
        .query::<(Entity, &Cable)>()
        .iter(world)
        .map(|(e, _)| e)
        .collect()
}

#[test]
fn cable_goes_on_plating_but_not_on_floor_or_twice() {
    let (mut world, grid) = world_with_tiles();
    let user = human(&mut world, grid);
    let coil = spawn_at(&mut world, "CableHVStack", grid, 1, 1);
    assert!(world.try_pickup(user, coil, None, false));
    let at = |world: &World, x: f32, y: f32| {
        let mut point = world.map_coordinates(grid).unwrap();
        point.position = astrocore_math::Vec2::new(x, y);
        point
    };
    let point = at(&world, 2.5, 1.5);
    world.user_interaction(user, point, None, false);
    assert_eq!(cables(&mut world).len(), 1);
    world.user_interaction(user, point, None, false);
    assert_eq!(
        cables(&mut world).len(),
        1,
        "второй кабель того же вида не кладётся"
    );
    let floor = at(&world, 2.5, 5.5);
    world.user_interaction(user, floor, None, false);
    assert_eq!(cables(&mut world).len(), 1, "на пол кабель не кладётся");
    assert_eq!(world.get::<crate::stack::Stack>(coil).unwrap().count, 9);
}

#[test]
fn neighbours_set_the_mask_and_cutting_drops_a_coil() {
    let (mut world, grid) = world_with_tiles();
    let left = spawn_at(&mut world, "CableHV", grid, 2, 1);
    let right = spawn_at(&mut world, "CableHV", grid, 3, 1);
    spawn_at(&mut world, "CableHV", grid, 2, 2);
    world.run_tick();
    // Сосед справа (восток) и сверху (север).
    assert_eq!(
        world.appearance(left, CONNECTED_MASK),
        Some(&AppearanceValue::Int(1 | 4))
    );
    assert_eq!(
        world.appearance(right, CONNECTED_MASK),
        Some(&AppearanceValue::Int(8))
    );

    let user = human(&mut world, grid);
    // Рядом с кабелем: дальность взаимодействия — полторы клетки.
    world
        .set_local_position(user, astrocore_math::Vec2::new(4.5, 2.5))
        .unwrap();
    let cutter = spawn_at(&mut world, "Wirecutter", grid, 4, 2);
    assert!(world.try_pickup(user, cutter, None, false));
    let at = world.map_coordinates(right).unwrap();
    world.interact_using(user, cutter, right, at);
    for _ in 0..30 {
        world.run_tick();
    }
    assert!(world.get_entity(right).is_err());
    let coils = world
        .query::<&MetaData>()
        .iter(&world)
        .filter(|m| m.prototype.as_deref() == Some("CableHVStack1"))
        .count();
    assert_eq!(coils, 1);
    assert_eq!(
        world.appearance(left, CONNECTED_MASK),
        Some(&AppearanceValue::Int(1))
    );
}
