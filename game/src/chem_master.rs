//! ChemMaster: ёмкость в слоте `beakerSlot`, своя «буферная» ёмкость без предела и выход —
//! бутылка или баночка для таблеток в слоте `outputSlot`. Реагенты переносятся между
//! ёмкостью и буфером или выливаются (режимы «перенос»/«слив»), по 1…120 или всё; из буфера
//! или ёмкости (источник) делаются таблетки (тип 0…19, доза до предела, сколько влезет в
//! баночку) и разливается в бутылку. Образец — `ChemMasterSystem`, `ChemMasterComponent`,
//! `SharedChemMaster`, `ChemMasterBoundUserInterfaceState`, сообщения окна SS14 @ 2f4a2aad.
//!
//! Этикетка (до 50 знаков) наклеивается на выход и на каждую таблетку; пустая — снимает.
//! Как в SS14: буфер меняется напрямую, без проведения изменений раствора (его вместимость
//! 0).

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{ComponentStartup, EventWorldExt, RegistryWorldExt};
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::{
    ContainerWorldExt, EntInsertedIntoContainer, EntRemovedFromContainer, SpawnWorldExt,
    TransformWorldExt,
};
use astrocore_ui::{BoundUiMessage, BoundUiOpened, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::ingestion::Pill;
use crate::item_slots;
use crate::popups::{PopupType, PopupWorldExt};
use crate::reagent_dispenser::{ContainerInfo, build_container_info};
use crate::solution::{ReagentId, Solution};
use crate::solution_container::{self as containers, SolutionChanged, SolutionComponent};
use crate::storage::{
    STORAGE_CONTAINER, Storage, StorageWorldExt, cumulative_item_areas, shape_area,
};

/// Типов таблеток (`PillTypes`).
pub const PILL_TYPES: u32 = 20;
pub const BUFFER_SOLUTION: &str = "buffer";
pub const INPUT_SLOT: &str = "beakerSlot";
pub const OUTPUT_SLOT: &str = "outputSlot";
pub const PILL_SOLUTION: &str = "food";
pub const BOTTLE_SOLUTION: &str = "drink";
pub const LABEL_MAX_LENGTH: usize = 50;
pub const UI_KEY: &str = "enum.ChemMasterUiKey.Key";
const PILL_PROTOTYPE: &str = "Pill";

/// Режим кнопок реагентов (`ChemMasterMode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChemMasterMode {
    #[default]
    Transfer,
    Discard,
}

/// Порядок буфера в окне (`ChemMasterSortingType`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChemMasterSortingType {
    #[default]
    None,
    Alphabetical,
    Quantity,
    Latest,
}

impl ChemMasterSortingType {
    fn next(self) -> Self {
        match self {
            Self::None => Self::Alphabetical,
            Self::Alphabetical => Self::Quantity,
            Self::Quantity => Self::Latest,
            Self::Latest => Self::None,
        }
    }
}

/// Откуда делать таблетки и бутылки (`ChemMasterDrawSource`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChemMasterDrawSource {
    #[default]
    Internal,
    External,
}

/// Количество кнопки реагента (`ChemMasterReagentAmount`): `None` — всё.
pub const REAGENT_AMOUNTS: [u32; 9] = [1, 5, 10, 15, 20, 30, 40, 60, 120];

fn click() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/machine_switch.ogg")
}

/// ChemMaster (`ChemMasterComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChemMaster {
    #[serde(default)]
    pub pill_type: u32,
    #[serde(default)]
    pub mode: ChemMasterMode,
    #[serde(default)]
    pub sorting_type: ChemMasterSortingType,
    pub pill_dosage_limit: u32,
    #[serde(default = "click")]
    pub click_sound: SoundSpecifier,
    #[serde(default)]
    pub draw_source: ChemMasterDrawSource,
}

/// Состояние окна (`ChemMasterBoundUserInterfaceState`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChemMasterUiState {
    pub input_container_info: Option<ContainerInfo>,
    pub output_container_info: Option<ContainerInfo>,
    pub buffer_reagents: Vec<(ReagentId, FixedPoint2)>,
    pub mode: ChemMasterMode,
    pub sorting_type: ChemMasterSortingType,
    pub buffer_current_volume: Option<FixedPoint2>,
    pub selected_pill_type: u32,
    pub pill_dosage_limit: u32,
    pub update_label: bool,
    pub draw_source: ChemMasterDrawSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChemMasterSetMode(pub ChemMasterMode);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChemMasterSortingTypeCycle;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChemMasterSetPillType(pub u32);

/// Кнопка количества у реагента (`ChemMasterReagentAmountButtonMessage`); `amount: None` —
/// «всё».
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChemMasterReagentAmountButton {
    pub reagent_id: ReagentId,
    pub amount: Option<u32>,
    pub from_buffer: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChemMasterCreatePills {
    pub dosage: u32,
    pub number: u32,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChemMasterOutputToBottle {
    pub dosage: u32,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChemMasterOutputDrawSource(pub ChemMasterDrawSource);

fn buffer(world: &mut World, master: Entity) -> Option<Entity> {
    containers::try_get_solution(world, master, BUFFER_SOLUTION)
}

fn buffer_solution(world: &mut World, master: Entity) -> Option<Solution> {
    let soln = buffer(world, master)?;
    containers::solution(world, soln).cloned()
}

/// Выход для окна (`BuildOutputContainerInfo`): бутылка — раствор, баночка — таблетки.
fn output_container_info(world: &mut World, container: Option<Entity>) -> Option<ContainerInfo> {
    let container = container?;
    let name = world.entity_name(container);
    if let Some(soln) = containers::try_get_solution(world, container, BOTTLE_SOLUTION)
        && let Some(solution) = containers::solution(world, soln).cloned()
    {
        return Some(ContainerInfo {
            display_name: name,
            current_volume: solution.volume,
            max_volume: solution.max_volume,
            reagents: Some(solution.contents),
            entities: None,
        });
    }
    let grid = world.get::<Storage>(container)?.grid.clone();
    let pills: Vec<Entity> = world
        .container(container, STORAGE_CONTAINER)
        .map(|c| c.entities().to_vec())
        .unwrap_or_default();
    let entities = pills
        .into_iter()
        .map(|pill| {
            let quantity = containers::try_get_solution(world, pill, PILL_SOLUTION)
                .and_then(|s| containers::solution(world, s).map(|s| s.volume))
                .unwrap_or_default();
            (world.entity_name(pill), quantity)
        })
        .collect();
    Some(ContainerInfo {
        display_name: name,
        current_volume: FixedPoint2::new(cumulative_item_areas(world, container)),
        max_volume: FixedPoint2::new(shape_area(&grid)),
        reagents: None,
        entities: Some(entities),
    })
}

/// Обновить окно (`UpdateUiState`).
pub fn update_ui_state(world: &mut World, master: Entity, update_label: bool) {
    let Some(component) = world.get::<ChemMaster>(master).cloned() else {
        return;
    };
    let Some(buffer) = buffer_solution(world, master) else {
        return;
    };
    let input = item_slots::slot_item(world, master, INPUT_SLOT);
    let output = item_slots::slot_item(world, master, OUTPUT_SLOT);
    let state = ChemMasterUiState {
        input_container_info: build_container_info(world, input),
        output_container_info: output_container_info(world, output),
        buffer_reagents: buffer.contents.clone(),
        mode: component.mode,
        sorting_type: component.sorting_type,
        buffer_current_volume: Some(buffer.volume),
        selected_pill_type: component.pill_type,
        pill_dosage_limit: component.pill_dosage_limit,
        update_label,
        draw_source: component.draw_source,
    };
    world.set_ui_state(master, UI_KEY, &state);
}

fn click_sound(world: &mut World, master: Entity) {
    if let Some(sound) = world
        .get::<ChemMaster>(master)
        .map(|m| m.click_sound.clone())
    {
        world.play_entity(&sound, master, None);
    }
}

fn amount_of(amount: Option<u32>) -> FixedPoint2 {
    amount.map_or(FixedPoint2::MAX, |a| FixedPoint2::new(a as i32))
}

/// Между ёмкостью и буфером (`TransferReagents`).
pub fn transfer_reagents(
    world: &mut World,
    master: Entity,
    id: &ReagentId,
    amount: FixedPoint2,
    from_buffer: bool,
) {
    let Some(container) = item_slots::slot_item(world, master, INPUT_SLOT) else {
        return;
    };
    let Some(container_soln) = containers::fits_in_dispenser_solution(world, container) else {
        return;
    };
    let Some(buffer) = buffer(world, master) else {
        return;
    };
    let Some(container_solution) = containers::solution(world, container_soln).cloned() else {
        return;
    };
    if from_buffer {
        let amount = amount.min(container_solution.available_volume());
        let removed = world
            .get_mut::<SolutionComponent>(buffer)
            .map_or(FixedPoint2::ZERO, |mut b| {
                b.solution.remove_reagent_ordered(id, amount, false, true)
            });
        containers::try_add_reagent(world, container_soln, id.clone(), removed, None);
    } else {
        let have = container_solution
            .contents
            .iter()
            .filter(|(other, _)| other == id)
            .fold(FixedPoint2::ZERO, |sum, (_, q)| sum + *q);
        let amount = amount.min(have);
        containers::remove_reagent(world, container_soln, id, amount);
        if let Some(mut b) = world.get_mut::<SolutionComponent>(buffer) {
            b.solution.add_reagent(id.clone(), amount);
        }
    }
    update_ui_state(world, master, true);
}

/// Вылить (`DiscardReagents`).
pub fn discard_reagents(
    world: &mut World,
    master: Entity,
    id: &ReagentId,
    amount: FixedPoint2,
    from_buffer: bool,
) {
    if from_buffer {
        let Some(buffer) = buffer(world, master) else {
            return;
        };
        if let Some(mut b) = world.get_mut::<SolutionComponent>(buffer) {
            b.solution.remove_reagent_ordered(id, amount, false, true);
        }
    } else {
        let Some(soln) = item_slots::slot_item(world, master, INPUT_SLOT)
            .and_then(|c| containers::fits_in_dispenser_solution(world, c))
        else {
            return;
        };
        containers::remove_reagent(world, soln, id, amount);
    }
    update_ui_state(world, master, from_buffer);
}

fn popup_cursor(world: &mut World, user: Option<Entity>, key: &str) {
    if let Some(user) = user {
        let text = loc(world, key, &[]);
        world.popup_cursor(&text, user, PopupType::Small);
    }
}

/// Взять из источника (`WithdrawFromSource`).
fn withdraw_from_source(
    world: &mut World,
    master: Entity,
    needed: FixedPoint2,
    user: Option<Entity>,
) -> Option<Solution> {
    let source = world.get::<ChemMaster>(master)?.draw_source;
    match source {
        ChemMasterDrawSource::Internal => {
            let soln = buffer(world, master)?;
            let volume = containers::solution(world, soln)?.volume;
            if volume == FixedPoint2::ZERO {
                popup_cursor(world, user, "chem-master-window-buffer-empty-text");
                return None;
            }
            if needed > volume {
                popup_cursor(world, user, "chem-master-window-buffer-low-text");
                return None;
            }
            // Буфер — без проведения изменений.
            let mut component = world.get_mut::<SolutionComponent>(soln)?;
            Some(component.solution.split(needed))
        }
        ChemMasterDrawSource::External => {
            let Some(container) = item_slots::slot_item(world, master, INPUT_SLOT) else {
                popup_cursor(world, user, "chem-master-window-no-beaker-text");
                return None;
            };
            let soln = containers::fits_in_dispenser_solution(world, container)?;
            let volume = containers::solution(world, soln)?.volume;
            if volume == FixedPoint2::ZERO {
                popup_cursor(world, user, "chem-master-window-beaker-empty-text");
                return None;
            }
            if needed > volume {
                popup_cursor(world, user, "chem-master-window-beaker-low-text");
                return None;
            }
            Some(containers::split_solution(world, soln, needed))
        }
    }
}

fn pretty_solution(solution: &Solution) -> String {
    solution
        .contents
        .iter()
        .map(|(id, quantity)| format!("{}: {quantity}", id.prototype))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Сделать таблетки (`OnCreatePillsMessage`).
pub fn create_pills(
    world: &mut World,
    master: Entity,
    user: Entity,
    message: &ChemMasterCreatePills,
) {
    let Some(container) = item_slots::slot_item(world, master, OUTPUT_SLOT) else {
        return;
    };
    let Some(grid) = world.get::<Storage>(container).map(|s| s.grid.clone()) else {
        return;
    };
    let free = shape_area(&grid) - cumulative_item_areas(world, container);
    if message.number == 0 || free < message.number as i32 {
        return;
    }
    let Some(limit) = world.get::<ChemMaster>(master).map(|m| m.pill_dosage_limit) else {
        return;
    };
    if message.dosage == 0 || message.dosage > limit {
        return;
    }
    if message.label.chars().count() > LABEL_MAX_LENGTH {
        return;
    }
    let needed = FixedPoint2::new((message.dosage * message.number) as i32);
    let Some(mut withdrawal) = withdraw_from_source(world, master, needed, Some(user)) else {
        return;
    };
    crate::labels::label(world, container, Some(&message.label));
    let pill_type = world.get::<ChemMaster>(master).map_or(0, |m| m.pill_type);
    let Some(at) = world.coordinates(container) else {
        return;
    };
    for _ in 0..message.number {
        let Ok(item) = world.spawn_attached(PILL_PROTOTYPE, at) else {
            continue;
        };
        world.storage_insert(container, item, Some(user), true);
        crate::labels::label(world, item, Some(&message.label));
        let Some((soln, _)) = containers::ensure_solution(world, item, PILL_SOLUTION) else {
            continue;
        };
        let dosage = FixedPoint2::new(message.dosage as i32);
        if let Some(mut component) = world.get_mut::<SolutionComponent>(soln) {
            component.solution.max_volume = dosage;
        }
        let part = withdrawal.split(dosage);
        containers::try_add_solution(world, soln, &part);
        match world.get_mut::<Pill>(item) {
            Some(mut pill) => pill.pill_type = pill_type,
            None => {
                world.entity_mut(item).insert(Pill { pill_type });
            }
        }
        let contents = containers::solution(world, soln)
            .map(pretty_solution)
            .unwrap_or_default();
        let text = format!(
            "{} printed {} {contents}",
            world.entity_name(user),
            world.entity_name(item)
        );
        add_admin_log(world, LogType::Action, LogImpact::Low, &text, &[], &[]);
    }
    update_ui_state(world, master, false);
    click_sound(world, master);
}

/// Разлить в бутылку (`OnOutputToBottleMessage`).
pub fn output_to_bottle(
    world: &mut World,
    master: Entity,
    user: Entity,
    message: &ChemMasterOutputToBottle,
) {
    let Some(container) = item_slots::slot_item(world, master, OUTPUT_SLOT) else {
        return;
    };
    let Some(soln) = containers::try_get_solution(world, container, BOTTLE_SOLUTION) else {
        return;
    };
    let available =
        containers::solution(world, soln).map_or(FixedPoint2::ZERO, |s| s.available_volume());
    let dosage = FixedPoint2::new(message.dosage as i32);
    if message.dosage == 0 || dosage > available {
        return;
    }
    if message.label.chars().count() > LABEL_MAX_LENGTH {
        return;
    }
    let Some(withdrawal) = withdraw_from_source(world, master, dosage, Some(user)) else {
        return;
    };
    crate::labels::label(world, container, Some(&message.label));
    containers::try_add_solution(world, soln, &withdrawal);
    let contents = containers::solution(world, soln)
        .map(pretty_solution)
        .unwrap_or_default();
    let text = format!(
        "{} bottled {} {contents}",
        world.entity_name(user),
        world.entity_name(container)
    );
    add_admin_log(world, LogType::Action, LogImpact::Low, &text, &[], &[]);
    update_ui_state(world, master, false);
    click_sound(world, master);
}

fn with_master<M: 'static + Clone + Send + Sync>(
    world: &mut World,
    handler: fn(&mut World, Entity, Entity, &M),
) {
    world.subscribe::<ChemMaster, BoundUiMessage<M>>(move |world, master, event| {
        let message = event.message.clone();
        handler(world, master, event.actor, &message);
    });
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<ChemMaster>();
    world.register_ui_message::<ChemMasterSetMode>();
    world.register_ui_message::<ChemMasterSortingTypeCycle>();
    world.register_ui_message::<ChemMasterSetPillType>();
    world.register_ui_message::<ChemMasterReagentAmountButton>();
    world.register_ui_message::<ChemMasterCreatePills>();
    world.register_ui_message::<ChemMasterOutputToBottle>();
    world.register_ui_message::<ChemMasterOutputDrawSource>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<ChemMaster, ComponentStartup>(|world, m, _| update_ui_state(world, m, false));
    world.subscribe::<ChemMaster, SolutionChanged>(|world, m, _| update_ui_state(world, m, false));
    world.subscribe::<ChemMaster, EntInsertedIntoContainer>(|world, m, _| {
        update_ui_state(world, m, false);
    });
    world.subscribe::<ChemMaster, EntRemovedFromContainer>(|world, m, _| {
        update_ui_state(world, m, false);
    });
    world.subscribe::<ChemMaster, BoundUiOpened>(|world, m, _| update_ui_state(world, m, false));
    with_master::<ChemMasterSetMode>(world, |world, m, _, message| {
        if let Some(mut master) = world.get_mut::<ChemMaster>(m) {
            master.mode = message.0;
        }
        update_ui_state(world, m, false);
        click_sound(world, m);
    });
    with_master::<ChemMasterSortingTypeCycle>(world, |world, m, _, _| {
        if let Some(mut master) = world.get_mut::<ChemMaster>(m) {
            master.sorting_type = master.sorting_type.next();
        }
        update_ui_state(world, m, false);
        click_sound(world, m);
    });
    with_master::<ChemMasterSetPillType>(world, |world, m, _, message| {
        if message.0 > PILL_TYPES - 1 {
            return;
        }
        if let Some(mut master) = world.get_mut::<ChemMaster>(m) {
            master.pill_type = message.0;
        }
        update_ui_state(world, m, false);
        click_sound(world, m);
    });
    with_master::<ChemMasterReagentAmountButton>(world, |world, m, _, message| {
        if message
            .amount
            .is_some_and(|a| !REAGENT_AMOUNTS.contains(&a))
        {
            return;
        }
        let amount = amount_of(message.amount);
        match world.get::<ChemMaster>(m).map(|c| c.mode) {
            Some(ChemMasterMode::Transfer) => {
                transfer_reagents(world, m, &message.reagent_id, amount, message.from_buffer);
            }
            Some(ChemMasterMode::Discard) => {
                discard_reagents(world, m, &message.reagent_id, amount, message.from_buffer);
            }
            None => return,
        }
        click_sound(world, m);
    });
    with_master::<ChemMasterCreatePills>(world, create_pills);
    with_master::<ChemMasterOutputToBottle>(world, output_to_bottle);
    with_master::<ChemMasterOutputDrawSource>(world, |world, m, _, message| {
        if let Some(mut master) = world.get_mut::<ChemMaster>(m) {
            master.draw_source = message.0;
        }
        update_ui_state(world, m, false);
        click_sound(world, m);
    });
}

#[cfg(test)]
mod tests;
