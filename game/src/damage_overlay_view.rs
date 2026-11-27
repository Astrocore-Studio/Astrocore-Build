//! Затемнение экрана от урона на клиенте: красная пульсация боли, темнота удушья (в крите —
//! постоянная, изредка «моргает»), белая дымка крита, которая сжимается к смерти и после
//! смерти тает. Уровни плавно тянутся к пришедшим с сервера. Образец — `DamageOverlay` и
//! `DamageOverlaySystem` клиента, шейдер `gradient_circle_mask` SS14 @ 2f4a2aad.

use astrocore_client::Game;
use astrocore_net::ClientNet;
use bevy::asset::{AssetPath, embedded_asset, embedded_path};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::ui_render::UiMaterialPlugin;
use bevy::ui_render::ui_material::{MaterialNode, UiMaterial};

use crate::damage_overlay::DamageOverlay;
use crate::mobs::MobState;

/// Параметры круга (`gradient_circle_mask`): цвет и прозрачность снаружи; время, радиусы.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Default)]
pub struct GradientCircleMaterial {
    #[uniform(0)]
    pub color: Vec4,
    #[uniform(0)]
    pub a: Vec4,
    #[uniform(0)]
    pub b: Vec4,
}

impl UiMaterial for GradientCircleMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Path(
            AssetPath::from_path_buf(embedded_path!("damage_overlay.wesl")).with_source("embedded"),
        )
    }
}

/// Слой затемнения.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Layer {
    Brute,
    Oxygen,
    Crit,
}

/// Сглаженные уровни (`_oldPainLevel` и другие).
#[derive(Resource, Debug, Clone, Copy)]
pub struct OverlayLevels {
    pub pain: f32,
    pub oxygen: f32,
    pub crit: f32,
    pub dead: f32,
}

impl Default for OverlayLevels {
    fn default() -> Self {
        Self {
            pain: 0.0,
            oxygen: 0.0,
            crit: 0.0,
            dead: 1.0,
        }
    }
}

/// Шаг сглаживания (`GetDiff`): пятая часть разницы за кадр в 1/5 с, не дальше цели.
pub fn ease(value: f32, frame_time: f32) -> f32 {
    let step = value * 5.0 * frame_time;
    step.clamp(-value.abs(), value.abs())
}

fn approach(current: &mut f32, target: f32, frame_time: f32) {
    if (current.abs() - target.abs()).abs() < 0.001 && (*current - target).abs() < 0.001 {
        *current = target;
    } else {
        *current += ease(target - *current, frame_time);
    }
}

/// Параметры слоя по уровням: `None` — слой не виден. `time` — секунды, `levels` —
/// сглаженные уровни.
pub fn layer_params(
    layer: u8,
    state: MobState,
    levels: &OverlayLevels,
    time: f32,
) -> Option<(Vec4, Vec4, Vec4)> {
    match layer {
        // Боль: красная пульсация, пока не в крите.
        0 => {
            let level = levels.pain;
            if level <= 0.0 || levels.crit > 0.0 {
                return None;
            }
            let (outer_max, outer_min, inner_max, inner_min) = (2.0, 0.8, 0.6, 0.2);
            let outer = outer_max - level * (outer_max - outer_min);
            let inner = inner_max - level * (inner_max - inner_min);
            let pulse = (time * 3.0).sin().max(0.0);
            Some((
                Vec4::new(1.0, 0.0, 0.0, 0.8),
                Vec4::new(pulse, outer, outer + 0.2, inner),
                Vec4::new(inner + 0.02, 0.0, 0.0, 0.0),
            ))
        }
        // Удушье, а в крите — постоянная темнота.
        1 => {
            let level = if state != MobState::Critical {
                levels.oxygen
            } else {
                1.0
            };
            if level <= 0.0 {
                return None;
            }
            let (outer_max, outer_min, inner) = (0.6, 0.06, 0.02);
            let outer = outer_max - level * (outer_max - outer_min);
            let darkness = if levels.crit > 0.0 {
                let t = time * 2.0;
                let blink =
                    ((t).sin() + 2.0 * (2.0 * t / 4.0).sin() + (t / 4.0).sin() - 3.0).max(0.0);
                if blink > 0.0 { 1.0 - blink / 1.5 } else { 1.0 }
            } else {
                (0.3 * level.ln() + 1.0).min(0.98)
            };
            Some((
                Vec4::new(0.0, 0.0, 0.0, darkness),
                Vec4::new(0.0, outer, outer + 0.2, inner),
                Vec4::new(inner, 0.0, 0.0, 0.0),
            ))
        }
        // Белая дымка крита; после смерти тает.
        _ => {
            let level = if state != MobState::Dead {
                levels.crit
            } else {
                levels.dead
            };
            if level <= 0.0 {
                return None;
            }
            let (outer_max, outer_min, inner_max, inner_min) = (2.0, 1.0, 0.6, 0.02);
            let outer = outer_max - level * (outer_max - outer_min);
            let inner = inner_max - level * (inner_max - inner_min);
            let pulse = time.sin().max(0.0);
            Some((
                Vec4::new(1.0, 1.0, 1.0, 1.0),
                Vec4::new(pulse, outer, outer + 0.2, inner),
                Vec4::new(inner + 0.005, 0.0, 0.0, 0.0),
            ))
        }
    }
}

fn spawn_layers(mut commands: Commands, mut materials: ResMut<Assets<GradientCircleMaterial>>) {
    for layer in [Layer::Brute, Layer::Oxygen, Layer::Crit] {
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            MaterialNode(materials.add(GradientCircleMaterial::default())),
            GlobalZIndex(-10),
            Visibility::Hidden,
            layer,
        ));
    }
}

#[allow(clippy::type_complexity)]
fn update_layers(
    game: Res<Game>,
    time: Res<Time<Real>>,
    mut levels: ResMut<OverlayLevels>,
    mut materials: ResMut<Assets<GradientCircleMaterial>>,
    mut layers: Query<(
        &Layer,
        &MaterialNode<GradientCircleMaterial>,
        &mut Visibility,
    )>,
) {
    let world = &game.0;
    let overlay = world
        .resource::<ClientNet>()
        .attached
        .and_then(|entity| world.get::<DamageOverlay>(entity))
        .cloned();
    let Some(overlay) = overlay else {
        for (_, _, mut visibility) in &mut layers {
            *visibility = Visibility::Hidden;
        }
        return;
    };
    let frame = time.delta_secs();
    let state = overlay.current_state;
    // После смерти дымка тает; до смерти — полная.
    if state != MobState::Dead {
        levels.dead = 1.0;
    } else if levels.dead.abs() > 0.001 {
        let dead = levels.dead;
        levels.dead += ease(-dead, frame);
    } else {
        levels.dead = 0.0;
    }
    approach(&mut levels.pain, overlay.pain_level, frame);
    approach(&mut levels.oxygen, overlay.oxygen_level, frame);
    approach(&mut levels.crit, overlay.crit_level, frame);
    let seconds = time.elapsed_secs();
    for (layer, node, mut visibility) in &mut layers {
        let index = match layer {
            Layer::Brute => 0,
            Layer::Oxygen => 1,
            Layer::Crit => 2,
        };
        match layer_params(index, state, &levels, seconds) {
            Some((color, a, b)) => {
                if let Some(mut material) = materials.get_mut(&node.0) {
                    material.color = color;
                    material.a = a;
                    material.b = b;
                }
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
}

/// Затемнение экрана от урона.
pub struct DamageOverlayViewPlugin;

impl Plugin for DamageOverlayViewPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "damage_overlay.wesl");
        app.add_plugins(UiMaterialPlugin::<GradientCircleMaterial>::default())
            .init_resource::<OverlayLevels>()
            .add_systems(Startup, spawn_layers)
            .add_systems(Update, update_layers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_never_overshoots() {
        assert!((ease(1.0, 0.01) - 0.05).abs() < 1e-6);
        assert_eq!(ease(1.0, 1.0), 1.0);
        assert_eq!(ease(-0.5, 1.0), -0.5);
    }

    #[test]
    fn layers_show_by_state() {
        let mut levels = OverlayLevels {
            pain: 0.5,
            ..OverlayLevels::default()
        };
        assert!(layer_params(0, MobState::Alive, &levels, 0.0).is_some());
        assert!(layer_params(1, MobState::Alive, &levels, 0.0).is_none());
        assert!(layer_params(2, MobState::Alive, &levels, 0.0).is_none());
        // Крит: боль не видна, темнота постоянная, белая дымка по уровню.
        levels.crit = 0.5;
        assert!(layer_params(0, MobState::Critical, &levels, 0.0).is_none());
        assert!(layer_params(1, MobState::Critical, &levels, 0.0).is_some());
        let (_, a, _) = layer_params(2, MobState::Critical, &levels, 0.0).unwrap();
        assert!((a.y - 1.5).abs() < 1e-6);
    }
}
