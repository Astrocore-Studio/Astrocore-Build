//! Радиация: источники излучения. Образец — `RadiationSourceComponent` и
//! `SharedRadiationSystem` SS14 @ 2f4a2aad.

use astrocore_ecs::RegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;

/// Источник излучения (`RadiationSourceComponent`): рад/с на расстоянии ноль, убывание на
/// метр (`slope`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadiationSource {
    #[serde(default = "one")]
    pub intensity: f32,
    #[serde(default = "half")]
    pub slope: f32,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn one() -> f32 {
    1.0
}
fn half() -> f32 {
    0.5
}
fn yes() -> bool {
    true
}

/// Сила источника (`SetIntensity`).
pub fn set_intensity(world: &mut World, entity: Entity, intensity: f32) {
    if let Some(mut source) = world.get_mut::<RadiationSource>(entity) {
        source.intensity = intensity;
    }
}

/// Изменить силу источника, не ниже нуля (`ChangeIntensity`).
pub fn change_intensity(world: &mut World, entity: Entity, delta: f32) {
    if let Some(mut source) = world.get_mut::<RadiationSource>(entity) {
        source.intensity = (source.intensity + delta).max(0.0);
    }
}

pub fn install(world: &mut World, _side: Side) {
    world.register_data_component::<RadiationSource>();
}
