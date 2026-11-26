//! Окна электросети на клиенте: контроллер AME (`AmeControllerBoundUserInterface`,
//! `AmeWindow`), АПЦ (`ApcBoundUserInterface`, `ApcMenu`) и консоль ID-карт
//! (`IdCardConsoleBoundUserInterface`, `IdCardConsoleWindow`) SS14 @ 2f4a2aad.

use astrocore_client::{UiAction, WindowKinds, spawn_button, spawn_label};
use astrocore_ui::UiWorldExt;
use bevy::prelude::*;

use crate::ame::{AME_UI_KEY, AmeUiButton, AmeUiState};
use crate::id_console::{ID_CONSOLE_UI_KEY, IdConsoleState, WriteToTargetId};
use crate::power::{APC_UI_KEY, ApcExternalPower, ApcToggleMainBreaker, ApcUiState};

fn yes_no(value: bool) -> &'static str {
    if value { "да" } else { "нет" }
}

/// Окна AME, АПЦ и консоли ID-карт.
pub struct PowerUiPlugin;

impl Plugin for PowerUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WindowKinds>();
        let mut kinds = app.world_mut().resource_mut::<WindowKinds>();
        kinds.register(
            "AmeControllerBoundUserInterface",
            |commands, content, ui, _, style| {
                let Some(state) = ui.state::<AmeUiState>() else {
                    spawn_label(commands, content, style, "Нет данных");
                    return;
                };
                let lines = [
                    format!("Питание: {}", yes_no(state.has_power)),
                    format!("Главный контроллер: {}", yes_no(state.is_master)),
                    format!(
                        "Впрыск: {}",
                        if state.injecting {
                            "идёт"
                        } else {
                            "остановлен"
                        }
                    ),
                    format!(
                        "Топливо: {}",
                        if state.has_fuel_jar {
                            state.fuel_amount.to_string()
                        } else {
                            "нет банки".to_owned()
                        }
                    ),
                    format!("Впрыск за раз: {}", state.injection_amount),
                    format!("Ядер: {}", state.core_count),
                    format!("Мощность: {:.0} кВт", state.current_power_supply),
                    format!("Цель: {:.0} кВт", state.targeted_power_supply),
                ];
                for line in lines {
                    spawn_label(commands, content, style, &line);
                }
                let entity = ui.entity;
                for (text, button) in [
                    ("Впрыск вкл/выкл", AmeUiButton::ToggleInjection),
                    ("Больше топлива", AmeUiButton::IncreaseFuel),
                    ("Меньше топлива", AmeUiButton::DecreaseFuel),
                    ("Вынуть банку", AmeUiButton::Eject),
                ] {
                    spawn_button(
                        commands,
                        content,
                        style,
                        text,
                        UiAction::new(move |world| {
                            world.send_ui_message(entity, AME_UI_KEY, &button);
                        }),
                    );
                }
            },
        );
        kinds.register(
            "IdCardConsoleBoundUserInterface",
            |commands, content, ui, _, style| {
                let Some(state) = ui.state::<IdConsoleState>() else {
                    spawn_label(commands, content, style, "Нет данных");
                    return;
                };
                let privileged = if !state.privileged_present {
                    "нет".to_owned()
                } else if state.privileged_authorized {
                    state.privileged_name.clone()
                } else {
                    format!("{} (нет прав)", state.privileged_name)
                };
                spawn_label(
                    commands,
                    content,
                    style,
                    &format!("Карта с правами: {privileged}"),
                );
                if !state.target_present {
                    spawn_label(commands, content, style, "Изменяемая карта: нет");
                    return;
                }
                let name = state.target_full_name.clone().unwrap_or_default();
                let job = state.target_job_title.clone().unwrap_or_default();
                for line in [
                    format!("Изменяемая карта: {}", state.target_name),
                    format!("Имя: {name}"),
                    format!("Должность: {job}"),
                ] {
                    spawn_label(commands, content, style, &line);
                }
                if !state.privileged_authorized {
                    return;
                }
                let entity = ui.entity;
                for level in &state.console_access {
                    let has = state.target_access.contains(level);
                    let allowed = state.allowed_access.contains(level);
                    let mark = if has { "[x]" } else { "[ ]" };
                    let text = if allowed {
                        format!("{mark} {level}")
                    } else {
                        format!("{mark} {level} (нельзя)")
                    };
                    let mut access: Vec<String> = state.target_access.clone();
                    if has {
                        access.retain(|tag| tag != level);
                    } else {
                        access.push(level.clone());
                    }
                    let message = WriteToTargetId {
                        full_name: name.clone(),
                        job_title: job.clone(),
                        access,
                    };
                    spawn_button(
                        commands,
                        content,
                        style,
                        &text,
                        UiAction::new(move |world| {
                            world.send_ui_message(entity, ID_CONSOLE_UI_KEY, &message);
                        }),
                    );
                }
            },
        );
        kinds.register(
            "ApcBoundUserInterface",
            |commands, content, ui, _, style| {
                let Some(state) = ui.state::<ApcUiState>() else {
                    spawn_label(commands, content, style, "Нет данных");
                    return;
                };
                let external = match state.external {
                    ApcExternalPower::None => "нет",
                    ApcExternalPower::Low => "мало",
                    ApcExternalPower::Good => "есть",
                };
                for line in [
                    format!(
                        "Главный выключатель: {}",
                        if state.main_breaker {
                            "включён"
                        } else {
                            "выключен"
                        }
                    ),
                    format!("Внешнее питание: {external}"),
                    format!("Заряд: {:.0} %", state.charge * 100.0),
                    format!("Нагрузка: {} Вт", state.power),
                ] {
                    spawn_label(commands, content, style, &line);
                }
                let entity = ui.entity;
                spawn_button(
                    commands,
                    content,
                    style,
                    "Переключить выключатель",
                    UiAction::new(move |world| {
                        world.send_ui_message(entity, APC_UI_KEY, &ApcToggleMainBreaker);
                    }),
                );
            },
        );
    }
}
