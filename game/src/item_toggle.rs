//! Включаемые предметы: включение и выключение использованием в руке, «использовать» (E) и
//! глаголом, звуки и сообщения, попытки включить и выключить (системы могут отказать —
//! тогда звук отказа и сообщение), событие переключения, вид `ToggleableVisuals.Enabled`,
//! строка «включено / выключено» при осмотре. Образец — `ItemToggleSystem`,
//! `ItemToggleComponent`, `ItemToggleExaminableStatusComponent` SS14 @ 2f4a2aad.
//!
//! Нет пока: включения при взятии в обе руки (`Wielded`), зацикленного звука включённого
//! (`ItemToggleActiveSound`), требований заряда и замка, смены размера, урона и вида в руке.

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{ComponentStartup, EventWorldExt, MapInit, RegistryWorldExt};
use astrocore_loc::EntityNameWorldExt;
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::action_blocker::ActionBlockerWorldExt;
use crate::examine::{Examined, loc};
use crate::flammable::TOGGLEABLE_ENABLED;
use crate::interaction::{ActivateInWorld, UseInHand};
use crate::popups::{PopupType, PopupWorldExt};
use crate::verbs::{GetVerbs, Verb, VerbKind};

fn yes() -> bool {
    true
}
fn verb_on() -> String {
    "item-toggle-activate".into()
}
fn verb_off() -> String {
    "item-toggle-deactivate".into()
}

/// Включаемый предмет (`ItemToggleComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemToggle {
    #[serde(default)]
    pub activated: bool,
    /// Переключается по «использовать» (E) и глаголом.
    #[serde(default = "yes")]
    pub on_activate: bool,
    /// Переключается использованием в руке.
    #[serde(default = "yes")]
    pub on_use: bool,
    #[serde(default = "verb_on")]
    pub verb_toggle_on: String,
    #[serde(default = "verb_off")]
    pub verb_toggle_off: String,
    #[serde(default = "yes")]
    pub predictable: bool,
    #[serde(default)]
    pub sound_activate: Option<SoundSpecifier>,
    #[serde(default)]
    pub sound_deactivate: Option<SoundSpecifier>,
    #[serde(default)]
    pub popup_activate: Option<String>,
    #[serde(default)]
    pub popup_deactivate: Option<String>,
    #[serde(default)]
    pub sound_fail_to_activate: Option<SoundSpecifier>,
    #[serde(default = "yes")]
    pub require_complex_interact: bool,
}

fn on_text() -> String {
    "item-toggle-examined-on".into()
}
fn off_text() -> String {
    "item-toggle-examined-off".into()
}

/// «Включено / выключено» при осмотре (`ItemToggleExaminableStatusComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemToggleExaminableStatus {
    #[serde(default = "on_text")]
    pub on_text: String,
    #[serde(default = "off_text")]
    pub off_text: String,
}

/// Можно ли включить (`ItemToggleActivateAttemptEvent`). На предмете.
#[derive(Debug, Clone, Default)]
pub struct ItemToggleActivateAttempt {
    pub user: Option<Entity>,
    pub cancelled: bool,
    /// Отказ без звука и сообщения.
    pub silent: bool,
    pub popup: Option<String>,
}

/// Можно ли выключить (`ItemToggleDeactivateAttemptEvent`). На предмете.
#[derive(Debug, Clone, Default)]
pub struct ItemToggleDeactivateAttempt {
    pub user: Option<Entity>,
    pub cancelled: bool,
    pub silent: bool,
    pub popup: Option<String>,
}

/// Предмет включили или выключили (`ItemToggledEvent`). На предмете.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemToggled {
    pub activated: bool,
    pub user: Option<Entity>,
}

/// Включён ли предмет (`IsActivated`); без компонента — считается включённым.
pub fn is_activated(world: &World, entity: Entity) -> bool {
    world.get::<ItemToggle>(entity).is_none_or(|t| t.activated)
}

fn complex_blocked(world: &World, toggle: &ItemToggle, user: Option<Entity>) -> bool {
    user.is_some_and(|user| toggle.require_complex_interact && !world.can_complex_interact(user))
}

/// Переключить (`Toggle`).
pub fn toggle(world: &mut World, entity: Entity, user: Option<Entity>, show_popup: bool) -> bool {
    let Some(active) = world.get::<ItemToggle>(entity).map(|t| t.activated) else {
        return false;
    };
    if active {
        try_deactivate(world, entity, user, show_popup)
    } else {
        try_activate(world, entity, user, show_popup)
    }
}

/// Включить (`TryActivate`).
pub fn try_activate(
    world: &mut World,
    entity: Entity,
    user: Option<Entity>,
    show_popup: bool,
) -> bool {
    let Some(toggle) = world.get::<ItemToggle>(entity).cloned() else {
        return false;
    };
    if toggle.activated {
        return true;
    }
    if complex_blocked(world, &toggle, user) {
        return false;
    }
    let mut attempt = ItemToggleActivateAttempt {
        user,
        ..Default::default()
    };
    world.raise(entity, &mut attempt);
    if attempt.cancelled {
        if attempt.silent {
            return false;
        }
        if let Some(sound) = &toggle.sound_fail_to_activate {
            world.play_entity(sound, entity, None);
        }
        if let (true, Some(popup), Some(user)) = (show_popup, attempt.popup, user) {
            world.popup_entity_for(&popup, entity, user, PopupType::Small);
        }
        return false;
    }
    set_active(world, entity, &toggle, true, user, show_popup);
    true
}

/// Выключить (`TryDeactivate`).
pub fn try_deactivate(
    world: &mut World,
    entity: Entity,
    user: Option<Entity>,
    show_popup: bool,
) -> bool {
    let Some(toggle) = world.get::<ItemToggle>(entity).cloned() else {
        return false;
    };
    if !toggle.activated {
        return true;
    }
    if complex_blocked(world, &toggle, user) {
        return false;
    }
    let mut attempt = ItemToggleDeactivateAttempt {
        user,
        ..Default::default()
    };
    world.raise(entity, &mut attempt);
    if attempt.cancelled {
        if let (false, true, Some(popup), Some(user)) =
            (attempt.silent, show_popup, attempt.popup, user)
        {
            world.popup_entity_for(&popup, entity, user, PopupType::Small);
        }
        return false;
    }
    set_active(world, entity, &toggle, false, user, show_popup);
    true
}

/// Включить или выключить без проверок (`Activate`, `Deactivate`).
fn set_active(
    world: &mut World,
    entity: Entity,
    toggle: &ItemToggle,
    active: bool,
    user: Option<Entity>,
    show_popup: bool,
) {
    let (sound, popup) = if active {
        (&toggle.sound_activate, &toggle.popup_activate)
    } else {
        (&toggle.sound_deactivate, &toggle.popup_deactivate)
    };
    if let Some(sound) = sound {
        world.play_entity(sound, entity, None);
    }
    if let (true, Some(popup), Some(user)) = (show_popup, popup, user) {
        let text = loc(world, popup, &[]);
        world.popup_entity_for(&text, user, user, PopupType::Small);
    }
    if let Some(mut component) = world.get_mut::<ItemToggle>(entity) {
        component.activated = active;
    }
    world.set_appearance(entity, TOGGLEABLE_ENABLED, active);
    world.raise(
        entity,
        &mut ItemToggled {
            activated: active,
            user,
        },
    );
}

/// Глагол «включить / выключить» (`OnActivateVerb`).
fn activation_verb(world: &mut World, entity: Entity, event: &mut GetVerbs) {
    if event.kind != VerbKind::Activation || !event.can_access || !event.can_interact {
        return;
    }
    let Some(component) = world.get::<ItemToggle>(entity).cloned() else {
        return;
    };
    if !component.on_activate || (component.require_complex_interact && !event.can_complex_interact)
    {
        return;
    }
    let user = event.user;
    let cancelled = if component.activated {
        let mut attempt = ItemToggleDeactivateAttempt {
            user: Some(user),
            ..Default::default()
        };
        world.raise(entity, &mut attempt);
        attempt.cancelled
    } else {
        let mut attempt = ItemToggleActivateAttempt {
            user: Some(user),
            ..Default::default()
        };
        world.raise(entity, &mut attempt);
        attempt.cancelled
    };
    if cancelled {
        return;
    }
    let key = if component.activated {
        &component.verb_toggle_off
    } else {
        &component.verb_toggle_on
    };
    event.verbs.push(Verb::new(
        VerbKind::Activation,
        loc(world, key, &[]),
        move |world| {
            toggle(world, entity, Some(user), true);
        },
    ));
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<ItemToggle>();
    world.register_data_component::<ItemToggleExaminableStatus>();
    world.register_net_serde::<ItemToggle>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<ItemToggle, ComponentStartup>(|world, entity, _| {
        let active = is_activated(world, entity);
        world.set_appearance(entity, TOGGLEABLE_ENABLED, active);
    });
    world.subscribe::<ItemToggle, MapInit>(|world, entity, _| {
        if world.get::<ItemToggle>(entity).is_some_and(|t| t.activated) {
            world.raise(
                entity,
                &mut ItemToggled {
                    activated: true,
                    user: None,
                },
            );
        }
    });
    world.subscribe::<ItemToggle, UseInHand>(|world, entity, event| {
        if event.handled || !world.get::<ItemToggle>(entity).is_some_and(|t| t.on_use) {
            return;
        }
        event.handled = true;
        toggle(world, entity, Some(event.user), true);
    });
    world.subscribe::<ItemToggle, ActivateInWorld>(|world, entity, event| {
        if event.handled
            || !world
                .get::<ItemToggle>(entity)
                .is_some_and(|t| t.on_activate)
        {
            return;
        }
        event.handled = true;
        toggle(world, entity, Some(event.user), true);
    });
    world.subscribe::<ItemToggle, GetVerbs>(activation_verb);
    world.subscribe::<ItemToggleExaminableStatus, Examined>(|world, entity, event| {
        let Some(status) = world.get::<ItemToggleExaminableStatus>(entity).cloned() else {
            return;
        };
        let key = if is_activated(world, entity) {
            status.on_text
        } else {
            status.off_text
        };
        let target = world.entity_arg(entity);
        let text = loc(world, &key, &[("target", target.into())]);
        event.push_markup(text, 0);
    });
}

#[cfg(test)]
mod tests;
