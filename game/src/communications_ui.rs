//! Окна консолей на клиенте. Консоль связи: поле объявления со счётчиком символов, кнопка
//! «объявить по радио», вызов и отзыв шаттла эвакуации с отсчётом «мм:сс» до прибытия.
//! Консоль шаттла эвакуации: отсчёт до досрочного запуска, «разрешить», «отозвать»,
//! «отозвать все», сколько разрешений осталось и чьи уже есть. Образец —
//! `CommunicationsConsoleMenu`, `MessagingControls`, `ShuttleControls`,
//! `EmergencyConsoleWindow` клиента SS14 @ 2f4a2aad.
//!
//! Нет пока: выбора уровня тревоги и вкладки экранов (уровней тревоги и экранов нет).

use std::time::Duration;

use astrocore_client::{
    Game, KeyboardFocus, UiAction, UiStyle, WindowKinds, spawn_button, spawn_label,
};
use astrocore_console::ConfigWorldExt;
use astrocore_ecs::GameTime;
use astrocore_loc::LocArg;
use astrocore_ui::{OpenUi, OpenUis, UiWorldExt};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy_ecs::world::World as GameWorld;

use crate::communications::{
    COMMS_UI_KEY, CommunicationsConsoleMessage, CommunicationsConsoleState,
};
use crate::emergency_shuttle::{
    EMERGENCY_CONSOLE_UI_KEY, EmergencyConsoleMessage, EmergencyConsoleState,
};
use crate::examine::loc;
use crate::lobby_ui::clock;

const FIELD: Color = Color::srgba(0.05, 0.06, 0.08, 1.0);
const FIELD_FOCUSED: Color = Color::srgba(0.10, 0.12, 0.18, 1.0);
const DISABLED: Color = Color::srgba(0.12, 0.12, 0.14, 1.0);
const LIME: Color = Color::srgb(0.2, 0.8, 0.2);
const ORANGE: Color = Color::srgb(1.0, 0.65, 0.0);

const COMMS_KIND: &str = "CommunicationsConsoleBoundUserInterface";
const EMERGENCY_KIND: &str = "EmergencyConsoleBoundUserInterface";

/// Набираемое объявление. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default)]
pub struct CommsInput {
    pub text: String,
    pub focused: bool,
}

fn input(world: &mut GameWorld) -> bevy_ecs::world::Mut<'_, CommsInput> {
    world.get_resource_or_insert_with(CommsInput::default)
}

fn redraw(world: &mut GameWorld) {
    if let Some(mut open) = world.get_resource_mut::<OpenUis>() {
        open.revision += 1;
    }
}

fn client_now(world: &GameWorld) -> Duration {
    world
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default()
}

/// Секунды до момента по часам игры, не меньше нуля.
fn seconds_until(world: &GameWorld, at: Option<Duration>) -> f32 {
    at.map_or(0.0, |at| at.saturating_sub(client_now(world)).as_secs_f32())
}

/// Кнопка; выключенная — серая и без действия.
fn button(
    commands: &mut Commands,
    parent: Entity,
    style: &UiStyle,
    text: &str,
    enabled: bool,
    action: UiAction,
) {
    if enabled {
        spawn_button(commands, parent, style, text, action);
        return;
    }
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
    colored(commands, node, style, text, Color::srgb(0.45, 0.45, 0.5));
}

/// Строка текста своим цветом.
fn colored(commands: &mut Commands, parent: Entity, style: &UiStyle, text: &str, color: Color) {
    commands
        .spawn((ChildOf(parent), style.text(text, 14.0)))
        .insert(TextColor(color));
}

fn row(commands: &mut Commands, parent: Entity) -> Entity {
    commands
        .spawn((
            ChildOf(parent),
            Node {
                align_items: AlignItems::Center,
                column_gap: Val::Px(12.0),
                ..default()
            },
        ))
        .id()
}

/// Счётчик символов (`comms-console-char-limit`).
pub fn char_limit(world: &GameWorld, text: &str) -> String {
    let max = world.cvar(&crate::chat::CHAT_MAX_ANNOUNCEMENT_LENGTH);
    loc(
        world,
        "comms-console-char-limit",
        &[
            ("count", LocArg::from(text.trim().chars().count() as i64)),
            ("max", LocArg::from(i64::from(max))),
        ],
    )
}

fn comms_window(
    commands: &mut Commands,
    content: Entity,
    ui: &OpenUi,
    world: &GameWorld,
    style: &UiStyle,
) {
    let state = ui.state::<CommunicationsConsoleState>().unwrap_or_default();
    let typed = world
        .get_resource::<CommsInput>()
        .cloned()
        .unwrap_or_default();
    let entity = ui.entity;
    spawn_label(
        commands,
        content,
        style,
        &loc(world, "comms-console-station-announcements-header", &[]),
    );
    // Поле ввода: щелчок — набор с клавиатуры.
    let shown = if typed.focused {
        format!("{}_", typed.text)
    } else if typed.text.is_empty() {
        loc(world, "comms-console-menu-announcement-placeholder", &[])
    } else {
        typed.text.clone()
    };
    commands.spawn((
        ChildOf(content),
        Interaction::default(),
        Node {
            min_width: Val::Px(400.0),
            min_height: Val::Px(100.0),
            padding: UiRect::all(Val::Px(6.0)),
            margin: UiRect::top(Val::Px(6.0)),
            ..default()
        },
        BackgroundColor(if typed.focused { FIELD_FOCUSED } else { FIELD }),
        UiAction::new(|world| {
            input(world).focused = true;
            redraw(world);
        }),
        children![style.text(shown, 14.0)],
    ));
    let announce_row = row(commands, content);
    spawn_label(
        commands,
        announce_row,
        style,
        &char_limit(world, &typed.text),
    );
    button(
        commands,
        announce_row,
        style,
        &loc(world, "comms-console-menu-announcement-button", &[]),
        state.can_announce,
        UiAction::new(move |world| {
            let text = std::mem::take(&mut input(world).text);
            input(world).focused = false;
            if !text.trim().is_empty() {
                world.send_ui_message(
                    entity,
                    COMMS_UI_KEY,
                    &CommunicationsConsoleMessage::Announce(text),
                );
            }
            redraw(world);
        }),
    );
    // Шаттл эвакуации.
    spawn_label(
        commands,
        content,
        style,
        &loc(world, "comms-console-shuttle-controls-header", &[]),
    );
    let shuttle_row = row(commands, content);
    button(
        commands,
        shuttle_row,
        style,
        &loc(world, "comms-console-call-button-label", &[]),
        state.can_call && !state.countdown_started,
        UiAction::new(move |world| {
            world.send_ui_message(
                entity,
                COMMS_UI_KEY,
                &CommunicationsConsoleMessage::CallEmergencyShuttle,
            );
        }),
    );
    let seconds = if state.countdown_started {
        seconds_until(world, state.expected_countdown_end)
    } else {
        0.0
    };
    spawn_label(
        commands,
        shuttle_row,
        style,
        &format!(
            "{} {}",
            loc(world, "comms-console-shuttle-status-label", &[]),
            clock(seconds)
        ),
    );
    button(
        commands,
        shuttle_row,
        style,
        &loc(world, "comms-console-recall-button-label", &[]),
        state.can_call && state.countdown_started,
        UiAction::new(move |world| {
            world.send_ui_message(
                entity,
                COMMS_UI_KEY,
                &CommunicationsConsoleMessage::RecallEmergencyShuttle,
            );
        }),
    );
}

/// Отсчёт консоли шаттла: до досрочного запуска, без него — «00:10» (`EmergencyConsoleWindow`).
pub fn emergency_countdown(world: &GameWorld, state: &EmergencyConsoleState) -> String {
    match state.early_launch_time {
        None => "00:10".into(),
        Some(at) => {
            let total = at.saturating_sub(client_now(world)).as_secs();
            format!("{:02}:{:02}", total / 60 % 60, total % 60)
        }
    }
}

fn emergency_window(
    commands: &mut Commands,
    content: Entity,
    ui: &OpenUi,
    world: &GameWorld,
    style: &UiStyle,
) {
    let state = ui.state::<EmergencyConsoleState>().unwrap_or_default();
    let entity = ui.entity;
    spawn_label(
        commands,
        content,
        style,
        &emergency_countdown(world, &state),
    );
    let engines = row(commands, content);
    spawn_label(
        commands,
        engines,
        style,
        &loc(world, "emergency-shuttle-ui-engines", &[]),
    );
    colored(
        commands,
        engines,
        style,
        &loc(world, "emergency-shuttle-ui-idle", &[]),
        ORANGE,
    );
    let launch = row(commands, content);
    spawn_label(
        commands,
        launch,
        style,
        &loc(world, "emergency-shuttle-ui-early-authorize", &[]),
    );
    let send = |message: EmergencyConsoleMessage| {
        UiAction::new(move |world| {
            world.send_ui_message(entity, EMERGENCY_CONSOLE_UI_KEY, &message);
        })
    };
    spawn_button(
        commands,
        launch,
        style,
        &loc(world, "emergency-shuttle-ui-repeal-all", &[]),
        send(EmergencyConsoleMessage::RepealAll),
    );
    let authorize = row(commands, content);
    spawn_button(
        commands,
        authorize,
        style,
        &loc(world, "emergency-shuttle-ui-authorize", &[]),
        send(EmergencyConsoleMessage::Authorize),
    );
    spawn_button(
        commands,
        authorize,
        style,
        &loc(world, "emergency-shuttle-ui-repeal", &[]),
        send(EmergencyConsoleMessage::Repeal),
    );
    let remaining = state.authorizations_required - state.authorizations.len() as i32;
    let count = row(commands, content);
    spawn_label(
        commands,
        count,
        style,
        &loc(world, "emergency-shuttle-ui-authorizations", &[]),
    );
    spawn_label(
        commands,
        count,
        style,
        &loc(
            world,
            "emergency-shuttle-ui-remaining",
            &[("remaining", LocArg::from(i64::from(remaining)))],
        ),
    );
    for name in &state.authorizations {
        colored(commands, content, style, name, LIME);
    }
}

fn console_open(world: &GameWorld) -> (bool, bool) {
    let Some(open) = world.get_resource::<OpenUis>() else {
        return (false, false);
    };
    (
        open.list.iter().any(|ui| ui.kind == COMMS_KIND),
        open.list.iter().any(|ui| ui.kind == EMERGENCY_KIND),
    )
}

/// Набор объявления: буквы, пробел, Backspace, Enter — новая строка, Escape — конец набора.
fn keys(
    mut events: MessageReader<KeyboardInput>,
    mut game: ResMut<Game>,
    mut focus: ResMut<KeyboardFocus>,
) {
    let world = &mut game.0;
    let (comms_open, _) = console_open(world);
    let was_focused = world
        .get_resource::<CommsInput>()
        .is_some_and(|i| i.focused);
    if !comms_open {
        if was_focused {
            input(world).focused = false;
            focus.0 = false;
        }
        events.read().for_each(drop);
        return;
    }
    if !was_focused {
        events.read().for_each(drop);
        return;
    }
    let mut changed = false;
    for event in events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }
        let mut typed = input(world);
        match &event.logical_key {
            Key::Escape => typed.focused = false,
            Key::Enter => typed.text.push('\n'),
            Key::Backspace => {
                typed.text.pop();
            }
            Key::Space => typed.text.push(' '),
            Key::Character(text) => typed
                .text
                .extend(text.chars().filter(|c| !c.is_control() && *c != '`')),
            _ => continue,
        }
        changed = true;
    }
    let focused = world
        .get_resource::<CommsInput>()
        .is_some_and(|i| i.focused);
    focus.0 = focused;
    if changed {
        redraw(world);
    }
}

/// Отсчёты в окнах перерисовываются раз в секунду.
fn tick_clock(mut game: ResMut<Game>, mut shown: Local<u64>) {
    let world = &mut game.0;
    let (comms, emergency) = console_open(world);
    if !comms && !emergency {
        return;
    }
    let second = client_now(world).as_secs();
    if *shown != second {
        *shown = second;
        redraw(world);
    }
}

/// Окна консолей связи и шаттла эвакуации.
pub struct CommunicationsUiPlugin;

impl Plugin for CommunicationsUiPlugin {
    fn build(&self, app: &mut App) {
        let mut kinds = app.world_mut().resource_mut::<WindowKinds>();
        kinds.register(COMMS_KIND, comms_window);
        kinds.register(EMERGENCY_KIND, emergency_window);
        app.add_systems(PreUpdate, keys.after(bevy::input::InputSystems));
        app.add_systems(Update, tick_clock);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emergency_countdown_shows_ten_seconds_until_early_launch() {
        let mut world = GameWorld::new();
        world.insert_resource(GameTime::default());
        let mut state = EmergencyConsoleState::default();
        assert_eq!(emergency_countdown(&world, &state), "00:10");
        state.early_launch_time = Some(client_now(&world) + Duration::from_secs(75));
        assert_eq!(emergency_countdown(&world, &state), "01:15");
    }
}
