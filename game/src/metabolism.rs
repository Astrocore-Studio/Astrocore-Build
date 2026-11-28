//! Метаболизм органов: раз в секунду орган перерабатывает свои стадии — берёт раствор стадии
//! (на теле или на себе: лёгкие — «Lung», желудок — «stomach», сердце — кровь, печень —
//! метаболиты), в случайном порядке, не больше `maxReagents` реагентов за раз: у реагента
//! этой стадии — эффекты из прототипа (с масштабом по переработанному количеству, вероятностью
//! и условиями) и метаболиты в раствор переноса; без стадии — переносит по 0,25 в раствор
//! переноса (с долей `transferEfficacy`) или убирает 1. Реагенты крови не трогаются; у
//! мёртвых реагент, который не действует на мёртвых, пропускается и не убывает (как в SS14).
//! На пределе реагентов стадия обрывается без проведения изменений раствора (как в SS14).
//! Тип метаболизма проверяется на органе, количество реагента — на растворе, остальное — на
//! теле. Здесь же — общие условия и эффекты урона и раствора. Образец — `MetabolizerSystem`, `MetabolizerComponent`,
//! `MetabolismSolutionEntry`, `ReagentEffectsEntry`, условия `ReagentCondition`,
//! `MetabolizerTypeCondition`, `MobStateCondition`, `TemperatureCondition`, `TagCondition`,
//! эффекты `HealthChange`, `EvenHealthChange`, `AdjustReagent` SS14 @ 2f4a2aad.
//!
//! Нет пока: множителя обмена веществ от статус-эффектов (`RushedMetabolism`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use astrocore_data::DataNode;
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::body::Organ;
use crate::damage::{DamageSpecifier, Damageable, DamageableWorldExt};
use crate::entity_effects::{
    ConditionKind, EffectKind, EffectsWorldExt, EntityCondition, EntityEffect, field, number,
    parse_effects,
};
use crate::fixed_point::FixedPoint2;
use crate::mobs::{MobState, MobStateWorldExt};
use crate::solution::ReagentId;
use crate::solution_container::{self as containers, SolutionComponent};
use crate::tag::TagWorldExt;

fn yes() -> bool {
    true
}
fn quarter() -> FixedPoint2 {
    FixedPoint2::from_cents(25)
}
fn one_fixed() -> FixedPoint2 {
    FixedPoint2::new(1)
}

/// Какой раствор перерабатывает стадия (`MetabolismSolutionEntry`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetabolismSolutionEntry {
    pub solution_name: String,
    #[serde(default = "yes")]
    pub solution_on_body: bool,
    #[serde(default)]
    pub metabolize_all: bool,
    #[serde(default)]
    pub transfer_solution_name: Option<String>,
    #[serde(default = "quarter")]
    pub transfer_rate: FixedPoint2,
    #[serde(default = "one_fixed")]
    pub transfer_efficacy: FixedPoint2,
    #[serde(default = "yes")]
    pub transfer_solution_on_body: bool,
}

fn entry(
    name: &str,
    on_body: bool,
    all: bool,
    transfer: Option<&str>,
    efficacy: FixedPoint2,
) -> MetabolismSolutionEntry {
    MetabolismSolutionEntry {
        solution_name: name.into(),
        solution_on_body: on_body,
        metabolize_all: all,
        transfer_solution_name: transfer.map(str::to_owned),
        transfer_rate: quarter(),
        transfer_efficacy: efficacy,
        transfer_solution_on_body: true,
    }
}

/// Растворы стадий по умолчанию (`Solutions`).
fn default_solutions() -> BTreeMap<String, MetabolismSolutionEntry> {
    BTreeMap::from([
        (
            "Respiration".into(),
            entry("Lung", false, true, Some("bloodstream"), one_fixed()),
        ),
        (
            "Digestion".into(),
            entry(
                "stomach",
                false,
                false,
                Some("bloodstream"),
                FixedPoint2::from_cents(50),
            ),
        ),
        (
            "Bloodstream".into(),
            entry("bloodstream", true, false, Some("metabolites"), one_fixed()),
        ),
        (
            "Metabolites".into(),
            entry("metabolites", true, false, None, one_fixed()),
        ),
    ])
}

fn one() -> f32 {
    1.0
}
fn three() -> i32 {
    3
}

/// Орган перерабатывает реагенты (`MetabolizerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metabolizer {
    /// С.
    #[serde(default = "one")]
    pub update_interval: f32,
    #[serde(default = "one")]
    pub update_interval_multiplier: f32,
    #[serde(default = "default_solutions")]
    pub solutions: BTreeMap<String, MetabolismSolutionEntry>,
    #[serde(default = "yes")]
    pub solution_on_body: bool,
    #[serde(default)]
    pub metabolizer_types: Option<Vec<String>>,
    #[serde(rename = "maxReagents", default = "three")]
    pub max_reagents_processable: i32,
    #[serde(default)]
    pub stages: Vec<String>,
    #[serde(skip)]
    pub next_update: Duration,
}

impl Metabolizer {
    /// Интервал с учётом множителя (`AdjustedUpdateInterval`).
    pub fn adjusted_interval(&self) -> Duration {
        Duration::from_secs_f32(self.update_interval * self.update_interval_multiplier)
    }
}

/// Стадия реагента (`ReagentEffectsEntry`).
#[derive(Debug, Clone)]
pub struct ReagentStage {
    pub metabolism_rate: FixedPoint2,
    pub effects: Vec<EntityEffect>,
    pub metabolites: Vec<(String, FixedPoint2)>,
}

/// Реагент для метаболизма: стадии и «действует на мёртвых».
#[derive(Debug, Clone, Default)]
pub struct ReagentMetabolisms {
    pub stages: HashMap<String, Arc<ReagentStage>>,
    pub works_on_the_dead: bool,
}

/// Метаболизм реагентов, разобранный при первом обращении. Ресурс `World`.
#[derive(Resource, Debug, Default)]
pub struct MetabolismCache(HashMap<String, Arc<ReagentMetabolisms>>);

fn parse_stage(world: &mut World, node: &DataNode) -> ReagentStage {
    ReagentStage {
        metabolism_rate: field(node, "metabolismRate").unwrap_or(FixedPoint2::from_cents(50)),
        effects: parse_effects(world, node.get("effects")),
        metabolites: node
            .get("metabolites")
            .and_then(DataNode::as_map)
            .map(|m| {
                m.iter()
                    .filter_map(|(id, r)| Some((id.to_owned(), astrocore_data::from_node(r).ok()?)))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Метаболизм реагента (`ReagentPrototype.Metabolisms`, `WorksOnTheDead`).
pub fn reagent_metabolisms(world: &mut World, reagent: &str) -> Arc<ReagentMetabolisms> {
    if let Some(found) = world
        .get_resource::<MetabolismCache>()
        .and_then(|c| c.0.get(reagent).cloned())
    {
        return found;
    }
    let node = world
        .get_resource::<Prototypes>()
        .and_then(|p| p.resolved_node("reagent", reagent).cloned());
    let mut parsed = ReagentMetabolisms::default();
    if let Some(node) = node {
        parsed.works_on_the_dead =
            crate::entity_effects::flag(&node, "worksOnTheDead").unwrap_or(false);
        if let Some(map) = node.get("metabolisms").and_then(DataNode::as_map) {
            for (stage, data) in map.iter() {
                let stage_data = parse_stage(world, data);
                parsed.stages.insert(stage.to_owned(), Arc::new(stage_data));
            }
        }
    }
    let parsed = Arc::new(parsed);
    world
        .get_resource_or_insert_with(MetabolismCache::default)
        .0
        .insert(reagent.to_owned(), parsed.clone());
    parsed
}

/// Реагенты, которые метаболизм не трогает (`MetabolismExclusionEvent`): кровь. На хозяине
/// раствора.
#[derive(Debug, Clone, Default)]
pub struct MetabolismExclusion {
    pub reagents: Vec<ReagentId>,
}

fn now(world: &World) -> Duration {
    world.resource::<GameTime>().cur_time()
}

/// Раствор стадии: на теле или на органе (`LookupSolution`).
fn lookup_solution(
    world: &mut World,
    organ: Entity,
    name: Option<&str>,
    on_body: bool,
) -> Option<(Entity, Entity)> {
    let name = name?;
    let owner = if on_body {
        world.get::<Organ>(organ)?.body?
    } else {
        organ
    };
    let soln = containers::try_get_solution(world, owner, name)?;
    Some((soln, owner))
}

/// Условия эффекта на нужных сущностях (`CanMetabolizeEffect`).
fn can_metabolize_effect(
    world: &mut World,
    body: Entity,
    organ: Entity,
    solution: Entity,
    conditions: &[EntityCondition],
) -> bool {
    conditions.iter().all(|condition| {
        let target = match condition.name.as_str() {
            "MetabolizerTypeCondition" => organ,
            "ReagentCondition" => solution,
            _ => body,
        };
        condition.inverted != condition.kind.check(world, target, None)
    })
}

/// Переработать одну стадию органа (`TryMetabolizeStage`).
fn metabolize_stage(world: &mut World, organ: Entity, stage: &str) {
    let Some(metabolizer) = world.get::<Metabolizer>(organ).cloned() else {
        return;
    };
    let Some(data) = metabolizer.solutions.get(stage).cloned() else {
        return;
    };
    let Some((soln, owner)) = lookup_solution(
        world,
        organ,
        Some(&data.solution_name),
        data.solution_on_body,
    ) else {
        return;
    };
    let Some(mut list) = containers::solution(world, soln).map(|s| s.contents.clone()) else {
        return;
    };
    if list.is_empty() {
        return;
    }
    let transfer = lookup_solution(
        world,
        organ,
        data.transfer_solution_name.as_deref(),
        data.transfer_solution_on_body,
    )
    .map(|(t, _)| t);
    let mut exclusion = MetabolismExclusion::default();
    world.raise(owner, &mut exclusion);
    // Случайный порядок, как в SS14.
    {
        let mut random = crate::random::random(world);
        for i in (1..list.len()).rev() {
            let j = random.range(0, i as i32 + 1) as usize;
            list.swap(i, j);
        }
    }
    let dead = world.is_mob_dead(owner);
    let body = world
        .get::<Organ>(organ)
        .and_then(|o| o.body)
        .unwrap_or(owner);
    let mut processed = 0;
    for (reagent, quantity) in list {
        if exclusion.reagents.contains(&reagent) {
            continue;
        }
        let info = reagent_metabolisms(world, &reagent.prototype);
        let Some(entry) = info.stages.get(stage).cloned() else {
            let most = data.transfer_rate.clamp(FixedPoint2::ZERO, quantity);
            match transfer {
                Some(transfer) => {
                    remove(world, soln, &reagent, most);
                    add(world, transfer, &reagent, most * data.transfer_efficacy);
                }
                None => remove(world, soln, &reagent, FixedPoint2::new(1)),
            }
            continue;
        };
        let rate = if data.metabolize_all {
            quantity
        } else {
            entry.metabolism_rate
        };
        let most = rate.clamp(FixedPoint2::ZERO, quantity);
        // Как в SS14: на пределе выход без проведения изменений раствора.
        if processed >= metabolizer.max_reagents_processable {
            return;
        }
        let mut scale = most.float();
        if !data.metabolize_all {
            scale /= rate.float();
        }
        if dead && !info.works_on_the_dead {
            continue;
        }
        for effect in &entry.effects {
            if scale < effect.min_scale {
                continue;
            }
            if crate::random::random(world).next_f32() >= effect.probability {
                continue;
            }
            if !can_metabolize_effect(world, body, organ, soln, &effect.conditions) {
                continue;
            }
            let target = match effect.name.as_str() {
                "ModifyLungGas" => organ,
                "AdjustReagent" => soln,
                _ => body,
            };
            let applied = if effect.scaling {
                scale
            } else {
                scale.min(1.0)
            };
            effect.kind.apply(world, target, applied, None);
        }
        if most > FixedPoint2::ZERO {
            remove(world, soln, &reagent, most);
            processed += 1;
            if let Some(transfer) = transfer {
                for (metabolite, ratio) in &entry.metabolites {
                    add(
                        world,
                        transfer,
                        &ReagentId::new(metabolite.clone()),
                        most * *ratio,
                    );
                }
            }
        }
    }
    containers::update_chemicals(world, soln, true);
    if let Some(transfer) = transfer {
        containers::update_chemicals(world, transfer, true);
    }
}

/// Убрать без проведения изменения: проводится раз в конце стадии.
fn remove(world: &mut World, soln: Entity, reagent: &ReagentId, amount: FixedPoint2) {
    if let Some(mut component) = world.get_mut::<SolutionComponent>(soln) {
        component.solution.remove_reagent(reagent, amount, false);
    }
}

fn add(world: &mut World, soln: Entity, reagent: &ReagentId, amount: FixedPoint2) {
    if let Some(mut component) = world.get_mut::<SolutionComponent>(soln) {
        component.solution.add_reagent(reagent.clone(), amount);
    }
}

/// Переработка органов по их интервалам (`MetabolizerSystem.Update`).
pub fn update(world: &mut World) {
    let time = now(world);
    let due: Vec<Entity> = world
        .query::<(Entity, &Metabolizer)>()
        .iter(world)
        .filter(|(_, m)| time >= m.next_update)
        .map(|(e, _)| e)
        .collect();
    for organ in due {
        let Some(mut metabolizer) = world.get_mut::<Metabolizer>(organ) else {
            continue;
        };
        let interval = metabolizer.adjusted_interval();
        metabolizer.next_update += interval;
        let stages = metabolizer.stages.clone();
        for stage in stages {
            metabolize_stage(world, organ, &stage);
        }
    }
}

/// Количество реагента в растворе в пределах (`ReagentCondition`).
#[derive(Debug)]
struct ReagentCondition {
    reagent: String,
    min: FixedPoint2,
    max: FixedPoint2,
}

impl ConditionKind for ReagentCondition {
    fn check(&self, world: &mut World, target: Entity, _: Option<Entity>) -> bool {
        containers::solution(world, target).is_some_and(|s| {
            let q = s.prototype_quantity(&self.reagent);
            q >= self.min && q <= self.max
        })
    }
}

/// Тип метаболизма органа (`MetabolizerTypeCondition`).
#[derive(Debug)]
struct MetabolizerTypeCondition {
    types: Vec<String>,
}

impl ConditionKind for MetabolizerTypeCondition {
    fn check(&self, world: &mut World, target: Entity, _: Option<Entity>) -> bool {
        world
            .get::<Metabolizer>(target)
            .and_then(|m| m.metabolizer_types.as_ref())
            .is_some_and(|types| self.types.iter().any(|t| types.contains(t)))
    }
}

/// Состояние моба (`MobStateCondition`).
#[derive(Debug)]
struct MobStateCondition {
    state: MobState,
}

impl ConditionKind for MobStateCondition {
    fn check(&self, world: &mut World, target: Entity, _: Option<Entity>) -> bool {
        world.mob_state(target) == Some(self.state)
    }
}

/// Температура тела или раствора в пределах (`TemperatureCondition`).
#[derive(Debug)]
struct TemperatureCondition {
    min: f32,
    max: f32,
}

impl ConditionKind for TemperatureCondition {
    fn check(&self, world: &mut World, target: Entity, _: Option<Entity>) -> bool {
        let temperature = crate::temperature::temperature_of(world, target)
            .or_else(|| containers::solution(world, target).map(|s| s.temperature));
        temperature.is_some_and(|t| t >= self.min && t <= self.max)
    }
}

/// Метки (`TagCondition`, `AllTagsCondition`, `AnyTagCondition`).
#[derive(Debug)]
struct TagsCondition {
    tags: Vec<String>,
    all: bool,
}

impl ConditionKind for TagsCondition {
    fn check(&self, world: &mut World, target: Entity, _: Option<Entity>) -> bool {
        if self.all {
            self.tags.iter().all(|t| world.has_tag(target, t))
        } else {
            self.tags.iter().any(|t| world.has_tag(target, t))
        }
    }
}

/// Урон или лечение по типам (`HealthChange`).
#[derive(Debug)]
struct HealthChange {
    damage: DamageSpecifier,
    ignore_resistances: bool,
}

impl EffectKind for HealthChange {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        if world.get::<Damageable>(target).is_some() {
            world.change_damage(
                target,
                &self.damage.scale(scale),
                self.ignore_resistances,
                false,
                None,
            );
        }
    }
}

/// Лечение поровну по группам (`EvenHealthChange`).
#[derive(Debug)]
struct EvenHealthChange {
    damage: Vec<(String, FixedPoint2)>,
}

impl EffectKind for EvenHealthChange {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        if world.get::<Damageable>(target).is_none() {
            return;
        }
        for (group, amount) in &self.damage {
            world.heal_evenly(target, amount.scale(scale), Some(group), None);
        }
    }
}

/// Добавить или убрать реагент раствора (`AdjustReagent`).
#[derive(Debug)]
struct AdjustReagent {
    reagent: String,
    amount: FixedPoint2,
}

impl EffectKind for AdjustReagent {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        if world.get::<SolutionComponent>(target).is_none() {
            return;
        }
        let quantity = self.amount.scale(scale);
        let id = ReagentId::new(self.reagent.clone());
        if quantity > FixedPoint2::ZERO {
            containers::try_add_reagent(world, target, id, quantity, None);
        } else {
            containers::remove_reagent(world, target, &id, -quantity);
        }
    }
}

fn mob_state(name: &str) -> MobState {
    match name {
        "Critical" => MobState::Critical,
        "Dead" => MobState::Dead,
        "Invalid" => MobState::Invalid,
        _ => MobState::Alive,
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Metabolizer>();
    world.register_condition("ReagentCondition", |node| {
        Some(Arc::new(ReagentCondition {
            reagent: crate::entity_effects::text(node, "reagent")?,
            min: field(node, "min").unwrap_or(FixedPoint2::ZERO),
            max: field(node, "max").unwrap_or(FixedPoint2::MAX),
        }))
    });
    world.register_condition("MetabolizerTypeCondition", |node| {
        Some(Arc::new(MetabolizerTypeCondition {
            types: field(node, "type")?,
        }))
    });
    world.register_condition("MobStateCondition", |node| {
        Some(Arc::new(MobStateCondition {
            state: mob_state(
                crate::entity_effects::text(node, "mobstate")
                    .as_deref()
                    .unwrap_or("Alive"),
            ),
        }))
    });
    world.register_condition("TemperatureCondition", |node| {
        Some(Arc::new(TemperatureCondition {
            min: number(node, "min").unwrap_or(0.0),
            max: number(node, "max").unwrap_or(f32::INFINITY),
        }))
    });
    world.register_condition("TagCondition", |node| {
        Some(Arc::new(TagsCondition {
            tags: vec![crate::entity_effects::text(node, "tag")?],
            all: true,
        }))
    });
    world.register_condition("AllTagsCondition", |node| {
        Some(Arc::new(TagsCondition {
            tags: field(node, "tags")?,
            all: true,
        }))
    });
    world.register_condition("AnyTagCondition", |node| {
        Some(Arc::new(TagsCondition {
            tags: field(node, "tags")?,
            all: false,
        }))
    });
    world.register_effect("HealthChange", |node| {
        Some(Arc::new(HealthChange {
            damage: field(node, "damage")?,
            ignore_resistances: crate::entity_effects::flag(node, "ignoreResistances")
                .unwrap_or(true),
        }))
    });
    world.register_effect("EvenHealthChange", |node| {
        let damage = node
            .get("damage")?
            .as_map()?
            .iter()
            .filter_map(|(group, amount)| {
                Some((group.to_owned(), astrocore_data::from_node(amount).ok()?))
            })
            .collect();
        Some(Arc::new(EvenHealthChange { damage }))
    });
    world.register_effect("AdjustReagent", |node| {
        Some(Arc::new(AdjustReagent {
            reagent: crate::entity_effects::text(node, "reagent")?,
            amount: field(node, "amount")?,
        }))
    });
    if !side.is_server() {
        return;
    }
    world.subscribe::<Metabolizer, MapInit>(|world, organ, _| {
        let time = now(world);
        if let Some(mut metabolizer) = world.get_mut::<Metabolizer>(organ) {
            metabolizer.next_update = time + metabolizer.adjusted_interval();
        }
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
