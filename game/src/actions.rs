//! Действия: кнопки горячей панели — режим боя, способности, действия предметов. Действие —
//! отдельная сущность из прототипа в контейнере `actions` владельца; клиент просит его
//! выполнить, сервер проверяет и поднимает событие действия из данных (`event: !type:…`).
//! Образец — `Content.Shared/Actions` SS14 @ 2f4a2aad (`ActionComponent`, `ActionsComponent`,
//! `InstantActionComponent`, `EntityTargetActionComponent`, `WorldTargetActionComponent`,
//! `TargetActionComponent`, `SharedActionsSystem`, `ActionContainerSystem`).

use std::collections::BTreeSet;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_data::{DataNode, TYPE_TAG_PREFIX};
use astrocore_ecs::{EventWorldExt, GameTime, LifecycleWorldExt, MapInit, RegistryWorldExt};
use astrocore_input::InputWorldExt;
use astrocore_map::{
    ContainerKind, ContainerWorldExt, MapCoordinates, MapId, SpawnWorldExt, TransformWorldExt,
};
use astrocore_math::Vec2;
use astrocore_net::{
    AppearanceWorldExt, NetComponent, NetEntities, NetEntity, NetError, NetEvent, NetEventWorldExt,
    NetRegistryWorldExt, WriteContext,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::action_blocker::ActionBlockerWorldExt;
use crate::interaction::{INTERACTION_RANGE, InteractionWorldExt};
use crate::popups::{PopupType, PopupWorldExt};
use crate::whitelist::EntityWhitelist;

/// Контейнер действий (`ActionsContainerComponent.ContainerId`).
pub const ACTIONS_CONTAINER: &str = "actions";
/// Ключ внешнего вида: включено (`ActionState.Toggled`).
pub const VISUALS_TOGGLED: &str = "enum.ActionState.Toggled";

fn yes() -> bool {
    true
}

/// Как значок действия предмета показывает предмет (`ItemActionIconStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ItemActionIconStyle {
    #[default]
    BigItem,
    BigAction,
    NoItem,
}

/// Перезарядка: начало и конец, секунды игры (`ActionCooldown`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ActionCooldown {
    pub start: f64,
    pub end: f64,
}

/// Действие. В данных — компонент `Action`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    #[serde(default)]
    pub keywords: BTreeSet<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub toggled: bool,
    #[serde(default)]
    pub cooldown: Option<ActionCooldown>,
    #[serde(default)]
    pub start_delay: bool,
    /// Пауза после использования, секунды.
    #[serde(default)]
    pub use_delay: Option<f64>,
    #[serde(default = "yes")]
    pub check_can_interact: bool,
    #[serde(default = "yes")]
    pub check_consciousness: bool,
    #[serde(default)]
    pub client_exclusive: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub raise_on_user: bool,
    #[serde(default)]
    pub raise_on_action: bool,
    #[serde(default = "yes")]
    pub auto_populate: bool,
    #[serde(default)]
    pub temporary: bool,
    #[serde(default)]
    pub item_icon_style: ItemActionIconStyle,
    #[serde(default)]
    pub sound: Option<SoundSpecifier>,
    /// Кто дал действие (предмет, сам владелец).
    #[serde(skip)]
    pub container: Option<Entity>,
    /// У кого действие на панели.
    #[serde(skip)]
    pub attached_entity: Option<Entity>,
}

/// Действия сущности на панели. В данных — компонент `Actions`.
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct Actions {
    pub actions: BTreeSet<Entity>,
}

impl NetComponent for Actions {
    const NAME: &'static str = "Actions";
    const DEFERRED: bool = true;

    fn write(&self, world: &World, _: Entity, _: &WriteContext, out: &mut Vec<u8>) {
        let list: Vec<NetEntity> = self
            .actions
            .iter()
            .filter_map(|a| world.get::<NetEntity>(*a).copied())
            .collect();
        if let Ok(bytes) = postcard::to_extend(&list, std::mem::take(out)) {
            *out = bytes;
        }
    }

    fn apply(world: &mut World, entity: Entity, bytes: &[u8]) -> Result<(), NetError> {
        let list: Vec<NetEntity> = postcard::from_bytes(bytes)?;
        let actions = list
            .into_iter()
            .filter_map(|net| world.get_resource::<NetEntities>()?.entity(net))
            .collect();
        world.add_comp(entity, Actions { actions });
        Ok(())
    }
}

/// Действие без цели. В данных — компонент `InstantAction`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstantAction {
    #[serde(default)]
    pub event: Option<DataNode>,
}

/// Общие настройки действий с целью. В данных — компонент `TargetAction`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetAction {
    #[serde(default)]
    pub repeat: bool,
    #[serde(default)]
    pub deselect_on_miss: bool,
    #[serde(default = "yes")]
    pub check_can_access: bool,
    #[serde(default)]
    pub check_can_see: bool,
    #[serde(default = "interaction_range")]
    pub range: f32,
    #[serde(default)]
    pub interact_on_miss: bool,
    #[serde(default = "yes")]
    pub targeting_indicator: bool,
}

fn interaction_range() -> f32 {
    INTERACTION_RANGE
}

/// Действие на сущность. В данных — компонент `EntityTargetAction`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityTargetAction {
    #[serde(default)]
    pub event: Option<DataNode>,
    #[serde(default)]
    pub whitelist: Option<EntityWhitelist>,
    #[serde(default)]
    pub blacklist: Option<EntityWhitelist>,
    #[serde(default = "yes")]
    pub can_target_self: bool,
    #[serde(default = "yes")]
    pub rotate_on_use: bool,
}

/// Действие на точку. В данных — компонент `WorldTargetAction`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldTargetAction {
    #[serde(default)]
    pub event: Option<DataNode>,
    #[serde(default = "yes")]
    pub rotate_on_use: bool,
}

/// Событие действия (`BaseActionEvent` и наследники). Вид — имя типа из данных
/// (`ToggleCombatActionEvent`), поля — в `data`. Поднимается на владельце (или на
/// источнике, или на самом действии) и широковещательно.
#[derive(Debug, Clone)]
pub struct ActionEvent {
    pub kind: String,
    pub data: DataNode,
    pub performer: Entity,
    pub action: Entity,
    pub target: Option<Entity>,
    pub coordinates: Option<MapCoordinates>,
    pub handled: bool,
    /// Переключить действие после выполнения (`Toggle`).
    pub toggle: bool,
}

/// Можно ли выполнить (`ActionAttemptEvent`). На действии.
#[derive(Debug, Clone)]
pub struct ActionAttempt {
    pub user: Entity,
    pub target: Option<Entity>,
    pub cancelled: bool,
    pub reason: Option<String>,
}

/// Действие выполнено (`ActionPerformedEvent`). На действии.
#[derive(Debug, Clone, Copy)]
pub struct ActionPerformed {
    pub performer: Entity,
    pub target: Option<Entity>,
}

/// Просьба клиента выполнить действие (`RequestPerformActionEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestPerformAction {
    pub action: NetEntity,
    pub entity_target: Option<NetEntity>,
    pub coordinates_target: Option<(MapId, Vec2, i16)>,
}

fn event_kind(node: &DataNode) -> String {
    node.tag
        .as_deref()
        .map(|tag| tag.strip_prefix(TYPE_TAG_PREFIX).unwrap_or(tag).to_owned())
        .unwrap_or_default()
}

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

/// Действия на `World`.
pub trait ActionsWorldExt {
    /// Создать действие из прототипа в контейнере источника и дать владельцу (`AddAction`).
    fn add_action(
        &mut self,
        performer: Entity,
        prototype: &str,
        container: Option<Entity>,
    ) -> Option<Entity>;
    /// Дать готовое действие (`AddActionDirect`).
    fn add_action_direct(&mut self, performer: Entity, action: Entity) -> bool;
    /// Убрать действие с панели (`RemoveAction`).
    fn remove_action(&mut self, performer: Entity, action: Entity);
    /// Действия владельца по приоритету и порядку добавления.
    fn actions_of(&self, performer: Entity) -> Vec<Entity>;
    fn set_toggled(&mut self, action: Entity, toggled: bool);
    fn set_action_enabled(&mut self, action: Entity, enabled: bool);
    fn set_action_cooldown(&mut self, action: Entity, seconds: f64);
    fn is_cooldown_active(&self, action: Entity) -> bool;
    /// Выполнить по просьбе клиента (`TryPerformAction`).
    fn try_perform_action(&mut self, user: Entity, request: &RequestPerformAction) -> bool;
    /// Выполнить без проверок (`PerformAction`).
    fn perform_action(
        &mut self,
        performer: Entity,
        action: Entity,
        target: Option<Entity>,
        coordinates: Option<MapCoordinates>,
    );
}

impl ActionsWorldExt for World {
    fn add_action(
        &mut self,
        performer: Entity,
        prototype: &str,
        container: Option<Entity>,
    ) -> Option<Entity> {
        let source = container.unwrap_or(performer);
        self.ensure_container(source, ACTIONS_CONTAINER, ContainerKind::List);
        let at = self.coordinates(source)?;
        let action = self.spawn_attached(prototype, at).ok()?;
        if self.get::<Action>(action).is_none() {
            self.delete_entity(action);
            return None;
        }
        if self
            .force_insert_into_container(action, source, ACTIONS_CONTAINER)
            .is_err()
        {
            self.delete_entity(action);
            return None;
        }
        if let Some(mut component) = self.get_mut::<Action>(action) {
            component.container = Some(source);
        }
        self.add_action_direct(performer, action).then_some(action)
    }

    fn add_action_direct(&mut self, performer: Entity, action: Entity) -> bool {
        let Some(component) = self.get::<Action>(action).cloned() else {
            return false;
        };
        if let Some(previous) = component.attached_entity {
            self.remove_action(previous, action);
        }
        if component.start_delay
            && let Some(delay) = component.use_delay
        {
            self.set_action_cooldown(action, delay);
        }
        if let Some(mut component) = self.get_mut::<Action>(action) {
            component.attached_entity = Some(performer);
        }
        self.ensure_comp::<Actions>(performer);
        if let Some(mut actions) = self.get_mut::<Actions>(performer) {
            actions.actions.insert(action);
        }
        true
    }

    fn remove_action(&mut self, performer: Entity, action: Entity) {
        if let Some(mut actions) = self.get_mut::<Actions>(performer) {
            actions.actions.remove(&action);
        }
        let temporary = match self.get_mut::<Action>(action) {
            Some(mut component) if component.attached_entity == Some(performer) => {
                component.attached_entity = None;
                component.temporary
            }
            _ => false,
        };
        if temporary {
            self.queue_delete(action);
        }
    }

    fn actions_of(&self, performer: Entity) -> Vec<Entity> {
        let Some(actions) = self.get::<Actions>(performer) else {
            return Vec::new();
        };
        let mut list: Vec<Entity> = actions.actions.iter().copied().collect();
        list.sort_by_key(|a| std::cmp::Reverse(self.get::<Action>(*a).map_or(0, |c| c.priority)));
        list
    }

    fn set_toggled(&mut self, action: Entity, toggled: bool) {
        let Some(mut component) = self.get_mut::<Action>(action) else {
            return;
        };
        if component.toggled == toggled {
            return;
        }
        component.toggled = toggled;
        self.set_appearance(action, VISUALS_TOGGLED, toggled);
    }

    fn set_action_enabled(&mut self, action: Entity, enabled: bool) {
        if let Some(mut component) = self.get_mut::<Action>(action)
            && component.enabled != enabled
        {
            component.enabled = enabled;
        }
    }

    fn set_action_cooldown(&mut self, action: Entity, seconds: f64) {
        let start = now(self);
        if let Some(mut component) = self.get_mut::<Action>(action) {
            component.cooldown = Some(ActionCooldown {
                start,
                end: start + seconds,
            });
        }
    }

    fn is_cooldown_active(&self, action: Entity) -> bool {
        let time = now(self);
        self.get::<Action>(action)
            .and_then(|a| a.cooldown)
            .is_some_and(|cooldown| cooldown.end > time)
    }

    fn try_perform_action(&mut self, user: Entity, request: &RequestPerformAction) -> bool {
        let Some(action) = self
            .get_resource::<NetEntities>()
            .and_then(|all| all.entity(request.action))
        else {
            return false;
        };
        if !self
            .get::<Actions>(user)
            .is_some_and(|actions| actions.actions.contains(&action))
        {
            return false;
        }
        let Some(component) = self.get::<Action>(action).cloned() else {
            return false;
        };
        if !component.enabled || self.is_cooldown_active(action) {
            return false;
        }
        let target = request
            .entity_target
            .and_then(|net| self.get_resource::<NetEntities>()?.entity(net));
        let mut attempt = ActionAttempt {
            user,
            target,
            cancelled: false,
            reason: None,
        };
        self.raise(action, &mut attempt);
        if attempt.cancelled {
            if let Some(reason) = attempt.reason {
                self.popup_entity_for(&reason, user, user, PopupType::Small);
            }
            return false;
        }
        // Проверки действующего (`OnValidate`).
        if (component.check_consciousness && !self.can_consciously_perform_action(user))
            || (component.check_can_interact && !self.can_interact(user, None))
        {
            return false;
        }
        let coordinates = request
            .coordinates_target
            .map(|(map, position, z)| MapCoordinates { map, position, z });
        let entity_action = self.get::<EntityTargetAction>(action).cloned();
        let world_action = self.get::<WorldTargetAction>(action).cloned();
        if let Some(entity_action) = entity_action.filter(|a| a.event.is_some()) {
            let Some(target) = target.filter(|t| self.is_alive(*t)) else {
                return false;
            };
            if entity_action.rotate_on_use
                && let Some(at) = self.map_coordinates(target)
            {
                crate::interaction::face(self, user, at);
            }
            if !validate_entity_target(self, user, target, action) {
                return false;
            }
        } else if let Some(world_action) = world_action {
            let Some(at) = coordinates else {
                return false;
            };
            if world_action.rotate_on_use {
                crate::interaction::face(self, user, at);
            }
            if !validate_base_target(self, user, at, action) {
                return false;
            }
            if let Some(target) = target
                && !validate_entity_target(self, user, target, action)
            {
                return false;
            }
        }
        self.perform_action(user, action, target, coordinates);
        true
    }

    fn perform_action(
        &mut self,
        performer: Entity,
        action: Entity,
        target: Option<Entity>,
        coordinates: Option<MapCoordinates>,
    ) {
        let Some(component) = self.get::<Action>(action).cloned() else {
            return;
        };
        if component
            .attached_entity
            .is_some_and(|attached| attached != performer)
        {
            return;
        }
        let node = self
            .get::<InstantAction>(action)
            .and_then(|a| a.event.clone())
            .or_else(|| {
                self.get::<EntityTargetAction>(action)
                    .and_then(|a| a.event.clone())
            })
            .or_else(|| {
                self.get::<WorldTargetAction>(action)
                    .and_then(|a| a.event.clone())
            });
        let Some(node) = node else {
            return;
        };
        let mut event = ActionEvent {
            kind: event_kind(&node),
            data: node,
            performer,
            action,
            target,
            coordinates,
            handled: false,
            toggle: false,
        };
        let mut raise_on = performer;
        if !component.raise_on_user
            && let Some(container) = component.container
        {
            raise_on = container;
        }
        if component.raise_on_action {
            raise_on = action;
        }
        self.raise_and_broadcast(raise_on, &mut event);
        if !event.handled {
            return;
        }
        if event.toggle {
            self.set_toggled(action, !component.toggled);
        }
        if let Some(sound) = &component.sound {
            self.play_entity(sound, performer, None);
        }
        if let Some(mut stored) = self.get_mut::<Action>(action) {
            stored.cooldown = None;
        }
        if let Some(delay) = component.use_delay {
            self.set_action_cooldown(action, delay);
        }
        self.raise(action, &mut ActionPerformed { performer, target });
    }
}

/// Проверка цели-сущности (`ValidateEntityTarget`).
fn validate_entity_target(world: &mut World, user: Entity, target: Entity, action: Entity) -> bool {
    let Some(entity_action) = world.get::<EntityTargetAction>(action).cloned() else {
        return false;
    };
    if !world.is_alive(target) {
        return false;
    }
    if entity_action
        .whitelist
        .as_ref()
        .is_some_and(|list| !list.is_valid(world, target))
        || entity_action
            .blacklist
            .as_ref()
            .is_some_and(|list| list.is_valid(world, target))
    {
        return false;
    }
    let check_interact = world
        .get::<Action>(action)
        .is_some_and(|a| a.check_can_interact);
    if check_interact && !world.can_interact(user, Some(target)) {
        return false;
    }
    if user == target {
        return entity_action.can_target_self;
    }
    let target_action = world
        .get::<TargetAction>(action)
        .cloned()
        .unwrap_or(TargetAction {
            repeat: false,
            deselect_on_miss: false,
            check_can_access: true,
            check_can_see: false,
            range: INTERACTION_RANGE,
            interact_on_miss: false,
            targeting_indicator: true,
        });
    if target_action.check_can_access {
        return world.in_range_unobstructed(user, target, target_action.range)
            && world.is_accessible(user, target);
    }
    if target_action.range > 0.0 {
        let (Some(a), Some(b)) = (world.map_coordinates(user), world.map_coordinates(target))
        else {
            return false;
        };
        if a.map != b.map || a.position.distance(b.position) > target_action.range {
            return false;
        }
    }
    world.is_accessible(user, target)
}

/// Проверка точки (`ValidateBaseTarget`).
fn validate_base_target(
    world: &mut World,
    user: Entity,
    at: MapCoordinates,
    action: Entity,
) -> bool {
    let Some(target_action) = world.get::<TargetAction>(action).cloned() else {
        return false;
    };
    if target_action.check_can_access {
        return world.in_range_unobstructed_point(user, at, target_action.range);
    }
    let Some(from) = world.map_coordinates(user) else {
        return false;
    };
    if from.map != at.map {
        return false;
    }
    target_action.range <= 0.0 || from.position.distance(at.position) <= target_action.range
}

/// Ячеек на горячей панели (`Hotbar1`…`Hotbar0`).
pub const HOTBAR_SLOTS: usize = 10;

/// Клиент: что лежит в ячейках панели. Новые действия занимают первую свободную ячейку
/// (`AutoPopulate`), снятые освобождают свою.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default, PartialEq)]
pub struct ActionBar {
    pub slots: [Option<Entity>; HOTBAR_SLOTS],
}

/// Клиент: действие ждёт выбора цели кликом (`ActionUIController` с целью).
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct ActionTargeting {
    pub action: Option<Entity>,
}

/// Нужна ли действию цель.
pub fn needs_target(world: &World, action: Entity) -> bool {
    world.get::<EntityTargetAction>(action).is_some()
        || world.get::<WorldTargetAction>(action).is_some()
}

/// Клиент: разложить действия игрока по ячейкам.
pub fn sync_action_bar(world: &mut World) {
    let player = world
        .get_resource::<astrocore_net::ClientNet>()
        .and_then(|net| net.attached);
    let actions = player
        .map(|player| world.actions_of(player))
        .unwrap_or_default();
    let mut bar = world
        .get_resource::<ActionBar>()
        .cloned()
        .unwrap_or_default();
    for slot in bar.slots.iter_mut() {
        if slot.is_some_and(|action| !actions.contains(&action)) {
            *slot = None;
        }
    }
    for action in actions {
        let auto = world.get::<Action>(action).is_some_and(|a| a.auto_populate);
        if !auto || bar.slots.contains(&Some(action)) {
            continue;
        }
        if let Some(free) = bar.slots.iter_mut().find(|slot| slot.is_none()) {
            *free = Some(action);
        }
    }
    let slots = bar.slots;
    if world.get_resource::<ActionBar>() != Some(&bar) {
        world.insert_resource(bar);
    }
    let targeting = world
        .get_resource::<ActionTargeting>()
        .and_then(|t| t.action);
    if targeting.is_some_and(|action| !slots.contains(&Some(action))) {
        world.insert_resource(ActionTargeting::default());
    }
}

/// Клиент: нажата ячейка — выполнить или ждать цели; повторное нажатие отменяет выбор цели.
pub fn activate_slot(world: &mut World, index: usize) {
    let Some(action) = world
        .get_resource::<ActionBar>()
        .and_then(|bar| bar.slots.get(index).copied().flatten())
    else {
        return;
    };
    if needs_target(world, action) {
        let current = world
            .get_resource::<ActionTargeting>()
            .and_then(|t| t.action);
        let next = if current == Some(action) {
            None
        } else {
            Some(action)
        };
        world.insert_resource(ActionTargeting { action: next });
        return;
    }
    request_action(world, action, None, None);
}

/// Клиент: попросить сервер выполнить действие.
pub fn request_action(
    world: &mut World,
    action: Entity,
    target: Option<Entity>,
    at: Option<MapCoordinates>,
) {
    let Some(net) = world.get::<NetEntity>(action).copied() else {
        return;
    };
    let request = RequestPerformAction {
        action: net,
        entity_target: target.and_then(|t| world.get::<NetEntity>(t).copied()),
        coordinates_target: at.map(|at| (at.map, at.position, at.z)),
    };
    world.send_net_event_to_server(&request);
}

fn receive(world: &mut World, event: &mut NetEvent<RequestPerformAction>) {
    let Some(user) = crate::hands::player(world, event.session) else {
        return;
    };
    world.try_perform_action(user, &event.event.clone());
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<Action>();
    world.register_data_component::<InstantAction>();
    world.register_data_component::<TargetAction>();
    world.register_data_component::<EntityTargetAction>();
    world.register_data_component::<WorldTargetAction>();
    world.register_net_serde::<Action>();
    world.register_net_component::<Actions>();
    world.register_net_event::<RequestPerformAction>();
    if !side.is_server() {
        world.init_resource::<ActionBar>();
        world.init_resource::<ActionTargeting>();
        // Клавиши 1…0 (`Hotbar1`…`Hotbar0`).
        for index in 0..HOTBAR_SLOTS {
            let function = format!("Hotbar{}", (index + 1) % 10);
            world.bind_input(&function, move |world, args| {
                if args.state == astrocore_input::BoundKeyState::Down {
                    activate_slot(world, index);
                }
                true
            });
        }
        return;
    }
    world.subscribe::<Action, MapInit>(|world, action, _| {
        let toggled = world.get::<Action>(action).is_some_and(|a| a.toggled);
        world.set_appearance(action, VISUALS_TOGGLED, toggled);
    });
    world.subscribe_net_event::<RequestPerformAction>(receive);
}

#[cfg(test)]
mod tests;
