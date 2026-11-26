use astrocore_ecs::{EventWorldExt, GameWorldExt};
use astrocore_map::{EntityCoordinates, TransformWorldExt};
use astrocore_math::Vec2;
use bevy_ecs::resource::Resource;

use super::*;
use crate::testing::{spawn_at, station, world_with};

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
  - type: Physics
    bodyType: KinematicController
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeCircle
          radius: 0.35
        layer: [ MobLayer ]
        mask: [ MobMask ]
- type: entity
  id: Corgi
  parent: Human
  components:
  - type: Tag
    tags: [ Dog ]
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
- type: entity
  id: Intercom
  components:
  - type: Transform
    anchored: true
  - type: WallMount
- type: entity
  id: Crowbar
  components:
  - type: Item
  - type: Physics
  - type: Fixtures
    fixtures:
      fix1:
        shape: !type:PhysShapeCircle
          radius: 0.2
        layer: [ ItemMask ]
"#;

#[derive(Resource, Default)]
struct Log(Vec<String>);

fn log(world: &mut World, line: String) {
    world.resource_mut::<Log>().0.push(line);
}

fn take(world: &mut World) -> Vec<String> {
    std::mem::take(&mut world.resource_mut::<Log>().0)
}

/// Предмет в руке для тестов: руки появятся в задаче 3.2.
#[derive(Resource, Default)]
struct Held(Option<Entity>);

fn scene() -> (World, Entity) {
    let mut world = world_with(PROTOTYPES);
    world.init_resource::<Log>();
    world.init_resource::<Held>();
    world.subscribe::<ComplexInteraction, GetUsedEntity>(|world, _, event| {
        event.used = world.resource::<Held>().0;
    });
    world.subscribe::<Fixtures, InteractHand>(|world, _, event| {
        log(world, format!("hand {:?}", event.target));
    });
    world.subscribe::<Fixtures, ActivateInWorld>(|world, _, event| {
        log(world, format!("activate complex={}", event.complex));
        event.handled = true;
    });
    world.subscribe::<Fixtures, InteractUsing>(|world, _, event| {
        log(world, format!("using {:?}", event.used));
    });
    world.subscribe::<Item, AfterInteract>(|world, _, event| {
        log(
            world,
            format!(
                "after reach={} target={}",
                event.can_reach,
                event.target.is_some()
            ),
        );
    });
    world.subscribe::<Item, UseInHand>(|world, _, event| {
        log(world, "use in hand".into());
        event.handled = true;
    });
    world.subscribe::<WallMount, ActivateInWorld>(|world, _, event| {
        log(world, "intercom".into());
        event.handled = true;
    });
    let grid = station(&mut world);
    (world, grid)
}

use crate::action_blocker::ComplexInteraction;

fn at(world: &World, entity: Entity) -> MapCoordinates {
    world.map_coordinates(entity).unwrap()
}

#[test]
fn empty_hand_click_interacts_then_activates_in_range_only() {
    let (mut world, grid) = scene();
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let near = spawn_at(&mut world, "Console", grid, 3, 2);
    let far = spawn_at(&mut world, "Console", grid, 6, 2);
    world.run_tick();

    let click = at(&world, near);
    world.user_interaction(user, click, Some(near), false);
    assert_eq!(
        take(&mut world),
        [format!("hand {near:?}"), "activate complex=true".into()]
    );
    let click = at(&world, far);
    world.user_interaction(user, click, Some(far), false);
    assert_eq!(take(&mut world), Vec::<String>::new());

    // Без сложных действий клик рукой сразу «использует».
    world.entity_mut(user).remove::<ComplexInteraction>();
    world.user_interaction(user, at(&world, near), Some(near), false);
    assert_eq!(take(&mut world), ["activate complex=false"]);

    // Тег снимает проверку дальности.
    world.add_tag(user, BYPASS_RANGE_TAG);
    world.user_interaction(user, at(&world, far), Some(far), false);
    assert_eq!(take(&mut world), ["activate complex=false"]);
}

#[test]
fn range_is_measured_edge_to_edge_and_walls_block() {
    let (mut world, grid) = scene();
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    // Пульт в двух тайлах: от края тела до края пульта 2 - 0.35 - 0.45 = 1.2 < 1.5.
    let console = spawn_at(&mut world, "Console", grid, 4, 2);
    // Центр к центру — 2 тайла: без фикстур у цели это уже далеко.
    let intercom_far = spawn_at(&mut world, "Intercom", grid, 4, 4);
    world.run_tick();
    assert!(world.in_range_unobstructed(user, console, INTERACTION_RANGE));
    assert!(!world.in_range_unobstructed(user, intercom_far, INTERACTION_RANGE));

    // Стена между игроком и пультом закрывает его.
    let _wall = spawn_at(&mut world, "Wall", grid, 3, 2);
    world.run_tick();
    assert!(!world.in_range_unobstructed(user, console, INTERACTION_RANGE));
    assert!(!world.interaction_activate(user, console, true));
    assert!(world.interaction_activate(user, console, false));
    take(&mut world);
}

#[test]
fn wall_mounted_entities_are_reached_through_their_wall_from_the_front() {
    let (mut world, grid) = scene();
    // Интерком висит на стене в (3, 3) и смотрит на юг: к нему подходят снизу.
    spawn_at(&mut world, "Wall", grid, 3, 3);
    let intercom = spawn_at(&mut world, "Intercom", grid, 3, 3);
    let front = spawn_at(&mut world, "Human", grid, 3, 2);
    let behind = spawn_at(&mut world, "Human", grid, 3, 4);
    world.run_tick();
    assert!(world.interaction_activate(front, intercom, true));
    assert_eq!(take(&mut world), ["intercom"]);
    assert!(!world.interaction_activate(behind, intercom, true));
    // Дуга во весь оборот пускает с любой стороны.
    world.get_mut::<WallMount>(intercom).unwrap().arc = Angle(std::f32::consts::TAU);
    assert!(world.interaction_activate(behind, intercom, true));
}

#[test]
fn held_item_clicks_use_interact_using_after_interact_and_use_in_hand() {
    let (mut world, grid) = scene();
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let console = spawn_at(&mut world, "Console", grid, 3, 2);
    let far = spawn_at(&mut world, "Console", grid, 9, 2);
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 2, 3);
    world.run_tick();
    world.resource_mut::<Held>().0 = Some(crowbar);

    world.user_interaction(user, at(&world, console), Some(console), false);
    assert_eq!(
        take(&mut world),
        [
            format!("using {crowbar:?}"),
            "after reach=true target=true".into()
        ]
    );
    world.user_interaction(user, at(&world, far), Some(far), false);
    assert_eq!(take(&mut world), ["after reach=false target=true"]);
    let floor = MapCoordinates {
        position: Vec2::new(2.5, 3.0),
        ..at(&world, user)
    };
    world.user_interaction(user, floor, None, false);
    assert_eq!(take(&mut world), ["after reach=true target=false"]);
    world.user_interaction(user, at(&world, crowbar), Some(crowbar), false);
    assert_eq!(take(&mut world), ["use in hand"]);
}

#[test]
fn items_inside_other_containers_are_not_accessible() {
    use astrocore_map::{ContainerKind, ContainerWorldExt};
    let (mut world, grid) = scene();
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let locker = spawn_at(&mut world, "Console", grid, 3, 2);
    let crowbar = spawn_at(&mut world, "Crowbar", grid, 2, 3);
    world.ensure_container(locker, "storage", ContainerKind::List);
    world
        .insert_into_container(crowbar, locker, "storage")
        .unwrap();
    assert!(!world.is_accessible(user, crowbar));
    // Что держит сам игрок — доступно.
    world.ensure_container(user, "hand", ContainerKind::Slot);
    world.insert_into_container(crowbar, user, "hand").unwrap();
    assert!(world.is_accessible(user, crowbar));
    // Владелец контейнера может открыть доступ (открытый рюкзак).
    world
        .insert_into_container(crowbar, locker, "storage")
        .unwrap();
    world.subscribe::<Fixtures, AccessViaContainer>(|_, _, event| event.accessible = true);
    assert!(world.is_accessible(user, crowbar));
    let _ = EntityCoordinates::new(grid, Vec2::ZERO);
}

#[test]
fn clicking_turns_a_standing_user_to_face_the_point() {
    let (mut world, grid) = scene();
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let east = MapCoordinates {
        position: Vec2::new(5.0, 2.5),
        ..at(&world, user)
    };
    world.user_interaction(user, east, None, false);
    let facing = world.world_transform(user).unwrap().rotation;
    assert_eq!(facing.direction(), astrocore_math::Direction::East);
}
