//! Панель значков тревог на клиенте: столбец справа под чатом, порядок по `alertOrder`,
//! значок уровня (вдвое крупнее), подсказка с названием и описанием, затемнение отсчёта,
//! нажатие уходит серверу. Образец — `AlertsUIController`, `AlertsUI`, `AlertControl`,
//! `ClientAlertsSystem` клиента SS14 @ 2f4a2aad.

use std::time::Duration;

use astrocore_client::{Game, GameResources, Textures, Tooltip, UiAction};
use astrocore_ecs::GameTime;
use astrocore_net::{ClientNet, NetEventWorldExt};
use astrocore_prototypes::Prototypes;
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;

use crate::alerts::{AlertIcon, AlertOrder, AlertPrototype, AlertState, Alerts, ClickAlert};
use crate::examine::loc;

/// Сторона значка: 32 пикселя RSI вдвое крупнее.
const ICON: f32 = 64.0;
const COOLDOWN: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);

#[derive(Component)]
struct AlertsPanel;

/// Затемнение отсчёта значка: начало и конец по часам игры.
#[derive(Component)]
struct CooldownShade(Duration, Duration);

/// Значки по порядку панели (`SyncAlerts`).
pub fn sorted_alerts(
    prototypes: &Prototypes,
    order: &AlertOrder,
    alerts: &std::collections::BTreeMap<String, AlertState>,
) -> Vec<(AlertPrototype, AlertState)> {
    let mut list: Vec<(AlertPrototype, AlertState)> = alerts
        .values()
        .filter_map(|state| {
            crate::alerts::alert_prototype(prototypes, &state.alert_type)
                .map(|alert| (alert, state.clone()))
        })
        .collect();
    list.sort_by(|a, b| order.compare(&a.0, &b.0));
    list
}

fn icon_image(
    icon: &AlertIcon,
    resources: &GameResources,
    textures: &mut Textures,
    images: &mut Assets<Image>,
    layouts: &mut Assets<TextureAtlasLayout>,
) -> Option<ImageNode> {
    match icon {
        AlertIcon::Rsi { sprite, state } => {
            let texture = textures.state(
                &resources.0,
                images,
                layouts,
                sprite.trim_start_matches('/'),
                state,
            )?;
            let index = texture.state.sheet_index(0, 0);
            Some(ImageNode::from_atlas_image(
                texture.image.clone(),
                TextureAtlas {
                    layout: texture.layout.clone(),
                    index,
                },
            ))
        }
        AlertIcon::Texture(path) => {
            let (image, _) = textures.single(&resources.0, images, path.trim_start_matches('/'))?;
            Some(ImageNode::new(image))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_panel(
    mut commands: Commands,
    game: Res<Game>,
    resources: Option<Res<GameResources>>,
    mut textures: ResMut<Textures>,
    mut images: ResMut<Assets<Image>>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>,
    mut shown: Local<Option<std::collections::BTreeMap<String, AlertState>>>,
    mut order: Local<Option<AlertOrder>>,
    panels: Query<Entity, With<AlertsPanel>>,
) {
    let world = &game.0;
    let alerts = world
        .resource::<ClientNet>()
        .attached
        .and_then(|entity| world.get::<Alerts>(entity))
        .map(|a| a.alerts.clone())
        .unwrap_or_default();
    if shown.as_ref() == Some(&alerts) {
        return;
    }
    let (Some(resources), Some(prototypes)) = (resources, world.get_resource::<Prototypes>())
    else {
        return;
    };
    *shown = Some(alerts.clone());
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    let order = order.get_or_insert_with(|| AlertOrder::from_prototypes(prototypes));
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(8.0),
                top: Val::Px(300.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            AlertsPanel,
        ))
        .id();
    for (alert, state) in sorted_alerts(prototypes, order, &alerts) {
        let Some(image) = alert.icon(state.severity).and_then(|icon| {
            icon_image(icon, &resources, &mut textures, &mut images, &mut layouts)
        }) else {
            continue;
        };
        let tooltip = format!(
            "{}\n{}",
            crate::examine::strip_markup(&loc(world, &alert.name, &[])),
            crate::examine::strip_markup(&loc(world, &alert.description, &[]))
        );
        let alert_type = alert.id.clone();
        let button = commands
            .spawn((
                ChildOf(panel),
                Node {
                    width: Val::Px(ICON),
                    height: Val::Px(ICON),
                    ..default()
                },
                Interaction::default(),
                Tooltip(tooltip),
                UiAction::new(move |world| {
                    world.send_net_event_to_server(&ClickAlert {
                        alert_type: alert_type.clone(),
                    });
                }),
            ))
            .id();
        commands.spawn((
            ChildOf(button),
            image,
            Node {
                width: Val::Px(ICON),
                height: Val::Px(ICON),
                ..default()
            },
        ));
        if let Some((start, end)) = state.cooldown.filter(|_| state.show_cooldown) {
            commands.spawn((
                ChildOf(button),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    width: Val::Px(ICON),
                    height: Val::Px(0.0),
                    ..default()
                },
                BackgroundColor(COOLDOWN),
                CooldownShade(start, end),
            ));
        }
    }
}

/// Доля оставшегося отсчёта.
pub fn cooldown_left(now: Duration, start: Duration, end: Duration) -> f32 {
    if end <= start || now >= end {
        return 0.0;
    }
    (end.saturating_sub(now).as_secs_f32() / (end - start).as_secs_f32()).clamp(0.0, 1.0)
}

fn update_cooldowns(game: Res<Game>, mut shades: Query<(&CooldownShade, &mut Node)>) {
    let now = game
        .0
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default();
    for (shade, mut node) in &mut shades {
        let height = Val::Px(ICON * cooldown_left(now, shade.0, shade.1));
        if node.height != height {
            node.height = height;
        }
    }
}

/// Панель значков тревог.
pub struct AlertsUiPlugin;

impl Plugin for AlertsUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (sync_panel, update_cooldowns).chain());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_shade_shrinks_to_nothing() {
        let (start, end) = (Duration::from_secs(10), Duration::from_secs(20));
        assert_eq!(cooldown_left(Duration::from_secs(10), start, end), 1.0);
        assert_eq!(cooldown_left(Duration::from_secs(15), start, end), 0.5);
        assert_eq!(cooldown_left(Duration::from_secs(25), start, end), 0.0);
    }
}
