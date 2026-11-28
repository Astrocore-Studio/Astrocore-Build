use astrocore_map::TransformWorldExt;
use astrocore_math::Vec2;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: Hands
    startingHands:
      hand_right:
        location: Right
- type: entity
  id: Table
  components:
  - type: PlaceableSurface
- type: entity
  id: Hotplate
  components:
  - type: PlaceableSurface
    placeCentered: true
    positionOffset: 0, 0.25
  - type: ItemPlacer
- type: entity
  id: Cup
  components:
  - type: Item
"#;

fn click(world: &mut World, user: Entity, used: Entity, surface: Entity, at: Vec2) -> bool {
    let mut map = world.map_coordinates(surface).unwrap();
    map.position = at;
    let mut event = AfterInteractUsing {
        user,
        used,
        target: surface,
        click: map,
        can_reach: true,
        handled: false,
    };
    world.raise(surface, &mut event);
    event.handled
}

#[test]
fn items_go_where_clicked_or_centered() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let table = spawn_at(&mut world, "Table", grid, 3, 2);
    let plate = spawn_at(&mut world, "Hotplate", grid, 4, 2);
    let cup = spawn_at(&mut world, "Cup", grid, 2, 2);
    assert!(world.try_pickup(user, cup, None, true));
    let table_at = world.map_coordinates(table).unwrap().position;
    let spot = table_at + Vec2::new(0.2, -0.1);
    assert!(click(&mut world, user, cup, table, spot));
    assert!(world.hand_holding(user, cup).is_none());
    let placed = world.map_coordinates(cup).unwrap().position;
    assert!((placed - spot).length() < 1e-4, "{placed:?}");
    // На плитку — по центру с отступом.
    assert!(world.try_pickup(user, cup, None, true));
    let plate_at = world.map_coordinates(plate).unwrap().position;
    assert!(click(&mut world, user, cup, plate, plate_at));
    let placed = world.map_coordinates(cup).unwrap().position;
    assert!((placed - (plate_at + Vec2::new(0.0, 0.25))).length() < 1e-4);
    // Закрытая поверхность не принимает.
    set_placeable(&mut world, table, false);
    assert!(world.try_pickup(user, cup, None, true));
    assert!(!click(&mut world, user, cup, table, table_at));
}

#[test]
fn placer_counts_items_up_to_its_limit() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let plate = spawn_at(&mut world, "Hotplate", grid, 4, 2);
    let cups = [
        spawn_at(&mut world, "Cup", grid, 4, 2),
        spawn_at(&mut world, "Cup", grid, 4, 2),
    ];
    let touch = |world: &mut World, other: Entity| {
        let mut event = StartCollide {
            our_entity: plate,
            other_entity: other,
            our_fixture: "itemcollide".into(),
            other_fixture: "fix1".into(),
        };
        world.raise(plate, &mut event);
    };
    touch(&mut world, cups[0]);
    // Предел 1 — поверхность закрылась, второй не считается.
    assert!(!world.get::<PlaceableSurface>(plate).unwrap().is_placeable);
    touch(&mut world, cups[1]);
    assert_eq!(
        world
            .get::<ItemPlacer>(plate)
            .unwrap()
            .placed_entities
            .len(),
        1
    );
    let mut end = EndCollide {
        our_entity: plate,
        other_entity: cups[0],
        our_fixture: "itemcollide".into(),
        other_fixture: "fix1".into(),
    };
    world.raise(plate, &mut end);
    assert!(
        world
            .get::<ItemPlacer>(plate)
            .unwrap()
            .placed_entities
            .is_empty()
    );
    assert!(world.get::<PlaceableSurface>(plate).unwrap().is_placeable);
}
