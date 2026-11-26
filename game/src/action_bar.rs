//! Горячая панель действий внизу экрана: десять ячеек, значок действия, номер клавиши,
//! рамка у включённого, затемнение на перезарядке. Клик или клавиша — выполнить; действие с
//! целью ждёт клика по сущности или точке, правый клик отменяет. Образец —
//! `ActionUIController` и `ActionButton` клиента SS14 @ 2f4a2aad.

use astrocore_client::{
    Cursor, Game, GameResources, Hovered, Textures, Tooltip, UiAction, UiHover, UiStyle,
};
use astrocore_ecs::GameTime;
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::{MapCoordinates, MapWorldExt, Transform, TransformWorldExt};
use astrocore_net::ClientNet;
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;

use crate::actions::{
    Action, ActionBar, ActionTargeting, HOTBAR_SLOTS, activate_slot, request_action,
    sync_action_bar,
};
use crate::hands_hud::icon;

const SLOT: f32 = 44.0;
const GAP: f32 = 4.0;
const SLOT_COLOR: Color = Color::srgba(0.11, 0.12, 0.15, 0.85);
const BORDER: Color = Color::srgb(0.30, 0.33, 0.42);
const TOGGLED: Color = Color::srgb(0.95, 0.75, 0.25);
const TARGETING: Color = Color::srgb(0.35, 0.80, 0.45);
const COOLDOWN: Color = Color::srgba(0.0, 0.0, 0.0, 0.6);

#[derive(Component)]
struct ActionBarPanel;

/// Ячейка: действие, включено, на перезарядке, ждёт цели, значок загружен.
type SlotState = (bevy_ecs::entity::Entity, bool, bool, bool, bool);

/// Что нарисовано.
#[derive(Default, PartialEq)]
struct Shown(Vec<Option<SlotState>>);

#[allow(clippy::too_many_arguments)]
fn sync_panel(
    mut commands: Commands,
    mut game: ResMut<Game>,
    style: Res<UiStyle>,
    resources: Option<Res<GameResources>>,
    mut textures: ResMut<Textures>,
    mut images: ResMut<Assets<Image>>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>,
    mut shown: Local<Shown>,
    panels: Query<Entity, With<ActionBarPanel>>,
) {
    sync_action_bar(&mut game.0);
    let Some(resources) = resources else {
        return;
    };
    let world = &game.0;
    let bar = world
        .get_resource::<ActionBar>()
        .cloned()
        .unwrap_or_default();
    let targeting = world
        .get_resource::<ActionTargeting>()
        .and_then(|t| t.action);
    let now = server_time(world);
    let state = |action: bevy_ecs::entity::Entity,
                 textures: &mut Textures,
                 images: &mut Assets<Image>,
                 layouts: &mut Assets<TextureAtlasLayout>| {
        let component = world.get::<Action>(action);
        let toggled = component.is_some_and(|a| a.toggled);
        let cooling = component
            .and_then(|a| a.cooldown)
            .is_some_and(|c| c.end > now);
        let loaded = icon(world, action, &resources, textures, images, layouts).is_some();
        (action, toggled, cooling, targeting == Some(action), loaded)
    };
    let now_shown = Shown(
        bar.slots
            .iter()
            .map(|slot| slot.map(|action| state(action, &mut textures, &mut images, &mut layouts)))
            .collect(),
    );
    if now_shown == *shown {
        return;
    }
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    if bar.slots.iter().all(Option::is_none) {
        *shown = now_shown;
        return;
    }
    let width = HOTBAR_SLOTS as f32 * (SLOT + GAP);
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                // Над панелью рук.
                bottom: Val::Px(84.0),
                left: Val::Percent(50.0),
                margin: UiRect::left(Val::Px(-width / 2.0)),
                width: Val::Px(width),
                height: Val::Px(SLOT),
                column_gap: Val::Px(GAP),
                ..default()
            },
            ActionBarPanel,
        ))
        .id();
    for (index, slot) in now_shown.0.iter().enumerate() {
        let border = match slot {
            Some((_, _, _, true, _)) => TARGETING,
            Some((_, true, _, _, _)) => TOGGLED,
            _ => BORDER,
        };
        let tooltip = slot
            .map(|(action, ..)| world.entity_name(action))
            .unwrap_or_default();
        let cell = commands
            .spawn((
                ChildOf(panel),
                Node {
                    width: Val::Px(SLOT),
                    height: Val::Px(SLOT),
                    border: UiRect::all(Val::Px(2.0)),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                BackgroundColor(SLOT_COLOR),
                BorderColor::all(border),
                Interaction::default(),
                Tooltip(tooltip),
                UiAction::new(move |world| activate_slot(world, index)),
            ))
            .id();
        if let Some((action, _, cooling, _, _)) = slot
            && let Some(image) = icon(
                world,
                *action,
                &resources,
                &mut textures,
                &mut images,
                &mut layouts,
            )
        {
            commands.spawn((
                ChildOf(cell),
                image,
                Node {
                    width: Val::Px(SLOT - 8.0),
                    height: Val::Px(SLOT - 8.0),
                    ..default()
                },
            ));
            if *cooling {
                commands.spawn((
                    ChildOf(cell),
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(COOLDOWN),
                ));
            }
        }
        commands.spawn((
            ChildOf(cell),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(2.0),
                top: Val::Px(0.0),
                ..default()
            },
            style.text(((index + 1) % 10).to_string(), 10.0),
        ));
    }
    *shown = now_shown;
}

/// Время сервера по последнему применённому состоянию.
fn server_time(world: &bevy_ecs::world::World) -> f64 {
    let tick = world
        .get_resource::<ClientNet>()
        .map_or(0, |net| net.applied.0);
    let rate = world
        .get_resource::<GameTime>()
        .map_or(30, |time| time.tick_rate);
    f64::from(tick) / f64::from(rate.max(1))
}

/// Клик в режиме выбора цели: левый — цель, правый — отмена. Нажатие не уходит дальше.
fn target_click(
    mut game: ResMut<Game>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    cursor: Res<Cursor>,
    hovered: Res<Hovered>,
    ui_hover: Res<UiHover>,
) {
    let world = &mut game.0;
    let Some(action) = world
        .get_resource::<ActionTargeting>()
        .and_then(|t| t.action)
    else {
        return;
    };
    if mouse.just_pressed(MouseButton::Right) {
        mouse.clear_just_pressed(MouseButton::Right);
        world.insert_resource(ActionTargeting::default());
        return;
    }
    if !mouse.just_pressed(MouseButton::Left) || ui_hover.0 {
        return;
    }
    mouse.clear_just_pressed(MouseButton::Left);
    let player = world
        .get_resource::<ClientNet>()
        .and_then(|net| net.attached);
    let at = player.zip(cursor.world).and_then(|(player, position)| {
        let map = world.map_id_of(world.get::<Transform>(player)?.map()?)?;
        let z = world.world_transform(player)?.level;
        Some(MapCoordinates { map, position, z })
    });
    let target = hovered.entities.first().copied();
    request_action(world, action, target, at);
    let repeat = world
        .get::<crate::actions::TargetAction>(action)
        .is_some_and(|t| t.repeat);
    if !repeat {
        world.insert_resource(ActionTargeting::default());
    }
}

/// Горячая панель действий.
pub struct ActionBarPlugin;

impl Plugin for ActionBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_panel);
        app.add_systems(PreUpdate, target_click.after(bevy::input::InputSystems));
    }
}
