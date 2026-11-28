//! Окно «Вернуться в тело» на клиенте: заголовок, вопрос, «Да» и «Нет». Образец —
//! `ReturnToBodyMenu`, `ReturnToBodyEui` клиента SS14 @ 2f4a2aad.

use astrocore_client::{Game, UiAction, UiStyle, spawn_button, spawn_label};
use bevy::prelude::*;

use crate::examine::loc;
use crate::return_to_body::{ClientReturnToBody, client_answer};

const PANEL: Color = Color::srgba(0.07, 0.08, 0.11, 0.97);

#[derive(Component)]
struct ReturnToBodyPanel;

fn draw(
    mut commands: Commands,
    game: Res<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<ReturnToBodyPanel>>,
    mut shown: Local<bool>,
) {
    let world = &game.0;
    let open = world
        .get_resource::<ClientReturnToBody>()
        .is_some_and(|state| state.open);
    if open == *shown {
        return;
    }
    *shown = open;
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    if !open {
        return;
    }
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(38.0),
                top: Val::Px(200.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                padding: UiRect::all(Val::Px(12.0)),
                row_gap: Val::Px(8.0),
                ..default()
            },
            BackgroundColor(PANEL),
            GlobalZIndex(40),
            ReturnToBodyPanel,
        ))
        .id();
    spawn_label(
        &mut commands,
        panel,
        &style,
        &loc(world, "ghost-return-to-body-title", &[]),
    );
    spawn_label(
        &mut commands,
        panel,
        &style,
        &loc(world, "ghost-return-to-body-text", &[]),
    );
    let buttons = commands
        .spawn((
            ChildOf(panel),
            Node {
                column_gap: Val::Px(20.0),
                ..default()
            },
        ))
        .id();
    spawn_button(
        &mut commands,
        buttons,
        &style,
        &loc(world, "accept-cloning-window-accept-button", &[]),
        UiAction::new(|world| client_answer(world, true)),
    );
    spawn_button(
        &mut commands,
        buttons,
        &style,
        &loc(world, "accept-cloning-window-deny-button", &[]),
        UiAction::new(|world| client_answer(world, false)),
    );
}

/// Окно «Вернуться в тело».
pub struct ReturnToBodyUiPlugin;

impl Plugin for ReturnToBodyUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, draw);
    }
}
