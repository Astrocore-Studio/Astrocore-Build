//! Статус-эффекты: какие эффекты сущность может получить. Образец — `StatusEffectsComponent`
//! и `StatusEffectsSystem.CanApplyEffect` SS14 @ 2f4a2aad.
//!
//! Пока только список разрешённых эффектов: сами эффекты (оглушение, дрожь, заикание) —
//! отдельной задачей.

use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;

/// Статус-эффекты сущности (`StatusEffectsComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusEffects {
    /// Эффекты, которые сущность может получить.
    pub allowed: Vec<String>,
}

/// Перед добавлением эффекта (`BeforeOldStatusEffectAddedEvent`): обработчик может запретить.
#[derive(Debug, Clone)]
pub struct BeforeStatusEffectAdded {
    pub key: String,
    pub cancelled: bool,
}

/// Можно ли наложить эффект (`CanApplyEffect`).
pub fn can_apply_effect(world: &mut World, entity: Entity, key: &str) -> bool {
    if world.get::<StatusEffects>(entity).is_none() {
        return false;
    }
    let mut event = BeforeStatusEffectAdded {
        key: key.to_owned(),
        cancelled: false,
    };
    world.raise(entity, &mut event);
    if event.cancelled {
        return false;
    }
    // Прототип эффекта (`StatusEffectPrototype`): без него эффекта нет.
    let Some(always_allowed) = world
        .get_resource::<Prototypes>()
        .and_then(|p| p.resolved_node("statusEffect", key))
        .map(|node| {
            node.get("alwaysAllowed")
                .and_then(astrocore_data::DataNode::as_str)
                == Some("true")
        })
    else {
        return false;
    };
    always_allowed
        || world
            .get::<StatusEffects>(entity)
            .is_some_and(|effects| effects.allowed.iter().any(|allowed| allowed == key))
}

pub fn install(world: &mut World, _side: Side) {
    world.register_data_component::<StatusEffects>();
}
