//! Замки шкафов, ящиков и машин: открыть или запереть можно с доступом считывателя. Образец —
//! `LockComponent`, `LockSystem` SS14 @ 2f4a2aad (`TryLock`, `TryUnlock`, `CanToggleLock`,
//! `HasUserAccess`, глагол, осмотр, `StorageOpenAttemptEvent`).

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{ComponentInit, EventWorldExt, RegistryWorldExt};
use astrocore_loc::{EntityNameWorldExt, LocArg};
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::access::is_allowed;
use crate::action_blocker::ActionBlockerWorldExt;
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterWorldExt};
use crate::entity_storage::StorageOpenAttempt;
use crate::examine::{Examined, loc};
use crate::interaction::{ActivateInWorld, UseInHand};
use crate::popups::{PopupType, PopupWorldExt};
use crate::verbs::{GetVerbs, Verb, VerbKind};

/// Ключ внешнего вида (`LockVisuals.Locked`).
pub const LOCK_VISUALS: &str = "enum.LockVisuals.Locked";

fn yes() -> bool {
    true
}

fn unlock_sound() -> Option<SoundSpecifier> {
    Some(SoundSpecifier::path("/Audio/Machines/door_lock_off.ogg"))
}

fn lock_sound() -> Option<SoundSpecifier> {
    Some(SoundSpecifier::path("/Audio/Machines/door_lock_on.ogg"))
}

/// Замок (`LockComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lock {
    #[serde(default = "yes")]
    pub locked: bool,
    #[serde(default = "yes")]
    pub show_lock_verbs: bool,
    #[serde(default = "yes")]
    pub show_examine: bool,
    #[serde(default)]
    pub lock_on_click: bool,
    #[serde(default = "yes")]
    pub unlock_on_click: bool,
    #[serde(default)]
    pub lock_in_hand: bool,
    #[serde(default)]
    pub unlock_in_hand: bool,
    #[serde(default = "yes")]
    pub use_access: bool,
    #[serde(default = "unlock_sound", rename = "unlockingSound")]
    pub unlock_sound: Option<SoundSpecifier>,
    #[serde(default = "lock_sound", rename = "lockingSound")]
    pub lock_sound: Option<SoundSpecifier>,
    #[serde(default = "yes")]
    pub break_on_access_breaker: bool,
    /// Секунды на запирание и отпирание.
    #[serde(default)]
    pub lock_time: f64,
    #[serde(default)]
    pub unlock_time: f64,
    #[serde(default)]
    pub allow_repeated_locking: bool,
}

/// Замок переключили (`LockToggledEvent`). Поднимается на сущности.
#[derive(Debug, Clone, Copy)]
pub struct LockToggled {
    pub locked: bool,
}

#[derive(Debug, Clone, Copy)]
struct LockDoAfter {
    lock: bool,
}

/// Заперто ли (`IsLocked`).
pub fn is_locked(world: &World, entity: Entity) -> bool {
    world.get::<Lock>(entity).is_some_and(|lock| lock.locked)
}

/// Может ли действующий переключать замок (`CanToggleLock`).
fn can_toggle(world: &mut World, entity: Entity, user: Entity) -> bool {
    if world.get::<Lock>(entity).is_none() {
        return false;
    }
    world.can_complex_interact(user)
}

/// Есть ли доступ (`HasUserAccess`): с сообщением об отказе.
fn has_access(world: &mut World, entity: Entity, user: Entity, quiet: bool) -> bool {
    if is_allowed(world, user, entity) {
        return true;
    }
    if !quiet {
        let text = loc(world, "lock-comp-has-user-access-fail", &[]);
        world.popup_entity_for(&text, entity, user, PopupType::Small);
    }
    false
}

fn set_locked(world: &mut World, entity: Entity, user: Option<Entity>, locked: bool) {
    let Some(lock) = world.get::<Lock>(entity).cloned() else {
        return;
    };
    if lock.locked == locked {
        return;
    }
    if let Some(user) = user {
        let name = world.entity_name(entity);
        let key = if locked {
            "lock-comp-do-lock-success"
        } else {
            "lock-comp-do-unlock-success"
        };
        let text = loc(world, key, &[("entityName", LocArg::from(name))]);
        world.popup_entity_for(&text, entity, user, PopupType::Small);
    }
    let sound = if locked {
        lock.lock_sound
    } else {
        lock.unlock_sound
    };
    if let Some(sound) = sound {
        world.play_entity(&sound, entity, None);
    }
    if let Some(mut component) = world.get_mut::<Lock>(entity) {
        component.locked = locked;
    }
    world.set_appearance(entity, LOCK_VISUALS, locked);
    world.raise_and_broadcast(entity, &mut LockToggled { locked });
}

/// Запирает без проверок (`Lock`).
pub fn lock(world: &mut World, entity: Entity, user: Option<Entity>) {
    set_locked(world, entity, user, true);
}

/// Отпирает без проверок (`Unlock`).
pub fn unlock(world: &mut World, entity: Entity, user: Option<Entity>) {
    set_locked(world, entity, user, false);
}

fn try_toggle(world: &mut World, entity: Entity, user: Entity, locking: bool) -> bool {
    let Some(component) = world.get::<Lock>(entity).cloned() else {
        return false;
    };
    if !can_toggle(world, entity, user) {
        return false;
    }
    if component.use_access && !has_access(world, entity, user, false) {
        return false;
    }
    let delay = if locking {
        component.lock_time
    } else {
        component.unlock_time
    };
    if delay > 0.0 {
        let mut args = DoAfterArgs::new(user, delay, Some(entity), Some(entity), Some(entity));
        args.break_on_damage = true;
        args.break_on_move = true;
        args.need_hand = true;
        return !matches!(
            world.try_start_do_after(args, LockDoAfter { lock: locking }),
            crate::do_after::DoAfterStart::Refused
        );
    }
    set_locked(world, entity, Some(user), locking);
    true
}

/// Запереть с проверками (`TryLock`).
pub fn try_lock(world: &mut World, entity: Entity, user: Entity) -> bool {
    try_toggle(world, entity, user, true)
}

/// Отпереть с проверками (`TryUnlock`).
pub fn try_unlock(world: &mut World, entity: Entity, user: Entity) -> bool {
    try_toggle(world, entity, user, false)
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Lock>();
    world.register_net_serde::<Lock>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Lock, ComponentInit>(|world, entity, _| {
        let locked = is_locked(world, entity);
        world.set_appearance(entity, LOCK_VISUALS, locked);
    });
    world.subscribe::<Lock, ActivateInWorld>(|world, entity, event| {
        if event.handled || !event.complex {
            return;
        }
        let Some(lock) = world.get::<Lock>(entity).cloned() else {
            return;
        };
        if lock.locked && lock.unlock_on_click {
            event.handled = true;
            try_unlock(world, entity, event.user);
        } else if !lock.locked && lock.lock_on_click {
            event.handled = true;
            try_lock(world, entity, event.user);
        }
    });
    world.subscribe::<Lock, UseInHand>(|world, entity, event| {
        if event.handled {
            return;
        }
        let Some(lock) = world.get::<Lock>(entity).cloned() else {
            return;
        };
        if lock.locked && lock.unlock_in_hand {
            event.handled = true;
            try_unlock(world, entity, event.user);
        } else if !lock.locked && lock.lock_in_hand {
            event.handled = true;
            try_lock(world, entity, event.user);
        }
    });
    world.subscribe::<Lock, StorageOpenAttempt>(|world, entity, event| {
        if !is_locked(world, entity) {
            return;
        }
        if !event.silent {
            let text = loc(world, "entity-storage-component-locked-message", &[]);
            world.popup_entity_for(&text, entity, event.user, PopupType::Small);
        }
        event.cancelled = true;
    });
    world.subscribe::<Lock, Examined>(|world, entity, event| {
        let Some(lock) = world.get::<Lock>(entity).cloned() else {
            return;
        };
        if !lock.show_examine {
            return;
        }
        let name = world.entity_name(entity);
        let key = if lock.locked {
            "lock-comp-on-examined-is-locked"
        } else {
            "lock-comp-on-examined-is-unlocked"
        };
        let text = loc(world, key, &[("entityName", LocArg::from(name))]);
        event.push_markup(text, 0);
    });
    world.subscribe::<Lock, GetVerbs>(|world, entity, event| {
        if event.kind != VerbKind::Alternative
            || !event.can_access
            || !event.can_interact
            || !event.can_complex_interact
        {
            return;
        }
        let Some(lock) = world.get::<Lock>(entity).cloned() else {
            return;
        };
        if !lock.show_lock_verbs {
            return;
        }
        let user = event.user;
        let locked = lock.locked;
        let text = loc(
            world,
            if locked {
                "toggle-lock-verb-unlock"
            } else {
                "toggle-lock-verb-lock"
            },
            &[],
        );
        let mut verb = Verb::new(VerbKind::Alternative, text, move |world| {
            if locked {
                try_unlock(world, entity, user);
            } else {
                try_lock(world, entity, user);
            }
        });
        verb.icon = Some(if locked {
            "/Textures/Interface/VerbIcons/unlock.svg.192dpi.png".into()
        } else {
            "/Textures/Interface/VerbIcons/lock.svg.192dpi.png".into()
        });
        event.verbs.push(verb);
    });
    world.subscribe::<Lock, DoAfterDone<LockDoAfter>>(|world, entity, event| {
        if event.cancelled {
            return;
        }
        set_locked(world, entity, Some(event.user), event.event.lock);
    });
}

#[cfg(test)]
mod tests;
