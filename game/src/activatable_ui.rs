//! Интерфейс устройства открывается взаимодействием: «использовать» рядом (E, клик рукой)
//! или использование в руке. Образец — `Content.Shared/UserInterface/ActivatableUISystem.cs`
//! SS14 @ 2f4a2aad.

use astrocore_ecs::{EventWorldExt, GameWorldExt, RegistryWorldExt};
use astrocore_net::{Actor, NetRegistryWorldExt};
use astrocore_ui::{UiWorldExt, UserInterface};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::action_blocker::ActionBlockerWorldExt;
use crate::interaction::{ActivateInWorld, UseInHand};

fn yes() -> bool {
    true
}

/// Интерфейс сущности открывается взаимодействием. В данных — компонент `ActivatableUI`.
#[derive(Component, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivatableUi {
    /// Ключ интерфейса в компоненте `UserInterface`.
    pub key: String,
    /// Открывается только из рук.
    #[serde(default)]
    pub in_hands_only: bool,
    /// Интерфейс открыт только у одного игрока.
    #[serde(default)]
    pub single_user: bool,
    /// Только через глагол, не взаимодействием.
    #[serde(default)]
    pub verb_only: bool,
    /// Нужны сложные действия (руки): животные и призраки не открывают.
    #[serde(default = "yes")]
    pub requires_complex: bool,
    /// Только у админов.
    #[serde(default)]
    pub admin_only: bool,
}

/// Открывает интерфейс игроку или закрывает открытый (`InteractUI`).
pub fn toggle_ui(world: &mut World, user: Entity, target: Entity) -> bool {
    let Some(activatable) = world.get::<ActivatableUi>(target).cloned() else {
        return false;
    };
    // Интерфейс открывается только игроку: у сущности должна быть сессия.
    if world.get::<Actor>(user).is_none() || activatable.admin_only {
        return false;
    }
    if !world.can_interact(user, Some(target)) {
        return false;
    }
    if world.ui_open_for(target, &activatable.key, user) {
        return world.close_ui(target, &activatable.key, user);
    }
    let busy = activatable.single_user
        && world
            .get::<UserInterface>(target)
            .is_some_and(|ui| !ui.actors(&activatable.key).is_empty());
    !busy && world.open_ui(target, &activatable.key, user)
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.add_plugin(astrocore_ui::UiPlugin);
    world.register_data_component_as::<ActivatableUi>("ActivatableUI");
    world.register_net_serde::<ActivatableUi>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<ActivatableUi, ActivateInWorld>(|world, entity, event| {
        let Some(activatable) = world.get::<ActivatableUi>(entity).cloned() else {
            return;
        };
        if event.handled
            || activatable.verb_only
            || activatable.in_hands_only
            || (activatable.requires_complex && !event.complex)
        {
            return;
        }
        event.handled = toggle_ui(world, event.user, entity);
    });
    world.subscribe::<ActivatableUi, UseInHand>(|world, entity, event| {
        let Some(activatable) = world.get::<ActivatableUi>(entity).cloned() else {
            return;
        };
        if event.handled || activatable.verb_only {
            return;
        }
        if activatable.requires_complex && !world.can_complex_interact(event.user) {
            return;
        }
        event.handled = toggle_ui(world, event.user, entity);
    });
}
