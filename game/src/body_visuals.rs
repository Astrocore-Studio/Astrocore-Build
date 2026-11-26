//! Вид тела на клиенте: каждый видимый орган задаёт слой спрайта тела — набор, состояние,
//! цвет кожи. Образец — `VisualBodySystem` клиента SS14 @ 2f4a2aad (`ApplyVisual`).

use astrocore_client::Sprite;
use astrocore_ecs::{EventWorldExt, GameWorldExt, Update};
use astrocore_map::{ContainerWorldExt, EntGotInsertedIntoContainer};
use bevy_ecs::entity::Entity;
use bevy_ecs::query::Changed;
use bevy_ecs::world::World;

use crate::body::{BODY_ORGANS, VisualOrgan};

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
    // Поле `data` в SS14 наследуется слиянием с родителем (`AlwaysPushInheritance`);
    // загрузчик прототипов его заменяет, поэтому недостающие набор и состояние берутся из
    // спрайта самого органа, где они те же.
    let own = world
        .get::<Sprite>(organ)
        .map(|s| (s.sprite.clone(), s.state.clone()));
    let rsi = visual
        .data
        .sprite
        .clone()
        .or_else(|| own.as_ref().and_then(|o| o.0.clone()));
    let state = visual
        .data
        .state
        .clone()
        .or_else(|| own.as_ref().and_then(|o| o.1.clone()));
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

pub(crate) fn install(world: &mut World) {
    world.subscribe::<VisualOrgan, EntGotInsertedIntoContainer>(|world, organ, _| {
        apply(world, organ)
    });
    world.add_game_systems(Update, apply_changed);
}
