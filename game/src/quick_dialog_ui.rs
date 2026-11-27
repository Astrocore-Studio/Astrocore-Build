//! Окна быстрого ввода на клиенте: заголовок, подписи и поля, «ОК» и «Отмена»; щелчок по
//! полю — набор с клавиатуры, Tab — следующее поле, Enter — «ОК», Escape — «Отмена».
//! Образец — `QuickDialogSystem` клиента и `DialogWindow` SS14 @ 2f4a2aad.

use astrocore_client::{Game, KeyboardFocus, UiAction, UiStyle, spawn_button, spawn_label};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy_ecs::world::World as GameWorld;

use crate::examine::loc;
use crate::quick_dialog::{
    CANCEL_BUTTON, ClientQuickDialogs, OK_BUTTON, QuickDialogEntryType, client_answer,
};

const PANEL: Color = Color::srgba(0.07, 0.08, 0.11, 0.97);
const FIELD: Color = Color::srgba(0.12, 0.14, 0.18, 1.0);
const FIELD_FOCUSED: Color = Color::srgba(0.20, 0.24, 0.34, 1.0);

#[derive(Component)]
struct DialogPanel;

fn dialogs(world: &mut GameWorld) -> bevy_ecs::world::Mut<'_, ClientQuickDialogs> {
    world.get_resource_or_insert_with(ClientQuickDialogs::default)
}

fn draw(
    mut commands: Commands,
    game: Res<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<DialogPanel>>,
    mut shown: Local<Option<u64>>,
) {
    let world = &game.0;
    let Some(state) = world.get_resource::<ClientQuickDialogs>() else {
        return;
    };
    if *shown == Some(state.version) {
        return;
    }
    *shown = Some(state.version);
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    let last = state.dialogs.len().saturating_sub(1);
    for (index, dialog) in state.dialogs.iter().enumerate() {
        let id = dialog.open.dialog_id;
        let panel = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(35.0),
                    top: Val::Px(160.0 + 24.0 * index as f32),
                    width: Val::Percent(30.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(10.0)),
                    row_gap: Val::Px(4.0),
                    ..default()
                },
                BackgroundColor(PANEL),
                GlobalZIndex(40),
                DialogPanel,
            ))
            .id();
        spawn_label(&mut commands, panel, &style, &dialog.open.title);
        for (field, prompt) in dialog.open.prompts.iter().enumerate() {
            if !prompt.prompt.is_empty() {
                spawn_label(&mut commands, panel, &style, &prompt.prompt);
            }
            let focused = index == last && field == dialog.focused;
            let value = &dialog.values[field];
            let shown_text = match (value.is_empty(), focused) {
                (_, true) => format!("{value}_"),
                (true, false) => prompt.placeholder.clone().unwrap_or_else(|| {
                    let kind = match prompt.kind {
                        QuickDialogEntryType::Integer => "integer",
                        QuickDialogEntryType::Float => "float",
                        QuickDialogEntryType::ShortText => "short-text",
                        QuickDialogEntryType::LongText => "long-text",
                    };
                    loc(world, &format!("quick-dialog-ui-{kind}"), &[])
                }),
                (false, false) => value.clone(),
            };
            commands.spawn((
                ChildOf(panel),
                Interaction::default(),
                Node {
                    min_height: Val::Px(26.0),
                    padding: UiRect::axes(Val::Px(6.0), Val::Px(4.0)),
                    ..default()
                },
                BackgroundColor(if focused { FIELD_FOCUSED } else { FIELD }),
                UiAction::new(move |world| {
                    let mut state = dialogs(world);
                    if let Some(dialog) = state.dialogs.iter_mut().find(|d| d.open.dialog_id == id)
                    {
                        dialog.focused = field;
                    }
                    state.version += 1;
                }),
                children![style.text(shown_text, 14.0)],
            ));
        }
        let buttons = commands
            .spawn((
                ChildOf(panel),
                Node {
                    column_gap: Val::Px(8.0),
                    ..default()
                },
            ))
            .id();
        if dialog.open.buttons & OK_BUTTON != 0 {
            spawn_button(
                &mut commands,
                buttons,
                &style,
                &loc(world, "quick-dialog-ui-ok", &[]),
                UiAction::new(move |world| client_answer(world, id, OK_BUTTON)),
            );
        }
        if dialog.open.buttons & CANCEL_BUTTON != 0 {
            spawn_button(
                &mut commands,
                buttons,
                &style,
                &loc(world, "quick-dialog-ui-cancel", &[]),
                UiAction::new(move |world| client_answer(world, id, CANCEL_BUTTON)),
            );
        }
    }
}

/// Набор в поле верхнего окна.
fn keys(
    mut events: MessageReader<KeyboardInput>,
    mut game: ResMut<Game>,
    mut focus: ResMut<KeyboardFocus>,
    mut had_dialog: Local<bool>,
) {
    let world = &mut game.0;
    let open = world
        .get_resource::<ClientQuickDialogs>()
        .is_some_and(|d| !d.dialogs.is_empty());
    if !open {
        if *had_dialog {
            focus.0 = false;
            *had_dialog = false;
        }
        return;
    }
    *had_dialog = true;
    focus.0 = true;
    for event in events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }
        let Some(id) = world
            .get_resource::<ClientQuickDialogs>()
            .and_then(|d| d.dialogs.last())
            .map(|d| d.open.dialog_id)
        else {
            return;
        };
        match &event.logical_key {
            Key::Enter => {
                client_answer(world, id, OK_BUTTON);
                continue;
            }
            Key::Escape => {
                client_answer(world, id, CANCEL_BUTTON);
                continue;
            }
            _ => {}
        }
        let mut state = dialogs(world);
        let Some(dialog) = state.dialogs.last_mut() else {
            return;
        };
        let count = dialog.values.len();
        match &event.logical_key {
            Key::Tab if count > 0 => dialog.focused = (dialog.focused + 1) % count,
            Key::Backspace => {
                if let Some(value) = dialog.values.get_mut(dialog.focused) {
                    value.pop();
                }
            }
            Key::Space => {
                if let Some(value) = dialog.values.get_mut(dialog.focused) {
                    value.push(' ');
                }
            }
            Key::Character(text) => {
                if let Some(value) = dialog.values.get_mut(dialog.focused) {
                    value.extend(text.chars().filter(|c| !c.is_control()));
                }
            }
            _ => continue,
        }
        state.version += 1;
    }
}

/// Окна быстрого ввода.
pub struct QuickDialogUiPlugin;

impl Plugin for QuickDialogUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, keys.after(bevy::input::InputSystems));
        app.add_systems(Update, draw);
    }
}
