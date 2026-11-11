//! Бой на клиенте: удар по клику и лечение.

use astrocore_client::{Net, RemoteSprites};
use astrocore_core::NetworkEntityId;
use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;
use gameplay::{CMD_ATTACK, CMD_HEAL};

/// На какой высоте над ступнями целиться в персонажа, м.
/// Насколько близко луч должен пройти к груди, чтобы считать, что игрок в цель попал, м.
const PICK_RADIUS_M: f32 = 0.55;

/// Высота спрайта персонажа, м.
const BODY_M: f32 = 1.0;

/// Куда пришёлся выстрел по спрайту: часть тела по высоте и по смещению в сторону (смещение от
/// камеры вправо — левая часть тела, ведь персонаж обычно смотрит на нас).
fn zone_of(height: f32, lateral: f32) -> &'static str {
    let h = (height / BODY_M).clamp(0.0, 1.0);
    let side_left = lateral > 0.0;
    if h >= 0.75 {
        "Head"
    } else if h >= 0.35 {
        if lateral.abs() > 0.18 {
            if side_left { "LeftArm" } else { "RightArm" }
        } else {
            "Torso"
        }
    } else if side_left {
        "LeftLeg"
    } else {
        "RightLeg"
    }
}

/// Ближайший вдоль луча персонаж, в которого можно попасть, и часть тела под курсором.
pub fn pick_target(
    origin: Vec3,
    dir: Vec3,
    candidates: &[(NetworkEntityId, Vec3)],
) -> Option<(NetworkEntityId, &'static str)> {
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    candidates
        .iter()
        .filter_map(|(id, feet)| {
            let center = *feet + Vec3::Y * (BODY_M * 0.5);
            let along = (center - origin).dot(dir);
            if along <= 0.0 {
                return None;
            }
            let miss = (center - (origin + dir * along)).length();
            if miss > PICK_RADIUS_M {
                return None;
            }
            // Точка луча, ближайшая к вертикальной оси персонажа, даёт высоту и смещение.
            let flat = Vec3::new(dir.x, 0.0, dir.z);
            let t = if flat.length_squared() > 1e-6 {
                ((*feet - origin).dot(flat)) / flat.length_squared()
            } else {
                along
            };
            let p = origin + dir * t;
            Some((*id, along, zone_of(p.y - feet.y, (p - *feet).dot(right))))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _, part)| (id, part))
}

/// Левая кнопка по миру — удар по игроку под курсором; `T` — применить предмет из руки на него.
#[allow(clippy::too_many_arguments)]
pub fn attack_and_heal(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    wants: Res<EguiWantsInput>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    remote: Res<RemoteSprites>,
    transforms: Query<&Transform>,
    loadout: Res<crate::Loadout>,
    net: Option<NonSendMut<Net>>,
) {
    let Some(mut net) = net else { return };
    let active = loadout.hands.as_ref().map_or(0, |h| h.active() as u8);
    if keys.just_pressed(KeyCode::KeyR)
        && let Some(me) = net.core.net_id()
    {
        net.send_command(CMD_HEAL, gameplay::heal_data(active, me));
    }
    let attack = buttons.just_pressed(MouseButton::Left) && !wants.wants_any_pointer_input();
    let heal_other = keys.just_pressed(KeyCode::KeyT);
    if !attack && !heal_other {
        return;
    }
    let (Ok(window), Ok((camera, cam_t))) = (windows.single(), cameras.single()) else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_t, cursor) else {
        return;
    };
    let candidates: Vec<(NetworkEntityId, Vec3)> = remote
        .0
        .iter()
        .filter_map(|(id, e)| Some((*id, transforms.get(*e).ok()?.translation)))
        .collect();
    let Some((target, part)) = pick_target(ray.origin, *ray.direction, &candidates) else {
        return;
    };
    if attack {
        let index = gameplay::PARTS.iter().position(|p| *p == part).unwrap_or(1) as u8;
        net.send_command(CMD_ATTACK, gameplay::attack_part_data(target, index));
    } else {
        net.send_command(CMD_HEAL, gameplay::heal_data(active, target));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ray_picks_the_nearest_player_and_the_part_it_hits() {
        let (a, b, c) = (NetworkEntityId(1), NetworkEntityId(2), NetworkEntityId(3));
        let dir = Vec3::NEG_Z;
        let near = (a, Vec3::new(0.0, 0.0, -2.0));
        let far = (b, Vec3::new(0.0, 0.0, -4.0));
        let aside = (c, Vec3::new(3.0, 0.0, -1.0));
        // Луч на высоте 0,5 м идёт в торс ближайшего.
        let origin = Vec3::new(0.0, 0.5, 0.0);
        assert_eq!(
            pick_target(origin, dir, &[far, near, aside]),
            Some((a, "Torso"))
        );
        assert_eq!(pick_target(origin, dir, &[aside]), None);
        assert_eq!(
            pick_target(origin, Vec3::Z, &[near]),
            None,
            "позади — не цель"
        );
        // Выше — голова, ниже — нога, сбоку — рука.
        assert_eq!(
            pick_target(Vec3::new(0.0, 0.9, 0.0), dir, &[near])
                .unwrap()
                .1,
            "Head"
        );
        assert_eq!(
            pick_target(Vec3::new(0.0, 0.1, 0.0), dir, &[near])
                .unwrap()
                .1,
            "RightLeg"
        );
        assert_eq!(
            pick_target(Vec3::new(0.3, 0.5, 0.0), dir, &[near])
                .unwrap()
                .1,
            "LeftArm"
        );
        assert_eq!(
            pick_target(Vec3::new(-0.3, 0.5, 0.0), dir, &[near])
                .unwrap()
                .1,
            "RightArm"
        );
    }
}
