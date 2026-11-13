//! Source RSI composites for ComputerAlert, AirAlarm, FireAlarm and AirSensor.
//! The engine's existing placement/material API is sufficient; no SS14 rules in Engine.
use astrocore_client::Net;
use astrocore_core::PrototypeId;
use astrocore_math::glam::Vec3 as MVec3;
use astrocore_render::manifest::PlacementBillboard;
use astrocore_sprites::{DirectionSet, select_direction};
use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use gameplay::alerts::Level;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

struct Animation {
    frames: Vec<Handle<Image>>,
    delays: Vec<f32>,
}
#[derive(Resource, Default)]
pub struct AlarmVisuals(BTreeMap<PrototypeId, BTreeMap<Level, Vec<Animation>>>);

pub fn install(root: &Path, images: &mut Assets<Image>, commands: &mut Commands) {
    let path = root.join("atmos-alerts-visuals.json");
    let data = match std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
    {
        Some(data) => data,
        None => {
            eprintln!("состояния сигнализаций не найдены: {}", path.display());
            return;
        }
    };
    let mut visuals = AlarmVisuals::default();
    let mut cache = HashMap::new();
    for (name, variants) in data["devices"].as_object().into_iter().flatten() {
        let Some(prototype) = prototypes::engine_id(gameplay::NAMESPACE, name) else {
            continue;
        };
        let mut states = BTreeMap::new();
        for (key, level) in [
            ("Invalid", Level::Invalid),
            ("Normal", Level::Normal),
            ("Warning", Level::Warning),
            ("Danger", Level::Danger),
        ] {
            let mut directions = Vec::new();
            for direction in variants[key]["directions"].as_array().into_iter().flatten() {
                let mut frames = Vec::new();
                let delays: Vec<_> = direction["delays"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_f64().map(|v| v as f32))
                    .collect();
                for frame in direction["images"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                {
                    if !cache.contains_key(frame) {
                        let Ok(pixels) = image::open(root.join(frame)).map(|i| i.to_rgba8()) else {
                            continue;
                        };
                        let mut image = Image::new(
                            Extent3d {
                                width: pixels.width(),
                                height: pixels.height(),
                                depth_or_array_layers: 1,
                            },
                            TextureDimension::D2,
                            pixels.into_raw(),
                            TextureFormat::Rgba8UnormSrgb,
                            RenderAssetUsages::default(),
                        );
                        image.sampler = ImageSampler::nearest();
                        cache.insert(frame.to_string(), images.add(image));
                    }
                    frames.push(cache[frame].clone());
                }
                if frames.len() == delays.len()
                    && !frames.is_empty()
                    && delays.iter().all(|d| d.is_finite() && *d > 0.0)
                {
                    directions.push(Animation { frames, delays });
                }
            }
            if !directions.is_empty() {
                states.insert(level, directions);
            }
        }
        visuals.0.insert(prototype, states);
    }
    commands.insert_resource(visuals);
}

#[allow(clippy::too_many_arguments)] // Bevy system parameters.
pub fn sync(
    net: Option<NonSend<Net>>,
    loadout: Res<crate::Loadout>,
    animations: Option<Res<AlarmVisuals>>,
    time: Res<Time>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    placements: Query<(&PlacementBillboard, &Children)>,
    quads: Query<&MeshMaterial3d<StandardMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (Some(net), Some(animations), Some(view)) =
        (net, animations, loadout.alarm_visuals.as_ref())
    else {
        return;
    };
    let Some(world) = net.core.world() else {
        return;
    };
    let Ok(camera) = cameras.single() else { return };
    let camera = MVec3::from_array(camera.translation().to_array());
    let states: BTreeMap<_, _> = view.states.iter().copied().collect();
    for (billboard, children) in &placements {
        let Some(level) = states.get(&billboard.0) else {
            continue;
        };
        let Some(object) = world.authored_object(billboard.0) else {
            continue;
        };
        let astrocore_world::AuthoredKind::Placement { prototype } = &object.kind else {
            continue;
        };
        let Some(variants) = animations.0.get(prototype) else {
            continue;
        };
        let Some(directions) = variants.get(level).or_else(|| variants.get(&Level::Normal)) else {
            continue;
        };
        let rot = object
            .metadata
            .get("rot")
            .and_then(|r| r.parse::<f32>().ok())
            .unwrap_or(0.0)
            * std::f32::consts::FRAC_PI_2;
        let facing = MVec3::new(rot.sin(), 0.0, rot.cos());
        let direction = select_direction(
            facing,
            (object.min() + object.max()) * 0.5,
            camera,
            DirectionSet::from_count(directions.len() as u8).unwrap_or(DirectionSet::One),
        )
        .index();
        let animation = &directions[direction.min(directions.len() - 1)];
        let mut elapsed = time.elapsed_secs() % animation.delays.iter().sum::<f32>();
        let mut frame = 0;
        while frame + 1 < animation.frames.len() && elapsed >= animation.delays[frame] {
            elapsed -= animation.delays[frame];
            frame += 1;
        }
        for child in children {
            if let Ok(handle) = quads.get(*child)
                && let Some(mut material) = materials.get_mut(&handle.0)
                && material.base_color_texture.as_ref() != Some(&animation.frames[frame])
            {
                material.base_color_texture = Some(animation.frames[frame].clone());
            }
        }
    }
}
