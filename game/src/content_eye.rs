//! Зум глаза: целевой зум сущности игрока (`targetZoom`) и наибольший (`maxZoom`, у призрака
//! 1,44); клавиши цифрового блока — 4 дальше, 6 ближе (в 1,5 раза), 5 — обычный. Клиент просит
//! сервер, сервер ограничивает зум от 1,5⁻³ до наибольшего (без ограничения — только с правом
//! `DEBUG`), компонент приходит клиенту, и зум глаза плавно тянется к целевому (8 в секунду).
//! Образец — `ContentEyeComponent`, `SharedContentEyeSystem` (`ZoomIn`, `ZoomOut`, `ResetZoom`,
//! `RequestTargetZoomEvent`), `EyeLerpingSystem.UpdateZoom` SS14 @ 2f4a2aad.
//!
//! Нет пока: масштаба PVS (`RequestPvsScaleEvent`) и смещения глаза (`GetEyeOffsetEvent`).

use astrocore_math::Vec2;
use astrocore_net::{NetEvent, NetEventWorldExt, NetRegistryWorldExt, Session};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;

/// Во сколько раз меняет зум одно нажатие (`ZoomMod`).
pub const ZOOM_MOD: f32 = 1.5;
/// Обычный зум (`DefaultZoom`).
pub const DEFAULT_ZOOM: Vec2 = Vec2::ONE;

/// Наименьший зум — самое близкое (`MinZoom`).
pub fn min_zoom() -> Vec2 {
    DEFAULT_ZOOM * ZOOM_MOD.powi(-3)
}

/// Зум глаза сущности (`ContentEyeComponent`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContentEye {
    /// К какому зуму тянется глаз.
    pub target_zoom: Vec2,
    /// Насколько далеко можно отдалить.
    pub max_zoom: Vec2,
}

impl Default for ContentEye {
    fn default() -> Self {
        Self {
            target_zoom: DEFAULT_ZOOM,
            max_zoom: DEFAULT_ZOOM,
        }
    }
}

/// Просьба клиента задать зум (`RequestTargetZoomEvent`). Клиент → сервер.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RequestTargetZoom {
    pub target_zoom: Vec2,
    pub ignore_limit: bool,
}

/// Зум в пределах (`Clamp`).
pub fn clamp_zoom(zoom: Vec2, eye: &ContentEye) -> Vec2 {
    zoom.clamp(min_zoom(), eye.max_zoom.max(min_zoom()))
}

/// Задать целевой зум (`SetZoom`).
pub fn set_zoom(world: &mut World, entity: Entity, zoom: Vec2, ignore_limits: bool) {
    let Some(mut eye) = world.get_mut::<ContentEye>(entity) else {
        return;
    };
    let zoom = if ignore_limits {
        zoom
    } else {
        clamp_zoom(zoom, &eye)
    };
    if eye.target_zoom != zoom {
        eye.target_zoom = zoom;
    }
}

/// Задать наибольший зум (`SetMaxZoom`): целевой подтягивается в пределы.
pub fn set_max_zoom(world: &mut World, entity: Entity, max: Vec2) {
    let Some(mut eye) = world.get_mut::<ContentEye>(entity) else {
        return;
    };
    eye.max_zoom = max;
    let clamped = clamp_zoom(eye.target_zoom, &eye);
    eye.target_zoom = clamped;
}

/// Нажатия зума (`ZoomIn`, `ZoomOut`, `ResetZoom`): новый целевой зум.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomKey {
    In,
    Out,
    Reset,
}

/// Целевой зум после нажатия.
pub fn zoom_after(eye: &ContentEye, key: ZoomKey) -> Vec2 {
    let zoom = match key {
        ZoomKey::In => eye.target_zoom / ZOOM_MOD,
        ZoomKey::Out => eye.target_zoom * ZOOM_MOD,
        ZoomKey::Reset => DEFAULT_ZOOM,
    };
    clamp_zoom(zoom, eye)
}

/// Шаг плавного зума к целевому (`EyeLerpingSystem.UpdateZoom`).
pub fn lerp_zoom(current: Vec2, target: Vec2, frame_time: f32) -> Vec2 {
    let diff = target - current;
    if diff.length_squared() < 0.00001 {
        return target;
    }
    current + diff * (8.0 * frame_time).min(1.0)
}

fn receive(world: &mut World, event: &mut NetEvent<RequestTargetZoom>) {
    let Some(session) = event.session else {
        return;
    };
    let Some(entity) = world.get::<Session>(session).and_then(|s| s.attached) else {
        return;
    };
    let ignore = event.event.ignore_limit
        && crate::admin_manager::has_flag(world, session, crate::admin_manager::flags::DEBUG);
    set_zoom(world, entity, event.event.target_zoom, ignore);
}

pub fn install(world: &mut World, side: Side) {
    use astrocore_ecs::RegistryWorldExt;
    world.register_data_component::<ContentEye>();
    world.register_net_serde::<ContentEye>();
    world.register_net_event::<RequestTargetZoom>();
    if side.is_server() {
        world.subscribe_net_event::<RequestTargetZoom>(receive);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_by_one_and_a_half_within_limits() {
        let mut eye = ContentEye::default();
        // Отдалить нельзя: наибольший зум — обычный.
        assert_eq!(zoom_after(&eye, ZoomKey::Out), Vec2::ONE);
        eye.target_zoom = zoom_after(&eye, ZoomKey::In);
        assert!((eye.target_zoom.x - 1.0 / 1.5).abs() < 1e-6);
        for _ in 0..5 {
            eye.target_zoom = zoom_after(&eye, ZoomKey::In);
        }
        assert!((eye.target_zoom.x - min_zoom().x).abs() < 1e-6);
        assert_eq!(zoom_after(&eye, ZoomKey::Reset), Vec2::ONE);
        // Призрак может отдалить до 1,44.
        eye.max_zoom = Vec2::splat(1.44);
        eye.target_zoom = Vec2::ONE;
        assert_eq!(zoom_after(&eye, ZoomKey::Out), Vec2::splat(1.44));
    }

    #[test]
    fn eye_zoom_eases_towards_the_target() {
        let step = lerp_zoom(Vec2::ONE, Vec2::splat(2.0), 0.05);
        assert!((step.x - 1.4).abs() < 1e-6);
        assert_eq!(
            lerp_zoom(Vec2::ONE, Vec2::splat(2.0), 1.0),
            Vec2::splat(2.0)
        );
        assert_eq!(
            lerp_zoom(Vec2::splat(1.999), Vec2::splat(2.0), 0.0),
            Vec2::splat(2.0)
        );
    }

    #[test]
    fn server_clamps_requests_unless_debug_admin() {
        let mut world = crate::testing::world_with("");
        let entity = world.spawn(ContentEye::default()).id();
        set_zoom(&mut world, entity, Vec2::splat(3.0), false);
        assert_eq!(
            world.get::<ContentEye>(entity).unwrap().target_zoom,
            Vec2::ONE
        );
        set_zoom(&mut world, entity, Vec2::splat(3.0), true);
        assert_eq!(
            world.get::<ContentEye>(entity).unwrap().target_zoom,
            Vec2::splat(3.0)
        );
        set_max_zoom(&mut world, entity, Vec2::splat(1.44));
        assert_eq!(
            world.get::<ContentEye>(entity).unwrap().target_zoom,
            Vec2::splat(1.44)
        );
    }
}
