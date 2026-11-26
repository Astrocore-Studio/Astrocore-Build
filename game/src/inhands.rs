//! Предметы в руках на спрайте держащего: слои `inhand-left` и `inhand-right` из набора RSI
//! предмета. Образец — `UpdateHandVisuals` (`Content.Client/Hands/Systems/HandsSystem.cs`)
//! и `GetInhandVisuals` (`Content.Client/Items/Systems/ItemSystem.cs`) SS14 @ 2f4a2aad.

use astrocore_client::{Sprite, SpriteLayer};
use astrocore_ecs::{GameWorldExt, PostUpdate};
use astrocore_map::ContainerManager;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::query::{Changed, Or, With};
use bevy_ecs::world::World;

use crate::hands::{HandLocation, Hands, HandsWorldExt};
use crate::item::{Item, ItemLayer};

/// Ключи слоёв, которые руки добавили в спрайт: при следующем обновлении они убираются.
#[derive(Component, Debug, Default)]
struct RevealedHandLayers(Vec<String>);

fn location_name(location: HandLocation) -> &'static str {
    match location {
        HandLocation::Left => "left",
        HandLocation::Right => "right",
        HandLocation::Middle => "middle",
    }
}

fn apply(layer: &mut SpriteLayer, data: &ItemLayer) {
    if let Some(sprite) = &data.sprite {
        layer.sprite = Some(sprite.clone());
    }
    if let Some(state) = &data.state {
        layer.state = Some(state.clone());
    }
    if let Some(texture) = &data.texture {
        layer.texture = Some(texture.clone());
    }
    if let Some(color) = data.color {
        layer.color = color;
    }
    if let Some(visible) = data.visible {
        layer.visible = visible;
    }
    if let Some(scale) = data.scale {
        layer.scale = scale;
    }
    if let Some(offset) = data.offset {
        layer.offset = offset;
    }
    if let Some(rotation) = data.rotation {
        layer.rotation = rotation;
    }
    if let Some(shader) = &data.shader {
        layer.shader = Some(shader.clone());
    }
}

/// Слои предмета в руке: свои из `inhandVisuals` или состояние по умолчанию.
fn inhand_layers(
    world: &World,
    item: Entity,
    location: HandLocation,
) -> Vec<(String, SpriteLayer)> {
    let Some(component) = world.get::<Item>(item) else {
        return Vec::new();
    };
    let default_key = format!("inhand-{}", location_name(location));
    // Набор RSI: из предмета, иначе из спрайта предмета.
    let rsi = component.sprite.clone().or_else(|| {
        world
            .get::<Sprite>(item)
            .and_then(|sprite| sprite.sprite.clone())
    });
    if let Some(layers) = component.inhand_visuals.get(location_name(location)) {
        return layers
            .iter()
            .enumerate()
            .map(|(index, data)| {
                let key = data
                    .map
                    .as_ref()
                    .and_then(|keys| keys.first().cloned())
                    .unwrap_or_else(|| {
                        if index == 0 {
                            default_key.clone()
                        } else {
                            format!("{default_key}-{index}")
                        }
                    });
                let mut layer = SpriteLayer {
                    sprite: rsi.clone(),
                    map: vec![key.clone()],
                    ..SpriteLayer::default()
                };
                apply(&mut layer, data);
                (key, layer)
            })
            .collect();
    }
    let Some(rsi) = rsi else {
        return Vec::new();
    };
    let state = match &component.held_prefix {
        Some(prefix) => format!("{prefix}-{default_key}"),
        None => default_key,
    };
    // Состояния, которого нет в наборе, клиент не рисует: проверка не нужна.
    vec![(
        state.clone(),
        SpriteLayer {
            sprite: Some(rsi),
            state: Some(state.clone()),
            map: vec![state],
            ..SpriteLayer::default()
        },
    )]
}

fn update_holder(world: &mut World, holder: Entity) {
    let Some(hands) = world.get::<Hands>(holder).cloned() else {
        return;
    };
    let mut wanted: Vec<(String, SpriteLayer)> = Vec::new();
    if hands.show_in_hands {
        for (name, hand) in &hands.hands {
            if let Some(item) = world.held_item(holder, name) {
                wanted.extend(inhand_layers(world, item, hand.location));
            }
        }
    }
    let previous = world
        .get::<RevealedHandLayers>(holder)
        .map(|revealed| revealed.0.clone())
        .unwrap_or_default();
    let Some(sprite) = world.get::<Sprite>(holder) else {
        return;
    };
    let mut updated = sprite.clone();
    updated
        .layers
        .retain(|layer| !layer.map.iter().any(|key| previous.contains(key)));
    for (key, layer) in &wanted {
        // Слой с таким ключом уже есть в спрайте (место под руку): данные ложатся в него.
        match updated
            .layers
            .iter_mut()
            .find(|existing| existing.map.contains(key))
        {
            Some(existing) => *existing = layer.clone(),
            None => updated.layers.push(layer.clone()),
        }
    }
    if updated != *sprite
        && let Some(mut sprite) = world.get_mut::<Sprite>(holder)
    {
        *sprite = updated;
    }
    let keys = wanted.into_iter().map(|(key, _)| key).collect();
    world.entity_mut(holder).insert(RevealedHandLayers(keys));
}

fn update(world: &mut World) {
    let changed: Vec<Entity> = world
        .query_filtered::<Entity, (
            With<Hands>,
            With<Sprite>,
            Or<(Changed<ContainerManager>, Changed<Hands>)>,
        )>()
        .iter(world)
        .collect();
    for holder in changed {
        update_holder(world, holder);
    }
}

/// Подключает отображение предметов в руках к миру клиента.
pub fn install(world: &mut World) {
    world.add_game_systems(PostUpdate, update);
}

#[cfg(test)]
mod tests {
    use astrocore_ecs::LifecycleWorldExt;
    use astrocore_map::{ContainerKind, ContainerWorldExt, TransformWorldExt};

    use super::*;
    use crate::hands::Hand;

    #[test]
    fn held_items_add_inhand_layers_and_remove_them_when_dropped() {
        let mut world = crate::testing::world_with("- type: entity\n  id: Dummy\n");
        astrocore_client::register_client_components(&mut world);
        let holder = world.spawn_entity();
        world.entity_mut(holder).insert((
            Sprite {
                sprite: Some("Mobs/Species/Human/parts.rsi".into()),
                layers: vec![SpriteLayer {
                    state: Some("torso_m".into()),
                    ..SpriteLayer::default()
                }],
                ..Sprite::default()
            },
            Hands::default(),
        ));
        world.initialize_and_start(holder, false);
        world.add_hand(
            holder,
            "hand_left",
            Hand {
                location: HandLocation::Left,
                ..Hand::default()
            },
        );
        let item = world.spawn_entity();
        world.entity_mut(item).insert((
            Item {
                held_prefix: Some("on".into()),
                ..Item::default()
            },
            Sprite {
                sprite: Some("Objects/Tools/flashlight.rsi".into()),
                ..Sprite::default()
            },
        ));
        world.initialize_and_start(item, false);
        world.ensure_container(holder, "hand_left", ContainerKind::Slot);
        world
            .force_insert_into_container(item, holder, "hand_left")
            .unwrap();
        update_holder(&mut world, holder);
        let layers = &world.get::<Sprite>(holder).unwrap().layers;
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[1].state.as_deref(), Some("on-inhand-left"));
        assert_eq!(
            layers[1].sprite.as_deref(),
            Some("Objects/Tools/flashlight.rsi")
        );

        let _ = world.remove_from_container(item, None);
        update_holder(&mut world, holder);
        assert_eq!(world.get::<Sprite>(holder).unwrap().layers.len(), 1);
    }
}
