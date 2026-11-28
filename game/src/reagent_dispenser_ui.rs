//! Окно химраздатчика на клиенте: слева — количество за раз (сетка 3×3, выбранное
//! выделено), имя и заполнение ёмкости, «Очистить» и «Вынуть» (без ёмкости недоступны);
//! справа — карточки банок по названию (цвет, название, остаток; пустую не нажать; кнопка
//! «вынуть банку») и состав ёмкости. Образец — `ReagentDispenserBoundUserInterface`,
//! `ReagentDispenserWindow`, `ReagentCardControl` клиента SS14 @ 2f4a2aad.
//!
//! Нет пока вида ёмкости в окне (`SpriteView`).

use astrocore_client::{UiAction, UiStyle, WindowKinds, spawn_button, spawn_label};
use astrocore_ui::{OpenUi, UiWorldExt};
use bevy::prelude::*;
use bevy_ecs::world::World as GameWorld;

use crate::examine::loc;
use crate::item_slots::ItemSlotButtonPressed;
use crate::reagent_dispenser::{
    ClearContainerSolution, DISPENSE_AMOUNTS, DispenseReagent, EjectContainer, OUTPUT_SLOT,
    ReagentDispenserUiState, SetDispenseAmount, UI_KEY,
};
use crate::solution::reagents_of;

const KIND: &str = "ReagentDispenserBoundUserInterface";
const SELECTED: Color = Color::srgb(0.30, 0.45, 0.65);
const DISABLED: Color = Color::srgba(0.12, 0.12, 0.14, 1.0);
const DIM: Color = Color::srgb(0.6, 0.6, 0.65);

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

fn column(commands: &mut Commands, parent: Entity, width: Option<f32>) -> Entity {
    commands
        .spawn((
            ChildOf(parent),
            Node {
                flex_direction: FlexDirection::Column,
                min_width: width.map_or(Val::Auto, Val::Px),
                row_gap: Val::Px(4.0),
                ..default()
            },
        ))
        .id()
}

fn row(commands: &mut Commands, parent: Entity) -> Entity {
    commands
        .spawn((
            ChildOf(parent),
            Node {
                column_gap: Val::Px(4.0),
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .id()
}

/// Окно по состоянию сервера (`UpdateState`).
fn window(
    commands: &mut Commands,
    content: Entity,
    ui: &OpenUi,
    world: &GameWorld,
    style: &UiStyle,
) {
    let entity = ui.entity;
    let state = ui.state::<ReagentDispenserUiState>().unwrap_or_default();
    let body = commands
        .spawn((
            ChildOf(content),
            Node {
                column_gap: Val::Px(12.0),
                ..default()
            },
        ))
        .id();
    let left = column(commands, body, Some(170.0));
    spawn_label(
        commands,
        left,
        style,
        &loc(
            world,
            "reagent-dispenser-window-amount-to-dispense-label",
            &[],
        ),
    );
    for chunk in DISPENSE_AMOUNTS.chunks(3) {
        let line = row(commands, left);
        for amount in chunk.iter().copied() {
            let node = spawn_button(
                commands,
                line,
                style,
                &amount.to_string(),
                UiAction::new(move |world| {
                    world.send_ui_message(entity, UI_KEY, &SetDispenseAmount { amount });
                }),
            );
            if amount == state.selected_dispense_amount {
                commands.entity(node).insert(BackgroundColor(SELECTED));
            }
        }
    }
    let catalog = reagents_of(world);
    let right = column(commands, body, Some(420.0));
    if let Some(info) = &state.output_container {
        spawn_label(commands, left, style, &info.display_name);
        spawn_label(
            commands,
            left,
            style,
            &format!("{}/{}", info.current_volume, info.max_volume),
        );
    }
    let buttons = row(commands, left);
    let clear = loc(world, "reagent-dispenser-window-clear-button", &[]);
    let eject = loc(world, "reagent-dispenser-window-eject-button", &[]);
    if state.output_container.is_some() {
        spawn_button(
            commands,
            buttons,
            style,
            &clear,
            UiAction::new(move |world| {
                world.send_ui_message(entity, UI_KEY, &ClearContainerSolution);
            }),
        );
        spawn_button(
            commands,
            buttons,
            style,
            &eject,
            UiAction::new(move |world| {
                world.send_ui_message(entity, UI_KEY, &ItemSlotButtonPressed::new(OUTPUT_SLOT));
            }),
        );
    } else {
        disabled_button(commands, buttons, style, &clear);
        disabled_button(commands, buttons, style, &eject);
    }
    // Карточки банок по названию.
    let mut inventory = state.inventory.clone();
    inventory.sort_by(|a, b| a.reagent_label.cmp(&b.reagent_label));
    let eject_jug = loc(
        world,
        "reagent-dispenser-window-eject-container-button",
        &[],
    );
    for chunk in inventory.chunks(3) {
        let line = row(commands, right);
        for item in chunk {
            let card = row(commands, line);
            let color = Srgba::hex(item.reagent_color.trim_start_matches('#'))
                .map(Color::from)
                .unwrap_or(Color::WHITE);
            commands.spawn((
                ChildOf(card),
                Node {
                    width: Val::Px(8.0),
                    height: Val::Px(28.0),
                    ..default()
                },
                BackgroundColor(color),
            ));
            let text = format!(
                "{} {}",
                item.reagent_label,
                loc(
                    world,
                    "reagent-dispenser-window-quantity-label-text",
                    &[("quantity", item.quantity.to_string().into())],
                )
            );
            let location = item.storage_location;
            if item.quantity.float() == 0.0 {
                disabled_button(commands, card, style, &text);
            } else {
                spawn_button(
                    commands,
                    card,
                    style,
                    &text,
                    UiAction::new(move |world| {
                        world.send_ui_message(
                            entity,
                            UI_KEY,
                            &DispenseReagent {
                                storage_location: location,
                            },
                        );
                    }),
                );
            }
            spawn_button(
                commands,
                card,
                style,
                &eject_jug,
                UiAction::new(move |world| {
                    world.send_ui_message(
                        entity,
                        UI_KEY,
                        &EjectContainer {
                            storage_location: location,
                        },
                    );
                }),
            );
        }
    }
    // Состав ёмкости.
    let Some(info) = &state.output_container else {
        spawn_label(
            commands,
            right,
            style,
            &loc(
                world,
                "reagent-dispenser-window-no-container-loaded-text",
                &[],
            ),
        );
        return;
    };
    for (reagent, quantity) in info.reagents.iter().flatten() {
        let name = match catalog.get(&reagent.prototype) {
            Some(found) => loc(world, &found.name, &[]),
            None => loc(
                world,
                "reagent-dispenser-window-reagent-name-not-found-text",
                &[],
            ),
        };
        let line = row(commands, right);
        spawn_label(commands, line, style, &format!("{name}: "));
        let text = loc(
            world,
            "reagent-dispenser-window-quantity-label-text",
            &[("quantity", quantity.to_string().into())],
        );
        commands
            .spawn((ChildOf(line), style.text(text, 14.0)))
            .insert(TextColor(DIM));
    }
}

/// Окно химраздатчика.
pub struct ReagentDispenserUiPlugin;

impl Plugin for ReagentDispenserUiPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<WindowKinds>()
            .register(KIND, window);
    }
}
