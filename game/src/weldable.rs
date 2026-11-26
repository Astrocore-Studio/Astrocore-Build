//! Заваривание сваркой: двери, шкафы, вентиляции. Образец — `WeldableComponent`,
//! `LayerChangeOnWeldComponent` и `WeldableSystem` SS14 @ 2f4a2aad.
//!
//! Топливо сварки списывается с бака сварочного аппарата вместе с химией; пока инструмент
//! с качеством `Welding` варит без расхода.

use astrocore_ecs::{ComponentInit, EventWorldExt, RegistryWorldExt};
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use astrocore_physics::{CollisionGroups, Fixtures, PhysicsWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::do_after::DoAfterDone;
use crate::entity_storage::{EntityStorage, StorageOpenAttempt};
use crate::examine::{Examined, loc};
use crate::interaction::InteractUsing;
use crate::popups::{PopupType, PopupWorldExt};
use crate::tools::{ToolWorldExt, register_tool_event};

/// Ключ внешнего вида (`WeldableVisuals.IsWelded`).
pub const WELDED_VISUALS: &str = "enum.WeldableVisuals.IsWelded";

fn welding() -> String {
    "Welding".into()
}
fn one() -> f64 {
    1.0
}
fn three() -> f32 {
    3.0
}
fn welded_message() -> Option<String> {
    Some("weldable-component-examine-is-welded".into())
}

/// Можно заварить (`WeldableComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Weldable {
    #[serde(default = "welding")]
    pub welding_quality: String,
    /// Секунды.
    #[serde(default = "one")]
    pub time: f64,
    #[serde(default = "three")]
    pub fuel: f32,
    #[serde(default = "welded_message")]
    pub welded_examine_message: Option<String>,
    #[serde(default)]
    pub is_welded: bool,
}

/// Заваренное меняет слой фикстур (`LayerChangeOnWeldComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerChangeOnWeld {
    pub un_welded_layer: CollisionGroups,
    pub welded_layer: CollisionGroups,
}

/// Можно ли варить (`WeldableAttemptEvent`). Поднимается на цели.
#[derive(Debug, Clone, Copy)]
pub struct WeldableAttempt {
    pub user: Entity,
    pub tool: Entity,
    pub cancelled: bool,
}

/// Заварено или разварено (`WeldableChangedEvent`).
#[derive(Debug, Clone, Copy)]
pub struct WeldableChanged {
    pub is_welded: bool,
}

/// Окончание сварки (`WeldFinishedEvent`).
#[derive(Debug, Clone, Copy)]
pub struct WeldFinished;

/// Заварено ли (`IsWelded`).
pub fn is_welded(world: &World, entity: Entity) -> bool {
    world.get::<Weldable>(entity).is_some_and(|w| w.is_welded)
}

fn can_weld(world: &mut World, entity: Entity, tool: Entity, user: Entity) -> bool {
    if world.get::<Weldable>(entity).is_none() {
        return false;
    }
    let mut attempt = WeldableAttempt {
        user,
        tool,
        cancelled: false,
    };
    world.raise(entity, &mut attempt);
    !attempt.cancelled
}

/// Начать сварку (`TryWeld`).
pub fn try_weld(world: &mut World, entity: Entity, tool: Entity, user: Entity) -> bool {
    let Some(weldable) = world.get::<Weldable>(entity).cloned() else {
        return false;
    };
    if !can_weld(world, entity, tool, user) {
        return false;
    }
    world.use_tool(
        tool,
        user,
        Some(entity),
        weldable.time,
        &[&weldable.welding_quality],
        WeldFinished,
        weldable.fuel,
    )
}

/// Заварить или разварить без проверок (`SetWeldedState`).
pub fn set_welded(world: &mut World, entity: Entity, welded: bool) {
    let Some(mut weldable) = world.get_mut::<Weldable>(entity) else {
        return;
    };
    if weldable.is_welded == welded {
        return;
    }
    weldable.is_welded = welded;
    world.raise(entity, &mut WeldableChanged { is_welded: welded });
    world.set_appearance(entity, WELDED_VISUALS, welded);
}

fn on_layer_change(world: &mut World, entity: Entity, welded: bool) {
    let Some(change) = world.get::<LayerChangeOnWeld>(entity).cloned() else {
        return;
    };
    let (from, to) = if welded {
        (change.un_welded_layer, change.welded_layer)
    } else {
        (change.welded_layer, change.un_welded_layer)
    };
    let Some(mut fixtures) = world.get_mut::<Fixtures>(entity) else {
        return;
    };
    let mut changed = false;
    for fixture in fixtures.fixtures.values_mut() {
        if fixture.layer == from {
            fixture.layer = to;
            changed = true;
        }
    }
    if changed {
        world.refresh_fixtures(entity);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Weldable>();
    world.register_data_component::<LayerChangeOnWeld>();
    world.register_net_serde::<Weldable>();
    if !side.is_server() {
        return;
    }
    register_tool_event::<WeldFinished>(world);
    world.subscribe::<Weldable, ComponentInit>(|world, entity, _| {
        let welded = is_welded(world, entity);
        world.set_appearance(entity, WELDED_VISUALS, welded);
    });
    world.subscribe::<Weldable, Examined>(|world, entity, event| {
        let Some(weldable) = world.get::<Weldable>(entity).cloned() else {
            return;
        };
        if let (true, Some(message)) = (weldable.is_welded, weldable.welded_examine_message) {
            let text = loc(world, &message, &[]);
            event.push_markup(text, 0);
        }
    });
    world.subscribe::<Weldable, InteractUsing>(|world, entity, event| {
        if event.handled {
            return;
        }
        event.handled = try_weld(world, entity, event.used, event.user);
    });
    world.subscribe::<Weldable, DoAfterDone<WeldFinished>>(|world, entity, event| {
        let Some(tool) = event.used else {
            return;
        };
        if event.cancelled || !can_weld(world, entity, tool, event.user) {
            return;
        }
        set_welded(world, entity, !is_welded(world, entity));
    });
    // Заваренный шкаф не открыть (`SharedEntityStorageSystem.CanOpen`), открытый — не
    // заварить (`OnWeldableAttempt`).
    world.subscribe::<Weldable, StorageOpenAttempt>(|world, entity, event| {
        if event.cancelled || !is_welded(world, entity) {
            return;
        }
        if !event.silent {
            let text = loc(world, "entity-storage-component-welded-shut-message", &[]);
            world.popup_entity_for(&text, entity, event.user, PopupType::Small);
        }
        event.cancelled = true;
    });
    world.subscribe::<EntityStorage, WeldableAttempt>(|world, entity, event| {
        if world.get::<EntityStorage>(entity).is_some_and(|s| s.open) {
            event.cancelled = true;
        }
    });
    world.subscribe::<LayerChangeOnWeld, WeldableChanged>(|world, entity, event| {
        on_layer_change(world, entity, event.is_welded);
    });
}
