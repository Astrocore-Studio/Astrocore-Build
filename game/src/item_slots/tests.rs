use astrocore_ecs::MetaData;
use astrocore_map::TransformWorldExt;
use astrocore_math::Vec2;

use super::*;
use crate::hands::HandsWorldExt;
use crate::interaction::InteractionWorldExt;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  id: Human
  components:
  - type: Hands
  - type: ComplexInteraction
  - type: Physics
    bodyType: KinematicController
- type: entity
  id: FuelJar
  components:
  - type: Item
  - type: Tag
    tags: [ Fuel ]
- type: entity
  id: Rock
  components:
  - type: Item
- type: entity
  id: Controller
  components:
  - type: ItemSlots
    slots:
      fuel:
        name: fuel slot
        whitelist:
          tags: [ Fuel ]
        whitelistFailPopup: wrong item
- type: entity
  id: Stocked
  components:
  - type: ItemSlots
    slots:
      fuel:
        startingItem: FuelJar
"#;

fn human(world: &mut World, grid: Entity) -> Entity {
    let user = spawn_at(world, "Human", grid, 1, 1);
    world.add_hand(user, "right", crate::hands::Hand::default());
    user
}

#[test]
fn fuel_goes_in_by_hand_and_comes_back_out() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = human(&mut world, grid);
    let controller = spawn_at(&mut world, "Controller", grid, 2, 1);
    let rock = spawn_at(&mut world, "Rock", grid, 1, 1);
    assert!(world.try_pickup(user, rock, None, false));
    // Камень не по белому списку.
    let at = world.map_coordinates(controller).unwrap();
    world.interact_using(user, rock, controller, at);
    assert!(slot_item(&world, controller, "fuel").is_none());
    world.try_drop(user, "right", None, false);

    let jar = spawn_at(&mut world, "FuelJar", grid, 1, 1);
    assert!(world.try_pickup(user, jar, None, false));
    world.interact_using(user, jar, controller, at);
    assert_eq!(slot_item(&world, controller, "fuel"), Some(jar));
    assert!(world.active_item(user).is_none());

    // Запертый слот не отдаёт предмет.
    set_lock(&mut world, controller, "fuel", true);
    assert!(!try_eject_to_hands(
        &mut world,
        controller,
        "fuel",
        Some(user)
    ));
    set_lock(&mut world, controller, "fuel", false);
    assert!(try_eject_to_hands(
        &mut world,
        controller,
        "fuel",
        Some(user)
    ));
    assert_eq!(world.active_item(user), Some(jar));
    let _ = Vec2::ZERO;
}

#[test]
fn starting_items_appear_on_map_init() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let stocked = spawn_at(&mut world, "Stocked", grid, 2, 1);
    let item = slot_item(&world, stocked, "fuel").unwrap();
    assert_eq!(
        world.get::<MetaData>(item).unwrap().prototype.as_deref(),
        Some("FuelJar")
    );
}
