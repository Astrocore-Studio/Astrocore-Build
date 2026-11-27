//! Призрак на клиенте: свет (обычный, ночное зрение, полный), показ других призраков,
//! кнопки «Вернуться в тело», «Телепорт», окно телепорта к игрокам и точкам станции.
//! Образец — `GhostSystem` клиента (`OnToggleLighting`, `ApplyGhostVisibility`),
//! `NightVisionComponent` наблюдателя, `GhostUIController`, `GhostGui`, `GhostTargetWindow`
//! SS14 @ 2f4a2aad.
//!
//! Нет пока: кнопки ролей призрака (6.5г), поиска в окне телепорта, «самый популярный» и
//! «случайный, за кем следят» — нет следования.

use astrocore_client::{Game, Lighting, Sprite, UiAction, UiStyle};
use astrocore_net::{ClientNet, NetEventWorldExt};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy_ecs::world::World as GameWorld;

use crate::examine::loc;
use crate::ghost::{Ghost, GhostLighting, GhostMessage, GhostView, GhostVisibilityMode};

const PANEL: Color = Color::srgba(0.08, 0.09, 0.12, 0.92);
const BUTTON: Color = Color::srgba(0.16, 0.18, 0.24, 1.0);
const DISABLED: Color = Color::srgba(0.12, 0.12, 0.14, 0.8);

/// Свет ночного зрения наблюдателя (`NightVision.lightingColor: #FFFFFF20`).
pub const HALF_BRIGHT: f32 = 0x20 as f32 / 255.0;

/// Сущность игрока, если это призрак.
pub fn local_ghost(world: &GameWorld) -> Option<bevy_ecs::entity::Entity> {
    let entity = world.get_resource::<ClientNet>()?.attached?;
    world.get::<Ghost>(entity).map(|_| entity)
}

/// Свет мира для режима призрака.
pub fn lighting_for(mode: GhostLighting) -> Lighting {
    let mut lighting = Lighting::default();
    match mode {
        GhostLighting::Normal => {}
        GhostLighting::HalfBright => {
            lighting.ambient =
                astrocore_math::Color::rgba(HALF_BRIGHT, HALF_BRIGHT, HALF_BRIGHT, 1.0);
        }
        GhostLighting::Fullbright => lighting.enabled = false,
    }
    lighting
}

/// Виден ли призрак при режиме показа (`GetGhostVisible`).
pub fn ghost_visible(mode: GhostVisibilityMode, own: bool) -> bool {
    match mode {
        GhostVisibilityMode::ShowAllGhosts => true,
        GhostVisibilityMode::HideOtherGhosts => own,
        GhostVisibilityMode::HideOtherGhostsAndSelf => false,
    }
}

/// Свет и видимость призраков по состоянию: вне призрака — обычный свет и все видны.
fn apply_view(mut game: ResMut<Game>, mut lighting: ResMut<Lighting>) {
    let world = &mut game.0;
    let ghost = local_ghost(world);
    if ghost.is_none() {
        let mut view = world.get_resource_or_insert_with(GhostView::default);
        if view.lighting != GhostLighting::Normal
            || view.visibility != GhostVisibilityMode::ShowAllGhosts
            || view.warp_window
        {
            view.lighting = GhostLighting::Normal;
            view.visibility = GhostVisibilityMode::ShowAllGhosts;
            view.warp_window = false;
            view.version += 1;
        }
    }
    let view = world
        .get_resource_or_insert_with(GhostView::default)
        .clone();
    let wanted = lighting_for(view.lighting);
    if lighting.enabled != wanted.enabled || lighting.ambient != wanted.ambient {
        lighting.enabled = wanted.enabled;
        lighting.ambient = wanted.ambient;
    }
    let ghosts: Vec<bevy_ecs::entity::Entity> = world
        .query_filtered::<bevy_ecs::entity::Entity, bevy_ecs::query::With<Ghost>>()
        .iter(world)
        .collect();
    for entity in ghosts {
        let visible = ghost_visible(view.visibility, Some(entity) == ghost);
        if let Some(mut sprite) = world.get_mut::<Sprite>(entity)
            && sprite.visible != visible
        {
            sprite.visible = visible;
        }
    }
}

#[derive(Component)]
struct GhostPanel;

fn button(
    commands: &mut Commands,
    style: &UiStyle,
    parent: Entity,
    text: String,
    enabled: bool,
    action: UiAction,
) {
    let mut node = commands.spawn((
        ChildOf(parent),
        Node {
            padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
            ..default()
        },
        BackgroundColor(if enabled { BUTTON } else { DISABLED }),
    ));
    if enabled {
        node.insert((Interaction::default(), action));
    }
    let id = node.id();
    commands.spawn((ChildOf(id), style.text(text, 14.0)));
}

/// Кнопки призрака и окно телепорта (`GhostGui`, `GhostTargetWindow`).
fn draw(
    mut commands: Commands,
    mut game: ResMut<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<GhostPanel>>,
    mut shown: Local<Option<(Option<bevy_ecs::entity::Entity>, bool, u64)>>,
) {
    let world = &mut game.0;
    let ghost = local_ghost(world);
    let can_return = ghost.is_some_and(|g| crate::ghost::can_return_to_body(world, g));
    let view = world
        .get_resource_or_insert_with(GhostView::default)
        .clone();
    let now = (ghost, can_return, view.version);
    if shown.as_ref() == Some(&now) {
        return;
    }
    *shown = Some(now);
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    if ghost.is_none() {
        return;
    }
    let bar = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(12.0),
                left: Val::Percent(50.0),
                margin: UiRect::left(Val::Px(-160.0)),
                column_gap: Val::Px(6.0),
                padding: UiRect::all(Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            GlobalZIndex(55),
            GhostPanel,
        ))
        .id();
    button(
        &mut commands,
        &style,
        bar,
        loc(world, "ghost-gui-return-to-body-button", &[]),
        can_return,
        UiAction::new(|world| world.send_net_event_to_server(&GhostMessage::ReturnToBody)),
    );
    button(
        &mut commands,
        &style,
        bar,
        loc(world, "ghost-gui-ghost-warp-button", &[]),
        true,
        UiAction::new(|world| {
            let mut view = world.get_resource_or_insert_with(GhostView::default);
            view.warp_window = !view.warp_window;
            view.version += 1;
            let open = view.warp_window;
            if open {
                world.send_net_event_to_server(&GhostMessage::WarpsRequest);
            }
        }),
    );
    if !view.warp_window {
        return;
    }
    let window = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(60.0),
                left: Val::Percent(50.0),
                // Левее центра: справа сверху — окно чата.
                margin: UiRect::left(Val::Px(-400.0)),
                width: Val::Px(560.0),
                max_height: Val::Percent(75.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                padding: UiRect::all(Val::Px(10.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(PANEL),
            GlobalZIndex(56),
            Interaction::default(),
            GhostPanel,
        ))
        .id();
    commands.spawn((
        ChildOf(window),
        style.text(loc(world, "ghost-target-window-title", &[]), 16.0),
    ));
    let quick = commands
        .spawn((
            ChildOf(window),
            Node {
                column_gap: Val::Px(6.0),
                ..default()
            },
        ))
        .id();
    button(
        &mut commands,
        &style,
        quick,
        loc(world, "ghost-target-window-warp-to-random", &[]),
        true,
        UiAction::new(|world| world.send_net_event_to_server(&GhostMessage::WarpToRandom)),
    );
    let list = commands
        .spawn((
            ChildOf(window),
            Node {
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(4.0),
                row_gap: Val::Px(4.0),
                ..default()
            },
        ))
        .id();
    for warp in view.warps {
        let name = if warp.is_warp_point {
            loc(
                world,
                "ghost-target-window-current-button",
                &[(
                    "name",
                    astrocore_loc::LocArg::from(warp.display_name.clone()),
                )],
            )
        } else {
            warp.display_name.clone()
        };
        let target = warp.entity;
        button(
            &mut commands,
            &style,
            list,
            name,
            true,
            UiAction::new(move |world| {
                world.send_net_event_to_server(&GhostMessage::WarpToTarget(target));
            }),
        );
    }
}

/// Призрак на клиенте.
pub struct GhostUiPlugin;

impl Plugin for GhostUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (apply_view, draw));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighting_modes_and_ghost_visibility() {
        assert!(lighting_for(GhostLighting::Normal).enabled);
        assert_eq!(
            lighting_for(GhostLighting::Normal).ambient,
            Lighting::default().ambient
        );
        let half = lighting_for(GhostLighting::HalfBright);
        assert!(half.enabled && (half.ambient.r - HALF_BRIGHT).abs() < 1e-6);
        assert!(!lighting_for(GhostLighting::Fullbright).enabled);
        assert!(ghost_visible(GhostVisibilityMode::ShowAllGhosts, false));
        assert!(ghost_visible(GhostVisibilityMode::HideOtherGhosts, true));
        assert!(!ghost_visible(GhostVisibilityMode::HideOtherGhosts, false));
        assert!(!ghost_visible(
            GhostVisibilityMode::HideOtherGhostsAndSelf,
            true
        ));
    }

    #[test]
    fn client_actions_cycle_and_popup() {
        let mut world = GameWorld::new();
        let ghost = world.spawn(Ghost::default()).id();
        crate::ghost::client_action(&mut world, ghost, "ToggleLightingActionEvent");
        assert_eq!(
            world.resource::<GhostView>().lighting,
            GhostLighting::HalfBright
        );
        crate::ghost::client_action(&mut world, ghost, "ToggleLightingActionEvent");
        crate::ghost::client_action(&mut world, ghost, "ToggleLightingActionEvent");
        assert_eq!(
            world.resource::<GhostView>().lighting,
            GhostLighting::Normal
        );
        crate::ghost::client_action(&mut world, ghost, "ToggleGhostsActionEvent");
        assert_eq!(
            world.resource::<GhostView>().visibility,
            GhostVisibilityMode::HideOtherGhosts
        );
        let popups = &world.resource::<crate::popups::PopupQueue>().0;
        assert_eq!(popups.len(), 4);
        assert_eq!(
            popups[0].message,
            "ghost-gui-toggle-lighting-manager-popup-half-bright"
        );
        // Не призрак — ничего.
        let body = world.spawn_empty().id();
        crate::ghost::client_action(&mut world, body, "ToggleLightingActionEvent");
        assert_eq!(world.resource::<crate::popups::PopupQueue>().0.len(), 4);
    }
}
