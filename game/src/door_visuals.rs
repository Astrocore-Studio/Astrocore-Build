//! Вид дверей на клиенте: кадры открытия и закрытия, мигание отказа, огни шлюза (питание,
//! болты, аварийный доступ), огни пожарного шлюза («держит давление», «держит огонь»),
//! заварка и панель техобслуживания. Образец — клиентские `DoorSystem`, `AirlockSystem`,
//! `FirelockSystem`, `WeldableVisualizerSystem` и `WiresVisualizerSystem` SS14 @ 2f4a2aad.
//!
//! Анимация SS14 (`AnimationTrackSpriteFlick`) — смена состояния слоя на время анимации с
//! перезапуском кадров; по окончании слой получает состояние открытой или закрытой двери.

use astrocore_client::{Sprite, draw_depth_named};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, Update};
use astrocore_net::{AppearanceChange, AppearanceValue, AppearanceWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::doors::{
    Airlock, BOLT_LIGHTS, CLOSED_LIGHTS, DOOR_STATE, DepthName, Door, DoorState, EMERGENCY_LIGHTS,
    Firelock,
};
use crate::firelock::{PRESSURE_WARNING, TEMPERATURE_WARNING};
use crate::power::POWERED_VISUALS as POWERED;
use crate::weldable::{WELDED_VISUALS, Weldable};

/// Слои двери (`DoorVisualLayers`).
pub const LAYER_BASE: &str = "enum.DoorVisualLayers.Base";
pub const LAYER_UNLIT: &str = "enum.DoorVisualLayers.BaseUnlit";
pub const LAYER_BOLTED: &str = "enum.DoorVisualLayers.BaseBolted";
pub const LAYER_EMERGENCY: &str = "enum.DoorVisualLayers.BaseEmergencyAccess";
pub const LAYER_EMAGGING: &str = "enum.DoorVisualLayers.BaseEmagging";
/// Слой заварки (`WeldableLayers.BaseWelded`).
pub const LAYER_WELDED: &str = "enum.WeldableLayers.BaseWelded";
/// Огни пожарного шлюза (`FirelockVisualLayersPressure`, `FirelockVisualLayersTemperature`).
pub const LAYER_PRESSURE: &str = "enum.FirelockVisualLayersPressure.Base";
pub const LAYER_TEMPERATURE: &str = "enum.FirelockVisualLayersTemperature.Base";
/// Слой панели (`WiresVisualLayers.MaintenancePanel`).
pub const LAYER_PANEL: &str = "enum.WiresVisualLayers.MaintenancePanel";
/// Панель открыта (`WiresVisuals.MaintenancePanelState`).
pub const PANEL_STATE: &str = "enum.WiresVisuals.MaintenancePanelState";

/// Идущая анимация двери: какая и до какого времени игры.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DoorAnimation {
    pub state: DoorState,
    pub until: f64,
}

fn appearance_bool(world: &World, entity: Entity, key: &str) -> Option<bool> {
    match world.appearance(entity, key) {
        Some(AppearanceValue::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn door_state(world: &World, entity: Entity) -> DoorState {
    match world.appearance(entity, DOOR_STATE) {
        Some(AppearanceValue::Text(name)) => DoorState::from_name(name).unwrap_or_default(),
        _ => DoorState::Closed,
    }
}

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

/// Состояние слоя; `None` — слой скрыт (как `LayerSetRsiState(null)`).
fn set_layer(sprite: &mut Sprite, key: &str, state: Option<&str>, restart: bool) {
    let Some(index) = sprite.layer_index(key) else {
        return;
    };
    let layer = &mut sprite.layers[index];
    match state {
        Some(state) => {
            layer.state = Some(state.to_owned());
            if restart {
                layer.animation_restart = layer.animation_restart.wrapping_add(1).max(1);
            }
        }
        None => layer.state = None,
    }
}

fn set_visible(sprite: &mut Sprite, key: &str, visible: bool) {
    if let Some(index) = sprite.layer_index(key) {
        sprite.layers[index].visible = visible;
    }
}

fn depth(name: &Option<DepthName>) -> Option<astrocore_client::DrawDepth> {
    match name.as_ref()? {
        DepthName::Number(value) => Some(astrocore_client::DrawDepth(*value)),
        DepthName::Name(name) => draw_depth_named(name),
    }
}

/// Состояния слоёв открытой или закрытой двери (`OpenSpriteStates`, `ClosedSpriteStates`).
fn rest_states(
    door: &Door,
    airlock: Option<&Airlock>,
    firelock: Option<&Firelock>,
    open: bool,
) -> Vec<(&'static str, Option<String>)> {
    let mut states = vec![(
        LAYER_BASE,
        if open {
            door.open_sprite_state.clone()
        } else {
            door.closed_sprite_state.clone()
        },
    )];
    if let Some(airlock) = airlock {
        if airlock.open_unlit_visible {
            states.push((
                LAYER_UNLIT,
                if open {
                    airlock.sprite_states.open_sprite_state.clone()
                } else {
                    airlock.sprite_states.closed_sprite_state.clone()
                },
            ));
        }
        if airlock.animate_panel {
            states.push((
                LAYER_PANEL,
                if open {
                    None
                } else {
                    airlock.sprite_states.open_panel_sprite_state.clone()
                },
            ));
        }
    }
    // Пожарный шлюз добавляет огни и панель (`FirelockSystem.OnComponentStartup`).
    if let Some(firelock) = firelock {
        states.push((LAYER_UNLIT, firelock.warning_light_sprite_state.clone()));
        if firelock.animate_panel {
            states.push((
                LAYER_PANEL,
                if open {
                    None
                } else {
                    firelock.open_panel_sprite_state.clone()
                },
            ));
        }
    }
    states
}

/// Кадры анимации состояния: слой → состояние.
fn flick_states(
    door: &Door,
    airlock: Option<&Airlock>,
    firelock: Option<&Firelock>,
    state: DoorState,
) -> (Vec<(&'static str, Option<String>)>, f64) {
    match state {
        DoorState::Opening => {
            let mut states = vec![(LAYER_BASE, door.opening_sprite_state.clone())];
            if let Some(airlock) = airlock {
                states.push((
                    LAYER_UNLIT,
                    airlock.sprite_states.opening_sprite_state.clone(),
                ));
                if airlock.animate_panel {
                    states.push((
                        LAYER_PANEL,
                        airlock.sprite_states.opening_panel_sprite_state.clone(),
                    ));
                }
            }
            if let Some(firelock) = firelock {
                states.push((LAYER_UNLIT, firelock.opening_light_sprite_state.clone()));
                if firelock.animate_panel {
                    states.push((LAYER_PANEL, firelock.opening_panel_sprite_state.clone()));
                }
            }
            (states, door.opening_animation_time)
        }
        DoorState::Closing => {
            let mut states = vec![(LAYER_BASE, door.closing_sprite_state.clone())];
            if let Some(airlock) = airlock {
                states.push((
                    LAYER_UNLIT,
                    airlock.sprite_states.closing_sprite_state.clone(),
                ));
                if airlock.animate_panel {
                    states.push((
                        LAYER_PANEL,
                        airlock.sprite_states.closing_panel_sprite_state.clone(),
                    ));
                }
            }
            if let Some(firelock) = firelock {
                states.push((LAYER_UNLIT, firelock.closing_light_sprite_state.clone()));
                if firelock.animate_panel {
                    states.push((LAYER_PANEL, firelock.closing_panel_sprite_state.clone()));
                }
            }
            (states, door.closing_animation_time)
        }
        DoorState::Denying => match airlock {
            Some(airlock) => (
                vec![(LAYER_UNLIT, airlock.sprite_states.deny_sprite_state.clone())],
                airlock.sprite_states.deny_animation_time,
            ),
            None => (Vec::new(), 0.0),
        },
        DoorState::Emagging => (
            vec![(LAYER_EMAGGING, door.emagging_sprite_state.clone())],
            door.emagging_animation_time,
        ),
        DoorState::Open | DoorState::Closed => (Vec::new(), 0.0),
    }
}

/// Вид по состоянию двери (`OnAppearanceChange`, `UpdateAppearanceForDoorState`).
fn apply_door(world: &mut World, entity: Entity) {
    let Some(door) = world.get::<Door>(entity).cloned() else {
        return;
    };
    let airlock = world.get::<Airlock>(entity).cloned();
    let firelock = world.get::<Firelock>(entity).cloned();
    let state = door_state(world, entity);
    let running = world.get::<DoorAnimation>(entity).copied();
    let time = now(world);
    let Some(mut sprite) = world.get::<Sprite>(entity).cloned() else {
        return;
    };
    if let Some(depth) = depth(if state == DoorState::Open {
        &door.open_draw_depth
    } else {
        &door.closed_draw_depth
    }) {
        sprite.drawdepth = depth;
    }
    set_visible(&mut sprite, LAYER_EMAGGING, state == DoorState::Emagging);
    let mut animation = running;
    match state {
        DoorState::Open | DoorState::Closed => {
            let opposite = if state == DoorState::Open {
                DoorState::Closing
            } else {
                DoorState::Opening
            };
            let same = if state == DoorState::Open {
                DoorState::Opening
            } else {
                DoorState::Closing
            };
            match running {
                // Анимация к этому состоянию ещё идёт: покой наступит по её окончании.
                Some(current) if current.state == same => {}
                _ => {
                    if running.is_some_and(|r| r.state == opposite) {
                        animation = None;
                    }
                    for (key, layer_state) in rest_states(
                        &door,
                        airlock.as_ref(),
                        firelock.as_ref(),
                        state == DoorState::Open,
                    ) {
                        set_layer(&mut sprite, key, layer_state.as_deref(), false);
                    }
                    animation = animation.filter(|a| a.state == same);
                }
            }
        }
        _ => {
            if running.is_none_or(|r| r.state != state) {
                let (states, length) =
                    flick_states(&door, airlock.as_ref(), firelock.as_ref(), state);
                if length > 0.0 && !states.is_empty() {
                    for (key, layer_state) in states {
                        set_layer(&mut sprite, key, layer_state.as_deref(), true);
                    }
                    animation = Some(DoorAnimation {
                        state,
                        until: time + length,
                    });
                }
            }
        }
    }
    if let Some(airlock) = &airlock {
        apply_airlock_lights(world, entity, airlock, state, &mut sprite);
    }
    if firelock.is_some() {
        apply_firelock_lights(world, entity, state, &mut sprite);
    }
    if world.get::<Sprite>(entity) != Some(&sprite) {
        world.entity_mut(entity).insert(sprite);
    }
    match animation {
        Some(animation) => {
            world.entity_mut(entity).insert(animation);
        }
        None => {
            world.entity_mut(entity).remove::<DoorAnimation>();
        }
    }
}

/// Огни шлюза (`AirlockSystem.OnAppearanceChange`).
fn apply_airlock_lights(
    world: &World,
    entity: Entity,
    airlock: &Airlock,
    state: DoorState,
    sprite: &mut Sprite,
) {
    let powered = appearance_bool(world, entity, POWERED).unwrap_or(false);
    let mut show_unlit = false;
    let mut show_bolted = false;
    let mut show_emergency = false;
    if powered {
        show_bolted = appearance_bool(world, entity, BOLT_LIGHTS).unwrap_or(false)
            && state == DoorState::Closed;
        show_emergency = appearance_bool(world, entity, EMERGENCY_LIGHTS).unwrap_or(false);
        if !show_bolted && !show_emergency {
            if matches!(
                state,
                DoorState::Closing | DoorState::Opening | DoorState::Denying | DoorState::Closed
            ) {
                show_unlit = true;
            }
            if state == DoorState::Open && airlock.open_unlit_visible {
                show_unlit = true;
            }
            if state == DoorState::Closed
                && appearance_bool(world, entity, CLOSED_LIGHTS).unwrap_or(false)
            {
                show_unlit = true;
            }
        }
    }
    set_visible(sprite, LAYER_UNLIT, show_unlit);
    set_visible(sprite, LAYER_BOLTED, show_bolted);
    if airlock.emergency_access_layer {
        let idle = !matches!(
            state,
            DoorState::Open | DoorState::Opening | DoorState::Closing
        );
        set_visible(
            sprite,
            LAYER_EMERGENCY,
            show_emergency && idle && !show_bolted,
        );
    }
    if airlock.open_unlit_visible {
        match state {
            DoorState::Open => set_layer(
                sprite,
                LAYER_UNLIT,
                airlock.sprite_states.open_sprite_state.as_deref(),
                false,
            ),
            DoorState::Closed => set_layer(
                sprite,
                LAYER_UNLIT,
                airlock.sprite_states.closed_sprite_state.as_deref(),
                false,
            ),
            _ => {}
        }
    }
}

/// Огни пожарного шлюза (`FirelockSystem.OnAppearanceChange`): огни движения, отказа и
/// «держит» горят, если не опущены болты; предупреждения — только у закрытого.
fn apply_firelock_lights(world: &World, entity: Entity, state: DoorState, sprite: &mut Sprite) {
    let bolted = appearance_bool(world, entity, BOLT_LIGHTS).unwrap_or(false);
    let unlit = matches!(
        state,
        DoorState::Closing | DoorState::Opening | DoorState::Denying
    ) || appearance_bool(world, entity, CLOSED_LIGHTS).unwrap_or(false);
    set_visible(sprite, LAYER_UNLIT, unlit && !bolted);
    set_visible(sprite, LAYER_BOLTED, bolted);
    let warning = matches!(state, DoorState::Closed | DoorState::Denying);
    let pressure = appearance_bool(world, entity, PRESSURE_WARNING).unwrap_or(false);
    let temperature = appearance_bool(world, entity, TEMPERATURE_WARNING).unwrap_or(false);
    set_visible(sprite, LAYER_PRESSURE, pressure && warning);
    set_visible(sprite, LAYER_TEMPERATURE, temperature && warning);
}

/// Заварка и панель (`WeldableVisualizerSystem`, `WiresVisualizerSystem`).
fn apply_weld_and_panel(world: &mut World, entity: Entity) {
    let welded = appearance_bool(world, entity, WELDED_VISUALS).unwrap_or(false);
    let panel = appearance_bool(world, entity, PANEL_STATE).unwrap_or(false);
    let has_weldable = world.get::<Weldable>(entity).is_some();
    let Some(sprite) = world.get::<Sprite>(entity) else {
        return;
    };
    let mut updated = sprite.clone();
    if has_weldable {
        set_visible(&mut updated, LAYER_WELDED, welded);
    }
    set_visible(&mut updated, LAYER_PANEL, panel);
    if world.get::<Sprite>(entity) != Some(&updated) {
        world.entity_mut(entity).insert(updated);
    }
}

/// Окончание анимаций (`OnAnimationCompleted`).
fn finish_animations(world: &mut World) {
    let time = now(world);
    let done: Vec<Entity> = world
        .query::<(Entity, &DoorAnimation)>()
        .iter(world)
        .filter(|(_, animation)| animation.until <= time)
        .map(|(entity, _)| entity)
        .collect();
    for entity in done {
        world.entity_mut(entity).remove::<DoorAnimation>();
        let state = door_state(world, entity);
        let (Some(door), Some(mut sprite)) = (
            world.get::<Door>(entity).cloned(),
            world.get::<Sprite>(entity).cloned(),
        ) else {
            continue;
        };
        let airlock = world.get::<Airlock>(entity).cloned();
        let firelock = world.get::<Firelock>(entity).cloned();
        match state {
            DoorState::Open | DoorState::Closed => {
                for (key, layer_state) in rest_states(
                    &door,
                    airlock.as_ref(),
                    firelock.as_ref(),
                    state == DoorState::Open,
                ) {
                    set_layer(&mut sprite, key, layer_state.as_deref(), false);
                }
            }
            _ => {}
        }
        if let Some(airlock) = &airlock {
            apply_airlock_lights(world, entity, airlock, state, &mut sprite);
        }
        if firelock.is_some() {
            apply_firelock_lights(world, entity, state, &mut sprite);
        }
        world.entity_mut(entity).insert(sprite);
        apply_weld_and_panel(world, entity);
    }
}

pub(crate) fn install(world: &mut World) {
    world.subscribe::<Door, AppearanceChange>(|world, entity, _| {
        apply_door(world, entity);
        apply_weld_and_panel(world, entity);
    });
    world.subscribe::<Weldable, AppearanceChange>(|world, entity, _| {
        if world.get::<Door>(entity).is_none() {
            apply_weld_and_panel(world, entity);
        }
    });
    world.add_game_systems(Update, finish_animations);
}

#[cfg(test)]
mod tests {
    use astrocore_ecs::LifecycleWorldExt;

    use super::*;

    const SPRITE: &str = r#"
sprite: Structures/Doors/Airlocks/Standard/firelock-base.rsi
layers:
- state: closed
  map: ["enum.DoorVisualLayers.Base"]
- state: closed_unlit
  map: ["enum.DoorVisualLayers.BaseUnlit"]
  visible: false
- state: bolted_unlit
  map: ["enum.DoorVisualLayers.BaseBolted"]
  visible: false
- state: pressure_unlit
  visible: false
  map: ["enum.FirelockVisualLayersPressure.Base"]
- state: temperature_unlit
  visible: false
  map: ["enum.FirelockVisualLayersTemperature.Base"]
"#;

    fn visible(world: &World, entity: Entity, key: &str) -> bool {
        let sprite = world.get::<Sprite>(entity).unwrap();
        sprite.layers[sprite.layer_index(key).unwrap()].visible
    }

    #[test]
    fn firelock_warning_lights_follow_appearance_only_when_closed() {
        let mut world = crate::testing::world_with("");
        install(&mut world);
        let sprite: Sprite =
            astrocore_data::from_node(&astrocore_data::parse_yaml(SPRITE).unwrap()).unwrap();
        let door: Door = astrocore_data::from_node(&astrocore_data::DataNode::map(
            astrocore_data::DataMap::new(),
        ))
        .unwrap();
        let entity = world.spawn_uninitialized();
        world.entity_mut(entity).insert((
            sprite,
            door,
            Firelock::default(),
            astrocore_net::Appearance::default(),
        ));
        world.set_appearance(entity, DOOR_STATE, "Closed");
        world.set_appearance(entity, PRESSURE_WARNING, true);
        world.set_appearance(entity, CLOSED_LIGHTS, true);
        assert!(visible(&world, entity, LAYER_PRESSURE));
        assert!(!visible(&world, entity, LAYER_TEMPERATURE));
        assert!(visible(&world, entity, LAYER_UNLIT));
        world.set_appearance(entity, TEMPERATURE_WARNING, true);
        assert!(visible(&world, entity, LAYER_TEMPERATURE));
        // Болты гасят огни движения и горят сами.
        world.set_appearance(entity, BOLT_LIGHTS, true);
        assert!(!visible(&world, entity, LAYER_UNLIT));
        assert!(visible(&world, entity, LAYER_BOLTED));
        // Открытый шлюз предупреждений не показывает.
        world.set_appearance(entity, BOLT_LIGHTS, false);
        world.set_appearance(entity, DOOR_STATE, "Open");
        assert!(!visible(&world, entity, LAYER_PRESSURE));
        assert!(!visible(&world, entity, LAYER_TEMPERATURE));
    }
}
