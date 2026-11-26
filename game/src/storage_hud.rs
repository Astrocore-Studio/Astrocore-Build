//! Окно хранилища: сетка клеток, предметы по своим местам и поворотам. Клик по предмету
//! берёт его в руку или применяет к нему предмет из руки, клик по пустой клетке кладёт
//! предмет из руки в эту клетку. Образец — `StorageWindow` и `StorageUIController` клиента
//! SS14 @ 2f4a2aad.

use std::collections::{BTreeMap, HashMap};

use astrocore_client::{Game, GameResources, Textures, Tooltip, UiAction, WindowKinds};
use astrocore_loc::EntityNameWorldExt;
use astrocore_math::IVec2;
use astrocore_net::NetEntity;
use astrocore_ui::UiWorldExt;
use bevy::math::Rot2;
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;

use crate::hands_hud::icon;
use crate::storage::{
    CellBox, ItemStorageLocation, STORAGE_UI_KEY, Storage, StorageMessage, adjusted_shape,
    bounding, item_shape,
};

const CELL: f32 = 40.0;
const CELL_COLOR: Color = Color::srgba(0.16, 0.17, 0.21, 0.95);
const ITEM_COLOR: Color = Color::srgba(0.30, 0.34, 0.45, 0.55);
const ITEM_BORDER: Color = Color::srgb(0.55, 0.60, 0.75);
const BORDER: Color = Color::srgb(0.30, 0.33, 0.42);

/// Узел окна, в котором рисуется сетка хранилища.
#[derive(Component)]
struct StorageGrid(bevy_ecs::entity::Entity);

type ShownItems = BTreeMap<bevy_ecs::entity::Entity, (ItemStorageLocation, bool)>;

/// Что нарисовано в каждом окне: сетка и предметы (с признаком «значок загружен»).
#[derive(Default)]
struct Shown(HashMap<Entity, (Vec<CellBox>, ShownItems)>);

fn same(a: &ShownItems, b: &ShownItems) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|((ea, (la, ia)), (eb, (lb, ib)))| {
            ea == eb && la.position == lb.position && la.rotation.0 == lb.rotation.0 && ia == ib
        })
}

#[allow(clippy::too_many_arguments)]
fn sync_grids(
    mut commands: Commands,
    game: Res<Game>,
    resources: Option<Res<GameResources>>,
    mut textures: ResMut<Textures>,
    mut images: ResMut<Assets<Image>>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>,
    mut shown: Local<Shown>,
    grids: Query<(Entity, &StorageGrid)>,
) {
    let Some(resources) = resources else {
        return;
    };
    let world = &game.0;
    shown.0.retain(|node, _| grids.contains(*node));
    for (node, grid) in &grids {
        let Some(storage) = world.get::<Storage>(grid.0) else {
            continue;
        };
        let items: ShownItems = storage
            .stored_items
            .iter()
            .map(|(item, location)| {
                let loaded = icon(
                    world,
                    *item,
                    &resources,
                    &mut textures,
                    &mut images,
                    &mut layouts,
                )
                .is_some();
                (*item, (*location, loaded))
            })
            .collect();
        if shown
            .0
            .get(&node)
            .is_some_and(|(cells, old)| *cells == storage.grid && same(old, &items))
        {
            continue;
        }
        commands.entity(node).despawn_related::<Children>();
        let Some(bounds) = bounding(&storage.grid) else {
            continue;
        };
        commands.entity(node).insert(Node {
            position_type: PositionType::Relative,
            width: Val::Px(bounds.width() as f32 * CELL),
            height: Val::Px(bounds.height() as f32 * CELL),
            ..default()
        });
        let storage_entity = grid.0;
        // Строка 0 сетки — верхняя, как в окне SS14.
        let place = |cell: IVec2| {
            (
                (cell.x - bounds.left) as f32 * CELL,
                (cell.y - bounds.bottom) as f32 * CELL,
            )
        };
        for cell in storage.grid.iter().flat_map(CellBox::cells) {
            let (left, top) = place(cell);
            commands.spawn((
                ChildOf(node),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(left),
                    top: Val::Px(top),
                    width: Val::Px(CELL),
                    height: Val::Px(CELL),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(CELL_COLOR),
                BorderColor::all(BORDER),
                Interaction::default(),
                UiAction::new(move |world| {
                    let location = ItemStorageLocation::new(astrocore_math::Angle::ZERO, cell);
                    world.send_ui_message(
                        storage_entity,
                        STORAGE_UI_KEY,
                        &StorageMessage::InsertItemIntoLocation(location),
                    );
                }),
            ));
        }
        for (item, location) in &storage.stored_items {
            let Some(net) = world.get::<NetEntity>(*item).copied() else {
                continue;
            };
            let shape = item_shape(world, *item);
            let placed = adjusted_shape(&shape, location.rotation, location.position);
            let (Some(area), Some(own)) = (bounding(&placed), bounding(&shape)) else {
                continue;
            };
            let (left, top) = place(IVec2::new(area.left, area.bottom));
            let (width, height) = (area.width() as f32 * CELL, area.height() as f32 * CELL);
            let holder = commands
                .spawn((
                    ChildOf(node),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(left),
                        top: Val::Px(top),
                        width: Val::Px(width),
                        height: Val::Px(height),
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BackgroundColor(ITEM_COLOR),
                    BorderColor::all(ITEM_BORDER),
                    Interaction::default(),
                    Tooltip(world.entity_name(*item)),
                    UiAction::new(move |world| {
                        world.send_ui_message(
                            storage_entity,
                            STORAGE_UI_KEY,
                            &StorageMessage::InteractWithItem(net),
                        );
                    }),
                ))
                .id();
            let Some(image) = icon(
                world,
                *item,
                &resources,
                &mut textures,
                &mut images,
                &mut layouts,
            ) else {
                continue;
            };
            let stored_rotation = world
                .get::<crate::item::Item>(*item)
                .map_or(0.0, |item| item.stored_rotation);
            // Значок рисуется в своём положении и поворачивается вместе с предметом.
            let (own_width, own_height) = (own.width() as f32 * CELL, own.height() as f32 * CELL);
            commands.spawn((
                ChildOf(holder),
                image,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px((width - own_width) / 2.0),
                    top: Val::Px((height - own_height) / 2.0),
                    width: Val::Px(own_width),
                    height: Val::Px(own_height),
                    ..default()
                },
                UiTransform {
                    rotation: Rot2::radians(-(location.rotation.0 + stored_rotation.to_radians())),
                    ..default()
                },
            ));
        }
        shown.0.insert(node, (storage.grid.clone(), items));
    }
}

/// Окно хранилища.
pub struct StorageHudPlugin;

impl Plugin for StorageHudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WindowKinds>();
        app.world_mut().resource_mut::<WindowKinds>().register(
            "StorageBoundUserInterface",
            |commands, content, ui, _, _| {
                commands.spawn((ChildOf(content), Node::default(), StorageGrid(ui.entity)));
            },
        );
        app.add_systems(Update, sync_grids);
    }
}
