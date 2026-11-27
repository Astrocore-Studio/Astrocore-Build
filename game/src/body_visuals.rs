//! Вид тела на клиенте: каждый видимый орган задаёт слой спрайта тела — набор, состояние,
//! цвет кожи; метки органа (волосы, узоры, бельё) встают слоями сразу за слоем своей части
//! тела со своими цветами. Образец — `VisualBodySystem` клиента SS14 @ 2f4a2aad
//! (`ApplyVisual`, `ApplyMarkings`, `ApplyMarkingLayer`, `RemoveMarkings`).
//!
//! Нет пока: белья по умолчанию при цензуре наготы (`NudityDefault`), смещения слоёв.

use astrocore_client::Sprite;
use astrocore_ecs::{EventWorldExt, GameWorldExt, Update};
use astrocore_map::{ContainerWorldExt, EntGotInsertedIntoContainer};
use bevy_ecs::entity::Entity;
use bevy_ecs::query::Changed;
use bevy_ecs::world::World;

use astrocore_client::SpriteLayer;
use astrocore_prototypes::Prototypes;

use crate::body::{BODY_ORGANS, VisualOrgan};
use crate::markings::{VisualOrganMarkings, layer_key, marking_proto};

/// Слой органа на теле (`ApplyVisual`).
fn apply(world: &mut World, organ: Entity) {
    let Some(container) = world.containing_container(organ).cloned() else {
        return;
    };
    if &*container.id != BODY_ORGANS {
        return;
    }
    let Some(visual) = world.get::<VisualOrgan>(organ).cloned() else {
        return;
    };
    let rsi = visual.data.sprite.clone();
    let state = visual.data.state.clone();
    let Some(sprite) = world.get::<Sprite>(container.owner) else {
        return;
    };
    let Some(index) = sprite.layer_index(&visual.layer) else {
        return;
    };
    let mut updated = sprite.clone();
    let layer = &mut updated.layers[index];
    if let Some(rsi) = rsi {
        layer.sprite = Some(rsi);
    }
    if let Some(state) = state {
        layer.state = Some(state);
    }
    if let Some(color) = visual.data.color {
        layer.color = color;
    }
    if let Some(visible) = visual.data.visible {
        layer.visible = visible;
    }
    if updated != *world.get::<Sprite>(container.owner).expect("спрайт тела")
        && let Some(mut sprite) = world.get_mut::<Sprite>(container.owner)
    {
        *sprite = updated;
    }
}

/// Органы, чьё состояние пришло с сервера заново.
fn apply_changed(world: &mut World) {
    let changed: Vec<Entity> = world
        .query_filtered::<Entity, Changed<VisualOrgan>>()
        .iter(world)
        .collect();
    for organ in changed {
        apply(world, organ);
    }
}

/// Метка слоя спрайта: принадлежит меткам органа.
fn marking_key(organ: Entity) -> String {
    format!("marking:{}", organ.to_bits())
}

/// Метки органа на спрайте тела (`RemoveMarkings` + `ApplyMarkings`).
fn apply_markings(world: &mut World, organ: Entity) {
    let Some(container) = world.containing_container(organ).cloned() else {
        return;
    };
    if &*container.id != BODY_ORGANS {
        return;
    }
    let Some(component) = world.get::<VisualOrganMarkings>(organ).cloned() else {
        return;
    };
    let Some(mut updated) = world.get::<Sprite>(container.owner).cloned() else {
        return;
    };
    let key = marking_key(organ);
    updated.layers.retain(|layer| !layer.map.contains(&key));
    if let Some(prototypes) = world.get_resource::<Prototypes>() {
        for markings in component.markings.values() {
            for marking in markings {
                let Some(proto) = marking_proto(prototypes, &marking.marking_id) else {
                    continue;
                };
                let Some(base) = updated.layer_index(&layer_key(&proto.body_part)) else {
                    continue;
                };
                let visible = updated.layers[base].visible;
                for (i, data) in proto.sprites.iter().enumerate() {
                    let color = marking
                        .marking_colors
                        .get(i)
                        .copied()
                        .unwrap_or(astrocore_math::Color::WHITE);
                    let layer = SpriteLayer {
                        sprite: Some(data.sprite.clone()),
                        state: Some(data.state.clone()),
                        color,
                        visible,
                        shader: data.shader.clone(),
                        map: vec![format!("{}-{}", proto.id, data.state), key.clone()],
                        ..SpriteLayer::default()
                    };
                    // Слои метки — за слоем части тела, по порядку.
                    let mut at = base + 1;
                    while at < updated.layers.len() && updated.layers[at].map.contains(&key) {
                        at += 1;
                    }
                    updated.layers.insert(at.min(updated.layers.len()), layer);
                }
            }
        }
    }
    if world.get::<Sprite>(container.owner) != Some(&updated)
        && let Some(mut sprite) = world.get_mut::<Sprite>(container.owner)
    {
        *sprite = updated;
    }
}

/// Органы с метками, чьё состояние пришло с сервера заново.
fn apply_changed_markings(world: &mut World) {
    let changed: Vec<Entity> = world
        .query_filtered::<Entity, Changed<VisualOrganMarkings>>()
        .iter(world)
        .collect();
    for organ in changed {
        apply_markings(world, organ);
    }
}

pub(crate) fn install(world: &mut World) {
    world.subscribe::<VisualOrgan, EntGotInsertedIntoContainer>(|world, organ, _| {
        apply(world, organ)
    });
    world.subscribe::<VisualOrganMarkings, EntGotInsertedIntoContainer>(|world, organ, _| {
        apply_markings(world, organ)
    });
    world.add_game_systems(Update, (apply_changed, apply_changed_markings));
}
