//! Эффекты и условия из данных (ADR-0018): реестр видов по тегу `!type:`, общие поля эффекта
//! (`conditions`, `minScale`, `scaling`, `probability`) и условия (`inverted`), применение
//! как в SS14 — порог масштаба, вероятность, условия, масштаб не выше 1 без `scaling`.
//! Неизвестные виды не применяются и учитываются. Здесь же — эффекты положения сущности:
//! появление сущностей рядом, всплывающее сообщение, газ в воздухе вокруг, взрыв. Образец —
//! `EntityEffect`, `SharedEntityEffectsSystem`, `EntityCondition`,
//! `SharedEntityConditionsSystem`, `SpawnEntityEntityEffectSystem`,
//! `PopupMessageEntityEffectSystem`, `CreateGasEntityEffectSystem`,
//! `ExplosionEntityEffectSystem` SS14 @ 2f4a2aad.
//!
//! Всплывающее «всем, кроме цели» пока показывается всем.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

use astrocore_data::DataNode;
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::{ContainerWorldExt, MapWorldExt, SpawnWorldExt, TransformWorldExt};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::examine::loc;
use crate::popups::{PopupType, PopupWorldExt};

/// Вид эффекта: что делает с целью.
pub trait EffectKind: Send + Sync + fmt::Debug {
    /// Применить к цели с масштабом (`RaiseEvent` → обработчик системы).
    fn apply(&self, world: &mut World, target: Entity, scale: f32, user: Option<Entity>);

    /// Можно ли масштабу быть больше 1 по умолчанию (`Scaling`).
    fn scaling(&self) -> bool {
        true
    }
}

/// Вид условия: выполнено ли для цели.
pub trait ConditionKind: Send + Sync + fmt::Debug {
    fn check(&self, world: &mut World, target: Entity, source: Option<Entity>) -> bool;
}

/// Условие с переворотом (`EntityCondition`).
#[derive(Debug, Clone)]
pub struct EntityCondition {
    pub name: String,
    pub kind: Arc<dyn ConditionKind>,
    pub inverted: bool,
}

/// Эффект с общими полями (`EntityEffect`).
#[derive(Debug, Clone)]
pub struct EntityEffect {
    pub name: String,
    pub kind: Arc<dyn EffectKind>,
    pub conditions: Vec<EntityCondition>,
    pub min_scale: f32,
    pub scaling: bool,
    pub probability: f32,
}

type EffectParser = fn(&DataNode) -> Option<Arc<dyn EffectKind>>;
type ConditionParser = fn(&DataNode) -> Option<Arc<dyn ConditionKind>>;

/// Виды эффектов и условий по тегу. Ресурс `World`.
#[derive(Resource, Default)]
pub struct EffectRegistry {
    effects: HashMap<&'static str, EffectParser>,
    conditions: HashMap<&'static str, ConditionParser>,
    /// Встреченные неизвестные виды и сколько раз.
    pub unknown: BTreeMap<String, usize>,
}

/// Регистрация видов на `World`.
pub trait EffectsWorldExt {
    fn register_effect(&mut self, name: &'static str, parser: EffectParser);
    fn register_condition(&mut self, name: &'static str, parser: ConditionParser);
}

impl EffectsWorldExt for World {
    fn register_effect(&mut self, name: &'static str, parser: EffectParser) {
        self.get_resource_or_insert_with(EffectRegistry::default)
            .effects
            .insert(name, parser);
    }

    fn register_condition(&mut self, name: &'static str, parser: ConditionParser) {
        self.get_resource_or_insert_with(EffectRegistry::default)
            .conditions
            .insert(name, parser);
    }
}

/// Имя вида из тега `!type:Имя`.
pub fn tag(node: &DataNode) -> &str {
    node.tag
        .as_deref()
        .and_then(|t| t.strip_prefix("type:"))
        .unwrap_or("")
}

/// Число поля.
pub fn number(node: &DataNode, key: &str) -> Option<f32> {
    node.get(key)?.as_str()?.trim().parse().ok()
}

/// Логическое поле.
pub fn flag(node: &DataNode, key: &str) -> Option<bool> {
    Some(node.get(key)?.as_str()?.trim().eq_ignore_ascii_case("true"))
}

/// Текстовое поле.
pub fn text(node: &DataNode, key: &str) -> Option<String> {
    node.get(key)?.as_str().map(str::to_owned)
}

/// Поле как значение serde.
pub fn field<T: serde::de::DeserializeOwned>(node: &DataNode, key: &str) -> Option<T> {
    astrocore_data::from_node(node.get(key)?).ok()
}

fn note_unknown(world: &mut World, name: &str) {
    *world
        .get_resource_or_insert_with(EffectRegistry::default)
        .unknown
        .entry(name.to_owned())
        .or_default() += 1;
}

/// Условие из данных; неизвестное — `None` и учёт.
pub fn parse_condition(world: &mut World, node: &DataNode) -> Option<EntityCondition> {
    let name = tag(node).to_owned();
    let parser = world
        .get_resource::<EffectRegistry>()
        .and_then(|r| r.conditions.get(name.as_str()).copied());
    let Some(kind) = parser.and_then(|p| p(node)) else {
        note_unknown(world, &name);
        return None;
    };
    Some(EntityCondition {
        name,
        kind,
        inverted: flag(node, "inverted").unwrap_or(false),
    })
}

/// Условия списка; неизвестное условие делает эффект неприменимым (`None`).
pub fn parse_conditions(
    world: &mut World,
    node: Option<&DataNode>,
) -> Option<Vec<EntityCondition>> {
    let Some(list) = node.and_then(DataNode::as_seq) else {
        return Some(Vec::new());
    };
    list.iter().map(|n| parse_condition(world, n)).collect()
}

/// Эффект из данных; неизвестный вид или условие — `None` и учёт.
pub fn parse_effect(world: &mut World, node: &DataNode) -> Option<EntityEffect> {
    let name = tag(node).to_owned();
    let parser = world
        .get_resource::<EffectRegistry>()
        .and_then(|r| r.effects.get(name.as_str()).copied());
    let Some(kind) = parser.and_then(|p| p(node)) else {
        note_unknown(world, &name);
        return None;
    };
    let conditions = parse_conditions(world, node.get("conditions"))?;
    let scaling = flag(node, "scaling").unwrap_or_else(|| kind.scaling());
    Some(EntityEffect {
        name,
        kind,
        conditions,
        min_scale: number(node, "minScale").unwrap_or(0.0),
        scaling,
        probability: number(node, "probability").unwrap_or(1.0),
    })
}

/// Список эффектов; неизвестные пропускаются.
pub fn parse_effects(world: &mut World, node: Option<&DataNode>) -> Vec<EntityEffect> {
    node.and_then(DataNode::as_seq)
        .map(|list| list.iter().filter_map(|n| parse_effect(world, n)).collect())
        .unwrap_or_default()
}

/// Все условия выполнены (`TryConditions`).
pub fn try_conditions(
    world: &mut World,
    target: Entity,
    conditions: &[EntityCondition],
    source: Option<Entity>,
) -> bool {
    conditions
        .iter()
        .all(|c| c.inverted != c.kind.check(world, target, source))
}

/// Применить эффект с проверками (`TryApplyEffect`).
pub fn try_apply_effect(
    world: &mut World,
    target: Entity,
    effect: &EntityEffect,
    scale: f32,
    user: Option<Entity>,
) -> bool {
    if scale < effect.min_scale {
        return false;
    }
    if effect.probability <= 1.0 && !crate::random::random(world).prob(effect.probability) {
        return false;
    }
    if !try_conditions(world, target, &effect.conditions, None) {
        return false;
    }
    let scale = if effect.scaling {
        scale
    } else {
        scale.min(1.0)
    };
    effect.kind.apply(world, target, scale, user);
    true
}

/// Применить список (`ApplyEffects`); `true` — сработал хоть один.
pub fn apply_effects(
    world: &mut World,
    target: Entity,
    effects: &[EntityEffect],
    scale: f32,
    user: Option<Entity>,
) -> bool {
    let mut any = false;
    for effect in effects {
        any |= try_apply_effect(world, target, effect, scale, user);
    }
    any
}

/// Сущности рядом с целью; в контейнере — рядом с хозяином (`SpawnEntity`).
#[derive(Debug)]
struct SpawnEntity {
    number: i32,
    entity: String,
}

/// Где появиться рядом с сущностью: у верхнего хозяина контейнеров (`SpawnNextToOrDrop`).
pub fn spawn_next_to(world: &mut World, prototype: &str, near: Entity) -> Option<Entity> {
    let mut place = near;
    while let Some(holder) = world.containing_container(place) {
        place = holder.owner;
    }
    let at = world.map_coordinates(place)?;
    let coordinates = world.snap_to_grid_or_map(at)?;
    world.spawn_attached(prototype, coordinates).ok()
}

impl EffectKind for SpawnEntity {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        let quantity = self.number * scale.floor() as i32;
        for _ in 0..quantity {
            spawn_next_to(world, &self.entity, target);
        }
    }
}

/// Кому показать (`PopupRecipients`), как (`PopupMethod`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recipients {
    Pvs,
    Local,
    PvsExceptTarget,
}

/// Всплывающее сообщение (`PopupMessage`).
#[derive(Debug)]
struct PopupMessage {
    messages: Vec<String>,
    recipients: Recipients,
    at_coordinates: bool,
    visual: PopupType,
}

fn popup_type(name: &str) -> PopupType {
    match name {
        "SmallCaution" => PopupType::SmallCaution,
        "Medium" => PopupType::Medium,
        "MediumCaution" => PopupType::MediumCaution,
        "Large" => PopupType::Large,
        "LargeCaution" => PopupType::LargeCaution,
        _ => PopupType::Small,
    }
}

impl EffectKind for PopupMessage {
    fn apply(&self, world: &mut World, target: Entity, _: f32, _: Option<Entity>) {
        if self.messages.is_empty() {
            return;
        }
        // Верхняя граница не включается.
        let count = self.messages.len() as i32;
        let index = crate::random::random(world).range(0, count) as usize;
        let name = world.entity_arg(target);
        let text = loc(world, &self.messages[index], &[("entity", name.into())]);
        match (self.at_coordinates, self.recipients) {
            (false, Recipients::Local) => {
                world.popup_entity_for(&text, target, target, self.visual)
            }
            (false, _) => world.popup_entity(&text, target, self.visual),
            (true, Recipients::Local) => {
                if let Some(at) = world.map_coordinates(target) {
                    world.popup_coordinates_for(&text, at, target, self.visual);
                }
            }
            (true, _) => {
                if let Some(at) = world.map_coordinates(target) {
                    world.popup_coordinates(&text, at, self.visual);
                }
            }
        }
    }
}

/// Газ в воздух вокруг (`CreateGas`).
#[derive(Debug)]
struct CreateGas {
    gas: usize,
    moles: f32,
}

impl EffectKind for CreateGas {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        let (gas, moles) = (self.gas, self.moles * scale);
        crate::barotrauma::with_containing_mixture(world, target, true, |air| {
            air.adjust_moles(gas, moles);
        });
    }
}

/// Взрыв (`ExplosionEffect`): сила по масштабу.
#[derive(Debug)]
struct ExplosionEffect {
    explosion_type: String,
    max_intensity: f32,
    intensity_slope: f32,
    max_total_intensity: f32,
    intensity_per_unit: f32,
    tile_break_scale: f32,
}

impl EffectKind for ExplosionEffect {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        let intensity = (self.intensity_per_unit * scale).min(self.max_total_intensity);
        let Some(at) = world.map_coordinates(target) else {
            return;
        };
        crate::explosion::queue_explosion(
            world,
            at,
            &self.explosion_type,
            intensity,
            self.intensity_slope,
            self.max_intensity,
            Some(target),
            self.tile_break_scale,
            i32::MAX,
            true,
        );
    }
}

pub fn install(world: &mut World) {
    world.init_resource::<EffectRegistry>();
    world.register_effect("SpawnEntity", |node| {
        Some(Arc::new(SpawnEntity {
            number: number(node, "number").map_or(1, |n| n as i32),
            entity: text(node, "entity")?,
        }))
    });
    world.register_effect("PopupMessage", |node| {
        let recipients = match text(node, "type").as_deref() {
            Some("Pvs") => Recipients::Pvs,
            Some("PvsExceptTarget") => Recipients::PvsExceptTarget,
            _ => Recipients::Local,
        };
        Some(Arc::new(PopupMessage {
            messages: field(node, "messages")?,
            recipients,
            at_coordinates: text(node, "method").as_deref() == Some("PopupCoordinates"),
            visual: popup_type(text(node, "visualType").as_deref().unwrap_or("Small")),
        }))
    });
    world.register_effect("CreateGas", |node| {
        let gas = text(node, "gas")?;
        let index = crate::atmos::gas::gas_index(&gas).or_else(|| gas.parse().ok())?;
        Some(Arc::new(CreateGas {
            gas: index,
            moles: number(node, "moles").unwrap_or(3.0),
        }))
    });
    world.register_effect("Explosion", |node| {
        Some(Arc::new(ExplosionEffect {
            explosion_type: text(node, "explosionType")?,
            max_intensity: number(node, "maxIntensity").unwrap_or(5.0),
            intensity_slope: number(node, "intensitySlope").unwrap_or(1.0),
            max_total_intensity: number(node, "maxTotalIntensity").unwrap_or(100.0),
            intensity_per_unit: number(node, "intensityPerUnit").unwrap_or(1.0),
            tile_break_scale: number(node, "tileBreakScale").unwrap_or(1.0),
        }))
    });
}

#[cfg(test)]
mod tests;
