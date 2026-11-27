//! Окна электросети на клиенте: контроллер AME (`AmeControllerBoundUserInterface`,
//! `AmeWindow`), АПЦ (`ApcBoundUserInterface`, `ApcMenu`) и консоль ID-карт
//! (`IdCardConsoleBoundUserInterface`, `IdCardConsoleWindow`), машина с зарядом — генератор
//! гравитации (`PowerChargeBoundUserInterface`, `PowerChargeWindow`) и пульт ускорителя частиц
//! (`ParticleAcceleratorBoundUserInterface`, `ParticleAcceleratorControlMenu`), газовый баллон
//! (`GasTankBoundUserInterface`, `GasTankWindow`), канистра (`GasCanisterBoundUserInterface`,
//! `GasCanisterWindow`) и термомашина (`GasThermomachineBoundUserInterface`,
//! `GasThermomachineWindow`), газоанализатор (`GasAnalyzerBoundUserInterface`,
//! `GasAnalyzerWindow`, без схемы устройства), воздушная тревога (`AirAlarmBoundUserInterface`,
//! `AirAlarmWindow`: режимы и показания; пороги и настройка отдельных устройств — позже)
//! SS14 @ 2f4a2aad. Поля ввода давления и температуры заменены
//! кнопками шага.

use astrocore_client::{UiAction, WindowKinds, spawn_button, spawn_label};
use astrocore_ui::UiWorldExt;
use bevy::prelude::*;

use crate::ame::{AME_UI_KEY, AmeUiButton, AmeUiState};
use crate::atmos_alarms::{
    AIR_ALARM_UI_KEY, AirAlarmMode, AirAlarmResyncAllDevices, AirAlarmUiState,
    AirAlarmUpdateAlarmMode, AirAlarmUpdateAutoMode, AtmosAlarmType,
};
use crate::atmos_portables::{
    CANISTER_UI_KEY, GasCanisterChangeReleasePressure, GasCanisterChangeReleaseValve,
    GasCanisterHoldingTankEject, GasCanisterUiState, GasThermomachineChangeTemperature,
    GasThermomachineToggle, THERMOMACHINE_UI_KEY, ThermoMachineUiState,
};
use crate::gas_analyzer::GasAnalyzerState;
use crate::gas_tank::{
    GAS_TANK_UI_KEY, GasTankSetPressure, GasTankToggleInternals, GasTankUiState,
};
use crate::gravity::{
    POWER_CHARGE_UI_KEY, PowerChargeStatus, PowerChargeUiState, SwitchChargingMachine,
};
use crate::id_console::{ID_CONSOLE_UI_KEY, IdConsoleState, WriteToTargetId};
use crate::particle_accelerator::{
    ParticleAcceleratorUiState, RescanParts, SetEnable, SetPowerState, UI_KEY as PA_UI_KEY,
};
use crate::power::{APC_UI_KEY, ApcExternalPower, ApcToggleMainBreaker, ApcUiState};

/// Название газа в окне анализатора (`gas-*` из перевода SS14).
fn gas_name(id: &str) -> &str {
    match id {
        "Oxygen" => "Кислород",
        "Nitrogen" => "Азот",
        "CarbonDioxide" => "Углекислый газ",
        "Plasma" => "Плазма",
        "Tritium" => "Тритий",
        "WaterVapor" => "Водяной пар",
        "Ammonia" => "Аммиак",
        "NitrousOxide" => "Оксид азота",
        "Frezon" => "Фрезон",
        other => other,
    }
}

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
            "ParticleAcceleratorBoundUserInterface",
            |commands, content, ui, _, style| {
                let Some(state) = ui.state::<ParticleAcceleratorUiState>() else {
                    spawn_label(commands, content, style, "Нет данных");
                    return;
                };
                let parts = [
                    ("торец", state.end_cap),
                    ("топливная камера", state.fuel_chamber),
                    ("силовой блок", state.power_box),
                    ("левый излучатель", state.emitter_port),
                    ("центральный излучатель", state.emitter_fore),
                    ("правый излучатель", state.emitter_starboard),
                ];
                let missing: Vec<&str> = parts
                    .iter()
                    .filter(|(_, ok)| !ok)
                    .map(|(n, _)| *n)
                    .collect();
                let level = match state.state {
                    2..=5 => (state.state - 1).to_string(),
                    _ => "ожидание".to_owned(),
                };
                for line in [
                    format!("Собран: {}", yes_no(state.assembled)),
                    format!("Включён: {}", yes_no(state.enabled)),
                    format!("Сила: {level}"),
                    format!(
                        "Мощность: {} из {} Вт",
                        state.power_receive, state.power_draw
                    ),
                ] {
                    spawn_label(commands, content, style, &line);
                }
                if !missing.is_empty() {
                    spawn_label(
                        commands,
                        content,
                        style,
                        &format!("Нет частей: {}", missing.join(", ")),
                    );
                }
                if state.interface_block {
                    spawn_label(commands, content, style, "Управление заблокировано");
                    return;
                }
                let entity = ui.entity;
                let enabled = !state.enabled;
                spawn_button(
                    commands,
                    content,
                    style,
                    if state.enabled {
                        "Выключить"
                    } else {
                        "Включить"
                    },
                    UiAction::new(move |world| {
                        world.send_ui_message(entity, PA_UI_KEY, &SetEnable { enabled });
                    }),
                );
                for (text, delta) in [("Сила −", -1i8), ("Сила +", 1)] {
                    let target = (state.state as i8 + delta).clamp(1, state.max_level as i8) as u8;
                    spawn_button(
                        commands,
                        content,
                        style,
                        text,
                        UiAction::new(move |world| {
                            world.send_ui_message(
                                entity,
                                PA_UI_KEY,
                                &SetPowerState { state: target },
                            );
                        }),
                    );
                }
                spawn_button(
                    commands,
                    content,
                    style,
                    "Найти части",
                    UiAction::new(move |world| {
                        world.send_ui_message(entity, PA_UI_KEY, &RescanParts);
                    }),
                );
            },
        );
        kinds.register(
            "PowerChargeBoundUserInterface",
            |commands, content, ui, _, style| {
                let Some(state) = ui.state::<PowerChargeUiState>() else {
                    spawn_label(commands, content, style, "Нет данных");
                    return;
                };
                let status = match state.status {
                    PowerChargeStatus::Off => "выключен",
                    PowerChargeStatus::Discharging => "разряжается",
                    PowerChargeStatus::Charging => "заряжается",
                    PowerChargeStatus::FullyCharged => "заряжен",
                };
                let eta = if state.eta_seconds >= 0 {
                    format!("{} с", state.eta_seconds)
                } else {
                    "—".to_owned()
                };
                for line in [
                    format!(
                        "Питание: {}",
                        if state.on {
                            "включено"
                        } else {
                            "выключено"
                        }
                    ),
                    format!(
                        "Потребление: {} из {} Вт",
                        state.power_draw, state.power_draw_max
                    ),
                    format!("Заряд: {:.0} %", f32::from(state.charge) / 255.0 * 100.0),
                    format!("Состояние: {status}"),
                    format!("До цели: {eta}"),
                ] {
                    spawn_label(commands, content, style, &line);
                }
                let entity = ui.entity;
                let on = !state.on;
                spawn_button(
                    commands,
                    content,
                    style,
                    if state.on {
                        "Выключить"
                    } else {
                        "Включить"
                    },
                    UiAction::new(move |world| {
                        world.send_ui_message(
                            entity,
                            POWER_CHARGE_UI_KEY,
                            &SwitchChargingMachine { on },
                        );
                    }),
                );
            },
        );
        kinds.register(
            "GasTankBoundUserInterface",
            |commands, content, ui, _, style| {
                let Some(state) = ui.state::<GasTankUiState>() else {
                    spawn_label(commands, content, style, "Нет данных");
                    return;
                };
                for line in [
                    format!("Давление в баллоне: {:.1} кПа", state.tank_pressure),
                    format!("Давление выпуска: {:.1} кПа", state.release_pressure),
                    format!(
                        "Дыхание из баллона: {}",
                        if state.connected {
                            "подключено"
                        } else {
                            "отключено"
                        }
                    ),
                ] {
                    spawn_label(commands, content, style, &line);
                }
                let entity = ui.entity;
                for (text, step) in [("Выпуск −10 кПа", -10.0f32), ("Выпуск +10 кПа", 10.0)]
                {
                    let pressure = (state.release_pressure + step)
                        .clamp(state.min_release_pressure, state.max_release_pressure);
                    spawn_button(
                        commands,
                        content,
                        style,
                        text,
                        UiAction::new(move |world| {
                            world.send_ui_message(
                                entity,
                                GAS_TANK_UI_KEY,
                                &GasTankSetPressure { pressure },
                            );
                        }),
                    );
                }
                if state.connected || state.can_connect {
                    let text = if state.connected {
                        "Отключить дыхание"
                    } else {
                        "Подключить дыхание"
                    };
                    spawn_button(
                        commands,
                        content,
                        style,
                        text,
                        UiAction::new(move |world| {
                            world.send_ui_message(entity, GAS_TANK_UI_KEY, &GasTankToggleInternals);
                        }),
                    );
                }
            },
        );
        kinds.register(
            "GasCanisterBoundUserInterface",
            |commands, content, ui, _, style| {
                let Some(state) = ui.state::<GasCanisterUiState>() else {
                    spawn_label(commands, content, style, "Нет данных");
                    return;
                };
                let mut lines = vec![
                    format!("Давление в канистре: {:.1} кПа", state.canister_pressure),
                    format!(
                        "Порт: {}",
                        if state.port_status {
                            "подключён"
                        } else {
                            "не подключён"
                        }
                    ),
                    format!("Давление выпуска: {:.1} кПа", state.release_pressure),
                    format!(
                        "Клапан: {}",
                        if state.release_valve_open {
                            "открыт"
                        } else {
                            "закрыт"
                        }
                    ),
                ];
                if state.has_tank {
                    lines.push(format!("Баллон: {:.1} кПа", state.tank_pressure));
                } else {
                    lines.push("Баллона нет".to_owned());
                }
                for line in lines {
                    spawn_label(commands, content, style, &line);
                }
                let entity = ui.entity;
                for (text, step) in [("Выпуск −50 кПа", -50.0f32), ("Выпуск +50 кПа", 50.0)]
                {
                    let pressure = (state.release_pressure + step)
                        .clamp(state.min_release_pressure, state.max_release_pressure);
                    spawn_button(
                        commands,
                        content,
                        style,
                        text,
                        UiAction::new(move |world| {
                            world.send_ui_message(
                                entity,
                                CANISTER_UI_KEY,
                                &GasCanisterChangeReleasePressure { pressure },
                            );
                        }),
                    );
                }
                let valve = !state.release_valve_open;
                spawn_button(
                    commands,
                    content,
                    style,
                    if valve {
                        "Открыть клапан"
                    } else {
                        "Закрыть клапан"
                    },
                    UiAction::new(move |world| {
                        world.send_ui_message(
                            entity,
                            CANISTER_UI_KEY,
                            &GasCanisterChangeReleaseValve { valve },
                        );
                    }),
                );
                if state.has_tank {
                    spawn_button(
                        commands,
                        content,
                        style,
                        "Вынуть баллон",
                        UiAction::new(move |world| {
                            world.send_ui_message(
                                entity,
                                CANISTER_UI_KEY,
                                &GasCanisterHoldingTankEject,
                            );
                        }),
                    );
                }
            },
        );
        kinds.register(
            "GasThermomachineBoundUserInterface",
            |commands, content, ui, _, style| {
                let Some(state) = ui.state::<ThermoMachineUiState>() else {
                    spawn_label(commands, content, style, "Нет данных");
                    return;
                };
                let mut lines = vec![
                    if state.heater {
                        "Нагреватель".to_owned()
                    } else {
                        "Охладитель".to_owned()
                    },
                    format!(
                        "Работа: {}",
                        if state.enabled {
                            "включён"
                        } else {
                            "выключен"
                        }
                    ),
                    format!(
                        "Цель: {:.1} K ({:.1} °C)",
                        state.target_temperature,
                        state.target_temperature - 273.15
                    ),
                ];
                if let Some(t) = state.inlet_temperature {
                    lines.push(format!("Сейчас: {t:.1} K"));
                }
                for line in lines {
                    spawn_label(commands, content, style, &line);
                }
                let entity = ui.entity;
                for (text, step) in [("−10 K", -10.0f32), ("+10 K", 10.0)] {
                    let temperature = (state.target_temperature + step)
                        .clamp(state.min_temperature, state.max_temperature);
                    spawn_button(
                        commands,
                        content,
                        style,
                        text,
                        UiAction::new(move |world| {
                            world.send_ui_message(
                                entity,
                                THERMOMACHINE_UI_KEY,
                                &GasThermomachineChangeTemperature { temperature },
                            );
                        }),
                    );
                }
                spawn_button(
                    commands,
                    content,
                    style,
                    "Вкл/выкл",
                    UiAction::new(move |world| {
                        world.send_ui_message(
                            entity,
                            THERMOMACHINE_UI_KEY,
                            &GasThermomachineToggle,
                        );
                    }),
                );
            },
        );
        kinds.register(
            "GasAnalyzerBoundUserInterface",
            |commands, content, ui, _, style| {
                let Some(state) = ui.state::<GasAnalyzerState>() else {
                    spawn_label(commands, content, style, "Нет данных");
                    return;
                };
                if !state.device_name.is_empty() {
                    spawn_label(
                        commands,
                        content,
                        style,
                        &format!("Устройство: {}", state.device_name),
                    );
                }
                for mix in &state.mixes {
                    spawn_label(commands, content, style, &format!("— {} —", mix.name));
                    spawn_label(
                        commands,
                        content,
                        style,
                        &format!(
                            "Давление {:.2} кПа, температура {:.1} K ({:.1} °C), объём {:.0} л",
                            mix.pressure,
                            mix.temperature,
                            mix.temperature - 273.15,
                            mix.volume
                        ),
                    );
                    let total: f32 = mix.gases.iter().map(|g| g.amount).sum();
                    for gas in &mix.gases {
                        let share = if total > 0.0 {
                            gas.amount / total * 100.0
                        } else {
                            0.0
                        };
                        spawn_label(
                            commands,
                            content,
                            style,
                            &format!(
                                "{}: {:.2} моль ({share:.1} %)",
                                gas_name(&gas.gas),
                                gas.amount
                            ),
                        );
                    }
                }
            },
        );
        kinds.register(
            "AirAlarmBoundUserInterface",
            |commands, content, ui, _, style| {
                let Some(state) = ui.state::<AirAlarmUiState>() else {
                    spawn_label(commands, content, style, "Нет данных");
                    return;
                };
                let alarm = match state.alarm_type {
                    AtmosAlarmType::Danger => "опасность",
                    AtmosAlarmType::Warning => "предупреждение",
                    AtmosAlarmType::Emagged => "взломана",
                    _ => "норма",
                };
                let mode = |m: AirAlarmMode| match m {
                    AirAlarmMode::None => "выключено",
                    AirAlarmMode::Filtering => "фильтрация",
                    AirAlarmMode::WideFiltering => "широкая фильтрация",
                    AirAlarmMode::Fill => "наполнение",
                    AirAlarmMode::Panic => "паника",
                };
                for line in [
                    format!("Адрес: {}", state.address),
                    format!("Состояние: {alarm}"),
                    format!(
                        "Режим: {}{}",
                        mode(state.mode),
                        if state.auto_mode { " (авто)" } else { "" }
                    ),
                    format!(
                        "Устройства: вентиляций {}, скрубберов {}, датчиков {}",
                        state.vents.len(),
                        state.scrubbers.len(),
                        state.sensors.len()
                    ),
                ] {
                    spawn_label(commands, content, style, &line);
                }
                for (address, sensor) in &state.sensors {
                    spawn_label(
                        commands,
                        content,
                        style,
                        &format!(
                            "Датчик {address}: {:.1} кПа, {:.1} K",
                            sensor.pressure, sensor.temperature
                        ),
                    );
                }
                let entity = ui.entity;
                for m in [
                    AirAlarmMode::Filtering,
                    AirAlarmMode::WideFiltering,
                    AirAlarmMode::Fill,
                    AirAlarmMode::Panic,
                    AirAlarmMode::None,
                ] {
                    spawn_button(
                        commands,
                        content,
                        style,
                        &format!("Режим: {}", mode(m)),
                        UiAction::new(move |world| {
                            world.send_ui_message(
                                entity,
                                AIR_ALARM_UI_KEY,
                                &AirAlarmUpdateAlarmMode { mode: m },
                            );
                        }),
                    );
                }
                let enabled = !state.auto_mode;
                spawn_button(
                    commands,
                    content,
                    style,
                    if enabled {
                        "Включить авторежим"
                    } else {
                        "Выключить авторежим"
                    },
                    UiAction::new(move |world| {
                        world.send_ui_message(
                            entity,
                            AIR_ALARM_UI_KEY,
                            &AirAlarmUpdateAutoMode { enabled },
                        );
                    }),
                );
                spawn_button(
                    commands,
                    content,
                    style,
                    "Опросить устройства",
                    UiAction::new(move |world| {
                        world.send_ui_message(entity, AIR_ALARM_UI_KEY, &AirAlarmResyncAllDevices);
                    }),
                );
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
