//! Надетая одежда на спрайте носящего: слои `equipped-<СЛОТ>` из набора RSI вещи.
//! Образец — `ClientClothingSystem` (`RenderEquipment`, `TryGetDefaultVisuals`) клиента
//! SS14 @ 2f4a2aad.

use std::collections::HashMap;

use astrocore_client::{Sprite, SpriteLayer};
use astrocore_ecs::{GameWorldExt, PostUpdate};
use astrocore_map::ContainerManager;
use astrocore_resources::Resources;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::query::{Changed, Or, With};
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::inventory::{Clothing, Inventory, InventoryWorldExt};
use crate::item::ItemLayer;

/// Имена состояний по слотам (`TemporarySlotMap`).
fn slot_state_name(slot: &str) -> String {
    match slot {
        "head" => "HELMET",
        "eyes" => "EYES",
        "ears" => "EARS",
        "mask" => "MASK",
        "outerClothing" => "OUTERCLOTHING",
        "jumpsuit" => "INNERCLOTHING",
        "neck" => "NECK",
        "back" => "BACKPACK",
        "belt" => "BELT",
        "gloves" => "HAND",
        "shoes" => "FEET",
        "id" => "IDCARD",
        "pocket1" => "POCKET1",
        "pocket2" => "POCKET2",
        "suitstorage" => "SUITSTORAGE",
        other => return other.to_owned(),
    }
    .to_owned()
}

/// Состояния наборов RSI: читаются один раз на набор.
#[derive(Resource, Default)]
struct RsiStates(HashMap<String, Vec<String>>);

fn has_state(world: &mut World, rsi: &str, state: &str) -> bool {
    let path = rsi.trim_start_matches('/');
    let path = if path.starts_with("Textures/") {
        path.to_owned()
    } else {
        format!("Textures/{path}")
    };
    if !world.contains_resource::<RsiStates>() {
        world.init_resource::<RsiStates>();
    }
    if !world.resource::<RsiStates>().0.contains_key(&path) {
        let states = world
            .get_resource::<Resources>()
            .and_then(|resources| resources.rsi_meta(&path).ok())
            .map(|meta| meta.states.into_iter().map(|state| state.name).collect())
            .unwrap_or_default();
        world
            .resource_mut::<RsiStates>()
            .0
            .insert(path.clone(), states);
    }
    world.resource::<RsiStates>().0[&path]
        .iter()
        .any(|known| known == state)
}

/// Ключи слоёв одежды по слотам: при обновлении слоя слота они убираются.
#[derive(Component, Debug, Default)]
struct RevealedClothingLayers(HashMap<String, Vec<String>>);

fn layer_from(data: &ItemLayer, rsi: Option<&String>, key: &str) -> SpriteLayer {
    let mut layer = SpriteLayer {
        sprite: rsi.cloned(),
        map: vec![key.to_owned()],
        ..SpriteLayer::default()
    };
    if let Some(sprite) = &data.sprite {
        layer.sprite = Some(sprite.clone());
    }
    if let Some(state) = &data.state {
        layer.state = Some(state.clone());
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
    layer
}

/// Слои вещи в слоте (`GetEquipmentVisuals`).
fn equipment_layers(
    world: &mut World,
    item: Entity,
    slot: &str,
    species: Option<&str>,
) -> Vec<(String, SpriteLayer)> {
    let Some(clothing) = world.get::<Clothing>(item).cloned() else {
        return Vec::new();
    };
    let rsi = clothing.sprite.clone().or_else(|| {
        world
            .get::<Sprite>(item)
            .and_then(|sprite| sprite.sprite.clone())
    });
    if let Some(layers) = clothing.clothing_visuals.get(slot) {
        return layers
            .iter()
            .enumerate()
            .map(|(index, data)| {
                let key = data
                    .map
                    .as_ref()
                    .and_then(|keys| keys.first().cloned())
                    .unwrap_or_else(|| format!("{slot}-{index}"));
                let layer = layer_from(data, rsi.as_ref(), &key);
                (key, layer)
            })
            .collect();
    }
    let Some(rsi) = rsi else {
        return Vec::new();
    };
    let slot_name = slot_state_name(slot);
    let mut state = match &clothing.equipped_prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}-equipped-{slot_name}"),
        _ => format!("equipped-{slot_name}"),
    };
    if let Some(explicit) = &clothing.equipped_state {
        state = explicit.clone();
    }
    if let Some(species) = species
        && has_state(world, &rsi, &format!("{state}-{species}"))
    {
        state = format!("{state}-{species}");
    } else if !has_state(world, &rsi, &state) {
        return Vec::new();
    }
    let key = format!("{slot}-equipped");
    vec![(
        key.clone(),
        SpriteLayer {
            sprite: Some(rsi),
            state: Some(state),
            map: vec![key],
            ..SpriteLayer::default()
        },
    )]
}

fn update_wearer(world: &mut World, wearer: Entity) {
    let Some(inventory) = world.get::<Inventory>(wearer).cloned() else {
        return;
    };
    let species = inventory.species_id.clone();
    let mut wanted: Vec<(String, Vec<(String, SpriteLayer)>)> = Vec::new();
    for slot in &inventory.slots {
        let layers = match world.slot_entity(wearer, &slot.name) {
            Some(item) => equipment_layers(world, item, &slot.name, species.as_deref()),
            None => Vec::new(),
        };
        wanted.push((slot.name.clone(), layers));
    }
    let previous = world
        .get::<RevealedClothingLayers>(wearer)
        .map(|revealed| revealed.0.clone())
        .unwrap_or_default();
    let Some(sprite) = world.get::<Sprite>(wearer) else {
        return;
    };
    let mut updated = sprite.clone();
    let old_keys: Vec<&String> = previous.values().flatten().collect();
    updated
        .layers
        .retain(|layer| !layer.map.iter().any(|key| old_keys.contains(&key)));
    let mut revealed = HashMap::new();
    for (slot, layers) in wanted {
        // Слой с ключом слота — закладка: одежда ложится сразу после него.
        let mut index = updated
            .layers
            .iter()
            .position(|layer| layer.map.contains(&slot));
        let mut keys = Vec::new();
        for (key, layer) in layers {
            keys.push(key);
            match index {
                Some(at) => {
                    updated.layers.insert(at + 1, layer);
                    index = Some(at + 1);
                }
                None => updated.layers.push(layer),
            }
        }
        revealed.insert(slot, keys);
    }
    if updated != *sprite
        && let Some(mut sprite) = world.get_mut::<Sprite>(wearer)
    {
        *sprite = updated;
    }
    world
        .entity_mut(wearer)
        .insert(RevealedClothingLayers(revealed));
}

fn update(world: &mut World) {
    let changed: Vec<Entity> = world
        .query_filtered::<Entity, (
            With<Inventory>,
            With<Sprite>,
            Or<(Changed<ContainerManager>, Changed<Inventory>)>,
        )>()
        .iter(world)
        .collect();
    for wearer in changed {
        update_wearer(world, wearer);
    }
}

/// Подключает отображение одежды к миру клиента.
pub fn install(world: &mut World) {
    world.add_game_systems(PostUpdate, update);
}
