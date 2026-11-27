//! Вид тревог на клиенте: рисунок слоя по уровню тревоги, без питания слои прячутся или
//! получают свой рисунок. Образец — `AtmosAlarmableVisualsComponent` и
//! `AtmosAlarmableVisualsSystem` клиента SS14 @ 2f4a2aad.

use std::collections::BTreeMap;

use astrocore_client::Sprite;
use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_net::{AppearanceChange, AppearanceValue, AppearanceWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::atmos_alarms::ALARM_TYPE;
use crate::power::POWERED_VISUALS;

/// Вид тревоги (`AtmosAlarmableVisualsComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtmosAlarmableVisuals {
    #[serde(default)]
    pub layer_map: String,
    /// Уровень тревоги (`Normal`, `Warning`, `Danger`…) → рисунок.
    #[serde(default)]
    pub alarm_states: BTreeMap<String, String>,
    #[serde(default)]
    pub hide_on_depowered: Option<Vec<String>>,
    #[serde(default)]
    pub set_on_depowered: Option<BTreeMap<String, String>>,
}

fn update(world: &mut World, entity: Entity) {
    let Some(visuals) = world.get::<AtmosAlarmableVisuals>(entity).cloned() else {
        return;
    };
    let Some(AppearanceValue::Bool(powered)) = world.appearance(entity, POWERED_VISUALS).cloned()
    else {
        return;
    };
    let alarm = match world.appearance(entity, ALARM_TYPE) {
        Some(AppearanceValue::Text(text)) => Some(text.clone()),
        _ => None,
    };
    let Some(mut sprite) = world.get::<Sprite>(entity).cloned() else {
        return;
    };
    if sprite.layer_index(&visuals.layer_map).is_none() {
        return;
    }
    let before = sprite.clone();
    for key in visuals.hide_on_depowered.iter().flatten() {
        if let Some(index) = sprite.layer_index(key) {
            sprite.layers[index].visible = powered;
        }
    }
    if !powered {
        for (key, state) in visuals.set_on_depowered.iter().flatten() {
            if let Some(index) = sprite.layer_index(key) {
                sprite.layers[index].state = Some(state.clone());
            }
        }
    }
    if powered
        && let Some(state) = alarm.and_then(|a| visuals.alarm_states.get(&a).cloned())
        && let Some(index) = sprite.layer_index(&visuals.layer_map)
    {
        sprite.layers[index].state = Some(state);
    }
    if sprite != before
        && let Some(mut current) = world.get_mut::<Sprite>(entity)
    {
        *current = sprite;
    }
}

pub(crate) fn install(world: &mut World) {
    world.register_data_component::<AtmosAlarmableVisuals>();
    world.subscribe::<AtmosAlarmableVisuals, AppearanceChange>(|world, entity, _| {
        update(world, entity)
    });
}
