//! Удар током: решётки на кабеле, шлюзы с закороченным проводом питания. Образец —
//! `ElectrifiedComponent`, `InsulatedComponent`, `ElectrocutionSystem` (`TryDoElectrifiedAct`,
//! `TryDoElectrocution`, `DoCommonElectrocution`) и `SharedElectrocutionSystem` SS14 @
//! 2f4a2aad.
//!
//! Урон — типом `Shock`, ослабленный изоляцией перчаток и одежды. Оглушение, заикание и
//! дрожь — вместе со статус-эффектами; виртуальная нагрузка на сеть при ударе от кабеля —
//! тоже позже.

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update};
use astrocore_loc::{EntityNameWorldExt, LocArg};
use astrocore_map::{MapGrid, TileCoord, Transform};
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use astrocore_physics::StartCollide;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::combat::{Attacked, CombatWorldExt};
use crate::damage::{DamageSpecifier, DamageableWorldExt};
use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::interaction::{ActivateInWorld, InteractHand, InteractUsing};
use crate::inventory::{Inventory, InventoryWorldExt, SlotFlags};
use crate::popups::{PopupType, PopupWorldExt};
use crate::power::{is_powered, node_max_supply};
use crate::random::random;
use crate::status_effects::can_apply_effect;
use crate::tag::TagWorldExt;

/// Внешний вид: искры (`ElectrifiedVisuals.ShowSparks`).
pub const SHOW_SPARKS: &str = "enum.ElectrifiedVisuals.ShowSparks";
/// Внешний вид: под током (`ElectrifiedVisuals.IsElectrified`).
pub const IS_ELECTRIFIED: &str = "enum.ElectrifiedVisuals.IsElectrified";

const STATUS_KEY: &str = "Electrocution";
const DAMAGE_TYPE: &str = "Shock";
const WINDOW_TAG: &str = "Window";
const RECURSIVE_DAMAGE_MULTIPLIER: f32 = 0.75;
/// Сколько видны искры после удара (`ActivatedElectrifiedComponent.TimeLeft`).
const SPARKS_TIME: f64 = 1.0;

fn yes() -> bool {
    true
}
fn f32_value<const N: u32, const D: u32>() -> f32 {
    N as f32 / D as f32
}
fn sparks() -> SoundSpecifier {
    SoundSpecifier::collection("sparks")
}

/// Бьёт током (`ElectrifiedComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Electrified {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub on_bump: bool,
    #[serde(default = "yes")]
    pub on_attacked: bool,
    #[serde(default)]
    pub no_window_in_tile: bool,
    #[serde(default = "yes")]
    pub on_hand_interact: bool,
    #[serde(default = "yes")]
    pub on_interact_using: bool,
    #[serde(default)]
    pub on_activate_in_world: bool,
    #[serde(default = "yes")]
    pub require_power: bool,
    #[serde(default)]
    pub uses_apc_power: bool,
    #[serde(default)]
    pub high_voltage_node: Option<String>,
    #[serde(default)]
    pub medium_voltage_node: Option<String>,
    #[serde(default)]
    pub low_voltage_node: Option<String>,
    #[serde(default = "f32_value::<3, 1>")]
    pub high_voltage_damage_multiplier: f32,
    #[serde(default = "f32_value::<2, 1>")]
    pub high_voltage_time_multiplier: f32,
    #[serde(default = "f32_value::<2, 1>")]
    pub medium_voltage_damage_multiplier: f32,
    #[serde(default = "f32_value::<3, 2>")]
    pub medium_voltage_time_multiplier: f32,
    #[serde(default = "f32_value::<15, 2>")]
    pub shock_damage: f32,
    #[serde(default = "f32_value::<5, 1>")]
    pub shock_time: f32,
    /// Пауза между ударами, секунды.
    #[serde(default)]
    pub shock_delay: Option<f64>,
    #[serde(skip)]
    pub next_shock: f64,
    #[serde(default = "f32_value::<1, 1>")]
    pub siemens_coefficient: f32,
    #[serde(default = "sparks")]
    pub shock_noises: SoundSpecifier,
    #[serde(default = "yes")]
    pub play_sound_on_shock: bool,
    #[serde(default = "f32_value::<20, 1>")]
    pub shock_volume: f32,
    #[serde(default = "f32_value::<1, 1>")]
    pub probability: f32,
    #[serde(default)]
    pub is_wire_cut: bool,
}

/// Изоляция (`InsulatedComponent`): множитель проводимости, 0 — не пропускает ток.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Insulated {
    #[serde(default)]
    pub coefficient: f32,
}

/// Сущность недавно била током: видны искры (`ActivatedElectrifiedComponent`).
#[derive(Component, Debug, Clone, Copy)]
pub struct ActivatedElectrified {
    pub until: f64,
}

/// Попытка ударить током (`ElectrocutionAttemptEvent`). Поднимается на цели и на её
/// одежде во всех слотах, кроме карманов.
#[derive(Debug, Clone, Copy)]
pub struct ElectrocutionAttempt {
    pub target: Entity,
    pub source: Option<Entity>,
    pub siemens_coefficient: f32,
    pub cancelled: bool,
}

/// Ударило током (`ElectrocutedEvent`). На цели и широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct Electrocuted {
    pub target: Entity,
    pub source: Option<Entity>,
    pub siemens_coefficient: f32,
}

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

/// Под напряжением ли (`SetElectrified`).
pub fn set_electrified(world: &mut World, entity: Entity, value: bool) {
    let Some(mut electrified) = world.get_mut::<Electrified>(entity) else {
        return;
    };
    if electrified.enabled == value {
        return;
    }
    electrified.enabled = value;
    world.set_appearance(entity, IS_ELECTRIFIED, value);
}

/// Провод питания перерезан (`SetElectrifiedWireCut`).
pub fn set_electrified_wire_cut(world: &mut World, entity: Entity, value: bool) {
    if let Some(mut electrified) = world.get_mut::<Electrified>(entity)
        && electrified.is_wire_cut != value
    {
        electrified.is_wire_cut = value;
    }
}

/// Узел под напряжением, по убыванию напряжения (`PoweredNode`).
fn powered_node(
    world: &World,
    entity: Entity,
    electrified: &Electrified,
) -> Option<(String, Voltage)> {
    [
        (&electrified.high_voltage_node, Voltage::High),
        (&electrified.medium_voltage_node, Voltage::Medium),
        (&electrified.low_voltage_node, Voltage::Low),
    ]
    .into_iter()
    .find_map(|(node, voltage)| {
        let node = node.as_ref()?;
        (node_max_supply(world, entity, node) > 0.0).then(|| (node.clone(), voltage))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Voltage {
    High,
    Medium,
    Low,
}

/// На клетке стоит окно.
fn window_in_tile(world: &World, entity: Entity) -> bool {
    let Some(transform) = world.get::<Transform>(entity) else {
        return false;
    };
    let Some(grid) = transform.grid().and_then(|g| world.get::<MapGrid>(g)) else {
        return false;
    };
    let cell = TileCoord::containing(transform.local_position(), transform.level());
    grid.anchored_at(cell)
        .iter()
        .any(|other| world.has_tag(*other, WINDOW_TAG))
}

/// Бьёт ли сейчас током (`IsPowered`).
fn electrified_powered(world: &World, entity: Entity, electrified: &Electrified) -> bool {
    if !electrified.enabled {
        return false;
    }
    if electrified.no_window_in_tile && window_in_tile(world, entity) {
        return false;
    }
    if electrified.uses_apc_power {
        is_powered(world, entity)
    } else {
        !electrified.require_power || powered_node(world, entity, electrified).is_some()
    }
}

/// Проводимость цели с учётом изоляции одежды (`DoCommonElectrocutionAttempt`).
fn electrocution_attempt(
    world: &mut World,
    target: Entity,
    source: Option<Entity>,
    siemens: f32,
    ignore_insulation: bool,
) -> Option<f32> {
    let mut event = ElectrocutionAttempt {
        target,
        source,
        siemens_coefficient: siemens,
        cancelled: false,
    };
    world.raise_and_broadcast(target, &mut event);
    if !ignore_insulation {
        // Ретрансляция в одежду (`InventoryRelayedEvent`), кроме карманов.
        let slots: Vec<(String, SlotFlags)> = world
            .get::<Inventory>(target)
            .map(|inventory| {
                inventory
                    .slots
                    .iter()
                    .map(|slot| (slot.name.clone(), slot.slot_flags))
                    .collect()
            })
            .unwrap_or_default();
        for (slot, flags) in slots {
            if !flags.intersects(SlotFlags(!SlotFlags::POCKET.0)) {
                continue;
            }
            if let Some(item) = world.slot_entity(target, &slot) {
                world.raise(item, &mut event);
            }
        }
    }
    (!event.cancelled).then_some(event.siemens_coefficient)
}

/// Урон и сообщения удара (`DoCommonElectrocution`).
fn common_electrocution(
    world: &mut World,
    target: Entity,
    source: Option<Entity>,
    shock_damage: Option<i32>,
    siemens: f32,
) -> bool {
    if siemens <= 0.0 {
        return false;
    }
    let shock_damage = shock_damage.map(|damage| (damage as f32 * siemens) as i32);
    if shock_damage.is_some_and(|damage| damage <= 0) {
        return false;
    }
    if !can_apply_effect(world, target, STATUS_KEY) {
        return false;
    }
    if let Some(damage) = shock_damage {
        let specifier =
            DamageSpecifier::new([(DAMAGE_TYPE.to_owned(), FixedPoint2::from_f32(damage as f32))]);
        world.change_damage(target, &specifier, false, true, source);
    }
    let text = loc(
        world,
        "electrocuted-component-mob-shocked-popup-player",
        &[],
    );
    let mob = world.entity_arg(target);
    let others = match source {
        Some(source) => {
            let source_name = world.entity_arg(source);
            loc(
                world,
                "electrocuted-component-mob-shocked-by-source-popup-others",
                &[
                    ("mob", LocArg::from(mob.clone())),
                    ("source", LocArg::from(source_name.clone())),
                ],
            )
        }
        None => loc(
            world,
            "electrocuted-component-mob-shocked-popup-others",
            &[("mob", LocArg::from(mob))],
        ),
    };
    world.popup_entity_split(&text, &others, target, target, PopupType::Small);
    if let Some(source) = source
        && let Some(electrified) = world.get::<Electrified>(source).cloned()
        && electrified.play_sound_on_shock
    {
        world.play_entity(&electrified.shock_noises, target, None);
    }
    true
}

/// Ударить током (`TryDoElectrocution`).
pub fn try_do_electrocution(
    world: &mut World,
    target: Entity,
    source: Option<Entity>,
    shock_damage: i32,
    siemens: f32,
    ignore_insulation: bool,
) -> bool {
    let Some(siemens) = electrocution_attempt(world, target, source, siemens, ignore_insulation)
    else {
        return false;
    };
    if !common_electrocution(world, target, source, Some(shock_damage), siemens) {
        return false;
    }
    world.raise_and_broadcast(
        target,
        &mut Electrocuted {
            target,
            source,
            siemens_coefficient: siemens,
        },
    );
    true
}

/// Цепочка тех, кого тащат и кто тащит (`GetChainedElectrocutionTargets`).
fn chained_targets(world: &World, source: Entity) -> Vec<(Entity, i32)> {
    let mut all = Vec::new();
    let mut stack = vec![(source, 1)];
    while let Some((entity, depth)) = stack.pop() {
        if all.iter().any(|(seen, _)| *seen == entity) {
            continue;
        }
        all.push((entity, depth));
        if let Some(puller) = world
            .get::<crate::pulling::Pullable>(entity)
            .and_then(|p| p.puller)
        {
            stack.push((puller, depth + 1));
        }
        if let Some(pulling) = world
            .get::<crate::pulling::Puller>(entity)
            .and_then(|p| p.pulling)
        {
            stack.push((pulling, depth + 1));
        }
    }
    all
}

/// Ударить того, кто коснулся сущности под током (`TryDoElectrifiedAct`).
pub fn try_do_electrified_act(
    world: &mut World,
    entity: Entity,
    target: Entity,
    siemens: f32,
) -> bool {
    let Some(electrified) = world.get::<Electrified>(entity).cloned() else {
        return false;
    };
    if !electrified_powered(world, entity, &electrified) {
        return false;
    }
    if !random(world).prob(electrified.probability) {
        return false;
    }
    let time = now(world);
    if electrified.shock_delay.is_some() && electrified.next_shock > time {
        return false;
    }
    world.entity_mut(entity).insert(ActivatedElectrified {
        until: time + SPARKS_TIME,
    });
    world.set_appearance(entity, SHOW_SPARKS, true);
    let siemens = siemens * electrified.siemens_coefficient;
    let Some(siemens) = electrocution_attempt(world, target, Some(entity), siemens, false) else {
        return false;
    };
    if siemens <= 0.0 {
        return false;
    }
    let targets = chained_targets(world, target);
    // Длительность (множители времени) — вместе с оглушением.
    let damage_scalar = if !electrified.require_power || electrified.uses_apc_power {
        1.0
    } else {
        match powered_node(world, entity, &electrified) {
            Some((_, Voltage::High)) => electrified.high_voltage_damage_multiplier,
            Some((_, Voltage::Medium)) => electrified.medium_voltage_damage_multiplier,
            Some((_, Voltage::Low)) => 1.0,
            None => return false,
        }
    };
    let mut last = true;
    for (target, depth) in targets.into_iter().rev() {
        let damage = (electrified.shock_damage
            * RECURSIVE_DAMAGE_MULTIPLIER.powi(depth)
            * damage_scalar) as i32;
        last = try_do_electrocution(
            world,
            target,
            Some(entity),
            damage,
            electrified.siemens_coefficient,
            false,
        );
    }
    if last
        && let Some(delay) = electrified.shock_delay
        && let Some(mut component) = world.get_mut::<Electrified>(entity)
    {
        component.next_shock = time + delay;
    }
    last
}

/// Искры гаснут (`UpdateState`).
fn update(world: &mut World) {
    let time = now(world);
    let active: Vec<(Entity, f64)> = world
        .query::<(Entity, &ActivatedElectrified)>()
        .iter(world)
        .map(|(e, a)| (e, a.until))
        .collect();
    for (entity, until) in active {
        let powered = world
            .get::<Electrified>(entity)
            .cloned()
            .is_some_and(|e| electrified_powered(world, entity, &e));
        if until <= time || !powered {
            world.set_appearance(entity, SHOW_SPARKS, false);
            world.entity_mut(entity).remove::<ActivatedElectrified>();
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Electrified>();
    world.register_data_component::<Insulated>();
    world.register_net_serde::<Insulated>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Insulated, ElectrocutionAttempt>(|world, entity, event| {
        if let Some(insulated) = world.get::<Insulated>(entity) {
            event.siemens_coefficient *= insulated.coefficient;
        }
    });
    world.subscribe::<Electrified, MapInit>(|world, entity, _| {
        let time = now(world);
        if let Some(mut electrified) = world.get_mut::<Electrified>(entity) {
            electrified.next_shock = time;
        }
    });
    world.subscribe::<Electrified, StartCollide>(|world, entity, event| {
        if world.get::<Electrified>(entity).is_some_and(|e| e.on_bump) {
            try_do_electrified_act(world, entity, event.other_entity, 1.0);
        }
    });
    world.subscribe::<Electrified, Attacked>(|world, entity, event| {
        if !world
            .get::<Electrified>(entity)
            .is_some_and(|e| e.on_attacked)
            || world.melee_damage(event.used, event.user).0.is_empty()
        {
            return;
        }
        try_do_electrified_act(world, entity, event.user, 1.0);
    });
    world.subscribe::<Electrified, InteractHand>(|world, entity, event| {
        if world
            .get::<Electrified>(entity)
            .is_some_and(|e| e.on_hand_interact)
        {
            try_do_electrified_act(world, entity, event.user, 1.0);
        }
    });
    world.subscribe::<Electrified, InteractUsing>(|world, entity, event| {
        if !world
            .get::<Electrified>(entity)
            .is_some_and(|e| e.on_interact_using)
        {
            return;
        }
        let siemens = world
            .get::<Insulated>(event.used)
            .map_or(1.0, |i| i.coefficient);
        try_do_electrified_act(world, entity, event.user, siemens);
    });
    world.subscribe::<Electrified, ActivateInWorld>(|world, entity, event| {
        if event.handled
            || !world
                .get::<Electrified>(entity)
                .is_some_and(|e| e.on_activate_in_world)
        {
            return;
        }
        if try_do_electrified_act(world, entity, event.user, 1.0) {
            event.handled = true;
        }
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
