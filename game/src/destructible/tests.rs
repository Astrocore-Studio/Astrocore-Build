use astrocore_ecs::{GameWorldExt, LifecycleWorldExt, MetaData};
use astrocore_map::{MapGrid, TileCoord};

use super::*;
use crate::damage::{DamageSpecifier, DamageableWorldExt};
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: stack
  id: Steel
  spawn: SheetSteel1
  maxCount: 30
- type: entity
  id: SheetSteel1
  components:
  - type: Item
  - type: Stack
    stackType: Steel
    count: 1
- type: constructionGraph
  id: TestGirder
  start: start
  graph:
  - node: start
  - node: girder
    entity: TestGirder
  - node: wall
    entity: TestWall
- type: entity
  id: TestGirder
  components:
  - type: Transform
    anchored: true
  - type: Construction
    graph: TestGirder
    node: girder
- type: entity
  id: TestWall
  components:
  - type: Transform
    anchored: true
  - type: Damageable
  - type: Injurable
  - type: Construction
    graph: TestGirder
    node: wall
  - type: Destructible
    thresholds:
    - trigger: !type:DamageTrigger
        damage: 100
      behaviors:
      - !type:ChangeConstructionNodeBehavior
        node: girder
      - !type:SpawnEntitiesBehavior
        spawn:
          SheetSteel1:
            min: 2
            max: 2
- type: entity
  id: TestCrate
  components:
  - type: Damageable
  - type: Injurable
  - type: Destructible
    thresholds:
    - trigger: !type:DamageTrigger
        damage: 50
      behaviors:
      - !type:DoActsBehavior
        acts: [ "Destruction" ]
      - !type:PlaySoundBehavior
        sound:
          path: /Audio/Effects/metalbreak.ogg
    - trigger: !type:DamageTypeTrigger
        damageType: Heat
        damage: 10
      behaviors:
      - !type:SpillBehavior
        solution: food
"#;

fn blunt(amount: f32) -> DamageSpecifier {
    DamageSpecifier::new([("Blunt", FixedPoint2::from_f32(amount))])
}

fn count(world: &mut World, prototype: &str) -> usize {
    world
        .query::<&MetaData>()
        .iter(world)
        .filter(|m| m.prototype.as_deref() == Some(prototype))
        .count()
}

#[test]
fn wall_turns_into_a_girder_and_drops_steel() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let wall = spawn_at(&mut world, "TestWall", grid, 3, 3);
    assert_eq!(destroyed_at(&world, wall), FixedPoint2::MAX);
    world.change_damage(wall, &blunt(60.0), false, false, None);
    assert!(world.is_alive(wall));
    world.change_damage(wall, &blunt(60.0), false, false, None);
    world.run_tick();
    assert!(!world.is_alive(wall));
    let anchored = world
        .get::<MapGrid>(grid)
        .unwrap()
        .anchored_at(TileCoord::new(3, 3, 0))
        .to_vec();
    assert_eq!(anchored.len(), 1);
    assert_eq!(
        world
            .get::<MetaData>(anchored[0])
            .unwrap()
            .prototype
            .as_deref(),
        Some("TestGirder")
    );
    assert_eq!(count(&mut world, "SheetSteel1"), 1);
    let steel = world
        .query::<&crate::stack::Stack>()
        .iter(&world)
        .next()
        .unwrap()
        .count;
    assert_eq!(steel, 2);
}

#[test]
fn crate_breaks_at_its_threshold_and_overkill_is_added() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let crate_ = spawn_at(&mut world, "TestCrate", grid, 3, 3);
    let thresholds = &world.get::<Destructible>(crate_).unwrap().thresholds;
    // Переизбыток: 2 × 50 = 100, но не меньше 100.
    assert_eq!(thresholds.len(), 3);
    assert_eq!(
        thresholds[0].trigger,
        Trigger::DamageTrigger {
            damage: FixedPoint2::from_f32(100.0)
        }
    );
    assert_eq!(destroyed_at(&world, crate_), FixedPoint2::from_f32(50.0));
    world.change_damage(crate_, &blunt(55.0), false, false, None);
    assert!(is_destroyed(&world, crate_));
    world.run_tick();
    assert!(!world.is_alive(crate_));
}

#[test]
fn equal_triggers_merge_and_sort_by_damage() {
    let threshold = |damage: f32, act: &str| DamageThreshold {
        trigger: Trigger::DamageTrigger {
            damage: FixedPoint2::from_f32(damage),
        },
        behaviors: vec![Behavior::DoActsBehavior {
            acts: vec![act.into()],
        }],
        triggered: false,
        triggers_once: false,
        old_triggered: false,
    };
    let list = normalize(vec![
        threshold(200.0, "Destruction"),
        threshold(50.0, "Breakage"),
        threshold(200.0, "Breakage"),
    ]);
    assert_eq!(list.len(), 2);
    assert_eq!(
        list[0].trigger,
        Trigger::DamageTrigger {
            damage: FixedPoint2::from_f32(50.0)
        }
    );
    assert_eq!(list[1].behaviors.len(), 2);
}
