//! Экран лобби на клиенте: сведения о сервере и раунде, отсчёт до старта, готовность,
//! персонаж с кнопкой настройки, вход в идущий раунд с выбором должности станции и
//! наблюдение; окно итогов раунда. Образец — `LobbyGui`, `LobbyUIController`,
//! `LateJoinGui` и `RoundEndSummaryWindow` клиента SS14 @ 2f4a2aad.
//!
//! Нет пока: фона и музыки лобби, списка игроков, куклы персонажа, кнопки «Выйти» (ждёт
//! меню выхода), вкладки манифеста игроков с поиском.

use std::time::Duration;

use astrocore_client::{Game, UiAction, UiStyle};
use astrocore_ecs::GameTime;
use astrocore_loc::LocArg;
use astrocore_net::NetEventWorldExt;
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy_ecs::world::World as GameWorld;

use crate::character_editor::departments;
use crate::character_ui::{Ui, text_or};
use crate::examine::{loc, strip_markup};
use crate::game_ticker::{ClientLobby, LobbyRequest, RoundEndMessage};
use crate::preferences::ClientPreferences;
use crate::stations::{ClientJobsAvailable, StationJobsInfo};

const BACKDROP: Color = Color::srgb(0.04, 0.05, 0.07);
const PANEL: Color = Color::srgba(0.08, 0.09, 0.12, 0.97);

/// Окна лобби. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default)]
pub struct LobbyWindows {
    pub late_join_open: bool,
    pub version: u64,
}

fn lobby_windows(world: &mut GameWorld) -> Mut<'_, LobbyWindows> {
    world.get_resource_or_insert_with(LobbyWindows::default)
}

fn client_now(world: &GameWorld) -> Duration {
    world
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default()
}

/// Время `мм:сс`.
pub fn clock(seconds: f32) -> String {
    let total = seconds.max(0.0).ceil() as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// Строка состояния: отсчёт, пауза, скоро или время раунда (`UpdateLobbyUi`).
pub fn status_line(world: &GameWorld, lobby: &ClientLobby) -> String {
    let now = client_now(world);
    if lobby.round_started {
        let seconds = lobby.round_time(now) as i64;
        return loc(
            world,
            "lobby-state-player-status-round-time",
            &[
                ("hours", LocArg::from(seconds / 3600)),
                ("minutes", LocArg::from(seconds % 3600 / 60)),
            ],
        );
    }
    if lobby.paused {
        return text_or(world, "lobby-state-paused", "Пауза");
    }
    match lobby.seconds_left(now) {
        Some(left) if left > 0.0 => loc(
            world,
            "lobby-state-round-start-countdown-text",
            &[("timeLeft", LocArg::from(clock(left).as_str()))],
        ),
        Some(_) => text_or(world, "lobby-state-soon", "Раунд скоро начнётся"),
        None => text_or(
            world,
            "lobby-state-player-status-round-not-started",
            "Раунд ещё не начался",
        ),
    }
}

/// Должности станции для окна позднего входа: по отделам, только со свободными слотами.
pub fn late_join_rows(
    world: &GameWorld,
    station: &StationJobsInfo,
) -> Vec<(String, Vec<(String, String)>)> {
    let Some(prototypes) = world.get_resource::<astrocore_prototypes::Prototypes>() else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for department in departments(prototypes) {
        let jobs: Vec<(String, String)> = department
            .jobs
            .iter()
            .filter_map(|job| {
                let slots = station.jobs.get(&job.id)?;
                if *slots == Some(0) {
                    return None;
                }
                let name = text_or(world, &job.name, &job.id);
                let label = match slots {
                    Some(amount) => loc(
                        world,
                        "late-join-gui-job-slot-capped",
                        &[
                            ("jobName", LocArg::from(name.as_str())),
                            ("amount", LocArg::from(*amount)),
                        ],
                    ),
                    None => loc(
                        world,
                        "late-join-gui-job-slot-uncapped",
                        &[("jobName", LocArg::from(name.as_str()))],
                    ),
                };
                Some((job.id.clone(), label))
            })
            .collect();
        if jobs.is_empty() {
            continue;
        }
        let name = loc(world, &department.name, &[]);
        let title = loc(
            world,
            "late-join-gui-department-jobs-label",
            &[("departmentName", LocArg::from(name.as_str()))],
        );
        rows.push((title, jobs));
    }
    rows
}

/// Строки итогов (`RoundEndSummaryWindow`): номер, режим, длительность, игроки.
pub fn round_end_lines(world: &GameWorld, end: &RoundEndMessage) -> Vec<String> {
    let seconds = end.round_duration.max(0.0) as i64;
    let mut lines = vec![
        loc(
            world,
            "round-end-summary-window-round-id-label",
            &[("roundId", LocArg::from(end.round_id))],
        ),
        loc(
            world,
            "round-end-summary-window-gamemode-name-label",
            &[("gamemode", LocArg::from(end.gamemode_title.as_str()))],
        ),
        loc(
            world,
            "round-end-summary-window-duration-label",
            &[
                ("hours", LocArg::from(seconds / 3600)),
                ("minutes", LocArg::from(seconds % 3600 / 60)),
                ("seconds", LocArg::from(seconds % 60)),
            ],
        ),
    ];
    lines.extend(
        end.round_end_text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned),
    );
    for player in &end.players {
        let ic = player.ic_name.clone().unwrap_or_default();
        let line = if player.observer {
            loc(
                world,
                "round-end-summary-window-player-info-if-observer-text",
                &[
                    ("playerOOCName", LocArg::from(player.ooc_name.as_str())),
                    ("playerICName", LocArg::from(ic.as_str())),
                ],
            )
        } else {
            loc(
                world,
                "round-end-summary-window-player-info-if-not-observer-text",
                &[
                    ("playerOOCName", LocArg::from(player.ooc_name.as_str())),
                    ("icNameColor", LocArg::from("white")),
                    ("playerICName", LocArg::from(ic.as_str())),
                    ("playerRole", LocArg::from(player.role.as_str())),
                ],
            )
        };
        lines.push(line);
    }
    lines.into_iter().map(|l| strip_markup(&l)).collect()
}

#[derive(Component)]
struct LobbyPanel;

/// Подпись выбранного персонажа.
fn character_line(world: &GameWorld) -> Option<String> {
    let prefs = world
        .get_resource::<ClientPreferences>()?
        .preferences
        .as_ref()?;
    let profile = prefs.selected_character()?;
    Some(crate::profiles::summary(world, profile))
}

fn window(
    mut commands: Commands,
    mut game: ResMut<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<LobbyPanel>>,
    mut shown: Local<Option<String>>,
) {
    let world = &mut game.0;
    let Some(lobby) = world.get_resource::<ClientLobby>().cloned() else {
        return;
    };
    let windows = lobby_windows(world).clone();
    let status = status_line(world, &lobby);
    let jobs = world
        .get_resource::<ClientJobsAvailable>()
        .map(|j| format!("{:?}", j.0))
        .unwrap_or_default();
    let character = character_line(world).unwrap_or_default();
    let signature = format!(
        "{}|{}|{status}|{character}|{}",
        lobby.version,
        windows.version,
        jobs.len()
    );
    if shown.as_deref() == Some(signature.as_str()) {
        return;
    }
    *shown = Some(signature);
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    let mut ui = Ui {
        commands: &mut commands,
        style: &style,
    };
    if lobby.in_lobby {
        lobby_screen(world, &mut ui, &lobby, &windows, &status, &character);
    }
    if lobby.show_round_end
        && let Some(end) = &lobby.round_end
    {
        round_end_window(world, &mut ui, end);
    }
}

fn panel(ui: &mut Ui, node: Node, background: Color, z: i32) -> Entity {
    ui.commands
        .spawn((
            node,
            BackgroundColor(background),
            Interaction::default(),
            GlobalZIndex(z),
            LobbyPanel,
        ))
        .id()
}

fn lobby_screen(
    world: &mut GameWorld,
    ui: &mut Ui,
    lobby: &ClientLobby,
    windows: &LobbyWindows,
    status: &str,
    character: &str,
) {
    let screen = panel(
        ui,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            padding: UiRect::all(Val::Px(24.0)),
            column_gap: Val::Px(24.0),
            ..default()
        },
        BACKDROP,
        40,
    );
    let info = ui.node(
        screen,
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            width: Val::Px(400.0),
            padding: UiRect::all(Val::Px(12.0)),
            ..default()
        },
    );
    ui.commands.entity(info).insert(BackgroundColor(PANEL));
    let title = loc(
        world,
        "ui-lobby-title",
        &[("serverName", LocArg::from("Astrocore"))],
    );
    ui.label(info, title, 18.0);
    ui.label(
        info,
        text_or(world, "ui-lobby-server-info-block", "Сервер"),
        14.0,
    );
    for line in strip_markup(&lobby.info).lines() {
        ui.label(info, line.trim_start(), 13.0);
    }

    let side = ui.node(
        screen,
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            width: Val::Px(780.0),
            padding: UiRect::all(Val::Px(12.0)),
            ..default()
        },
    );
    ui.commands.entity(side).insert(BackgroundColor(PANEL));
    if !character.is_empty() {
        ui.label(side, character, 14.0);
    }
    ui.chip(
        side,
        text_or(
            world,
            "character-setup-gui-character-setup-label",
            "Настройка персонажа",
        ),
        false,
        UiAction::new(crate::character_ui::toggle),
    );
    ui.label(side, status, 15.0);
    let buttons = ui.row(side);
    if lobby.round_started {
        if !lobby.late_join_disallowed {
            ui.chip(
                buttons,
                text_or(world, "lobby-state-ready-button-join-state", "Войти"),
                windows.late_join_open,
                UiAction::new(|world| {
                    let mut windows = lobby_windows(world);
                    windows.late_join_open = !windows.late_join_open;
                    windows.version += 1;
                }),
            );
        }
        ui.chip(
            buttons,
            text_or(world, "ui-lobby-observe-button", "Наблюдать"),
            false,
            UiAction::new(|world| world.send_net_event_to_server(&LobbyRequest::Observe)),
        );
    } else {
        let ready = lobby.ready;
        ui.chip(
            buttons,
            text_or(world, "ui-lobby-ready-up-button", "Готов"),
            ready,
            UiAction::new(move |world| {
                world.send_net_event_to_server(&LobbyRequest::ToggleReady(!ready));
            }),
        );
        let state = if ready {
            text_or(world, "lobby-state-player-status-ready", "Готов")
        } else {
            text_or(world, "lobby-state-player-status-not-ready", "Не готов")
        };
        ui.label(buttons, state, 13.0);
    }
    if lobby.round_started && windows.late_join_open {
        late_join_window(world, ui, side);
    }
}

fn late_join_window(world: &mut GameWorld, ui: &mut Ui, parent: Entity) {
    let column = ui.column(parent, None);
    ui.label(
        column,
        text_or(world, "late-join-gui-title", "Поздний вход"),
        15.0,
    );
    let stations = world
        .get_resource::<ClientJobsAvailable>()
        .map(|j| j.0.stations.clone())
        .unwrap_or_default();
    for station in stations {
        ui.label(column, station.name.clone(), 14.0);
        for (department, jobs) in late_join_rows(world, &station) {
            ui.label(column, department, 12.0);
            let row = ui.row(column);
            for (job, label) in jobs {
                let bits = station.station;
                ui.chip(
                    row,
                    label,
                    false,
                    UiAction::new(move |world| {
                        world.send_net_event_to_server(&LobbyRequest::JoinGame {
                            station: Some(bits),
                            job: Some(job.clone()),
                        });
                        let mut windows = lobby_windows(world);
                        windows.late_join_open = false;
                        windows.version += 1;
                    }),
                );
            }
        }
    }
}

fn round_end_window(world: &mut GameWorld, ui: &mut Ui, end: &RoundEndMessage) {
    let window = panel(
        ui,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(240.0),
            top: Val::Px(80.0),
            width: Val::Px(800.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(4.0),
            padding: UiRect::all(Val::Px(12.0)),
            ..default()
        },
        PANEL,
        70,
    );
    ui.label(
        window,
        text_or(world, "round-end-summary-window-title", "Итоги раунда"),
        17.0,
    );
    for line in round_end_lines(world, end) {
        ui.label(window, line, 13.0);
    }
    ui.chip(
        window,
        text_or(
            world,
            "character-setup-gui-character-setup-close-button",
            "Закрыть",
        ),
        false,
        UiAction::new(|world| {
            let mut lobby = world.get_resource_or_insert_with(ClientLobby::default);
            lobby.show_round_end = false;
            lobby.version += 1;
        }),
    );
}

/// Экран лобби и итоги раунда.
pub struct LobbyUiPlugin;

impl Plugin for LobbyUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, window);
    }
}

#[cfg(test)]
mod tests;
