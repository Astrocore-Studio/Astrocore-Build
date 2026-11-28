//! Сварочный аппарат: топливо — сварочное топливо в растворе `welder`. Зажечь можно только
//! с топливом (зажигание тратит 0,5), горящий тратит 1 в секунду и гаснет без топлива;
//! сварка требует зажжённого и топлива на действие (проверка и в начале, и во время
//! действия) и тратит его по окончании; щелчок по топливному баку заправляет. Включённый
//! горячий предмет (`ItemToggleHot`) поджигает. Осмотр: зажжён ли и сколько топлива.
//! Образец — `SharedToolSystem.Welder`, `WelderComponent`, `ReagentTankComponent`,
//! `ItemToggleHotComponent`, `SharedIgnitionSourceSystem.OnItemToggle` SS14 @ 2f4a2aad.
//!
//! Нет пока: взрыва и урона бака от горящего аппарата (`DamageOnToolInteract`), защиты
//! глаз (`RequiresEyeProtection`), света, размера и урона включённого (`ItemToggle*`).

use std::time::Duration;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update};
use astrocore_loc::EntityNameWorldExt;
use astrocore_net::NetRegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::examine::{Examined, loc};
use crate::fixed_point::FixedPoint2;
use crate::flammable::{IgnitionSource, IsHot};
use crate::interaction::AfterInteract;
use crate::item_toggle::{ItemToggleActivateAttempt, ItemToggled, is_activated, toggle};
use crate::popups::{PopupType, PopupWorldExt};
use crate::solution::ReagentId;
use crate::solution_container as containers;
use crate::tools::{ToolDoAfterAttempt, ToolDoAfterFinished, ToolUseAttempt};

fn one() -> f32 {
    1.0
}
fn welder_solution() -> String {
    "welder".into()
}
fn welding_fuel() -> String {
    "WeldingFuel".into()
}
fn one_fixed() -> FixedPoint2 {
    FixedPoint2::new(1)
}
fn half() -> FixedPoint2 {
    FixedPoint2::from_cents(50)
}
fn refill() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Effects/refill.ogg")
}

/// Сварочный аппарат (`WelderComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Welder {
    #[serde(default)]
    pub enabled: bool,
    #[serde(skip)]
    pub next_update: Duration,
    /// С.
    #[serde(default = "one")]
    pub welder_update_timer: f32,
    #[serde(default = "welder_solution")]
    pub fuel_solution_name: String,
    #[serde(default = "welding_fuel")]
    pub fuel_reagent: String,
    #[serde(default = "one_fixed")]
    pub fuel_consumption: FixedPoint2,
    #[serde(default = "half")]
    pub fuel_lit_cost: FixedPoint2,
    #[serde(default = "refill")]
    pub welder_refill: SoundSpecifier,
    #[serde(default)]
    pub tank_safe: bool,
}

/// Вид бака (`ReagentTankType`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReagentTankType {
    #[default]
    Unspecified,
    Fuel,
}

fn ten() -> FixedPoint2 {
    FixedPoint2::new(10)
}

/// Бак с реагентом (`ReagentTankComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReagentTank {
    #[serde(default = "ten")]
    pub transfer_amount: FixedPoint2,
    #[serde(default)]
    pub tank_type: ReagentTankType,
}

/// Включённый предмет горячий (`ItemToggleHotComponent`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ItemToggleHot;

fn now(world: &World) -> Duration {
    world.resource::<GameTime>().cur_time()
}

fn fuel_solution(world: &mut World, welder: Entity) -> Option<Entity> {
    let name = world.get::<Welder>(welder)?.fuel_solution_name.clone();
    containers::try_get_solution(world, welder, &name)
}

/// Топливо и вместимость (`GetWelderFuelAndCapacity`).
pub fn fuel_and_capacity(world: &mut World, welder: Entity) -> (FixedPoint2, FixedPoint2) {
    let Some(reagent) = world.get::<Welder>(welder).map(|w| w.fuel_reagent.clone()) else {
        return Default::default();
    };
    let Some(soln) = fuel_solution(world, welder) else {
        return Default::default();
    };
    containers::solution(world, soln).map_or_else(Default::default, |s| {
        (s.prototype_quantity(&reagent), s.max_volume)
    })
}

fn remove_fuel(world: &mut World, welder: Entity, amount: FixedPoint2) {
    let Some(reagent) = world.get::<Welder>(welder).map(|w| w.fuel_reagent.clone()) else {
        return;
    };
    if let Some(soln) = fuel_solution(world, welder) {
        containers::remove_reagent(world, soln, &ReagentId::new(reagent), amount);
    }
}

fn log(world: &mut World, user: Option<Entity>, welder: Entity, state: &str) {
    let who = user.map_or_else(|| "unknown".to_owned(), |u| world.entity_name(u));
    let text = format!("{who} toggled {} {state}", world.entity_name(welder));
    add_admin_log(
        world,
        LogType::InteractActivate,
        LogImpact::Low,
        &text,
        &[],
        &[],
    );
}

/// Зажечь: тратит стоимость зажигания (`TurnOn`).
fn turn_on(world: &mut World, welder: Entity, user: Option<Entity>) {
    if fuel_solution(world, welder).is_none() {
        return;
    }
    let cost = world
        .get::<Welder>(welder)
        .map_or(FixedPoint2::ZERO, |w| w.fuel_lit_cost);
    remove_fuel(world, welder, cost);
    log(world, user, welder, "on");
    if let Some(mut component) = world.get_mut::<Welder>(welder) {
        component.enabled = true;
    }
}

/// Погасить (`TurnOff`).
fn turn_off(world: &mut World, welder: Entity, user: Option<Entity>) {
    log(world, user, welder, "off");
    if let Some(mut component) = world.get_mut::<Welder>(welder) {
        component.enabled = false;
    }
}

fn popup(world: &mut World, key: &str, welder: Entity, user: Entity) {
    let text = loc(world, key, &[("owner", world.entity_arg(welder).into())]);
    world.popup_entity_for(&text, welder, user, PopupType::Small);
}

/// Можно ли варить (`CanCancelWelderUse`): зажжён и хватает топлива.
fn can_weld(world: &mut World, welder: Entity, user: Entity, required: f32) -> bool {
    let mut ok = true;
    if !is_activated(world, welder) {
        popup(
            world,
            "welder-component-welder-not-lit-message",
            welder,
            user,
        );
        ok = false;
    }
    let (fuel, _) = fuel_and_capacity(world, welder);
    if FixedPoint2::from_f32(required) > fuel {
        popup(world, "welder-component-cannot-weld-message", welder, user);
        ok = false;
    }
    ok
}

/// Заправка от топливного бака (`OnWelderAfterInteract`).
fn on_after_interact(world: &mut World, welder: Entity, event: &mut AfterInteract) {
    if event.handled || !event.can_reach {
        return;
    }
    let Some(target) = event.target else {
        return;
    };
    if world
        .get::<ReagentTank>(target)
        .is_none_or(|t| t.tank_type != ReagentTankType::Fuel)
    {
        return;
    }
    let (Some(tank), Some(own)) = (
        containers::drainable_solution(world, target),
        fuel_solution(world, welder),
    ) else {
        return;
    };
    let tank_volume = containers::solution(world, tank).map_or(FixedPoint2::ZERO, |s| s.volume);
    let available =
        containers::solution(world, own).map_or(FixedPoint2::ZERO, |s| s.available_volume());
    let amount = available.min(tank_volume);
    let user = event.user;
    if amount > FixedPoint2::ZERO {
        let drained = containers::split_solution(world, tank, amount);
        containers::try_add_solution(world, own, &drained);
        if let Some(sound) = world.get::<Welder>(welder).map(|w| w.welder_refill.clone()) {
            world.play_entity(&sound, welder, None);
        }
        let text = loc(
            world,
            "welder-component-after-interact-refueled-message",
            &[],
        );
        world.popup_entity_for(&text, welder, user, PopupType::Small);
    } else if available <= FixedPoint2::ZERO {
        popup(world, "welder-component-already-full", welder, user);
    } else {
        let text = loc(
            world,
            "welder-component-no-fuel-in-tank",
            &[("target", world.entity_arg(target).into())],
        );
        world.popup_entity_for(&text, welder, user, PopupType::Small);
    }
    event.handled = true;
}

/// Расход горящих (`UpdateWelders`).
fn update(world: &mut World) {
    let time = now(world);
    let due: Vec<Entity> = world
        .query::<(Entity, &Welder)>()
        .iter(world)
        .filter(|(_, w)| time >= w.next_update)
        .map(|(e, _)| e)
        .collect();
    for welder in due {
        let Some(component) = world.get::<Welder>(welder).cloned() else {
            continue;
        };
        if let Some(mut w) = world.get_mut::<Welder>(welder) {
            w.next_update += Duration::from_secs_f32(component.welder_update_timer);
        }
        if !component.enabled {
            continue;
        }
        if fuel_solution(world, welder).is_none() {
            continue;
        }
        remove_fuel(
            world,
            welder,
            component
                .fuel_consumption
                .scale(component.welder_update_timer),
        );
        let (fuel, _) = fuel_and_capacity(world, welder);
        if fuel <= FixedPoint2::ZERO {
            toggle(world, welder, None, true);
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Welder>();
    world.register_net_serde::<Welder>();
    world.register_data_component::<ReagentTank>();
    world.register_data_component::<ItemToggleHot>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Welder, MapInit>(|world, welder, _| {
        let time = now(world);
        if let Some(mut component) = world.get_mut::<Welder>(welder) {
            component.next_update = time + Duration::from_secs_f32(component.welder_update_timer);
        }
    });
    world.subscribe::<Welder, Examined>(|world, welder, event| {
        let key = if is_activated(world, welder) {
            "welder-component-on-examine-welder-lit-message"
        } else {
            "welder-component-on-examine-welder-not-lit-message"
        };
        let line = loc(world, key, &[]);
        event.push_markup(line, 0);
        if event.is_in_details_range {
            let (fuel, capacity) = fuel_and_capacity(world, welder);
            let color = if fuel < capacity / FixedPoint2::new(4) {
                "darkorange"
            } else {
                "orange"
            };
            let line = loc(
                world,
                "welder-component-on-examine-detailed-message",
                &[
                    ("colorName", color.into()),
                    ("fuelLeft", fuel.to_string().into()),
                    ("fuelCapacity", capacity.to_string().into()),
                    ("status", "".into()),
                ],
            );
            event.push_markup(line, 0);
        }
    });
    world.subscribe::<Welder, AfterInteract>(on_after_interact);
    world.subscribe::<Welder, ToolUseAttempt>(|world, welder, event| {
        if !can_weld(world, welder, event.user, event.fuel) {
            event.cancelled = true;
        }
    });
    world.subscribe::<Welder, ToolDoAfterAttempt>(|world, welder, event| {
        if !can_weld(world, welder, event.user, event.fuel) {
            event.cancelled = true;
        }
    });
    world.subscribe::<Welder, ToolDoAfterFinished>(|world, welder, event| {
        if event.cancelled {
            return;
        }
        remove_fuel(world, welder, FixedPoint2::from_f32(event.fuel));
    });
    world.subscribe::<Welder, ItemToggled>(|world, welder, event| {
        if event.activated {
            turn_on(world, welder, event.user);
        } else {
            turn_off(world, welder, event.user);
        }
    });
    world.subscribe::<Welder, ItemToggleActivateAttempt>(|world, welder, event| {
        let Some(reagent) = world.get::<Welder>(welder).map(|w| w.fuel_reagent.clone()) else {
            return;
        };
        let message = loc(
            world,
            "welder-component-no-fuel-message",
            &[("owner", world.entity_arg(welder).into())],
        );
        let Some(soln) = fuel_solution(world, welder) else {
            event.cancelled = true;
            event.popup = Some(message);
            return;
        };
        let fuel = containers::solution(world, soln)
            .map_or(FixedPoint2::ZERO, |s| s.prototype_quantity(&reagent));
        let cost = world
            .get::<Welder>(welder)
            .map_or(FixedPoint2::ZERO, |w| w.fuel_lit_cost);
        if fuel == FixedPoint2::ZERO || fuel < cost {
            event.popup = Some(message);
            event.cancelled = true;
        }
    });
    world.subscribe::<ItemToggleHot, IsHot>(|world, item, event| {
        event.is_hot |= is_activated(world, item);
    });
    world.subscribe::<ItemToggleHot, ItemToggled>(|world, item, event| {
        if let Some(mut source) = world.get_mut::<IgnitionSource>(item) {
            source.ignited = event.activated;
        }
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
