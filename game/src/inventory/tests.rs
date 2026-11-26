use astrocore_ecs::GameWorldExt;
use astrocore_map::{ContainerWorldExt, TransformWorldExt};

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: inventoryTemplate
  id: human
  slots:
  - name: shoes
    slotFlags: FEET
    uiWindowPos: 1,0
    strippingWindowPos: 1,3
    displayName: Shoes
  - name: jumpsuit
    slotFlags: INNERCLOTHING
    uiWindowPos: 0,1
    strippingWindowPos: 0,2
    displayName: Jumpsuit
  - name: pocket1
    slotFlags: POCKET
    slotGroup: MainHotbar
    dependsOn: jumpsuit
    uiWindowPos: 0,3
    strippingWindowPos: 0,4
    displayName: Pocket 1
- type: itemSize
  id: Small
  weight: 2
- type: itemSize
  id: Normal
  weight: 4
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: DoAfter
  - type: Inventory
  - type: Hands
    startingHands:
      hand_right:
        location: Right
      hand_left:
        location: Left
- type: entity
  id: Jumpsuit
  components:
  - type: Item
    size: Normal
  - type: Clothing
    slots: [innerclothing]
- type: entity
  id: SlowSuit
  parent: Jumpsuit
  components:
  - type: Clothing
    slots: [innerclothing]
    equipDelay: 1
- type: entity
  id: Shoes
  components:
  - type: Item
    size: Normal
  - type: Clothing
    slots:
    - FEET
- type: entity
  id: Pen
  components:
  - type: Item
    size: Tiny
- type: entity
  id: Toolbox
  components:
  - type: Item
    size: Normal
"#;

fn scene() -> (World, Entity, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    (world, grid, user)
}

#[test]
fn template_slots_become_containers_and_flags_parse() {
    let (world, _, user) = scene();
    let inventory = world.get::<Inventory>(user).unwrap();
    assert_eq!(inventory.slots.len(), 3);
    assert!(world.container(user, "pocket1").is_some());
    assert_eq!(inventory.slots[2].depends_on.as_deref(), Some("jumpsuit"));
    assert_eq!(
        inventory.slots[2].ui_window_pos,
        astrocore_math::IVec2::new(0, 3)
    );
    let parse = |yaml: &str| {
        astrocore_data::from_node::<SlotFlags>(&astrocore_data::parse_yaml(yaml).unwrap()).unwrap()
    };
    assert_eq!(parse("FEET"), SlotFlags(1 << 14));
    assert_eq!(
        parse("[innerclothing, feet]"),
        SlotFlags((1 << 6) | (1 << 14))
    );
    assert_eq!(parse("HEAD, EYES"), SlotFlags(0b110));
}

#[test]
fn equip_checks_flags_pockets_and_dependencies() {
    let (mut world, grid, user) = scene();
    let suit = spawn_at(&mut world, "Jumpsuit", grid, 2, 3);
    let shoes = spawn_at(&mut world, "Shoes", grid, 2, 3);
    let pen = spawn_at(&mut world, "Pen", grid, 2, 3);
    let toolbox = spawn_at(&mut world, "Toolbox", grid, 2, 3);
    world.run_tick();

    // Ботинки не надеваются как комбинезон; карман недоступен без комбинезона.
    assert!(
        world
            .can_equip(user, user, shoes, "jumpsuit", false)
            .is_err()
    );
    assert_eq!(
        world.can_equip(user, user, pen, "pocket1", false),
        Err("inventory-component-can-equip-cannot".into())
    );
    assert_eq!(
        world.try_equip(
            user,
            user,
            suit,
            "jumpsuit",
            EquipOptions {
                silent: true,
                force: false,
                delay: true
            }
        ),
        EquipResult::Done
    );
    assert_eq!(world.slot_entity(user, "jumpsuit"), Some(suit));
    assert_eq!(
        world.get::<Clothing>(suit).unwrap().in_slot.as_deref(),
        Some("jumpsuit")
    );
    // Мелочь в карман влезает, ящик с инструментами — нет.
    assert!(world.can_equip(user, user, pen, "pocket1", false).is_ok());
    assert_eq!(
        world.can_equip(user, user, toolbox, "pocket1", false),
        Err("inventory-component-can-equip-does-not-fit".into())
    );
    assert_eq!(
        world.try_equip(
            user,
            user,
            pen,
            "pocket1",
            EquipOptions {
                silent: true,
                force: false,
                delay: true
            }
        ),
        EquipResult::Done
    );
    assert_eq!(
        world.try_equip(
            user,
            user,
            shoes,
            "shoes",
            EquipOptions {
                silent: true,
                force: false,
                delay: true
            }
        ),
        EquipResult::Done
    );

    // Снятый комбинезон роняет то, что лежало в кармане.
    let (result, item) = world.try_unequip(
        user,
        user,
        "jumpsuit",
        EquipOptions {
            silent: true,
            force: false,
            delay: true,
        },
    );
    assert_eq!((result, item), (EquipResult::Done, Some(suit)));
    assert_eq!(world.slot_entity(user, "pocket1"), None);
    assert!(world.containing_container(pen).is_none());
    assert!(world.containing_container(suit).is_none());
}

#[test]
fn slot_clicks_equip_from_hand_and_unequip_into_hand() {
    let (mut world, grid, user) = scene();
    let suit = spawn_at(&mut world, "Jumpsuit", grid, 2, 3);
    world.run_tick();
    assert!(world.try_pickup(user, suit, None, true));
    use_slot(&mut world, user, "jumpsuit");
    assert_eq!(world.slot_entity(user, "jumpsuit"), Some(suit));
    assert_eq!(world.active_item(user), None);
    use_slot(&mut world, user, "jumpsuit");
    assert_eq!(world.slot_entity(user, "jumpsuit"), None);
    assert_eq!(world.active_item(user), Some(suit));
}

#[test]
fn quick_equip_and_delayed_equip() {
    let (mut world, grid, user) = scene();
    let shoes = spawn_at(&mut world, "Shoes", grid, 2, 3);
    let slow = spawn_at(&mut world, "SlowSuit", grid, 2, 3);
    world.run_tick();
    assert!(world.try_pickup(user, shoes, None, true));
    assert!(world.use_in_hand_interaction(user, shoes));
    assert_eq!(world.slot_entity(user, "shoes"), Some(shoes));

    // Задержка надевания: вещь в слоте только после действия.
    assert!(world.try_pickup(user, slow, None, true));
    assert!(world.use_in_hand_interaction(user, slow));
    assert_eq!(world.slot_entity(user, "jumpsuit"), None);
    for _ in 0..40 {
        world.run_tick();
    }
    assert_eq!(world.slot_entity(user, "jumpsuit"), Some(slow));
    let _ = world.map_coordinates(user);
}
