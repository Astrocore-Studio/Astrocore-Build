//! Окно ahelp на клиенте: F1 открывает переписку с администраторами; у администратора —
//! список каналов игроков, переключатели «звук» и «только администраторам»; значок
//! непрочитанного. Образец — `AHelpUIController`, `BwoinkWindow`, `BwoinkControl`,
//! `BwoinkPanel`, `UserAHelpUIHandler`, `AdminAHelpUIHandler` клиента SS14 @ 2f4a2aad.
//!
//! Нет пока: поиска и списка всех игроков у администратора (каналы — по переписке),
//! индикатора набора, кнопок действий над игроком в окне.

use astrocore_client::{Game, KeyboardFocus, UiAction, UiStyle};
use astrocore_math::Color as GameColor;
use astrocore_net::{NetEventWorldExt, NetUserId};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy_ecs::world::World as GameWorld;

use crate::admin_manager::{ClientAdmin, flags};
use crate::bwoink::{BwoinkTextMessage, ClientBwoink, SYSTEM_USER};
use crate::chat_ui::{markup_spans, spans_text};
use crate::examine::loc;

const PANEL: Color = Color::srgba(0.07, 0.08, 0.11, 0.96);
const ROW: Color = Color::srgba(0.16, 0.18, 0.24, 1.0);
const SELECTED: Color = Color::srgba(0.30, 0.34, 0.50, 1.0);
const UNREAD: Color = Color::srgb(0.85, 0.20, 0.20);

/// Состояние окна. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Clone)]
pub struct AhelpWindow {
    pub open: bool,
    pub selected: Option<NetUserId>,
    pub input: String,
    pub play_sound: bool,
    pub admin_only: bool,
    pub version: u64,
}

impl Default for AhelpWindow {
    fn default() -> Self {
        Self {
            open: false,
            selected: None,
            input: String::new(),
            play_sound: true,
            admin_only: false,
            version: 0,
        }
    }
}

/// Администратор ли с правом ahelp (`AdminAHelpUIHandler`).
pub fn is_ahelp_admin(world: &GameWorld) -> bool {
    world
        .get_resource::<ClientAdmin>()
        .is_some_and(|a| a.has_flag(flags::ADMINHELP))
}

/// Канал, в который пишет игрок: свой; администратор — выбранный.
pub fn current_channel(world: &GameWorld) -> Option<NetUserId> {
    if is_ahelp_admin(world) {
        world.get_resource::<AhelpWindow>()?.selected
    } else {
        world.get_resource::<ClientAdmin>()?.user_id
    }
}

/// Открыть или закрыть окно (`ToggleWindow`): открытие снимает непрочитанное.
pub fn toggle(world: &mut GameWorld) {
    let open = {
        let mut window = world.get_resource_or_insert_with(AhelpWindow::default);
        window.open = !window.open;
        window.version += 1;
        window.open
    };
    if open {
        // Администратор сразу видит последний канал с сообщениями.
        if is_ahelp_admin(world) && world.resource::<AhelpWindow>().selected.is_none() {
            let last = world
                .get_resource::<ClientBwoink>()
                .and_then(|b| b.channels.keys().next_back().copied());
            world.resource_mut::<AhelpWindow>().selected = last;
        }
        let mut bwoink = world.get_resource_or_insert_with(ClientBwoink::default);
        bwoink.unread = false;
        bwoink.version += 1;
    }
}

/// Отправить набранное (`SendMessageAction`).
pub fn submit(world: &mut GameWorld) {
    let Some(channel) = current_channel(world) else {
        return;
    };
    let (text, play_sound, admin_only) = {
        let mut window = world.get_resource_or_insert_with(AhelpWindow::default);
        window.version += 1;
        (
            std::mem::take(&mut window.input),
            window.play_sound,
            window.admin_only,
        )
    };
    if text.trim().is_empty() {
        return;
    }
    world.send_net_event_to_server(&BwoinkTextMessage {
        user_id: channel,
        true_sender: SYSTEM_USER,
        text: text.trim().to_owned(),
        play_sound,
        admin_only,
        channel_name: String::new(),
    });
}

fn type_into(
    mut events: MessageReader<KeyboardInput>,
    mut focus: ResMut<KeyboardFocus>,
    mut game: ResMut<Game>,
) {
    let world = &mut game.0;
    if world.get_resource::<ClientBwoink>().is_none() {
        return;
    }
    let was_open = world.get_resource::<AhelpWindow>().is_some_and(|w| w.open);
    for event in events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }
        if event.key_code == KeyCode::F1 {
            // F1 работает, только если клавиатуру не держит другая строка ввода.
            if was_open || !focus.0 {
                toggle(world);
            }
            continue;
        }
        let open = world.get_resource::<AhelpWindow>().is_some_and(|w| w.open);
        if !open {
            continue;
        }
        let mut window = world.resource_mut::<AhelpWindow>();
        match &event.logical_key {
            Key::Escape => {
                window.open = false;
                window.version += 1;
            }
            Key::Enter => submit(world),
            Key::Backspace => {
                window.input.pop();
                window.version += 1;
            }
            Key::Space => {
                window.input.push(' ');
                window.version += 1;
            }
            Key::Character(text) => {
                window
                    .input
                    .extend(text.chars().filter(|c| !c.is_control()));
                window.version += 1;
            }
            _ => {}
        }
    }
    let open = world.get_resource::<AhelpWindow>().is_some_and(|w| w.open);
    if open != was_open {
        focus.0 = open;
    }
}

#[derive(Component)]
struct AhelpPanel;

fn chip(
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

fn draw(
    mut commands: Commands,
    mut game: ResMut<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<AhelpPanel>>,
    mut shown: Local<Option<(u64, u64, u64)>>,
) {
    let world = &mut game.0;
    let Some(bwoink) = world.get_resource::<ClientBwoink>().cloned() else {
        return;
    };
    let window = world
        .get_resource_or_insert_with(AhelpWindow::default)
        .clone();
    let admin_version = world.get_resource::<ClientAdmin>().map_or(0, |a| a.version);
    let now = (bwoink.version, window.version, admin_version);
    if shown.as_ref() == Some(&now) {
        return;
    }
    *shown = Some(now);
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    if !window.open {
        if bwoink.unread {
            // Значок непрочитанного (`UnreadAHelpReceived`).
            let badge = commands
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        right: Val::Px(12.0),
                        top: Val::Px(310.0),
                        padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
                        ..default()
                    },
                    BackgroundColor(UNREAD),
                    GlobalZIndex(61),
                    Interaction::default(),
                    UiAction::new(toggle),
                    AhelpPanel,
                ))
                .id();
            commands.spawn((ChildOf(badge), style.text("AHelp (F1)", 14.0)));
        }
        return;
    }
    let admin = is_ahelp_admin(world);
    let title = loc(
        world,
        if admin {
            "bwoink-admin-title"
        } else {
            "bwoink-user-title"
        },
        &[],
    );
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(20.0),
                margin: UiRect::left(Val::Px(-320.0)),
                width: Val::Px(640.0),
                height: Val::Px(380.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                padding: UiRect::all(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            GlobalZIndex(70),
            Interaction::default(),
            AhelpPanel,
        ))
        .id();
    commands.spawn((ChildOf(panel), style.text(title, 16.0)));
    let body = commands
        .spawn((
            ChildOf(panel),
            Node {
                flex_grow: 1.0,
                column_gap: Val::Px(8.0),
                overflow: Overflow::clip(),
                ..default()
            },
        ))
        .id();
    let channel = current_channel(world);
    if admin {
        let list = commands
            .spawn((
                ChildOf(body),
                Node {
                    width: Val::Px(160.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(3.0),
                    ..default()
                },
            ))
            .id();
        if bwoink.channels.is_empty() {
            commands.spawn((
                ChildOf(list),
                style.text(loc(world, "bwoink-title-none-selected", &[]), 12.0),
            ));
        }
        for (user, (name, _)) in &bwoink.channels {
            let user = *user;
            let label = if name.is_empty() {
                user.to_string()
            } else {
                name.clone()
            };
            chip(
                &mut commands,
                &style,
                list,
                label,
                if Some(user) == channel { SELECTED } else { ROW },
                UiAction::new(move |world| {
                    let mut window = world.get_resource_or_insert_with(AhelpWindow::default);
                    window.selected = Some(user);
                    window.version += 1;
                }),
            );
        }
    }
    let log = commands
        .spawn((
            ChildOf(body),
            Node {
                flex_grow: 1.0,
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::FlexEnd,
                row_gap: Val::Px(2.0),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.3)),
        ))
        .id();
    let lines = channel
        .and_then(|c| bwoink.channels.get(&c))
        .map(|(_, lines)| lines.clone())
        .unwrap_or_default();
    for line in lines.iter().rev().take(40).rev() {
        let spans = markup_spans(
            line,
            GameColor::parse("lightgray").unwrap_or(GameColor::WHITE),
        );
        spans_text(&mut commands, &style, log, &spans, 13.0);
    }
    if admin {
        let toggles = commands
            .spawn((
                ChildOf(panel),
                Node {
                    column_gap: Val::Px(6.0),
                    ..default()
                },
            ))
            .id();
        // Переключатели: нажатый подсвечен (`Button.ToggleMode`).
        let shade = |on: bool| if on { SELECTED } else { ROW };
        chip(
            &mut commands,
            &style,
            toggles,
            loc(world, "admin-bwoink-play-sound", &[]),
            shade(window.play_sound),
            UiAction::new(|world| {
                let mut window = world.get_resource_or_insert_with(AhelpWindow::default);
                window.play_sound = !window.play_sound;
                window.version += 1;
            }),
        );
        chip(
            &mut commands,
            &style,
            toggles,
            loc(world, "bwoink-message-admin-only", &[]),
            shade(window.admin_only),
            UiAction::new(|world| {
                let mut window = world.get_resource_or_insert_with(AhelpWindow::default);
                window.admin_only = !window.admin_only;
                window.version += 1;
            }),
        );
    }
    let input = commands
        .spawn((
            ChildOf(panel),
            Node {
                padding: UiRect::all(Val::Px(5.0)),
                ..default()
            },
            BackgroundColor(ROW),
        ))
        .id();
    let shown_input = if channel.is_some() {
        format!("{}_", window.input)
    } else {
        loc(world, "bwoink-title-none-selected", &[])
    };
    commands.spawn((ChildOf(input), style.text(shown_input, 14.0)));
}

/// Окно ahelp.
pub struct AhelpUiPlugin;

impl Plugin for AhelpUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, type_into.after(bevy::input::InputSystems));
        app.add_systems(Update, draw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_manager::AdminData;

    fn world(admin: bool) -> GameWorld {
        let mut world = GameWorld::new();
        let user = NetUserId(uuid::Uuid::from_bytes([1; 16]));
        world.insert_resource(ClientAdmin {
            admin: admin.then_some(AdminData {
                flags: flags::ADMINHELP,
                title: None,
                active: true,
                stealth: false,
            }),
            available_commands: Vec::new(),
            user_id: Some(user),
            version: 0,
        });
        world.init_resource::<ClientBwoink>();
        world
    }

    #[test]
    fn players_write_to_their_own_channel_and_admins_to_the_selected_one() {
        let mut world = world(false);
        assert_eq!(
            current_channel(&world),
            Some(NetUserId(uuid::Uuid::from_bytes([1; 16])))
        );
        let mut admin = self::world(true);
        assert_eq!(current_channel(&admin), None);
        let other = NetUserId(uuid::Uuid::from_bytes([7; 16]));
        crate::bwoink::client_receive(
            &mut admin,
            &BwoinkTextMessage {
                user_id: other,
                true_sender: other,
                text: "help".into(),
                play_sound: false,
                admin_only: false,
                channel_name: "Urist".into(),
            },
        );
        assert!(admin.resource::<ClientBwoink>().unread);
        // Открытие выбирает канал и снимает непрочитанное.
        toggle(&mut admin);
        assert_eq!(current_channel(&admin), Some(other));
        assert!(!admin.resource::<ClientBwoink>().unread);
        // Пустое не отправляется, набранное — очищается.
        world.init_resource::<AhelpWindow>();
        world.resource_mut::<AhelpWindow>().input = "   ".into();
        submit(&mut world);
        assert_eq!(world.resource::<AhelpWindow>().input, "");
    }
}
