use astrocore_data::parse_yaml;
use astrocore_ecs::MetaData;
use astrocore_map::ContainerWorldExt;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Part
  components:
  - type: Item
- type: entity
  id: Jar
  components:
  - type: Item
- type: entityTable
  id: SpareParts
  table: !type:AllSelector
    children:
    - id: Part
    - id: Jar
      amount: 2
- type: entity
  id: Crate
  components:
  - type: EntityStorage
  - type: EntityTableContainerFill
    containers:
      entity_storage:
        id: Part
        amount: 9
- type: entity
  id: NestedCrate
  components:
  - type: EntityStorage
  - type: EntityTableContainerFill
    containers:
      entity_storage: !type:NestedSelector
        tableId: SpareParts
- type: entity
  id: Locker
  components:
  - type: EntityStorage
  - type: StorageFill
    contents:
    - id: Part
      amount: 2
    - id: Jar
      prob: 0
"#;

fn contents(world: &World, owner: Entity) -> Vec<String> {
    let mut found: Vec<String> = world
        .container(owner, crate::entity_storage::ENTITY_STORAGE_CONTAINER)
        .map(|c| c.entities().to_vec())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            world
                .get::<MetaData>(e)?
                .prototype
                .as_deref()
                .map(str::to_owned)
        })
        .collect();
    found.sort();
    found
}

#[test]
fn crates_and_lockers_get_their_contents_on_map_init() {
    let mut world = world_with(PROTOTYPES);
    world.insert_resource(GameRandom::seeded(1));
    let grid = station(&mut world);
    let crate_ = spawn_at(&mut world, "Crate", grid, 1, 1);
    assert_eq!(contents(&world, crate_), vec!["Part".to_owned(); 9]);
    let nested = spawn_at(&mut world, "NestedCrate", grid, 2, 1);
    assert_eq!(contents(&world, nested), ["Jar", "Jar", "Part"]);
    let locker = spawn_at(&mut world, "Locker", grid, 3, 1);
    assert_eq!(contents(&world, locker), ["Part", "Part"]);
}

#[test]
fn tables_roll_groups_ranges_and_probabilities() {
    let world = world_with(PROTOTYPES);
    let prototypes = world.resource::<Prototypes>();
    let table = parse_yaml(
        "!type:GroupSelector\nchildren:\n- id: Part\n  weight: 0\n- id: Jar\n  amount: 2,3\n",
    )
    .unwrap();
    let mut rng = GameRandom::seeded(3);
    for _ in 0..20 {
        let spawns = table_spawns(&table, prototypes, &mut rng);
        assert!((2..=3).contains(&spawns.len()), "{spawns:?}");
        assert!(spawns.iter().all(|s| s == "Jar"));
    }
    let none = parse_yaml("id: Part\nprob: 0\n").unwrap();
    assert!(table_spawns(&none, prototypes, &mut rng).is_empty());
}

#[test]
fn or_groups_pick_one_entry() {
    let entries: Vec<EntitySpawnEntry> = astrocore_data::from_node(
        &parse_yaml(
            "- id: A\n  orGroup: g\n  prob: 0.5\n- id: B\n  orGroup: g\n  prob: 0.5\n- id: C\n  amount: 2\n  maxAmount: 4\n",
        )
        .unwrap(),
    )
    .unwrap();
    let mut rng = GameRandom::seeded(5);
    for _ in 0..20 {
        let spawns = spawn_entries(&entries, &mut rng);
        let picked = spawns.iter().filter(|s| *s == "A" || *s == "B").count();
        assert_eq!(picked, 1, "{spawns:?}");
        let c = spawns.iter().filter(|s| *s == "C").count();
        assert!((2..4).contains(&c), "{spawns:?}");
    }
}
