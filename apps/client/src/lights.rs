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
const LUMENS_PER_ENERGY: f32 = 25_000.0;
/// Яркость общего света и солнца на станции со светильниками.
const DIM_AMBIENT: f32 = 4.0;
/// Внутри станции солнца нет: свет только от ламп и фонарика.
const DIM_SUN: f32 = 0.0;

#[derive(Component)]
pub struct StationLight;

#[allow(clippy::too_many_arguments)]
pub fn sync_lights(
    mut commands: Commands,
    net: Option<NonSend<Net>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    existing: Query<Entity, With<StationLight>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear: ResMut<ClearColor>,
    mut suns: Query<&mut DirectionalLight>,
    mut last: Local<Option<gameplay::electric::LightsView>>,
    mut last_cam: Local<Vec3>,
    mut frames: Local<u32>,
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
    // Отладка: снимок через 40 и 100 кадров после появления светильников (ASTRO_SHOT_LIGHTS=каталог).
    if let Some(dir) = std::env::var_os("ASTRO_SHOT_LIGHTS") {
        *frames += 1;
        if matches!(*frames, 40 | 100) {
            let path = std::path::Path::new(&dir).join(format!("lights{}.png", *frames));
            commands
                .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
                .observe(bevy::render::view::screenshot::save_to_disk(path));
        }
    }
    let Ok(cam) = cameras.single() else { return };
    let cam_pos = cam.translation();
    // Отладка: ASTRO_LUMENS подбирает яркость ламп.
    let lumens = std::env::var("ASTRO_LUMENS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(LUMENS_PER_ENERGY);
    // Пересобираем при смене состояния или заметном смещении камеры.
    if last.as_ref() == Some(&view) && cam_pos.distance(*last_cam) < 3.0 {
        return;
    }
    *last = Some(view.clone());
    *last_cam = cam_pos;
    ambient.brightness = DIM_AMBIENT;
    // Космос за станцией чёрный, а не голубое небо.
    clear.0 = Color::BLACK;
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
                intensity: k[3] * lumens,
                range: k[4],
                cast_shadows: false,
            },
            Transform::from_xyz(l.0, l.1, l.2),
        ));
    }
}

// --- Фонарик ---

/// Фонарик игрока: прожектор на камере, клавиша Q включает и выключает.
#[derive(Component)]
pub struct Flashlight;

/// Включён ли фонарик (для затемнения краёв экрана).
#[derive(Resource, Default)]
pub struct FlashlightOn(pub bool);

/// Полураствор конуса, рад.
const BEAM_HALF_ANGLE: f32 = 0.42;
/// Дальность луча, м, и сила света, люмен.
const BEAM_RANGE_M: f32 = 18.0;
const BEAM_LUMENS: f32 = 2_500_000.0;

pub fn flashlight(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut on: ResMut<FlashlightOn>,
    cameras: Query<Entity, With<Camera3d>>,
    lights: Query<Entity, With<Flashlight>>,
    mut started: Local<bool>,
) {
    let Ok(cam) = cameras.single() else { return };
    // Отладка: ASTRO_FLASHLIGHT=1 включает фонарик сразу.
    if std::env::var_os("ASTRO_FLASHLIGHT").is_some() && !on.0 && !*started {
        on.0 = true;
    }
    *started = true;
    if keys.just_pressed(KeyCode::KeyQ) {
        on.0 = !on.0;
    }
    let existing = lights.iter().next();
    match (on.0, existing) {
        (true, None) => {
            let light = commands
                .spawn((
                    Flashlight,
                    LightSource {
                        kind: LightKind::Spot {
                            half_angle: BEAM_HALF_ANGLE,
                        },
                        color: Color::srgb(1.0, 0.96, 0.88),
                        intensity: BEAM_LUMENS,
                        range: BEAM_RANGE_M,
                        cast_shadows: true,
                    },
                    // Камера смотрит вдоль −Z, прожектор тоже; чуть ниже и вперёд от глаз.
                    Transform::from_xyz(0.0, -0.15, -0.1),
                ))
                .id();
            commands.entity(cam).add_child(light);
        }
        (false, Some(e)) => commands.entity(e).despawn(),
        _ => {}
    }
}

/// Тёмная виньетка по краям экрана, пока горит фонарик: кольцо из прозрачного центра в чёрный край.
pub fn vignette(mut contexts: bevy_egui::EguiContexts, on: Res<FlashlightOn>) {
    if !on.0 {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    use bevy_egui::egui;
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("vignette"),
    ));
    let centre = screen.center();
    let (rx, ry) = (screen.width() * 0.5, screen.height() * 0.5);
    const STEPS: usize = 48;
    let mut mesh = egui::Mesh::default();
    // Внутренний край кольца прозрачный (0,55 половины экрана), внешний — почти чёрный.
    for i in 0..=STEPS {
        let a = i as f32 / STEPS as f32 * std::f32::consts::TAU;
        let (sin, cos) = a.sin_cos();
        let inner = egui::pos2(centre.x + cos * rx * 0.55, centre.y + sin * ry * 0.55);
        let outer = egui::pos2(centre.x + cos * rx * 1.6, centre.y + sin * ry * 1.6);
        mesh.colored_vertex(inner, egui::Color32::TRANSPARENT);
        mesh.colored_vertex(outer, egui::Color32::from_black_alpha(235));
    }
    for i in 0..STEPS as u32 {
        let b = i * 2;
        mesh.add_triangle(b, b + 1, b + 2);
        mesh.add_triangle(b + 1, b + 3, b + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}
