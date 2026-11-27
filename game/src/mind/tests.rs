use astrocore_ecs::{EventWorldExt, LifecycleWorldExt};
use astrocore_net::{ConnId, NetUserId, Session};
use bevy_ecs::resource::Resource;

use super::*;
use crate::game_ticker::{GameRunLevel, GameTicker};
use crate::ghost::{Ghost, can_return_to_body};
use crate::testing::{spawn_at, station, world_with};

pub(crate) const PROTOTYPES: &str = r#"
- type: damageType
  id: Asphyxiation
  name: damage-type-asphyxiation
- type: damageType
  id: Blunt
  name: damage-type-blunt
- type: entity
  id: Person
  name: Urist
  components:
  - type: MindContainer
  - type: MindExaminable
  - type: Damageable
  - type: Injurable
  - type: MobState
  - type: MobThresholds
    thresholds:
      0: Alive
      100: Critical
      200: Dead
- type: entity
  id: MobObserver
  name: observer
  components:
  - type: MindContainer
  - type: Ghost
  - type: Eye
"#;

pub(crate) struct Scene {
    pub world: World,
    pub grid: Entity,
}

pub(crate) fn scene(prototypes: &str) -> Scene {
    let mut world = world_with(prototypes);
    let mut ticker = GameTicker::default();
    ticker.run_level = GameRunLevel::InRound;
    world.insert_resource(ticker);
    let grid = station(&mut world);
    Scene { world, grid }
}

impl Scene {
    pub(crate) fn session(&mut self, n: u8) -> (Entity, NetUserId) {
        let user = NetUserId(uuid::Uuid::from_bytes([n; 16]));
        let session = self
            .world
            .spawn(Session {
                conn: ConnId(u64::from(n)),
                user_id: user,
                name: format!("player{n}"),
                attached: None,
                last_input: 0,
                sync: Default::default(),
            })
            .id();
        (session, user)
    }

    /// Игрок в новом теле, как при появлении в раунде.
    pub(crate) fn player(&mut self, n: u8) -> (Entity, NetUserId, Entity, Entity) {
        let (session, user) = self.session(n);
        let body = spawn_at(&mut self.world, "Person", self.grid, 2, 2);
        let mind = create_mind(&mut self.world, Some(user), Some("Urist"));
        transfer_to(&mut self.world, mind, Some(body), false, true);
        (session, user, body, mind)
    }

    pub(crate) fn attached(&self, session: Entity) -> Option<Entity> {
        self.world.get::<Session>(session).and_then(|s| s.attached)
    }

    pub(crate) fn state(&self, mind: Entity) -> Mind {
        self.world.get::<Mind>(mind).unwrap().clone()
    }
}

#[derive(Resource, Default)]
struct Log(Vec<&'static str>);

#[test]
fn mind_ties_user_session_and_body() {
    let mut scene = scene(PROTOTYPES);
    scene.world.init_resource::<Log>();
    scene
        .world
        .subscribe::<MindContainer, MindAdded>(|world, _, _| {
            world.resource_mut::<Log>().0.push("added");
        });
    scene
        .world
        .subscribe::<MindContainer, MindRemoved>(|world, _, _| {
            world.resource_mut::<Log>().0.push("removed");
        });
    let (session, user, body, mind) = scene.player(1);
    assert_eq!(scene.attached(session), Some(body));
    assert_eq!(mind_of(&scene.world, body), Some(mind));
    assert_eq!(mind_of_user(&scene.world, user), Some(mind));
    assert_eq!(character_name(&scene.world, user).as_deref(), Some("Urist"));
    // В другое тело: прежнее остаётся без разума, но помнит последний.
    let other = spawn_at(&mut scene.world, "Person", scene.grid, 4, 4);
    transfer_to(&mut scene.world, mind, Some(other), false, true);
    assert_eq!(scene.attached(session), Some(other));
    assert_eq!(mind_of(&scene.world, body), None);
    assert_eq!(last_mind(&scene.world, body), Some(mind));
    assert_eq!(scene.state(mind).original_owned_entity, Some(body));
    assert_eq!(
        scene.world.resource::<Log>().0,
        ["added", "removed", "added"]
    );
    // Новый разум того же пользователя забирает его; старый остаётся без владельца.
    let fresh = create_mind(&mut scene.world, Some(user), None);
    assert_eq!(mind_of_user(&scene.world, user), Some(fresh));
    assert_eq!(scene.state(mind).user_id, None);
    assert_eq!(scene.attached(session), None);
}

#[test]
fn ghost_visits_and_returns_to_body() {
    let mut scene = scene(PROTOTYPES);
    let (session, _, body, mind) = scene.player(1);
    let ghost = crate::ghost::spawn_ghost(&mut scene.world, mind, None, true).unwrap();
    assert_eq!(scene.attached(session), Some(ghost));
    assert_eq!(scene.state(mind).owned_entity, Some(body));
    assert_eq!(scene.state(mind).visiting_entity, Some(ghost));
    assert_eq!(mind_of(&scene.world, ghost), Some(mind));
    assert!(can_return_to_body(&scene.world, ghost));
    assert_eq!(scene.world.entity_name(ghost), "Urist");
    // Обратно в тело: призрак удаляется.
    unvisit(&mut scene.world, mind);
    scene.world.flush_deletions();
    assert_eq!(scene.attached(session), Some(body));
    assert!(!scene.world.is_alive(ghost));
    assert_eq!(scene.state(mind).visiting_entity, None);
}

#[test]
fn ghosting_while_alive_is_final_and_dead_bodies_can_be_returned_to() {
    let mut scene = scene(PROTOTYPES);
    let (session, _, body, mind) = scene.player(1);
    assert!(crate::ghost::on_ghost_attempt(
        &mut scene.world,
        mind,
        true,
        true,
        false
    ));
    let ghost = scene.attached(session).unwrap();
    assert!(scene.world.get::<Ghost>(ghost).is_some());
    // Живое тело брошено: призрак — новое тело разума, тело — кататоник.
    assert!(!can_return_to_body(&scene.world, ghost));
    assert_eq!(scene.state(mind).owned_entity, Some(ghost));
    assert_eq!(mind_state(&mut scene.world, body), MindState::Catatonic);

    let (session, _, body, mind) = scene.player(2);
    scene
        .world
        .change_mob_state(body, crate::mobs::MobState::Dead, None);
    assert!(scene.state(mind).time_of_death.is_some());
    assert!(crate::ghost::on_ghost_attempt(
        &mut scene.world,
        mind,
        true,
        false,
        false
    ));
    let ghost = scene.attached(session).unwrap();
    assert!(can_return_to_body(&scene.world, ghost));
    assert_eq!(scene.state(mind).owned_entity, Some(body));
    assert_eq!(mind_state(&mut scene.world, body), MindState::Dead);
}

#[test]
fn critical_bodies_are_finished_off_when_ghosting() {
    let mut scene = scene(PROTOTYPES);
    let (session, _, body, mind) = scene.player(1);
    let damage = crate::damage::DamageSpecifier::new([(
        "Blunt",
        crate::fixed_point::FixedPoint2::from_f32(150.0),
    )]);
    crate::damage::DamageableWorldExt::change_damage(
        &mut scene.world,
        body,
        &damage,
        true,
        false,
        None,
    );
    assert!(scene.world.is_mob_critical(body));
    assert!(crate::ghost::on_ghost_attempt(
        &mut scene.world,
        mind,
        true,
        false,
        false
    ));
    assert!(scene.world.is_mob_dead(body));
    assert!(can_return_to_body(
        &scene.world,
        scene.attached(session).unwrap()
    ));
}

#[test]
fn deleted_bodies_leave_ghosts_and_restart_leaves_none() {
    let mut scene = scene(PROTOTYPES);
    let (session, _, body, mind) = scene.player(1);
    scene.world.delete_entity(body);
    let ghost = scene.attached(session).unwrap();
    assert!(scene.world.get::<Ghost>(ghost).is_some());
    assert_eq!(scene.state(mind).owned_entity, Some(ghost));
    // Перед перезапуском разумы стираются: удаление тел призраков не порождает.
    let (_, _, body2, _) = scene.player(2);
    scene
        .world
        .raise_broadcast(&mut crate::game_ticker::RoundRestartCleanup);
    assert!(scene.world.resource::<Minds>().user_minds.is_empty());
    scene.world.delete_entity(body2);
    let ghosts = scene.world.query::<&Ghost>().iter(&scene.world).count();
    assert_eq!(ghosts, 1);
}

#[test]
fn examine_tells_ssd_from_catatonic() {
    let mut scene = scene(PROTOTYPES);
    let (session, user, body, _) = scene.player(1);
    assert_eq!(mind_state(&mut scene.world, body), MindState::None);
    // Вышел — SSD; без игрока совсем — кататоник.
    scene.world.despawn(session);
    assert_eq!(mind_state(&mut scene.world, body), MindState::Ssd);
    wipe_user_mind(&mut scene.world, user);
    let lonely = spawn_at(&mut scene.world, "Person", scene.grid, 5, 5);
    assert_eq!(mind_state(&mut scene.world, lonely), MindState::Catatonic);
    assert_eq!(mind_state(&mut scene.world, body), MindState::Catatonic);
    let lines = crate::examine::examine_text(&mut scene.world, lonely, lonely);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("comp-mind-examined-catatonic") && l.contains("mediumpurple"))
    );
}
