//! Голосования на клиенте: окошко каждого идущего голосования (кто вызвал, заголовок,
//! варианты с числом голосов, свой голос выделен, полоса и остаток времени), меню вызова
//! (вид — перезапуск, режим, карта; недоступный вид с причиной) и игровое меню на F10 с
//! кнопкой вызова голосования. Меню вызова открывается и из лобби.
//! Образец — `VotePopup`, `VoteCallMenu`, `VoteCallMenuButton`, `EscapeMenu` клиента SS14
//! @ 2f4a2aad.
//!
//! В игровом меню пока только «Голосование» и «Выход»: правил, справочника, вики, журнала
//! изменений, настроек и отключения у нас ещё нет (ROADMAP, 8.9).

use std::time::Instant;

use astrocore_client::{Game, KeyboardFocus, UiAction, UiStyle};
use astrocore_loc::LocArg;
use astrocore_net::NetEventWorldExt;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy_ecs::world::World as GameWorld;

use crate::examine::loc;
use crate::voting::{ClientVotes, StandardVoteType, VoteMenuOpened, send_cast_vote};

const PANEL: Color = Color::srgba(0.07, 0.08, 0.11, 0.92);
const ROW: Color = Color::srgba(0.16, 0.18, 0.24, 1.0);
const SELECTED: Color = Color::srgba(0.30, 0.34, 0.50, 1.0);
const OFF: Color = Color::srgba(0.10, 0.11, 0.14, 1.0);
const BAR: Color = Color::srgb(0.45, 0.65, 0.95);

/// Окна голосований. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default)]
pub struct VoteWindows {
    pub call_menu: bool,
    pub selected: Option<StandardVoteType>,
    pub escape_menu: bool,
    pub quit: bool,
    pub version: u64,
}

fn windows(world: &mut GameWorld) -> bevy_ecs::world::Mut<'_, VoteWindows> {
    world.get_resource_or_insert_with(VoteWindows::default)
}

/// Открыть меню вызова (`VoteCallMenu.Opened`): сервер записывает это в журнал.
pub fn open_call_menu(world: &mut GameWorld) {
    let mut windows = windows(world);
    windows.call_menu = true;
    windows.escape_menu = false;
    windows.version += 1;
    world.send_net_event_to_server(&VoteMenuOpened);
}

fn type_key(kind: StandardVoteType) -> &'static str {
    match kind {
        StandardVoteType::Restart => "ui-vote-type-restart",
        StandardVoteType::Preset => "ui-vote-type-gamemode",
        StandardVoteType::Map => "ui-vote-type-map",
    }
}

/// Остаток времени как в SS14: минуты и секунды, с округлением вверх.
pub fn time_left_text(seconds: f32) -> String {
    let total = seconds.max(0.0).ceil() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// Причина, по которой вид недоступен (`ui-vote-type-timeout` / `ui-vote-type-not-available`).
fn unavailable_reason(
    world: &GameWorld,
    votes: &ClientVotes,
    kind: StandardVoteType,
) -> Option<String> {
    match votes.can_call_standard(kind) {
        Ok(()) => None,
        Err(Some(wait)) => Some(loc(
            world,
            "ui-vote-type-timeout",
            &[(
                "remaining",
                LocArg::from(time_left_text(wait.as_secs_f32())),
            )],
        )),
        Err(None) => Some(loc(world, "ui-vote-type-not-available", &[])),
    }
}

fn button(
    commands: &mut Commands,
    style: &UiStyle,
    parent: Entity,
    text: String,
    color: Color,
    action: Option<UiAction>,
) -> Entity {
    let mut entity = commands.spawn((
        ChildOf(parent),
        Node {
            padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(color),
    ));
    if let Some(action) = action {
        entity.insert((Interaction::default(), action));
    }
    let id = entity.id();
    commands.spawn((ChildOf(id), style.text(text, 13.0)));
    id
}

fn panel(commands: &mut Commands, node: Node) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(5.0),
                padding: UiRect::all(Val::Px(8.0)),
                ..node
            },
            BackgroundColor(PANEL),
            GlobalZIndex(74),
            Interaction::default(),
            VotePanel,
        ))
        .id()
}

#[derive(Component)]
struct VotePanel;

fn draw_popups(commands: &mut Commands, style: &UiStyle, world: &GameWorld, votes: &ClientVotes) {
    let now = Instant::now();
    let column = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(10.0),
                top: Val::Px(70.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },
            GlobalZIndex(73),
            VotePanel,
        ))
        .id();
    for (id, vote) in &votes.votes {
        let popup = commands
            .spawn((
                ChildOf(column),
                Node {
                    width: Val::Px(260.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    padding: UiRect::all(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(PANEL),
                Interaction::default(),
            ))
            .id();
        commands.spawn((
            ChildOf(popup),
            style.text(
                loc(
                    world,
                    "ui-vote-created",
                    &[("initiator", LocArg::from(vote.initiator.as_str()))],
                ),
                12.0,
            ),
        ));
        commands.spawn((ChildOf(popup), style.text(vote.title.clone(), 15.0)));
        for (index, (text, count)) in vote.entries.iter().enumerate() {
            let label = if vote.display_votes {
                loc(
                    world,
                    "ui-vote-button",
                    &[
                        ("text", LocArg::from(text.as_str())),
                        ("votes", LocArg::from(i64::from(*count))),
                    ],
                )
            } else {
                loc(
                    world,
                    "ui-vote-button-no-votes",
                    &[("text", LocArg::from(text.as_str()))],
                )
            };
            let ours = vote.our_vote == Some(index as u8);
            let id = *id;
            button(
                commands,
                style,
                popup,
                label,
                if ours { SELECTED } else { ROW },
                Some(UiAction::new(move |world| {
                    send_cast_vote(world, id, index as u8);
                })),
            );
        }
        // Полоса прошедшего времени и остаток (`TimeLeftBar`, `TimeLeftText`).
        let total = vote
            .end
            .saturating_duration_since(vote.start)
            .as_secs_f32()
            .max(0.001);
        let left = vote.end.saturating_duration_since(now).as_secs_f32();
        let progress = (1.0 - left / total).clamp(0.0, 1.0);
        let bar = commands
            .spawn((
                ChildOf(popup),
                Node {
                    height: Val::Px(6.0),
                    ..default()
                },
                BackgroundColor(OFF),
            ))
            .id();
        commands.spawn((
            ChildOf(bar),
            Node {
                width: Val::Percent(progress * 100.0),
                ..default()
            },
            BackgroundColor(BAR),
        ));
        commands.spawn((ChildOf(popup), style.text(time_left_text(left), 12.0)));
    }
}

fn draw_call_menu(
    commands: &mut Commands,
    style: &UiStyle,
    world: &GameWorld,
    votes: &ClientVotes,
    state: &VoteWindows,
) {
    let root = panel(
        commands,
        Node {
            left: Val::Percent(50.0),
            top: Val::Percent(30.0),
            margin: UiRect::left(Val::Px(-160.0)),
            width: Val::Px(320.0),
            ..default()
        },
    );
    let header = commands
        .spawn((
            ChildOf(root),
            Node {
                column_gap: Val::Px(8.0),
                ..default()
            },
        ))
        .id();
    commands.spawn((
        ChildOf(header),
        style.text(loc(world, "ui-vote-create-title", &[]), 16.0),
    ));
    button(
        commands,
        style,
        header,
        "X".into(),
        ROW,
        Some(UiAction::new(|world| {
            let mut windows = windows(world);
            windows.call_menu = false;
            windows.version += 1;
        })),
    );
    for kind in StandardVoteType::ALL {
        let selected = state.selected == Some(kind);
        button(
            commands,
            style,
            root,
            loc(world, type_key(kind), &[]),
            if selected { SELECTED } else { ROW },
            Some(UiAction::new(move |world| {
                let mut windows = windows(world);
                windows.selected = Some(kind);
                windows.version += 1;
            })),
        );
    }
    let reason = state
        .selected
        .and_then(|kind| unavailable_reason(world, votes, kind));
    if let Some(reason) = &reason {
        commands.spawn((ChildOf(root), style.text(reason.clone(), 12.0)));
    }
    let can = votes.can_call && state.selected.is_some() && reason.is_none();
    let selected = state.selected;
    button(
        commands,
        style,
        root,
        loc(world, "ui-vote-create-button", &[]),
        if can { SELECTED } else { OFF },
        can.then(|| {
            UiAction::new(move |world| {
                if let Some(kind) = selected {
                    astrocore_net::RemoteConsoleWorldExt::execute_remote_command(
                        world,
                        &format!("createvote {kind:?}"),
                    );
                }
                let mut windows = windows(world);
                windows.call_menu = false;
                windows.version += 1;
            })
        }),
    );
}

fn draw_escape_menu(
    commands: &mut Commands,
    style: &UiStyle,
    world: &GameWorld,
    votes: &ClientVotes,
) {
    let root = panel(
        commands,
        Node {
            left: Val::Percent(50.0),
            top: Val::Percent(35.0),
            margin: UiRect::left(Val::Px(-110.0)),
            width: Val::Px(220.0),
            ..default()
        },
    );
    commands.spawn((
        ChildOf(root),
        style.text(loc(world, "ui-escape-title", &[]), 16.0),
    ));
    button(
        commands,
        style,
        root,
        loc(world, "ui-vote-menu-button", &[]),
        if votes.can_call { ROW } else { OFF },
        votes.can_call.then(|| UiAction::new(open_call_menu)),
    );
    button(
        commands,
        style,
        root,
        loc(world, "ui-escape-quit", &[]),
        ROW,
        Some(UiAction::new(|world| windows(world).quit = true)),
    );
}

fn keys(
    mut events: MessageReader<KeyboardInput>,
    focus: Res<KeyboardFocus>,
    mut game: ResMut<Game>,
) {
    for event in events.read() {
        if event.state != ButtonState::Pressed || focus.0 {
            continue;
        }
        // Игровое меню (`ShowEscapeMenu`, F10).
        if event.key_code == KeyCode::F10 {
            let mut windows = windows(&mut game.0);
            windows.escape_menu = !windows.escape_menu;
            windows.version += 1;
        }
    }
}

fn draw(
    mut commands: Commands,
    mut game: ResMut<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<VotePanel>>,
    mut shown: Local<Option<(u64, u64, u64)>>,
    mut exit: MessageWriter<AppExit>,
) {
    let world = &mut game.0;
    let Some(votes) = world.get_resource::<ClientVotes>().cloned() else {
        return;
    };
    let state = windows(world).clone();
    if state.quit {
        exit.write(AppExit::Success);
        return;
    }
    // Остаток времени перерисовывается раз в секунду.
    let second = votes
        .votes
        .values()
        .map(|v| v.end.saturating_duration_since(Instant::now()).as_secs())
        .sum::<u64>();
    let now = (votes.version, state.version, second);
    if shown.as_ref() == Some(&now) {
        return;
    }
    *shown = Some(now);
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    if !votes.votes.is_empty() {
        draw_popups(&mut commands, &style, world, &votes);
    }
    if state.call_menu {
        draw_call_menu(&mut commands, &style, world, &votes, &state);
    }
    if state.escape_menu {
        draw_escape_menu(&mut commands, &style, world, &votes);
    }
}

/// Голосования и игровое меню.
pub struct VoteUiPlugin;

impl Plugin for VoteUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, keys.after(bevy::input::InputSystems));
        app.add_systems(Update, draw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_rounds_up_to_whole_seconds() {
        assert_eq!(time_left_text(0.0), "0:00");
        assert_eq!(time_left_text(0.2), "0:01");
        assert_eq!(time_left_text(59.5), "1:00");
        assert_eq!(time_left_text(90.0), "1:30");
    }
}
