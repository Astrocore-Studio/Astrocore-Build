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

/// Команды стенда по кадрам автоплея: вещи, затем шаг в сторону.
pub fn commands(frame: u32, net: &mut Net, look: Option<&mut Look>) {
    if !enabled() {
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
    if !enabled() {
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
