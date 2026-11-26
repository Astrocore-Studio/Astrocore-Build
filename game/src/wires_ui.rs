//! Окно проводов на клиенте: провода с цветом и буквой, кнопки «резать», «починить»,
//! «импульс» и лампочки состояния. Образец — `WiresBoundUserInterface` и `WiresMenu`
//! SS14 @ 2f4a2aad.

use astrocore_client::{UiAction, WindowKinds, spawn_button, spawn_label};
use astrocore_ui::UiWorldExt;
use bevy::prelude::*;

use crate::examine::loc;
use crate::wires::{StatusLightState, WIRES_UI_KEY, WiresAction, WiresActionMessage, WiresUiState};

fn light(state: StatusLightState) -> &'static str {
    match state {
        StatusLightState::Off => "не горит",
        StatusLightState::On => "горит",
        StatusLightState::BlinkingFast => "часто мигает",
        StatusLightState::BlinkingSlow => "медленно мигает",
    }
}

/// Окно проводов.
pub struct WiresUiPlugin;

impl Plugin for WiresUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WindowKinds>();
        let mut kinds = app.world_mut().resource_mut::<WindowKinds>();
        kinds.register(
            "WiresBoundUserInterface",
            |commands, content, ui, world, style| {
                let Some(state) = ui.state::<WiresUiState>() else {
                    spawn_label(commands, content, style, "Нет данных");
                    return;
                };
                let title = match &state.serial_number {
                    Some(serial) => format!("{} — {serial}", state.board_name),
                    None => state.board_name.clone(),
                };
                spawn_label(commands, content, style, &title);
                for status in &state.statuses {
                    spawn_label(
                        commands,
                        content,
                        style,
                        &format!("{}: {}", status.text, light(status.state)),
                    );
                }
                let entity = ui.entity;
                for wire in &state.wires {
                    let color = loc(world, &wire.color.loc_id(), &[]);
                    let cut = if wire.is_cut {
                        " (перерезан)"
                    } else {
                        ""
                    };
                    spawn_label(
                        commands,
                        content,
                        style,
                        &format!("{} {color}{cut}", wire.letter),
                    );
                    let id = wire.id;
                    let (text, action) = if wire.is_cut {
                        ("Починить", WiresAction::Mend)
                    } else {
                        ("Резать", WiresAction::Cut)
                    };
                    spawn_button(
                        commands,
                        content,
                        style,
                        text,
                        UiAction::new(move |world| {
                            world.send_ui_message(
                                entity,
                                WIRES_UI_KEY,
                                &WiresActionMessage { id, action },
                            );
                        }),
                    );
                    if !wire.is_cut {
                        spawn_button(
                            commands,
                            content,
                            style,
                            "Импульс",
                            UiAction::new(move |world| {
                                world.send_ui_message(
                                    entity,
                                    WIRES_UI_KEY,
                                    &WiresActionMessage {
                                        id,
                                        action: WiresAction::Pulse,
                                    },
                                );
                            }),
                        );
                    }
                }
            },
        );
    }
}
