use astrocore_data::{parse_yaml, to_yaml_string};
use astrocore_map::{MapLoadWorldExt, MapSaveOptions, MapSaveWorldExt, TileDefinitions};
use astrocore_math::Vec2;
use bevy_ecs::world::World;

use super::*;
use crate::testing::world_with;

const PROTOTYPES: &str = r#"
- type: entity
  id: ChunkEntity
  name: Chunk Entity
"#;

/// Карта с гридом 2×1 и одной чанк-сущностью, как в Saltern.
const MAP: &str = r#"
meta:
  format: 7
maps: [1]
grids: [2]
orphans: []
nullspace: [3]
tilemap:
  0: Space
  1: FloorSteel
entities:
- proto: ""
  entities:
  - uid: 1
    components:
    - type: MetaData
    - type: Transform
    - type: Map
      mapPaused: True
  - uid: 2
    components:
    - type: MetaData
    - type: Transform
      parent: 1
    - type: MapGrid
      chunks:
        -1,-2:
          ind: -1,-2
          tiles: AQAAAAAAAA==
          version: 7
    - type: ChunkContainer
      chunkEntities:
      - 3
- proto: ChunkEntity
  entities:
  - uid: 3
    components:
    - type: ChunkEntity
      root: 2
      chunk: -1,-2
    - type: DecalChunk
      maxDecalId: 719
      decals:
        version: 1
        nodes:
        - node:
            id: Arrows
            color: '#FFFFFFFF'
            angle: -1.5707963267948966 rad
          decals:
            581: -12,-30
        - node:
            zIndex: 1
            id: Bot
            color: '#FFFFFFFF'
          decals:
            710: -7,-26
            711: -6,-26
"#;

fn world() -> World {
    let mut world = world_with(PROTOTYPES);
    world
        .resource_mut::<TileDefinitions>()
        .register("FloorSteel", None, 1);
    world
}

#[test]
fn chunk_entities_become_grid_decals_and_save_back_the_same() {
    let mut world = world();
    let report = world.load_map(&parse_yaml(MAP).unwrap(), false).unwrap();
    let grid = report.grids[0];
    let decals = world.get::<GridDecals>(grid).unwrap();
    assert_eq!(decals.decals.len(), 3);
    let arrow = &decals.decals[&(-1, -2, 581)];
    assert_eq!(arrow.id, "Arrows");
    assert_eq!(arrow.coordinates, Vec2::new(-12.0, -30.0));
    assert!((arrow.angle + std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    assert_eq!(decals.decals[&(-1, -2, 710)].z_index, 1);

    let map = report.maps[0].0;
    write_for_save(&mut world, map);
    let saved = world.save_map(map, MapSaveOptions::default()).unwrap();
    let text = to_yaml_string(&saved.document);
    assert!(text.contains("nullspace:\n- 3\n"), "{text}");
    assert!(text.contains("            581: -12,-30\n"), "{text}");
    assert!(text.contains("      maxDecalId: 719\n"), "{text}");
    assert!(text.contains("      chunkEntities:\n      - 3\n"), "{text}");

    // Повторная загрузка и сохранение дают тот же текст.
    let again = world.load_map(&parse_yaml(&text).unwrap(), false).unwrap();
    let map_again = again.maps[0].0;
    write_for_save(&mut world, map_again);
    let second = world
        .save_map(map_again, MapSaveOptions::default())
        .unwrap();
    assert_eq!(to_yaml_string(&second.document), text);
}

#[test]
fn placed_decals_get_chunk_ids_and_new_chunk_entities() {
    let mut world = world();
    let report = world.load_map(&parse_yaml(MAP).unwrap(), false).unwrap();
    let grid = report.grids[0];
    let map = report.maps[0].0;
    let decal = |id: &str| Decal {
        coordinates: Vec2::ZERO,
        id: id.into(),
        color: None,
        angle: 0.0,
        z_index: 0,
        cleanable: false,
    };
    // Тот же чанк: номер после наибольшего выданного; другой чанк — с единицы.
    assert_eq!(
        place(&mut world, grid, Vec2::new(-10.0, -20.0), decal("Bot")),
        (-1, -2, 720)
    );
    assert_eq!(
        place(&mut world, grid, Vec2::new(3.0, 1.0), decal("Box")),
        (0, 0, 1)
    );
    assert_eq!(
        remove_in(
            &mut world,
            grid,
            Vec2::new(-13.0, -31.0),
            Vec2::new(-11.0, -29.0)
        ),
        1
    );

    write_for_save(&mut world, map);
    let text = to_yaml_string(
        &world
            .save_map(map, MapSaveOptions::default())
            .unwrap()
            .document,
    );
    // Новая чанк-сущность получила следующий свободный номер файла.
    assert!(text.contains("nullspace:\n- 3\n- 4\n"), "{text}");
    assert!(
        text.contains("      chunkEntities:\n      - 3\n      - 4\n"),
        "{text}"
    );
    assert!(!text.contains("581:"), "{text}");
    let reloaded = world.load_map(&parse_yaml(&text).unwrap(), false).unwrap();
    let decals = world.get::<GridDecals>(reloaded.grids[0]).unwrap();
    assert_eq!(decals.decals.len(), 4);
    assert_eq!(decals.decals[&(0, 0, 1)].id, "Box");
}

#[test]
fn old_decal_grid_is_read_and_saved_as_chunk_entities() {
    let mut world = world();
    let old = MAP
        .replace(
            "    - type: ChunkContainer\n      chunkEntities:\n      - 3\n",
            "    - type: DecalGrid\n      chunkCollection:\n        version: 2\n        nodes:\n        - node:\n            color: '#FFFFFFFF'\n            id: Rock01\n          decals:\n            32: 7.6594696,-11.012184\n",
        );
    let old = &old[..old.find("- proto: ChunkEntity").unwrap()];
    let old = old.replace("nullspace: [3]", "nullspace: []");
    let report = world.load_map(&parse_yaml(&old).unwrap(), false).unwrap();
    let grid = report.grids[0];
    let decals = world.get::<GridDecals>(grid).unwrap();
    assert_eq!(decals.decals.len(), 1);
    let rock = decals.decals.values().next().unwrap();
    assert_eq!(rock.coordinates, Vec2::new(7.6594696, -11.012184));

    let map = report.maps[0].0;
    write_for_save(&mut world, map);
    let text = to_yaml_string(
        &world
            .save_map(map, MapSaveOptions::default())
            .unwrap()
            .document,
    );
    assert!(!text.contains("DecalGrid"), "{text}");
    assert!(text.contains("- proto: ChunkEntity\n"), "{text}");
    assert!(text.contains("7.6594696,-11.012184"), "{text}");
}
