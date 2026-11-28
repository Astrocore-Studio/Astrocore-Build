//! Окно ChemMaster на клиенте и вид таблетки по типу. Вкладка «Вход»: ёмкость (вынуть,
//! реагенты с кнопками 1…120 и «Всё»), буфер (порядок, «Перенос»/«Слив», реагенты с
//! кнопками). Вкладка «Выход»: выход (вынуть, состав или таблетки), источник (буфер или
//! ёмкость) и его объём, этикетка, тип таблетки (20), число таблеток и доза, «Создать»;
//! доза бутылки, «Создать». При новом состоянии поля подставляются как в SS14: доза — меньшее
//! из объёма источника и предела, число — сколько выйдет и влезет, бутылка — сколько влезет;
//! этикетка — название реагента с наименьшим количеством. Образец — `ChemMasterWindow`,
//! `ChemMasterBoundUserInterface`, `ReagentDisplayControl`, клиентский `PillSystem` SS14 @
//! 2f4a2aad.
//!
//! Типы таблеток в окне — номерами, не картинками.

use astrocore_client::{
    Game, KeyboardFocus, Sprite, UiAction, UiStyle, WindowKinds, spawn_button, spawn_label,
};
use astrocore_ecs::EventWorldExt;
use astrocore_net::StateApplied;
use astrocore_ui::{OpenUi, OpenUis, UiWorldExt};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy_ecs::world::World as GameWorld;

use crate::chem_master::{
    ChemMasterCreatePills, ChemMasterDrawSource, ChemMasterMode, ChemMasterOutputDrawSource,
    ChemMasterOutputToBottle, ChemMasterReagentAmountButton, ChemMasterSetMode,
    ChemMasterSetPillType, ChemMasterSortingType, ChemMasterSortingTypeCycle, ChemMasterUiState,
    INPUT_SLOT, LABEL_MAX_LENGTH, OUTPUT_SLOT, PILL_TYPES, REAGENT_AMOUNTS, UI_KEY,
};
use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::ingestion::Pill;
use crate::item_slots::ItemSlotButtonPressed;
use crate::reagent_dispenser::ContainerInfo;
use crate::solution::{ReagentId, reagents_of};

const KIND: &str = "ChemMasterBoundUserInterface";
const SELECTED: Color = Color::srgb(0.30, 0.45, 0.65);
const DISABLED: Color = Color::srgba(0.12, 0.12, 0.14, 1.0);
const DIM: Color = Color::srgb(0.6, 0.6, 0.65);
const FIELD: Color = Color::srgba(0.05, 0.06, 0.08, 1.0);
const FIELD_FOCUSED: Color = Color::srgba(0.10, 0.12, 0.18, 1.0);

/// Поля окна. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default)]
pub struct ChemMasterInput {
    pub output_tab: bool,
    pub label: String,
    pub label_focused: bool,
    pub pill_number: i32,
    pub pill_dosage: i32,
    pub bottle_dosage: i32,
    last_state: Option<ChemMasterUiState>,
}

fn input(world: &mut GameWorld) -> bevy_ecs::world::Mut<'_, ChemMasterInput> {
    world.get_resource_or_insert_with(ChemMasterInput::default)
}

fn redraw(world: &mut GameWorld) {
    if let Some(mut open) = world.get_resource_mut::<OpenUis>() {
        open.revision += 1;
    }
}

/// Поля по новому состоянию (`UpdateDosageFields`, `GenerateLabel`).
pub fn apply_state(
    fields: &mut ChemMasterInput,
    state: &ChemMasterUiState,
    lowest_name: impl FnOnce(&ReagentId) -> String,
) {
    if state.update_label {
        fields.label = generate_label(state, lowest_name);
    }
    let output = state.output_container_info.as_ref();
    let remaining = output.map_or(0, |o| (o.max_volume - o.current_volume).int());
    let holds_reagents = output.is_some_and(|o| o.reagents.is_some());
    let pill_number_max = if holds_reagents { 0 } else { remaining };
    let bottle_max = if holds_reagents { remaining } else { 0 };
    let output_volume = match state.draw_source {
        ChemMasterDrawSource::Internal => state.buffer_current_volume.map_or(0, FixedPoint2::int),
        ChemMasterDrawSource::External => state
            .input_container_info
            .as_ref()
            .map_or(0, |i| i.current_volume.int()),
    };
    fields.pill_dosage = output_volume.min(state.pill_dosage_limit as i32);
    if fields.pill_number > pill_number_max {
        fields.pill_number = pill_number_max;
    }
    if fields.bottle_dosage > bottle_max {
        fields.bottle_dosage = bottle_max;
    }
    fields.pill_number = if fields.pill_dosage > 0 {
        (output_volume / fields.pill_dosage).min(pill_number_max)
    } else {
        0
    };
    fields.bottle_dosage = bottle_max.min(output_volume);
}

fn generate_label(state: &ChemMasterUiState, name: impl FnOnce(&ReagentId) -> String) -> String {
    let buffer_empty = state.buffer_current_volume == Some(FixedPoint2::ZERO)
        && state.draw_source == ChemMasterDrawSource::Internal;
    let input_empty = state
        .input_container_info
        .as_ref()
        .is_some_and(|i| i.current_volume == FixedPoint2::ZERO)
        && state.draw_source == ChemMasterDrawSource::External;
    let Some(input_reagents) = state
        .input_container_info
        .as_ref()
        .and_then(|i| i.reagents.as_ref())
    else {
        return String::new();
    };
    if buffer_empty || input_empty {
        return String::new();
    }
    let list = match state.draw_source {
        ChemMasterDrawSource::Internal => &state.buffer_reagents,
        ChemMasterDrawSource::External => input_reagents,
    };
    list.iter()
        .min_by_key(|(_, q)| *q)
        .map(|(id, _)| name(id))
        .unwrap_or_default()
}

fn row(commands: &mut Commands, parent: Entity) -> Entity {
    commands
        .spawn((
            ChildOf(parent),
            Node {
                column_gap: Val::Px(4.0),
                align_items: AlignItems::Center,
                flex_wrap: FlexWrap::Wrap,
                ..default()
            },
        ))
        .id()
}

fn disabled_button(commands: &mut Commands, parent: Entity, style: &UiStyle, text: &str) {
    let node = commands
        .spawn((
            ChildOf(parent),
            Node {
                padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                margin: UiRect::top(Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(DISABLED),
        ))
        .id();
    commands
        .spawn((ChildOf(node), style.text(text, 14.0)))
        .insert(TextColor(DIM));
}

fn toggle(
    commands: &mut Commands,
    parent: Entity,
    style: &UiStyle,
    text: &str,
    on: bool,
    action: UiAction,
) {
    let node = spawn_button(commands, parent, style, text, action);
    if on {
        commands.entity(node).insert(BackgroundColor(SELECTED));
    }
}

fn dim(commands: &mut Commands, parent: Entity, style: &UiStyle, text: &str) {
    commands
        .spawn((ChildOf(parent), style.text(text, 14.0)))
        .insert(TextColor(DIM));
}

fn reagent_name(world: &GameWorld, id: &ReagentId) -> String {
    match reagents_of(world).get(&id.prototype) {
        Some(info) => loc(world, &info.name, &[]),
        None => loc(world, "chem-master-window-unknown-reagent-text", &[]),
    }
}

/// Строка реагента с кнопками количеств (`CreateReagentTransferButtons`).
#[allow(clippy::too_many_arguments)]
fn reagent_row(
    commands: &mut Commands,
    parent: Entity,
    world: &GameWorld,
    style: &UiStyle,
    entity: Entity,
    id: &ReagentId,
    quantity: FixedPoint2,
    from_buffer: Option<bool>,
) {
    let line = row(commands, parent);
    spawn_label(
        commands,
        line,
        style,
        &format!("{}: ", reagent_name(world, id)),
    );
    dim(commands, line, style, &format!("{quantity}u"));
    let Some(from_buffer) = from_buffer else {
        return;
    };
    let mut amounts: Vec<(String, Option<u32>)> = REAGENT_AMOUNTS
        .iter()
        .map(|a| (a.to_string(), Some(*a)))
        .collect();
    amounts.push((
        loc(world, "chem-master-window-buffer-all-amount", &[]),
        None,
    ));
    for (text, amount) in amounts {
        let reagent_id = id.clone();
        spawn_button(
            commands,
            line,
            style,
            &text,
            UiAction::new(move |world| {
                world.send_ui_message(
                    entity,
                    UI_KEY,
                    &ChemMasterReagentAmountButton {
                        reagent_id: reagent_id.clone(),
                        amount,
                        from_buffer,
                    },
                );
            }),
        );
    }
}

/// Ёмкость или выход (`BuildContainerUI`).
fn container_ui(
    commands: &mut Commands,
    parent: Entity,
    world: &GameWorld,
    style: &UiStyle,
    entity: Entity,
    info: Option<&ContainerInfo>,
    buttons: bool,
) {
    let Some(info) = info else {
        dim(
            commands,
            parent,
            style,
            &loc(world, "chem-master-window-no-container-loaded-text", &[]),
        );
        return;
    };
    let line = row(commands, parent);
    spawn_label(commands, line, style, &format!("{}: ", info.display_name));
    dim(
        commands,
        line,
        style,
        &format!("{}/{}", info.current_volume, info.max_volume),
    );
    for (name, quantity) in info.entities.iter().flatten() {
        let line = row(commands, parent);
        spawn_label(commands, line, style, &format!("{name}: "));
        dim(commands, line, style, &format!("{quantity}u"));
    }
    for (id, quantity) in info.reagents.iter().flatten() {
        reagent_row(
            commands,
            parent,
            world,
            style,
            entity,
            id,
            *quantity,
            buttons.then_some(false),
        );
    }
}

/// Поле числа с кнопками −10, −1, +1, +10 (`SpinBox.InitDefaultButtons`).
fn spin_box(
    commands: &mut Commands,
    parent: Entity,
    style: &UiStyle,
    value: i32,
    valid: impl Fn(i32) -> bool + Copy + Send + Sync + 'static,
    set: fn(&mut ChemMasterInput, i32),
) {
    let line = row(commands, parent);
    for (text, delta) in [("-10", -10), ("-1", -1)] {
        spin_button(commands, line, style, text, value, delta, valid, set);
    }
    spawn_label(commands, line, style, &value.to_string());
    for (text, delta) in [("+1", 1), ("+10", 10)] {
        spin_button(commands, line, style, text, value, delta, valid, set);
    }
}

#[allow(clippy::too_many_arguments)]
fn spin_button(
    commands: &mut Commands,
    parent: Entity,
    style: &UiStyle,
    text: &str,
    value: i32,
    delta: i32,
    valid: impl Fn(i32) -> bool + Copy + Send + Sync + 'static,
    set: fn(&mut ChemMasterInput, i32),
) {
    if !valid(value + delta) {
        disabled_button(commands, parent, style, text);
        return;
    }
    spawn_button(
        commands,
        parent,
        style,
        text,
        UiAction::new(move |world| {
            set(&mut input(world), value + delta);
            redraw(world);
        }),
    );
}

fn input_tab(
    commands: &mut Commands,
    content: Entity,
    world: &GameWorld,
    style: &UiStyle,
    entity: Entity,
    state: &ChemMasterUiState,
) {
    let header = row(commands, content);
    spawn_label(
        commands,
        header,
        style,
        &loc(world, "chem-master-window-container-label", &[]),
    );
    let eject = loc(world, "chem-master-window-eject-button", &[]);
    if state.input_container_info.is_some() {
        spawn_button(
            commands,
            header,
            style,
            &eject,
            UiAction::new(move |world| {
                world.send_ui_message(entity, UI_KEY, &ItemSlotButtonPressed::new(INPUT_SLOT));
            }),
        );
    } else {
        disabled_button(commands, header, style, &eject);
    }
    container_ui(
        commands,
        content,
        world,
        style,
        entity,
        state.input_container_info.as_ref(),
        true,
    );
    let header = row(commands, content);
    spawn_label(
        commands,
        header,
        style,
        &loc(world, "chem-master-window-buffer-text", &[]),
    );
    let sort = match state.sorting_type {
        ChemMasterSortingType::Alphabetical => "chem-master-window-sort-type-alphabetical",
        ChemMasterSortingType::Quantity => "chem-master-window-sort-type-quantity",
        ChemMasterSortingType::Latest => "chem-master-window-sort-type-latest",
        ChemMasterSortingType::None => "chem-master-window-sort-type-none",
    };
    spawn_button(
        commands,
        header,
        style,
        &loc(world, sort, &[]),
        UiAction::new(move |world| {
            world.send_ui_message(entity, UI_KEY, &ChemMasterSortingTypeCycle);
        }),
    );
    for (key, mode) in [
        (
            "chem-master-window-transfer-button",
            ChemMasterMode::Transfer,
        ),
        ("chem-master-window-discard-button", ChemMasterMode::Discard),
    ] {
        toggle(
            commands,
            header,
            style,
            &loc(world, key, &[]),
            state.mode == mode,
            UiAction::new(move |world| {
                world.send_ui_message(entity, UI_KEY, &ChemMasterSetMode(mode));
            }),
        );
    }
    if state.buffer_reagents.is_empty() {
        dim(
            commands,
            content,
            style,
            &loc(world, "chem-master-window-no-container-loaded-text", &[]),
        );
        return;
    }
    let line = row(commands, content);
    spawn_label(
        commands,
        line,
        style,
        &format!("{} ", loc(world, "chem-master-window-buffer-label", &[])),
    );
    let volume = state.buffer_current_volume.unwrap_or_default();
    dim(
        commands,
        line,
        style,
        &format!("{volume}{}", loc(world, "units-u", &[])),
    );
    let mut list: Vec<(ReagentId, String, FixedPoint2)> = state
        .buffer_reagents
        .iter()
        .map(|(id, q)| (id.clone(), reagent_name(world, id), *q))
        .collect();
    match state.sorting_type {
        ChemMasterSortingType::Alphabetical => list.sort_by(|a, b| a.1.cmp(&b.1)),
        ChemMasterSortingType::Quantity => list.sort_by_key(|a| std::cmp::Reverse(a.2)),
        ChemMasterSortingType::Latest => list.reverse(),
        ChemMasterSortingType::None => {}
    }
    for (id, _, quantity) in list {
        reagent_row(
            commands,
            content,
            world,
            style,
            entity,
            &id,
            quantity,
            Some(true),
        );
    }
}

fn output_tab(
    commands: &mut Commands,
    content: Entity,
    world: &GameWorld,
    style: &UiStyle,
    entity: Entity,
    state: &ChemMasterUiState,
    fields: &ChemMasterInput,
) {
    let header = row(commands, content);
    spawn_label(
        commands,
        header,
        style,
        &loc(world, "chem-master-window-container-label", &[]),
    );
    let eject = loc(world, "chem-master-window-eject-button", &[]);
    if state.output_container_info.is_some() {
        spawn_button(
            commands,
            header,
            style,
            &eject,
            UiAction::new(move |world| {
                world.send_ui_message(entity, UI_KEY, &ItemSlotButtonPressed::new(OUTPUT_SLOT));
            }),
        );
    } else {
        disabled_button(commands, header, style, &eject);
    }
    container_ui(
        commands,
        content,
        world,
        style,
        entity,
        state.output_container_info.as_ref(),
        false,
    );
    let source = row(commands, content);
    spawn_label(
        commands,
        source,
        style,
        &loc(world, "chem-master-output-source", &[]),
    );
    for (key, draw) in [
        (
            "chem-master-output-buffer-draw",
            ChemMasterDrawSource::Internal,
        ),
        (
            "chem-master-output-beaker-draw",
            ChemMasterDrawSource::External,
        ),
    ] {
        toggle(
            commands,
            source,
            style,
            &loc(world, key, &[]),
            state.draw_source == draw,
            UiAction::new(move |world| {
                world.send_ui_message(entity, UI_KEY, &ChemMasterOutputDrawSource(draw));
            }),
        );
    }
    let (volume, key) = match state.draw_source {
        ChemMasterDrawSource::Internal => (
            state.buffer_current_volume,
            "chem-master-output-buffer-draw",
        ),
        ChemMasterDrawSource::External => (
            state
                .input_container_info
                .as_ref()
                .map(|i| i.current_volume),
            "chem-master-output-beaker-draw",
        ),
    };
    let current = row(commands, content);
    spawn_label(commands, current, style, &loc(world, key, &[]));
    spawn_label(
        commands,
        current,
        style,
        &loc(world, "chem-master-current-text-label", &[]),
    );
    dim(
        commands,
        current,
        style,
        &format!(" {}u", volume.unwrap_or_default()),
    );
    // Этикетка.
    let shown = if fields.label_focused {
        format!("{}_", fields.label)
    } else {
        fields.label.clone()
    };
    commands.spawn((
        ChildOf(content),
        Interaction::default(),
        Node {
            min_width: Val::Px(260.0),
            min_height: Val::Px(24.0),
            padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
            ..default()
        },
        BackgroundColor(if fields.label_focused {
            FIELD_FOCUSED
        } else {
            FIELD
        }),
        UiAction::new(|world| {
            input(world).label_focused = true;
            redraw(world);
        }),
        children![style.text(shown, 14.0)],
    ));
    // Тип таблетки.
    spawn_label(
        commands,
        content,
        style,
        &loc(world, "chem-master-window-pill-type-label", &[]),
    );
    for start in [0, 10] {
        let line = row(commands, content);
        for pill in start..(start + 10).min(PILL_TYPES) {
            toggle(
                commands,
                line,
                style,
                &(pill + 1).to_string(),
                state.selected_pill_type == pill,
                UiAction::new(move |world| {
                    world.send_ui_message(entity, UI_KEY, &ChemMasterSetPillType(pill));
                }),
            );
        }
    }
    let output = state.output_container_info.as_ref();
    let remaining = output.map_or(0, |o| (o.max_volume - o.current_volume).int());
    let holds_reagents = output.is_some_and(|o| o.reagents.is_some());
    let pill_number_max = if holds_reagents { 0 } else { remaining };
    let bottle_max = if holds_reagents { remaining } else { 0 };
    let limit = state.pill_dosage_limit as i32;
    spawn_label(
        commands,
        content,
        style,
        &loc(world, "chem-master-window-pills-label", &[]),
    );
    spawn_label(
        commands,
        content,
        style,
        &loc(world, "chem-master-window-pills-number-label", &[]),
    );
    spin_box(
        commands,
        content,
        style,
        fields.pill_number,
        move |x| (0..=pill_number_max).contains(&x),
        |f, v| f.pill_number = v,
    );
    spawn_label(
        commands,
        content,
        style,
        &loc(world, "chem-master-window-dose-label", &[]),
    );
    spin_box(
        commands,
        content,
        style,
        fields.pill_dosage,
        move |x| x > 0 && x <= limit,
        |f, v| f.pill_dosage = v,
    );
    let create = loc(world, "chem-master-window-create-button", &[]);
    if output.is_some_and(|o| o.entities.is_some()) {
        let (dosage, number, label) =
            (fields.pill_dosage, fields.pill_number, fields.label.clone());
        spawn_button(
            commands,
            content,
            style,
            &create,
            UiAction::new(move |world| {
                world.send_ui_message(
                    entity,
                    UI_KEY,
                    &ChemMasterCreatePills {
                        dosage: dosage.max(0) as u32,
                        number: number.max(0) as u32,
                        label: label.clone(),
                    },
                );
            }),
        );
    } else {
        disabled_button(commands, content, style, &create);
    }
    spawn_label(
        commands,
        content,
        style,
        &loc(world, "chem-master-window-bottles-label", &[]),
    );
    spawn_label(
        commands,
        content,
        style,
        &loc(world, "chem-master-window-dose-label", &[]),
    );
    spin_box(
        commands,
        content,
        style,
        fields.bottle_dosage,
        move |x| (0..=bottle_max).contains(&x),
        |f, v| f.bottle_dosage = v,
    );
    if holds_reagents {
        let (dosage, label) = (fields.bottle_dosage, fields.label.clone());
        spawn_button(
            commands,
            content,
            style,
            &create,
            UiAction::new(move |world| {
                world.send_ui_message(
                    entity,
                    UI_KEY,
                    &ChemMasterOutputToBottle {
                        dosage: dosage.max(0) as u32,
                        label: label.clone(),
                    },
                );
            }),
        );
    } else {
        disabled_button(commands, content, style, &create);
    }
}

fn window(
    commands: &mut Commands,
    content: Entity,
    ui: &OpenUi,
    world: &GameWorld,
    style: &UiStyle,
) {
    let entity = ui.entity;
    let state = ui.state::<ChemMasterUiState>().unwrap_or_default();
    let fields = world
        .get_resource::<ChemMasterInput>()
        .cloned()
        .unwrap_or_default();
    let tabs = row(commands, content);
    for (key, output) in [
        ("chem-master-window-input-tab", false),
        ("chem-master-window-output-tab", true),
    ] {
        toggle(
            commands,
            tabs,
            style,
            &loc(world, key, &[]),
            fields.output_tab == output,
            UiAction::new(move |world| {
                input(world).output_tab = output;
                redraw(world);
            }),
        );
    }
    if fields.output_tab {
        output_tab(commands, content, world, style, entity, &state, &fields);
    } else {
        input_tab(commands, content, world, style, entity, &state);
    }
}

fn chem_master_open(world: &GameWorld) -> Option<ChemMasterUiState> {
    world
        .get_resource::<OpenUis>()?
        .list
        .iter()
        .find(|ui| ui.kind == KIND)
        .and_then(|ui| ui.state::<ChemMasterUiState>())
}

/// Новое состояние сервера — подставить поля; набор этикетки с клавиатуры.
fn update_fields(
    mut events: MessageReader<KeyboardInput>,
    mut game: ResMut<Game>,
    mut focus: ResMut<KeyboardFocus>,
) {
    let world = &mut game.0;
    let Some(state) = chem_master_open(world) else {
        if world
            .get_resource::<ChemMasterInput>()
            .is_some_and(|i| i.label_focused)
        {
            input(world).label_focused = false;
            focus.0 = false;
        }
        return;
    };
    let mut changed = false;
    if input(world).last_state.as_ref() != Some(&state) {
        let name = |id: &ReagentId| reagent_name(world, id);
        let mut fields = world
            .get_resource::<ChemMasterInput>()
            .cloned()
            .unwrap_or_default();
        apply_state(&mut fields, &state, name);
        fields.last_state = Some(state);
        *input(world) = fields;
        changed = true;
    }
    if world
        .get_resource::<ChemMasterInput>()
        .is_some_and(|i| i.label_focused)
    {
        for event in events.read() {
            if event.state != ButtonState::Pressed {
                continue;
            }
            let mut fields = input(world);
            match &event.logical_key {
                Key::Escape | Key::Enter => fields.label_focused = false,
                Key::Backspace => {
                    fields.label.pop();
                }
                Key::Space if fields.label.chars().count() < LABEL_MAX_LENGTH => {
                    fields.label.push(' ')
                }
                Key::Character(text) => {
                    for c in text.chars() {
                        if fields.label.chars().count() < LABEL_MAX_LENGTH {
                            fields.label.push(c);
                        }
                    }
                }
                _ => continue,
            }
            changed = true;
        }
    }
    focus.0 = world
        .get_resource::<ChemMasterInput>()
        .is_some_and(|i| i.label_focused);
    if changed {
        redraw(world);
    }
}

/// Слой таблетки по типу (`PillSystem.OnHandleState`): `pill{тип+1}`.
fn update_pill_sprites(world: &mut GameWorld) {
    let list: Vec<(bevy_ecs::entity::Entity, u32)> = world
        .query::<(bevy_ecs::entity::Entity, &Pill, &Sprite)>()
        .iter(world)
        .map(|(e, pill, _)| (e, pill.pill_type))
        .collect();
    for (entity, pill_type) in list {
        let state = format!("pill{}", pill_type + 1);
        let Some(mut sprite) = world.get_mut::<Sprite>(entity) else {
            continue;
        };
        if let Some(layer) = sprite.layers.first()
            && layer.state.as_deref() != Some(state.as_str())
        {
            sprite.layers[0].state = Some(state);
        }
    }
}

/// Вид таблеток в мире клиента.
pub(crate) fn install(world: &mut GameWorld) {
    world.subscribe_broadcast::<StateApplied>(|world, _| update_pill_sprites(world));
}

/// Окно ChemMaster.
pub struct ChemMasterUiPlugin;

impl Plugin for ChemMasterUiPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<WindowKinds>()
            .register(KIND, window);
        app.add_systems(PreUpdate, update_fields.after(bevy::input::InputSystems));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_follow_the_new_state_like_ss14() {
        let mut fields = ChemMasterInput::default();
        let state = ChemMasterUiState {
            buffer_current_volume: Some(FixedPoint2::new(47)),
            pill_dosage_limit: 20,
            output_container_info: Some(ContainerInfo {
                display_name: "canister".into(),
                current_volume: FixedPoint2::new(1),
                max_volume: FixedPoint2::new(10),
                reagents: None,
                entities: Some(Vec::new()),
            }),
            ..Default::default()
        };
        apply_state(&mut fields, &state, |_| String::new());
        // Доза — до предела, число — сколько выйдет из 47 и влезет в 9 мест.
        assert_eq!(fields.pill_dosage, 20);
        assert_eq!(fields.pill_number, 2);
        assert_eq!(fields.bottle_dosage, 0);
    }
}
