//! Светильники станции: сервер присылает точки, цвет и «горит ли» (запитан ли светильник), клиент
//! держит настоящие источники света у ближайших включённых. Когда на карте есть светильники,
//! солнце и общий свет приглушаются: станция освещена только лампами.

use astrocore_client::Net;
use astrocore_render::{LightKind, LightSource};
use bevy::prelude::*;
use gameplay::electric::{LIGHTS_COMPONENT, decode_lights};

/// Сколько ближайших включённых светильников одновременно рисуется.
const MAX_ACTIVE: usize = 40;
/// Дальше этого от камеры светильник не нужен, м.
const SEE_M: f32 = 35.0;
/// Люмен на единицу `energy` SS14: подобрано по виду сцены.
const LUMENS_PER_ENERGY: f32 = 500_000.0;
/// Яркость общего света и солнца на станции со светильниками.
const DIM_AMBIENT: f32 = 60.0;
const DIM_SUN: f32 = 700.0;

#[derive(Component)]
pub struct StationLight;

#[allow(clippy::too_many_arguments)]
pub fn sync_lights(
    mut commands: Commands,
    net: Option<NonSend<Net>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    existing: Query<Entity, With<StationLight>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut suns: Query<&mut DirectionalLight>,
    mut last: Local<Option<gameplay::electric::LightsView>>,
    mut last_cam: Local<Vec3>,
) {
    let Some(net) = net else { return };
    let Some(me) = net.core.net_id() else { return };
    let view = net
        .core
        .component(me, LIGHTS_COMPONENT)
        .and_then(decode_lights)
        .unwrap_or_default();
    if view.lights.is_empty() {
        return;
    }
    let Ok(cam) = cameras.single() else { return };
    let cam_pos = cam.translation();
    // Пересобираем при смене состояния или заметном смещении камеры.
    if last.as_ref() == Some(&view) && cam_pos.distance(*last_cam) < 3.0 {
        return;
    }
    *last = Some(view.clone());
    *last_cam = cam_pos;
    ambient.brightness = DIM_AMBIENT;
    for mut sun in &mut suns {
        sun.illuminance = DIM_SUN;
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    type Lamp = (f32, f32, f32, u8, bool);
    let mut on: Vec<(f32, &Lamp)> = view
        .lights
        .iter()
        .filter(|l| l.4)
        .map(|l| (Vec3::new(l.0, l.1, l.2).distance(cam_pos), l))
        .filter(|(d, _)| *d <= SEE_M)
        .collect();
    on.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, l) in on.into_iter().take(MAX_ACTIVE) {
        let Some(k) = view.kinds.get(usize::from(l.3)) else {
            continue;
        };
        commands.spawn((
            StationLight,
            LightSource {
                kind: LightKind::Point,
                color: Color::srgb_u8(k[0] as u8, k[1] as u8, k[2] as u8),
                intensity: k[3] * LUMENS_PER_ENERGY,
                range: k[4],
                cast_shadows: false,
            },
            Transform::from_xyz(l.0, l.1, l.2),
        ));
    }
}
