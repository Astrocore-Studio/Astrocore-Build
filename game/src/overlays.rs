//! Слой поверх мира на клиенте: всплывающие сообщения, полосы действий с задержкой, окно
//! осмотра и контекстное меню. Образец — `PopupOverlay`, `DoAfterOverlay`,
//! `ExamineSystem` и `ContextMenuUIController` клиента SS14 @ 2f4a2aad.

use astrocore_client::{Game, Hovered, Tooltip, UiAction, UiHover, UiStyle};
use astrocore_ecs::GameTime;
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::TransformWorldExt;
use astrocore_net::{ClientNet, NetEntity, NetEventWorldExt};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy::window::PrimaryWindow;

use crate::do_after::DoAfterBars;
use crate::examine::{ExamineMessage, ExamineResult, strip_markup};
use crate::hands::HandsWorldExt;
use crate::popups::{Popup, PopupPlace, PopupQueue, PopupType, popup_lifetime};
use crate::verbs::{VerbMenuData, VerbMessage};

const PANEL: Color = Color::srgba(0.09, 0.10, 0.13, 0.95);
const BORDER: Color = Color::srgb(0.30, 0.33, 0.42);
const ROW: Color = Color::srgba(0.16, 0.18, 0.24, 1.0);
const DISABLED: Color = Color::srgb(0.5, 0.5, 0.55);

fn world_to_screen(camera: &Camera, transform: &GlobalTransform, point: Vec2) -> Option<Vec2> {
    camera.world_to_viewport(transform, point.extend(0.0)).ok()
}

fn entity_point(world: &bevy_ecs::world::World, entity: bevy_ecs::entity::Entity) -> Option<Vec2> {
    let at = world.world_transform(entity)?.position;
    Some(Vec2::new(at.x, at.y))
}

// ---------------------------------------------------------------- всплывающие сообщения

/// Показанное сообщение.
#[derive(Component)]
struct PopupLabel {
    popup: Popup,
    age: f32,
    lifetime: f32,
    /// Точка на экране для сообщений у курсора.
    cursor: Option<Vec2>,
    color: Color,
}

fn popup_size(kind: PopupType) -> f32 {
    match kind {
        PopupType::Small | PopupType::SmallCaution => 13.0,
        PopupType::Medium | PopupType::MediumCaution => 17.0,
        PopupType::Large | PopupType::LargeCaution => 21.0,
    }
}

fn spawn_popups(
    mut commands: Commands,
    mut game: ResMut<Game>,
    style: Res<UiStyle>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
) {
    let popups = match game.0.get_resource_mut::<PopupQueue>() {
        Some(mut queue) if !queue.0.is_empty() => std::mem::take(&mut queue.0),
        _ => return,
    };
    let cursor = window.and_then(|window| window.cursor_position());
    for popup in popups {
        let color = if popup.kind.is_caution() {
            Color::srgb(0.95, 0.25, 0.25)
        } else {
            Color::srgb(0.95, 0.95, 0.95)
        };
        let size = popup_size(popup.kind);
        let lifetime = popup_lifetime(&popup.message);
        let text = popup.message.clone();
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            style.text(text, size),
            GlobalZIndex(500),
            PopupLabel {
                cursor: matches!(popup.place, PopupPlace::Cursor)
                    .then_some(cursor)
                    .flatten(),
                popup,
                age: 0.0,
                lifetime,
                color,
            },
        ));
    }
}

fn move_popups(
    mut commands: Commands,
    game: Res<Game>,
    time: Res<Time>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    mut labels: Query<(
        Entity,
        &mut PopupLabel,
        &mut Node,
        &mut TextColor,
        &ComputedNode,
    )>,
) {
    let (camera, transform) = *camera;
    for (entity, mut label, mut node, mut color, computed) in &mut labels {
        label.age += time.delta_secs();
        if label.age > label.lifetime {
            commands.entity(entity).despawn();
            continue;
        }
        // Сообщение всплывает вверх и тает в последней трети жизни.
        let rise = label.age * 0.6;
        let screen = match label.popup.place {
            PopupPlace::Entity(target) => match entity_point(&game.0, target) {
                Some(point) => {
                    world_to_screen(camera, transform, point + Vec2::new(0.0, 0.5 + rise))
                }
                None => {
                    commands.entity(entity).despawn();
                    continue;
                }
            },
            PopupPlace::Coordinates(at) => world_to_screen(
                camera,
                transform,
                Vec2::new(at.position.x, at.position.y + 0.5 + rise),
            ),
            PopupPlace::Cursor => label
                .cursor
                .map(|cursor| cursor - Vec2::new(0.0, 20.0 + rise * 32.0)),
        };
        let Some(screen) = screen else {
            continue;
        };
        let size = computed.size() * computed.inverse_scale_factor();
        node.left = Val::Px(screen.x - size.x / 2.0);
        node.top = Val::Px(screen.y - size.y / 2.0);
        let fade = ((label.lifetime - label.age) / (label.lifetime / 3.0)).clamp(0.0, 1.0);
        color.0 = label.color.with_alpha(fade);
    }
}

// ---------------------------------------------------------------- полосы действий

/// Полоса действия над сущностью.
#[derive(Component)]
struct BarView {
    entity: bevy_ecs::entity::Entity,
    index: u16,
    fill: Entity,
}

fn sync_bars(
    mut commands: Commands,
    game: Res<Game>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    mut views: Query<(Entity, &BarView, &mut Node)>,
    mut fills: Query<(&mut Node, &mut BackgroundColor), Without<BarView>>,
) {
    let world = &game.0;
    let (camera, transform) = *camera;
    // Время сервера: по последнему применённому состоянию.
    let applied = world.resource::<ClientNet>().applied;
    let now = world.resource::<GameTime>().time_of(applied).as_secs_f64();
    let mut wanted: Vec<(bevy_ecs::entity::Entity, u16, f32, Color)> = Vec::new();
    let mut query = world.try_query::<(bevy_ecs::entity::Entity, &DoAfterBars)>();
    if let Some(query) = query.as_mut() {
        for (entity, bars) in query.iter(world) {
            for (row, bar) in bars.bars.iter().enumerate() {
                let progress =
                    ((now - bar.start_time) / bar.delay.max(1e-3)).clamp(0.0, 1.0) as f32;
                let color = match (bar.cancelled_time, bar.completed) {
                    (Some(_), _) => Color::srgb(0.85, 0.15, 0.15),
                    (None, true) => Color::srgb(0.2, 0.85, 0.3),
                    (None, false) => Color::srgb(1.0 - progress * 0.8, 0.85, 0.25),
                };
                let _ = row;
                wanted.push((
                    entity,
                    bar.index,
                    if bar.completed { 1.0 } else { progress },
                    color,
                ));
            }
        }
    }
    for (view, bar, mut node) in &mut views {
        let Some((_, _, progress, color)) = wanted
            .iter()
            .find(|(entity, index, _, _)| *entity == bar.entity && *index == bar.index)
            .copied()
        else {
            commands.entity(view).despawn();
            continue;
        };
        let row = wanted
            .iter()
            .filter(|(entity, _, _, _)| *entity == bar.entity)
            .position(|(_, index, _, _)| *index == bar.index)
            .unwrap_or(0);
        if let Some(point) = entity_point(world, bar.entity)
            && let Some(screen) = world_to_screen(camera, transform, point + Vec2::new(0.0, 0.75))
        {
            node.left = Val::Px(screen.x - 16.0);
            node.top = Val::Px(screen.y - 6.0 * (row as f32 + 1.0));
        }
        if let Ok((mut fill, mut fill_color)) = fills.get_mut(bar.fill) {
            fill.width = Val::Percent(progress * 100.0);
            fill_color.0 = color;
        }
    }
    for (entity, index, _, _) in wanted {
        if views
            .iter()
            .any(|(_, bar, _)| bar.entity == entity && bar.index == index)
        {
            continue;
        }
        let frame = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(32.0),
                    height: Val::Px(4.0),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
                GlobalZIndex(400),
            ))
            .id();
        let fill = commands
            .spawn((
                ChildOf(frame),
                Node {
                    width: Val::Percent(0.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(Color::srgb(1.0, 0.85, 0.25)),
            ))
            .id();
        commands.entity(frame).insert(BarView {
            entity,
            index,
            fill,
        });
    }
}

// ---------------------------------------------------------------- окно осмотра

#[derive(Component)]
struct ExamineWindow;

fn show_examine(
    mut commands: Commands,
    game: Res<Game>,
    style: Res<UiStyle>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    mut seen: Local<u64>,
    existing: Query<Entity, With<ExamineWindow>>,
) {
    let Some(result) = game.0.get_resource::<ExamineResult>() else {
        return;
    };
    if result.revision == *seen {
        return;
    }
    *seen = result.revision;
    for old in &existing {
        commands.entity(old).despawn();
    }
    let cursor = window
        .and_then(|window| window.cursor_position())
        .unwrap_or(Vec2::new(200.0, 200.0));
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(cursor.x + 12.0),
                top: Val::Px(cursor.y + 12.0),
                max_width: Val::Px(360.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(8.0)),
                row_gap: Val::Px(4.0),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            BorderColor::all(BORDER),
            GlobalZIndex(900),
            Interaction::default(),
            ExamineWindow,
            UiAction::new(|_| {}),
        ))
        .id();
    commands.spawn((ChildOf(panel), style.text(result.name.clone(), 16.0)));
    for line in &result.lines {
        commands.spawn((ChildOf(panel), style.text(strip_markup(line), 13.0)));
    }
}

/// Окно осмотра закрывается кликом мимо него.
fn close_examine(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    hover: Res<UiHover>,
    windows: Query<Entity, With<ExamineWindow>>,
) {
    if mouse.get_just_pressed().next().is_some() && !hover.0 {
        for window in &windows {
            commands.entity(window).despawn();
        }
    }
}

// ---------------------------------------------------------------- контекстное меню

#[derive(Component)]
struct ContextMenu;

/// Что показывает меню: список сущностей или глаголы одной.
#[derive(Resource, Default)]
struct MenuState {
    open: bool,
    at: Vec2,
    entities: Vec<bevy_ecs::entity::Entity>,
    target: Option<bevy_ecs::entity::Entity>,
    /// Ревизия ответа с глаголами, по которой построено меню.
    built: Option<u64>,
    dirty: bool,
}

fn request_verbs(world: &mut bevy_ecs::world::World, target: bevy_ecs::entity::Entity) {
    if let Some(net) = world.get::<NetEntity>(target).copied() {
        world.send_net_event_to_server(&VerbMessage::Request(net));
    }
}

fn open_menu(
    mouse: Res<ButtonInput<MouseButton>>,
    hover: Res<UiHover>,
    hovered: Res<Hovered>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    mut game: ResMut<Game>,
    mut state: ResMut<MenuState>,
) {
    if hover.0 {
        return;
    }
    if mouse.just_pressed(MouseButton::Left) && state.open {
        *state = MenuState::default();
        state.dirty = true;
        return;
    }
    if !mouse.just_pressed(MouseButton::Right) {
        return;
    }
    let Some(cursor) = window.and_then(|window| window.cursor_position()) else {
        return;
    };
    *state = MenuState {
        open: !hovered.entities.is_empty(),
        at: cursor,
        entities: hovered.entities.clone(),
        target: None,
        built: None,
        dirty: true,
    };
    if let [only] = hovered.entities.as_slice() {
        state.target = Some(*only);
        request_verbs(&mut game.0, *only);
    }
}

fn menu_row(
    commands: &mut Commands,
    parent: Entity,
    style: &UiStyle,
    text: String,
    color: Color,
    action: UiAction,
    tooltip: Option<String>,
) {
    let mut row = commands.spawn((
        ChildOf(parent),
        Node {
            padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)),
            min_width: Val::Px(160.0),
            ..default()
        },
        BackgroundColor(ROW),
        Interaction::default(),
        action,
    ));
    if let Some(tooltip) = tooltip {
        row.insert(Tooltip(tooltip));
    }
    let row = row.id();
    commands
        .spawn((ChildOf(row), style.text(text, 14.0)))
        .insert(TextColor(color));
}

#[allow(clippy::too_many_arguments)]
fn build_menu(
    mut commands: Commands,
    game: Res<Game>,
    style: Res<UiStyle>,
    mut state: ResMut<MenuState>,
    menus: Query<Entity, With<ContextMenu>>,
    mut picks: Local<Option<std::sync::Arc<std::sync::Mutex<Option<bevy_ecs::entity::Entity>>>>>,
) {
    let world = &game.0;
    let picked = picks.get_or_insert_with(Default::default).clone();
    // Выбор сущности в списке приходит из действия строки.
    if let Some(target) = picked.lock().ok().and_then(|mut slot| slot.take()) {
        state.target = Some(target);
        state.built = None;
        state.dirty = true;
    }
    let revision = world
        .get_resource::<VerbMenuData>()
        .map_or(0, |menu| menu.revision);
    let verbs_ready = state.target.is_some()
        && world
            .get_resource::<VerbMenuData>()
            .is_some_and(|menu| menu.target == state.target)
        && state.built != Some(revision);
    if !state.dirty && !verbs_ready {
        return;
    }
    state.dirty = false;
    for menu in &menus {
        commands.entity(menu).despawn();
    }
    if !state.open {
        return;
    }
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(state.at.x),
                top: Val::Px(state.at.y),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(1.0),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            BorderColor::all(BORDER),
            GlobalZIndex(950),
            ContextMenu,
        ))
        .id();
    match state.target {
        None => {
            for entity in state.entities.clone() {
                let slot = picked.clone();
                let action = UiAction::new(move |world| {
                    if let Ok(mut slot) = slot.lock() {
                        *slot = Some(entity);
                    }
                    request_verbs(world, entity);
                });
                menu_row(
                    &mut commands,
                    panel,
                    &style,
                    world.entity_name(entity),
                    Color::WHITE,
                    action,
                    None,
                );
            }
        }
        Some(target) => {
            commands.spawn((
                ChildOf(panel),
                Node {
                    padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)),
                    ..default()
                },
                children![style.text(world.entity_name(target), 15.0)],
            ));
            // Осмотр — глагол клиента (`ExamineSystem.AddExamineVerb`).
            let examine = UiAction::new(move |world| {
                if let Some(net) = world.get::<NetEntity>(target).copied() {
                    world.send_net_event_to_server(&ExamineMessage::Request(net));
                }
            });
            let examine_text = crate::examine::loc(world, "examine-verb-name", &[]);
            menu_row(
                &mut commands,
                panel,
                &style,
                examine_text,
                Color::WHITE,
                examine,
                None,
            );
            if let Some(menu) = world
                .get_resource::<VerbMenuData>()
                .filter(|menu| menu.target == Some(target))
            {
                state.built = Some(menu.revision);
                for verb in &menu.verbs {
                    let text = match &verb.key.category {
                        Some(category) => format!("{category}: {}", verb.key.text),
                        None => verb.key.text.clone(),
                    };
                    let key = verb.key.clone();
                    let disabled = verb.disabled;
                    let action = UiAction::new(move |world| {
                        if disabled {
                            return;
                        }
                        if let Some(net) = world.get::<NetEntity>(target).copied() {
                            world.send_net_event_to_server(&VerbMessage::Execute {
                                target: net,
                                verb: key.clone(),
                            });
                        }
                    });
                    let color = if verb.disabled {
                        DISABLED
                    } else {
                        Color::WHITE
                    };
                    menu_row(
                        &mut commands,
                        panel,
                        &style,
                        text,
                        color,
                        action,
                        verb.message.clone(),
                    );
                }
            }
        }
    }
}

/// Строка меню нажата: глагол выполнен, меню закрывается.
#[allow(clippy::type_complexity)]
fn close_menu_after_choice(
    rows: Query<(&Interaction, &ChildOf), (Changed<Interaction>, With<UiAction>)>,
    menus: Query<(), With<ContextMenu>>,
    mut state: ResMut<MenuState>,
) {
    for (interaction, parent) in &rows {
        if *interaction == Interaction::Pressed
            && menus.contains(parent.parent())
            && state.target.is_some()
        {
            state.open = false;
            state.dirty = true;
        }
    }
}

/// Подсказки ячеек рук: название предмета.
fn hand_tooltips(
    game: Res<Game>,
    mut commands: Commands,
    slots: Query<(Entity, &crate::hands_hud::HandSlot), Without<Tooltip>>,
) {
    let world = &game.0;
    let Some(user) = world.resource::<ClientNet>().attached else {
        return;
    };
    for (slot, hand) in &slots {
        if let Some(item) = world.held_item(user, &hand.0) {
            commands
                .entity(slot)
                .insert(Tooltip(world.entity_name(item)));
        }
    }
}

/// Слой поверх мира.
pub struct OverlaysPlugin;

impl Plugin for OverlaysPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MenuState>().add_systems(
            Update,
            (
                spawn_popups,
                move_popups,
                sync_bars,
                show_examine,
                close_examine,
                open_menu,
                close_menu_after_choice,
                build_menu,
                hand_tooltips,
            )
                .chain(),
        );
    }
}
