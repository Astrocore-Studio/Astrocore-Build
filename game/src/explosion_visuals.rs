//! Вид взрыва на клиенте: огонь на клетках прошедших волн, рисунок по силе волны.
//! Образец — `ExplosionOverlay` и `ExplosionOverlaySystem` клиента SS14 @ 2f4a2aad. Свет
//! взрыва и случайные кадры огня не переносятся: кадры идут анимацией рисунка.

use std::collections::HashMap;

use astrocore_client::{DrawDepth, Sprite, SpriteLayer, draw_depth_named};
use astrocore_ecs::{GameWorldExt, LifecycleWorldExt, Update};
use astrocore_map::{EntityCoordinates, TileCoord, Transform, TransformWorldExt};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::explosion::ExplosionVisuals;

/// Местные сущности огня по сущностям вида: показано волн и сами сущности.
#[derive(Resource, Default)]
struct Fires(HashMap<Entity, (usize, Vec<Entity>)>);

/// Рисунок огня волны (`frameIndex`): номер состояния по силе, не больше числа состояний.
pub fn fire_state(intensity: f32, per_state: f32, states: u32) -> String {
    let index = ((intensity / per_state.max(0.001)) as u32).min(states.saturating_sub(1));
    (index + 1).to_string()
}

fn spawn_fire(
    world: &mut World,
    root: Entity,
    cell: TileCoord,
    visuals: &ExplosionVisuals,
    state: String,
) -> Entity {
    let entity = world.spawn_uninitialized();
    world.entity_mut(entity).insert(Transform::default());
    let _ = world.set_coordinates(
        entity,
        EntityCoordinates::on_level(root, cell.center(), cell.z),
        None,
    );
    let sprite = Sprite {
        layers: vec![SpriteLayer {
            sprite: Some(visuals.texture.clone()),
            state: Some(state),
            color: visuals.fire_color.unwrap_or_default(),
            shader: Some("unshaded".into()),
            ..SpriteLayer::default()
        }],
        drawdepth: draw_depth_named("Overlays").unwrap_or(DrawDepth(30)),
        no_rot: true,
        ..Sprite::default()
    };
    world.entity_mut(entity).insert(sprite);
    world.initialize_and_start(entity, false);
    entity
}

fn update(world: &mut World) {
    world.init_resource::<Fires>();
    let visuals: Vec<(Entity, ExplosionVisuals)> = world
        .query::<(Entity, &ExplosionVisuals)>()
        .iter(world)
        .map(|(e, v)| (e, v.clone()))
        .collect();
    let mut fires = std::mem::take(&mut world.resource_mut::<Fires>().0);
    // Вид исчез — огонь тоже.
    let gone: Vec<Entity> = fires
        .keys()
        .filter(|e| !visuals.iter().any(|(v, _)| v == *e))
        .copied()
        .collect();
    for visual in gone {
        if let Some((_, locals)) = fires.remove(&visual) {
            for local in locals {
                if world.get_entity(local).is_ok() {
                    world.delete_entity(local);
                }
            }
        }
    }
    for (visual, data) in visuals {
        let Some(root) = world.parent(visual) else {
            continue;
        };
        let entry = fires.entry(visual).or_insert((0, Vec::new()));
        let wanted = (data.progress as usize).min(data.tiles.len());
        for iteration in entry.0..wanted {
            let intensity = data.intensity.get(iteration).copied().unwrap_or(0.0);
            let state = fire_state(intensity, data.intensity_per_state, data.fire_states);
            for (x, y) in &data.tiles[iteration] {
                let cell = TileCoord::new(*x, *y, data.level);
                let fire = spawn_fire(world, root, cell, &data, state.clone());
                entry.1.push(fire);
            }
        }
        entry.0 = entry.0.max(wanted);
    }
    world.resource_mut::<Fires>().0 = fires;
}

pub(crate) fn install(world: &mut World) {
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests {
    use super::fire_state;

    #[test]
    fn fire_state_grows_with_intensity_and_stops_at_the_last() {
        assert_eq!(fire_state(5.0, 12.0, 3), "1");
        assert_eq!(fire_state(13.0, 12.0, 3), "2");
        assert_eq!(fire_state(500.0, 12.0, 3), "3");
    }
}
