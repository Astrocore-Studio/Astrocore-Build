//! Руки: в каждой руке — контейнер на один предмет, одна рука активна. Подбор, выброс с
//! проверкой стен, смена руки, бросок, запросы с панели рук. Образец —
//! `Content.Shared/Hands` SS14 @ 2f4a2aad (`HandsComponent`, `SharedHandsSystem.*`) и
//! `Content.Server/Hands/Systems/HandsSystem.cs` (бросок).

use std::collections::BTreeMap;

use astrocore_ecs::{
    ComponentInit, EventWorldExt, GameTime, LifecycleWorldExt, MapInit, MetaData, RegistryWorldExt,
};
use astrocore_input::{BoundKeyState, InputWorldExt};
use astrocore_map::{
    ContainerKind, ContainerManager, ContainerWorldExt, EntGotInsertedIntoContainer,
    EntGotRemovedFromContainer, MapCoordinates, TransformWorldExt,
};
use astrocore_math::{Angle, Vec2};
use astrocore_net::{NetEvent, NetEventWorldExt, NetRegistryWorldExt, Session};
use astrocore_physics::{BodyType, CollisionGroups, Physics, Ray, RayWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::action_blocker::ActionBlockerWorldExt;
use crate::content::groups;
use crate::interaction::{GetUsedEntity, INTERACTION_RANGE, InteractHand, InteractionWorldExt};
use crate::item::Item;
use crate::ordered::OrderedMap;
use crate::tag::TagWorldExt;
use crate::whitelist::{EntityWhitelist, check_both};

/// Тег, снимающий проверки дальности и стен при выбросе (призрак админа).
pub const BYPASS_DROP_TAG: &str = "BypassDropChecks";

/// Функции ввода рук (`ContentKeyFunctions`).
pub mod functions {
    pub const SWAP_HANDS: &str = "SwapHands";
    pub const SWAP_HANDS_REVERSE: &str = "SwapHandsReverse";
    pub const DROP: &str = "Drop";
    pub const THROW_ITEM_IN_HAND: &str = "ThrowItemInHand";
    pub const ALT_USE_ITEM_IN_HAND: &str = "AltUseItemInHand";
}

/// Где рука: от неё зависят порядок рук и сторона предмета на спрайте.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum HandLocation {
    Right,
    #[default]
    Middle,
    Left,
}

/// Рука (`Hand`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hand {
    #[serde(default)]
    pub location: HandLocation,
    /// Подпись пустой руки на панели.
    #[serde(default)]
    pub empty_label: Option<String>,
    #[serde(default)]
    pub whitelist: Option<EntityWhitelist>,
    #[serde(default)]
    pub blacklist: Option<EntityWhitelist>,
}

fn throw_speed() -> f32 {
    11.0
}

fn throw_range() -> f32 {
    8.0
}

fn throw_cooldown() -> f32 {
    0.5
}

fn yes() -> bool {
    true
}

/// Руки сущности. В данных — компонент `Hands`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hands {
    #[serde(default)]
    pub active_hand_id: Option<String>,
    /// Руки, которые появляются при `MapInit`, в порядке данных.
    #[serde(default)]
    pub starting_hands: OrderedMap<Hand>,
    #[serde(default)]
    pub hands: BTreeMap<String, Hand>,
    /// Руки по порядку: справа налево.
    #[serde(default)]
    pub sorted_hands: Vec<String>,
    #[serde(default = "throw_speed")]
    pub base_throwspeed: f32,
    #[serde(default = "throw_range")]
    pub throw_range: f32,
    /// Пауза между бросками, секунды.
    #[serde(default = "throw_cooldown")]
    pub throw_cooldown: f32,
    /// Предметы в руках видны на спрайте.
    #[serde(default = "yes")]
    pub show_in_hands: bool,
    #[serde(default = "yes")]
    pub can_be_stripped: bool,
    /// Время игры, раньше которого бросать нельзя.
    #[serde(skip)]
    pub next_throw_time: f64,
}

impl Default for Hands {
    fn default() -> Self {
        Self {
            active_hand_id: None,
            starting_hands: OrderedMap::default(),
            hands: BTreeMap::new(),
            sorted_hands: Vec::new(),
            base_throwspeed: throw_speed(),
            throw_range: throw_range(),
            throw_cooldown: throw_cooldown(),
            show_in_hands: true,
            can_be_stripped: true,
            next_throw_time: 0.0,
        }
    }
}

/// Предмет попал в руку (`DidEquipHandEvent`). Поднимается на владельце рук.
#[derive(Debug, Clone)]
pub struct DidEquipHand {
    pub user: Entity,
    pub item: Entity,
    pub hand: String,
}

/// Предмет взяли в руку (`GotEquippedHandEvent`). Поднимается на предмете.
#[derive(Debug, Clone)]
pub struct GotEquippedHand {
    pub user: Entity,
    pub item: Entity,
    pub hand: String,
}

/// Предмет ушёл из руки (`DidUnequipHandEvent`). Поднимается на владельце рук.
#[derive(Debug, Clone)]
pub struct DidUnequipHand {
    pub user: Entity,
    pub item: Entity,
    pub hand: String,
}

/// Предмет вынули из руки (`GotUnequippedHandEvent`). Поднимается на предмете.
#[derive(Debug, Clone)]
pub struct GotUnequippedHand {
    pub user: Entity,
    pub item: Entity,
    pub hand: String,
}

/// Рука с предметом стала активной (`HandSelectedEvent`). Поднимается на предмете.
#[derive(Debug, Clone, Copy)]
pub struct HandSelected {
    pub user: Entity,
}

/// Рука с предметом перестала быть активной (`HandDeselectedEvent`). Поднимается на предмете.
#[derive(Debug, Clone, Copy)]
pub struct HandDeselected {
    pub user: Entity,
}

/// Перед тем как взять предмет (`BeforeEquippingHandEvent`). Поднимается на владельце рук.
#[derive(Debug, Clone, Copy)]
pub struct BeforeEquippingHand {
    pub item: Entity,
    pub cancelled: bool,
}

/// Перед тем как предмет возьмут (`BeforeGettingEquippedHandEvent`). Поднимается на предмете.
#[derive(Debug, Clone, Copy)]
pub struct BeforeGettingEquippedHand {
    pub user: Entity,
    pub cancelled: bool,
}

/// Число рук изменилось (`HandCountChangedEvent`). Поднимается на владельце рук.
#[derive(Debug, Clone, Copy)]
pub struct HandCountChanged {
    pub user: Entity,
}

/// Предмет выпустили из рук (`DroppedEvent`). Поднимается на предмете и широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct Dropped {
    pub user: Entity,
}

/// Перед броском из руки (`BeforeThrowEvent`): системы меняют предмет, направление или
/// силу либо отменяют бросок. Поднимается на бросающем.
#[derive(Debug, Clone, Copy)]
pub struct BeforeThrow {
    pub item: Entity,
    pub direction: Vec2,
    pub throw_speed: f32,
    pub user: Entity,
    pub cancelled: bool,
}

/// Запросы панели рук клиента (`RequestSetHandEvent` и соседи).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HandsRequest {
    /// Сделать руку активной.
    SetHand(String),
    /// «Использовать» предмет в руке (E по слоту).
    ActivateInHand(String),
    /// Ударить предметом из активной руки по предмету в другой руке.
    InteractUsingInHand(String),
    /// Использовать предмет в активной руке.
    UseInHand,
    /// Переложить предмет из руки в пустую активную.
    MoveItemToActive(String),
    /// Альтернативное действие предметом в руке.
    AltInteractInHand(String),
}

/// Руки на `World`.
pub trait HandsWorldExt {
    fn add_hand(&mut self, user: Entity, name: &str, hand: Hand);
    fn remove_hand(&mut self, user: Entity, name: &str);
    /// Предмет в руке.
    fn held_item(&self, user: Entity, hand: &str) -> Option<Entity>;
    /// Предмет в активной руке.
    fn active_item(&self, user: Entity) -> Option<Entity>;
    fn active_hand(&self, user: Entity) -> Option<String>;
    /// Руки: сначала активная, затем остальные по порядку.
    fn hands_in_order(&self, user: Entity) -> Vec<String>;
    /// Предметы в руках в том же порядке.
    fn held_items(&self, user: Entity) -> Vec<Entity>;
    /// В какой руке предмет.
    fn hand_holding(&self, user: Entity, item: Entity) -> Option<String>;
    fn empty_hand(&self, user: Entity) -> Option<String>;
    fn set_active_hand(&mut self, user: Entity, hand: Option<&str>) -> bool;
    fn swap_hands(&mut self, user: Entity, reverse: bool);
    fn can_pickup_to_hand(
        &mut self,
        user: Entity,
        item: Entity,
        hand: &str,
        check_blocker: bool,
    ) -> bool;
    /// Взять предмет в руку (активную, если `hand` не задана).
    fn try_pickup(
        &mut self,
        user: Entity,
        item: Entity,
        hand: Option<&str>,
        check_blocker: bool,
    ) -> bool;
    fn try_pickup_any_hand(&mut self, user: Entity, item: Entity, check_blocker: bool) -> bool;
    /// Взять в руку или положить рядом, если рук или места нет.
    fn pickup_or_drop(&mut self, user: Option<Entity>, item: Entity);
    fn can_drop_held(&mut self, user: Entity, hand: &str, check_blocker: bool) -> bool;
    /// Выпустить предмет из руки к точке (не дальше дальности взаимодействия и не сквозь
    /// стены) или под ноги.
    fn try_drop(
        &mut self,
        user: Entity,
        hand: &str,
        at: Option<MapCoordinates>,
        check_blocker: bool,
    ) -> bool;
    /// Бросить предмет из активной руки в точку (`ThrowHeldItem`).
    fn throw_held_item(&mut self, user: Entity, at: MapCoordinates) -> bool;
}

impl HandsWorldExt for World {
    fn add_hand(&mut self, user: Entity, name: &str, hand: Hand) {
        let Some(hands) = self.get::<Hands>(user) else {
            return;
        };
        if hands.hands.contains_key(name) {
            return;
        }
        ensure_hand_container(self, user, name);
        let Some(mut hands) = self.get_mut::<Hands>(user) else {
            return;
        };
        hands.hands.insert(name.to_owned(), hand);
        hands.sorted_hands.push(name.to_owned());
        // Устойчивая сортировка по месту руки: справа налево.
        let order: BTreeMap<String, HandLocation> = hands
            .hands
            .iter()
            .map(|(name, hand)| (name.clone(), hand.location))
            .collect();
        hands
            .sorted_hands
            .sort_by_key(|name| order.get(name).copied());
        let needs_active = hands.active_hand_id.is_none();
        if needs_active {
            self.set_active_hand(user, Some(name));
        }
        self.raise(user, &mut HandCountChanged { user });
    }

    fn remove_hand(&mut self, user: Entity, name: &str) {
        if self
            .get::<Hands>(user)
            .is_none_or(|hands| !hands.hands.contains_key(name))
        {
            return;
        }
        self.try_drop(user, name, None, false);
        let Some(mut hands) = self.get_mut::<Hands>(user) else {
            return;
        };
        hands.hands.remove(name);
        hands.sorted_hands.retain(|hand| hand != name);
        let next = (hands.active_hand_id.as_deref() == Some(name))
            .then(|| hands.sorted_hands.first().cloned());
        if let Some(next) = next {
            self.set_active_hand(user, next.as_deref());
        }
        self.raise(user, &mut HandCountChanged { user });
    }

    fn held_item(&self, user: Entity, hand: &str) -> Option<Entity> {
        self.get::<Hands>(user)?.hands.get(hand)?;
        self.container(user, hand)?.entities().first().copied()
    }

    fn active_item(&self, user: Entity) -> Option<Entity> {
        let active = self.get::<Hands>(user)?.active_hand_id.clone()?;
        self.held_item(user, &active)
    }

    fn active_hand(&self, user: Entity) -> Option<String> {
        self.get::<Hands>(user)?.active_hand_id.clone()
    }

    fn hands_in_order(&self, user: Entity) -> Vec<String> {
        let Some(hands) = self.get::<Hands>(user) else {
            return Vec::new();
        };
        let active = hands.active_hand_id.iter().cloned();
        active
            .chain(
                hands
                    .sorted_hands
                    .iter()
                    .filter(|hand| Some(*hand) != hands.active_hand_id.as_ref())
                    .cloned(),
            )
            .collect()
    }

    fn held_items(&self, user: Entity) -> Vec<Entity> {
        self.hands_in_order(user)
            .iter()
            .filter_map(|hand| self.held_item(user, hand))
            .collect()
    }

    fn hand_holding(&self, user: Entity, item: Entity) -> Option<String> {
        let hands = self.get::<Hands>(user)?;
        hands
            .hands
            .keys()
            .find(|hand| self.held_item(user, hand) == Some(item))
            .cloned()
    }

    fn empty_hand(&self, user: Entity) -> Option<String> {
        self.hands_in_order(user)
            .into_iter()
            .find(|hand| self.held_item(user, hand).is_none())
    }

    fn set_active_hand(&mut self, user: Entity, hand: Option<&str>) -> bool {
        let Some(hands) = self.get::<Hands>(user) else {
            return false;
        };
        if hands.active_hand_id.as_deref() == hand
            || hand.is_some_and(|hand| !hands.hands.contains_key(hand))
        {
            return false;
        }
        if let Some(old) = self.active_item(user) {
            self.raise(old, &mut HandDeselected { user });
        }
        if let Some(mut hands) = self.get_mut::<Hands>(user) {
            hands.active_hand_id = hand.map(str::to_owned);
        }
        if let Some(new) = hand.and_then(|hand| self.held_item(user, hand)) {
            self.raise(new, &mut HandSelected { user });
        }
        true
    }

    fn swap_hands(&mut self, user: Entity, reverse: bool) {
        let Some(hands) = self.get::<Hands>(user) else {
            return;
        };
        let (Some(active), count) = (hands.active_hand_id.clone(), hands.hands.len()) else {
            return;
        };
        if count < 2 {
            return;
        }
        let current = hands
            .sorted_hands
            .iter()
            .position(|hand| *hand == active)
            .unwrap_or(0);
        let step = if reverse { count - 1 } else { 1 };
        let next = hands.sorted_hands[(current + step) % count].clone();
        self.set_active_hand(user, Some(&next));
    }

    fn can_pickup_to_hand(
        &mut self,
        user: Entity,
        item: Entity,
        hand: &str,
        check_blocker: bool,
    ) -> bool {
        if self.get::<Hands>(user).is_none() || self.held_item(user, hand).is_some() {
            return false;
        }
        if self.container(user, hand).is_none() || self.get::<Item>(item).is_none() {
            return false;
        }
        if self
            .get::<Physics>(item)
            .is_some_and(|body| body.body_type == BodyType::Static)
        {
            return false;
        }
        if check_blocker && !self.can_pickup(user, item) {
            return false;
        }
        let rules = self
            .get::<Hands>(user)
            .and_then(|hands| hands.hands.get(hand).cloned())
            .unwrap_or_default();
        if !check_both(
            self,
            item,
            rules.blacklist.as_ref(),
            rules.whitelist.as_ref(),
        ) {
            return false;
        }
        self.can_insert(item, user, hand).is_ok()
    }

    fn try_pickup(
        &mut self,
        user: Entity,
        item: Entity,
        hand: Option<&str>,
        check_blocker: bool,
    ) -> bool {
        let hand = match hand {
            Some(hand) => hand.to_owned(),
            None => match self.active_hand(user) {
                Some(hand) => hand,
                None => return false,
            },
        };
        if !self.is_alive(item)
            || self
                .get::<MetaData>(item)
                .is_some_and(|meta| meta.terminating())
        {
            return false;
        }
        if !self.can_pickup_to_hand(user, item, &hand, check_blocker) {
            return false;
        }
        let mut before = BeforeEquippingHand {
            item,
            cancelled: false,
        };
        self.raise(user, &mut before);
        let mut getting = BeforeGettingEquippedHand {
            user,
            cancelled: false,
        };
        if !before.cancelled {
            self.raise(item, &mut getting);
        }
        if before.cancelled || getting.cancelled {
            return false;
        }
        do_pickup(self, user, &hand, item)
    }

    fn try_pickup_any_hand(&mut self, user: Entity, item: Entity, check_blocker: bool) -> bool {
        match self.empty_hand(user) {
            Some(hand) => self.try_pickup(user, item, Some(&hand), check_blocker),
            None => false,
        }
    }

    fn pickup_or_drop(&mut self, user: Option<Entity>, item: Entity) {
        if let Some(user) = user
            && self.try_pickup_any_hand(user, item, true)
        {
            return;
        }
        if self.containing_container(item).is_some() {
            let at = user.and_then(|user| self.coordinates(user));
            let _ = self.remove_from_container(item, at);
        }
    }

    fn can_drop_held(&mut self, user: Entity, hand: &str, check_blocker: bool) -> bool {
        if self.held_item(user, hand).is_none() {
            return false;
        }
        if check_blocker && !self.can_drop(user) {
            return false;
        }
        true
    }

    fn try_drop(
        &mut self,
        user: Entity,
        hand: &str,
        at: Option<MapCoordinates>,
        check_blocker: bool,
    ) -> bool {
        if !self.can_drop_held(user, hand, check_blocker) {
            return false;
        }
        let Some(item) = self.held_item(user, hand) else {
            return false;
        };
        // Держащий сидит в контейнере: предмет остаётся в том же контейнере.
        if let Some(outer) = self.containing_container(user).cloned() {
            if self.remove_from_container(item, None).is_err() {
                return false;
            }
            let _ = self.insert_into_container(item, outer.owner, &outer.id);
            return true;
        }
        do_drop(self, user, hand, item, at, true)
    }

    fn throw_held_item(&mut self, user: Entity, at: MapCoordinates) -> bool {
        if self.containing_container(user).is_some() {
            return false;
        }
        let Some(item) = self.active_item(user) else {
            return false;
        };
        if !self.can_throw(user, item) {
            return false;
        }
        let now = self.resource::<GameTime>().cur_time().as_secs_f64();
        let Some(hands) = self.get::<Hands>(user) else {
            return false;
        };
        if now < hands.next_throw_time {
            return false;
        }
        let (speed, range, cooldown) = (
            hands.base_throwspeed,
            hands.throw_range,
            hands.throw_cooldown,
        );
        if let Some(mut hands) = self.get_mut::<Hands>(user) {
            hands.bypass_change_detection_mut().next_throw_time = now + f64::from(cooldown);
        }
        let Some(own) = self.map_coordinates(user) else {
            return false;
        };
        if own.map != at.map {
            return false;
        }
        let mut direction = at.position - own.position;
        if direction == Vec2::ZERO {
            return true;
        }
        let length = direction.length();
        direction *= length.clamp(0.1, range) / length;
        let mut before = BeforeThrow {
            item,
            direction,
            throw_speed: speed,
            user,
            cancelled: false,
        };
        self.raise(user, &mut before);
        if before.cancelled {
            return true;
        }
        if let Some(hand) = self.hand_holding(user, before.item)
            && !self.try_drop(user, &hand, None, true)
        {
            return false;
        }
        let compensate = self
            .get::<crate::throwing::LandAtCursor>(before.item)
            .is_none();
        crate::throwing::try_throw(
            self,
            before.item,
            before.direction,
            before.throw_speed,
            Some(user),
            compensate,
        );
        true
    }
}

/// Помощник для полей, изменение которых клиенту не нужно.
trait BypassChange {
    fn bypass_change_detection_mut(&mut self) -> &mut Hands;
}

impl BypassChange for bevy_ecs::world::Mut<'_, Hands> {
    fn bypass_change_detection_mut(&mut self) -> &mut Hands {
        use bevy_ecs::change_detection::DetectChangesMut;
        self.bypass_change_detection()
    }
}

fn ensure_hand_container(world: &mut World, user: Entity, hand: &str) {
    world.ensure_container(user, hand, ContainerKind::Slot);
    if let Some(mut containers) = world.get_mut::<ContainerManager>(user)
        && let Some(container) = containers.containers.get_mut(hand)
        && container.occludes_light
    {
        container.occludes_light = false;
    }
}

fn do_pickup(world: &mut World, user: Entity, hand: &str, item: Entity) -> bool {
    if world.held_item(user, hand).is_some() {
        return false;
    }
    if world.insert_into_container(item, user, hand).is_err() {
        return false;
    }
    if world.active_hand(user).as_deref() == Some(hand) {
        world.raise(item, &mut HandSelected { user });
    }
    true
}

/// Точка выброса: не дальше дальности взаимодействия и не за стеной (`GetFinalDropCoordinates`).
fn final_drop_position(
    world: &World,
    user: Entity,
    item: Entity,
    origin: MapCoordinates,
    target: Vec2,
) -> Vec2 {
    let mut vector = target - origin.position;
    let requested = vector.length();
    if requested == 0.0 {
        return target;
    }
    let mut length = requested;
    if !world.has_tag(user, BYPASS_DROP_TAG) {
        if requested > INTERACTION_RANGE {
            vector = vector.normalize() * INTERACTION_RANGE;
        }
        let ray = Ray {
            origin,
            direction: vector,
            max_distance: vector.length(),
            mask: CollisionGroups(groups::IN_RANGE_UNOBSTRUCTED),
        };
        length = world.unobstructed_distance(ray, &|entity| entity == user || entity == item);
    }
    if length < requested {
        origin.position + vector.normalize() * length
    } else {
        target
    }
}

fn do_drop(
    world: &mut World,
    user: Entity,
    hand: &str,
    item: Entity,
    at: Option<MapCoordinates>,
    drop_interaction: bool,
) -> bool {
    let was_active = world.active_hand(user).as_deref() == Some(hand);
    if world.remove_from_container(item, None).is_err() {
        return false;
    }
    if let (Some(at), Some(origin)) = (at, world.map_coordinates(item))
        && at.map == origin.map
    {
        let position = final_drop_position(world, user, item, origin, at.position);
        let _ = world.set_world_position(item, position);
    }
    if drop_interaction {
        // Выпущенный предмет лежит ровно (`DroppedInteraction`).
        let _ = world.set_local_rotation(item, Angle::ZERO);
        world.raise_and_broadcast(item, &mut Dropped { user });
    }
    if was_active {
        world.raise(item, &mut HandDeselected { user });
    }
    true
}

/// Проверка команды клиента: у сессии есть живое тело.
fn player(world: &World, session: Option<Entity>) -> Option<Entity> {
    let user = world.get::<Session>(session?)?.attached?;
    world.is_alive(user).then_some(user)
}

fn handle_request(world: &mut World, event: &mut NetEvent<HandsRequest>) {
    let Some(user) = player(world, event.session) else {
        return;
    };
    if world.get::<Hands>(user).is_none() {
        return;
    }
    match event.event.clone() {
        HandsRequest::SetHand(hand) => {
            world.set_active_hand(user, Some(&hand));
        }
        HandsRequest::ActivateInHand(hand) => {
            let hand = if world.held_item(user, &hand).is_some() {
                Some(hand)
            } else {
                world.active_hand(user)
            };
            if let Some(item) = hand.and_then(|hand| world.held_item(user, &hand)) {
                world.interaction_activate(user, item, true);
            }
        }
        HandsRequest::InteractUsingInHand(hand) => {
            if let (Some(active), Some(held)) =
                (world.active_item(user), world.held_item(user, &hand))
                && let Some(click) = world.map_coordinates(held)
            {
                world.interact_using(user, active, held, click);
            }
        }
        HandsRequest::UseInHand => {
            if let Some(item) = world.active_item(user) {
                world.use_in_hand_interaction(user, item);
            }
        }
        HandsRequest::MoveItemToActive(hand) => {
            move_to_active(world, user, &hand);
        }
        HandsRequest::AltInteractInHand(hand) => {
            let hand = if world.held_item(user, &hand).is_some() {
                Some(hand)
            } else {
                world.active_hand(user)
            };
            if let Some(item) = hand.and_then(|hand| world.held_item(user, &hand)) {
                crate::verbs::alt_interact(world, user, item);
            }
        }
    }
}

/// Переложить предмет в пустую активную руку (`TryMoveHeldEntityToActiveHand`).
fn move_to_active(world: &mut World, user: Entity, hand: &str) -> bool {
    let Some(active) = world.active_hand(user) else {
        return false;
    };
    if world.held_item(user, &active).is_some() {
        return false;
    }
    let Some(item) = world.held_item(user, hand) else {
        return false;
    };
    if !world.can_drop_held(user, hand, true) {
        return false;
    }
    if world.remove_from_container(item, None).is_err() {
        return false;
    }
    if !world.can_pickup_to_hand(user, item, &active, true)
        || !do_pickup(world, user, &active, item)
    {
        // Вернуть в прежнюю руку.
        let _ = world.insert_into_container(item, user, hand);
        return false;
    }
    true
}

fn bind_inputs(world: &mut World) {
    world.bind_input(functions::SWAP_HANDS, |world, args| {
        if args.state == BoundKeyState::Down
            && let Some(user) = args.player
            && world.can_interact(user, None)
        {
            world.swap_hands(user, false);
        }
        false
    });
    world.bind_input(functions::SWAP_HANDS_REVERSE, |world, args| {
        if args.state == BoundKeyState::Down
            && let Some(user) = args.player
            && world.can_interact(user, None)
        {
            world.swap_hands(user, true);
        }
        false
    });
    world.bind_input(functions::DROP, |world, args| {
        if args.state == BoundKeyState::Down
            && let Some(user) = args.player
            && let Some(hand) = world.active_hand(user)
        {
            world.try_drop(user, &hand, args.coordinates, true);
        }
        false
    });
    world.bind_input(functions::ALT_USE_ITEM_IN_HAND, |world, args| {
        if args.state == BoundKeyState::Down
            && let Some(user) = args.player
            && let Some(item) = world.active_item(user)
        {
            crate::verbs::alt_interact(world, user, item);
        }
        false
    });
    world.bind_input(functions::THROW_ITEM_IN_HAND, |world, args| {
        if args.state == BoundKeyState::Down
            && let (Some(user), Some(at)) = (args.player, args.coordinates)
            && world.is_alive(user)
        {
            world.throw_held_item(user, at);
        }
        false
    });
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<Hands>();
    world.register_net_serde::<Hands>();
    world.register_net_event::<HandsRequest>();
    // Контейнеры рук создаются вместе с компонентом, руки — при `MapInit`.
    world.subscribe::<Hands, ComponentInit>(|world, user, _| {
        let names: Vec<String> = world
            .get::<Hands>(user)
            .map(|hands| hands.hands.keys().cloned().collect())
            .unwrap_or_default();
        for name in names {
            ensure_hand_container(world, user, &name);
        }
    });
    if !side.is_server() {
        return;
    }
    world.subscribe::<Hands, MapInit>(|world, user, _| {
        let starting: Vec<(String, Hand)> = world
            .get::<Hands>(user)
            .map(|hands| {
                hands
                    .starting_hands
                    .iter()
                    .map(|(name, hand)| (name.clone(), hand.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for (name, hand) in starting {
            world.add_hand(user, &name, hand);
        }
        if world.active_hand(user).is_none() {
            let first = world
                .get::<Hands>(user)
                .and_then(|hands| hands.sorted_hands.first().cloned());
            world.set_active_hand(user, first.as_deref());
        }
    });
    world.subscribe::<Hands, GetUsedEntity>(|world, user, event| {
        if event.used.is_none() {
            event.used = world.active_item(user);
        }
    });
    // Клик рукой по предмету — взять его (`SharedItemSystem.OnHandInteract`).
    world.subscribe::<Item, InteractHand>(|world, item, event| {
        if !event.handled {
            event.handled = world.try_pickup(event.user, item, None, true);
        }
    });
    world.subscribe::<Item, EntGotInsertedIntoContainer>(|world, item, event| {
        let is_hand = world
            .get::<Hands>(event.owner)
            .is_some_and(|hands| hands.hands.contains_key(&*event.container));
        if is_hand {
            let (user, hand) = (event.owner, event.container.to_string());
            world.raise(
                user,
                &mut DidEquipHand {
                    user,
                    item,
                    hand: hand.clone(),
                },
            );
            world.raise(item, &mut GotEquippedHand { user, item, hand });
        }
    });
    world.subscribe::<Item, EntGotRemovedFromContainer>(|world, item, event| {
        let is_hand = world
            .get::<Hands>(event.owner)
            .is_some_and(|hands| hands.hands.contains_key(&*event.container));
        if is_hand {
            let (user, hand) = (event.owner, event.container.to_string());
            world.raise(
                item,
                &mut GotUnequippedHand {
                    user,
                    item,
                    hand: hand.clone(),
                },
            );
            world.raise(user, &mut DidUnequipHand { user, item, hand });
        }
    });
    // Глагол «Взять» (`SharedItemSystem.AddPickupVerb`).
    world.subscribe::<Item, crate::verbs::GetVerbs>(|world, item, event| {
        if event.kind != crate::verbs::VerbKind::Interaction
            || event.using.is_some()
            || !event.can_access
            || !event.can_interact
        {
            return;
        }
        let user = event.user;
        let Some(hand) = world.empty_hand(user) else {
            return;
        };
        if !world.can_pickup_to_hand(user, item, &hand, false) {
            return;
        }
        let in_own_container = world
            .containing_container(item)
            .is_some_and(|container| container.owner == user);
        let key = if in_own_container {
            "pick-up-verb-get-data-text-inventory"
        } else {
            "pick-up-verb-get-data-text"
        };
        let text = crate::examine::loc(world, key, &[]);
        let mut verb =
            crate::verbs::Verb::new(crate::verbs::VerbKind::Interaction, text, move |world| {
                world.try_pickup_any_hand(user, item, false);
            });
        verb.icon = Some("/Textures/Interface/VerbIcons/pickup.svg.192dpi.png".into());
        event.verbs.push(verb);
    });
    world.subscribe_net_event::<HandsRequest>(handle_request);
    bind_inputs(world);
}

#[cfg(test)]
mod tests;
