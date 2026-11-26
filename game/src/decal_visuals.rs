//! Отрисовка декалей на клиенте: каждая декаль грида — спрайт её прототипа над тайлами и под
//! сущностями, с цветом и поворотом. Образец — `DecalOverlay` клиента SS14 @ 2f4a2aad.

use std::collections::HashMap;

use astrocore_client::{Eye, Game, GameResources, Textures};
use astrocore_data::DataNode;
use astrocore_map::{MapGrid, TransformWorldExt};
use astrocore_prototypes::Prototypes;
use bevy::prelude::*;

use crate::decals::{Decal, GridDecals};

/// Глубина декалей: над тайлами (−500), под сущностями.
const DECAL_DEPTH: f32 = -400.0;

/// Набор и состояние рисунка декали из прототипа `decal`.
pub fn decal_sprite(prototypes: &Prototypes, id: &str) -> Option<(String, String)> {
    let node = prototypes.resolved_node("decal", id)?;
    let sprite = node.get("sprite")?;
    let rsi = sprite.get("sprite").and_then(DataNode::as_str)?;
    let state = sprite.get("state").and_then(DataNode::as_str)?;
    Some((rsi.trim_start_matches('/').to_owned(), state.to_owned()))
}

/// Спрайт декали в мире: размер клетки, цвет декали.
pub fn decal_image(
    world: &bevy_ecs::world::World,
    decal: &str,
    color: Option<&str>,
    resources: &GameResources,
    textures: &mut Textures,
    images: &mut Assets<Image>,
    layouts: &mut Assets<TextureAtlasLayout>,
) -> Option<Sprite> {
    let prototypes = world.get_resource::<Prototypes>()?;
    let (rsi, state) = decal_sprite(prototypes, decal)?;
    let texture = textures.state(&resources.0, images, layouts, &rsi, &state)?;
    let index = texture.state.sheet_index(0, 0);
    let mut sprite = Sprite::from_atlas_image(
        texture.image.clone(),
        TextureAtlas {
            layout: texture.layout.clone(),
            index,
        },
    );
    sprite.custom_size = Some(texture.size);
    if let Some(color) = color.and_then(astrocore_math::Color::parse) {
        sprite.color = Color::srgba(color.r, color.g, color.b, color.a);
    }
    Some(sprite)
}

#[derive(Component)]
struct DecalView;

/// Что нарисовано для грида: номер изменения, положение грида, спрайты.
struct Drawn {
    revision: u64,
    count: usize,
    placement: (Vec2, f32, i16),
    sprites: Vec<Entity>,
}

/// Перерисовывает декали гридов, у которых они изменились или грид сдвинулся.
#[allow(clippy::too_many_arguments)]
fn draw(
    mut commands: Commands,
    game: Res<Game>,
    eye: Res<Eye>,
    resources: Option<Res<GameResources>>,
    mut textures: ResMut<Textures>,
    mut images: ResMut<Assets<Image>>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>,
    mut drawn: Local<HashMap<bevy_ecs::entity::Entity, Drawn>>,
) {
    let Some(resources) = resources else {
        return;
    };
    let world = &game.0;
    let mut grids: Vec<(bevy_ecs::entity::Entity, GridDecalsRef)> = Vec::new();
    if let Some(mut query) = world.try_query::<(bevy_ecs::entity::Entity, &GridDecals, &MapGrid)>()
    {
        for (grid, decals, _) in query.iter(world) {
            let on_eye_map = world
                .get::<astrocore_map::Transform>(grid)
                .and_then(astrocore_map::Transform::map)
                .is_some_and(|map| Some(map) == eye.map);
            if on_eye_map {
                grids.push((grid, GridDecalsRef(decals.revision, decals.decals.len())));
            }
        }
    }
    // Гриды, которых больше нет на карте глаза.
    drawn.retain(|grid, state| {
        let keep = grids.iter().any(|(g, _)| g == grid);
        if !keep {
            for sprite in state.sprites.drain(..) {
                commands.entity(sprite).despawn();
            }
        }
        keep
    });
    for (grid, GridDecalsRef(revision, count)) in grids {
        let Some(transform) = world.world_transform(grid) else {
            continue;
        };
        let placement = (
            Vec2::new(transform.position.x, transform.position.y),
            transform.rotation.0,
            transform.level,
        );
        if let Some(state) = drawn.get(&grid)
            && state.revision == revision
            && state.count == count
            && state.placement == placement
        {
            continue;
        }
        if let Some(mut old) = drawn.remove(&grid) {
            for sprite in old.sprites.drain(..) {
                commands.entity(sprite).despawn();
            }
        }
        let Some(decals) = world.get::<GridDecals>(grid) else {
            continue;
        };
        let mut sprites = Vec::with_capacity(decals.decals.len());
        for decal in decals.decals.values() {
            let Some(sprite) = decal_image(
                world,
                &decal.id,
                decal.color.as_deref(),
                &resources,
                &mut textures,
                &mut images,
                &mut layouts,
            ) else {
                continue;
            };
            sprites.push(
                commands
                    .spawn((sprite, place(&transform, decal), DecalView))
                    .id(),
            );
        }
        drawn.insert(
            grid,
            Drawn {
                revision,
                count,
                placement,
                sprites,
            },
        );
    }
}

struct GridDecalsRef(u64, usize);

/// Место декали: центр её клетки в мире, поворот грида и декали.
fn place(grid: &astrocore_map::WorldTransform, decal: &Decal) -> Transform {
    let center = grid.to_world(decal.coordinates + astrocore_math::Vec2::splat(0.5));
    let z = DECAL_DEPTH + decal.z_index as f32 * 0.01;
    Transform::from_xyz(center.x, center.y, z)
        .with_rotation(Quat::from_rotation_z(grid.rotation.0 + decal.angle))
}

/// Декали на клиенте.
pub struct DecalVisualsPlugin;

impl Plugin for DecalVisualsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, draw);
    }
}
