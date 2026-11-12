//! Статусы тела на экране (`vitals.statuses`): список в углу и эффекты — дрожь и покачивание камеры,
//! чёрный экран слепоты, радужная пелена, сонливость, ночное зрение. Названия статусов — как в SS14
//! без префикса `StatusEffect`.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::Loadout;

/// Подпись статуса для списка; неизвестные не показываются.
pub fn label(name: &str) -> Option<&'static str> {
    Some(match name {
        "Drunk" => "Опьянение",
        "Woozy" => "Головокружение",
        "Jitter" => "Дрожь",
        "Blindness" => "Слепота",
        "SeeingRainbow" => "Радуга перед глазами",
        "Drowsiness" => "Сонливость",
        "NightVision" => "Ночное зрение",
        "Stunned" => "Оглушён",
        "Knockdown" => "Сбит с ног",
        "ForcedSleeping" => "Сон",
        "Muted" => "Нем",
        "Pacified" => "Миролюбие",
        "Adrenaline" => "Адреналин",
        "Frail" => "Хрупкость",
        "Anticoagulant" => "Кровь не сворачивается",
        "PressureImmunity" => "Защита от давления",
        "RadiationProtection" => "Защита от радиации",
        "PainNumbness" => "Обезболивание",
        "Stutter" | "Scrambled" | "OwO" | "Bark" | "RatvarianLanguage" => "Речь искажена",
        n if n.starts_with("Speed:") => "Скорость изменена",
        _ => return None,
    })
}

fn has(statuses: &[(String, f32)], name: &str) -> bool {
    statuses.iter().any(|s| s.0 == name)
}

/// Слой поверх экрана и список статусов.
pub fn overlay(mut contexts: EguiContexts, loadout: Res<Loadout>, time: Res<Time>) {
    let Some(v) = loadout.vitals.as_ref() else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let t = time.elapsed_secs();
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("status-fx"),
    ));
    let s = &v.statuses;
    if has(s, "Blindness") {
        painter.rect_filled(screen, 0.0, egui::Color32::BLACK);
    }
    if has(s, "SeeingRainbow") {
        let hue = (t * 0.25).fract();
        let c = egui::ecolor::Hsva::new(hue, 0.8, 1.0, 0.2);
        painter.rect_filled(screen, 0.0, egui::Color32::from(c));
    }
    if has(s, "Drowsiness") {
        let a = (110.0 + 60.0 * (t * 0.8).sin()) as u8;
        painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(a));
    }
    if has(s, "NightVision") {
        painter.rect_filled(
            screen,
            0.0,
            egui::Color32::from_rgba_unmultiplied(40, 255, 80, 30),
        );
    }
    let shown: Vec<String> = s
        .iter()
        .filter_map(|(n, left)| {
            let l = label(n)?;
            Some(if *left >= 0.0 {
                format!("{l} ({left:.0} с)")
            } else {
                l.to_string()
            })
        })
        .collect();
    if !shown.is_empty() {
        egui::Area::new(egui::Id::new("status-list"))
            .anchor(egui::Align2::RIGHT_TOP, [-8.0, 40.0])
            .interactable(false)
            .show(ctx, |ui| {
                for line in shown {
                    ui.label(
                        egui::RichText::new(line).color(egui::Color32::from_rgb(255, 230, 160)),
                    );
                }
            });
    }
}

/// Дрожь и покачивание: камера смещается после того, как её поставил движок.
pub fn camera_shake(
    loadout: Res<Loadout>,
    time: Res<Time>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
) {
    let Some(v) = loadout.vitals.as_ref() else {
        return;
    };
    let t = time.elapsed_secs();
    let s = &v.statuses;
    let jitter = has(s, "Jitter");
    let sway = has(s, "Drunk") || has(s, "Woozy");
    if !jitter && !sway {
        return;
    }
    for mut tr in &mut cameras {
        if jitter {
            let n = |f: f32| (t * f).sin() * (t * f * 1.7).cos();
            tr.translation += Vec3::new(n(37.0), n(41.0), n(29.0)) * 0.05;
        }
        if sway {
            tr.rotate_local_z((t * 1.3).sin() * 0.03);
        }
    }
}
