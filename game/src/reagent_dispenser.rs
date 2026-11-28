//! Химраздатчик: банки с реагентами лежат в его хранилище, ёмкость — в слоте
//! `beakerSlot`. Окно: количество за раз (1…120), карточки банок (название, остаток, цвет;
//! нажатие — налить в ёмкость, кнопка — вынуть банку в руку), состав ёмкости, очистить и
//! вынуть ёмкость. Налив — обычное переливание из банки в ёмкость. Образец —
//! `ReagentDispenserSystem`, `ReagentDispenserComponent`, `SharedReagentDispenser`,
//! `ReagentDispenserBoundUserInterfaceState`, `ContainerInfo` SS14 @ 2f4a2aad.
//!
//! Нет пока: этикеток банок (`Label`, вместо них — имя), открытия закрытой банки при наливе
//! (`Openable`).

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{ComponentStartup, EventWorldExt, MapInit, RegistryWorldExt};
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::{EntInsertedIntoContainer, EntRemovedFromContainer};
use astrocore_net::NetEntity;
use astrocore_ui::{BoundUiMessage, BoundUiOpened, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::fixed_point::FixedPoint2;
use crate::hands::HandsWorldExt;
use crate::item_slots::{self, ItemSlot};
use crate::solution::{ReagentId, reagents};
use crate::solution_container::{self as containers, SolutionChanged};
use crate::solution_transfer::{SolutionTransferData, transfer};
use crate::storage::{ItemStorageLocation, Storage};

/// Слот ёмкости (`SharedReagentDispenser.OutputSlotName`).
pub const OUTPUT_SLOT: &str = "beakerSlot";
/// Ключ окна (`ReagentDispenserUiKey.Key`).
pub const UI_KEY: &str = "enum.ReagentDispenserUiKey.Key";
/// Количества за раз (`ReagentDispenserDispenseAmount`).
pub const DISPENSE_AMOUNTS: [u32; 9] = [1, 5, 10, 15, 20, 30, 40, 60, 120];

fn click() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/machine_switch.ogg")
}
fn ten() -> u32 {
    10
}

/// Раздатчик (`ReagentDispenserComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReagentDispenser {
    #[serde(default)]
    pub beaker_slot: ItemSlot,
    #[serde(default = "click")]
    pub click_sound: SoundSpecifier,
    #[serde(skip, default = "ten")]
    pub dispense_amount: u32,
}

/// Ёмкость в окне (`ContainerInfo`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub display_name: String,
    pub current_volume: FixedPoint2,
    pub max_volume: FixedPoint2,
    pub reagents: Option<Vec<(ReagentId, FixedPoint2)>>,
}

/// Банка в окне (`ReagentInventoryItem`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReagentInventoryItem {
    pub storage_location: ItemStorageLocation,
    pub reagent_label: String,
    pub quantity: FixedPoint2,
    /// `#rrggbbaa`.
    pub reagent_color: String,
}

/// Состояние окна (`ReagentDispenserBoundUserInterfaceState`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReagentDispenserUiState {
    pub output_container: Option<ContainerInfo>,
    pub output_container_entity: Option<NetEntity>,
    pub inventory: Vec<ReagentInventoryItem>,
    pub selected_dispense_amount: u32,
}

/// Задать количество за раз (`ReagentDispenserSetDispenseAmountMessage`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SetDispenseAmount {
    pub amount: u32,
}

/// Налить из банки (`ReagentDispenserDispenseReagentMessage`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DispenseReagent {
    pub storage_location: ItemStorageLocation,
}

/// Вынуть банку (`ReagentDispenserEjectContainerMessage`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EjectContainer {
    pub storage_location: ItemStorageLocation,
}

/// Очистить ёмкость (`ReagentDispenserClearContainerSolutionMessage`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ClearContainerSolution;

/// Ёмкость для окна (`BuildOutputContainerInfo`).
pub fn build_container_info(world: &mut World, container: Option<Entity>) -> Option<ContainerInfo> {
    let container = container?;
    let soln = containers::fits_in_dispenser_solution(world, container)?;
    let solution = containers::solution(world, soln)?.clone();
    Some(ContainerInfo {
        display_name: world.entity_name(container),
        current_volume: solution.volume,
        max_volume: solution.max_volume,
        reagents: Some(solution.contents),
    })
}

/// Банки хранилища (`GetInventory`).
fn inventory(world: &mut World, dispenser: Entity) -> Vec<ReagentInventoryItem> {
    let Some(stored) = world
        .get::<Storage>(dispenser)
        .map(|s| s.stored_items.clone())
    else {
        return Vec::new();
    };
    let catalog = reagents(world);
    let mut list = Vec::new();
    for (item, location) in stored {
        let label = world.entity_name(item);
        let (quantity, color) = match containers::drainable_solution(world, item)
            .and_then(|soln| containers::solution(world, soln).cloned())
        {
            Some(solution) => (solution.volume, solution.color(&catalog).to_hex()),
            None => (FixedPoint2::ZERO, "#ffffffff".to_owned()),
        };
        list.push(ReagentInventoryItem {
            storage_location: location,
            reagent_label: label,
            quantity,
            reagent_color: color,
        });
    }
    list
}

/// Обновить окно (`UpdateUiState`).
pub fn update_ui_state(world: &mut World, dispenser: Entity) {
    let Some(amount) = world
        .get::<ReagentDispenser>(dispenser)
        .map(|d| d.dispense_amount)
    else {
        return;
    };
    let output = item_slots::slot_item(world, dispenser, OUTPUT_SLOT);
    let state = ReagentDispenserUiState {
        output_container: build_container_info(world, output),
        output_container_entity: output.and_then(|e| world.get::<NetEntity>(e).copied()),
        inventory: inventory(world, dispenser),
        selected_dispense_amount: amount,
    };
    world.set_ui_state(dispenser, UI_KEY, &state);
}

fn click_sound(world: &mut World, dispenser: Entity) {
    if let Some(sound) = world
        .get::<ReagentDispenser>(dispenser)
        .map(|d| d.click_sound.clone())
    {
        world.play_entity(&sound, dispenser, None);
    }
}

fn stored_at(world: &World, dispenser: Entity, location: ItemStorageLocation) -> Option<Entity> {
    world
        .get::<Storage>(dispenser)?
        .stored_items
        .iter()
        .find(|(_, at)| **at == location)
        .map(|(item, _)| *item)
}

/// Налить (`OnDispenseReagentMessage`).
pub fn dispense(world: &mut World, dispenser: Entity, location: ItemStorageLocation) {
    if world.get::<Storage>(dispenser).is_none() {
        return;
    }
    let Some(stored) = stored_at(world, dispenser, location) else {
        return;
    };
    let Some(output) = item_slots::slot_item(world, dispenser, OUTPUT_SLOT) else {
        return;
    };
    if containers::fits_in_dispenser_solution(world, output).is_none() {
        return;
    }
    let amount = world
        .get::<ReagentDispenser>(dispenser)
        .map_or(10, |d| d.dispense_amount);
    if let (Some(source), Some(target)) = (
        containers::drainable_solution(world, stored),
        containers::refillable_solution(world, output),
    ) {
        transfer(
            world,
            SolutionTransferData {
                user: dispenser,
                source_entity: stored,
                source,
                target_entity: output,
                target,
                amount: FixedPoint2::new(amount as i32),
            },
        );
    }
    update_ui_state(world, dispenser);
    click_sound(world, dispenser);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<ReagentDispenser>();
    world.register_ui_message::<SetDispenseAmount>();
    world.register_ui_message::<DispenseReagent>();
    world.register_ui_message::<EjectContainer>();
    world.register_ui_message::<ClearContainerSolution>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<ReagentDispenser, ComponentStartup>(|world, d, _| update_ui_state(world, d));
    world.subscribe::<ReagentDispenser, SolutionChanged>(|world, d, _| update_ui_state(world, d));
    world.subscribe::<ReagentDispenser, EntInsertedIntoContainer>(|world, d, _| {
        update_ui_state(world, d);
    });
    world.subscribe::<ReagentDispenser, EntRemovedFromContainer>(|world, d, _| {
        update_ui_state(world, d);
    });
    world.subscribe::<ReagentDispenser, BoundUiOpened>(|world, d, _| update_ui_state(world, d));
    world.subscribe::<ReagentDispenser, BoundUiMessage<SetDispenseAmount>>(|world, d, event| {
        // В SS14 значение — перечисление; чужое число не принимается.
        if !DISPENSE_AMOUNTS.contains(&event.message.amount) {
            return;
        }
        if let Some(mut dispenser) = world.get_mut::<ReagentDispenser>(d) {
            dispenser.dispense_amount = event.message.amount;
        }
        update_ui_state(world, d);
        click_sound(world, d);
    });
    world.subscribe::<ReagentDispenser, BoundUiMessage<DispenseReagent>>(|world, d, event| {
        dispense(world, d, event.message.storage_location);
    });
    world.subscribe::<ReagentDispenser, BoundUiMessage<EjectContainer>>(|world, d, event| {
        if let Some(stored) = stored_at(world, d, event.message.storage_location) {
            world.try_pickup_any_hand(event.actor, stored, true);
        }
    });
    world.subscribe::<ReagentDispenser, BoundUiMessage<ClearContainerSolution>>(|world, d, _| {
        let Some(output) = item_slots::slot_item(world, d, OUTPUT_SLOT) else {
            return;
        };
        let Some(soln) = containers::fits_in_dispenser_solution(world, output) else {
            return;
        };
        containers::remove_all_solution(world, soln);
        update_ui_state(world, d);
        click_sound(world, d);
    });
    world.subscribe::<ReagentDispenser, MapInit>(|world, d, _| {
        if let Some(slot) = world
            .get::<ReagentDispenser>(d)
            .map(|r| r.beaker_slot.clone())
        {
            item_slots::add_item_slot(world, d, OUTPUT_SLOT, slot);
        }
    });
}

#[cfg(test)]
mod tests;
