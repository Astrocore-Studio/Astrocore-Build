//! Вид горящей сущности на клиенте: слой пламени поверх спрайта (второй рисунок, когда
//! огня много) и свет, растущий с числом «стопок». Образец — `FireVisualsComponent` и
//! `FireVisualizerSystem` клиента SS14 @ 2f4a2aad. Смещение пламени по форме тела
//! (`DisplacementMap`) не переносится.

use astrocore_client::{Sprite, SpriteLayer};
use astrocore_ecs::{EventWorldExt, LifecycleWorldExt, RegistryWorldExt};
use astrocore_light::PointLight;
use astrocore_map::{EntityCoordinates, Transform, TransformWorldExt};
use astrocore_math::{Color, Vec2};
use astrocore_net::{AppearanceChange, AppearanceValue, AppearanceWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::flammable::{FIRE_STACKS, ON_FIRE};

/// Слой пламени (`FireVisualLayers.Fire`).
pub const FIRE_LAYER: &str = "enum.FireVisualLayers.Fire";

fn alternate_stacks() -> i32 {
    3
}
fn energy_per_stack() -> f32 {
    0.5
}
fn radius_per_stack() -> f32 {
    0.3
}
fn max_energy() -> f32 {
    10.0
}
fn max_radius() -> f32 {
    4.0
}
fn orange() -> Color {
    Color::rgba(1.0, 0.647_058_83, 0.0, 1.0)
}

/// Вид огня (`FireVisualsComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FireVisuals {
    #[serde(default = "alternate_stacks")]
    pub fire_stack_alternate_state: i32,
    #[serde(default)]
    pub normal_state: Option<String>,
    #[serde(default)]
    pub alternate_state: Option<String>,
    #[serde(default)]
    pub sprite: Option<String>,
    #[serde(default = "energy_per_stack")]
    pub light_energy_per_stack: f32,
    #[serde(default = "radius_per_stack")]
    pub light_radius_per_stack: f32,
    #[serde(default = "max_energy")]
    pub max_light_energy: f32,
    #[serde(default = "max_radius")]
    pub max_light_radius: f32,
    #[serde(default = "orange")]
    pub light_color: Color,
    #[serde(skip)]
    pub light_entity: Option<Entity>,
}

fn remove_light(world: &mut World, entity: Entity) {
    let light = world
        .get_mut::<FireVisuals>(entity)
        .and_then(|mut v| v.light_entity.take());
    if let Some(light) = light
        && world.get_entity(light).is_ok()
    {
        world.delete_entity(light);
    }
}

/// Обновить пламя (`UpdateAppearance`); слой появляется при первом обновлении
/// (`LayerMapReserve` в `OnComponentInit`).
fn update(world: &mut World, entity: Entity) {
    let Some(visuals) = world.get::<FireVisuals>(entity).cloned() else {
        return;
    };
    let on_fire = matches!(
        world.appearance(entity, ON_FIRE),
        Some(AppearanceValue::Bool(true))
    );
    let stacks = match world.appearance(entity, FIRE_STACKS) {
        Some(AppearanceValue::Float(value)) => *value,
        _ => 0.0,
    };
    let Some(mut sprite) = world.get::<Sprite>(entity).cloned() else {
        return;
    };
    let before = sprite.clone();
    let index = match sprite.layer_index(FIRE_LAYER) {
        Some(index) => index,
        None => {
            sprite.layers.push(SpriteLayer {
                sprite: visuals.sprite.clone(),
                visible: false,
                shader: Some("unshaded".into()),
                map: vec![FIRE_LAYER.to_owned()],
                ..SpriteLayer::default()
            });
            sprite.layers.len() - 1
        }
    };
    sprite.layers[index].visible = on_fire;
    if on_fire {
        let state = if stacks > visuals.fire_stack_alternate_state as f32
            && visuals
                .alternate_state
                .as_deref()
                .is_some_and(|s| !s.is_empty())
        {
            visuals.alternate_state.clone()
        } else {
            visuals.normal_state.clone()
        };
        sprite.layers[index].state = state;
    }
    if sprite != before
        && let Some(mut current) = world.get_mut::<Sprite>(entity)
    {
        *current = sprite;
    }
    if !on_fire {
        remove_light(world, entity);
        return;
    }
    // Свет: радиус от 1,5 и яркость от 1, растут со «стопками».
    let light = match visuals
        .light_entity
        .filter(|l| world.get_entity(*l).is_ok())
    {
        Some(light) => light,
        None => {
            let light = world.spawn_uninitialized();
            world.entity_mut(light).insert(Transform::default());
            let _ = world.set_coordinates(light, EntityCoordinates::new(entity, Vec2::ZERO), None);
            world.entity_mut(light).insert(PointLight::default());
            world.initialize_and_start(light, false);
            if let Some(mut visuals) = world.get_mut::<FireVisuals>(entity) {
                visuals.light_entity = Some(light);
            }
            light
        }
    };
    if let Some(mut point) = world.get_mut::<PointLight>(light) {
        point.color = visuals.light_color;
        point.radius =
            (1.5 + visuals.light_radius_per_stack * stacks).clamp(0.0, visuals.max_light_radius);
        point.energy =
            (1.0 + visuals.light_energy_per_stack * stacks).clamp(0.0, visuals.max_light_energy);
    }
}

pub(crate) fn install(world: &mut World) {
    world.register_data_component::<FireVisuals>();
    world.subscribe::<FireVisuals, AppearanceChange>(|world, entity, _| update(world, entity));
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrocore_map::SpawnWorldExt;

    #[test]
    fn defaults_match_ss14() {
        let visuals: FireVisuals = astrocore_data::from_node(
            &astrocore_data::parse_yaml("sprite: Mobs/Effects/onfire.rsi\nnormalState: a\n")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(visuals.fire_stack_alternate_state, 3);
        assert_eq!(visuals.max_light_radius, 4.0);
        assert_eq!(visuals.light_color, orange());
    }

    #[test]
    fn burning_shows_flame_layer_and_light() {
        let mut world = crate::testing::world_with("");
        install(&mut world);
        let visuals: FireVisuals = astrocore_data::from_node(
            &astrocore_data::parse_yaml(
                "sprite: Mobs/Effects/onfire.rsi\nnormalState: Generic_mob_burning\nalternateState: Standing\n",
            )
            .unwrap(),
        )
        .unwrap();
        let grid = crate::testing::station(&mut world);
        let mob = world
            .spawn_attached("", EntityCoordinates::new(grid, Vec2::new(1.5, 1.5)))
            .unwrap_or_else(|_| {
                let e = world.spawn_uninitialized();
                world.entity_mut(e).insert(Transform::default());
                let _ = world.set_coordinates(
                    e,
                    EntityCoordinates::new(grid, Vec2::new(1.5, 1.5)),
                    None,
                );
                e
            });
        world.entity_mut(mob).insert((
            Sprite::default(),
            visuals,
            astrocore_net::Appearance::default(),
        ));
        world.set_appearance(mob, FIRE_STACKS, 2.0f32);
        world.set_appearance(mob, ON_FIRE, true);
        let sprite = world.get::<Sprite>(mob).unwrap();
        let layer = &sprite.layers[sprite.layer_index(FIRE_LAYER).unwrap()];
        assert!(layer.visible);
        assert_eq!(layer.state.as_deref(), Some("Generic_mob_burning"));
        let light = world.get::<FireVisuals>(mob).unwrap().light_entity.unwrap();
        assert!((world.get::<PointLight>(light).unwrap().radius - 2.1).abs() < 0.001);
        // Много огня — второй рисунок; погасло — слой скрыт, свет убран.
        world.set_appearance(mob, FIRE_STACKS, 5.0f32);
        let sprite = world.get::<Sprite>(mob).unwrap();
        let layer = &sprite.layers[sprite.layer_index(FIRE_LAYER).unwrap()];
        assert_eq!(layer.state.as_deref(), Some("Standing"));
        world.set_appearance(mob, ON_FIRE, false);
        let sprite = world.get::<Sprite>(mob).unwrap();
        assert!(!sprite.layers[sprite.layer_index(FIRE_LAYER).unwrap()].visible);
        assert!(world.get_entity(light).is_err());
    }
}
