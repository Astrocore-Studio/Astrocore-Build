//! Взаимодействие игрока с миром: клик рукой, клик предметом, «использовать» (E),
//! использование предмета в руке, проверка досягаемости и доступа, ограничение частоты
//! запросов клиента. Образец —
//! `Content.Shared/Interaction/SharedInteractionSystem.cs` SS14 @ 2f4a2aad.
//!
//! Цепочка клика (`UserInteraction`): пустая рука — `InteractHand` на цели, а если его никто
//! не обработал — «использовать» (`ActivateInWorld`); предмет в руке и цель рядом —
//! `InteractUsing` на цели, затем `AfterInteract` на предмете; цель далеко — сразу
//! `AfterInteract` с `can_reach: false`. Клик по самому предмету в руке — `UseInHand`.
//! Системы отвечают на события и помечают их обработанными.

use astrocore_console::{CVarDef, CVarFlags, ConfigWorldExt};
use astrocore_ecs::{EventWorldExt, GameWorldExt, LifecycleWorldExt, MetaData, RegistryWorldExt};
use astrocore_input::{BoundKeyState, InputArgs, InputWorldExt};
use astrocore_map::{ContainerWorldExt, MapCoordinates, MapGrid, MapWorldExt, TransformWorldExt};
use astrocore_math::Angle;
use astrocore_movement::InputMover;
use astrocore_net::NetRegistryWorldExt;
use astrocore_physics::{
    CollisionGroups, DistanceWorldExt, Fixtures, Physics, PhysicsWorldExt, Ray, RayWorldExt,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::action_blocker::ActionBlockerWorldExt;
use crate::content::groups;
use crate::item::Item;
use crate::tag::TagWorldExt;

/// Дальность взаимодействия в тайлах (`InteractionRange`).
pub const INTERACTION_RANGE: f32 = 1.5;

/// Самый длинный луч досягаемости (`MaxRaycastRange`).
pub const MAX_RAYCAST_RANGE: f32 = 100.0;

/// Тег, снимающий проверки дальности и доступа (админы, маппинг).
pub const BYPASS_RANGE_TAG: &str = "BypassInteractionRangeChecks";

/// Функции ввода взаимодействия (`EngineKeyFunctions`, `ContentKeyFunctions`).
pub mod functions {
    pub const USE: &str = "Use";
    pub const ACTIVATE_IN_WORLD: &str = "ActivateItemInWorld";
    pub const ALT_ACTIVATE_IN_WORLD: &str = "AltActivateItemInWorld";
    pub const USE_ITEM_IN_HAND: &str = "UseItemInHand";
}

/// Настенный предмет: до него достают через стену, на которой он висит, со стороны дуги
/// `arc` вокруг направления `direction`. В данных — компонент `WallMount`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WallMount {
    #[serde(default = "half_turn")]
    pub arc: Angle,
    #[serde(default)]
    pub direction: Angle,
    #[serde(default = "yes", rename = "directionalVisibility")]
    pub directional_visibility: bool,
}

fn half_turn() -> Angle {
    Angle(std::f32::consts::PI)
}

fn yes() -> bool {
    true
}

/// Каким предметом действует сущность (`GetUsedEntityEvent`): руки отвечают предметом в
/// активной руке. Поднимается на действующем.
#[derive(Debug, Clone, Copy)]
pub struct GetUsedEntity {
    pub used: Option<Entity>,
}

/// Перед кликом рукой (`BeforeInteractHandEvent`). Поднимается на действующем.
#[derive(Debug, Clone, Copy)]
pub struct BeforeInteractHand {
    pub target: Entity,
    pub handled: bool,
}

/// Клик пустой рукой по цели (`InteractHandEvent`). Поднимается на цели и широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct InteractHand {
    pub user: Entity,
    pub target: Entity,
    pub handled: bool,
}

/// То же для действующего (`UserInteractHandEvent`). Поднимается на действующем.
#[derive(Debug, Clone, Copy)]
pub struct UserInteractHand {
    pub user: Entity,
    pub target: Entity,
    pub handled: bool,
}

/// Перед действием предметом на расстоянии или вплотную (`BeforeRangedInteractEvent`).
/// Поднимается на предмете.
#[derive(Debug, Clone, Copy)]
pub struct BeforeRangedInteract {
    pub user: Entity,
    pub used: Entity,
    pub target: Option<Entity>,
    pub click: MapCoordinates,
    pub can_reach: bool,
    pub handled: bool,
}

/// Предметом кликнули по далёкой цели (`RangedInteractEvent`). Поднимается на цели.
#[derive(Debug, Clone, Copy)]
pub struct RangedInteract {
    pub user: Entity,
    pub used: Entity,
    pub target: Entity,
    pub click: MapCoordinates,
    pub handled: bool,
}

/// Предметом кликнули по цели рядом (`InteractUsingEvent`). Поднимается на цели и
/// широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct InteractUsing {
    pub user: Entity,
    pub used: Entity,
    pub target: Entity,
    pub click: MapCoordinates,
    pub handled: bool,
}

/// То же для действующего (`UserInteractUsingEvent`). Поднимается на действующем.
#[derive(Debug, Clone, Copy)]
pub struct UserInteractUsing {
    pub user: Entity,
    pub used: Entity,
    pub target: Entity,
    pub click: MapCoordinates,
    pub handled: bool,
}

/// После клика предметом, если цель его не обработала (`AfterInteractEvent`).
/// Поднимается на предмете.
#[derive(Debug, Clone, Copy)]
pub struct AfterInteract {
    pub user: Entity,
    pub used: Entity,
    pub target: Option<Entity>,
    pub click: MapCoordinates,
    pub can_reach: bool,
    pub handled: bool,
}

/// После клика предметом — цели (`AfterInteractUsingEvent`). Поднимается на цели.
#[derive(Debug, Clone, Copy)]
pub struct AfterInteractUsing {
    pub user: Entity,
    pub used: Entity,
    pub target: Entity,
    pub click: MapCoordinates,
    pub can_reach: bool,
    pub handled: bool,
}

/// «Использовать» сущность (`ActivateInWorldEvent`): клавиша E или клик рукой, который не
/// обработали. `complex` — действующий умеет сложные действия. Поднимается на цели и
/// широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct ActivateInWorld {
    pub user: Entity,
    pub target: Entity,
    pub complex: bool,
    pub handled: bool,
}

/// То же для действующего (`UserActivateInWorldEvent`). Поднимается на действующем.
#[derive(Debug, Clone, Copy)]
pub struct UserActivateInWorld {
    pub user: Entity,
    pub target: Entity,
    pub complex: bool,
    pub handled: bool,
}

/// Предмет использовали в руке (`UseInHandEvent`). Поднимается на предмете и
/// широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct UseInHand {
    pub user: Entity,
    pub used: Entity,
    pub handled: bool,
}

/// Сущности соприкоснулись при взаимодействии (`ContactInteractionEvent`): отпечатки,
/// заразы. Поднимается на обеих.
#[derive(Debug, Clone, Copy)]
pub struct ContactInteraction {
    pub other: Entity,
}

/// Переопределение досягаемости (`InRangeOverrideEvent`). Поднимается на действующем.
#[derive(Debug, Clone, Copy)]
pub struct InRangeOverride {
    pub user: Entity,
    pub target: Entity,
    pub handled: bool,
    pub in_range: bool,
}

/// Переопределение доступа (`AccessibleOverrideEvent`). Поднимается на действующем и на
/// цели.
#[derive(Debug, Clone, Copy)]
pub struct AccessibleOverride {
    pub user: Entity,
    pub target: Entity,
    pub handled: bool,
    pub accessible: bool,
}

/// Сущность в контейнере, вложена в него или удаляется.
fn is_deleted(world: &World, entity: Entity) -> bool {
    !world.is_alive(entity)
        || world
            .get::<MetaData>(entity)
            .is_none_or(|meta| meta.terminating())
}

fn should_check_access(world: &World, user: Entity) -> bool {
    !world.has_tag(user, BYPASS_RANGE_TAG)
}

fn paused(world: &World, entity: Entity) -> bool {
    world
        .get::<MetaData>(entity)
        .is_none_or(|meta| meta.paused())
}

/// Сущности, которые не мешают лучу к цели: стены, в которых застрял предмет, и стена,
/// на которой висит настенный предмет (`GetPredicate`).
fn ray_exemptions(world: &World, from: MapCoordinates, target: Entity) -> Vec<Entity> {
    let Some(at) = world.map_coordinates(target) else {
        return Vec::new();
    };
    let mut ignored = Vec::new();
    let collidable_item = world.get::<Item>(target).is_some()
        && world
            .get::<Physics>(target)
            .is_some_and(|body| body.can_collide);
    if collidable_item {
        // Предмет, застрявший в стене, всё равно можно поднять.
        let Some(grid) = world.find_grid_at(at) else {
            return ignored;
        };
        let Some(placement) = world.world_transform(grid) else {
            return ignored;
        };
        let local = placement.to_local(at.position);
        let probe = astrocore_math::Box2::centered(local, astrocore_math::Vec2::splat(0.02));
        for other in world.entities_intersecting(grid, at.z - placement.level, probe) {
            let blocks = world
                .get::<Physics>(other)
                .is_some_and(|body| body.can_collide)
                && world.get::<Fixtures>(other).is_some_and(|fixtures| {
                    fixtures.fixtures.values().any(|f| {
                        f.layer
                            .intersects(CollisionGroups(groups::IN_RANGE_UNOBSTRUCTED))
                    })
                });
            if other != target && blocks {
                ignored.push(other);
            }
        }
    } else if let Some(mount) = world.get::<WallMount>(target) {
        let ignore_anchored = if mount.arc.0 >= std::f32::consts::TAU {
            true
        } else {
            let rotation = world.world_transform(target).map_or(0.0, |t| t.rotation.0);
            let angle = Angle::from_world_vec(from.position - at.position);
            let delta = (mount.direction.0 + rotation - angle.0).rem_euclid(std::f32::consts::TAU);
            delta < mount.arc.0 / 2.0 || std::f32::consts::TAU - delta < mount.arc.0 / 2.0
        };
        if ignore_anchored
            && let Some(tile) = world.tile_at(at)
            && let Some(grid) = world.get::<MapGrid>(tile.grid)
        {
            ignored.extend_from_slice(grid.anchored_at(tile.coord));
        }
    }
    ignored
}

/// Взаимодействие на `World`.
pub trait InteractionWorldExt {
    /// Предмет, которым действует сущность: предмет в активной руке.
    fn used_entity(&mut self, user: Entity) -> Option<Entity>;

    /// Цель в пределах досягаемости и без преград (`InRangeUnobstructed`): расстояние меряется
    /// от края тела до края тела, если у обоих есть фикстуры, иначе от центра до центра.
    fn in_range_unobstructed(&mut self, user: Entity, target: Entity, range: f32) -> bool;

    /// Точка карты в пределах досягаемости и без преград.
    fn in_range_unobstructed_point(&self, user: Entity, point: MapCoordinates, range: f32) -> bool;

    /// Цель доступна: не за стенкой контейнера (`IsAccessible`).
    fn is_accessible(&mut self, user: Entity, target: Entity) -> bool;

    /// Клик (`UserInteraction`).
    fn user_interaction(
        &mut self,
        user: Entity,
        click: MapCoordinates,
        target: Option<Entity>,
        alt: bool,
    );

    /// Клик пустой рукой (`InteractHand`).
    fn interact_hand(&mut self, user: Entity, target: Entity);

    /// Клик предметом по цели рядом (`InteractUsing`). `true` — кто-то обработал.
    fn interact_using(
        &mut self,
        user: Entity,
        used: Entity,
        target: Entity,
        click: MapCoordinates,
    ) -> bool;

    /// «Использовать» (`InteractionActivate`). `check_access` — проверять досягаемость и доступ.
    fn interaction_activate(&mut self, user: Entity, target: Entity, check_access: bool) -> bool;

    /// Использовать предмет в руке (`UseInHandInteraction`).
    fn use_in_hand_interaction(&mut self, user: Entity, used: Entity) -> bool;
}

impl InteractionWorldExt for World {
    fn used_entity(&mut self, user: Entity) -> Option<Entity> {
        let mut event = GetUsedEntity { used: None };
        self.raise(user, &mut event);
        event.used
    }

    fn in_range_unobstructed(&mut self, user: Entity, target: Entity, range: f32) -> bool {
        let mut event = InRangeOverride {
            user,
            target,
            handled: false,
            in_range: false,
        };
        self.raise(user, &mut event);
        if event.handled {
            return event.in_range;
        }
        let (Some(origin), Some(other)) =
            (self.map_coordinates(user), self.map_coordinates(target))
        else {
            return false;
        };
        if origin.map != other.map {
            return false;
        }
        let check = should_check_access(self, user);
        let mut range = range;
        if range > 0.0 {
            match self.nearest_distance(user, target) {
                Some(0.0) => return true,
                Some(_) if !check => return true,
                Some(distance) if distance > range => return false,
                // В пределах: луч от центра до центра проверяется на всю длину.
                Some(_) => range = origin.position.distance(other.position),
                None => {}
            }
        }
        if !check {
            return true;
        }
        if origin.z != other.z {
            return false;
        }
        let ignored = ray_exemptions(self, origin, target);
        let ignore =
            |entity: Entity| entity == user || entity == target || ignored.contains(&entity);
        ray_clear(self, origin, other, range, &ignore)
    }

    fn in_range_unobstructed_point(&self, user: Entity, point: MapCoordinates, range: f32) -> bool {
        let Some(origin) = self.map_coordinates(user) else {
            return false;
        };
        if origin.map != point.map {
            return false;
        }
        if !should_check_access(self, user) {
            return true;
        }
        if origin.z != point.z {
            return false;
        }
        ray_clear(self, origin, point, range, &|entity| entity == user)
    }

    fn is_accessible(&mut self, user: Entity, target: Entity) -> bool {
        let mut event = AccessibleOverride {
            user,
            target,
            handled: false,
            accessible: false,
        };
        self.raise(user, &mut event);
        self.raise(target, &mut event);
        if event.handled {
            return event.accessible;
        }
        can_access(self, user, target)
    }

    fn user_interaction(
        &mut self,
        user: Entity,
        click: MapCoordinates,
        target: Option<Entity>,
        alt: bool,
    ) {
        if target.is_some_and(|target| is_deleted(self, target)) {
            return;
        }
        // В режиме боя клик — удар, кроме взятия предмета пустой рукой.
        if !alt
            && crate::combat::CombatWorldExt::is_in_combat_mode(self, user)
            && !crate::combat::combat_mode_can_hand_interact(self, user, target)
        {
            return;
        }
        // Клик по другой карте не засчитывается; стоящий на месте поворачивается к точке.
        let Some(own) = self.map_coordinates(user) else {
            return;
        };
        if own.map != click.map {
            return;
        }
        face(self, user, click);
        if alt {
            // Альтернативное действие — первый альтернативный глагол цели (`AltInteract`).
            if let Some(target) = target {
                crate::verbs::alt_interact(self, user, target);
            }
            return;
        }
        if !self.can_interact(user, target) {
            return;
        }
        let check = should_check_access(self, user);
        if check && target.is_some_and(|target| !self.is_accessible(user, target)) {
            return;
        }
        let in_range = match target {
            _ if !check => true,
            None => self.in_range_unobstructed_point(user, click, INTERACTION_RANGE),
            Some(target) => self.in_range_unobstructed(user, target, INTERACTION_RANGE),
        };

        let Some(used) = self.used_entity(user) else {
            if in_range && let Some(target) = target {
                self.interact_hand(user, target);
            }
            return;
        };
        if target == Some(used) {
            self.use_in_hand_interaction(user, used);
            return;
        }
        if !self.can_use_held_entity(user, used) {
            return;
        }
        if in_range && let Some(target) = target {
            self.interact_using(user, used, target, click);
            return;
        }
        interact_using_ranged(self, user, used, target, click, in_range);
    }

    fn interact_hand(&mut self, user: Entity, target: Entity) {
        if is_deleted(self, user) || is_deleted(self, target) {
            return;
        }
        let complex = self.can_complex_interact(user);
        if !complex {
            activate(self, user, target, complex);
            return;
        }
        let mut before = BeforeInteractHand {
            target,
            handled: false,
        };
        self.raise(user, &mut before);
        if before.handled {
            return;
        }
        let mut message = InteractHand {
            user,
            target,
            handled: false,
        };
        self.raise_and_broadcast(target, &mut message);
        let mut user_message = UserInteractHand {
            user,
            target,
            handled: false,
        };
        self.raise_and_broadcast(user, &mut user_message);
        if message.handled {
            contact(self, user, target);
        }
        if message.handled || user_message.handled || is_deleted(self, target) {
            return;
        }
        activate(self, user, target, complex);
    }

    fn interact_using(
        &mut self,
        user: Entity,
        used: Entity,
        target: Entity,
        click: MapCoordinates,
    ) -> bool {
        if is_deleted(self, user) || is_deleted(self, used) || is_deleted(self, target) {
            return false;
        }
        if before_ranged(self, user, used, Some(target), click, true) {
            return true;
        }
        let mut message = InteractUsing {
            user,
            used,
            target,
            click,
            handled: false,
        };
        self.raise_and_broadcast(target, &mut message);
        let mut user_message = UserInteractUsing {
            user,
            used,
            target,
            click,
            handled: false,
        };
        self.raise_and_broadcast(user, &mut user_message);
        if message.handled {
            contact(self, user, used);
            contact(self, user, target);
        }
        if message.handled || user_message.handled {
            return true;
        }
        after_interact(self, user, used, Some(target), click, true)
    }

    fn interaction_activate(&mut self, user: Entity, target: Entity, check_access: bool) -> bool {
        if is_deleted(self, user) || is_deleted(self, target) {
            return false;
        }
        if !self.can_interact(user, Some(target)) {
            return false;
        }
        if check_access
            && (!self.in_range_unobstructed(user, target, INTERACTION_RANGE)
                || !self.is_accessible(user, target))
        {
            return false;
        }
        let complex = self.can_complex_interact(user);
        activate(self, user, target, complex)
    }

    fn use_in_hand_interaction(&mut self, user: Entity, used: Entity) -> bool {
        if is_deleted(self, user) || is_deleted(self, used) {
            return false;
        }
        if !self.can_interact(user, Some(used)) || !self.can_use_held_entity(user, used) {
            return false;
        }
        let mut message = UseInHand {
            user,
            used,
            handled: false,
        };
        self.raise_and_broadcast(used, &mut message);
        if message.handled {
            contact(self, user, used);
            return true;
        }
        // Иначе предмет «используется», как по клавише E.
        if is_deleted(self, used) {
            return false;
        }
        let complex = self.can_complex_interact(user);
        activate(self, user, used, complex)
    }
}

/// Луч от точки до точки свободен от преград маски досягаемости.
fn ray_clear(
    world: &World,
    from: MapCoordinates,
    to: MapCoordinates,
    range: f32,
    ignore: &dyn Fn(Entity) -> bool,
) -> bool {
    let direction = to.position - from.position;
    let mut length = direction.length();
    if range > 0.0 && length > range {
        return false;
    }
    if length < 1e-6 {
        return true;
    }
    length = length.min(MAX_RAYCAST_RANGE);
    let ray = Ray {
        origin: from,
        direction,
        max_distance: length,
        mask: CollisionGroups(groups::IN_RANGE_UNOBSTRUCTED),
    };
    world.intersect_ray(ray, ignore, true).is_empty()
}

/// Цель в том же контейнере, что и действующий, или один держит другого
/// (`IsInSameOrParentContainer`), либо лежит в контейнере, владелец которого открывает к
/// нему доступ (`CanAccessViaStorage`: открытое окно рюкзака).
fn can_access(world: &mut World, user: Entity, target: Entity) -> bool {
    let user_container = world.containing_container(user).cloned();
    let target_container = world.containing_container(target).cloned();
    match (&user_container, &target_container) {
        (None, None) => true,
        (Some(own), _) if own.owner == target => true,
        (_, Some(other)) if other.owner == user => true,
        (Some(own), Some(other)) if own.owner == other.owner && own.id == other.id => true,
        (_, Some(other)) => {
            let mut event = AccessViaContainer {
                user,
                target,
                container: other.id.to_string(),
                accessible: false,
            };
            world.raise(other.owner, &mut event);
            event.accessible
        }
        (Some(_), None) => false,
    }
}

/// Цель лежит в контейнере сущности, на которой поднято событие: может ли действующий
/// дотянуться до неё (`CanAccessViaStorage`). Поднимается на владельце контейнера.
#[derive(Debug, Clone)]
pub struct AccessViaContainer {
    pub user: Entity,
    pub target: Entity,
    pub container: String,
    pub accessible: bool,
}

/// Поворот к точке клика, если сущность не идёт (`ValidateInteractAndFace`).
pub(crate) fn face(world: &mut World, user: Entity, click: MapCoordinates) {
    let moving = world
        .get::<InputMover>(user)
        .is_some_and(|mover| mover.held.direction() != astrocore_math::Vec2::ZERO);
    let (Some(own), Some(placement)) = (world.map_coordinates(user), world.world_transform(user))
    else {
        return;
    };
    let to = click.position - own.position;
    if moving || to.length_squared() < 1e-6 {
        return;
    }
    let wanted = Angle::from_world_vec(to);
    let own_local = world
        .get::<astrocore_map::Transform>(user)
        .map_or(0.0, |transform| transform.local_rotation().0);
    let parent_rotation = placement.rotation.0 - own_local;
    let _ = world.set_local_rotation(user, Angle(wanted.0 - parent_rotation));
}

fn contact(world: &mut World, a: Entity, b: Entity) {
    if a == b || paused(world, a) || paused(world, b) {
        return;
    }
    world.raise(a, &mut ContactInteraction { other: b });
    world.raise(b, &mut ContactInteraction { other: a });
}

/// «Использовать» без проверок: событие на цели, затем на действующем.
fn activate(world: &mut World, user: Entity, target: Entity, complex: bool) -> bool {
    let mut message = ActivateInWorld {
        user,
        target,
        complex,
        handled: false,
    };
    world.raise_and_broadcast(target, &mut message);
    if message.handled {
        contact(world, user, target);
        return true;
    }
    if is_deleted(world, user) || is_deleted(world, target) {
        return false;
    }
    let mut user_message = UserActivateInWorld {
        user,
        target,
        complex,
        handled: false,
    };
    world.raise_and_broadcast(user, &mut user_message);
    if user_message.handled {
        contact(world, user, target);
    }
    user_message.handled
}

fn before_ranged(
    world: &mut World,
    user: Entity,
    used: Entity,
    target: Option<Entity>,
    click: MapCoordinates,
    can_reach: bool,
) -> bool {
    let mut message = BeforeRangedInteract {
        user,
        used,
        target,
        click,
        can_reach,
        handled: false,
    };
    world.raise(used, &mut message);
    if message.handled {
        contact(world, user, used);
    }
    message.handled
}

fn interact_using_ranged(
    world: &mut World,
    user: Entity,
    used: Entity,
    target: Option<Entity>,
    click: MapCoordinates,
    in_range: bool,
) {
    if before_ranged(world, user, used, target, click, in_range) {
        return;
    }
    if let Some(target) = target {
        let mut message = RangedInteract {
            user,
            used,
            target,
            click,
            handled: false,
        };
        world.raise_and_broadcast(target, &mut message);
        if message.handled {
            contact(world, user, used);
            return;
        }
    }
    after_interact(world, user, used, target, click, in_range);
}

fn after_interact(
    world: &mut World,
    user: Entity,
    used: Entity,
    target: Option<Entity>,
    click: MapCoordinates,
    can_reach: bool,
) -> bool {
    let mut message = AfterInteract {
        user,
        used,
        target,
        click,
        can_reach,
        handled: false,
    };
    world.raise(used, &mut message);
    if message.handled {
        contact(world, user, used);
        if can_reach && let Some(target) = target {
            contact(world, user, target);
        }
        return true;
    }
    let Some(target) = target else {
        return false;
    };
    if is_deleted(world, target) {
        return false;
    }
    let mut message = AfterInteractUsing {
        user,
        used,
        target,
        click,
        can_reach,
        handled: false,
    };
    world.raise(target, &mut message);
    if message.handled {
        contact(world, user, used);
        if can_reach {
            contact(world, user, target);
        }
    }
    message.handled
}

pub const INTERACTION_RATE_LIMIT_COUNT: CVarDef<i32> = CVarDef::new(
    "interaction.rate_limit_count",
    5,
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Сколько взаимодействий игрок может сделать за период.",
);
pub const INTERACTION_RATE_LIMIT_PERIOD: CVarDef<f32> = CVarDef::new(
    "interaction.rate_limit_period",
    0.5,
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Период подсчёта взаимодействий, секунды.",
);
pub const INTERACTION_RATE_LIMIT_ANNOUNCE_ADMINS_DELAY: CVarDef<i32> = CVarDef::new(
    "interaction.rate_limit_announce_admins_delay",
    120,
    CVarFlags::SERVERONLY,
    "Наименьшая пауза между оповещениями администраторов о превышении, секунды; \
     отрицательная — не оповещать.",
);
/// Ключ ограничения частоты взаимодействий (`RateLimitKey`).
pub const INTERACTION_RATE_LIMIT_KEY: &str = "Interaction";

/// Оповещение администраторов (`RateLimitAlertAdmins`).
fn rate_limit_alert_admins(world: &mut World, session: Entity) {
    let name = world
        .get::<astrocore_net::Session>(session)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let text = crate::examine::loc(
        world,
        "interaction-rate-limit-admin-announcement",
        &[("player", astrocore_loc::LocArg::from(name))],
    );
    crate::chat::send_admin_alert(world, &text);
}

/// Учесть взаимодействие игрока (конец `ValidateClientInput`): короткий период и большое
/// число — ловит изменённые клиенты, а не быстрые нажатия. Ввод без сессии (сервер,
/// тесты) не считается.
pub fn interaction_rate_allows(world: &mut World, args: &InputArgs) -> bool {
    match args.session {
        Some(session) => {
            crate::rate_limit::count_action(world, session, INTERACTION_RATE_LIMIT_KEY)
                == crate::rate_limit::RateLimitStatus::Allowed
        }
        None => true,
    }
}

/// Проверка команды клиента: у сессии есть тело, точка на карте, цель существует, частота
/// не превышена (`ValidateClientInput`).
fn client_input(
    world: &mut World,
    args: &InputArgs,
) -> Option<(Entity, MapCoordinates, Option<Entity>)> {
    let user = args.player.filter(|user| world.is_alive(*user))?;
    let click = args.coordinates?;
    world.map_entity(click.map)?;
    let target = args.target.filter(|target| !is_deleted(world, *target));
    if !interaction_rate_allows(world, args) {
        return None;
    }
    Some((user, click, target))
}

fn bind_inputs(world: &mut World) {
    world.bind_input(functions::USE, |world, args| {
        if args.state != BoundKeyState::Down {
            return false;
        }
        if let Some((user, click, target)) = client_input(world, args) {
            world.user_interaction(user, click, target, false);
        }
        false
    });
    world.bind_input(functions::ALT_ACTIVATE_IN_WORLD, |world, args| {
        if args.state != BoundKeyState::Down {
            return false;
        }
        if let Some((user, click, target)) = client_input(world, args) {
            world.user_interaction(user, click, target, true);
        }
        false
    });
    world.bind_input(functions::ACTIVATE_IN_WORLD, |world, args| {
        if args.state != BoundKeyState::Down {
            return false;
        }
        if let Some((user, _, Some(target))) = client_input(world, args) {
            let check = should_check_access(world, user);
            world.interaction_activate(user, target, check);
        }
        false
    });
    world.bind_input(functions::USE_ITEM_IN_HAND, |world, args| {
        if args.state != BoundKeyState::Down {
            return false;
        }
        let Some(user) = args.player.filter(|user| world.is_alive(*user)) else {
            return false;
        };
        if let Some(used) = world.used_entity(user) {
            world.use_in_hand_interaction(user, used);
        }
        false
    });
}

/// Подключает взаимодействие: компоненты на обеих сторонах, функции ввода — на сервере.
pub fn install(world: &mut World, side: crate::Side) {
    world.add_plugin(astrocore_input::InputPlugin);
    world.register_data_component::<WallMount>();
    world.register_net_serde::<WallMount>();
    world.register_cvar(&INTERACTION_RATE_LIMIT_COUNT);
    world.register_cvar(&INTERACTION_RATE_LIMIT_PERIOD);
    world.register_cvar(&INTERACTION_RATE_LIMIT_ANNOUNCE_ADMINS_DELAY);
    if side.is_server() {
        crate::rate_limit::register_rate_limit(
            world,
            INTERACTION_RATE_LIMIT_KEY,
            crate::rate_limit::RateLimitRegistration {
                period: INTERACTION_RATE_LIMIT_PERIOD,
                count: INTERACTION_RATE_LIMIT_COUNT,
                player_limited: None,
                admin_announce_delay: Some(INTERACTION_RATE_LIMIT_ANNOUNCE_ADMINS_DELAY),
                admin_announce: Some(rate_limit_alert_admins),
                log_type: crate::admin_logs::LogType::RateLimited,
            },
        );
        bind_inputs(world);
    }
}

#[cfg(test)]
mod tests;
