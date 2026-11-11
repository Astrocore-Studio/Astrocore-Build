//! Огонь на экране: пламя на клетках (`Effects/fire.rsi`, состояния 1–3, как у `Hotspot` в SS14) и
//! горящие персонажи (`Mobs/Effects/onfire.rsi`). Рисуется поверх мира в точках, куда проецируются
//! клетки и тела.

use astrocore_client::{LocalBody, Net, RemoteSprites};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use gameplay::fire::{BURNING_COMPONENT, FIRES_COMPONENT, decode_fires};

/// Размер пламени в мире, м, и его высота над полом.
const FLAME_M: f32 = 1.0;
/// Центр пламени на горящем персонаже, м над ступнями.
const BODY_FLAME_Y: f32 = 0.7;
const BODY_FLAME_M: f32 = 1.1;
/// Дальше этого расстояния огонь не рисуется, м.
const SEE_M: f32 = 40.0;

#[allow(clippy::too_many_arguments)]
pub fn draw_fires(
    mut contexts: EguiContexts,
    net: Option<NonSend<Net>>,
    mut hud: ResMut<crate::Hud>,
    remote: Res<RemoteSprites>,
    local: Res<LocalBody>,
    time: Res<Time>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    transforms: Query<&Transform>,
) -> Result {
    let (Some(net), Ok((camera, cam_t))) = (net, cameras.single()) else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("fires"),
    ));
    let t = f64::from(time.elapsed_secs());
    let Some(me) = net.core.net_id() else {
        return Ok(());
    };
    let eye = cam_t.translation();
    let draw = |icons: &mut crate::hud::Icons, pos: Vec3, size_m: f32, rsi: (&str, &str)| {
        let dist = eye.distance(pos);
        if dist > SEE_M {
            return;
        }
        let Ok(p) = camera.world_to_viewport(cam_t, pos) else {
            return;
        };
        let Some((tex, uv)) = icons.animated_in(&ctx, rsi.0, rsi.1, t) else {
            return;
        };
        // Масштаб: метр на расстоянии d занимает столько же пикселей, сколько и у камеры.
        let Ok(q) = camera.world_to_viewport(cam_t, pos + cam_t.right() * size_m) else {
            return;
        };
        let px = (q.x - p.x).abs().clamp(8.0, 600.0);
        painter.image(
            tex,
            egui::Rect::from_center_size(egui::pos2(p.x, p.y), egui::vec2(px, px)),
            uv,
            egui::Color32::WHITE,
        );
    };

    let fires = net
        .core
        .component(me, FIRES_COMPONENT)
        .and_then(decode_fires)
        .unwrap_or_default();
    for f in fires {
        let pos = Vec3::new(f.x, f.y + FLAME_M * 0.5, f.z);
        let state = f.state.clamp(1, 3).to_string();
        draw(
            &mut hud.icons,
            pos,
            FLAME_M,
            ("Effects", &format!("fire.rsi/{state}")),
        );
    }

    let bodies = remote
        .0
        .iter()
        .map(|(id, e)| (Some(*id), *e))
        .chain(local.0.map(|e| (None, e)));
    for (id, e) in bodies {
        let burning = net
            .core
            .component(id.unwrap_or(me), BURNING_COMPONENT)
            .is_some_and(|b| b.first() == Some(&1));
        if !burning {
            continue;
        }
        let Ok(tr) = transforms.get(e) else { continue };
        draw(
            &mut hud.icons,
            tr.translation + Vec3::Y * BODY_FLAME_Y,
            BODY_FLAME_M,
            ("Mobs/Effects", "onfire.rsi/Generic_mob_burning"),
        );
    }
    Ok(())
}
