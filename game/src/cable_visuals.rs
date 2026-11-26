//! Клиент: вид кабелей по маске соседей и скрытие под полом. Образец —
//! `CableVisualizerSystem` и `SubFloorHideSystem` клиента SS14 @ 2f4a2aad.

use astrocore_client::Sprite;
use astrocore_ecs::{EventWorldExt, GameWorldExt, Update};
use astrocore_map::{TileCoord, Transform};
use astrocore_net::{AppearanceChange, AppearanceValue, AppearanceWorldExt};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::cables::{CONNECTED_MASK, CableVisualizer, SubFloorHide, is_subfloor};

fn apply_mask(world: &mut World, entity: Entity) {
    let Some(visualizer) = world.get::<CableVisualizer>(entity).cloned() else {
        return;
    };
    let mask = match world.appearance(entity, CONNECTED_MASK) {
        Some(AppearanceValue::Int(mask)) => *mask,
        _ => 0,
    };
    let Some(mut sprite) = world.get::<Sprite>(entity).cloned() else {
        return;
    };
    let before = sprite.clone();
    for (index, prefix) in [visualizer.state_prefix, visualizer.extra_layer_prefix]
        .into_iter()
        .enumerate()
    {
        if let (Some(prefix), Some(layer)) = (prefix, sprite.layers.get_mut(index)) {
            layer.state = Some(format!("{prefix}{mask}"));
        }
    }
    if sprite != before
        && let Some(mut current) = world.get_mut::<Sprite>(entity)
    {
        *current = sprite;
    }
}

/// Видимость скрываемых под полом сущностей: видны на подполе и не привязанными.
fn update_subfloor(world: &mut World, mut frame: bevy_ecs::system::Local<u32>) {
    *frame += 1;
    if *frame % 10 != 1 {
        return;
    }
    let entities: Vec<Entity> = world
        .query::<(Entity, &SubFloorHide)>()
        .iter(world)
        .map(|(e, _)| e)
        .collect();
    for entity in entities {
        let Some(transform) = world.get::<Transform>(entity) else {
            continue;
        };
        let visible = match (transform.grid(), transform.anchored()) {
            (Some(grid), true) => is_subfloor(
                world,
                grid,
                TileCoord::containing(transform.local_position(), transform.level()),
            ),
            _ => true,
        };
        if world
            .get::<Sprite>(entity)
            .is_some_and(|s| s.visible != visible)
            && let Some(mut sprite) = world.get_mut::<Sprite>(entity)
        {
            sprite.visible = visible;
        }
    }
}

pub(crate) fn install(world: &mut World) {
    world.subscribe::<CableVisualizer, AppearanceChange>(|world, entity, _| {
        apply_mask(world, entity)
    });
    world.add_game_systems(Update, update_subfloor);
}
