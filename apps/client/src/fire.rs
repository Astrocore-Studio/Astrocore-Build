//! Огонь и лужи в мире: настоящие сущности сцены, а не пятна поверх экрана, поэтому стены и
//! предметы их закрывают. Пламя на клетках — `Effects/fire.rsi` (состояния 1–3, как у `Hotspot`
//! в SS14), горящие персонажи — `Mobs/Effects/onfire.rsi`, лужи — цветные диски на полу.

use std::collections::HashMap;
use std::path::Path;

use astrocore_client::{Net, RemoteSprites};
use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use gameplay::fire::{BURNING_COMPONENT, FIRES_COMPONENT, decode_fires};
use gameplay::puddles::{PUDDLES_COMPONENT, PuddleView, decode_puddles};

/// Размер пламени на клетке и на горящем персонаже, м.
const FLAME_M: f32 = 1.0;
const BODY_FLAME_M: f32 = 1.1;
/// Высота лужи над полом, м (чтобы не мерцала на плитке).
const PUDDLE_LIFT_M: f32 = 0.02;

/// Кадры анимации: задержка и картинка.
type Frames = Vec<(f32, Handle<Image>)>;

#[derive(Resource, Default)]
pub struct FxAssets {
    ready: bool,
    fire: HashMap<u8, Frames>,
    body: Frames,
    rect: Handle<Mesh>,
    disc: Handle<Mesh>,
}

/// Пламя: какой набор кадров играет и повернуть ли к камере.
#[derive(Component)]
pub struct Flame {
    state: u8,
    body: bool,
}

/// Метка сущностей, которыми управляет эта подсистема.
#[derive(Component)]
pub struct Fx;

/// Нарезать состояние RSI на кадры по `meta.json`.
fn load_frames(root: &Path, rsi: &str, state: &str, images: &mut Assets<Image>) -> Frames {
    let dir = root.join("Textures").join(rsi);
    let Ok(img) = image::open(dir.join(format!("{state}.png"))) else {
        return Vec::new();
    };
    let img = img.to_rgba8();
    let delays: Vec<f32> = std::fs::read(dir.join("meta.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|m| {
            m.get("states")?
                .as_array()?
                .iter()
                .find(|s| s.get("name").and_then(|n| n.as_str()) == Some(state))?
                .get("delays")?
                .as_array()?
                .first()?
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                        .collect()
                })
        })
        .unwrap_or_default();
    let cols = (img.width() / 32).max(1);
    let count = if delays.is_empty() {
        1
    } else {
        delays.len() as u32
    };
    (0..count)
        .map(|i| {
            let (cx, cy) = (i % cols, i / cols);
            let frame = image::imageops::crop_imm(&img, cx * 32, cy * 32, 32, 32).to_image();
            let mut out = Image::from_dynamic(
                image::DynamicImage::ImageRgba8(frame),
                true,
                RenderAssetUsages::default(),
            );
            out.sampler = ImageSampler::nearest();
            (
                delays.get(i as usize).copied().unwrap_or(1.0),
                images.add(out),
            )
        })
        .collect()
}

fn frame_at(frames: &Frames, t: f32) -> Option<&Handle<Image>> {
    let total: f32 = frames.iter().map(|f| f.0).sum();
    if frames.is_empty() {
        return None;
    }
    let mut t = t % total.max(0.001);
    for (d, h) in frames {
        if t < *d {
            return Some(h);
        }
        t -= d;
    }
    frames.last().map(|f| &f.1)
}

#[allow(clippy::too_many_arguments)]
pub fn sync_fx(
    mut commands: Commands,
    net: Option<NonSend<Net>>,
    hud: Res<crate::Hud>,
    mut assets: ResMut<FxAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    remote: Res<RemoteSprites>,
    mut last_fires: Local<Vec<gameplay::fire::FireView>>,
    mut last_puddles: Local<Vec<PuddleView>>,
    mut world_fx: Local<Vec<Entity>>,
    mut burning: Local<HashMap<Entity, Entity>>,
) {
    let Some(net) = net else { return };
    let Some(me) = net.core.net_id() else { return };
    if !assets.ready {
        let root = hud.icons.root().to_path_buf();
        for s in 1..=3u8 {
            let f = load_frames(&root, "Effects/fire.rsi", &s.to_string(), &mut images);
            assets.fire.insert(s, f);
        }
        assets.body = load_frames(
            &root,
            "Mobs/Effects/onfire.rsi",
            "Generic_mob_burning",
            &mut images,
        );
        assets.rect = meshes.add(Rectangle::new(1.0, 1.0));
        assets.disc = meshes.add(Circle::new(0.5));
        assets.ready = true;
    }
    let fires = net
        .core
        .component(me, FIRES_COMPONENT)
        .and_then(decode_fires)
        .unwrap_or_default();
    let puddles = net
        .core
        .component(me, PUDDLES_COMPONENT)
        .and_then(decode_puddles)
        .unwrap_or_default();
    if fires != *last_fires || puddles != *last_puddles {
        for e in world_fx.drain(..) {
            commands.entity(e).despawn();
        }
        for f in &fires {
            let mat = materials.add(StandardMaterial {
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                double_sided: true,
                cull_mode: None,
                ..default()
            });
            let e = commands
                .spawn((
                    Fx,
                    Flame {
                        state: f.state.clamp(1, 3),
                        body: false,
                    },
                    Mesh3d(assets.rect.clone()),
                    MeshMaterial3d(mat),
                    Transform::from_xyz(f.x, f.y + FLAME_M * 0.5, f.z)
                        .with_scale(Vec3::splat(FLAME_M)),
                ))
                .id();
            world_fx.push(e);
        }
        for p in &puddles {
            let mat = materials.add(StandardMaterial {
                base_color: Color::srgba_u8(p.color[0], p.color[1], p.color[2], 190),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                double_sided: true,
                cull_mode: None,
                ..default()
            });
            // Диск лежит на полу; размер растёт с объёмом.
            let r = 0.9 * p.fill.clamp(0.1, 1.0).sqrt();
            let e = commands
                .spawn((
                    Fx,
                    Mesh3d(assets.disc.clone()),
                    MeshMaterial3d(mat),
                    Transform::from_xyz(p.x, p.y + PUDDLE_LIFT_M, p.z)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::splat(r)),
                ))
                .id();
            world_fx.push(e);
        }
        *last_fires = fires;
        *last_puddles = puddles;
    }

    // Горящие персонажи: пламя дочерней сущностью тела.
    let bodies: Vec<(Entity, bool)> = remote
        .0
        .iter()
        .map(|(id, e)| {
            (
                *e,
                net.core
                    .component(*id, BURNING_COMPONENT)
                    .is_some_and(|b| b.first() == Some(&1)),
            )
        })
        .collect();
    for (body, on_fire) in bodies {
        match (on_fire, burning.contains_key(&body)) {
            (true, false) => {
                let mat = materials.add(StandardMaterial {
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    double_sided: true,
                    cull_mode: None,
                    ..default()
                });
                let flame = commands
                    .spawn((
                        Fx,
                        Flame {
                            state: 0,
                            body: true,
                        },
                        Mesh3d(assets.rect.clone()),
                        MeshMaterial3d(mat),
                        Transform::from_xyz(0.0, 0.7, 0.05).with_scale(Vec3::splat(BODY_FLAME_M)),
                    ))
                    .id();
                commands.entity(body).add_child(flame);
                burning.insert(body, flame);
            }
            (false, true) => {
                if let Some(f) = burning.remove(&body) {
                    commands.entity(f).despawn();
                }
            }
            _ => {}
        }
    }
    burning.retain(|b, _| remote.0.values().any(|e| e == b));
}

/// Пламя поворачивается к камере и меняет кадры.
pub fn animate_flames(
    time: Res<Time>,
    assets: Res<FxAssets>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut flames: Query<(
        &Flame,
        &GlobalTransform,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok(cam) = camera.single() else { return };
    let t = time.elapsed_secs();
    for (flame, _global, mut tr, mat) in &mut flames {
        if !flame.body {
            // Плоскость пламени параллельна экрану: видно и сверху, и сбоку.
            tr.rotation = cam.rotation();
        }
        let frames = if flame.body {
            Some(&assets.body)
        } else {
            assets.fire.get(&flame.state)
        };
        if let Some(frames) = frames
            && let Some(h) = frame_at(frames, t)
            && let Some(mut m) = materials.get_mut(&mat.0)
            && m.base_color_texture.as_ref() != Some(h)
        {
            m.base_color_texture = Some(h.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ss14")
    }

    #[test]
    fn the_flame_sets_are_cut_into_frames() {
        let mut images = Assets::<Image>::default();
        for (state, n) in [("1", 6), ("2", 14), ("3", 14)] {
            let f = load_frames(&root(), "Effects/fire.rsi", state, &mut images);
            assert_eq!(f.len(), n, "{state}");
        }
        let b = load_frames(
            &root(),
            "Mobs/Effects/onfire.rsi",
            "Generic_mob_burning",
            &mut images,
        );
        assert!(!b.is_empty());
    }
}
