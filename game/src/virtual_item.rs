//! Виртуальный предмет: занимает руку, пока она занята чем-то вне рук (тянет ящик, держит
//! наручники). Выпущенный из руки — исчезает и сообщает об этом. Образец —
//! `Content.Shared/Inventory/VirtualItem` SS14 @ 2f4a2aad.

use astrocore_ecs::{EventWorldExt, LifecycleWorldExt, RegistryWorldExt};
use astrocore_map::{SpawnWorldExt, TransformWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::action_blocker::GettingInteractedWithAttempt;
use crate::hands::{GotUnequippedHand, HandsWorldExt};
use crate::interaction::BeforeRangedInteract;
use crate::inventory::EquipAttempt;

/// Прототип виртуального предмета (`VirtualItem`).
pub const VIRTUAL_ITEM: &str = "VirtualItem";

fn placeholder() -> Entity {
    Entity::PLACEHOLDER
}

/// Что занимает руку. В данных — компонент `VirtualItem`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VirtualItem {
    /// Сущность, ради которой занята рука.
    #[serde(skip, default = "placeholder")]
    pub blocking_entity: Entity,
}

impl Default for VirtualItem {
    fn default() -> Self {
        Self {
            blocking_entity: Entity::PLACEHOLDER,
        }
    }
}

/// Виртуальный предмет удалён (`VirtualItemDeletedEvent`). На держащем и на сущности,
/// которую он обозначал.
#[derive(Debug, Clone, Copy)]
pub struct VirtualItemDeleted {
    pub blocking_entity: Entity,
    pub user: Entity,
}

/// Виртуальные предметы на `World`.
pub trait VirtualItemWorldExt {
    /// Занять свободную руку (`TrySpawnVirtualItemInHand`).
    fn try_spawn_virtual_item_in_hand(&mut self, blocking: Entity, user: Entity) -> Option<Entity>;
    /// Убрать из рук виртуальные предметы этой сущности (`DeleteInHandsMatching`).
    fn delete_in_hands_matching(&mut self, user: Entity, matching: Entity);
    /// Удалить виртуальный предмет с событиями (`DeleteVirtualItem`).
    fn delete_virtual_item(&mut self, item: Entity, user: Entity);
}

impl VirtualItemWorldExt for World {
    fn try_spawn_virtual_item_in_hand(&mut self, blocking: Entity, user: Entity) -> Option<Entity> {
        let hand = self.empty_hand(user)?;
        let at = self.coordinates(user)?;
        let item = self.spawn_attached(VIRTUAL_ITEM, at).ok()?;
        if let Some(mut component) = self.get_mut::<VirtualItem>(item) {
            component.blocking_entity = blocking;
        }
        if !self.try_pickup(user, item, Some(&hand), false) {
            self.delete_entity(item);
            return None;
        }
        Some(item)
    }

    fn delete_in_hands_matching(&mut self, user: Entity, matching: Entity) {
        for held in self.held_items(user) {
            if self
                .get::<VirtualItem>(held)
                .is_some_and(|v| v.blocking_entity == matching)
            {
                self.delete_virtual_item(held, user);
            }
        }
    }

    fn delete_virtual_item(&mut self, item: Entity, user: Entity) {
        let Some(blocking) = self.get::<VirtualItem>(item).map(|v| v.blocking_entity) else {
            return;
        };
        let mut event = VirtualItemDeleted {
            blocking_entity: blocking,
            user,
        };
        self.raise(user, &mut event);
        if self.is_alive(blocking) {
            self.raise(blocking, &mut event);
        }
        if self.is_alive(item) {
            self.queue_delete(item);
        }
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<VirtualItem>();
    if !side.is_server() {
        return;
    }
    // Выпущенный из руки виртуальный предмет исчезает (`SharedHandsSystem.Drop`).
    world.subscribe::<VirtualItem, GotUnequippedHand>(|world, item, event| {
        world.delete_virtual_item(item, event.user);
    });
    world.subscribe::<VirtualItem, EquipAttempt>(|_, item, event| {
        if event.item == item {
            event.cancelled = true;
        }
    });
    world.subscribe::<VirtualItem, BeforeRangedInteract>(|_, _, event| {
        event.handled = true;
    });
    world.subscribe::<VirtualItem, GettingInteractedWithAttempt>(|_, _, event| {
        event.cancelled = true;
    });
}
