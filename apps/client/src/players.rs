//! Чужие и свой персонажи в мире: полоски действий над головами и покачивание при ходьбе.

use std::collections::HashMap;

use astrocore_client::{LocalBody, Net, RemoteSprites};
use astrocore_core::NetworkEntityId;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use gameplay::{DOAFTER_COMPONENT, DoAfterView, MobState, decode_doafter};

/// Высота головы над ступнями, м: сюда крепится полоска.
const HEAD_M: f32 = 1.15;
/// Скорость, с которой персонаж считается идущим, м/с.
const WALK_SPEED: f32 = 0.3;
/// Размах покачивания, м, и частота шага, рад/с.
const BOB_M: f32 = 0.035;
const BOB_RATE: f32 = 14.0;

/// Когда клиент узнал о действии игрока.
#[derive(Default)]
pub struct Seen(HashMap<NetworkEntityId, (DoAfterView, f32)>);

/// Полоски над головами тех, кто сейчас надевает, снимает или вскрывает.
pub fn other_bars(
    mut contexts: EguiContexts,
    net: Option<NonSend<Net>>,
    remote: Res<RemoteSprites>,
    time: Res<Time>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    transforms: Query<&Transform>,
    mut seen: Local<Seen>,
) -> Result {
    let (Some(net), Ok((camera, cam_t))) = (net, cameras.single()) else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    let now = time.elapsed_secs();
    seen.0.retain(|id, _| remote.0.contains_key(id));
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("others_bars"),
    ));
    for (&id, &entity) in &remote.0 {
        let view = net
            .core
            .component(id, DOAFTER_COMPONENT)
            .and_then(decode_doafter);
        let Some(view) = view else {
            seen.0.remove(&id);
            continue;
        };
        let since = match seen.0.get(&id) {
            Some((old, since)) if *old == view => *since,
            _ => now,
        };
        seen.0.insert(id, (view.clone(), since));
        let Ok(t) = transforms.get(entity) else {
            continue;
        };
        let Ok(p) = camera.world_to_viewport(cam_t, t.translation + Vec3::Y * HEAD_M) else {
            continue;
        };
        let fraction = ((now - since) / view.delay_s.max(0.01)).clamp(0.0, 1.0);
        let rect = egui::Rect::from_center_size(egui::pos2(p.x, p.y), egui::vec2(56.0, 8.0));
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(2),
            egui::Color32::from_black_alpha(190),
        );
        let filled =
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction, rect.height()));
        painter.rect_filled(
            filled,
            egui::CornerRadius::same(2),
            egui::Color32::from_rgb(40, 150, 200),
        );
    }
    Ok(())
}

/// Покачивание при ходьбе: скорость берётся из смещения спрайта между кадрами, поэтому одинаково
/// работает для своего тела и для чужих.
#[allow(clippy::too_many_arguments)]
pub fn walk_bob(
    net: Option<NonSend<Net>>,
    time: Res<Time>,
    remote: Res<RemoteSprites>,
    local: Res<LocalBody>,
    roots: Query<&Transform, Without<astrocore_render::BillboardQuad>>,
    children: Query<&Children>,
    mut quads: Query<&mut Transform, With<astrocore_render::BillboardQuad>>,
    mut state: Local<HashMap<Entity, (Vec3, f32, i32)>>,
    mut commands: Commands,
    sounds: Option<ResMut<crate::sounds::Sounds>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
) {
    let mut sounds = sounds;
    let listener = cameras.single().map(GlobalTransform::translation).ok();
    let dt = time.delta_secs().max(1e-4);
    let me = net.as_ref().and_then(|n| n.core.net_id());
    let tagged: Vec<(Entity, Option<NetworkEntityId>)> = remote
        .0
        .iter()
        .map(|(id, e)| (*e, Some(*id)))
        .chain(local.0.map(|e| (e, me)))
        .collect();
    let entities: Vec<Entity> = tagged.iter().map(|(e, _)| *e).collect();
    state.retain(|e, _| entities.contains(e));
    for (e, id) in tagged {
        let Ok(root) = roots.get(e) else { continue };
        let entry = state.entry(e).or_insert((root.translation, 0.0, 0));
        let speed = Vec3::new(
            root.translation.x - entry.0.x,
            0.0,
            root.translation.z - entry.0.z,
        )
        .length()
            / dt;
        entry.0 = root.translation;
        let moving = speed > WALK_SPEED;
        if moving {
            entry.1 += dt * BOB_RATE;
            // Шаг на каждый полупериод покачивания.
            let step = (entry.1 / std::f32::consts::PI) as i32;
            if step != entry.2 {
                entry.2 = step;
                if let (Some(s), Some(l)) = (sounds.as_mut(), listener) {
                    s.step(&mut commands, l.distance(root.translation));
                }
            }
        }
        // В покое спрайт стоит ровно.
        let bob = if moving {
            entry.1.sin().abs() * BOB_M
        } else {
            0.0
        };
        let lying = match (net.as_ref(), id) {
            (Some(n), Some(id)) => n
                .core
                .component(id, gameplay::MOBSTATE_COMPONENT)
                .and_then(|b| b.first().copied())
                .is_some_and(|b| MobState::from_byte(b) != MobState::Alive),
            _ => false,
        };
        if let Ok(kids) = children.get(e) {
            for &k in kids {
                if let Ok(mut q) = quads.get_mut(k) {
                    if lying {
                        // Лежит на боку: спрайт повёрнут на 90° вокруг основания и чуть сдвинут.
                        q.rotation = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2);
                        q.translation = Vec3::new(0.45, 0.05, 0.0);
                    } else {
                        q.rotation = Quat::IDENTITY;
                        q.translation = Vec3::new(0.0, bob, 0.0);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_bob_stays_within_its_amplitude() {
        for i in 0..200 {
            let b = (i as f32 * 0.1).sin().abs() * super::BOB_M;
            assert!((0.0..=super::BOB_M).contains(&b));
        }
    }
}
