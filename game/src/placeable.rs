//! Поверхности, на которые кладут предметы: щелчок предметом по столу кладёт его в точку
//! щелчка (или по центру с отступом — плитки), открытый шкаф с поверхностью — тоже
//! поверхность. Держатель предметов (`ItemPlacer`) помнит, что на нём лежит, по касанию
//! фикстур, до предела — тогда поверхность закрывается. Образец — `PlaceableSurfaceSystem`,
//! `PlaceableSurfaceComponent`, `ItemPlacerSystem`, `ItemPlacerComponent`,
//! `ItemPlacedEvent`, `ItemRemovedEvent` SS14 @ 2f4a2aad.
//!
//! Нет пока: высыпания содержимого на поверхность (`Dumpable`), «сна» столкновений лежащих
//! предметов (`CollisionWake`).

use std::collections::BTreeSet;

use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_map::{EntityCoordinates, TransformWorldExt};
use astrocore_math::Vec2;
use astrocore_net::NetRegistryWorldExt;
use astrocore_physics::{EndCollide, StartCollide};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::entity_storage::{StorageAfterClose, StorageAfterOpen};
use crate::hands::HandsWorldExt;
use crate::interaction::AfterInteractUsing;
use crate::whitelist::EntityWhitelist;

fn yes() -> bool {
    true
}

/// Сюда кладут предметы (`PlaceableSurfaceComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceableSurface {
    #[serde(default = "yes")]
    pub is_placeable: bool,
    #[serde(default)]
    pub place_centered: bool,
    #[serde(default)]
    pub position_offset: Vec2,
}

fn one() -> u32 {
    1
}

/// Держит положенные предметы (`ItemPlacerComponent`); `max_entities` 0 — без предела.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemPlacer {
    #[serde(skip)]
    pub placed_entities: BTreeSet<Entity>,
    #[serde(default)]
    pub whitelist: Option<EntityWhitelist>,
    #[serde(default = "one")]
    pub max_entities: u32,
}

/// Предмет положен (`ItemPlacedEvent`). На держателе.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemPlaced {
    pub other_entity: Entity,
}

/// Предмет убран (`ItemRemovedEvent`). На держателе.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemRemoved {
    pub other_entity: Entity,
}

/// Открыть или закрыть поверхность (`SetPlaceable`).
pub fn set_placeable(world: &mut World, surface: Entity, placeable: bool) {
    if let Some(mut component) = world.get_mut::<PlaceableSurface>(surface)
        && component.is_placeable != placeable
    {
        component.is_placeable = placeable;
    }
}

/// Положить предмет щелчком (`OnAfterInteractUsing`).
fn on_after_interact_using(world: &mut World, surface: Entity, event: &mut AfterInteractUsing) {
    if event.handled || !event.can_reach {
        return;
    }
    let Some(component) = world.get::<PlaceableSurface>(surface).cloned() else {
        return;
    };
    if !component.is_placeable {
        return;
    }
    let Some(hand) = world.hand_holding(event.user, event.used) else {
        return;
    };
    if !world.try_drop(event.user, &hand, None, true) {
        return;
    }
    let used = event.used;
    if component.place_centered {
        if let Some(at) = world.coordinates(surface) {
            let target = EntityCoordinates {
                position: at.position + component.position_offset,
                ..at
            };
            let _ = world.set_coordinates(used, target, None);
        }
    } else if world
        .map_coordinates(used)
        .is_some_and(|m| m.map == event.click.map)
    {
        let _ = world.set_world_position(used, event.click.position);
    }
    event.handled = true;
}

fn on_start_collide(world: &mut World, placer: Entity, event: &mut StartCollide) {
    let other = event.other_entity;
    let Some(component) = world.get::<ItemPlacer>(placer).cloned() else {
        return;
    };
    if component
        .whitelist
        .as_ref()
        .is_some_and(|w| !w.is_valid(world, other))
    {
        return;
    }
    let count = component.placed_entities.len() as u32;
    if component.max_entities == 0 || count < component.max_entities {
        if let Some(mut placer_component) = world.get_mut::<ItemPlacer>(placer) {
            placer_component.placed_entities.insert(other);
        }
        world.raise(
            placer,
            &mut ItemPlaced {
                other_entity: other,
            },
        );
    }
    if component.max_entities > 0 && count + 1 >= component.max_entities {
        set_placeable(world, placer, false);
    }
}

fn on_end_collide(world: &mut World, placer: Entity, event: &mut EndCollide) {
    let other = event.other_entity;
    if let Some(mut component) = world.get_mut::<ItemPlacer>(placer) {
        component.placed_entities.remove(&other);
    }
    world.raise(
        placer,
        &mut ItemRemoved {
            other_entity: other,
        },
    );
    set_placeable(world, placer, true);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<PlaceableSurface>();
    world.register_net_serde::<PlaceableSurface>();
    world.register_data_component::<ItemPlacer>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<PlaceableSurface, AfterInteractUsing>(on_after_interact_using);
    world.subscribe::<PlaceableSurface, StorageAfterOpen>(|world, surface, _| {
        set_placeable(world, surface, true);
    });
    world.subscribe::<PlaceableSurface, StorageAfterClose>(|world, surface, _| {
        set_placeable(world, surface, false);
    });
    world.subscribe::<ItemPlacer, StartCollide>(on_start_collide);
    world.subscribe::<ItemPlacer, EndCollide>(on_end_collide);
}

#[cfg(test)]
mod tests;
