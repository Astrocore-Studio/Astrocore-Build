//! Админ-меню и окно журнала на клиенте. F7 открывает меню активного администратора:
//! вкладка «Админ» — кнопки команд, «Раунд» — старт, конец и перезапуск раунда, «Игроки» —
//! список игроков с действиями над выбранным (телепорт к нему и к себе, кик, ahelp).
//! Кнопки команд видны, только если команда разрешена. Окно журнала открывает команда
//! `adminlogs`: раунд, поиск по тексту, отбор по важности, следующая пачка.
//! Образец — `AdminUIController`, `AdminMenuWindow`, `AdminTab`, `RoundTab`, `PlayerTab`,
//! `CommandButton`, `AdminLogsEui`, `AdminLogsControl`, `AdminLogLabel` клиента SS14 @ 2f4a2aad.
//!
//! Нет пока: вкладок «Админбас», «Атмос», «Сервер», «Объекты», окна банов, меню глаголов
//! по правому щелчку, отбора журнала по видам и игрокам в окне, выгрузки журнала.

use astrocore_client::{Game, KeyboardFocus, UiAction, UiStyle};
use astrocore_loc::LocArg;
use astrocore_math::Color as GameColor;
use astrocore_net::{NetEventWorldExt, NetUserId, RemoteConsoleWorldExt};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy_ecs::world::World as GameWorld;

use crate::admin_logs::LogImpact;
use crate::admin_logs_eui::{AdminLogsMessage, AdminLogsWindow, IMPACTS, close_logs, request_logs};
use crate::admin_manager::ClientAdmin;
use crate::admin_system::{ClientPlayerList, PlayerInfo};
use crate::ahelp_ui::AhelpWindow;
use crate::chat_ui::{markup_spans, spans_text};
use crate::examine::loc;

const PANEL: Color = Color::srgba(0.07, 0.08, 0.11, 0.96);
const ROW: Color = Color::srgba(0.16, 0.18, 0.24, 1.0);
const SELECTED: Color = Color::srgba(0.30, 0.34, 0.50, 1.0);
const OFF: Color = Color::srgba(0.10, 0.11, 0.14, 1.0);
/// Записей журнала на странице окна.
pub const LOGS_PER_PAGE: usize = 22;

/// Вкладка меню.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AdminMenuTab {
    #[default]
    Admin,
    Round,
    Players,
}

/// Состояние админ-меню. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default)]
pub struct AdminMenu {
    pub open: bool,
    pub tab: AdminMenuTab,
    pub selected: Option<NetUserId>,
    pub show_disconnected: bool,
    pub version: u64,
}

/// Может ли открыть меню (`CanAdminMenu`): активный администратор.
pub fn can_admin_menu(world: &GameWorld) -> bool {
    world
        .get_resource::<ClientAdmin>()
        .and_then(|a| a.admin.as_ref())
        .is_some_and(|a| a.active)
}

/// Разрешена ли команда (`CommandButton`: видна, только если можно выполнить).
pub fn can_command(world: &GameWorld, command: &str) -> bool {
    world
        .get_resource::<ClientAdmin>()
        .is_some_and(|a| a.available_commands.iter().any(|c| c == command))
}

/// Открыть или закрыть меню.
pub fn toggle_menu(world: &mut GameWorld) {
    if !can_admin_menu(world) {
        return;
    }
    let mut menu = world.get_resource_or_insert_with(AdminMenu::default);
    menu.open = !menu.open;
    menu.version += 1;
}

fn own_username(world: &GameWorld) -> Option<String> {
    let user = world.get_resource::<ClientAdmin>()?.user_id?;
    Some(
        world
            .get_resource::<ClientPlayerList>()?
            .players
            .get(&user)?
            .username
            .clone(),
    )
}

/// Команды действий над игроком (глаголы администратора SS14): что выполнить в консоли.
pub fn player_action(
    world: &GameWorld,
    action: PlayerAction,
    player: &PlayerInfo,
) -> Option<String> {
    match action {
        PlayerAction::TeleportTo => Some(format!("tpto {}", player.username)),
        PlayerAction::TeleportHere => {
            Some(format!("tpto {} {}", own_username(world)?, player.username))
        }
        PlayerAction::Kick => Some(format!("kick {}", player.username)),
        PlayerAction::Ahelp => None,
    }
}

/// Действие над выбранным игроком.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerAction {
    TeleportTo,
    TeleportHere,
    Kick,
    Ahelp,
}

impl PlayerAction {
    fn label(self) -> &'static str {
        match self {
            Self::TeleportTo => "admin-verbs-teleport-to",
            Self::TeleportHere => "admin-verbs-teleport-here",
            Self::Kick => "admin-player-actions-kick",
            Self::Ahelp => "admin-player-actions-ahelp",
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::TeleportTo | Self::TeleportHere => "tpto",
            Self::Kick => "kick",
            Self::Ahelp => "",
        }
    }
}

/// Выполнить действие над игроком.
pub fn run_player_action(world: &mut GameWorld, action: PlayerAction, user: NetUserId) {
    let Some(player) = world
        .get_resource::<ClientPlayerList>()
        .and_then(|l| l.players.get(&user).cloned())
    else {
        return;
    };
    if action == PlayerAction::Ahelp {
        let mut window = world.get_resource_or_insert_with(AhelpWindow::default);
        window.open = true;
        window.selected = Some(user);
        window.version += 1;
        return;
    }
    if let Some(command) = player_action(world, action, &player) {
        world.execute_remote_command(&command);
    }
}

/// Время записи: часы, минуты, секунды UTC (`AdminLogLabel`).
pub fn log_time(date: i64) -> String {
    let seconds = date.div_euclid(1000).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn type_into(
    mut events: MessageReader<KeyboardInput>,
    mut focus: ResMut<KeyboardFocus>,
    mut game: ResMut<Game>,
) {
    let world = &mut game.0;
    if world.get_resource::<ClientAdmin>().is_none() {
        return;
    }
    let was_focused = world
        .get_resource::<AdminLogsWindow>()
        .is_some_and(|w| w.open && w.search_focused);
    for event in events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }
        if event.key_code == KeyCode::F7 && !focus.0 {
            toggle_menu(world);
            continue;
        }
        let focused = world
            .get_resource::<AdminLogsWindow>()
            .is_some_and(|w| w.open && w.search_focused);
        if !focused {
            continue;
        }
        let mut window = world.resource_mut::<AdminLogsWindow>();
        window.version += 1;
        match &event.logical_key {
            Key::Escape => window.search_focused = false,
            Key::Enter => {
                window.search_focused = false;
                request_logs(world);
            }
            Key::Backspace => {
                window.search.pop();
            }
            Key::Space => window.search.push(' '),
            Key::Character(text) => window
                .search
                .extend(text.chars().filter(|c| !c.is_control())),
            _ => {}
        }
    }
    let focused = world
        .get_resource::<AdminLogsWindow>()
        .is_some_and(|w| w.open && w.search_focused);
    if focused != was_focused {
        focus.0 = focused;
    }
}

#[derive(Component)]
struct AdminPanel;

fn button(
    commands: &mut Commands,
    style: &UiStyle,
    parent: Entity,
    text: String,
    color: Color,
    action: UiAction,
) {
    let id = commands
        .spawn((
            ChildOf(parent),
            Node {
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(color),
            Interaction::default(),
            action,
        ))
        .id();
    commands.spawn((ChildOf(id), style.text(text, 13.0)));
}

fn row(commands: &mut Commands, parent: Entity) -> Entity {
    commands
        .spawn((
            ChildOf(parent),
            Node {
                column_gap: Val::Px(6.0),
                row_gap: Val::Px(4.0),
                flex_wrap: FlexWrap::Wrap,
                ..default()
            },
        ))
        .id()
}

fn panel(commands: &mut Commands, left: f32, top: f32, width: f32, height: f32) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(left),
                top: Val::Px(top),
                width: Val::Px(width),
                height: Val::Px(height),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                padding: UiRect::all(Val::Px(10.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(PANEL),
            GlobalZIndex(72),
            Interaction::default(),
            AdminPanel,
        ))
        .id()
}

fn command_button(
    commands: &mut Commands,
    style: &UiStyle,
    world: &GameWorld,
    parent: Entity,
    command: &'static str,
    label: &str,
) {
    if !can_command(world, command) {
        return;
    }
    button(
        commands,
        style,
        parent,
        loc(world, label, &[]),
        ROW,
        UiAction::new(move |world| world.execute_remote_command(command)),
    );
}

fn draw_menu(commands: &mut Commands, style: &UiStyle, world: &GameWorld, menu: &AdminMenu) {
    let root = panel(commands, 40.0, 60.0, 560.0, 420.0);
    let header = row(commands, root);
    commands.spawn((
        ChildOf(header),
        style.text(loc(world, "admin-menu-title", &[]), 16.0),
    ));
    let tabs = row(commands, root);
    for (tab, key) in [
        (AdminMenuTab::Admin, "admin-menu-admin-tab"),
        (AdminMenuTab::Round, "admin-menu-round-tab"),
        (AdminMenuTab::Players, "admin-menu-players-tab"),
    ] {
        button(
            commands,
            style,
            tabs,
            loc(world, key, &[]),
            if menu.tab == tab { SELECTED } else { ROW },
            UiAction::new(move |world| {
                let mut menu = world.get_resource_or_insert_with(AdminMenu::default);
                menu.tab = tab;
                menu.version += 1;
            }),
        );
    }
    match menu.tab {
        AdminMenuTab::Admin => {
            let grid = row(commands, root);
            command_button(
                commands,
                style,
                world,
                grid,
                "aghost",
                "admin-player-actions-window-admin-ghost",
            );
            command_button(
                commands,
                style,
                world,
                grid,
                "adminlogs",
                "admin-player-actions-window-admin-logs",
            );
        }
        AdminMenuTab::Round => {
            let grid = row(commands, root);
            command_button(
                commands,
                style,
                world,
                grid,
                "startround",
                "administration-ui-round-tab-start-round",
            );
            command_button(
                commands,
                style,
                world,
                grid,
                "endround",
                "administration-ui-round-tab-end-round",
            );
            command_button(
                commands,
                style,
                world,
                grid,
                "restartround",
                "administration-ui-round-tab-restart-round",
            );
            command_button(
                commands,
                style,
                world,
                grid,
                "restartroundnow",
                "administration-ui-round-tab-restart-round-now",
            );
        }
        AdminMenuTab::Players => draw_players(commands, style, world, menu, root),
    }
}

fn draw_players(
    commands: &mut Commands,
    style: &UiStyle,
    world: &GameWorld,
    menu: &AdminMenu,
    root: Entity,
) {
    let players: Vec<PlayerInfo> = world
        .get_resource::<ClientPlayerList>()
        .map(|l| {
            l.players
                .values()
                .filter(|p| p.connected || menu.show_disconnected)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let top = row(commands, root);
    commands.spawn((
        ChildOf(top),
        style.text(
            loc(
                world,
                "player-tab-player-count",
                &[("count", LocArg::from(players.len() as i64))],
            ),
            13.0,
        ),
    ));
    button(
        commands,
        style,
        top,
        loc(world, "player-tab-show-disconnected", &[]),
        if menu.show_disconnected {
            SELECTED
        } else {
            ROW
        },
        UiAction::new(|world| {
            let mut menu = world.get_resource_or_insert_with(AdminMenu::default);
            menu.show_disconnected = !menu.show_disconnected;
            menu.version += 1;
        }),
    );
    let table = commands
        .spawn((
            ChildOf(root),
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                flex_grow: 1.0,
                overflow: Overflow::clip(),
                ..default()
            },
        ))
        .id();
    let header = [
        loc(world, "player-tab-username", &[]),
        loc(world, "player-tab-character", &[]),
        loc(world, "player-tab-job", &[]),
    ];
    table_row(commands, style, table, &header, OFF, None);
    for player in &players {
        let user = player.user_id;
        let cells = [
            player.username.clone(),
            player.character_name.clone(),
            player.starting_job.clone(),
        ];
        let color = if menu.selected == Some(user) {
            SELECTED
        } else if player.connected {
            ROW
        } else {
            OFF
        };
        table_row(
            commands,
            style,
            table,
            &cells,
            color,
            Some(UiAction::new(move |world| {
                let mut menu = world.get_resource_or_insert_with(AdminMenu::default);
                menu.selected = Some(user);
                menu.version += 1;
            })),
        );
    }
    let Some(selected) = menu
        .selected
        .and_then(|user| players.iter().find(|p| p.user_id == user))
    else {
        return;
    };
    let actions = row(commands, root);
    for action in [
        PlayerAction::TeleportTo,
        PlayerAction::TeleportHere,
        PlayerAction::Kick,
        PlayerAction::Ahelp,
    ] {
        let allowed = match action {
            PlayerAction::Ahelp => true,
            _ => selected.connected && can_command(world, action.command()),
        };
        if !allowed {
            continue;
        }
        let user = selected.user_id;
        button(
            commands,
            style,
            actions,
            loc(world, action.label(), &[]),
            ROW,
            UiAction::new(move |world| run_player_action(world, action, user)),
        );
    }
}

fn table_row(
    commands: &mut Commands,
    style: &UiStyle,
    table: Entity,
    cells: &[String],
    color: Color,
    action: Option<UiAction>,
) {
    let mut entity = commands.spawn((
        ChildOf(table),
        Node {
            padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
            ..default()
        },
        BackgroundColor(color),
    ));
    if let Some(action) = action {
        entity.insert((Interaction::default(), action));
    }
    let id = entity.id();
    for cell in cells {
        let cell_node = commands
            .spawn((
                ChildOf(id),
                Node {
                    width: Val::Px(170.0),
                    overflow: Overflow::clip(),
                    ..default()
                },
            ))
            .id();
        commands.spawn((ChildOf(cell_node), style.text(cell.clone(), 13.0)));
    }
}

fn impact_color(impact: LogImpact) -> GameColor {
    match impact {
        LogImpact::Low => GameColor::parse("lightgray"),
        LogImpact::Medium => GameColor::parse("white"),
        LogImpact::High => GameColor::parse("orange"),
        LogImpact::Extreme => GameColor::parse("red"),
    }
    .unwrap_or(GameColor::WHITE)
}

fn draw_logs(
    commands: &mut Commands,
    style: &UiStyle,
    world: &GameWorld,
    window: &AdminLogsWindow,
) {
    let root = panel(commands, 120.0, 40.0, 1000.0, 600.0);
    let header = row(commands, root);
    commands.spawn((
        ChildOf(header),
        style.text(loc(world, "admin-logs-title", &[]), 16.0),
    ));
    button(
        commands,
        style,
        header,
        "X".into(),
        ROW,
        UiAction::new(close_logs),
    );
    let controls = row(commands, root);
    let step = |delta: i64| {
        UiAction::new(move |world| {
            let mut window = world.get_resource_or_insert_with(AdminLogsWindow::default);
            window.round = (window.round + delta).max(0);
            window.version += 1;
            request_logs(world);
        })
    };
    button(commands, style, controls, "−".into(), ROW, step(-1));
    commands.spawn((
        ChildOf(controls),
        style.text(
            format!("{}#{}", loc(world, "admin-logs-round", &[]), window.round),
            14.0,
        ),
    ));
    button(commands, style, controls, "+".into(), ROW, step(1));
    button(
        commands,
        style,
        controls,
        loc(
            world,
            "admin-logs-reset-with-id",
            &[("id", LocArg::from(window.current_round))],
        ),
        ROW,
        UiAction::new(|world| {
            let mut window = world.get_resource_or_insert_with(AdminLogsWindow::default);
            window.round = window.current_round;
            window.version += 1;
            request_logs(world);
        }),
    );
    let search = if window.search.is_empty() && !window.search_focused {
        loc(world, "admin-logs-search-logs-placeholder", &[])
    } else if window.search_focused {
        format!("{}_", window.search)
    } else {
        window.search.clone()
    };
    button(
        commands,
        style,
        controls,
        search,
        if window.search_focused { SELECTED } else { OFF },
        UiAction::new(|world| {
            let mut window = world.get_resource_or_insert_with(AdminLogsWindow::default);
            window.search_focused = true;
            window.version += 1;
        }),
    );
    button(
        commands,
        style,
        controls,
        loc(world, "admin-logs-refresh", &[]),
        ROW,
        UiAction::new(request_logs),
    );
    for impact in IMPACTS {
        let on = window.impacts.contains(&impact);
        button(
            commands,
            style,
            controls,
            format!("{impact:?}"),
            if on { SELECTED } else { OFF },
            UiAction::new(move |world| {
                let mut window = world.get_resource_or_insert_with(AdminLogsWindow::default);
                if !window.impacts.remove(&impact) {
                    window.impacts.insert(impact);
                }
                window.page = 0;
                window.version += 1;
            }),
        );
    }
    let visible = window.visible();
    let pages = visible.len().div_ceil(LOGS_PER_PAGE).max(1);
    let page = window.page.min(pages - 1);
    let status = row(commands, root);
    commands.spawn((
        ChildOf(status),
        style.text(
            loc(
                world,
                "admin-logs-count",
                &[
                    ("showing", LocArg::from(visible.len() as i64)),
                    ("total", LocArg::from(window.logs.len() as i64)),
                    ("round", LocArg::from(window.round_logs)),
                ],
            ),
            13.0,
        ),
    ));
    let turn = |delta: isize| {
        UiAction::new(move |world| {
            let mut window = world.get_resource_or_insert_with(AdminLogsWindow::default);
            window.page = window.page.saturating_add_signed(delta);
            window.version += 1;
        })
    };
    if page > 0 {
        button(commands, style, status, "<".into(), ROW, turn(-1));
    }
    commands.spawn((
        ChildOf(status),
        style.text(format!("{}/{pages}", page + 1), 13.0),
    ));
    if page + 1 < pages {
        button(commands, style, status, ">".into(), ROW, turn(1));
    }
    if window.has_next {
        button(
            commands,
            style,
            status,
            loc(world, "admin-logs-next", &[]),
            ROW,
            UiAction::new(|world| world.send_net_event_to_server(&AdminLogsMessage::Next)),
        );
    }
    let list = commands
        .spawn((
            ChildOf(root),
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(1.0),
                flex_grow: 1.0,
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.3)),
        ))
        .id();
    for log in visible
        .iter()
        .skip(page * LOGS_PER_PAGE)
        .take(LOGS_PER_PAGE)
    {
        let text = format!("{}: {}", log_time(log.date), log.message);
        let spans = markup_spans(&text, impact_color(log.impact));
        spans_text(commands, style, list, &spans, 13.0);
    }
}

fn draw(
    mut commands: Commands,
    mut game: ResMut<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<AdminPanel>>,
    mut shown: Local<Option<(u64, u64, u64, u64)>>,
) {
    let world = &mut game.0;
    if world.get_resource::<ClientAdmin>().is_none() {
        return;
    }
    let menu = world
        .get_resource_or_insert_with(AdminMenu::default)
        .clone();
    let logs = world
        .get_resource_or_insert_with(AdminLogsWindow::default)
        .clone();
    let admin_version = world.resource::<ClientAdmin>().version;
    let players_version = world
        .get_resource::<ClientPlayerList>()
        .map_or(0, |l| l.version);
    let now = (menu.version, logs.version, admin_version, players_version);
    if shown.as_ref() == Some(&now) {
        return;
    }
    *shown = Some(now);
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    // Снятые права закрывают меню.
    if menu.open && can_admin_menu(world) {
        draw_menu(&mut commands, &style, world, &menu);
    }
    if logs.open {
        draw_logs(&mut commands, &style, world, &logs);
    }
}

/// Админ-меню и окно журнала.
pub struct AdminUiPlugin;

impl Plugin for AdminUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, type_into.after(bevy::input::InputSystems));
        app.add_systems(Update, draw);
    }
}

#[cfg(test)]
mod tests;
