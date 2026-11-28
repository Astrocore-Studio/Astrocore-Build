//! Химические реакции: при каждом изменении раствора (`update_chemicals`) реакции, первым
//! реагентом которых служит что-то из раствора, проверяются по порядку (приоритет выше —
//! раньше; при равном — меньше продуктов, затем по имени): температура в пределах, нужные
//! виды смешивания, все реагенты есть (катализаторы не тратятся), число «единиц реакции» —
//! меньшее частное «есть / нужно» (целое у квантованных). Первая подходящая реакция тратит
//! реагенты, добавляет продукты, сохраняет тепло (если `conserveEnergy`), применяет эффекты к
//! раствору, звучит и пишется в журнал; продукты добавляют в очередь свои реакции. Не больше
//! 20 проходов. Здесь же — эффекты температуры раствора. Образец — `ChemicalReactionSystem`,
//! `ReactionPrototype`, `ReactantInfo`, `ReactionMixerComponent`, `ReactionAttemptEvent`,
//! `SolutionTemperatureEntityEffectsSystem` SS14 @ 2f4a2aad.
//!
//! Нет пока: расширений реакций (`DnaImprinterReaction` — ДНК в продуктах).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_data::DataNode;
use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_loc::EntityNameWorldExt;
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType};
use crate::entity_effects::{
    EffectKind, EffectsWorldExt, EntityEffect, apply_effects, field, number, parse_effects,
};
use crate::fixed_point::FixedPoint2;
use crate::solution::ReagentId;
use crate::solution_container::{self as containers, ReactionHook, SolutionComponent};

/// Наибольшее число проходов (`MaxReactionIterations`).
pub const MAX_REACTION_ITERATIONS: usize = 20;

/// Реагент реакции (`ReactantInfo`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reactant {
    pub amount: FixedPoint2,
    pub catalyst: bool,
}

/// Реакция (`ReactionPrototype`).
#[derive(Debug, Clone)]
pub struct Reaction {
    pub id: String,
    pub name: String,
    /// Реагенты по порядку данных: первый — ключ быстрого поиска.
    pub reactants: Vec<(String, Reactant)>,
    pub min_temperature: f32,
    pub max_temperature: f32,
    pub conserve_energy: bool,
    pub mixing_categories: Option<Vec<String>>,
    pub products: Vec<(String, FixedPoint2)>,
    pub effects: Vec<EntityEffect>,
    /// Виды всех эффектов из данных, в том числе ещё не перенесённых.
    pub effect_kinds: Vec<String>,
    pub impact: LogImpact,
    pub sound: Option<SoundSpecifier>,
    pub quantized: bool,
    pub priority: i32,
}

/// Место реакции в очереди: приоритет выше, меньше продуктов, по имени.
type OrderKey = (std::cmp::Reverse<i32>, usize, String);

impl Reaction {
    /// Порядок `CompareTo`: приоритет выше, меньше продуктов, по имени.
    fn order_key(&self) -> OrderKey {
        (
            std::cmp::Reverse(self.priority),
            self.products.len(),
            self.id.clone(),
        )
    }
}

/// Реакции из прототипов и указатели по реагентам. Ресурс `World`.
#[derive(Debug, Default)]
pub struct ReactionCatalog {
    pub reactions: Vec<Reaction>,
    /// По первому реагенту (`_reactionsSingle`).
    by_first: HashMap<String, Vec<usize>>,
    /// По любому реагенту (`_reactions`).
    by_any: HashMap<String, Vec<usize>>,
}

impl ReactionCatalog {
    pub fn get(&self, id: &str) -> Option<&Reaction> {
        self.reactions.iter().find(|r| r.id == id)
    }
}

#[derive(Resource)]
struct SharedReactions(Arc<ReactionCatalog>);

fn parse_reaction(world: &mut World, id: &str, node: &DataNode) -> Option<Reaction> {
    let reactants: Vec<(String, Reactant)> = node
        .get("reactants")?
        .as_map()?
        .iter()
        .filter_map(|(name, info)| {
            Some((
                name.to_string(),
                Reactant {
                    amount: field(info, "amount")?,
                    catalyst: crate::entity_effects::flag(info, "catalyst").unwrap_or(false),
                },
            ))
        })
        .collect();
    if reactants.is_empty() {
        return None;
    }
    let products: Vec<(String, FixedPoint2)> = node
        .get("products")
        .and_then(DataNode::as_map)
        .map(|map| {
            map.iter()
                .filter_map(|(name, amount)| {
                    Some((name.to_string(), astrocore_data::from_node(amount).ok()?))
                })
                .collect()
        })
        .unwrap_or_default();
    let impact = match crate::entity_effects::text(node, "impact").as_deref() {
        Some("Medium") => LogImpact::Medium,
        Some("High") => LogImpact::High,
        Some("Extreme") => LogImpact::Extreme,
        _ => LogImpact::Low,
    };
    let sound = match node.get("sound") {
        Some(sound) => astrocore_data::from_node(sound).ok(),
        None => Some(SoundSpecifier::path("/Audio/Effects/Chemistry/bubbles.ogg")),
    };
    let effects = parse_effects(world, node.get("effects"));
    let effect_kinds = node
        .get("effects")
        .and_then(DataNode::as_seq)
        .map(|list| {
            list.iter()
                .map(|n| crate::entity_effects::tag(n).to_owned())
                .collect()
        })
        .unwrap_or_default();
    Some(Reaction {
        id: id.to_owned(),
        name: crate::entity_effects::text(node, "name").unwrap_or_default(),
        reactants,
        min_temperature: number(node, "minTemp").unwrap_or(0.0),
        max_temperature: number(node, "maxTemp").unwrap_or(f32::INFINITY),
        conserve_energy: crate::entity_effects::flag(node, "conserveEnergy").unwrap_or(true),
        mixing_categories: field(node, "requiredMixerCategories"),
        products,
        effects,
        effect_kinds,
        impact,
        sound,
        quantized: crate::entity_effects::flag(node, "quantized").unwrap_or(false),
        priority: number(node, "priority").map_or(0, |p| p as i32),
    })
}

/// Каталог реакций мира; собирается при первом обращении (`InitializeReactionCache`).
pub fn reactions(world: &mut World) -> Arc<ReactionCatalog> {
    if let Some(shared) = world.get_resource::<SharedReactions>() {
        return shared.0.clone();
    }
    let nodes: Vec<(String, DataNode)> = world
        .get_resource::<Prototypes>()
        .map(|p| {
            p.ids("reaction")
                .filter_map(|id| Some((id.to_owned(), p.resolved_node("reaction", id)?.clone())))
                .collect()
        })
        .unwrap_or_default();
    let mut catalog = ReactionCatalog::default();
    for (id, node) in nodes {
        if let Some(reaction) = parse_reaction(world, &id, &node) {
            catalog.reactions.push(reaction);
        }
    }
    catalog.reactions.sort_by(|a, b| a.id.cmp(&b.id));
    for (index, reaction) in catalog.reactions.iter().enumerate() {
        let first = reaction.reactants[0].0.clone();
        catalog.by_first.entry(first).or_default().push(index);
        for (reagent, _) in &reaction.reactants {
            catalog
                .by_any
                .entry(reagent.clone())
                .or_default()
                .push(index);
        }
    }
    let catalog = Arc::new(catalog);
    world.insert_resource(SharedReactions(catalog.clone()));
    catalog
}

fn yes() -> bool {
    true
}

/// Смешивает раствор особыми способами (`ReactionMixerComponent`): мешалка, центрифуга.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionMixer {
    #[serde(default)]
    pub reaction_types: Vec<String>,
    #[serde(default)]
    pub mix_message: Option<String>,
    /// С.
    #[serde(default)]
    pub time_to_mix: f32,
    #[serde(default = "yes")]
    pub mix_on_interact: bool,
}

/// Можно ли реакции идти (`ReactionAttemptEvent`). На сущности раствора.
#[derive(Debug, Clone)]
pub struct ReactionAttempt {
    pub reaction: String,
    pub solution: Entity,
    pub cancelled: bool,
}

/// Может ли реакция идти и сколько единиц (`CanReact`).
fn can_react(
    world: &mut World,
    soln: Entity,
    reaction: &Reaction,
    mixer: Option<&[String]>,
) -> Option<FixedPoint2> {
    let solution = containers::solution(world, soln)?;
    if solution.temperature < reaction.min_temperature
        || solution.temperature > reaction.max_temperature
    {
        return None;
    }
    if let Some(required) = &reaction.mixing_categories {
        let types = mixer?;
        if required.iter().any(|r| !types.contains(r)) {
            return None;
        }
    }
    let quantities: Vec<FixedPoint2> = reaction
        .reactants
        .iter()
        .map(|(name, _)| solution.prototype_quantity(name))
        .collect();
    let mut attempt = ReactionAttempt {
        reaction: reaction.id.clone(),
        solution: soln,
        cancelled: false,
    };
    world.raise(soln, &mut attempt);
    if attempt.cancelled {
        return None;
    }
    let mut lowest = FixedPoint2::MAX;
    for ((_, reactant), quantity) in reaction.reactants.iter().zip(quantities) {
        if quantity <= FixedPoint2::ZERO {
            return None;
        }
        if reactant.catalyst {
            if reaction.quantized && quantity < reactant.amount {
                return None;
            }
            continue;
        }
        let units = quantity / reactant.amount;
        if units < lowest {
            lowest = units;
        }
    }
    if reaction.quantized {
        lowest = FixedPoint2::new(lowest.int());
    }
    (lowest > FixedPoint2::ZERO).then_some(lowest)
}

/// Провести реакцию (`PerformReaction`); возвращает продукты.
fn perform_reaction(
    world: &mut World,
    soln: Entity,
    reaction: &Reaction,
    units: FixedPoint2,
) -> Vec<(String, FixedPoint2)> {
    let catalog = crate::solution::reagents(world);
    let products: Vec<(String, FixedPoint2)> = reaction
        .products
        .iter()
        .map(|(id, amount)| (id.clone(), *amount * units))
        .collect();
    if let Some(mut component) = world.get_mut::<SolutionComponent>(soln) {
        let solution = &mut component.solution;
        let energy = if reaction.conserve_energy {
            solution.thermal_energy(&catalog)
        } else {
            0.0
        };
        for (name, reactant) in &reaction.reactants {
            if !reactant.catalyst {
                solution.remove_reagent(
                    &ReagentId::new(name.clone()),
                    units * reactant.amount,
                    true,
                );
            }
        }
        for (id, amount) in &products {
            solution.add_reagent(ReagentId::new(id.clone()), *amount);
        }
        if reaction.conserve_energy {
            let heat = solution.heat_capacity(&catalog);
            if heat > 0.0 {
                solution.temperature = energy / heat;
            }
        }
    }
    on_reaction(world, soln, reaction, units);
    products
}

/// После реакции: журнал, эффекты, звук (`OnReaction`).
fn on_reaction(world: &mut World, soln: Entity, reaction: &Reaction, units: FixedPoint2) {
    let name = world.entity_name(soln);
    crate::admin_logs::add_admin_log(
        world,
        LogType::ChemicalReaction,
        reaction.impact,
        &format!(
            "Chemical reaction {} occurred with strength {units} on entity {name}",
            reaction.id
        ),
        &[],
        &[],
    );
    apply_effects(world, soln, &reaction.effects, units.float(), None);
    if let Some(sound) = &reaction.sound {
        let owner = containers::solution_owner(world, soln);
        world.play_entity(sound, owner, None);
    }
}

/// Провести все возможные реакции раствора (`FullyReactSolution`).
pub fn fully_react_solution(world: &mut World, soln: Entity, mixer: Option<&[String]>) {
    let catalog = reactions(world);
    let Some(contents) = containers::solution(world, soln).map(|s| s.contents.clone()) else {
        return;
    };
    let mut queue: BTreeSet<(OrderKey, usize)> = BTreeSet::new();
    let enqueue = |queue: &mut BTreeSet<_>, list: Option<&Vec<usize>>| {
        for &index in list.into_iter().flatten() {
            queue.insert((catalog.reactions[index].order_key(), index));
        }
    };
    for (id, _) in &contents {
        enqueue(&mut queue, catalog.by_first.get(&id.prototype));
    }
    for _ in 0..MAX_REACTION_ITERATIONS {
        let mut products = None;
        for (_, index) in queue.iter() {
            let reaction = &catalog.reactions[*index];
            if let Some(units) = can_react(world, soln, reaction, mixer) {
                products = Some(perform_reaction(world, soln, reaction, units));
                break;
            }
        }
        let Some(products) = products else {
            return;
        };
        for (product, _) in &products {
            enqueue(&mut queue, catalog.by_any.get(product));
        }
    }
}

/// Задать температуру раствора (`SetSolutionTemperature`).
#[derive(Debug)]
struct SetSolutionTemperature {
    temperature: f32,
}

impl EffectKind for SetSolutionTemperature {
    fn apply(&self, world: &mut World, target: Entity, _: f32, _: Option<Entity>) {
        if world.get::<SolutionComponent>(target).is_some() {
            containers::set_temperature(world, target, self.temperature);
        }
    }
}

/// Сдвинуть температуру раствора в пределах (`AdjustSolutionTemperature`).
#[derive(Debug)]
struct AdjustSolutionTemperature {
    delta: f32,
    min: f32,
    max: f32,
}

impl EffectKind for AdjustSolutionTemperature {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        let Some(current) = containers::solution(world, target).map(|s| s.temperature) else {
            return;
        };
        let temperature = (current + scale * self.delta).clamp(self.min, self.max);
        containers::set_temperature(world, target, temperature);
    }
}

/// Добавить тепло раствору в пределах (`AdjustSolutionThermalEnergy`).
#[derive(Debug)]
struct AdjustSolutionThermalEnergy {
    delta: f32,
    min: f32,
    max: f32,
}

impl EffectKind for AdjustSolutionThermalEnergy {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        let Some(current) = containers::solution(world, target).map(|s| s.temperature) else {
            return;
        };
        let delta = scale * self.delta;
        if (delta > 0.0 && current >= self.max)
            || (delta < 0.0 && current <= self.min)
            || delta == 0.0
        {
            return;
        }
        containers::add_thermal_energy_clamped(world, target, delta, self.min, self.max);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<ReactionMixer>();
    world.register_effect("SetSolutionTemperature", |node| {
        Some(Arc::new(SetSolutionTemperature {
            temperature: number(node, "temperature")?,
        }))
    });
    world.register_effect("AdjustSolutionTemperature", |node| {
        Some(Arc::new(AdjustSolutionTemperature {
            delta: number(node, "delta")?,
            min: number(node, "minTemp").unwrap_or(0.0),
            max: number(node, "maxTemp").unwrap_or(f32::INFINITY),
        }))
    });
    world.register_effect("AdjustSolutionThermalEnergy", |node| {
        Some(Arc::new(AdjustSolutionThermalEnergy {
            delta: number(node, "delta")?,
            min: number(node, "minTemp").unwrap_or(0.0),
            max: number(node, "maxTemp").unwrap_or(f32::INFINITY),
        }))
    });
    if !side.is_server() {
        return;
    }
    world.insert_resource(ReactionHook(|world, soln| {
        fully_react_solution(world, soln, None);
    }));
}

#[cfg(test)]
mod tests;
