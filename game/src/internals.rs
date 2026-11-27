//! Дыхание из баллона (internals): надетая маска подключается к телу, тело подключает
//! баллон со спины, из слота скафандра, рук или карманов и вдыхает из него. Другому
//! человеку баллон подключают с задержкой. Образец — `InternalsComponent`,
//! `BreathToolComponent` (`BreathMask`), `SharedInternalsSystem`, `InternalsSystem` и
//! `LungSystem.OnGotEquipped` SS14 @ 2f4a2aad.
//!
//! Нет пока: значка тревоги (`InternalsAlert`), опускаемой маски (`MaskComponent`),
//! автоподключения со стартовым снаряжением (фаза 6) и реактивных ранцев.

use std::collections::BTreeSet;

use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterWorldExt};
use crate::gas_tank::{GasTank, can_connect_to_internals, connect_to_internals};
use crate::inventory::{Equipped, InventoryWorldExt, SlotFlags, Unequipped};
use crate::popups::PopupWorldExt;
use crate::respiration::InhaleLocation;
use crate::verbs::{GetVerbs, Verb, VerbKind};

fn three() -> f32 {
    3.0
}

/// Может дышать из баллона (`InternalsComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Internals {
    #[serde(skip)]
    pub gas_tank_entity: Option<Entity>,
    #[serde(skip)]
    pub breath_tools: BTreeSet<Entity>,
    /// Задержка подключения другому, с.
    #[serde(default = "three")]
    pub delay: f32,
}

fn mask_or_head() -> SlotFlags {
    SlotFlags(SlotFlags::MASK.0 | SlotFlags::HEAD.0)
}

/// Маска для дыхания (`BreathToolComponent`, в данных `BreathMask`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreathTool {
    #[serde(default = "mask_or_head")]
    pub allowed_slots: SlotFlags,
    #[serde(skip)]
    pub connected_internals_entity: Option<Entity>,
}

impl BreathTool {
    /// Подключена к телу (`IsFunctional`).
    pub fn is_functional(&self) -> bool {
        self.connected_internals_entity.is_some()
    }
}

/// Режим переключения (`ToggleMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleMode {
    Toggle,
    On,
    Off,
}

/// Итог подключения другому (`InternalsDoAfterEvent`).
#[derive(Debug, Clone, PartialEq)]
pub struct InternalsDoAfter {
    pub mode: ToggleMode,
}

/// Работает ли дыхание из баллона (`AreInternalsWorking`): маска подключена и баллон есть.
pub fn are_internals_working(world: &World, entity: Entity) -> bool {
    let Some(internals) = world.get::<Internals>(entity) else {
        return false;
    };
    internals
        .breath_tools
        .iter()
        .next()
        .and_then(|tool| world.get::<BreathTool>(*tool))
        .is_some_and(BreathTool::is_functional)
        && internals
            .gas_tank_entity
            .is_some_and(|tank| world.get::<GasTank>(tank).is_some())
}

/// Подключить маску (`ConnectBreathTool`).
pub fn connect_breath_tool(world: &mut World, entity: Entity, tool: Entity) {
    let Some(mut internals) = world.get_mut::<Internals>(entity) else {
        return;
    };
    if !internals.breath_tools.insert(tool) {
        return;
    }
    if let Some(mut breath) = world.get_mut::<BreathTool>(tool) {
        breath.connected_internals_entity = Some(entity);
    }
}

/// Отключить маску (`DisconnectBreathTool`); без масок отключается и баллон.
pub fn disconnect_breath_tool(world: &mut World, entity: Entity, tool: Entity, forced: bool) {
    let Some(mut internals) = world.get_mut::<Internals>(entity) else {
        return;
    };
    if !internals.breath_tools.remove(&tool) {
        return;
    }
    let empty = internals.breath_tools.is_empty();
    if let Some(mut breath) = world.get_mut::<BreathTool>(tool) {
        breath.connected_internals_entity = None;
    }
    if empty {
        disconnect_tank(world, entity, forced);
    }
}

/// Отключить баллон (`DisconnectTank`).
pub fn disconnect_tank(world: &mut World, entity: Entity, forced: bool) {
    let tank = world
        .get_mut::<Internals>(entity)
        .and_then(|mut i| i.gas_tank_entity.take());
    if let Some(tank) = tank
        && world.get::<GasTank>(tank).is_some()
    {
        crate::gas_tank::disconnect_from_internals(world, tank, None, forced);
    }
}

/// Подключить баллон (`TryConnectTank`): нужна маска.
pub fn try_connect_tank(world: &mut World, entity: Entity, tank: Entity) -> bool {
    let Some(internals) = world.get::<Internals>(entity) else {
        return false;
    };
    if internals.breath_tools.is_empty() {
        return false;
    }
    if let Some(previous) = internals.gas_tank_entity
        && world.get::<GasTank>(previous).is_some()
    {
        crate::gas_tank::disconnect_from_internals(world, previous, None, false);
    }
    if let Some(mut internals) = world.get_mut::<Internals>(entity) {
        internals.gas_tank_entity = Some(tank);
    }
    true
}

/// Лучший баллон (`FindBestGasTank`): спина, слот скафандра, руки и карманы.
pub fn find_best_gas_tank(world: &mut World, user: Entity) -> Option<Entity> {
    let usable = |world: &World, item: Entity| {
        world.get::<GasTank>(item).is_some() && can_connect_to_internals(world, item)
    };
    for slot in ["back", "suitstorage"] {
        if let Some(item) = world.slot_entity(user, slot)
            && usable(world, item)
        {
            return Some(item);
        }
    }
    crate::hands::HandsWorldExt::held_items(world, user)
        .into_iter()
        .chain(crate::temperature::worn_items(world, user))
        .chain(pockets(world, user))
        .find(|item| usable(world, *item))
}

fn pockets(world: &World, user: Entity) -> Vec<Entity> {
    let Some(inventory) = world.get::<crate::inventory::Inventory>(user) else {
        return Vec::new();
    };
    inventory
        .slots
        .iter()
        .filter(|slot| slot.slot_flags.intersects(SlotFlags::POCKET))
        .filter_map(|slot| world.slot_entity(user, &slot.name))
        .collect()
}

fn popup(world: &mut World, key: &str, target: Entity, user: Entity) {
    let name = astrocore_loc::EntityNameWorldExt::entity_arg(world, target);
    let text = crate::examine::loc(
        world,
        key,
        &[("ent", astrocore_loc::LocArg::from(name.clone()))],
    );
    world.popup_entity_for(&text, target, user, crate::popups::PopupType::Small);
}

/// Переключить дыхание из баллона (`ToggleInternals`).
pub fn toggle_internals(
    world: &mut World,
    target: Entity,
    user: Entity,
    force: bool,
    mode: ToggleMode,
) -> bool {
    let Some(internals) = world.get::<Internals>(target).cloned() else {
        return false;
    };
    let own = user == target;
    if internals.breath_tools.is_empty() {
        let key = if own {
            "internals-self-no-breath-tool"
        } else {
            "internals-other-no-breath-tool"
        };
        popup(world, key, target, user);
        return false;
    }
    let Some(tank) = find_best_gas_tank(world, target) else {
        let key = if own {
            "internals-self-no-tank"
        } else {
            "internals-other-no-tank"
        };
        popup(world, key, target, user);
        return false;
    };
    // Другому — с задержкой.
    if !force && !own {
        let mut args = DoAfterArgs::new(
            user,
            f64::from(internals.delay),
            Some(target),
            None,
            Some(target),
        );
        args.break_on_damage = true;
        args.break_on_move = true;
        args.movement_threshold = 0.1;
        return !matches!(
            world.try_start_do_after(args, InternalsDoAfter { mode }),
            crate::do_after::DoAfterStart::Refused
        );
    }
    if let Some(connected) = internals
        .gas_tank_entity
        .filter(|t| world.get::<GasTank>(*t).is_some())
    {
        if mode == ToggleMode::On {
            return false;
        }
        return crate::gas_tank::disconnect_from_internals(world, connected, Some(user), false);
    }
    if mode == ToggleMode::Off {
        return false;
    }
    connect_to_internals(world, tank, Some(user))
}

/// Глагол «Включить/выключить баллон» (`OnGetInteractionVerbs`).
fn internals_verbs(world: &mut World, target: Entity, event: &mut GetVerbs) {
    if event.kind != VerbKind::Interaction || !event.can_access || !event.can_interact {
        return;
    }
    let Some(internals) = world.get::<Internals>(target) else {
        return;
    };
    let working = are_internals_working(world, target);
    if !working && internals.breath_tools.is_empty() {
        return;
    }
    let user = event.user;
    let (key, mode) = if working {
        ("action-name-internals-toggle-off", ToggleMode::Off)
    } else {
        ("action-name-internals-toggle-on", ToggleMode::On)
    };
    let text = crate::examine::loc(world, key, &[]);
    let mut verb = Verb::new(VerbKind::Interaction, text, move |world| {
        toggle_internals(world, target, user, false, mode);
    });
    verb.icon = Some("/Textures/Interface/VerbIcons/dot.svg.192dpi.png".into());
    event.verbs.push(verb);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Internals>();
    world.register_data_component_as::<BreathTool>("BreathMask");
    if !side.is_server() {
        return;
    }
    world.subscribe::<Internals, GetVerbs>(internals_verbs);
    world.subscribe::<Internals, DoAfterDone<InternalsDoAfter>>(|world, target, event| {
        if event.cancelled || event.handled {
            return;
        }
        toggle_internals(world, target, event.user, true, event.event.mode);
        event.handled = true;
    });
    // Вдох из баллона (`OnInhaleLocation`).
    world.subscribe::<Internals, InhaleLocation>(|world, entity, event| {
        if are_internals_working(world, entity) {
            event.tank = world
                .get::<Internals>(entity)
                .and_then(|i| i.gas_tank_entity);
        }
    });
    // Маска надета или снята (`OnGotEquipped`, `OnGotUnequipped`).
    world.subscribe::<BreathTool, Equipped>(|world, tool, event| {
        if event.item != tool {
            return;
        }
        let Some(allowed) = world.get::<BreathTool>(tool).map(|b| b.allowed_slots) else {
            return;
        };
        if !event.slot_flags.intersects(allowed) {
            return;
        }
        if world.get::<Internals>(event.wearer).is_some() {
            connect_breath_tool(world, event.wearer, tool);
        }
    });
    world.subscribe::<BreathTool, Unequipped>(|world, tool, event| {
        if event.item != tool {
            return;
        }
        let connected = world
            .get::<BreathTool>(tool)
            .and_then(|b| b.connected_internals_entity);
        if let Some(entity) = connected {
            disconnect_breath_tool(world, entity, tool, false);
        }
    });
}

#[cfg(test)]
pub(crate) mod tests;
