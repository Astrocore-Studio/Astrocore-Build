use astrocore_ecs::LifecycleWorldExt;
use astrocore_net::NetEvent;

use super::*;
use crate::ghost::on_ghost_attempt;
use crate::mind::tests::{PROTOTYPES as MIND_PROTOTYPES, Scene, scene};
use crate::mobs::MobStateWorldExt;
use crate::testing::spawn_at;

const EXTRA: &str = r#"
- type: ghostRoleRaffleSettings
  id: short
  settings:
    initialDuration: 10
    joinExtendsDurationBy: 2
    maxDuration: 13
- type: entity
  id: Mouse
  name: mouse
  components:
  - type: MobState
  - type: MobThresholds
    thresholds:
      0: Alive
      100: Dead
  - type: GhostRole
    name: mouse-role
    description: mouse-description
    allowMovement: true
  - type: GhostTakeoverAvailable
- type: entity
  id: RaffledMouse
  parent: Mouse
  components:
  - type: GhostRole
    name: raffled-mouse
    raffle:
      settings: short
- type: entity
  id: MouseSpawner
  components:
  - type: GhostRole
    name: spawner-role
  - type: GhostRoleMobSpawner
    prototype: MobObserverless
    availableTakeovers: 2
- type: entity
  id: MobObserverless
  name: drone
  components:
  - type: MobState
- type: entity
  id: RareMouse
  parent: Mouse
  components:
  - type: GhostRole
    prob: 0
"#;

fn prototypes() -> String {
    format!("{MIND_PROTOTYPES}{EXTRA}")
}

/// Игрок-призрак, бросивший живое тело.
fn ghost_player(scene: &mut Scene, n: u8) -> (Entity, Entity) {
    let (session, _, _, mind_id) = scene.player(n);
    on_ghost_attempt(&mut scene.world, mind_id, true, false, false);
    (session, scene.attached(session).unwrap())
}

fn id_of(world: &World, entity: Entity) -> u32 {
    world.get::<GhostRole>(entity).unwrap().identifier
}

#[test]
fn ghosts_take_free_bodies() {
    let mut scene = scene(&prototypes());
    let mouse = spawn_at(&mut scene.world, "Mouse", scene.grid, 5, 5);
    assert_eq!(ghost_role_count(&mut scene.world), 1);
    let (session, ghost) = ghost_player(&mut scene, 1);
    let info = ghost_roles_info(&mut scene.world, Some(session));
    assert_eq!(info[0].kind, GhostRoleKind::FirstComeFirstServe);
    assert_eq!(info[0].name, "mouse-role");
    let id = id_of(&scene.world, mouse);
    scene.world.raise_broadcast(&mut NetEvent {
        session: Some(session),
        event: GhostRolesMessage::Request(id),
    });
    scene.world.flush_deletions();
    // Игрок в мыши, призрак удалён, роли больше нет.
    assert_eq!(scene.attached(session), Some(mouse));
    assert!(!scene.world.is_alive(ghost));
    assert_eq!(ghost_role_count(&mut scene.world), 0);
    assert!(
        scene
            .world
            .get::<astrocore_movement::InputMover>(mouse)
            .is_some()
    );
    let mind_id = mind::mind_of(&scene.world, mouse).unwrap();
    assert_eq!(
        scene
            .world
            .get::<mind::Mind>(mind_id)
            .unwrap()
            .character_name
            .as_deref(),
        Some("mouse")
    );
    // Второй призрак опоздал.
    let (other, _) = ghost_player(&mut scene, 2);
    assert!(!takeover(&mut scene.world, other, id));
}

#[test]
fn dead_bodies_are_not_offered_and_unlikely_roles_vanish() {
    let mut scene = scene(&prototypes());
    let mouse = spawn_at(&mut scene.world, "Mouse", scene.grid, 5, 5);
    scene.world.change_mob_state(mouse, MobState::Dead, None);
    assert_eq!(ghost_role_count(&mut scene.world), 0);
    scene.world.change_mob_state(mouse, MobState::Alive, None);
    assert_eq!(ghost_role_count(&mut scene.world), 1);
    let rare = spawn_at(&mut scene.world, "RareMouse", scene.grid, 6, 5);
    assert!(scene.world.get::<GhostRole>(rare).is_none());
    assert_eq!(ghost_role_count(&mut scene.world), 1);
}

#[test]
fn raffles_extend_up_to_the_limit_and_pick_one_winner() {
    let mut scene = scene(&prototypes());
    scene
        .world
        .insert_resource(crate::random::GameRandom::seeded(5));
    let mouse = spawn_at(&mut scene.world, "RaffledMouse", scene.grid, 5, 5);
    let id = id_of(&scene.world, mouse);
    let (a, _) = ghost_player(&mut scene, 1);
    let (b, _) = ghost_player(&mut scene, 2);
    let (c, _) = ghost_player(&mut scene, 3);
    assert_eq!(
        ghost_roles_info(&mut scene.world, Some(a))[0].kind,
        GhostRoleKind::RaffleReady
    );
    request(&mut scene.world, a, id);
    assert_eq!(
        scene.world.get::<GhostRoleRaffle>(mouse).unwrap().countdown,
        10.0
    );
    request(&mut scene.world, b, id);
    // Второй продлил до 12, третий уже не может: предел 13.
    request(&mut scene.world, c, id);
    let raffle = scene.world.get::<GhostRoleRaffle>(mouse).unwrap().clone();
    assert_eq!(raffle.countdown, 12.0);
    assert_eq!(raffle.current_members.len(), 3);
    assert_eq!(
        ghost_roles_info(&mut scene.world, Some(a))[0].kind,
        GhostRoleKind::RaffleJoined
    );
    leave_raffle(&mut scene.world, c, id);
    update_raffles(&mut scene.world, 11.0);
    assert!(scene.world.get::<GhostRoleRaffle>(mouse).is_some());
    update_raffles(&mut scene.world, 1.5);
    scene.world.flush_deletions();
    let winners: Vec<Entity> = [a, b]
        .into_iter()
        .filter(|s| scene.attached(*s) == Some(mouse))
        .collect();
    assert_eq!(winners.len(), 1);
    assert_ne!(scene.attached(c), Some(mouse));
    assert!(scene.world.get::<GhostRoleRaffle>(mouse).is_none());
    assert_eq!(ghost_role_count(&mut scene.world), 0);
}

#[test]
fn empty_raffles_are_cancelled() {
    let mut scene = scene(&prototypes());
    let mouse = spawn_at(&mut scene.world, "RaffledMouse", scene.grid, 5, 5);
    let id = id_of(&scene.world, mouse);
    let (a, _) = ghost_player(&mut scene, 1);
    request(&mut scene.world, a, id);
    leave_all_raffles(&mut scene.world, a);
    update_raffles(&mut scene.world, 0.1);
    assert!(scene.world.get::<GhostRoleRaffle>(mouse).is_none());
    assert_eq!(ghost_role_count(&mut scene.world), 1);
}

#[test]
fn spawners_make_creatures_until_used_up() {
    let mut scene = scene(&prototypes());
    let spawner = spawn_at(&mut scene.world, "MouseSpawner", scene.grid, 5, 5);
    let id = id_of(&scene.world, spawner);
    let (a, _) = ghost_player(&mut scene, 1);
    assert!(takeover(&mut scene.world, a, id));
    let drone = scene.attached(a).unwrap();
    assert_eq!(scene.world.entity_name(drone), "drone");
    assert!(scene.world.is_alive(spawner));
    let (b, _) = ghost_player(&mut scene, 2);
    assert!(takeover(&mut scene.world, b, id));
    scene.world.flush_deletions();
    assert!(!scene.world.is_alive(spawner));
    assert_ne!(scene.attached(b), Some(drone));
}
