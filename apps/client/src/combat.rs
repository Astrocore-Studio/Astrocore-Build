//! Бой и здоровье на клиенте: удар по клику, лечение, панель состояния, вспышки урона.

use astrocore_client::{Net, RemoteSprites};
use astrocore_core::NetworkEntityId;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui, input::EguiWantsInput};
use gameplay::{CMD_ATTACK, CMD_HEAL, CRITICAL_AT, DEAD_AT, GROUPS, HealthView, MobState};

/// На какой высоте над ступнями целиться в персонажа, м.
const CHEST_M: f32 = 1.0;
/// Насколько близко луч должен пройти к груди, чтобы считать, что игрок в цель попал, м.
const PICK_RADIUS_M: f32 = 0.55;

/// Ближайший вдоль луча персонаж, в которого можно попасть.
pub fn pick_target(
    origin: Vec3,
    dir: Vec3,
    candidates: &[(NetworkEntityId, Vec3)],
) -> Option<NetworkEntityId> {
    candidates
        .iter()
        .filter_map(|(id, feet)| {
            let chest = *feet + Vec3::Y * CHEST_M;
            let along = (chest - origin).dot(dir);
            if along <= 0.0 {
                return None;
            }
            let miss = (chest - (origin + dir * along)).length();
            (miss <= PICK_RADIUS_M).then_some((*id, along))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
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
    let Some(target) = pick_target(ray.origin, *ray.direction, &candidates) else {
        return;
    };
    if attack {
        net.send_command(CMD_ATTACK, gameplay::attack_data(target));
    } else {
        net.send_command(CMD_HEAL, gameplay::heal_data(active, target));
    }
}

/// Состояние для рисования вспышек: прошлая сумма урона и время последней вспышки.
#[derive(Default)]
pub struct Flash {
    last_total: f32,
    started: f32,
}

fn total(view: &HealthView) -> f32 {
    view.damage.values().sum()
}

/// Панель здоровья справа вверху, вспышка при уроне, затемнение при крите и смерти.
pub fn health_overlay(
    mut contexts: EguiContexts,
    loadout: Res<crate::Loadout>,
    time: Res<Time>,
    mut flash: Local<Flash>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    let view = loadout.health.clone().unwrap_or(HealthView {
        damage: Default::default(),
        state: MobState::Alive,
    });
    let now = time.elapsed_secs();
    let t = total(&view);
    if t > flash.last_total + 0.01 {
        flash.started = now;
    }
    flash.last_total = t;

    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("health_overlay"),
    ));
    let since = now - flash.started;
    if since < 0.5 {
        let a = ((1.0 - since / 0.5) * 110.0) as u8;
        painter.rect_filled(
            screen,
            0.0,
            egui::Color32::from_rgba_unmultiplied(200, 0, 0, a),
        );
    }
    match view.state {
        MobState::Critical => {
            let pulse = 70.0 + 40.0 * (now * 3.0).sin();
            painter.rect_filled(
                screen,
                0.0,
                egui::Color32::from_rgba_unmultiplied(120, 0, 0, pulse as u8),
            );
        }
        MobState::Dead => {
            painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(170));
        }
        MobState::Alive => {}
    }

    egui::Area::new(egui::Id::new("health_panel"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
        .show(&ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_black_alpha(150))
                .inner_margin(8.0)
                .show(ui, |ui| {
                    let (label, color) = match view.state {
                        MobState::Alive => ("В сознании", egui::Color32::from_rgb(120, 200, 120)),
                        MobState::Critical => (
                            "Критическое состояние",
                            egui::Color32::from_rgb(230, 160, 60),
                        ),
                        MobState::Dead => ("Мёртв", egui::Color32::from_rgb(220, 70, 70)),
                    };
                    ui.colored_label(color, label);
                    ui.add(
                        egui::ProgressBar::new((t / DEAD_AT).clamp(0.0, 1.0))
                            .desired_width(160.0)
                            .text(format!("{t:.0} / {DEAD_AT:.0}")),
                    );
                    for (group, kinds) in GROUPS {
                        let sum: f32 = kinds
                            .iter()
                            .map(|k| view.damage.get(*k).copied().unwrap_or(0.0))
                            .sum();
                        if sum > 0.0 {
                            ui.small(format!("{}: {sum:.0}", group_ru(group)));
                        }
                    }
                    ui.small(format!("порог крита {CRITICAL_AT:.0}"));
                });
        });
    if view.state == MobState::Dead {
        egui::Area::new(egui::Id::new("death_banner"))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0))
            .show(&ctx, |ui| {
                ui.label(
                    egui::RichText::new("Вы мертвы")
                        .size(40.0)
                        .color(egui::Color32::from_rgb(220, 70, 70)),
                );
            });
    }
    Ok(())
}

fn group_ru(g: &str) -> &str {
    match g {
        "Brute" => "Травмы",
        "Burn" => "Ожоги",
        "Toxin" => "Токсины",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ray_picks_the_nearest_player_it_passes_close_to() {
        let (a, b, c) = (NetworkEntityId(1), NetworkEntityId(2), NetworkEntityId(3));
        let origin = Vec3::new(0.0, 1.5, 0.0);
        let dir = Vec3::NEG_Z;
        let near = (a, Vec3::new(0.1, 0.0, -2.0));
        let far = (b, Vec3::new(0.0, 0.0, -4.0));
        let aside = (c, Vec3::new(3.0, 0.0, -1.0));
        assert_eq!(pick_target(origin, dir, &[far, near, aside]), Some(a));
        assert_eq!(pick_target(origin, dir, &[aside]), None);
        assert_eq!(
            pick_target(origin, Vec3::Z, &[near]),
            None,
            "позади — не цель"
        );
    }
}
