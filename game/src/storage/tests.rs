use astrocore_ecs::GameWorldExt;
use astrocore_map::{ContainerWorldExt, TransformWorldExt};
use astrocore_net::Actor;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: itemSize
  id: Tiny
  weight: 1
  defaultShape:
  - 0,0,0,0
- type: itemSize
  id: Small
  weight: 2
  defaultShape:
  - 0,0,0,1
- type: itemSize
  id: Normal
  weight: 4
  defaultShape:
  - 0,0,1,1
- type: itemSize
  id: Large
  weight: 8
  defaultShape:
  - 0,0,3,1
- type: itemSize
  id: Huge
  weight: 16
  defaultShape:
  - 0,0,3,3
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: Hands
    startingHands:
      hand_right:
        location: Right
      hand_left:
        location: Left
- type: entity
  id: Backpack
  components:
  - type: Item
    size: Huge
  - type: Storage
    grid:
    - 0,0,3,1
  - type: UserInterface
    interfaces:
      enum.StorageUiKey.Key:
        type: StorageBoundUserInterface
- type: entity
  id: Pouch
  components:
  - type: Item
    size: Small
  - type: Storage
    grid:
    - 0,0,1,1
- type: entity
  id: Tray
  components:
  - type: Storage
    maxItemSize: Small
    grid:
    - 0,0,1,0
- type: entity
  id: Pen
  components:
  - type: Item
    size: Tiny
- type: entity
  id: Crowbar
  components:
  - type: Item
    size: Small
- type: entity
  id: Toolbox
  components:
  - type: Item
    size: Normal
- type: entity
  id: Rifle
  components:
  - type: Item
    size: Large
- type: entity
  id: Bent
  components:
  - type: Item
    size: Normal
    shape:
    - 0,0,1,0
    - 0,1,0,1
"#;

fn scene() -> (World, Entity, Entity, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let session = world.spawn_empty().id();
    world.entity_mut(user).insert(Actor { session });
    let backpack = spawn_at(&mut world, "Backpack", grid, 3, 2);
    world.run_tick();
    (world, grid, user, backpack)
}

fn cells(boxes: &[CellBox]) -> Vec<(i32, i32)> {
    let mut all: Vec<_> = boxes
        .iter()
        .flat_map(CellBox::cells)
        .map(|c| (c.x, c.y))
        .collect();
    all.sort();
    all
}

#[test]
fn boxes_parse_and_shapes_rotate_about_their_center() {
    let parsed: Vec<CellBox> = astrocore_data::from_node(
        &astrocore_data::parse_yaml("[\"0,0,3,1\", \"1, 2, 1, 2\"]").unwrap(),
    )
    .unwrap();
    assert_eq!(
        parsed,
        vec![CellBox::new(0, 0, 3, 1), CellBox::new(1, 2, 1, 2)]
    );

    // Лежачий 4×2 предмет, повёрнутый на четверть оборота, встаёт 2×4 от той же клетки.
    let large = [CellBox::new(0, 0, 3, 1)];
    let turned = adjusted_shape(&large, Angle::degrees(90.0), IVec2::new(5, 1));
    assert_eq!(turned, vec![CellBox::new(5, 1, 6, 4)]);
    assert_eq!(
        adjusted_shape(&large, Angle::ZERO, IVec2::new(1, 0)),
        vec![CellBox::new(1, 0, 4, 1)]
    );

    // Уголок: после поворота клетки остаются в ограничивающем квадрате 2×2.
    let bent = [CellBox::new(0, 0, 1, 0), CellBox::new(0, 1, 0, 1)];
    let turned = cells(&adjusted_shape(&bent, Angle::degrees(90.0), IVec2::ZERO));
    assert_eq!(turned.len(), 3);
    assert!(
        turned
            .iter()
            .all(|(x, y)| (0..=1).contains(x) && (0..=1).contains(y))
    );
    assert_ne!(turned, cells(&bent));
}

#[test]
fn grid_fills_bottom_left_first_and_rotates_to_fit() {
    let (mut world, grid, _, backpack) = scene();
    assert_eq!(max_item_size(&world, backpack), "Large");

    let toolbox = spawn_at(&mut world, "Toolbox", grid, 3, 2);
    assert!(world.storage_insert(backpack, toolbox, None, false));
    let location = world.get::<Storage>(backpack).unwrap().stored_items[&toolbox];
    assert_eq!(location.position, IVec2::ZERO);

    let second = spawn_at(&mut world, "Toolbox", grid, 3, 2);
    assert!(world.storage_insert(backpack, second, None, false));
    assert_eq!(
        world.get::<Storage>(backpack).unwrap().stored_items[&second].position,
        IVec2::new(2, 0)
    );

    // Места больше нет: 4×2 сетка занята двумя 2×2.
    let pen = spawn_at(&mut world, "Pen", grid, 3, 2);
    assert_eq!(
        world.can_insert_storage(backpack, pen, false),
        Err(Some("comp-storage-insufficient-capacity".into()))
    );
    // Вынутый предмет освобождает клетки.
    world.remove_from_container(toolbox, None).unwrap();
    assert!(
        !world
            .get::<Storage>(backpack)
            .unwrap()
            .stored_items
            .contains_key(&toolbox)
    );
    assert!(world.storage_insert(backpack, pen, None, false));
    assert_eq!(
        world.get::<Storage>(backpack).unwrap().stored_items[&pen].position,
        IVec2::ZERO
    );

    // В сетку 2×2 мешочка лежачий 1×2 предмет помещается как есть, а размер Normal уже нет.
    let pouch = spawn_at(&mut world, "Pouch", grid, 3, 2);
    assert_eq!(max_item_size(&world, pouch), "Tiny");
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 3, 2);
    assert_eq!(
        world.can_insert_storage(pouch, crowbar, false),
        Err(Some("comp-storage-too-big".into()))
    );
}

#[test]
fn tall_item_turns_when_it_does_not_fit_upright() {
    let (mut world, grid, _, backpack) = scene();
    // В сетке 4×2 стоячий 1×2 лом занимает столбец.
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 3, 2);
    assert!(world.storage_insert(backpack, crowbar, None, false));
    let rifle = spawn_at(&mut world, "Rifle", grid, 3, 2);
    // Винтовке 4×2 места нет ни лёжа, ни стоя.
    assert!(available_space(&world, backpack, rifle).is_none());
    world.remove_from_container(crowbar, None).unwrap();
    assert_eq!(
        available_space(&world, backpack, rifle),
        Some(ItemStorageLocation::new(Angle::ZERO, IVec2::ZERO))
    );
    // Своё место не мешает переложить предмет.
    assert!(world.storage_insert(backpack, rifle, None, false));
    let here = world.get::<Storage>(backpack).unwrap().stored_items[&rifle];
    assert!(item_fits(&world, backpack, rifle, here));
    assert!(!world.set_item_location(
        backpack,
        rifle,
        ItemStorageLocation::new(Angle::ZERO, IVec2::new(1, 0))
    ));

    // В лоток 2×1 стоячий лом ложится только повёрнутым.
    let tray = spawn_at(&mut world, "Tray", grid, 3, 2);
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 3, 2);
    let location = available_space(&world, tray, crowbar).unwrap();
    assert_eq!(location.position, IVec2::ZERO);
    assert!((location.rotation.0 - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    assert!(world.storage_insert(tray, crowbar, None, false));
}

#[test]
fn clicking_with_held_item_puts_it_inside_and_open_storage_grants_access() {
    let (mut world, grid, user, backpack) = scene();
    let pen = spawn_at(&mut world, "Pen", grid, 2, 2);
    world.run_tick();
    assert!(world.try_pickup_any_hand(user, pen, true));
    let click = world.map_coordinates(backpack).unwrap();
    assert!(world.interact_using(user, pen, backpack, click));
    assert!(
        world
            .container(backpack, STORAGE_CONTAINER)
            .unwrap()
            .contains(pen)
    );
    assert_eq!(world.active_item(user), None);

    // Закрытый рюкзак не даёт достать предмет; открытый — даёт.
    assert!(!world.is_accessible(user, pen));
    let activate = world.interaction_activate(user, backpack, true);
    assert!(activate);
    assert!(world.ui_open_for(backpack, STORAGE_UI_KEY, user));
    assert!(world.is_accessible(user, pen));

    // Клик по предмету в окне пустой рукой берёт его.
    let net = *world.get::<NetEntity>(pen).unwrap();
    let mut message = BoundUiMessage {
        entity: backpack,
        key: STORAGE_UI_KEY.to_owned(),
        actor: user,
        message: StorageMessage::InteractWithItem(net),
    };
    world.raise(backpack, &mut message);
    assert_eq!(world.active_item(user), Some(pen));
    assert!(
        world
            .get::<Storage>(backpack)
            .unwrap()
            .stored_items
            .is_empty()
    );

    // Повторное «использовать» закрывает окно.
    world.interaction_activate(user, backpack, true);
    assert!(!world.ui_open_for(backpack, STORAGE_UI_KEY, user));
}

#[test]
fn too_big_item_stays_in_hand_with_a_reason() {
    let (mut world, grid, user, _) = scene();
    let pouch = spawn_at(&mut world, "Pouch", grid, 3, 2);
    let toolbox = spawn_at(&mut world, "Toolbox", grid, 2, 2);
    assert!(world.try_pickup_any_hand(user, toolbox, true));
    assert!(!world.player_insert_held(pouch, user));
    assert_eq!(world.active_item(user), Some(toolbox));
    // Хранилище не кладётся в хранилище того же или большего размера.
    let other = spawn_at(&mut world, "Pouch", grid, 3, 2);
    assert_eq!(
        world.can_insert_storage(pouch, other, false),
        Err(Some("comp-storage-too-big".into()))
    );
}

#[test]
fn bent_shape_takes_three_cells() {
    let (mut world, grid, _, backpack) = scene();
    let bent = spawn_at(&mut world, "Bent", grid, 3, 2);
    assert_eq!(item_shape(&world, bent).len(), 2);
    assert!(world.storage_insert(backpack, bent, None, false));
    let component = world.get::<Storage>(backpack).unwrap();
    let taken = occupied(&world, component, None);
    assert_eq!(taken.len(), 3);
    assert!(!taken.contains(&IVec2::new(1, 1)));
    // Свободная клетка в изгибе подходит крошечному предмету.
    let pen = spawn_at(&mut world, "Pen", grid, 3, 2);
    assert!(item_fits(
        &world,
        backpack,
        pen,
        ItemStorageLocation::new(Angle::ZERO, IVec2::new(1, 1))
    ));
}

#[test]
fn items_put_straight_into_the_container_get_a_place() {
    let (mut world, grid, _, backpack) = scene();
    let pen = spawn_at(&mut world, "Pen", grid, 3, 2);
    let expected = available_space(&world, backpack, pen);
    assert!(expected.is_some());
    world
        .force_insert_into_container(pen, backpack, STORAGE_CONTAINER)
        .unwrap();
    let location = world
        .get::<Storage>(backpack)
        .unwrap()
        .stored_items
        .get(&pen)
        .copied();
    assert_eq!(location, expected);
}
