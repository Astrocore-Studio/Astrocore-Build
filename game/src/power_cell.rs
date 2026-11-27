//! Батарейки в устройствах: слот батарейки (только `PowerCell`), заряд устройства берётся из
//! батарейки в слоте, потребление устройства (`PowerCellDraw`: постоянное и за действие),
//! включаемое потребление (`ToggleCellDraw`: включить можно только с зарядом, разряд или
//! пустой слот выключают). События батарейки (скорость заряда, изменение, «пуста») доходят
//! до устройства, полное исцеление — до батарейки. Образец — `PowerCellSystem` (с `API`,
//! `Draw`, `Relay`), `PowerCellComponent`, `PowerCellSlotComponent`,
//! `PowerCellDrawComponent`, `ToggleCellDrawSystem`, `PowerCellEvents` SS14 @ 2f4a2aad.
//!
//! Нет пока: вида заряда устройства (`BatteryVisuals`), зарядников.

use astrocore_ecs::{ComponentStartup, EventWorldExt, MapInit, RegistryWorldExt};
use astrocore_map::{
    ContainerIsInsertingAttempt, ContainerWorldExt, EntInsertedIntoContainer,
    EntRemovedFromContainer,
};
use astrocore_net::NetRegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::battery::{self, BatteryState, BatteryStateChanged, ChargeChanged, RefreshChargeRate};
use crate::damage::Rejuvenate;
use crate::examine::{Examined, loc};
use crate::item_toggle::{self, ItemToggleActivateAttempt, ItemToggled};
use crate::popups::{PopupType, PopupWorldExt};
use crate::power::Battery;

/// Батарейка (`PowerCellComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PowerCell {}

fn yes() -> bool {
    true
}

/// Слот батарейки устройства (`PowerCellSlotComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerCellSlot {
    pub cell_slot_id: String,
    #[serde(default = "yes")]
    pub fits_in_charger: bool,
}

fn one() -> f32 {
    1.0
}

/// Потребление устройства (`PowerCellDrawComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerCellDraw {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Постоянный расход, Дж/с.
    #[serde(default = "one")]
    pub draw_rate: f32,
    /// Расход за одно действие, Дж.
    #[serde(default)]
    pub use_charge: f32,
}

/// Потребление идёт, пока предмет включён (`ToggleCellDrawComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToggleCellDraw {}

/// Батарейка в слоте разрядилась или её вынули (`PowerCellSlotEmptyEvent`). На устройстве.
#[derive(Debug, Clone, Copy, Default)]
pub struct PowerCellSlotEmpty;

/// Батарейку вставили или вынули (`PowerCellChangedEvent`). На устройстве.
#[derive(Debug, Clone, Copy)]
pub struct PowerCellChanged {
    pub ejected: bool,
}

/// Батарейка в слоте устройства (`TryGetBatteryFromSlot`).
pub fn battery_in_slot(world: &World, device: Entity) -> Option<Entity> {
    let slot = world.get::<PowerCellSlot>(device)?;
    let cell = crate::item_slots::slot_item(world, device, &slot.cell_slot_id)?;
    world.get::<Battery>(cell).is_some().then_some(cell)
}

/// Устройство, в слоте которого стоит батарейка.
pub fn device_of_cell(world: &World, cell: Entity) -> Option<Entity> {
    let holder = world.containing_container(cell)?;
    let slot = world.get::<PowerCellSlot>(holder.owner)?;
    (*holder.id == *slot.cell_slot_id).then_some(holder.owner)
}

/// Хватает ли заряда (`HasCharge`); с `user` — сообщение, почему нет.
pub fn has_charge(world: &mut World, device: Entity, charge: f32, user: Option<Entity>) -> bool {
    let Some(cell) = battery_in_slot(world, device) else {
        popup(world, device, user, "power-cell-no-battery");
        return false;
    };
    if battery::charge(world, cell) < charge {
        popup(world, device, user, "power-cell-insufficient");
        return false;
    }
    true
}

/// Потратить заряд батарейки (`TryUseCharge`).
pub fn try_use_charge(
    world: &mut World,
    device: Entity,
    charge: f32,
    user: Option<Entity>,
) -> bool {
    let Some(cell) = battery_in_slot(world, device) else {
        popup(world, device, user, "power-cell-no-battery");
        return false;
    };
    if !battery::try_use_charge(world, cell, charge) {
        popup(world, device, user, "power-cell-insufficient");
        return false;
    }
    true
}

fn popup(world: &mut World, device: Entity, user: Option<Entity>, key: &str) {
    if let Some(user) = user {
        let text = loc(world, key, &[]);
        world.popup_entity_for(&text, device, user, PopupType::Small);
    }
}

/// Хватит ли на одно действие (`HasActivatableCharge`); без потребления или слота — да.
pub fn has_activatable_charge(world: &mut World, device: Entity, user: Option<Entity>) -> bool {
    let (Some(draw), Some(_)) = (
        world.get::<PowerCellDraw>(device).cloned(),
        world.get::<PowerCellSlot>(device),
    ) else {
        return true;
    };
    has_charge(world, device, draw.use_charge, user)
}

/// Потратить заряд одного действия (`TryUseActivatableCharge`).
pub fn try_use_activatable_charge(world: &mut World, device: Entity, user: Option<Entity>) -> bool {
    let (Some(draw), Some(_)) = (
        world.get::<PowerCellDraw>(device).cloned(),
        world.get::<PowerCellSlot>(device),
    ) else {
        return true;
    };
    try_use_charge(world, device, draw.use_charge, user)
}

/// Хватит ли на секунду постоянного расхода (`HasDrawCharge`).
pub fn has_draw_charge(world: &mut World, device: Entity, user: Option<Entity>) -> bool {
    let (Some(draw), Some(_)) = (
        world.get::<PowerCellDraw>(device).cloned(),
        world.get::<PowerCellSlot>(device),
    ) else {
        return true;
    };
    has_charge(world, device, draw.draw_rate, user)
}

/// Включить или выключить постоянный расход (`SetDrawEnabled`).
pub fn set_draw_enabled(world: &mut World, device: Entity, enabled: bool) {
    if let Some(mut draw) = world.get_mut::<PowerCellDraw>(device)
        && draw.enabled != enabled
    {
        draw.enabled = enabled;
    }
    if let Some(cell) = battery_in_slot(world, device) {
        battery::refresh_charge_rate(world, cell);
    }
}

fn charge_percent_text(world: &World, cell: Entity) -> String {
    let percent = format!("{:.0}", battery::charge_level(world, cell) * 100.0);
    loc(
        world,
        "power-cell-component-examine-details",
        &[("currentCharge", percent.into())],
    )
}

/// Событие батарейки — устройству (`RelayToCellSlot`).
fn relay_to_device<E: 'static>(world: &mut World, cell: Entity, event: &mut E) {
    if let Some(device) = device_of_cell(world, cell) {
        world.raise(device, event);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<PowerCell>();
    world.register_data_component::<PowerCellSlot>();
    world.register_data_component::<PowerCellDraw>();
    world.register_data_component::<ToggleCellDraw>();
    world.register_net_serde::<PowerCellSlot>();
    world.register_net_serde::<PowerCellDraw>();
    if !side.is_server() {
        return;
    }
    // В слот батарейки — только батарейки (`OnCellSlotInsertAttempt`).
    world.subscribe::<PowerCellSlot, ContainerIsInsertingAttempt>(|world, device, event| {
        let Some(slot) = world.get::<PowerCellSlot>(device) else {
            return;
        };
        if *event.container == *slot.cell_slot_id && world.get::<PowerCell>(event.entity).is_none()
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<PowerCellSlot, EntInsertedIntoContainer>(|world, device, event| {
        let Some(slot) = world.get::<PowerCellSlot>(device) else {
            return;
        };
        if *event.container != *slot.cell_slot_id {
            return;
        }
        world.raise(device, &mut PowerCellChanged { ejected: false });
        battery::refresh_charge_rate(world, event.entity);
    });
    world.subscribe::<PowerCellSlot, EntRemovedFromContainer>(|world, device, event| {
        let Some(slot) = world.get::<PowerCellSlot>(device) else {
            return;
        };
        if *event.container != *slot.cell_slot_id {
            return;
        }
        world.raise(device, &mut PowerCellChanged { ejected: true });
        world.raise(device, &mut PowerCellSlotEmpty);
        battery::refresh_charge_rate(world, event.entity);
    });
    world.subscribe::<PowerCellSlot, BatteryStateChanged>(|world, device, event| {
        if event.new_state == BatteryState::Empty {
            world.raise(device, &mut PowerCellSlotEmpty);
        }
    });
    world.subscribe::<PowerCellSlot, Examined>(|world, device, event| {
        let text = match battery_in_slot(world, device) {
            Some(cell) => charge_percent_text(world, cell),
            None => loc(
                world,
                "power-cell-component-examine-details-no-battery",
                &[],
            ),
        };
        event.push_markup(text, 0);
    });
    world.subscribe::<PowerCellSlot, Rejuvenate>(|world, device, event| {
        if let Some(cell) = battery_in_slot(world, device) {
            world.raise(cell, event);
        }
    });
    world.subscribe::<PowerCell, Examined>(|world, cell, event| {
        if world.get::<Battery>(cell).is_some() {
            let text = charge_percent_text(world, cell);
            event.push_markup(text, 0);
        }
    });
    world.subscribe::<PowerCell, RefreshChargeRate>(relay_to_device);
    world.subscribe::<PowerCell, ChargeChanged>(relay_to_device);
    world.subscribe::<PowerCell, BatteryStateChanged>(relay_to_device);
    // Постоянный расход устройства из батарейки (`OnDrawRefreshChargeRate`).
    world.subscribe::<PowerCellDraw, RefreshChargeRate>(|world, device, event| {
        let Some(draw) = world.get::<PowerCellDraw>(device) else {
            return;
        };
        if !draw.enabled {
            return;
        }
        event.new_charge_rate -= draw.draw_rate;
        if let Some(cell) = battery_in_slot(world, device) {
            battery::try_set_charge_cooldown(world, cell);
        }
    });
    world.subscribe::<PowerCellDraw, ComponentStartup>(|world, device, _| {
        if world
            .get::<PowerCellDraw>(device)
            .is_some_and(|d| d.enabled)
            && let Some(cell) = battery_in_slot(world, device)
        {
            battery::refresh_charge_rate(world, cell);
        }
    });
    // Включаемый расход (`ToggleCellDrawSystem`).
    world.subscribe::<ToggleCellDraw, MapInit>(|world, device, _| {
        let active = item_toggle::is_activated(world, device);
        set_draw_enabled(world, device, active);
    });
    world.subscribe::<ToggleCellDraw, ItemToggleActivateAttempt>(|world, device, event| {
        if !has_draw_charge(world, device, event.user)
            || !has_activatable_charge(world, device, event.user)
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<ToggleCellDraw, ItemToggled>(|world, device, event| {
        set_draw_enabled(world, device, event.activated);
    });
    world.subscribe::<ToggleCellDraw, PowerCellSlotEmpty>(|world, device, _| {
        item_toggle::try_deactivate(world, device, None, true);
    });
}

#[cfg(test)]
mod tests;
