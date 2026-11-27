//! Газовый баллон: предмет с запасом газа. Пока только хранит смесь из данных — её
//! расходует коллектор излучения. Образец — `GasTankComponent` SS14 @ 2f4a2aad.
//!
//! Нет пока: выпуска газа, дыхания из баллона, клапана, окна, взрыва от давления (5.5в,
//! 5.6б).

use std::collections::BTreeMap;

use astrocore_ecs::{EventWorldExt, MapInit, RegistryWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::atmos::{GasMixture, gas};

/// Газы смеси в данных: список чисел по порядку газов или словарь по именам.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MolesData {
    List(Vec<f32>),
    Named(BTreeMap<String, f32>),
}

/// Смесь в данных (`GasMixture`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AirData {
    #[serde(default)]
    pub volume: f32,
    #[serde(default = "room")]
    pub temperature: f32,
    #[serde(default)]
    pub moles: Option<MolesData>,
}

impl Default for AirData {
    fn default() -> Self {
        Self {
            volume: 0.0,
            temperature: room(),
            moles: None,
        }
    }
}

fn room() -> f32 {
    gas::T20C
}

impl AirData {
    /// Смесь по данным.
    pub fn mixture(&self) -> GasMixture {
        let mut mixture = GasMixture::new(self.volume).with_temperature(self.temperature);
        match &self.moles {
            Some(MolesData::List(list)) => {
                for (index, moles) in list.iter().enumerate().take(gas::GAS_COUNT) {
                    mixture.moles[index] = *moles;
                }
            }
            Some(MolesData::Named(map)) => {
                for (name, moles) in map {
                    if let Some(index) = gas::gas_index(name) {
                        mixture.moles[index] = *moles;
                    }
                }
            }
            None => {}
        }
        mixture
    }
}

/// Газовый баллон (`GasTankComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasTank {
    #[serde(default)]
    pub air: AirData,
    #[serde(default)]
    pub release_pressure: f32,
    /// Смесь в баллоне; заполняется из `air` при появлении.
    #[serde(skip)]
    pub mixture: Option<GasMixture>,
}

/// Смесь баллона.
pub fn tank_air(world: &World, tank: Entity) -> Option<&GasMixture> {
    world.get::<GasTank>(tank)?.mixture.as_ref()
}

/// Изменить смесь баллона.
pub fn with_tank_air<R>(
    world: &mut World,
    tank: Entity,
    f: impl FnOnce(&mut GasMixture) -> R,
) -> Option<R> {
    let mut component = world.get_mut::<GasTank>(tank)?;
    component.mixture.as_mut().map(f)
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<GasTank>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<GasTank, MapInit>(|world, tank, _| {
        if let Some(mut component) = world.get_mut::<GasTank>(tank)
            && component.mixture.is_none()
        {
            component.mixture = Some(component.air.mixture());
        }
    });
}
