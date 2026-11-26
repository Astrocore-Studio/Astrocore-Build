//! Консоль разработчика клиента: `~` открывает и закрывает её, Enter отправляет строку
//! команды серверу, стрелки листают историю, Escape закрывает. Пока консоль открыта,
//! клавиши не управляют персонажем. Образец — `DevConsole` и `ShowDebugConsole` Robust
//! @ 1afd692.

use astrocore_client::{Game, KeyboardFocus, UiStyle};
use astrocore_net::{ConsoleLog, RemoteConsoleWorldExt};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;

/// Сколько строк журнала видно.
const VISIBLE_LINES: usize = 18;
const PANEL: Color = Color::srgba(0.05, 0.06, 0.08, 0.92);
const ERROR: Color = Color::srgb(0.95, 0.45, 0.40);
const TEXT: Color = Color::srgb(0.85, 0.87, 0.90);

/// Состояние консоли.
#[derive(Resource, Debug, Default)]
pub struct DevConsole {
    pub open: bool,
    pub input: String,
    /// Отправленные строки, последняя в конце.
    pub history: Vec<String>,
    /// Место в истории при листании стрелками.
    browsing: Option<usize>,
}

impl DevConsole {
    /// Отправляет набранную строку и запоминает её в истории.
    fn submit(&mut self, world: &mut bevy_ecs::world::World) {
        let line = std::mem::take(&mut self.input);
        self.browsing = None;
        if line.trim().is_empty() {
            return;
        }
        if self.history.last() != Some(&line) {
            self.history.push(line.clone());
        }
        world.execute_remote_command(&line);
    }

    fn browse(&mut self, older: bool) {
        if self.history.is_empty() {
            return;
        }
        let last = self.history.len() - 1;
        let index = match (self.browsing, older) {
            (None, true) => Some(last),
            (None, false) => None,
            (Some(index), true) => Some(index.saturating_sub(1)),
            (Some(index), false) if index < last => Some(index + 1),
            (Some(_), false) => None,
        };
        self.browsing = index;
        self.input = index.map(|i| self.history[i].clone()).unwrap_or_default();
    }
}

#[derive(Component)]
struct ConsolePanel;

/// Клавиши консоли: открытие, набор, отправка, история.
fn type_into(
    mut events: MessageReader<KeyboardInput>,
    mut console: ResMut<DevConsole>,
    mut focus: ResMut<KeyboardFocus>,
    mut game: ResMut<Game>,
) {
    for event in events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }
        if event.key_code == KeyCode::Backquote {
            console.open = !console.open;
            continue;
        }
        if !console.open {
            continue;
        }
        match &event.logical_key {
            Key::Escape => console.open = false,
            Key::Enter => console.submit(&mut game.0),
            Key::Backspace => {
                console.input.pop();
            }
            Key::ArrowUp => console.browse(true),
            Key::ArrowDown => console.browse(false),
            Key::Space => console.input.push(' '),
            Key::Character(text) => {
                console.input.extend(
                    text.chars()
                        .filter(|c| !c.is_control() && *c != '`' && *c != '~'),
                );
            }
            _ => {}
        }
    }
    if focus.0 != console.open {
        focus.0 = console.open;
    }
}

/// Перерисовывает панель, когда меняется журнал, ввод или видимость.
fn draw(
    mut commands: Commands,
    console: Res<DevConsole>,
    game: Res<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<ConsolePanel>>,
    mut shown: Local<Option<(bool, u64, String)>>,
) {
    let version = game
        .0
        .get_resource::<ConsoleLog>()
        .map_or(0, |log| log.version);
    let now = (console.open, version, console.input.clone());
    if shown.as_ref() == Some(&now) {
        return;
    }
    *shown = Some(now);
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    if !console.open {
        return;
    }
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(8.0)),
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(PANEL),
            GlobalZIndex(100),
            ConsolePanel,
        ))
        .id();
    let lines = game
        .0
        .get_resource::<ConsoleLog>()
        .map(|log| log.lines.clone())
        .unwrap_or_default();
    let start = lines.len().saturating_sub(VISIBLE_LINES);
    for line in &lines[start..] {
        let color = if line.error { ERROR } else { TEXT };
        commands.spawn((ChildOf(panel), text(&style, line.text.clone(), 14.0, color)));
    }
    let input = format!("> {}_", console.input);
    commands.spawn((ChildOf(panel), text(&style, input, 15.0, Color::WHITE)));
}

fn text(style: &UiStyle, text: String, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(text),
        TextFont {
            font: style.font.clone().into(),
            font_size: size.into(),
            ..default()
        },
        TextColor(color),
    )
}

/// Консоль разработчика.
pub struct DevConsolePlugin;

impl Plugin for DevConsolePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DevConsole>();
        app.add_systems(PreUpdate, type_into.after(bevy::input::InputSystems));
        app.add_systems(Update, draw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_browses_back_and_forth() {
        let mut console = DevConsole {
            history: vec!["lsmap".into(), "tp 1 2".into()],
            ..DevConsole::default()
        };
        console.browse(true);
        assert_eq!(console.input, "tp 1 2");
        console.browse(true);
        assert_eq!(console.input, "lsmap");
        console.browse(true);
        assert_eq!(console.input, "lsmap");
        console.browse(false);
        assert_eq!(console.input, "tp 1 2");
        console.browse(false);
        assert_eq!(console.input, "");
    }
}
