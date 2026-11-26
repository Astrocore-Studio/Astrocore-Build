use astrocore_ecs::{EventWorldExt, GameWorldExt};
use astrocore_map::{ContainerWorldExt, TransformWorldExt};
use astrocore_physics::Physics;
use bevy_ecs::resource::Resource;

use super::*;
use crate::testing::{spawn_at, station, world_with};
use crate::throwing::{Land, ThrowHitBy, ThrownItem};

const PROTOTYPES: &str = r#"
- type: entity
  id: Wall
  components:
  - type: Transform
    anchored: true
  - type: Physics
    bodyType: Static
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.5,-0.5,0.5,0.5"
        layer: [ WallLayer ]
        mask: [ FullTileMask ]
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
  - type: Physics
    bodyType: KinematicController
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeCircle
          radius: 0.35
        density: 185
        layer: [ MobLayer ]
        mask: [ MobMask ]
- type: entity
  id: BaseItem
  abstract: true
  components:
  - type: Item
  - type: Physics
    bodyType: Dynamic
    fixedRotation: false
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.25,-0.25,0.25,0.25"
        density: 20
        mask: [ ItemMask ]
        restitution: 0.3
        friction: 0.2
- type: entity
  id: Crowbar
  parent: BaseItem
- type: entity
  id: Toolbox
  parent: BaseItem
  components:
  - type: Item
    size: Normal
- type: entity
  id: Console
  components:
  - type: Transform
    anchored: true
  - type: Physics
    bodyType: Static
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeAabb
          bounds: "-0.45,-0.45,0.45,0.45"
        layer: [ MachineLayer ]
        mask: [ MachineMask ]
"#;

#[derive(Resource, Default)]
struct Log(Vec<String>);

fn scene() -> (World, Entity, Entity) {
    let mut world = world_with(PROTOTYPES);
    world.init_resource::<Log>();
    let grid = station(&mut world);
    let human = spawn_at(&mut world, "Human", grid, 2, 2);
    (world, grid, human)
}

fn at(world: &World, entity: Entity) -> MapCoordinates {
    world.map_coordinates(entity).unwrap()
}

#[test]
fn starting_hands_are_added_sorted_and_the_first_is_active() {
    let (world, _, human) = scene();
    let hands = world.get::<Hands>(human).unwrap();
    assert_eq!(hands.sorted_hands, ["hand_right", "hand_left"]);
    assert_eq!(hands.active_hand_id.as_deref(), Some("hand_right"));
    assert!(world.container(human, "hand_left").is_some());
    assert_eq!(world.hands_in_order(human), ["hand_right", "hand_left"]);
}

#[test]
fn clicking_an_item_picks_it_up_then_swap_and_drop() {
    let (mut world, grid, human) = scene();
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 3, 2);
    let toolbox = spawn_at(&mut world, "Toolbox", grid, 2, 3);
    world.subscribe::<Item, GotEquippedHand>(|world, _, event| {
        world
            .resource_mut::<Log>()
            .0
            .push(format!("equipped {}", event.hand));
    });
    world.subscribe::<Item, Dropped>(|world, _, _| {
        world.resource_mut::<Log>().0.push("dropped".into());
    });
    world.run_tick();

    world.user_interaction(human, at(&world, crowbar), Some(crowbar), false);
    assert_eq!(world.active_item(human), Some(crowbar));
    assert_eq!(world.used_entity(human), Some(crowbar));
    // Активная рука занята: клик по другому предмету — `InteractUsing`, а не подбор.
    world.user_interaction(human, at(&world, toolbox), Some(toolbox), false);
    assert_eq!(world.held_item(human, "hand_left"), None);
    world.swap_hands(human, false);
    assert_eq!(world.active_hand(human).as_deref(), Some("hand_left"));
    world.user_interaction(human, at(&world, toolbox), Some(toolbox), false);
    assert_eq!(world.held_item(human, "hand_left"), Some(toolbox));
    assert_eq!(world.held_items(human), [toolbox, crowbar]);
    assert_eq!(world.empty_hand(human), None);

    // Выброс к точке за пределами досягаемости ограничен 1,5 тайла.
    let far = MapCoordinates {
        position: at(&world, human).position + Vec2::new(5.0, 0.0),
        ..at(&world, human)
    };
    assert!(world.try_drop(human, "hand_left", Some(far), true));
    let dropped = at(&world, toolbox).position - at(&world, human).position;
    assert!(
        (dropped.length() - INTERACTION_RANGE).abs() < 1e-3,
        "{dropped:?}"
    );
    assert_eq!(world.held_item(human, "hand_left"), None);
    assert_eq!(
        world.resource::<Log>().0,
        ["equipped hand_right", "equipped hand_left", "dropped"]
    );
}

#[test]
fn drop_stops_at_walls_and_static_or_non_items_are_not_picked_up() {
    let (mut world, grid, human) = scene();
    spawn_at(&mut world, "Wall", grid, 3, 2);
    let console = spawn_at(&mut world, "Console", grid, 2, 3);
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 1, 2);
    world.run_tick();
    assert!(!world.try_pickup(human, console, None, true));
    assert!(world.try_pickup(human, crowbar, None, true));
    let behind_wall = MapCoordinates {
        position: Vec2::new(4.5, 2.5),
        ..at(&world, human)
    };
    assert!(world.try_drop(human, "hand_right", Some(behind_wall), true));
    // Стена начинается с x = 3: предмет остаётся перед ней.
    assert!(
        at(&world, crowbar).position.x <= 3.0,
        "{:?}",
        at(&world, crowbar)
    );

    // Анкерованный предмет не поднять.
    world.get_mut::<Physics>(crowbar).unwrap().body_type = astrocore_physics::BodyType::Static;
    assert!(!world.try_pickup(human, crowbar, None, true));
}

#[test]
fn hand_whitelists_and_moving_items_between_hands() {
    let (mut world, grid, human) = scene();
    world
        .get_mut::<Hands>(human)
        .unwrap()
        .hands
        .get_mut("hand_left")
        .unwrap()
        .whitelist = Some(EntityWhitelist {
        sizes: Some(vec!["Normal".into()]),
        ..EntityWhitelist::default()
    });
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 3, 2);
    assert!(!world.try_pickup(human, crowbar, Some("hand_left"), true));
    assert!(world.try_pickup(human, crowbar, Some("hand_right"), true));
    // Перекладывание в пустую активную руку проверяет её правила.
    world.set_active_hand(human, Some("hand_left"));
    assert!(!move_to_active(&mut world, human, "hand_right"));
    assert_eq!(world.held_item(human, "hand_right"), Some(crowbar));
    world
        .get_mut::<Hands>(human)
        .unwrap()
        .hands
        .get_mut("hand_left")
        .unwrap()
        .whitelist = None;
    assert!(move_to_active(&mut world, human, "hand_right"));
    assert_eq!(world.held_item(human, "hand_left"), Some(crowbar));

    // Убранная рука роняет предмет, активной становится оставшаяся.
    world.remove_hand(human, "hand_left");
    assert!(world.containing_container(crowbar).is_none());
    assert_eq!(world.active_hand(human).as_deref(), Some("hand_right"));
}

#[test]
fn thrown_items_fly_hit_land_and_slow_down_on_the_floor() {
    let (mut world, grid, human) = scene();
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 2, 3);
    // Сосед в двух тайлах: предмет задевает его, пока летит (полёт — 80% пути, остальное
    // он скользит по полу и уже никого не задевает, как в SS14).
    let target = spawn_at(&mut world, "Human", grid, 4, 2);
    world.subscribe::<Hands, ThrowHitBy>(|world, _, _| {
        world.resource_mut::<Log>().0.push("hit".into());
    });
    world.subscribe::<Item, Land>(|world, _, _| {
        world.resource_mut::<Log>().0.push("land".into());
    });
    world.run_tick();
    assert!(world.try_pickup(human, crowbar, None, true));

    // Бросок в соседа.
    let aim = at(&world, target);
    assert!(world.throw_held_item(human, aim));
    assert!(world.get::<ThrownItem>(crowbar).is_some());
    // Повторный бросок раньше паузы не проходит.
    assert!(!world.throw_held_item(human, aim));
    for _ in 0..90 {
        world.run_tick();
    }
    assert!(
        world.resource::<Log>().0.contains(&"hit".to_owned()),
        "{:?}",
        world.resource::<Log>().0
    );
    assert!(world.resource::<Log>().0.contains(&"land".to_owned()));
    assert!(world.get::<ThrownItem>(crowbar).is_none());
    // Приземлившись, предмет остановился трением о пол.
    let body = world.get::<Physics>(crowbar).unwrap();
    assert!(
        body.linear_velocity.length() < 0.05,
        "{:?}",
        body.linear_velocity
    );
    assert!(!body.awake || body.linear_velocity.length() < 0.05);
}

#[test]
fn thrown_items_land_near_the_aim_point() {
    let (mut world, grid, human) = scene();
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 2, 3);
    world.run_tick();
    assert!(world.try_pickup(human, crowbar, None, true));
    let aim = MapCoordinates {
        position: at(&world, human).position + Vec2::new(0.0, 6.0),
        ..at(&world, human)
    };
    assert!(world.throw_held_item(human, aim));
    for _ in 0..120 {
        world.run_tick();
    }
    let landed = at(&world, crowbar).position;
    assert!(
        landed.distance(aim.position) < 1.0,
        "упал в {landed:?}, цель {:?}",
        aim.position
    );
}
