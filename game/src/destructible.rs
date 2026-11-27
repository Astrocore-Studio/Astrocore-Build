//! Разрушаемость: пороги урона и что происходит при их достижении — разрушение, обломки,
//! звук, смена узла строительства, взрыв, опустошение контейнеров, выброс газа. Образец —
//! `DestructibleSystem`, `SharedDestructibleSystem`, `DamageThreshold`,
//! `DamageThresholdsSerializer`, триггеры `Content.Shared/Destructible/Triggers` и поведения
//! `Content.Server/Destructible/Thresholds/Behaviors` SS14 @ 2f4a2aad.
//!
//! Поведения, чьих систем ещё нет (разлив жидкостей, разрыв тел, торговые автоматы,
//! таймеры и сигналы), читаются из данных и пропускаются.

use std::collections::BTreeMap;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, LifecycleWorldExt, MapInit, RegistryWorldExt};
use astrocore_map::{
    ContainerManager, ContainerWorldExt, MapCoordinates, SpawnWorldExt, TransformWorldExt,
};
use astrocore_math::Vec2;
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::damage::{DamageCatalog, DamageChanged, Damageable};
use crate::fixed_point::FixedPoint2;
use crate::gas_tank::AirData;
use crate::random::random;
use crate::stack::{Stack, StackWorldExt};

/// Наименьший порог переизбытка урона (`MinimumOverkill`).
const MINIMUM_OVERKILL: f32 = 100.0;
/// Во сколько раз порог переизбытка выше порога разрушения (`OverkillMultiplier`).
const OVERKILL_MULTIPLIER: f32 = 2.0;

/// Условие порога (`IThresholdTrigger`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Trigger {
    /// Общий урон (`DamageTrigger`).
    DamageTrigger { damage: FixedPoint2 },
    /// Урон одного типа (`DamageTypeTrigger`).
    #[serde(rename_all = "camelCase")]
    DamageTypeTrigger {
        damage_type: String,
        damage: FixedPoint2,
    },
    /// Урон группы (`DamageGroupTrigger`).
    #[serde(rename_all = "camelCase")]
    DamageGroupTrigger {
        damage_group: String,
        damage: FixedPoint2,
    },
    /// Все условия (`AndTrigger`).
    AndTrigger { triggers: Vec<Trigger> },
    /// Любое условие (`OrTrigger`).
    OrTrigger { triggers: Vec<Trigger> },
}

/// Разброс количества (`MinMax`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct MinMax {
    #[serde(default)]
    pub min: f32,
    #[serde(default)]
    pub max: f32,
}

/// Поведение при пороге (`IThresholdBehavior`). Варианты без полей — поведения, которые пока
/// не исполняются: их поля в данных пропускаются.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Behavior {
    /// `Breakage` и/или `Destruction` (`DoActsBehavior`).
    DoActsBehavior {
        #[serde(default)]
        acts: Vec<String>,
    },
    SpawnEntitiesBehavior {
        #[serde(default)]
        spawn: BTreeMap<String, MinMax>,
        #[serde(default = "half")]
        offset: f32,
        #[serde(default, rename = "spawnInContainer")]
        spawn_in_container: bool,
    },
    PlaySoundBehavior {
        sound: SoundSpecifier,
    },
    ChangeConstructionNodeBehavior {
        #[serde(default)]
        node: String,
    },
    ExplodeBehavior {},
    EmptyContainersBehaviour {
        #[serde(default)]
        containers: Vec<String>,
    },
    EmptyAllContainersBehaviour {},
    SpawnGasBehavior {
        #[serde(rename = "gasMixture")]
        gas_mixture: AirData,
    },
    BurnBodyBehavior {},
    DumpCanisterBehavior {},
    DumpRestockInventory {},
    EjectVendorItems {},
    GibBehavior {},
    OpenBehavior {},
    PopupBehavior {},
    SolutionExplosionBehavior {},
    SpillBehavior {},
    TimerStartBehavior {},
    TriggerBehavior {},
    VomitBehavior {},
    WeightedSpawnEntityBehavior {},
}

fn half() -> f32 {
    0.5
}

impl Behavior {
    fn has_act(&self, act: &str) -> bool {
        matches!(self, Behavior::DoActsBehavior { acts } if acts.iter().any(|a| a == act))
    }
}

/// Порог (`DamageThreshold`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DamageThreshold {
    pub trigger: Trigger,
    #[serde(default)]
    pub behaviors: Vec<Behavior>,
    #[serde(default)]
    pub triggered: bool,
    #[serde(default)]
    pub triggers_once: bool,
    #[serde(skip)]
    pub old_triggered: bool,
}

/// Разрушаемая сущность (`DestructibleComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Destructible {
    #[serde(default)]
    pub thresholds: Vec<DamageThreshold>,
    #[serde(default)]
    pub is_broken: bool,
    #[serde(default = "yes")]
    pub generate_overkill_threshold: bool,
}

fn yes() -> bool {
    true
}

/// Порог достигнут (`DamageThresholdReached`). На сущности.
#[derive(Debug, Clone)]
pub struct DamageThresholdReached {
    pub threshold: usize,
}

/// Можно ли разрушить (`DestructionAttemptEvent`). На сущности.
#[derive(Debug, Clone, Copy, Default)]
pub struct DestructionAttempt {
    pub cancelled: bool,
}

/// Сущность разрушена и сейчас удалится (`DestructionEventArgs`). На сущности.
#[derive(Debug, Clone, Copy)]
pub struct Destroyed;

/// Сущность сломалась (`BreakageEventArgs`). На сущности.
#[derive(Debug, Clone, Copy)]
pub struct Broken;

/// Совпадают ли условия (`IsEqual` сериализатора).
fn same_trigger(a: &Trigger, b: &Trigger) -> bool {
    a == b
}

/// Порядок порогов (`DamageThreshold.CompareTo`): общий урон — по величине, прочие равны.
fn compare(a: &DamageThreshold, b: &DamageThreshold) -> std::cmp::Ordering {
    match (&a.trigger, &b.trigger) {
        (Trigger::DamageTrigger { damage: x }, Trigger::DamageTrigger { damage: y }) => x.cmp(y),
        (
            Trigger::DamageTypeTrigger {
                damage_type: t1,
                damage: x,
            },
            Trigger::DamageTypeTrigger {
                damage_type: t2,
                damage: y,
            },
        ) if t1 == t2 => x.cmp(y),
        (
            Trigger::DamageGroupTrigger {
                damage_group: g1,
                damage: x,
            },
            Trigger::DamageGroupTrigger {
                damage_group: g2,
                damage: y,
            },
        ) if g1 == g2 => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    }
}

/// Пороги как после `DamageThresholdsSerializer.Read`: одинаковые условия сливаются,
/// пороги сортируются.
pub fn normalize(thresholds: Vec<DamageThreshold>) -> Vec<DamageThreshold> {
    let mut list: Vec<DamageThreshold> = Vec::new();
    for item in thresholds {
        if let Some(existing) = list
            .iter_mut()
            .find(|t| same_trigger(&t.trigger, &item.trigger))
        {
            for behavior in item.behaviors {
                if !existing.behaviors.contains(&behavior) {
                    existing.behaviors.push(behavior);
                }
            }
        } else {
            list.push(item);
        }
    }
    list.sort_by(compare);
    list
}

fn reached(world: &World, trigger: &Trigger, damageable: &Damageable) -> bool {
    match trigger {
        Trigger::DamageTrigger { damage } => damageable.total_damage >= *damage,
        Trigger::DamageTypeTrigger {
            damage_type,
            damage,
        } => damageable
            .damage
            .types
            .get(damage_type)
            .is_some_and(|value| value >= damage),
        Trigger::DamageGroupTrigger {
            damage_group,
            damage,
        } => world
            .get_resource::<DamageCatalog>()
            .map(|c| c.per_group(&damageable.damage))
            .and_then(|groups| groups.get(damage_group).copied())
            .is_some_and(|value| value >= *damage),
        Trigger::AndTrigger { triggers } => triggers.iter().all(|t| reached(world, t, damageable)),
        Trigger::OrTrigger { triggers } => triggers.iter().any(|t| reached(world, t, damageable)),
    }
}

/// Урон, при котором сущность разрушится или сломается (`DestroyedAt`).
pub fn destroyed_at(world: &World, entity: Entity) -> FixedPoint2 {
    let Some(destructible) = world.get::<Destructible>(entity) else {
        return FixedPoint2::MAX;
    };
    destructible
        .thresholds
        .iter()
        .filter_map(|threshold| match threshold.trigger {
            Trigger::DamageTrigger { damage }
                if threshold
                    .behaviors
                    .iter()
                    .any(|b| b.has_act("Destruction") || b.has_act("Breakage")) =>
            {
                Some(damage)
            }
            _ => None,
        })
        .min()
        .unwrap_or(FixedPoint2::MAX)
}

/// Разрушить (`DestroyEntity`): если никто не против — событие и удаление.
pub fn destroy_entity(world: &mut World, entity: Entity) -> bool {
    let mut attempt = DestructionAttempt::default();
    world.raise(entity, &mut attempt);
    if attempt.cancelled {
        return false;
    }
    world.raise(entity, &mut Destroyed);
    world.entity_mut(entity).insert(DestroyedMarker);
    world.queue_delete(entity);
    true
}

/// Сущность разрушена в этом такте и ждёт удаления (`IsQueuedForDeletion`).
#[derive(Component, Debug, Clone, Copy)]
pub struct DestroyedMarker;

/// Разрушена ли сущность и ждёт удаления.
pub fn is_destroyed(world: &World, entity: Entity) -> bool {
    !world.is_alive(entity) || world.get::<DestroyedMarker>(entity).is_some()
}

/// Сломать (`BreakEntity`).
pub fn break_entity(world: &mut World, entity: Entity) {
    world.raise(entity, &mut Broken);
}

fn random_offset(world: &mut World, offset: f32) -> Vec2 {
    let mut rng = random(world);
    Vec2::new(
        (rng.next_f32() * 2.0 - 1.0) * offset,
        (rng.next_f32() * 2.0 - 1.0) * offset,
    )
}

fn spawn_entities(world: &mut World, owner: Entity, spawn: &BTreeMap<String, MinMax>, offset: f32) {
    let Some(position) = world.map_coordinates(owner) else {
        return;
    };
    let executions = world.get::<Stack>(owner).map_or(1, |s| s.count.max(1));
    for (prototype, range) in spawn {
        let is_stack = world
            .get_resource::<Prototypes>()
            .and_then(|p| p.entity(prototype))
            .is_some_and(|p| p.component("Stack").is_some());
        for _ in 0..executions {
            let count = if range.min >= range.max {
                range.min
            } else {
                let roll = random(world).next_f32();
                (range.min + roll * (range.max + 1.0 - range.min))
                    .floor()
                    .min(range.max)
            } as i32;
            if count <= 0 {
                continue;
            }
            let copies = if is_stack { 1 } else { count };
            for _ in 0..copies {
                let shift = random_offset(world, offset);
                let at =
                    MapCoordinates::on_level(position.map, position.position + shift, position.z);
                if let Ok(spawned) = world.spawn_on_map(prototype, at)
                    && is_stack
                {
                    world.set_stack_count(spawned, count);
                }
            }
        }
    }
}

fn empty_containers(world: &mut World, owner: Entity, names: Option<&[String]>) {
    let all: Vec<String> = world
        .get::<ContainerManager>(owner)
        .map(|m| m.containers.keys().cloned().collect())
        .unwrap_or_default();
    for name in all {
        if names.is_none_or(|n| n.contains(&name)) {
            world.empty_container(owner, &name);
        }
    }
}

/// Исполнить поведение (`IThresholdBehavior.Execute`).
fn execute(world: &mut World, owner: Entity, behavior: &Behavior, cause: Option<Entity>) {
    match behavior {
        Behavior::DoActsBehavior { .. } => {
            if behavior.has_act("Breakage") {
                break_entity(world, owner);
            }
            if behavior.has_act("Destruction") {
                destroy_entity(world, owner);
            }
        }
        Behavior::SpawnEntitiesBehavior { spawn, offset, .. } => {
            spawn_entities(world, owner, spawn, *offset)
        }
        Behavior::PlaySoundBehavior { sound } => {
            if let Some(at) = world.map_coordinates(owner) {
                world.play_at(sound, at, None);
            }
        }
        Behavior::ChangeConstructionNodeBehavior { node } => {
            if !node.is_empty()
                && world
                    .get::<crate::construction::Construction>(owner)
                    .is_some()
            {
                crate::construction::change_node(world, owner, None, node, true);
            }
        }
        Behavior::ExplodeBehavior {} => crate::explosion::trigger_explosive(world, owner, cause),
        Behavior::EmptyContainersBehaviour { containers } => {
            empty_containers(world, owner, Some(containers))
        }
        Behavior::EmptyAllContainersBehaviour {} => empty_containers(world, owner, None),
        Behavior::SpawnGasBehavior { gas_mixture } => {
            let gas = gas_mixture.mixture();
            if let Some(at) = world.map_coordinates(owner) {
                crate::atmos::merge_at(world, at, &gas);
            }
        }
        _ => {}
    }
}

/// Добавить порог переизбытка урона (`AddOverkillThreshold`).
fn add_overkill_threshold(world: &mut World, entity: Entity) {
    let Some(mut destructible) = world.get::<Destructible>(entity).cloned() else {
        return;
    };
    destructible.thresholds = normalize(std::mem::take(&mut destructible.thresholds));
    if destructible.generate_overkill_threshold {
        let mut max_trigger = FixedPoint2::ZERO;
        let mut only_destruction = false;
        for threshold in &destructible.thresholds {
            let Trigger::DamageTrigger { damage } = threshold.trigger else {
                continue;
            };
            for behavior in &threshold.behaviors {
                if !behavior.has_act("Destruction") {
                    continue;
                }
                if threshold.behaviors.len() == 1 {
                    only_destruction = true;
                }
                max_trigger = max_trigger.max(damage);
            }
        }
        if !only_destruction && max_trigger != FixedPoint2::ZERO {
            let damage =
                FixedPoint2::from_f32(MINIMUM_OVERKILL).max(max_trigger.scale(OVERKILL_MULTIPLIER));
            destructible.thresholds.insert(
                0,
                DamageThreshold {
                    trigger: Trigger::DamageTrigger { damage },
                    behaviors: vec![Behavior::DoActsBehavior {
                        acts: vec!["Destruction".into()],
                    }],
                    triggered: false,
                    triggers_once: false,
                    old_triggered: false,
                },
            );
        }
    }
    world.entity_mut(entity).insert(destructible);
}

/// Урон изменился (`OnDamageChanged`): исполнить достигнутые пороги.
fn on_damage_changed(world: &mut World, entity: Entity, event: &mut DamageChanged) {
    let Some(damageable) = world.get::<Damageable>(entity).cloned() else {
        return;
    };
    let count = world
        .get::<Destructible>(entity)
        .map_or(0, |d| d.thresholds.len());
    if let Some(mut destructible) = world.get_mut::<Destructible>(entity) {
        destructible.is_broken = false;
    }
    for index in 0..count {
        let Some(mut threshold) = world
            .get::<Destructible>(entity)
            .and_then(|d| d.thresholds.get(index).cloned())
        else {
            return;
        };
        // `Triggered`.
        let fire = if threshold.triggered && threshold.triggers_once {
            false
        } else if threshold.old_triggered {
            threshold.old_triggered = reached(world, &threshold.trigger, &damageable);
            false
        } else if reached(world, &threshold.trigger, &damageable) {
            threshold.old_triggered = true;
            true
        } else {
            false
        };
        if fire {
            threshold.triggered = true;
        }
        let breaks = threshold.old_triggered
            && threshold
                .behaviors
                .iter()
                .any(|b| b.has_act("Breakage") || b.has_act("Destruction"));
        if let Some(mut destructible) = world.get_mut::<Destructible>(entity) {
            if let Some(slot) = destructible.thresholds.get_mut(index) {
                *slot = threshold.clone();
            }
            destructible.is_broken |= breaks;
        }
        if fire {
            world.raise(entity, &mut DamageThresholdReached { threshold: index });
            for behavior in &threshold.behaviors {
                if is_destroyed(world, entity) {
                    return;
                }
                execute(world, entity, behavior, event.origin);
            }
        }
        if is_destroyed(world, entity) || world.get::<Destructible>(entity).is_none() {
            return;
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Destructible>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Destructible, MapInit>(|world, entity, _| {
        add_overkill_threshold(world, entity)
    });
    world.subscribe::<Destructible, DamageChanged>(on_damage_changed);
}

#[cfg(test)]
mod tests;
