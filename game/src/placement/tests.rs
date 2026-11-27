use astrocore_ecs::{GameWorldExt, MetaData};
use astrocore_map::{MapGrid, MapId, MapWorldExt, TileCoord, TileDefinitions, TransformWorldExt};
use astrocore_math::Vec2;
use astrocore_net::{ConnId, NetEntity, ServerNet, Session, SessionWorldExt};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Body
  components:
  - type: Physics
    bodyType: KinematicController
- type: entity
  id: Crate
- type: decal
  id: Bot
  sprite:
    sprite: Decals/markings.rsi
    state: bot
"#;

fn mapper(world: &mut World, address: &str) -> (Entity, Entity) {
    let grid = station(world);
    let body = spawn_at(world, "Body", grid, 1, 1);
    world
        .resource_mut::<ServerNet>()
        .addresses
        .insert(ConnId(1), address.parse().unwrap());
    let session = world
        .spawn(Session {
            conn: ConnId(1),
            user_id: astrocore_net::NetUserId::random(),
            name: "Mapper".into(),
            attached: None,
            last_input: 0,
            sync: Default::default(),
        })
        .id();
    world.attach_player(session, body);
    crate::admin_manager::load_admin(world, session);
    (session, grid)
}

fn crates(world: &mut World) -> Vec<Entity> {
    world
        .query::<(Entity, &MetaData)>()
        .iter(world)
        .filter(|(_, meta)| meta.prototype.as_deref() == Some("Crate"))
        .map(|(entity, _)| entity)
        .collect()
}

#[test]
fn mapper_places_and_erases_entities() {
    let mut world = world_with(PROTOTYPES);
    let (session, grid) = mapper(&mut world, "127.0.0.1:1");
    let map = world.map_coordinates(grid).unwrap().map;
    handle(
        &mut world,
        session,
        &PlacementRequest::Entity {
            prototype: "Crate".into(),
            map: map.0,
            position: Vec2::new(4.5, 4.5),
            rotation: std::f32::consts::FRAC_PI_2,
        },
    );
    let placed = crates(&mut world);
    assert_eq!(placed.len(), 1);
    assert_eq!(world.parent(placed[0]), Some(grid));
    assert_eq!(
        world.map_coordinates(placed[0]).unwrap().position,
        Vec2::new(4.5, 4.5)
    );

    // Стирание по сетевому номеру: номер сущность получает в такте сервера.
    world.run_tick();
    let net = *world.get::<NetEntity>(placed[0]).unwrap();
    handle(
        &mut world,
        session,
        &PlacementRequest::EraseEntity { entity: net },
    );
    assert!(crates(&mut world).is_empty());
}

#[test]
fn rect_erase_spares_grids_and_players() {
    let mut world = world_with(PROTOTYPES);
    let (session, grid) = mapper(&mut world, "127.0.0.1:1");
    for x in 0..4 {
        spawn_at(&mut world, "Crate", grid, x, 1);
    }
    let map = world.map_coordinates(grid).unwrap().map;
    handle(
        &mut world,
        session,
        &PlacementRequest::EraseRect {
            map: map.0,
            min: Vec2::new(0.0, 0.0),
            max: Vec2::new(2.9, 3.0),
        },
    );
    // Ящики в клетках 0..2 стёрты, в клетке 3 — нет; игрок в клетке (1, 1) цел.
    assert_eq!(crates(&mut world).len(), 1);
    assert!(world.get_entity(grid).is_ok());
    let body = world.get::<Session>(session).unwrap().attached.unwrap();
    assert!(world.get_entity(body).is_ok());
}

#[test]
fn tiles_go_onto_grids_or_start_a_new_grid() {
    let mut world = world_with(PROTOTYPES);
    let (session, grid) = mapper(&mut world, "127.0.0.1:1");
    let map = world.map_coordinates(grid).unwrap().map;
    let space = astrocore_map::SPACE_TILE.to_owned();
    handle(
        &mut world,
        session,
        &PlacementRequest::Tile {
            tile: space,
            map: map.0,
            position: Vec2::new(2.5, 2.5),
        },
    );
    assert_eq!(
        world
            .get::<MapGrid>(grid)
            .unwrap()
            .tile(TileCoord::new(2, 2, 0)),
        astrocore_map::Tile::EMPTY
    );

    world
        .resource_mut::<TileDefinitions>()
        .register("FloorSteel", None, 1);
    let floor = "FloorSteel".to_owned();
    handle(
        &mut world,
        session,
        &PlacementRequest::Tile {
            tile: floor,
            map: map.0,
            position: Vec2::new(40.5, 40.5),
        },
    );
    let map_entity = world.map_entity(map).unwrap();
    let grids = world
        .get::<astrocore_map::Map>(map_entity)
        .unwrap()
        .grids()
        .to_vec();
    assert_eq!(grids.len(), 2);
    let new_grid = grids.into_iter().find(|g| *g != grid).unwrap();
    assert_eq!(world.get::<MapGrid>(new_grid).unwrap().tile_count(), 1);
    assert_eq!(
        world.map_coordinates(new_grid).unwrap().position,
        Vec2::new(40.0, 40.0)
    );
}

#[test]
fn strangers_may_not_place() {
    let mut world = world_with(PROTOTYPES);
    let (session, grid) = mapper(&mut world, "192.0.2.4:1");
    let map = world.map_coordinates(grid).unwrap().map;
    handle(
        &mut world,
        session,
        &PlacementRequest::Entity {
            prototype: "Crate".into(),
            map: map.0,
            position: Vec2::new(4.5, 4.5),
            rotation: 0.0,
        },
    );
    assert!(crates(&mut world).is_empty());
    let _ = MapId(0);
}

#[test]
fn decals_are_placed_on_the_grid_and_erased() {
    let mut world = world_with(PROTOTYPES);
    let (session, grid) = mapper(&mut world, "127.0.0.1:1");
    let map = world.map_coordinates(grid).unwrap().map;
    let place = |id: &str| PlacementRequest::Decal {
        id: id.into(),
        map: map.0,
        corner: Vec2::new(3.0, 4.0),
        rotation: 0.0,
        color: Some("#FF0000FF".into()),
        z_index: 2,
        cleanable: true,
    };
    handle(&mut world, session, &place("Bot"));
    // Неизвестная декаль не ставится.
    handle(&mut world, session, &place("NoSuchDecal"));
    let decals = world.get::<crate::decals::GridDecals>(grid).unwrap();
    assert_eq!(decals.decals.len(), 1);
    let decal = decals.decals.values().next().unwrap();
    assert_eq!(decal.coordinates, Vec2::new(3.0, 4.0));
    assert_eq!(decal.color.as_deref(), Some("#FF0000FF"));
    assert_eq!((decal.z_index, decal.cleanable), (2, true));

    handle(
        &mut world,
        session,
        &PlacementRequest::EraseDecals {
            map: map.0,
            min: Vec2::new(2.5, 3.5),
            max: Vec2::new(3.5, 4.5),
        },
    );
    assert!(
        world
            .get::<crate::decals::GridDecals>(grid)
            .unwrap()
            .decals
            .is_empty()
    );
}
