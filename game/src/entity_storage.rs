//! Шкафы, ящики и другие хранилища целых сущностей: открываются и закрываются, при закрытии
//! забирают то, что лежит рядом. Образец — `Content.Shared/Storage` SS14 @ 2f4a2aad
//! (`EntityStorageComponent`, `SharedEntityStorageSystem`).

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{
    ComponentInit, EventWorldExt, GameTime, GameWorldExt, LifecycleWorldExt, RegistryWorldExt,
    Update,
};
use astrocore_map::{
    ContainerKind, ContainerManager, ContainerWorldExt, EntGotRemovedFromContainer,
    EntityCoordinates, MapCoordinates, MapWorldExt, TransformWorldExt,
};
use astrocore_math::{Box2, Vec2};
use astrocore_movement::{InputMover, MoveButtons};
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use astrocore_physics::{
    BodyType, CollisionGroups, Fixtures, Physics, PhysicsWorldExt, Pose, Ray, RayWorldExt,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::content::groups;
use crate::hands::Hands;
use crate::interaction::{ActivateInWorld, WallMount};
use crate::popups::{PopupType, PopupWorldExt};
use crate::verbs::{GetVerbs, Verb, VerbKind};
use crate::whitelist::{EntityWhitelist, check_both};

/// Контейнер содержимого (`SharedEntityStorageSystem.ContainerName`).
pub const ENTITY_STORAGE_CONTAINER: &str = "entity_storage";
/// Ключ внешнего вида: открыто (`StorageVisuals.Open`).
pub const VISUALS_OPEN: &str = "enum.StorageVisuals.Open";
/// Ключ внешнего вида: внутри что-то есть (`StorageVisuals.HasContents`).
pub const VISUALS_HAS_CONTENTS: &str = "enum.StorageVisuals.HasContents";

fn yes() -> bool {
    true
}

fn max_size() -> f32 {
    1.0
}

fn open_attempt_delay() -> f64 {
    0.5
}

fn masks_to_remove() -> i32 {
    groups::MID | groups::HIGH | groups::LOW
}

fn capacity() -> usize {
    30
}

fn entering_range() -> f32 {
    0.18
}

fn close_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Effects/closetclose.ogg")
}

fn open_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Effects/closetopen.ogg")
}

fn default_whitelist() -> Option<EntityWhitelist> {
    Some(EntityWhitelist {
        components: Some(vec!["MobState".into(), "Item".into()]),
        ..EntityWhitelist::default()
    })
}

/// Хранилище сущностей. В данных — компонент `EntityStorage`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityStorage {
    /// Наибольшая сторона сущности, которая влезает.
    #[serde(default = "max_size")]
    pub max_size: f32,
    /// Пауза между попытками открыть изнутри, секунды.
    #[serde(default = "open_attempt_delay")]
    pub internal_open_attempt_delay: f64,
    #[serde(skip)]
    pub next_internal_open_attempt: f64,
    /// Слои столкновений, которые открытое хранилище снимает с себя.
    #[serde(default = "masks_to_remove")]
    pub masks_to_remove: i32,
    #[serde(default)]
    pub removed_masks: i32,
    #[serde(default = "capacity")]
    pub capacity: usize,
    #[serde(default)]
    pub is_collidable_when_open: bool,
    #[serde(default = "yes")]
    pub open_on_move: bool,
    /// Где выкладывается и откуда собирается содержимое.
    #[serde(default)]
    pub entering_offset: Vec2,
    #[serde(default = "entering_range")]
    pub entering_range: f32,
    #[serde(default)]
    pub show_contents: bool,
    #[serde(default = "yes")]
    pub occludes_light: bool,
    #[serde(default)]
    pub delete_contents_on_destruction: bool,
    #[serde(default = "yes")]
    pub airtight: bool,
    #[serde(default)]
    pub open: bool,
    #[serde(default = "close_sound")]
    pub close_sound: SoundSpecifier,
    #[serde(default = "open_sound")]
    pub open_sound: SoundSpecifier,
    #[serde(default = "default_whitelist")]
    pub whitelist: Option<EntityWhitelist>,
    #[serde(default)]
    pub blacklist: Option<EntityWhitelist>,
}

impl Default for EntityStorage {
    fn default() -> Self {
        Self {
            max_size: max_size(),
            internal_open_attempt_delay: open_attempt_delay(),
            next_internal_open_attempt: 0.0,
            masks_to_remove: masks_to_remove(),
            removed_masks: 0,
            capacity: capacity(),
            is_collidable_when_open: false,
            open_on_move: true,
            entering_offset: Vec2::ZERO,
            entering_range: entering_range(),
            show_contents: false,
            occludes_light: true,
            delete_contents_on_destruction: false,
            airtight: true,
            open: false,
            close_sound: close_sound(),
            open_sound: open_sound(),
            whitelist: default_whitelist(),
            blacklist: None,
        }
    }
}

/// Сущность лежит в хранилище сущностей (`InsideEntityStorageComponent`).
#[derive(Component, Debug, Clone, Copy)]
pub struct InsideEntityStorage {
    pub storage: Entity,
}

/// Можно ли открыть (`StorageOpenAttemptEvent`). На хранилище.
#[derive(Debug, Clone, Copy)]
pub struct StorageOpenAttempt {
    pub user: Entity,
    pub silent: bool,
    pub cancelled: bool,
}

/// Можно ли закрыть (`StorageCloseAttemptEvent`). На хранилище.
#[derive(Debug, Clone, Copy)]
pub struct StorageCloseAttempt {
    pub user: Option<Entity>,
    pub cancelled: bool,
}

/// Хранилище открылось (`StorageAfterOpenEvent`).
#[derive(Debug, Clone, Copy)]
pub struct StorageAfterOpen {
    pub user: Option<Entity>,
}

/// Хранилище закрывается: можно поменять, что попадёт внутрь (`StorageBeforeCloseEvent`).
#[derive(Debug, Clone)]
pub struct StorageBeforeClose {
    pub user: Option<Entity>,
    pub contents: Vec<Entity>,
    /// Попадут внутрь без проверок.
    pub bypass_checks: Vec<Entity>,
}

/// Хранилище закрылось (`StorageAfterCloseEvent`).
#[derive(Debug, Clone, Copy)]
pub struct StorageAfterClose {
    pub user: Option<Entity>,
}

/// Можно ли положить сущность (`InsertIntoEntityStorageAttemptEvent`). На сущности.
#[derive(Debug, Clone, Copy)]
pub struct InsertIntoEntityStorageAttempt {
    pub storage: Entity,
    pub item: Entity,
    pub cancelled: bool,
}

/// Можно ли положить в это хранилище (`EntityStorageInsertedIntoAttemptEvent`). На хранилище.
#[derive(Debug, Clone, Copy)]
pub struct EntityStorageInsertedIntoAttempt {
    pub storage: Entity,
    pub item: Entity,
    pub cancelled: bool,
}

/// Хранилища сущностей на `World`.
pub trait EntityStorageWorldExt {
    fn is_open(&self, storage: Entity) -> bool;
    fn toggle_open(&mut self, user: Entity, storage: Entity);
    fn try_open_storage(&mut self, user: Entity, storage: Entity, silent: bool) -> bool;
    fn try_close_storage(&mut self, storage: Entity, user: Option<Entity>) -> bool;
    fn can_open(&mut self, user: Entity, storage: Entity, silent: bool) -> bool;
    fn can_close(&mut self, storage: Entity, user: Option<Entity>) -> bool;
    /// Открыть без проверок (`OpenStorage`): содержимое выкладывается.
    fn open_storage(&mut self, storage: Entity, user: Option<Entity>);
    /// Закрыть без проверок (`CloseStorage`): лежащее рядом попадает внутрь.
    fn close_storage(&mut self, storage: Entity, user: Option<Entity>);
    /// Выложить содержимое (`EmptyContents`).
    fn empty_contents(&mut self, storage: Entity);
    /// Положить внутрь; в открытое — рядом (`Insert`).
    fn entity_storage_insert(&mut self, item: Entity, storage: Entity) -> bool;
    /// Вынуть наружу (`Remove`).
    fn entity_storage_remove(&mut self, item: Entity, storage: Entity) -> bool;
    /// Влезет ли сущность (`CanInsert`).
    fn can_insert_entity_storage(&mut self, item: Entity, storage: Entity) -> bool;
}

fn contents(world: &World, storage: Entity) -> Vec<Entity> {
    world
        .container(storage, ENTITY_STORAGE_CONTAINER)
        .map(|container| container.entities().to_vec())
        .unwrap_or_default()
}

fn contains(world: &World, storage: Entity, entity: Entity) -> bool {
    world
        .container(storage, ENTITY_STORAGE_CONTAINER)
        .is_some_and(|container| container.contains(entity))
}

/// Место выкладывания: хранилище, сдвинутое на `enteringOffset` с учётом поворота.
fn entering_point(world: &World, storage: Entity, offset: Vec2) -> Option<EntityCoordinates> {
    let mut coordinates = world.coordinates(storage)?;
    let rotation = world
        .get::<astrocore_map::Transform>(storage)?
        .local_rotation();
    coordinates.position += rotation.rotate(offset);
    Some(coordinates)
}

/// Наибольшая сторона фигуры сущности без поворота (`GetAABBNoContainer`).
fn aabb_size(world: &World, entity: Entity) -> Vec2 {
    let pose = Pose {
        position: Vec2::ZERO,
        rotation: astrocore_math::Angle::ZERO,
    };
    world
        .get::<Fixtures>(entity)
        .and_then(|fixtures| {
            fixtures
                .fixtures
                .values()
                .map(|fixture| fixture.shape.at(&pose).aabb())
                .reduce(|a, b| a.union(&b))
        })
        .map_or(Vec2::ZERO, |bounds| bounds.size())
}

/// Незакреплённые сущности рядом с точкой (`GetEntitiesInRange` с `Approximate | Dynamic |
/// Sundries`).
fn entities_near(world: &World, at: MapCoordinates, range: f32) -> Vec<Entity> {
    let (root, local, z) = match world.find_grid_at(at) {
        Some(grid) => match world.world_transform(grid) {
            Some(placement) => (
                grid,
                placement.to_local(at.position),
                at.z - placement.level,
            ),
            None => return Vec::new(),
        },
        None => match world.map_entity(at.map) {
            Some(map) => (map, at.position, at.z),
            None => return Vec::new(),
        },
    };
    let probe = Box2::centered(local, Vec2::splat(range * 2.0));
    world
        .entities_intersecting(root, z, probe)
        .into_iter()
        .filter(|entity| {
            let anchored = world
                .get::<astrocore_map::Transform>(*entity)
                .is_some_and(|transform| transform.anchored());
            let static_body = world
                .get::<Physics>(*entity)
                .is_some_and(|body| body.body_type == BodyType::Static);
            !anchored && !static_body
        })
        .collect()
}

/// Открытое хранилище не мешает проходу; закрытое возвращает свои слои (`ModifyComponents`).
fn modify_components(world: &mut World, storage: Entity) {
    let Some(component) = world.get::<EntityStorage>(storage).cloned() else {
        return;
    };
    if !component.is_collidable_when_open
        && let Some(mut fixtures) = world.get_mut::<Fixtures>(storage)
        && let Some(fixture) = fixtures.fixtures.values_mut().next()
    {
        let removed = if component.open {
            let removed = fixture.layer.0 & component.masks_to_remove;
            fixture.layer = CollisionGroups(fixture.layer.0 & !component.masks_to_remove);
            removed
        } else {
            fixture.layer = CollisionGroups(fixture.layer.0 | component.removed_masks);
            0
        };
        if let Some(mut storage) = world.get_mut::<EntityStorage>(storage) {
            storage.removed_masks = removed;
        }
        world.wake_body(storage);
    }
    let has_contents = !contents(world, storage).is_empty();
    world.set_appearance(storage, VISUALS_OPEN, component.open);
    world.set_appearance(storage, VISUALS_HAS_CONTENTS, has_contents);
}

impl EntityStorageWorldExt for World {
    fn is_open(&self, storage: Entity) -> bool {
        self.get::<EntityStorage>(storage).is_some_and(|s| s.open)
    }

    fn toggle_open(&mut self, user: Entity, storage: Entity) {
        if self.get::<EntityStorage>(storage).is_none() {
            return;
        }
        if self.is_open(storage) {
            self.try_close_storage(storage, Some(user));
        } else {
            self.try_open_storage(user, storage, false);
        }
    }

    fn try_open_storage(&mut self, user: Entity, storage: Entity, silent: bool) -> bool {
        if !self.can_open(user, storage, silent) {
            return false;
        }
        self.open_storage(storage, Some(user));
        true
    }

    fn try_close_storage(&mut self, storage: Entity, user: Option<Entity>) -> bool {
        if !self.can_close(storage, user) {
            return false;
        }
        self.close_storage(storage, user);
        true
    }

    fn can_open(&mut self, user: Entity, storage: Entity, silent: bool) -> bool {
        let Some(component) = self.get::<EntityStorage>(storage).cloned() else {
            return false;
        };
        let inside = contains(self, storage, user);
        if !inside && self.get::<Hands>(user).is_none() {
            return false;
        }
        // Выкладывать некуда: точка выкладывания в стене.
        if component.entering_offset != Vec2::ZERO && self.get::<WallMount>(storage).is_none() {
            let from = self.map_coordinates(storage);
            let to = entering_point(self, storage, component.entering_offset)
                .and_then(|point| self.to_map_coordinates(point));
            if let (Some(from), Some(to)) = (from, to) {
                let direction = to.position - from.position;
                let ray = Ray {
                    origin: from,
                    direction,
                    max_distance: direction.length(),
                    mask: CollisionGroups(groups::IMPASSABLE | groups::MID),
                };
                let blocked = direction.length() > 0.0
                    && !self
                        .intersect_ray(ray, &|entity| entity == storage, true)
                        .is_empty();
                if blocked {
                    if !silent {
                        let text = crate::examine::loc(
                            self,
                            "entity-storage-component-cannot-open-no-space",
                            &[],
                        );
                        self.popup_entity(&text, storage, PopupType::Small);
                    }
                    return false;
                }
            }
        }
        let mut event = StorageOpenAttempt {
            user,
            silent,
            cancelled: false,
        };
        self.raise_and_broadcast(storage, &mut event);
        !event.cancelled
    }

    fn can_close(&mut self, storage: Entity, user: Option<Entity>) -> bool {
        let mut event = StorageCloseAttempt {
            user,
            cancelled: false,
        };
        self.raise(storage, &mut event);
        !event.cancelled
    }

    fn open_storage(&mut self, storage: Entity, user: Option<Entity>) {
        let Some(mut component) = self.get_mut::<EntityStorage>(storage) else {
            return;
        };
        if component.open {
            return;
        }
        component.open = true;
        let sound = component.open_sound.clone();
        self.empty_contents(storage);
        modify_components(self, storage);
        self.play_entity(&sound, storage, None);
        self.raise(storage, &mut StorageAfterOpen { user });
    }

    fn close_storage(&mut self, storage: Entity, user: Option<Entity>) {
        let Some(mut component) = self.get_mut::<EntityStorage>(storage) else {
            return;
        };
        if !component.open {
            return;
        }
        component.open = false;
        let component = component.clone();
        let mut found = entering_point(self, storage, component.entering_offset)
            .and_then(|point| self.to_map_coordinates(point))
            .map(|at| entities_near(self, at, component.entering_range))
            .unwrap_or_default();
        found.retain(|entity| *entity != storage);
        let mut event = StorageBeforeClose {
            user,
            contents: found,
            bypass_checks: Vec::new(),
        };
        self.raise(storage, &mut event);
        for entity in event.contents {
            if !event.bypass_checks.contains(&entity)
                && !self.can_insert_entity_storage(entity, storage)
            {
                continue;
            }
            if entity == storage || !self.entity_storage_insert(entity, storage) {
                continue;
            }
            if contents(self, storage).len() >= component.capacity {
                break;
            }
        }
        modify_components(self, storage);
        self.play_entity(&component.close_sound, storage, None);
        self.raise(storage, &mut StorageAfterClose { user });
    }

    fn empty_contents(&mut self, storage: Entity) {
        for entity in contents(self, storage) {
            self.entity_storage_remove(entity, storage);
        }
    }

    fn entity_storage_insert(&mut self, item: Entity, storage: Entity) -> bool {
        let Some(component) = self.get::<EntityStorage>(storage).cloned() else {
            return false;
        };
        if component.open {
            // В открытое хранилище сущность просто ставится рядом (`DropNextTo`).
            if let Some(at) = self.coordinates(storage) {
                if self.containing_container(item).is_some() {
                    return self.remove_from_container(item, Some(at)).is_ok();
                }
                return self.set_coordinates(item, at, None).is_ok();
            }
            return false;
        }
        if self
            .insert_into_container(item, storage, ENTITY_STORAGE_CONTAINER)
            .is_err()
        {
            return false;
        }
        self.add_comp(item, InsideEntityStorage { storage });
        true
    }

    fn entity_storage_remove(&mut self, item: Entity, storage: Entity) -> bool {
        let Some(component) = self.get::<EntityStorage>(storage).cloned() else {
            return false;
        };
        let destination = entering_point(self, storage, component.entering_offset);
        if self.remove_from_container(item, destination).is_err() {
            return false;
        }
        // Хранилище само лежит в контейнере: содержимое переходит туда же.
        if let Some(outer) = self.containing_container(storage).cloned()
            && self
                .insert_into_container(item, outer.owner, &outer.id)
                .is_ok()
        {
            return true;
        }
        self.remove_comp::<InsideEntityStorage>(item);
        true
    }

    fn can_insert_entity_storage(&mut self, item: Entity, storage: Entity) -> bool {
        let Some(component) = self.get::<EntityStorage>(storage).cloned() else {
            return false;
        };
        if component.open {
            return true;
        }
        if contents(self, storage).len() >= component.capacity {
            return false;
        }
        let size = aabb_size(self, item);
        if component.max_size < size.x || component.max_size < size.y {
            return false;
        }
        let mut attempt = InsertIntoEntityStorageAttempt {
            storage,
            item,
            cancelled: false,
        };
        self.raise(item, &mut attempt);
        if attempt.cancelled {
            return false;
        }
        let mut attempt = EntityStorageInsertedIntoAttempt {
            storage,
            item,
            cancelled: false,
        };
        self.raise(storage, &mut attempt);
        if attempt.cancelled {
            return false;
        }
        check_both(
            self,
            item,
            component.blacklist.as_ref(),
            component.whitelist.as_ref(),
        )
    }
}

/// Сидящий внутри пытается выйти, когда жмёт движение (`ContainerRelayMovementEntityEvent`).
fn relay_movement(world: &mut World) {
    let now = world.resource::<GameTime>().cur_time().as_secs_f64();
    let pressing: Vec<(Entity, Entity)> = world
        .query::<(Entity, &InsideEntityStorage, &InputMover)>()
        .iter(world)
        .filter(|(_, _, mover)| mover.can_move && mover.held.0 & !MoveButtons::WALK.0 != 0)
        .map(|(entity, inside, _)| (entity, inside.storage))
        .collect();
    for (entity, storage) in pressing {
        let Some(mut component) = world.get_mut::<EntityStorage>(storage) else {
            continue;
        };
        if now < component.next_internal_open_attempt {
            continue;
        }
        component.next_internal_open_attempt = now + component.internal_open_attempt_delay;
        if component.open_on_move {
            world.try_open_storage(entity, storage, false);
        }
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<EntityStorage>();
    world.register_net_serde::<EntityStorage>();
    world.subscribe::<EntityStorage, ComponentInit>(|world, storage, _| {
        world.ensure_container(storage, ENTITY_STORAGE_CONTAINER, ContainerKind::List);
        let Some(component) = world.get::<EntityStorage>(storage).cloned() else {
            return;
        };
        if let Some(mut manager) = world.get_mut::<ContainerManager>(storage)
            && let Some(container) = manager.containers.get_mut(ENTITY_STORAGE_CONTAINER)
        {
            container.show_ents = component.show_contents;
            container.occludes_light = component.occludes_light;
        }
    });
    if !side.is_server() {
        return;
    }
    world.subscribe::<EntityStorage, astrocore_ecs::MapInit>(|world, storage, _| {
        let open = world.is_open(storage);
        world.set_appearance(storage, VISUALS_OPEN, open);
    });
    world.subscribe::<EntityStorage, ActivateInWorld>(|world, storage, event| {
        if event.handled || !event.complex {
            return;
        }
        event.handled = true;
        world.toggle_open(event.user, storage);
    });
    world.subscribe::<EntityStorage, GetVerbs>(|world, storage, event| {
        if event.kind != VerbKind::Interaction || !event.can_access || !event.can_interact {
            return;
        }
        if !world.can_open(event.user, storage, true) {
            return;
        }
        let (key, icon) = if world.is_open(storage) {
            (
                "verb-common-close",
                "/Textures/Interface/VerbIcons/close.svg.192dpi.png",
            )
        } else {
            (
                "verb-common-open",
                "/Textures/Interface/VerbIcons/open.svg.192dpi.png",
            )
        };
        let user = event.user;
        let text = crate::examine::loc(world, key, &[]);
        let mut verb = Verb::new(VerbKind::Interaction, text, move |world| {
            world.toggle_open(user, storage)
        });
        verb.icon = Some(icon.to_owned());
        event.verbs.push(verb);
    });
    world.subscribe::<InsideEntityStorage, EntGotRemovedFromContainer>(|world, entity, event| {
        let ours = world
            .get::<InsideEntityStorage>(entity)
            .is_some_and(|inside| inside.storage == event.owner);
        if ours {
            world.remove_comp::<InsideEntityStorage>(entity);
        }
    });
    world.add_game_systems(Update, relay_movement);
}

#[cfg(test)]
mod tests;
