use super::{Ground, Hud};
use astrocore_client::Net;
use astrocore_core::ObjectId;
use astrocore_math::GridCell;
use astrocore_world::{SlotKey, World as Station};
use bevy::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::FRAC_PI_2;

#[derive(Clone, PartialEq)]
struct Sprite {
    rsi: &'static str,
    state: String,
    tint: [u8; 4],
    at: Vec3,
    rotation: u8,
}

#[derive(Resource, Default)]
pub(super) struct Visuals {
    revision: Option<u64>,
    mesh: Option<Handle<Mesh>>,
    materials: BTreeMap<(&'static str, String, [u8; 4]), Handle<StandardMaterial>>,
    // One cable can have both a coloured wire and white stripes.
    placed: BTreeMap<(ObjectId, u8), (Sprite, Entity)>,
    preview: Option<(Sprite, Entity)>,
}

fn pipe_sprite(id: &str, at: Vec3, rotation: u8) -> Option<Sprite> {
    if let Some((rsi, state)) = gameplay::crafting::pipe_appearance(id) {
        return Some(Sprite {
            rsi,
            state: state.into(),
            at,
            rotation,
            tint: [255; 4],
        });
    }
    let shape = id.strip_prefix("GasPipe")?;
    let (shape, rsi, layer) = if let Some(shape) = shape.strip_suffix("Alt1") {
        (shape, "Structures/Piping/Atmospherics/pipe_alt1.rsi", 1)
    } else if let Some(shape) = shape.strip_suffix("Alt2") {
        (shape, "Structures/Piping/Atmospherics/pipe_alt2.rsi", 2)
    } else {
        (shape, "Structures/Piping/Atmospherics/pipe.rsi", 0)
    };
    Some(Sprite {
        rsi,
        state: format!("pipe{shape}"),
        at: at + Vec3::Y * (layer as f32 * 0.001),
        rotation,
        tint: [255; 4],
    })
}

fn visible_sprites(world: &Station) -> BTreeMap<(ObjectId, u8), Sprite> {
    let mut sprites = BTreeMap::new();
    let mut cables = BTreeSet::new();
    for o in world.authored() {
        if let Some(group) = o.metadata.get("power_cable") {
            let at = (o.min() + o.max()) * 0.5;
            cables.insert((
                o.floor.0,
                at.x.floor() as i32,
                at.z.floor() as i32,
                group.as_str(),
            ));
        }
    }
    for o in world
        .authored()
        .filter(|o| o.metadata.contains_key("pipe_proto") || o.metadata.contains_key("power_cable"))
    {
        let middle = (o.min() + o.max()) * 0.5;
        let cell = GridCell::new(middle.x.floor() as i32, middle.z.floor() as i32);
        let Some(slab) = world.element_at(o.floor, SlotKey::FloorSlab(cell)) else {
            continue;
        };
        if !gameplay::crafting::is_subfloor(slab.material.as_str()) {
            continue;
        }
        let Some(floor) = world.floor(o.floor) else {
            continue;
        };
        let surface = Vec3::new(
            cell.x as f32 + 0.5,
            floor.base_y + slab.thickness + 0.006,
            cell.z as f32 + 0.5,
        );
        if let Some(group) = o.metadata.get("power_cable") {
            let (prefix, rsi, tint, height) = match group.as_str() {
                "hv" => (
                    "hvcable_",
                    "Structures/Power/Cables/hv_cable.rsi",
                    [255; 4],
                    0.012,
                ),
                "mv" => (
                    "mvcable_",
                    "Structures/Power/Cables/mv_cable.rsi",
                    [255, 255, 0, 255],
                    0.008,
                ),
                "apc" => (
                    "lvcable_",
                    "Structures/Power/Cables/lv_cable.rsi",
                    [0, 128, 0, 255],
                    0.004,
                ),
                _ => continue,
            };
            let mut mask = 0;
            for (bit, dx, dz) in [(1, 0, -1), (2, 0, 1), (4, 1, 0), (8, -1, 0)] {
                if cables.contains(&(o.floor.0, cell.x + dx, cell.z + dz, group.as_str())) {
                    mask |= bit;
                }
            }
            let at = surface + Vec3::Y * height;
            sprites.insert(
                (o.id, 0),
                Sprite {
                    rsi,
                    state: format!("{prefix}{mask}"),
                    tint,
                    at,
                    rotation: 0,
                },
            );
            if group == "mv" {
                sprites.insert(
                    (o.id, 1),
                    Sprite {
                        rsi,
                        state: format!("mvstripes_{mask}"),
                        tint: [255; 4],
                        at: at + Vec3::Y * 0.001,
                        rotation: 0,
                    },
                );
            }
        } else if let Some(recipe) = gameplay::crafting::pipes()
            .iter()
            .find(|p| !p.device && Some(&p.id) == o.metadata.get("pipe_proto"))
            && let Some(dirs) = o
                .metadata
                .get("gas_pipe")
                .or_else(|| o.metadata.get("loose_pipe_dirs"))
            && let Some(rotation) = recipe.rotation_for(dirs)
            && let Some(sprite) = pipe_sprite(&recipe.id, surface, rotation)
        {
            sprites.insert((o.id, 0), sprite);
        }
    }
    sprites
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync(
    mut commands: Commands,
    net: Option<NonSend<Net>>,
    hud: Res<Hud>,
    ground: Res<Ground>,
    mut visuals: ResMut<Visuals>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let revision = net
        .as_ref()
        .and_then(|n| n.core.world())
        .map(|w| w.revision().0);
    let world = net.as_ref().and_then(|n| n.core.world());
    let mesh = visuals
        .mesh
        .get_or_insert_with(|| meshes.add(Rectangle::new(1., 1.)))
        .clone();
    let mut spawn = |sprite: &Sprite, cache: &mut Visuals, commands: &mut Commands| {
        let key = (sprite.rsi, sprite.state.clone(), sprite.tint);
        if !cache.materials.contains_key(&key) {
            let image =
                crate::fire::load_frames(hud.icons.root(), sprite.rsi, &sprite.state, &mut images)
                    .into_iter()
                    .next()?
                    .1;
            let [r, g, b, a] = sprite.tint;
            let mat = materials.add(StandardMaterial {
                base_color_texture: Some(image),
                base_color: Color::srgba_u8(r, g, b, a),
                alpha_mode: if a < 255 {
                    AlphaMode::Blend
                } else {
                    AlphaMode::Mask(0.1)
                },
                double_sided: true,
                cull_mode: None,
                ..default()
            });
            cache.materials.insert(key.clone(), mat);
        }
        Some(
            commands
                .spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(cache.materials[&key].clone()),
                    Transform::from_translation(sprite.at).with_rotation(
                        Quat::from_rotation_y(f32::from(sprite.rotation) * FRAC_PI_2)
                            * Quat::from_rotation_x(-FRAC_PI_2),
                    ),
                ))
                .id(),
        )
    };
    if revision != visuals.revision {
        let wanted = world.map(visible_sprites).unwrap_or_default();
        visuals.placed.retain(|key, (current, entity)| {
            if Some(&*current) == wanted.get(key) {
                return true;
            }
            commands.entity(*entity).despawn();
            false
        });
        for (key, sprite) in wanted {
            if !visuals.placed.contains_key(&key)
                && let Some(entity) = spawn(&sprite, &mut visuals, &mut commands)
            {
                visuals.placed.insert(key, (sprite, entity));
            }
        }
        visuals.revision = revision;
    }
    let preview = hud.state.placement.as_ref().and_then(|(id, rotation)| {
        let [x, y, z] = ground.0?;
        let net = net.as_ref()?;
        let feet = net.core.position();
        if (x - feet.x).hypot(z - feet.z) > 8. {
            return None;
        }
        let at = Vec3::new(x.floor() + 0.5, y + 0.04, z.floor() + 0.5);
        let mut sprite = if matches!(
            id.as_str(),
            "Girder" | "Wall" | "ReinforcedGirder" | "ReinforcedWall"
        ) {
            Sprite {
                rsi: if id.starts_with("Reinforced") {
                    "Structures/Walls/solid-reinforced.rsi"
                } else {
                    "Structures/Walls/solid.rsi"
                },
                state: (if id.ends_with("Wall") {
                    "full"
                } else if id == "ReinforcedGirder" {
                    "reinforced_wall_girder"
                } else {
                    "wall_girder"
                })
                .into(),
                tint: [255; 4],
                at,
                rotation: 0,
            }
        } else if matches!(id.as_str(), "Computer" | "MachineFrame") {
            Sprite {
                rsi: "Structures/Machines/parts.rsi",
                state: if id == "Computer" { "0" } else { "box_0" }.into(),
                at,
                rotation: 0,
                tint: [255; 4],
            }
        } else {
            pipe_sprite(
                gameplay::crafting::placement_variant(id, ground.1),
                at,
                *rotation,
            )?
        };
        sprite.tint[3] = 140;
        Some(sprite)
    });
    if visuals.preview.as_ref().map(|(s, _)| s) != preview.as_ref() {
        if let Some((_, entity)) = visuals.preview.take() {
            commands.entity(entity).despawn();
        }
        if let Some(s) = preview
            && let Some(e) = spawn(&s, &mut visuals, &mut commands)
        {
            visuals.preview = Some((s, e));
        }
    }
}
