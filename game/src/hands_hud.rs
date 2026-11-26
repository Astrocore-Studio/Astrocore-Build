//! Панель рук внизу экрана: ячейка на руку, активная подсвечена, в ячейке — значок
//! предмета. Клик по ячейке — как в SS14 (`HandsUIController`, `UIHandClick`
//! `Content.Client/Hands/Systems/HandsSystem.cs` @ 2f4a2aad): по активной с предметом —
//! использовать предмет; по пустой другой — сделать её активной; по другой с предметом —
//! ударить по нему активным предметом или переложить его в пустую активную руку.

use astrocore_client::{
    Game, GameResources, LayerSource, Sprite as GameSprite, Textures, UiAction, UiStyle,
};
use astrocore_loc::EntityNameWorldExt;
use astrocore_net::{ClientNet, NetEventWorldExt};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;

use crate::hands::{HandLocation, Hands, HandsRequest, HandsWorldExt};

const SLOT: f32 = 64.0;
const SLOT_COLOR: Color = Color::srgba(0.11, 0.12, 0.15, 0.85);
const ACTIVE_COLOR: Color = Color::srgba(0.30, 0.36, 0.52, 0.95);
const BORDER: Color = Color::srgb(0.30, 0.33, 0.42);

/// Корень панели рук.
#[derive(Component)]
struct HandsPanel;

/// Предмет в ячейке: сущность и описание её первого слоя (значок меняется вместе с ним).
type ShownItem = (bevy_ecs::entity::Entity, Option<String>);

/// Что сейчас нарисовано: руки, активная, предметы в руках.
#[derive(Default, PartialEq)]
struct Shown(Vec<(String, bool, Option<ShownItem>)>);

/// Клик по ячейке руки (`UIHandClick`).
fn click_hand(world: &mut bevy_ecs::world::World, hand: &str) {
    let Some(user) = world.resource::<ClientNet>().attached else {
        return;
    };
    let Some(active) = world.active_hand(user) else {
        return;
    };
    let pressed_item = world.held_item(user, hand);
    let active_item = world.active_item(user);
    let request = match (hand == active, pressed_item, active_item) {
        (true, _, Some(_)) => HandsRequest::UseInHand,
        (false, None, _) => HandsRequest::SetHand(hand.to_owned()),
        (false, Some(_), Some(_)) => HandsRequest::InteractUsingInHand(hand.to_owned()),
        (false, Some(_), None) => HandsRequest::MoveItemToActive(hand.to_owned()),
        (true, _, None) => return,
    };
    world.send_net_event_to_server(&request);
}

/// Значок сущности: первый слой её спрайта, первое направление и кадр.
fn icon(
    world: &bevy_ecs::world::World,
    entity: bevy_ecs::entity::Entity,
    resources: &GameResources,
    textures: &mut Textures,
    images: &mut Assets<Image>,
    layouts: &mut Assets<TextureAtlasLayout>,
) -> Option<ImageNode> {
    let layer = world
        .get::<GameSprite>(entity)?
        .resolved_layers()
        .into_iter()
        .next()?;
    match layer.source {
        LayerSource::Rsi { rsi, state } => {
            let texture = textures.state(&resources.0, images, layouts, &rsi, &state)?;
            let index = texture.state.sheet_index(0, 0);
            Some(ImageNode::from_atlas_image(
                texture.image.clone(),
                TextureAtlas {
                    layout: texture.layout.clone(),
                    index,
                },
            ))
        }
        LayerSource::Texture(path) => {
            let (image, _) = textures.single(&resources.0, images, &path)?;
            Some(ImageNode::new(image))
        }
    }
}

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
    panels: Query<Entity, With<HandsPanel>>,
) {
    let world = &game.0;
    let user = world.resource::<ClientNet>().attached;
    let mut now = Shown::default();
    if let Some(user) = user
        && let Some(hands) = world.get::<Hands>(user)
    {
        for name in &hands.sorted_hands {
            let item = world.held_item(user, name).map(|item| {
                let state = world
                    .get::<GameSprite>(item)
                    .and_then(|sprite| sprite.resolved_layers().into_iter().next())
                    .map(|layer| format!("{:?}", layer.source));
                (item, state)
            });
            now.0.push((
                name.clone(),
                hands.active_hand_id.as_ref() == Some(name),
                item,
            ));
        }
    }
    if now == *shown {
        return;
    }
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    let Some(resources) = resources else {
        return;
    };
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(12.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                column_gap: Val::Px(6.0),
                ..default()
            },
            HandsPanel,
        ))
        .id();
    for (name, active, item) in &now.0 {
        let location = user
            .and_then(|user| world.get::<Hands>(user))
            .and_then(|hands| hands.hands.get(name))
            .map_or(HandLocation::Middle, |hand| hand.location);
        let label = match location {
            HandLocation::Left => "Л",
            HandLocation::Right => "П",
            HandLocation::Middle => "С",
        };
        let hand = name.clone();
        let slot = commands
            .spawn((
                ChildOf(panel),
                Node {
                    width: Val::Px(SLOT),
                    height: Val::Px(SLOT),
                    border: UiRect::all(Val::Px(if *active { 2.0 } else { 1.0 })),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                BackgroundColor(if *active { ACTIVE_COLOR } else { SLOT_COLOR }),
                BorderColor::all(BORDER),
                Interaction::default(),
                UiAction::new(move |world| click_hand(world, &hand)),
            ))
            .id();
        if let Some((item, _)) = item {
            if let Some(image) = icon(
                world,
                *item,
                &resources,
                &mut textures,
                &mut images,
                &mut layouts,
            ) {
                commands.spawn((
                    ChildOf(slot),
                    image,
                    Node {
                        width: Val::Px(SLOT - 8.0),
                        height: Val::Px(SLOT - 8.0),
                        ..default()
                    },
                ));
            } else {
                commands.spawn((ChildOf(slot), style.text(world.entity_name(*item), 10.0)));
            }
        }
        commands.spawn((
            ChildOf(slot),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(3.0),
                top: Val::Px(1.0),
                ..default()
            },
            style.text(label, 11.0),
        ));
    }
    *shown = now;
}

/// Панель рук.
pub struct HandsHudPlugin;

impl Plugin for HandsHudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_panel);
    }
}
