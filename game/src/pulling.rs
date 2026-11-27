//! Перетаскивание: тянущий держит сущность на «верёвке» (дистанционное сочленение физики),
//! рука занята виртуальным предметом; Ctrl+клик тянет и отпускает, H отпускает,
//! Ctrl+правый клик подтаскивает к точке. Образец — `Content.Shared/Movement/Pulling`
//! (`PullingSystem`, `PullableComponent`, `PullerComponent`) и `Content.Server/Movement/
//! Systems/PullController.cs` SS14 @ 2f4a2aad. Значки «тащу» и «меня тащат»; нажатие на
//! них отпускает (`StopPullingAlert`, `StopBeingPulledAlert`).

use astrocore_ecs::{
    EventWorldExt, GameTime, GameWorldExt, LifecycleWorldExt, RegistryWorldExt, Update,
};
use astrocore_input::{BoundKeyState, InputWorldExt};
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::{
    ContainerWorldExt, EntGotInsertedIntoContainer, MapCoordinates, TransformWorldExt,
};
use astrocore_math::Vec2;
use astrocore_movement::{InputMover, MoveButtons};
use astrocore_net::{NetEntity, NetRegistryWorldExt};
use astrocore_physics::{BodyType, JointRemoved, JointWorldExt, Physics, PhysicsWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::action_blocker::ActionBlockerWorldExt;
use crate::examine::loc;
use crate::hands::{Hands, HandsWorldExt};
use crate::interaction::{INTERACTION_RANGE, InteractionWorldExt};
use crate::mobs::{MobState, MobStateChanged};
use crate::popups::{PopupType, PopupWorldExt};
use crate::standing::DropHandItems;
use crate::verbs::{GetVerbs, Verb, VerbKind};
use crate::virtual_item::{VirtualItemDeleted, VirtualItemWorldExt};

/// Функции ввода (`ContentKeyFunctions`).
pub mod functions {
    pub const TRY_PULL_OBJECT: &str = "TryPullObject";
    pub const MOVE_PULLED_OBJECT: &str = "MovePulledObject";
    pub const RELEASE_PULLED_OBJECT: &str = "ReleasePulledObject";
}

/// Множитель скорости тянущего (`PullerComponent.WalkSpeedModifier`).
pub const PULLING_SPEED_MODIFIER: f32 = 0.95;

// Подтаскивание к точке (`PullController`).
const ACCEL_MODIFIER_HIGH: f32 = 15.0;
const ACCEL_MODIFIER_LOW: f32 = 60.0;
const ACCEL_MODIFIER_HIGH_MASS: f32 = 70.0;
const ACCEL_MODIFIER_LOW_MASS: f32 = 5.0;
const MAXIMUM_SETTLE_VELOCITY: f32 = 0.1;
const MAXIMUM_SETTLE_DISTANCE: f32 = 0.1;
const SETTLE_MINIMUM_SHUTDOWN_VELOCITY: f32 = 0.25;
const SETTLE_SHUTDOWN_DISTANCE: f32 = 1.0;
const SETTLE_SHUTDOWN_MULTIPLIER: f32 = 20.0;

fn pulled_alert() -> String {
    "Pulled".into()
}

fn pulling_alert() -> String {
    "Pulling".into()
}

fn one_second() -> f64 {
    1.0
}

fn yes() -> bool {
    true
}

/// Сущность можно тянуть. В данных — компонент `Pullable`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pullable {
    #[serde(skip)]
    pub puller: Option<Entity>,
    #[serde(default)]
    pub pull_joint_id: Option<String>,
    #[serde(skip)]
    pub fixed_rotation_on_pull: bool,
    #[serde(default)]
    pub prev_fixed_rotation: bool,
    #[serde(default = "pulled_alert")]
    pub pulled_alert: String,
}

impl Default for Pullable {
    fn default() -> Self {
        Self {
            puller: None,
            pull_joint_id: None,
            fixed_rotation_on_pull: false,
            prev_fixed_rotation: false,
            pulled_alert: pulled_alert(),
        }
    }
}

/// Сущность умеет тянуть. В данных — компонент `Puller`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Puller {
    #[serde(skip)]
    pub next_throw: f64,
    #[serde(default = "one_second")]
    pub throw_cooldown: f64,
    #[serde(skip)]
    pub pulling: Option<Entity>,
    #[serde(default = "yes")]
    pub needs_hands: bool,
    #[serde(default = "pulling_alert")]
    pub pulling_alert: String,
}

impl Default for Puller {
    fn default() -> Self {
        Self {
            next_throw: 0.0,
            throw_cooldown: one_second(),
            pulling: None,
            needs_hands: true,
            pulling_alert: pulling_alert(),
        }
    }
}

/// Тянущий сейчас тянет (`ActivePullerComponent`).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ActivePuller;

/// Подтаскивается к точке (`PullMovingComponent`).
#[derive(Component, Debug, Clone, Copy)]
pub struct PullMoving {
    pub moving_to: MapCoordinates,
}

/// Можно ли тянуть эту сущность (`BeingPulledAttemptEvent`). На тянутой.
#[derive(Debug, Clone, Copy)]
pub struct BeingPulledAttempt {
    pub puller: Entity,
    pub pulled: Entity,
    pub cancelled: bool,
}

/// Может ли тянуть (`StartPullAttemptEvent`). На тянущем.
#[derive(Debug, Clone, Copy)]
pub struct StartPullAttempt {
    pub puller: Entity,
    pub pulled: Entity,
    pub cancelled: bool,
}

/// Попытка начать (`PullAttemptEvent`). На тянущем, затем на тянутой.
#[derive(Debug, Clone, Copy)]
pub struct PullAttempt {
    pub puller: Entity,
    pub pulled: Entity,
    pub cancelled: bool,
}

/// Попытка отпустить (`AttemptStopPullingEvent`). На тянутой.
#[derive(Debug, Clone, Copy)]
pub struct AttemptStopPulling {
    pub user: Option<Entity>,
    pub cancelled: bool,
}

/// Начали тянуть (`PullStartedMessage`). На обоих.
#[derive(Debug, Clone, Copy)]
pub struct PullStarted {
    pub puller: Entity,
    pub pulled: Entity,
}

/// Перестали тянуть (`PullStoppedMessage`). На обоих.
#[derive(Debug, Clone, Copy)]
pub struct PullStopped {
    pub puller: Entity,
    pub pulled: Entity,
}

/// Перетаскивание на `World`.
pub trait PullingWorldExt {
    fn is_pulled(&self, entity: Entity) -> bool;
    fn puller_of(&self, entity: Entity) -> Option<Entity>;
    fn pulling_of(&self, puller: Entity) -> Option<Entity>;
    /// Можно ли тянуть (`CanPull`).
    fn can_pull(&mut self, puller: Entity, pulled: Entity) -> bool;
    /// Начать (`TryStartPull`).
    fn try_start_pull(&mut self, puller: Entity, pulled: Entity) -> bool;
    /// Отпустить (`TryStopPull`).
    fn try_stop_pull(&mut self, pulled: Entity, user: Option<Entity>) -> bool;
    /// Начать или отпустить (`TogglePull`).
    fn toggle_pull(&mut self, pulled: Entity, puller: Entity) -> bool;
}

impl PullingWorldExt for World {
    fn is_pulled(&self, entity: Entity) -> bool {
        self.puller_of(entity).is_some()
    }

    fn puller_of(&self, entity: Entity) -> Option<Entity> {
        self.get::<Pullable>(entity)?.puller
    }

    fn pulling_of(&self, puller: Entity) -> Option<Entity> {
        self.get::<Puller>(puller)?.pulling
    }

    fn can_pull(&mut self, puller: Entity, pulled: Entity) -> bool {
        let Some(component) = self.get::<Puller>(puller).cloned() else {
            return false;
        };
        if component.needs_hands && self.empty_hand(puller).is_none() && component.pulling.is_none()
        {
            return false;
        }
        if !self.can_interact(puller, Some(pulled)) {
            return false;
        }
        let Some(body) = self.get::<Physics>(pulled) else {
            return false;
        };
        if body.body_type == BodyType::Static || puller == pulled {
            return false;
        }
        let container = |world: &World, entity| {
            world
                .containing_container(entity)
                .map(|c| (c.owner, c.id.clone()))
        };
        if container(self, puller) != container(self, pulled) {
            return false;
        }
        let mut being = BeingPulledAttempt {
            puller,
            pulled,
            cancelled: false,
        };
        self.raise_and_broadcast(pulled, &mut being);
        let mut start = StartPullAttempt {
            puller,
            pulled,
            cancelled: false,
        };
        self.raise_and_broadcast(puller, &mut start);
        !start.cancelled && !being.cancelled
    }

    fn try_start_pull(&mut self, puller: Entity, pulled: Entity) -> bool {
        if self.get::<Puller>(puller).is_none() || self.get::<Pullable>(pulled).is_none() {
            return false;
        }
        if self.pulling_of(puller) == Some(pulled) {
            return true;
        }
        if !self.can_pull(puller, pulled) {
            return false;
        }
        if self.get::<Physics>(puller).is_none() || self.get::<Physics>(pulled).is_none() {
            return false;
        }
        if let Some(old) = self.pulling_of(puller)
            && !self.try_stop_pull(old, Some(puller))
        {
            return false;
        }
        if let Some(other) = self.puller_of(pulled)
            && (other == puller || !self.try_stop_pull(pulled, Some(other)))
        {
            return false;
        }
        let mut attempt = PullAttempt {
            puller,
            pulled,
            cancelled: false,
        };
        self.raise(puller, &mut attempt);
        if attempt.cancelled {
            return false;
        }
        self.raise(pulled, &mut attempt);
        if attempt.cancelled {
            return false;
        }
        let net = self
            .get::<NetEntity>(pulled)
            .map_or(pulled.to_bits(), |n| u64::from(n.0));
        let joint_id = format!("pull-joint-{net}");
        let prev_fixed = self
            .get::<Physics>(pulled)
            .is_some_and(|b| b.fixed_rotation);
        let fixed_on_pull = self
            .get::<Pullable>(pulled)
            .is_some_and(|p| p.fixed_rotation_on_pull);
        self.ensure_comp::<ActivePuller>(puller);
        if let Some(mut component) = self.get_mut::<Puller>(puller) {
            component.pulling = Some(pulled);
        }
        if let Some(mut component) = self.get_mut::<Pullable>(pulled) {
            component.puller = Some(puller);
            component.pull_joint_id = Some(joint_id.clone());
            component.prev_fixed_rotation = prev_fixed;
        }
        if self
            .create_distance_joint(pulled, puller, Vec2::ZERO, Vec2::ZERO, &joint_id)
            .is_some()
        {
            self.edit_joint(pulled, &joint_id, |joint| {
                joint.collide_connected = false;
                if let Some(rope) = joint.distance_mut() {
                    rope.max_length = rope.length + 0.15;
                    rope.min_length = 0.0;
                    rope.stiffness = 0.0;
                }
            });
        }
        if let Some(mut body) = self.get_mut::<Physics>(pulled) {
            body.fixed_rotation = fixed_on_pull;
        }
        refresh_speed(self, puller);
        let pulling_alert = self.get::<Puller>(puller).map(|p| p.pulling_alert.clone());
        let pulled_alert = self.get::<Pullable>(pulled).map(|p| p.pulled_alert.clone());
        if let Some(alert) = pulling_alert {
            crate::alerts::show_alert(self, puller, &alert, None);
        }
        if let Some(alert) = pulled_alert {
            crate::alerts::show_alert(self, pulled, &alert, None);
        }
        let mut started = PullStarted { puller, pulled };
        self.raise(puller, &mut started);
        self.raise(pulled, &mut started);
        let name = self.entity_arg(puller);
        let text = loc(
            self,
            "getting-pulled-popup",
            &[("puller", name.clone().into())],
        );
        self.popup_entity_for(&text, pulled, pulled, PopupType::Small);
        true
    }

    fn try_stop_pull(&mut self, pulled: Entity, user: Option<Entity>) -> bool {
        if self.puller_of(pulled).is_none() {
            return true;
        }
        let mut attempt = AttemptStopPulling {
            user,
            cancelled: false,
        };
        self.raise_and_broadcast(pulled, &mut attempt);
        if attempt.cancelled {
            return false;
        }
        stop_pulling(self, pulled);
        true
    }

    fn toggle_pull(&mut self, pulled: Entity, puller: Entity) -> bool {
        if self.get::<Pullable>(pulled).is_none() {
            return false;
        }
        if self.puller_of(pulled) == Some(puller) {
            return self.try_stop_pull(pulled, None);
        }
        self.try_start_pull(puller, pulled)
    }
}

/// Отпустить без проверок (`StopPulling`).
fn stop_pulling(world: &mut World, pulled: Entity) {
    let Some(component) = world.get::<Pullable>(pulled).cloned() else {
        return;
    };
    let Some(puller) = component.puller else {
        return;
    };
    if let Some(mut pullable) = world.get_mut::<Pullable>(pulled) {
        pullable.puller = None;
        pullable.pull_joint_id = None;
    }
    if let Some(id) = &component.pull_joint_id {
        world.remove_joint(pulled, id);
    }
    if let Some(mut body) = world.get_mut::<Physics>(pulled) {
        body.fixed_rotation = component.prev_fixed_rotation;
    }
    world.remove_comp::<ActivePuller>(puller);
    world.remove_comp::<PullMoving>(pulled);
    crate::alerts::clear_alert(world, pulled, &component.pulled_alert);
    let Some(pulling_alert) = world.get_mut::<Puller>(puller).map(|mut puller_component| {
        puller_component.pulling = None;
        puller_component.pulling_alert.clone()
    }) else {
        return;
    };
    crate::alerts::clear_alert(world, puller, &pulling_alert);
    refresh_speed(world, puller);
    let mut stopped = PullStopped { puller, pulled };
    world.raise(puller, &mut stopped);
    world.raise(pulled, &mut stopped);
}

/// Скорость тянущего (`OnRefreshMovespeed`): 0,95 от обычной.
fn refresh_speed(world: &mut World, puller: Entity) {
    crate::movement_speed::refresh_movement_speed_modifiers(world, puller);
}

/// Глаголы «Тянуть» и «Перестать тянуть» (`AddPullVerbs`).
fn pull_verbs(world: &mut World, pulled: Entity, event: &mut GetVerbs) {
    if event.kind != VerbKind::Other
        || !event.can_access
        || !event.can_interact
        || event.user == pulled
    {
        return;
    }
    let user = event.user;
    if world.puller_of(pulled) == Some(user) {
        let text = loc(world, "pulling-verb-get-data-text-stop-pulling", &[]);
        let mut verb = Verb::new(VerbKind::Other, text, move |world| {
            world.try_stop_pull(pulled, Some(user));
        });
        verb.do_contact_interaction = Some(false);
        event.verbs.push(verb);
    } else if world.can_pull(user, pulled) {
        let text = loc(world, "pulling-verb-get-data-text", &[]);
        let mut verb = Verb::new(VerbKind::Other, text, move |world| {
            world.try_start_pull(user, pulled);
        });
        verb.do_contact_interaction = Some(false);
        event.verbs.push(verb);
    }
}

/// Тянутый сам пошёл — вырвался (`OnPullableMoveInput`); подтаскивание к точке
/// (`PullController.UpdateBeforeSolve`).
fn update(world: &mut World) {
    let escaping: Vec<Entity> = world
        .query::<(Entity, &Pullable, &InputMover)>()
        .iter(world)
        .filter(|(_, pullable, mover)| {
            pullable.puller.is_some() && mover.can_move && mover.held.0 & !MoveButtons::WALK.0 != 0
        })
        .map(|(entity, _, _)| entity)
        .collect();
    for entity in escaping {
        world.try_stop_pull(entity, Some(entity));
    }

    let dt = world.resource::<GameTime>().frame_time();
    let moving: Vec<(Entity, PullMoving)> = world
        .query::<(Entity, &PullMoving)>()
        .iter(world)
        .map(|(entity, moving)| (entity, *moving))
        .collect();
    for (pulled, moving) in moving {
        let Some(puller) = world.puller_of(pulled) else {
            world.remove_comp::<PullMoving>(pulled);
            continue;
        };
        let (Some(puller_at), Some(pulled_at)) =
            (world.map_coordinates(puller), world.map_coordinates(pulled))
        else {
            world.remove_comp::<PullMoving>(pulled);
            continue;
        };
        let Some(body) = world.get::<Physics>(pulled).cloned() else {
            world.remove_comp::<PullMoving>(pulled);
            continue;
        };
        if moving.moving_to.map != puller_at.map
            || moving.moving_to.map != pulled_at.map
            || body.body_type == BodyType::Static
        {
            world.remove_comp::<PullMoving>(pulled);
            continue;
        }
        let diff = moving.moving_to.position - pulled_at.position;
        let distance = diff.length();
        if distance < MAXIMUM_SETTLE_DISTANCE
            && body.linear_velocity.length() < MAXIMUM_SETTLE_VELOCITY
        {
            world.set_linear_velocity(pulled, Vec2::ZERO);
            world.remove_comp::<PullMoving>(pulled);
            continue;
        }
        let lerp = ((body.mass - ACCEL_MODIFIER_LOW_MASS)
            / (ACCEL_MODIFIER_HIGH_MASS - ACCEL_MODIFIER_LOW_MASS))
            .clamp(0.0, 1.0);
        let modifier = ACCEL_MODIFIER_LOW + (ACCEL_MODIFIER_HIGH - ACCEL_MODIFIER_LOW) * lerp;
        let multiplier = if distance < 1.0 {
            modifier * distance
        } else {
            modifier
        };
        let mut accel = diff.normalize_or_zero() * multiplier;
        if distance < SETTLE_SHUTDOWN_DISTANCE
            && body.linear_velocity.length() >= SETTLE_MINIMUM_SHUTDOWN_VELOCITY
        {
            let scaling = (SETTLE_SHUTDOWN_DISTANCE - distance) / SETTLE_SHUTDOWN_DISTANCE;
            accel -= body.linear_velocity * SETTLE_SHUTDOWN_MULTIPLIER * scaling;
        }
        world.wake_body(pulled);
        world.apply_linear_impulse(pulled, accel * body.mass * dt);
    }
}

/// Ctrl+правый клик: подтащить тянутое к точке, не дальше длины верёвки
/// (`OnRequestMovePulledObject`).
fn move_pulled(world: &mut World, player: Entity, to: MapCoordinates) {
    let Some(pulled) = world.pulling_of(player) else {
        return;
    };
    if world.get::<Pullable>(pulled).is_none() || world.containing_container(player).is_some() {
        return;
    }
    let now = world.resource::<GameTime>().cur_time().as_secs_f64();
    if let Some(mut puller) = world.get_mut::<Puller>(player) {
        puller.next_throw = now + puller.throw_cooldown;
    }
    let Some(from) = world.map_coordinates(player) else {
        return;
    };
    let mut target = to;
    let mut range = 2.0f32;
    if from.map != to.map || from.position.distance(to.position) > range {
        let joint = world
            .get::<Pullable>(pulled)
            .and_then(|p| p.pull_joint_id.clone());
        if let Some(rope) =
            joint.and_then(|id| world.joint(player, &id).and_then(|j| j.distance().copied()))
        {
            range = (rope.max_length - 0.01).max(0.01);
        }
        let direction = (to.position - from.position).normalize_or_zero();
        target = MapCoordinates {
            position: from.position + direction * (range - 0.01),
            ..from
        };
    }
    world.add_comp(pulled, PullMoving { moving_to: target });
}

fn bind_inputs(world: &mut World) {
    // Ctrl+клик (`HandleTryPullObject`).
    world.bind_input(functions::TRY_PULL_OBJECT, |world, args| {
        if args.state != BoundKeyState::Down {
            return false;
        }
        let (Some(user), Some(target)) = (args.player, args.target) else {
            return false;
        };
        if user == target || !world.is_alive(target) {
            return false;
        }
        if !crate::interaction::interaction_rate_allows(world, args) {
            return false;
        }
        if !world.in_range_unobstructed(user, target, INTERACTION_RANGE) {
            return false;
        }
        world.toggle_pull(target, user);
        false
    });
    world.bind_input(functions::RELEASE_PULLED_OBJECT, |world, args| {
        if args.state != BoundKeyState::Down {
            return false;
        }
        if let Some(player) = args.player
            && let Some(pulled) = world.pulling_of(player)
        {
            world.try_stop_pull(pulled, Some(player));
        }
        false
    });
    world.bind_input(functions::MOVE_PULLED_OBJECT, |world, args| {
        if args.state == BoundKeyState::Down
            && let (Some(player), Some(at)) = (args.player, args.coordinates)
        {
            move_pulled(world, player, at);
        }
        false
    });
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<Pullable>();
    world.register_data_component::<Puller>();
    world.register_net_serde::<Pullable>();
    world.register_net_serde::<Puller>();
    if !side.is_server() {
        return;
    }
    // Нажатия на значки (`OnStopPullingAlert`, `OnStopBeingPulledAlert`).
    world.subscribe::<Puller, crate::alerts::AlertClicked>(|world, entity, event| {
        if event.handled || event.event != "StopPullingAlertEvent" {
            return;
        }
        if let Some(pulled) = world.pulling_of(entity) {
            event.handled = world.try_stop_pull(pulled, Some(entity));
        }
    });
    world.subscribe::<Pullable, crate::alerts::AlertClicked>(|world, entity, event| {
        if event.handled || event.event != "StopBeingPulledAlertEvent" {
            return;
        }
        event.handled = world.try_stop_pull(entity, Some(entity));
    });
    world.subscribe::<Pullable, GetVerbs>(pull_verbs);
    // Сочленение убрали (удаление тела, физика) — перетаскивание кончилось.
    world.subscribe::<Pullable, JointRemoved>(|world, pulled, event| {
        let ours = world.get::<Pullable>(pulled).is_some_and(|p| {
            p.puller == Some(event.other)
                && p.pull_joint_id.as_deref() == Some(event.joint.id.as_str())
        });
        if ours {
            stop_pulling(world, pulled);
        }
    });
    world.subscribe::<Pullable, EntGotInsertedIntoContainer>(|world, pulled, _| {
        world.try_stop_pull(pulled, None);
    });
    world.subscribe::<Puller, crate::movement_speed::RefreshMovementSpeedModifiers>(
        |world, puller, event| {
            if world.pulling_of(puller).is_some() {
                event.modify_speed(PULLING_SPEED_MODIFIER, PULLING_SPEED_MODIFIER);
            }
        },
    );
    world.subscribe::<Puller, EntGotInsertedIntoContainer>(|world, puller, _| {
        if let Some(pulled) = world.pulling_of(puller) {
            world.try_stop_pull(pulled, Some(puller));
        }
    });
    world.subscribe::<Puller, MobStateChanged>(|world, puller, event| {
        if matches!(event.new_state, MobState::Critical | MobState::Dead)
            && let Some(pulled) = world.pulling_of(puller)
        {
            world.try_stop_pull(pulled, None);
        }
    });
    world.subscribe::<Puller, DropHandItems>(|world, puller, _| {
        let needs_hands = world.get::<Puller>(puller).is_some_and(|p| p.needs_hands);
        if !needs_hands && let Some(pulled) = world.pulling_of(puller) {
            world.try_stop_pull(pulled, Some(puller));
        }
    });
    // Выпустил виртуальный предмет из руки — отпустил (`OnVirtualItemDeleted`).
    world.subscribe::<Puller, VirtualItemDeleted>(|world, puller, event| {
        if world.pulling_of(puller) == Some(event.blocking_entity) {
            world.try_stop_pull(event.blocking_entity, None);
        }
    });
    // Рука занята, пока тянет (`HandlePullStarted`, `HandlePullStopped`).
    world.subscribe::<Hands, PullStarted>(|world, entity, event| {
        if event.puller != entity || world.get::<Puller>(entity).is_some_and(|p| !p.needs_hands) {
            return;
        }
        world.try_spawn_virtual_item_in_hand(event.pulled, entity);
    });
    world.subscribe::<Hands, PullStopped>(|world, entity, event| {
        if event.puller == entity {
            world.delete_in_hands_matching(entity, event.pulled);
        }
    });
    bind_inputs(world);
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
