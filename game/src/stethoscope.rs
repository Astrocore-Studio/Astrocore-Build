//! Стетоскоп: надетый даёт действие и глагол «Послушать стетоскопом» на живое существо.
//! Слушание повторяется каждые 1,75 с, пока слушающий стоит на месте: сообщение о дыхании по
//! урону удушья (`Asphyxiation`) и, начиная со второго раза, лучше оно, хуже или ровно.
//! Мёртвый или не задыхающийся — «ничего не слышно». Образец — `StethoscopeComponent`,
//! `StethoscopeSystem`, `StethoscopeDoAfterEvent` SS14 @ 2f4a2aad.

use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_map::ContainerWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::actions::{ActionEvent, GetItemActions, ensure_action};
use crate::damage::Damageable;
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterWorldExt, DuplicateConditions};
use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::inventory::Inventory;
use crate::mobs::{MobStateComponent, MobStateWorldExt};
use crate::popups::{PopupType, PopupWorldExt};
use crate::verbs::{GetVerbs, Verb, VerbKind};

/// Урон, который «слышен» (`DamageToListenFor`).
const DAMAGE_TO_LISTEN_FOR: &str = "Asphyxiation";

fn delay() -> f32 {
    1.75
}
fn action() -> String {
    "ActionStethoscope".into()
}

/// Стетоскоп (`StethoscopeComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stethoscope {
    /// Между прослушиваниями, с.
    #[serde(default = "delay")]
    pub delay: f32,
    #[serde(default)]
    pub last_measured_damage: Option<FixedPoint2>,
    #[serde(default = "action")]
    pub action: String,
    #[serde(skip)]
    pub action_entity: Option<Entity>,
}

/// Прослушивание (`StethoscopeDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
pub struct StethoscopeDoAfter;

/// Сообщение о дыхании по урону удушья (`GetAbsoluteDamageString`).
pub fn absolute_damage_key(damage: FixedPoint2) -> &'static str {
    match damage.int() {
        i32::MIN..10 => "stethoscope-normal",
        10..30 => "stethoscope-raggedy",
        30..60 => "stethoscope-hyper",
        60..80 => "stethoscope-irregular",
        _ => "stethoscope-fucked",
    }
}

/// Лучше, хуже или ровно (`GetDeltaDamageString`).
pub fn delta_damage_key(last: FixedPoint2, current: FixedPoint2) -> &'static str {
    if last > current {
        "stethoscope-delta-improving"
    } else if last < current {
        "stethoscope-delta-worsening"
    } else {
        "stethoscope-delta-steady"
    }
}

/// Начать слушать (`StartListening`): слушает тот, на ком стетоскоп.
fn start_listening(world: &mut World, stethoscope: Entity, target: Entity) {
    let Some(wearer) = world.containing_container(stethoscope).map(|c| c.owner) else {
        return;
    };
    let Some(delay) = world.get::<Stethoscope>(stethoscope).map(|s| s.delay) else {
        return;
    };
    let mut args = DoAfterArgs::new(
        wearer,
        f64::from(delay),
        Some(target),
        Some(stethoscope),
        Some(stethoscope),
    );
    args.duplicate_condition = DuplicateConditions {
        same_target: false,
        same_tool: false,
        same_event: true,
    };
    args.break_on_move = true;
    args.hidden = true;
    args.break_on_hand_change = false;
    world.try_start_do_after(args, StethoscopeDoAfter);
}

/// Послушать (`ExamineWithStethoscope`).
fn examine_with_stethoscope(world: &mut World, stethoscope: Entity, user: Entity, target: Entity) {
    let asphyxiation = world
        .get::<Damageable>(target)
        .and_then(|d| d.damage.types.get(DAMAGE_TO_LISTEN_FOR).copied());
    let audible = world.get::<MobStateComponent>(target).is_some() && !world.is_mob_dead(target);
    let Some(damage) = asphyxiation.filter(|_| audible) else {
        let text = loc(world, "stethoscope-nothing", &[]);
        world.popup_entity_for(&text, target, user, PopupType::Small);
        if let Some(mut component) = world.get_mut::<Stethoscope>(stethoscope) {
            component.last_measured_damage = None;
        }
        return;
    };
    let absolute = loc(world, absolute_damage_key(damage), &[]);
    let last = world
        .get::<Stethoscope>(stethoscope)
        .and_then(|s| s.last_measured_damage);
    let text = match last {
        // В первый раз — без изменения.
        None => absolute,
        Some(last) => {
            let delta = loc(world, delta_damage_key(last, damage), &[]);
            loc(
                world,
                "stethoscope-combined-status",
                &[("absolute", absolute.into()), ("delta", delta.into())],
            )
        }
    };
    world.popup_entity_for(&text, target, user, PopupType::Small);
    if let Some(mut component) = world.get_mut::<Stethoscope>(stethoscope) {
        component.last_measured_damage = Some(damage);
    }
}

fn on_do_after(
    world: &mut World,
    stethoscope: Entity,
    event: &mut DoAfterDone<StethoscopeDoAfter>,
) {
    let target = event.target;
    let Some(target) = target.filter(|_| !event.handled && !event.cancelled) else {
        if let Some(mut component) = world.get_mut::<Stethoscope>(stethoscope) {
            component.last_measured_damage = None;
        }
        return;
    };
    examine_with_stethoscope(world, stethoscope, event.user, target);
    event.repeat = true;
}

/// Глагол надетого стетоскопа (`AddStethoscopeVerb` через `InventoryRelayedEvent`).
fn wearer_verbs(world: &mut World, wearer: Entity, event: &mut GetVerbs) {
    if event.kind != VerbKind::Innate || !event.can_interact || !event.can_access {
        return;
    }
    let target = event.target;
    if world.get::<MobStateComponent>(target).is_none() {
        return;
    }
    let Some(inventory) = world.get::<Inventory>(wearer) else {
        return;
    };
    let worn: Vec<Entity> = inventory
        .slots
        .iter()
        .filter_map(|slot| {
            world
                .container(wearer, &slot.name)?
                .entities()
                .first()
                .copied()
        })
        .filter(|item| world.get::<Stethoscope>(*item).is_some())
        .collect();
    for stethoscope in worn {
        let mut verb = Verb::new(
            VerbKind::Innate,
            loc(world, "stethoscope-verb", &[]),
            move |world| start_listening(world, stethoscope, target),
        );
        verb.priority = 2;
        event.verbs.push(verb);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Stethoscope>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Stethoscope, GetItemActions>(|world, stethoscope, event| {
        let Some(component) = world.get::<Stethoscope>(stethoscope) else {
            return;
        };
        let (existing, prototype) = (component.action_entity, component.action.clone());
        if let Some(action) = ensure_action(world, stethoscope, existing, &prototype) {
            if let Some(mut component) = world.get_mut::<Stethoscope>(stethoscope) {
                component.action_entity = Some(action);
            }
            event.actions.push(action);
        }
    });
    world.subscribe::<Stethoscope, ActionEvent>(|world, stethoscope, event| {
        if event.handled || event.kind != "StethoscopeActionEvent" {
            return;
        }
        if let Some(target) = event.target {
            start_listening(world, stethoscope, target);
            event.handled = true;
        }
    });
    world.subscribe::<Stethoscope, DoAfterDone<StethoscopeDoAfter>>(on_do_after);
    world.subscribe::<Inventory, GetVerbs>(wearer_verbs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breathing_messages_follow_asphyxiation() {
        let key = |value| absolute_damage_key(FixedPoint2::new(value));
        assert_eq!(key(0), "stethoscope-normal");
        assert_eq!(key(10), "stethoscope-raggedy");
        assert_eq!(key(59), "stethoscope-hyper");
        assert_eq!(key(60), "stethoscope-irregular");
        assert_eq!(key(80), "stethoscope-fucked");
        let delta = |a, b| delta_damage_key(FixedPoint2::new(a), FixedPoint2::new(b));
        assert_eq!(delta(20, 10), "stethoscope-delta-improving");
        assert_eq!(delta(10, 20), "stethoscope-delta-worsening");
        assert_eq!(delta(10, 10), "stethoscope-delta-steady");
    }
}
