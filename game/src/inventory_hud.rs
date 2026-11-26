//! Окно слотов инвентаря в левом нижнем углу: ячейки по местам `uiWindowPos` шаблона, в
//! ячейке — значок надетой вещи. Клик по ячейке — `UseSlot` на сервер. Образец —
//! `InventoryUIController` клиента SS14 @ 2f4a2aad.

use astrocore_client::{Game, GameResources, Textures, Tooltip, UiAction, UiStyle};
use astrocore_loc::EntityNameWorldExt;
use astrocore_net::{ClientNet, NetEventWorldExt};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;

use crate::hands_hud::icon;
use crate::inventory::{Inventory, InventoryWorldExt, UseSlot};

const SLOT: f32 = 48.0;
const GAP: f32 = 4.0;
const SLOT_COLOR: Color = Color::srgba(0.11, 0.12, 0.15, 0.85);
const BORDER: Color = Color::srgb(0.30, 0.33, 0.42);

#[derive(Component)]
struct InventoryPanel;

/// Что нарисовано: слоты и вещи в них.
#[derive(Default, PartialEq)]
struct Shown(Vec<(String, Option<bevy_ecs::entity::Entity>)>);

#[allow(clippy::too_many_arguments)]
fn sync_panel(
    mut commands: Commands,
    game: Res<Game>,
    style: Res<UiStyle>,
    resources: Option<Res<GameResources>>,
    mut textures: ResMut<Textures>,
    mut images: ResMut<Assets<Image>>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>,
    mut shown: Local<Shown>,
    panels: Query<Entity, With<InventoryPanel>>,
) {
    let world = &game.0;
    let user = world.resource::<ClientNet>().attached;
    let slots = user
        .and_then(|user| world.get::<Inventory>(user))
        .map(|inventory| inventory.slots.clone())
        .unwrap_or_default();
    let now = Shown(
        slots
            .iter()
            .filter(|slot| slot.show_in_window)
            .map(|slot| {
                (
                    slot.name.clone(),
                    user.and_then(|user| world.slot_entity(user, &slot.name)),
                )
            })
            .collect(),
    );
    if now == *shown {
        return;
    }
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    let Some(resources) = resources else {
        return;
    };
    if now.0.is_empty() {
        *shown = now;
        return;
    }
    let rows = slots
        .iter()
        .map(|slot| slot.ui_window_pos.y)
        .max()
        .unwrap_or(0)
        + 1;
    let columns = slots
        .iter()
        .map(|slot| slot.ui_window_pos.x)
        .max()
        .unwrap_or(0)
        + 1;
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                bottom: Val::Px(12.0),
                width: Val::Px(columns as f32 * (SLOT + GAP)),
                height: Val::Px(rows as f32 * (SLOT + GAP)),
                ..default()
            },
            InventoryPanel,
        ))
        .id();
    for slot in slots.iter().filter(|slot| slot.show_in_window) {
        let worn = user.and_then(|user| world.slot_entity(user, &slot.name));
        let name = slot.name.clone();
        let tooltip = match worn {
            Some(item) => world.entity_name(item),
            None => slot.display_name.clone(),
        };
        let cell = commands
            .spawn((
                ChildOf(panel),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(slot.ui_window_pos.x as f32 * (SLOT + GAP)),
                    bottom: Val::Px(slot.ui_window_pos.y as f32 * (SLOT + GAP)),
                    width: Val::Px(SLOT),
                    height: Val::Px(SLOT),
                    border: UiRect::all(Val::Px(1.0)),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                BackgroundColor(SLOT_COLOR),
                BorderColor::all(BORDER),
                Interaction::default(),
                Tooltip(tooltip),
                UiAction::new(move |world| {
                    world.send_net_event_to_server(&UseSlot { slot: name.clone() });
                }),
            ))
            .id();
        match worn.and_then(|item| {
            icon(
                world,
                item,
                &resources,
                &mut textures,
                &mut images,
                &mut layouts,
            )
        }) {
            Some(image) => {
                commands.spawn((
                    ChildOf(cell),
                    image,
                    Node {
                        width: Val::Px(SLOT - 6.0),
                        height: Val::Px(SLOT - 6.0),
                        ..default()
                    },
                ));
            }
            None => {
                let short: String = slot.display_name.chars().take(4).collect();
                commands.spawn((ChildOf(cell), style.text(short, 10.0)));
            }
        }
    }
    *shown = now;
}

/// Окно слотов инвентаря.
pub struct InventoryHudPlugin;

impl Plugin for InventoryHudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_panel);
    }
}
