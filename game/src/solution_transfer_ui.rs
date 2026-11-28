//! Окно «Количество за раз» на клиенте: поле ввода целого числа (щелчок — набор с
//! клавиатуры), наименьшее и наибольшее, кнопка «Задать» доступна только для числа в
//! пределах; задание закрывает окно. Образец — `TransferAmountBoundUserInterface`,
//! `TransferAmountWindow` клиента SS14 @ 2f4a2aad.

use astrocore_client::{
    Game, KeyboardFocus, UiAction, UiStyle, WindowKinds, spawn_button, spawn_label,
};
use astrocore_ui::{OpenUi, OpenUis, UiWorldExt};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy_ecs::world::World as GameWorld;

use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::solution_transfer::{SolutionTransfer, TRANSFER_AMOUNT_UI_KEY, TransferAmountSetValue};

const KIND: &str = "TransferAmountBoundUserInterface";
const FIELD: Color = Color::srgba(0.05, 0.06, 0.08, 1.0);
const FIELD_FOCUSED: Color = Color::srgba(0.10, 0.12, 0.18, 1.0);
const DISABLED: Color = Color::srgba(0.12, 0.12, 0.14, 1.0);

/// Набираемое количество. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default)]
pub struct TransferAmountInput {
    pub text: String,
    pub focused: bool,
}

fn input(world: &mut GameWorld) -> bevy_ecs::world::Mut<'_, TransferAmountInput> {
    world.get_resource_or_insert_with(TransferAmountInput::default)
}

fn redraw(world: &mut GameWorld) {
    if let Some(mut open) = world.get_resource_mut::<OpenUis>() {
        open.revision += 1;
    }
}

/// Число в пределах (`OnValueChanged`).
pub fn valid_amount(text: &str, min: i32, max: i32) -> Option<i32> {
    text.trim()
        .parse::<i32>()
        .ok()
        .filter(|v| (min..=max).contains(v))
}

fn window(
    commands: &mut Commands,
    content: Entity,
    ui: &OpenUi,
    world: &GameWorld,
    style: &UiStyle,
) {
    let entity = ui.entity;
    let (min, max) = world
        .get::<SolutionTransfer>(entity)
        .map_or((1, i32::MAX), |c| {
            (
                c.minimum_transfer_amount.int(),
                c.maximum_transfer_amount.int(),
            )
        });
    let typed = world
        .get_resource::<TransferAmountInput>()
        .cloned()
        .unwrap_or_default();
    let shown = if typed.focused {
        format!("{}_", typed.text)
    } else if typed.text.is_empty() {
        loc(world, "ui-transfer-amount-line-edit-placeholder", &[])
    } else {
        typed.text.clone()
    };
    commands.spawn((
        ChildOf(content),
        Interaction::default(),
        Node {
            min_width: Val::Px(240.0),
            min_height: Val::Px(26.0),
            padding: UiRect::axes(Val::Px(6.0), Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(if typed.focused { FIELD_FOCUSED } else { FIELD }),
        UiAction::new(|world| {
            input(world).focused = true;
            redraw(world);
        }),
        children![style.text(shown, 14.0)],
    ));
    let bounds = commands
        .spawn((
            ChildOf(content),
            Node {
                justify_content: JustifyContent::SpaceBetween,
                margin: UiRect::vertical(Val::Px(4.0)),
                ..default()
            },
        ))
        .id();
    spawn_label(
        commands,
        bounds,
        style,
        &loc(
            world,
            "comp-solution-transfer-set-amount-min",
            &[("amount", i64::from(min).into())],
        ),
    );
    spawn_label(
        commands,
        bounds,
        style,
        &loc(
            world,
            "comp-solution-transfer-set-amount-max",
            &[("amount", i64::from(max).into())],
        ),
    );
    let label = loc(world, "ui-transfer-amount-apply", &[]);
    match valid_amount(&typed.text, min, max) {
        Some(value) => {
            spawn_button(
                commands,
                content,
                style,
                &label,
                UiAction::new(move |world| {
                    world.send_ui_message(
                        entity,
                        TRANSFER_AMOUNT_UI_KEY,
                        &TransferAmountSetValue {
                            value: FixedPoint2::new(value),
                        },
                    );
                    *input(world) = TransferAmountInput::default();
                    world.request_close_ui(entity, TRANSFER_AMOUNT_UI_KEY);
                }),
            );
        }
        None => {
            commands.spawn((
                ChildOf(content),
                Node {
                    padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                    ..default()
                },
                BackgroundColor(DISABLED),
                children![style.text(label, 14.0)],
            ));
        }
    }
}

fn amount_window_open(world: &GameWorld) -> bool {
    world
        .get_resource::<OpenUis>()
        .is_some_and(|open| open.list.iter().any(|ui| ui.kind == KIND))
}

/// Набор числа: цифры, Backspace; Enter и Escape — конец набора.
fn keys(
    mut events: MessageReader<KeyboardInput>,
    mut game: ResMut<Game>,
    mut focus: ResMut<KeyboardFocus>,
) {
    let world = &mut game.0;
    let focused = world
        .get_resource::<TransferAmountInput>()
        .is_some_and(|i| i.focused);
    if !amount_window_open(world) {
        if focused {
            *input(world) = TransferAmountInput::default();
            focus.0 = false;
        }
        return;
    }
    if !focused {
        return;
    }
    let mut changed = false;
    for event in events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }
        let mut typed = input(world);
        match &event.logical_key {
            Key::Escape | Key::Enter => typed.focused = false,
            Key::Backspace => {
                typed.text.pop();
            }
            Key::Character(text) => typed.text.extend(text.chars().filter(char::is_ascii_digit)),
            _ => continue,
        }
        changed = true;
    }
    focus.0 = world
        .get_resource::<TransferAmountInput>()
        .is_some_and(|i| i.focused);
    if changed {
        redraw(world);
    }
}

/// Окно количества за раз.
pub struct SolutionTransferUiPlugin;

impl Plugin for SolutionTransferUiPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<WindowKinds>()
            .register(KIND, window);
        app.add_systems(PreUpdate, keys.after(bevy::input::InputSystems));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_must_be_a_whole_number_within_bounds() {
        assert_eq!(valid_amount("30", 5, 120), Some(30));
        assert_eq!(valid_amount("3", 5, 120), None);
        assert_eq!(valid_amount("121", 5, 120), None);
        assert_eq!(valid_amount("1.5", 5, 120), None);
    }
}
