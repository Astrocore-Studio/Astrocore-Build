//! Стенд съёмки (отладка, `ASTRO_SHOWCASE=1` вместе с `--autoplay`): персонаж кладёт у ног
//! несколько вещей (консоль, нужен админ-ранг — так запускает `scripts/run_all`), отходит на
//! шаг, а камера встаёт перед ним и смотрит на него и на вещи. Так кадры `ASTRO_SHOT`
//! показывают модели персонажа и предметов, а не стену у точки появления.

use astrocore_client::{LocalBody, Look, Net};
use astrocore_render::DirectionalSprite;
use bevy::prelude::*;

/// Что кладётся у ног (прототипы, которые есть в контенте Build).
const ITEMS: &[&str] = &[
    "Crowbar",
    "Wrench",
    "FoodBurgerPlain",
    "DrinkWaterBottleFull",
];

pub fn enabled() -> bool {
    std::env::var_os("ASTRO_SHOWCASE").is_some()
}

/// `ASTRO_SHOWCASE=map`: вместо камеры перед персонажем — окно маппинга (вид сверху) и
/// расставленные через него автоматы, шкафы, стул и стол.
fn map_mode() -> bool {
    matches!(
        std::env::var("ASTRO_SHOWCASE").as_deref(),
        Ok("map") | Ok("tiles")
    )
}

/// `ASTRO_SHOWCASE=tiles`: пустая карта (`mapping`) и плитки, положенные прямоугольниками.
fn tiles_mode() -> bool {
    std::env::var("ASTRO_SHOWCASE").as_deref() == Ok("tiles")
}

/// Что кладёт режим `tiles` (плитка или пусто, углы прямоугольника).
const TILE_SET: &[(Option<&str>, [i32; 2], [i32; 2])] = &[
    (Some("ss14:tile_floorsteel"), [-3, -3], [4, 3]),
    (Some("ss14:tile_floordark"), [-1, -1], [2, 1]),
    (Some("ss14:tile_floorwhite"), [5, -3], [7, 3]),
    (None, [6, 0], [6, 0]),
];

/// Что расставляет режим `map` (прототип, сдвиг от персонажа по X и Z, поворот).
const MAP_SET: &[(&str, f32, f32, u8)] = &[
    ("ss14:vending_machine_cola", -1.0, -2.0, 0),
    ("ss14:vending_machine_snack", 0.0, -2.0, 0),
    ("ss14:closet_steel_base", 1.0, -2.0, 0),
    ("ss14:closet_emergency_filled_random", 2.0, -2.0, 0),
    ("ss14:chair", -1.0, 1.0, 2),
    ("ss14:table", 0.0, 1.0, 0),
    ("ss14:table_reinforced", 1.0, 1.0, 0),
];

/// Команды стенда по кадрам автоплея: вещи, затем шаг в сторону.
pub fn commands(
    frame: u32,
    net: &mut Net,
    look: Option<&mut Look>,
    mapping: &mut crate::mapping_ui::MappingState,
) {
    if !enabled() {
        return;
    }
    if tiles_mode() {
        mapping.open = true;
        mapping.tab = crate::mapping_ui::Tab::Tiles;
        if frame == 40 {
            net.send_command(gameplay::admin::CMD_CONSOLE, b"mapping".to_vec());
        }
        for (i, (tile, from, to)) in TILE_SET.iter().enumerate() {
            if frame == 200 + 5 * i as u32 {
                let r = gameplay::systems::mapping::TileRequest {
                    tile: tile.map(Into::into),
                    from: *from,
                    to: *to,
                    level: 0,
                    ceiling: false,
                };
                net.send_command(
                    gameplay::systems::mapping::CMD_TILE,
                    gameplay::systems::mapping::tile_data(&r),
                );
            }
        }
        return;
    }
    if map_mode() {
        mapping.open = true;
        for (i, (proto, dx, dz, rot)) in MAP_SET.iter().enumerate() {
            if frame == 40 + 3 * i as u32 {
                let p = net.core.position();
                let r = gameplay::systems::mapping::SpawnRequest {
                    proto: (*proto).into(),
                    at: crate::mapping_ui::snap([p.x + dx, p.y + 0.05, p.z + dz], false),
                    rot: *rot,
                };
                net.send_command(
                    gameplay::systems::mapping::CMD_SPAWN,
                    gameplay::systems::mapping::spawn_data(&r),
                );
            }
        }
        return;
    }
    for (i, item) in ITEMS.iter().enumerate() {
        if frame == 30 + 4 * i as u32 {
            net.send_command(
                gameplay::admin::CMD_CONSOLE,
                format!("spawn {item}").into_bytes(),
            );
        }
    }
    if frame == 60 {
        let p = net.core.position();
        net.send_command(
            gameplay::admin::CMD_CONSOLE,
            format!("tp {:.2} {:.2}", p.x + 1.0, p.z).into_bytes(),
        );
    }
    // Своё тело видно только не от первого лица.
    if let Some(look) = look {
        look.first_person = false;
    }
}

/// Камера перед персонажем: в 2,6 м по направлению его взгляда, на высоте 1,5 м, смотрит на
/// точку между ним и вещами. Ставится в `Last`, поверх обычной камеры, вместе с итоговым
/// положением — рендер берёт его в этом же кадре.
pub fn camera(
    local: Res<LocalBody>,
    bodies: Query<(&Transform, &DirectionalSprite), Without<Camera3d>>,
    mut cameras: Query<(&mut Transform, &mut GlobalTransform), With<Camera3d>>,
) {
    if !enabled() || map_mode() {
        return;
    }
    // `ASTRO_SHOWCASE_CAM=ex,ey,ez,fx,fy,fz`: камера в точке e смотрит на f (проверка геометрии).
    if let Some(v) = std::env::var("ASTRO_SHOWCASE_CAM").ok().and_then(|s| {
        let v: Vec<f32> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (v.len() == 6).then_some(v)
    }) {
        let t =
            Transform::from_xyz(v[0], v[1], v[2]).looking_at(Vec3::new(v[3], v[4], v[5]), Vec3::Y);
        for (mut transform, mut global) in &mut cameras {
            *transform = t;
            *global = GlobalTransform::from(t);
        }
        return;
    }
    let Some((body, sprite)) = local.0.and_then(|e| bodies.get(e).ok()) else {
        return;
    };
    let f = Vec3::new(sprite.facing.x, 0.0, sprite.facing.z).normalize_or(Vec3::NEG_Z);
    let feet = body.translation;
    let focus = feet + Vec3::new(-0.5, 0.8, 0.0);
    let eye = feet + f * 2.6 + Vec3::new(0.6, 1.5, 0.0);
    let t = Transform::from_translation(eye).looking_at(focus, Vec3::Y);
    for (mut transform, mut global) in &mut cameras {
        *transform = t;
        *global = GlobalTransform::from(t);
    }
}
