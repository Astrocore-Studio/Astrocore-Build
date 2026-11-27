//! Разум игрока: сущность, связывающая пользователя с телом и с сущностью, которую он
//! посещает (призрак с возможностью вернуться). Создание, перенос в другое тело, посещение
//! и возвращение, стирание, смена владельца, управление чужим телом; тело без разума после
//! удаления становится призраком; осмотр («кататоник», «SSD», «мёртв»). Образец —
//! `SharedMindSystem`, `MindSystem`, `MindComponent`, `MindContainerComponent`,
//! `VisitingMindComponent`, `MindExamineSystem`, `IsDeadICSystem` SS14 @ 2f4a2aad (ADR-0013).
//!
//! Нет пока: ролей разума и целей (фаза 7), переноса разума при разрыве тела
//! (`TransferMindOnGib`), команд `mindinfo`, `makesentient`, `rename`.

use std::collections::HashMap;
use std::time::Duration;

use astrocore_ecs::{
    EntityTerminating, EventWorldExt, GameTime, LifecycleWorldExt, MetaData, RegistryWorldExt,
};
use astrocore_loc::{EntityNameWorldExt, LocArg};
use astrocore_map::{EntityCoordinates, MapWorldExt, SpawnWorldExt, TransformWorldExt};
use astrocore_net::{NetUserId, Session, SessionWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::examine::{Examined, loc};
use crate::mobs::MobStateWorldExt;

// Компоненты.

/// Разум (`MindComponent`). Живёт на отдельной сущности без положения.
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct Mind {
    pub user_id: Option<NetUserId>,
    pub original_owner_user_id: Option<NetUserId>,
    /// Тело.
    pub owned_entity: Option<Entity>,
    /// Первое тело (`OriginalOwnedEntity`).
    pub original_owned_entity: Option<Entity>,
    /// Посещаемая сущность: призрак, из которого можно вернуться.
    pub visiting_entity: Option<Entity>,
    pub character_name: Option<String>,
    pub time_of_death: Option<Duration>,
    pub prevent_ghosting: bool,
    pub prevent_suicide: bool,
    /// Роль наблюдателя (`MindRoleObserver`) — до системы ролей (ADR-0013).
    pub observer: bool,
}

impl Mind {
    /// Где сейчас игрок: посещаемая сущность или тело (`CurrentEntity`).
    pub fn current_entity(&self) -> Option<Entity> {
        self.visiting_entity.or(self.owned_entity)
    }
}

/// Носитель разума (`MindContainerComponent`). В данных — `MindContainer`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MindContainer {
    #[serde(skip)]
    pub mind: Option<Entity>,
    #[serde(skip)]
    pub last_mind: Option<Entity>,
    /// Без разума после удаления — призрак.
    pub ghost_on_shutdown: bool,
}

impl Default for MindContainer {
    fn default() -> Self {
        Self {
            mind: None,
            last_mind: None,
            ghost_on_shutdown: true,
        }
    }
}

impl MindContainer {
    pub fn has_mind(&self) -> bool {
        self.mind.is_some()
    }
}

/// Сущность, которую посещает разум (`VisitingMindComponent`).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct VisitingMind {
    pub mind: Option<Entity>,
}

/// Осматривающий видит состояние разума (`MindExaminableComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MindExaminable {}

/// Мёртв по смыслу игры (`IsDeadICComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IsDeadIc {}

/// Пользователь → разум (`UserMinds`). Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct Minds {
    pub user_minds: HashMap<NetUserId, Entity>,
}

// События.

/// Разум добавлен в тело (`MindAddedMessage`): на теле. `MindGotAdded` — на разуме.
#[derive(Debug, Clone, Copy)]
pub struct MindAdded {
    pub mind: Entity,
    pub container: Entity,
    /// Прежнее тело.
    pub transfer_entity: Option<Entity>,
}

/// То же на разуме (`MindGotAddedEvent`).
#[derive(Debug, Clone, Copy)]
pub struct MindGotAdded(pub MindAdded);

/// Разум уходит из тела (`BeforeMindRemovedMessage`).
#[derive(Debug, Clone, Copy)]
pub struct BeforeMindRemoved {
    pub mind: Entity,
    pub container: Entity,
    /// Новое тело.
    pub transfer_entity: Option<Entity>,
}

/// Разум ушёл из тела (`MindRemovedMessage`): на теле. `MindGotRemoved` — на разуме.
#[derive(Debug, Clone, Copy)]
pub struct MindRemoved {
    pub mind: Entity,
    pub container: Entity,
    pub transfer_entity: Option<Entity>,
}

/// То же на разуме (`MindGotRemovedEvent`).
#[derive(Debug, Clone, Copy)]
pub struct MindGotRemoved(pub MindRemoved);

/// Разум перестал посещать сущность (`MindUnvisitedMessage`): на ней и широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct MindUnvisited {
    pub entity: Entity,
}

/// Мёртв ли персонаж по смыслу игры (`GetCharactedDeadIcEvent`).
#[derive(Debug, Clone, Copy, Default)]
pub struct GetCharacterDeadIc {
    pub dead: Option<bool>,
}

// Поиск.

/// Сессия пользователя.
pub fn session_of_user(world: &mut World, user: NetUserId) -> Option<Entity> {
    world
        .query::<(Entity, &Session)>()
        .iter(world)
        .find(|(_, s)| s.user_id == user)
        .map(|(e, _)| e)
}

/// Разум пользователя (`TryGetMind(NetUserId)`).
pub fn mind_of_user(world: &World, user: NetUserId) -> Option<Entity> {
    let mind = *world.get_resource::<Minds>()?.user_minds.get(&user)?;
    world.get::<Mind>(mind).is_some().then_some(mind)
}

/// Разум сущности (`TryGetMind(EntityUid)`): свой или посещающий.
pub fn mind_of(world: &World, entity: Entity) -> Option<Entity> {
    let container = world.get::<MindContainer>(entity)?;
    let mind = match container.mind {
        Some(mind) => mind,
        None => world.get::<VisitingMind>(entity)?.mind?,
    };
    world.get::<Mind>(mind).is_some().then_some(mind)
}

/// Последний разум тела (`TryGetLastMind`).
pub fn last_mind(world: &World, entity: Entity) -> Option<Entity> {
    let mind = world.get::<MindContainer>(entity)?.last_mind?;
    world.get::<Mind>(mind).is_some().then_some(mind)
}

/// Имя персонажа пользователя (`GetCharacterName`).
pub fn character_name(world: &World, user: NetUserId) -> Option<String> {
    let mind = mind_of_user(world, user)?;
    world.get::<Mind>(mind)?.character_name.clone()
}

fn mind(world: &World, mind: Entity) -> Option<Mind> {
    world.get::<Mind>(mind).cloned()
}

fn with_mind(world: &mut World, mind: Entity, f: impl FnOnce(&mut Mind)) {
    if let Some(mut component) = world.get_mut::<Mind>(mind) {
        f(&mut component);
    }
}

fn exists(world: &World, entity: Entity) -> bool {
    world.is_alive(entity)
}

/// Место сущности на гриде или карте (`TryGetMapOrGridCoordinates`): из шкафа или рук —
/// там, где он стоит.
pub fn grid_or_map_coordinates(world: &World, entity: Entity) -> Option<EntityCoordinates> {
    let at = world.map_coordinates(entity)?;
    world.snap_to_grid_or_map(at)
}

// Создание и владелец.

/// Новый разум (`CreateMind`).
pub fn create_mind(world: &mut World, user: Option<NetUserId>, name: Option<&str>) -> Entity {
    let entity = world.spawn_uninitialized();
    if let Some(mut meta) = world.get_mut::<MetaData>(entity) {
        meta.name = Some(match name {
            Some(name) => format!("mind ({name})"),
            None => "mind".into(),
        });
    }
    world.entity_mut(entity).insert(Mind {
        character_name: name.map(str::to_owned),
        ..Mind::default()
    });
    world.initialize_and_start(entity, false);
    set_user_id(world, entity, user);
    entity
}

/// Разум пользователя или новый (`GetOrCreateMind`).
pub fn get_or_create_mind(world: &mut World, user: NetUserId) -> Entity {
    match mind_of_user(world, user) {
        Some(mind) => mind,
        None => create_mind(world, Some(user), None),
    }
}

/// Сменить владельца разума (`SetUserId`): сессия прежнего отсоединяется, новый попадает в
/// текущую сущность разума.
pub fn set_user_id(world: &mut World, mind_id: Entity, user: Option<NetUserId>) {
    let Some(state) = mind(world, mind_id) else {
        return;
    };
    if state.user_id == user {
        return;
    }
    if let Some(old) = state.user_id {
        if let Some(session) = session_of_user(world, old) {
            world.detach_player(session);
        }
        world
            .get_resource_or_insert_with(Minds::default)
            .user_minds
            .remove(&old);
        with_mind(world, mind_id, |m| m.user_id = None);
    }
    let Some(user) = user else {
        return;
    };
    // У пользователя один разум: прежний отпускается.
    if let Some(previous) = mind_of_user(world, user)
        && previous != mind_id
    {
        set_user_id(world, previous, None);
    }
    world
        .get_resource_or_insert_with(Minds::default)
        .user_minds
        .insert(user, mind_id);
    with_mind(world, mind_id, |m| {
        m.user_id = Some(user);
        m.original_owner_user_id.get_or_insert(user);
    });
    if let Some(session) = session_of_user(world, user) {
        match mind(world, mind_id).and_then(|m| m.current_entity()) {
            Some(entity) => world.attach_player(session, entity),
            None => world.detach_player(session),
        }
    }
}

// Перенос и посещение.

/// Посетить сущность (`Visit`): игрок управляет ею, тело остаётся за разумом.
pub fn visit(world: &mut World, mind_id: Entity, entity: Entity) {
    let Some(state) = mind(world, mind_id) else {
        return;
    };
    if state.visiting_entity.is_some() {
        eprintln!("разум уже посещает сущность");
        return;
    }
    if world.get::<VisitingMind>(entity).is_some() {
        eprintln!("сущность уже посещает разум");
        return;
    }
    with_mind(world, mind_id, |m| m.visiting_entity = Some(entity));
    world.entity_mut(entity).insert(VisitingMind {
        mind: Some(mind_id),
    });
    if let Some(user) = state.user_id
        && let Some(session) = session_of_user(world, user)
    {
        world.attach_player(session, entity);
    }
}

fn remove_visiting_entity(world: &mut World, mind_id: Entity) {
    let Some(visiting) = mind(world, mind_id).and_then(|m| m.visiting_entity) else {
        return;
    };
    with_mind(world, mind_id, |m| m.visiting_entity = None);
    if let Ok(mut entity) = world.get_entity_mut(visiting) {
        entity.remove::<VisitingMind>();
    }
    world.raise_and_broadcast(visiting, &mut MindUnvisited { entity: visiting });
}

/// Вернуться из посещаемой сущности в тело (`UnVisit`).
pub fn unvisit(world: &mut World, mind_id: Entity) {
    let Some(state) = mind(world, mind_id) else {
        return;
    };
    let Some(visiting) = state.visiting_entity else {
        return;
    };
    remove_visiting_entity(world, mind_id);
    let Some(user) = state.user_id else {
        return;
    };
    let Some(session) = session_of_user(world, user) else {
        return;
    };
    let attached = world.get::<Session>(session).and_then(|s| s.attached);
    if attached != Some(visiting) {
        return;
    }
    match state.owned_entity {
        Some(owned) if exists(world, owned) => {
            world.attach_player(session, owned);
            let name = world.get::<Session>(session).map(|s| s.name.clone());
            let body = world.entity_name(owned);
            add_admin_log(
                world,
                LogType::Mind,
                LogImpact::Low,
                &format!("{} returned to {body}", name.unwrap_or_default()),
                &[],
                &[user],
            );
        }
        _ => world.detach_player(session),
    }
}

/// Перенести разум в сущность или убрать из тела (`TransferTo`). Без сущности и с
/// `create_ghost` — призрак без права вернуться.
pub fn transfer_to(
    world: &mut World,
    mind_id: Entity,
    entity: Option<Entity>,
    ghost_check_override: bool,
    create_ghost: bool,
) {
    let Some(state) = mind(world, mind_id) else {
        return;
    };
    if entity == state.owned_entity {
        return;
    }
    let mut entity = entity.filter(|e| exists(world, *e));
    let mut already_attached = false;
    if let Some(target) = entity {
        if world.get::<MindContainer>(target).is_none() {
            world.entity_mut(target).insert(MindContainer::default());
        }
        // У цели свой разум — он становится призраком.
        if let Some(other) = mind_of(world, target)
            && other != mind_id
        {
            crate::ghost::on_ghost_attempt(world, other, false, false, false);
        }
        if let Some(session) = world.session_of(target) {
            let ours = state
                .user_id
                .and_then(|u| session_of_user(world, u))
                .is_some_and(|s| s == session);
            if !ours {
                eprintln!("у сущности уже есть игрок");
                return;
            }
            already_attached = true;
        }
    } else if create_ghost {
        let at = state
            .owned_entity
            .filter(|e| exists(world, *e))
            .and_then(|e| grid_or_map_coordinates(world, e))
            .or_else(|| crate::game_ticker::observer_spawn_point(world));
        if let Some(at) = at
            && let Ok(ghost) = world.spawn_attached(crate::game_ticker::OBSERVER_PROTOTYPE, at)
        {
            if world.get::<MindContainer>(ghost).is_none() {
                world.entity_mut(ghost).insert(MindContainer::default());
            }
            crate::ghost::set_can_return_to_body(world, ghost, false);
            entity = Some(ghost);
        }
    }

    let old_entity = state.owned_entity;
    if let Some(old) = old_entity
        && world.get::<MindContainer>(old).is_some()
    {
        let mut before = BeforeMindRemoved {
            mind: mind_id,
            container: old,
            transfer_entity: entity,
        };
        world.raise(old, &mut before);
        if let Some(mut container) = world.get_mut::<MindContainer>(old) {
            container.mind = None;
        }
        with_mind(world, mind_id, |m| m.owned_entity = entity);
        let removed = MindRemoved {
            mind: mind_id,
            container: old,
            transfer_entity: entity,
        };
        world.raise(old, &mut removed.clone());
        world.raise(mind_id, &mut MindGotRemoved(removed));
    }

    let visiting = mind(world, mind_id).and_then(|m| m.visiting_entity);
    if already_attached {
        // Перенос в посещаемую сущность: посещение кончается, вернуться некуда.
        with_mind(world, mind_id, |m| m.visiting_entity = None);
        if let Some(target) = entity {
            if let Ok(mut e) = world.get_entity_mut(target) {
                e.remove::<VisitingMind>();
            }
            crate::ghost::set_can_return_to_body(world, target, false);
        }
    } else if let Some(visiting) = visiting
        && (ghost_check_override || !crate::ghost::can_return_to_body(world, visiting))
    {
        remove_visiting_entity(world, mind_id);
    }

    let state = mind(world, mind_id).unwrap_or_default();
    if let Some(user) = state.user_id
        && !already_attached
        && state.visiting_entity.is_none()
        && let Some(session) = session_of_user(world, user)
    {
        match entity {
            Some(target) => world.attach_player(session, target),
            None => world.detach_player(session),
        }
    }

    if let Some(target) = entity {
        // Прежний разум цели уходит в свою посещаемую сущность.
        let previous = world.get::<MindContainer>(target).and_then(|c| c.mind);
        if let Some(previous) = previous
            && previous != mind_id
        {
            let previous_visiting = mind(world, previous).and_then(|m| m.visiting_entity);
            transfer_to(world, previous, previous_visiting, false, true);
        }
        if let Some(mut container) = world.get_mut::<MindContainer>(target) {
            container.mind = Some(mind_id);
            container.last_mind = Some(mind_id);
        }
        with_mind(world, mind_id, |m| {
            m.owned_entity = Some(target);
            m.original_owned_entity.get_or_insert(target);
        });
        let added = MindAdded {
            mind: mind_id,
            container: target,
            transfer_entity: old_entity,
        };
        world.raise(target, &mut added.clone());
        world.raise(mind_id, &mut MindGotAdded(added));
    }
}

/// Стереть разум (`WipeMind`): без тела и без владельца.
pub fn wipe_mind(world: &mut World, mind_id: Entity) {
    if world.get::<Mind>(mind_id).is_none() {
        return;
    }
    transfer_to(world, mind_id, None, false, false);
    set_user_id(world, mind_id, None);
}

/// Стереть разум пользователя.
pub fn wipe_user_mind(world: &mut World, user: NetUserId) {
    if let Some(mind) = mind_of_user(world, user) {
        wipe_mind(world, mind);
    }
}

/// Стереть все разумы (`WipeAllMinds`).
pub fn wipe_all_minds(world: &mut World) {
    let minds: Vec<Entity> = world
        .get_resource::<Minds>()
        .map(|m| m.user_minds.values().copied().collect())
        .unwrap_or_default();
    for mind in minds {
        wipe_mind(world, mind);
    }
    world
        .get_resource_or_insert_with(Minds::default)
        .user_minds
        .clear();
}

/// Взять под управление сущность (`ControlMob`).
pub fn control_mob(world: &mut World, user: NetUserId, target: Entity) {
    let mind_id = get_or_create_mind(world, user);
    let Some(state) = mind(world, mind_id) else {
        return;
    };
    if state.current_entity() == Some(target) {
        return;
    }
    if state.owned_entity == Some(target) {
        unvisit(world, mind_id);
        return;
    }
    if world.get::<MindContainer>(target).is_none() {
        world.entity_mut(target).insert(MindContainer::default());
    }
    transfer_to(world, mind_id, Some(target), true, true);
}

// Смерть.

/// Тело мертво (`IsCharacterDeadPhysically`): нет тела, не существо или мёртвое существо.
pub fn is_character_dead_physically(world: &World, mind_id: Entity) -> bool {
    let Some(owned) = world.get::<Mind>(mind_id).and_then(|m| m.owned_entity) else {
        return true;
    };
    if world.mob_state(owned).is_none() {
        return true;
    }
    world.is_mob_dead(owned)
}

/// Мёртв по смыслу игры (`IsCharacterDeadIc`).
pub fn is_character_dead_ic(world: &mut World, mind_id: Entity) -> bool {
    if let Some(owned) = world.get::<Mind>(mind_id).and_then(|m| m.owned_entity) {
        let mut event = GetCharacterDeadIc::default();
        world.raise(owned, &mut event);
        if let Some(dead) = event.dead {
            return dead;
        }
    }
    is_character_dead_physically(world, mind_id)
}

// Осмотр.

/// Состояние разума для осмотра (`MindState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MindState {
    None,
    Irrecoverable,
    DeadSsd,
    Dead,
    Catatonic,
    Ssd,
}

/// Состояние разума тела (`RefreshMindStatus`).
pub fn mind_state(world: &mut World, entity: Entity) -> MindState {
    let dead = world.is_mob_dead(entity);
    let user = mind_of(world, entity).and_then(|m| world.get::<Mind>(m)?.user_id);
    let online = user.is_some_and(|u| session_of_user(world, u).is_some());
    match (dead, user, online) {
        (true, None, _) => MindState::Irrecoverable,
        (true, Some(_), false) => MindState::DeadSsd,
        (true, Some(_), true) => MindState::Dead,
        (false, None, _) => MindState::Catatonic,
        (false, Some(_), false) => MindState::Ssd,
        (false, Some(_), true) => MindState::None,
    }
}

fn examine_line(world: &mut World, entity: Entity) -> Option<String> {
    let (key, color) = match mind_state(world, entity) {
        MindState::Irrecoverable => ("comp-mind-examined-dead-and-irrecoverable", "mediumpurple"),
        MindState::DeadSsd => ("comp-mind-examined-dead-and-ssd", "yellow"),
        MindState::Dead => ("comp-mind-examined-dead", "red"),
        MindState::Catatonic => ("comp-mind-examined-catatonic", "mediumpurple"),
        MindState::Ssd => ("comp-mind-examined-ssd", "yellow"),
        MindState::None => return None,
    };
    let arg = world.entity_arg(entity);
    let text = loc(world, key, &[("ent", LocArg::Entity(arg))]);
    Some(format!("[color={color}]{text}[/color]"))
}

// Подписки.

/// Тело удаляется (`OnMindContainerTerminating`): игрок уходит в посещаемую сущность или
/// становится призраком.
fn on_container_terminating(world: &mut World, entity: Entity) {
    let Some(mind_id) = mind_of(world, entity) else {
        return;
    };
    let Some(state) = mind(world, mind_id) else {
        return;
    };
    if let Some(visiting) = state.visiting_entity
        && visiting != entity
        && exists(world, visiting)
    {
        transfer_to(world, mind_id, Some(visiting), false, true);
        crate::ghost::set_can_return_to_body(world, visiting, false);
        return;
    }
    transfer_to(world, mind_id, None, false, false);
    let ghost_on_shutdown = world
        .get::<MindContainer>(entity)
        .is_some_and(|c| c.ghost_on_shutdown);
    if !ghost_on_shutdown
        || crate::game_ticker::run_level(world) == crate::game_ticker::GameRunLevel::PreRoundLobby
    {
        return;
    }
    let at = grid_or_map_coordinates(world, entity);
    if crate::ghost::spawn_ghost(world, mind_id, at, false).is_none() {
        eprintln!("тело удалено, а места для призрака нет");
    }
}

/// Разум удаляется (`OnMindShutdown`).
fn on_mind_terminating(world: &mut World, mind_id: Entity) {
    let Some(state) = mind(world, mind_id) else {
        return;
    };
    if let Some(user) = state.user_id {
        if let Some(mut minds) = world.get_resource_mut::<Minds>()
            && minds.user_minds.get(&user) == Some(&mind_id)
        {
            minds.user_minds.remove(&user);
        }
        with_mind(world, mind_id, |m| m.user_id = None);
    }
    if state.owned_entity.is_some_and(|e| exists(world, e)) {
        transfer_to(world, mind_id, None, false, false);
    }
    with_mind(world, mind_id, |m| m.owned_entity = None);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<MindContainer>();
    world.register_data_component::<MindExaminable>();
    world.register_data_component::<IsDeadIc>();
    if !side.is_server() {
        return;
    }
    world.init_resource::<Minds>();
    world.subscribe::<MindContainer, EntityTerminating>(|world, entity, _| {
        on_container_terminating(world, entity);
    });
    world.subscribe::<VisitingMind, EntityTerminating>(|world, entity, _| {
        if let Some(mind) = world.get::<VisitingMind>(entity).and_then(|v| v.mind) {
            unvisit(world, mind);
        }
    });
    world.subscribe::<Mind, EntityTerminating>(|world, entity, _| {
        on_mind_terminating(world, entity);
    });
    world.subscribe::<IsDeadIc, GetCharacterDeadIc>(|_, _, event| {
        event.dead = Some(true);
    });
    world.subscribe_broadcast::<crate::game_ticker::RoundRestartCleanup>(|world, _| {
        wipe_all_minds(world);
    });
    world.subscribe::<MindExaminable, Examined>(|world, entity, event| {
        if !event.is_in_details_range {
            return;
        }
        if let Some(line) = examine_line(world, entity) {
            event.push_markup(line, 0);
        }
    });
    // Время смерти — в разум: призрак покажет его.
    world.subscribe::<MindContainer, crate::mobs::MobStateChanged>(|world, entity, event| {
        if event.new_state != crate::mobs::MobState::Dead {
            return;
        }
        let now = world.resource::<GameTime>().cur_time();
        if let Some(mind) = mind_of(world, entity) {
            with_mind(world, mind, |m| m.time_of_death = Some(now));
        }
    });
}

#[cfg(test)]
pub(crate) mod tests;
