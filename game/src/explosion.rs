//! Взрывы (ADR-0008). Заготовка: взрывчатка.

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::Side;

/// Взорвать взрывчатку сущности (`TriggerExplosive`).
pub fn trigger_explosive(_world: &mut World, _entity: Entity, _user: Option<Entity>) {}

pub fn install(_world: &mut World, _side: Side) {}
