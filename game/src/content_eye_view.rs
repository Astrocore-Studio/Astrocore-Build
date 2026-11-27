//! Зум глаза на клиенте: цифровой блок 4 — дальше, 6 — ближе, 5 — обычный (`ZoomOut`, `ZoomIn`,
//! `ResetZoom`); зум сразу меняется у своей сущности и просится у сервера, глаз камеры плавно
//! тянется к целевому. Образец — `SharedContentEyeSystem` (привязки клавиш),
//! `EyeLerpingSystem` клиента SS14 @ 2f4a2aad.

use astrocore_client::{Eye, Game, KeyboardFocus};
use astrocore_net::{ClientNet, NetEventWorldExt};
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;

use crate::content_eye::{ContentEye, RequestTargetZoom, ZoomKey, lerp_zoom, zoom_after};

fn keys(
    mut events: MessageReader<KeyboardInput>,
    focus: Res<KeyboardFocus>,
    mut game: ResMut<Game>,
) {
    for event in events.read() {
        if event.state != ButtonState::Pressed || focus.0 {
            continue;
        }
        let key = match event.key_code {
            KeyCode::Numpad4 => ZoomKey::Out,
            KeyCode::Numpad6 => ZoomKey::In,
            KeyCode::Numpad5 => ZoomKey::Reset,
            _ => continue,
        };
        let world = &mut game.0;
        let Some(entity) = world.resource::<ClientNet>().attached else {
            continue;
        };
        let Some(eye) = world.get::<ContentEye>(entity).copied() else {
            continue;
        };
        let zoom = zoom_after(&eye, key);
        if let Some(mut own) = world.get_mut::<ContentEye>(entity) {
            own.target_zoom = zoom;
        }
        world.send_net_event_to_server(&RequestTargetZoom {
            target_zoom: zoom,
            ignore_limit: false,
        });
    }
}

/// Глаз камеры тянется к целевому зуму своей сущности; без неё — обычный зум.
fn ease_zoom(game: Res<Game>, time: Res<Time>, mut eye: ResMut<Eye>) {
    let world = &game.0;
    let target = world
        .resource::<ClientNet>()
        .attached
        .and_then(|entity| world.get::<ContentEye>(entity))
        .map_or(1.0, |content| content.target_zoom.x);
    let zoom = lerp_zoom(
        astrocore_math::Vec2::splat(eye.zoom),
        astrocore_math::Vec2::splat(target),
        time.delta_secs(),
    )
    .x;
    if eye.zoom != zoom {
        eye.zoom = zoom;
    }
}

/// Зум камеры.
pub struct ContentEyeViewPlugin;

impl Plugin for ContentEyeViewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, keys.after(bevy::input::InputSystems));
        app.add_systems(Update, ease_zoom);
    }
}
