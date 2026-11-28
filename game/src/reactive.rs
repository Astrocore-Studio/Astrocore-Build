//! Реакция сущности на реагент (касание, впрыск, проглатывание): у реагента — эффекты по
//! группам (`reactiveEffects`: «горючее», «тушит», «кислота»…) с допустимыми способами, у
//! сущности — группы, на которые она отзывается, и свои реакции на конкретные реагенты.
//! Масштаб эффектов — количество реагента. Образец — `ReactiveSystem`, `ReactiveComponent`,
//! `ReactionEntityEvent`, `SharedEntityEffectsSystem.OnReactive`, `ReagentPrototype.
//! ReactiveEffects` SS14 @ 2f4a2aad.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use astrocore_data::DataNode;
use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::entity_effects::{EntityEffect, apply_effects, field, parse_effects};
use crate::fixed_point::FixedPoint2;
use crate::solution::Solution;

/// Способ попадания реагента (`ReactionMethod`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ReactionMethod {
    Touch,
    Injection,
    Ingestion,
}

/// Реакция сущности на реагенты (`ReactiveReagentEffectEntry`), разобранная.
#[derive(Debug, Clone)]
pub struct ReactiveEntry {
    pub methods: Vec<ReactionMethod>,
    /// `None` — на любой реагент.
    pub reagents: Option<Vec<String>>,
    pub effects: Vec<EntityEffect>,
}

/// Отзывается на реагенты (`ReactiveComponent`).
#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reactive {
    /// Группа реагентов → способы, которыми она действует.
    #[serde(default, rename = "groups")]
    pub reactive_groups: Option<BTreeMap<String, Vec<ReactionMethod>>>,
    /// Свои реакции; разбираются при первом обращении.
    #[serde(default)]
    pub reactions: Option<DataNode>,
    #[serde(skip)]
    pub parsed: Option<Arc<Vec<ReactiveEntry>>>,
}

impl PartialEq for Reactive {
    fn eq(&self, other: &Self) -> bool {
        self.reactive_groups == other.reactive_groups && self.reactions == other.reactions
    }
}

/// Реакция на реагент (`ReactionEntityEvent`). На сущности.
#[derive(Debug, Clone, PartialEq)]
pub struct ReactionEntity {
    pub method: ReactionMethod,
    pub reagent: String,
    pub quantity: FixedPoint2,
}

/// Эффекты реагента по группам (`ReactiveEffects`). Ресурс `World`.
#[derive(Resource, Debug, Default)]
pub struct ReactiveEffectsCache(HashMap<String, Arc<Vec<(String, ReactiveEntry)>>>);

fn parse_entry(world: &mut World, node: &DataNode) -> ReactiveEntry {
    ReactiveEntry {
        methods: field(node, "methods").unwrap_or_default(),
        reagents: field(node, "reagents"),
        effects: parse_effects(world, node.get("effects")),
    }
}

/// Эффекты реагента по группам из прототипа.
pub fn reagent_reactive_effects(
    world: &mut World,
    reagent: &str,
) -> Arc<Vec<(String, ReactiveEntry)>> {
    if let Some(found) = world
        .get_resource::<ReactiveEffectsCache>()
        .and_then(|c| c.0.get(reagent).cloned())
    {
        return found;
    }
    let node = world.get_resource::<Prototypes>().and_then(|p| {
        p.resolved_node("reagent", reagent)?
            .get("reactiveEffects")
            .cloned()
    });
    let mut list = Vec::new();
    if let Some(map) = node.as_ref().and_then(DataNode::as_map) {
        for (group, entry) in map.iter() {
            let parsed = parse_entry(world, entry);
            list.push((group.to_owned(), parsed));
        }
    }
    let list = Arc::new(list);
    world
        .get_resource_or_insert_with(ReactiveEffectsCache::default)
        .0
        .insert(reagent.to_owned(), list.clone());
    list
}

fn own_reactions(world: &mut World, entity: Entity) -> Arc<Vec<ReactiveEntry>> {
    let Some(component) = world.get::<Reactive>(entity) else {
        return Arc::default();
    };
    if let Some(parsed) = &component.parsed {
        return parsed.clone();
    }
    let node = component.reactions.clone();
    let list: Vec<ReactiveEntry> = node
        .as_ref()
        .and_then(DataNode::as_seq)
        .map(|seq| seq.iter().map(|n| parse_entry(world, n)).collect())
        .unwrap_or_default();
    let list = Arc::new(list);
    if let Some(mut component) = world.get_mut::<Reactive>(entity) {
        component.parsed = Some(list.clone());
    }
    list
}

/// Реакция сущности на раствор (`DoEntityReaction`): по каждому реагенту.
pub fn do_entity_reaction(
    world: &mut World,
    entity: Entity,
    solution: &Solution,
    method: ReactionMethod,
) {
    for (id, quantity) in solution.contents.clone() {
        reaction_entity(world, entity, method, &id.prototype, quantity);
    }
}

/// Реакция на один реагент (`ReactionEntity`): неизвестный реагент пропускается.
pub fn reaction_entity(
    world: &mut World,
    entity: Entity,
    method: ReactionMethod,
    reagent: &str,
    quantity: FixedPoint2,
) {
    if quantity == FixedPoint2::ZERO || crate::solution::reagents(world).get(reagent).is_none() {
        return;
    }
    world.raise(
        entity,
        &mut ReactionEntity {
            method,
            reagent: reagent.to_owned(),
            quantity,
        },
    );
}

/// Эффекты групп и свои реакции (`OnReactive`).
fn on_reactive(world: &mut World, entity: Entity, event: &mut ReactionEntity) {
    let scale = event.quantity.float();
    let groups = world
        .get::<Reactive>(entity)
        .and_then(|r| r.reactive_groups.clone());
    if let Some(groups) = groups {
        let effects = reagent_reactive_effects(world, &event.reagent);
        for (group, entry) in effects.iter() {
            if !entry.methods.contains(&event.method) {
                continue;
            }
            if !groups
                .get(group)
                .is_some_and(|methods| methods.contains(&event.method))
            {
                continue;
            }
            apply_effects(world, entity, &entry.effects, scale, None);
        }
    }
    for entry in own_reactions(world, entity).iter() {
        if !entry.methods.contains(&event.method) {
            continue;
        }
        if entry
            .reagents
            .as_ref()
            .is_none_or(|list| list.contains(&event.reagent))
        {
            apply_effects(world, entity, &entry.effects, scale, None);
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Reactive>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Reactive, ReactionEntity>(on_reactive);
}

#[cfg(test)]
mod tests;
