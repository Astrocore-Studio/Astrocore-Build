//! Дыхание: раз в 2 секунды вдох или выдох, лёгкие превращают воздух в реагенты и раз в
//! секунду перерабатывают их по эффектам стадии `Respiration` из прототипов реагентов SS14:
//! кислород насыщает, плазма и углекислый газ ранят, кислород в лёгких становится
//! углекислым газом. Без насыщения — удушье. Образец — `RespiratorComponent`,
//! `RespiratorSystem`, `LungComponent`, `LungSystem`, `MetabolizerComponent`,
//! `MetabolizerSystem` (стадия `Respiration`), эффекты `Oxygenate`, `HealthChange`,
//! `ModifyLungGas` и условие `MetabolizerTypeCondition` SS14 @ 2f4a2aad.
//!
//! Растворов пока нет (ADR-0009): реагенты лёгких — список «реагент → количество». Нет
//! пока: остальных эффектов дыхания (сон, опьянение, рвота, эмоции), вздоха при удушье
//! (`Gasp`, чат) и значков тревоги.

use std::collections::{BTreeMap, HashMap};
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
use crate::atmos::gas::{GAS_COUNT, gas_index};
use crate::atmos::{GasData, GasMixture};
use crate::body::{BodyWorldExt, Organ};
use crate::damage::{DamageSpecifier, Damageable, DamageableWorldExt};
use crate::fixed_point::FixedPoint2;
use crate::mobs::MobStateWorldExt;

/// Объём вдоха (`Atmospherics.BreathVolume`), л.
pub const BREATH_VOLUME: f32 = 0.5;
/// Моли газа → количество реагента (`BreathMolesToReagentMultiplier`).
pub const BREATH_MOLES_TO_REAGENT: f32 = 1144.0;
/// Температура воздуха лёгких по умолчанию (`Atmospherics.NormalBodyTemperature`).
const LUNG_AIR_TEMPERATURE: f32 = 37.0;
/// Стадия метаболизма лёгких.
pub const RESPIRATION_STAGE: &str = "Respiration";
/// Сколько реагента без эффектов дыхания уходит за раз (`TransferRate`).
const TRANSFER_RATE: f32 = 0.25;

fn breath_volume() -> f32 {
    BREATH_VOLUME
}
fn one() -> f32 {
    1.0
}
fn two_seconds() -> f32 {
    2.0
}
fn five() -> f32 {
    5.0
}
fn minus_two() -> f32 {
    -2.0
}
fn three() -> i32 {
    3
}

/// Вдох или выдох следующим (`RespiratorStatus`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RespiratorStatus {
    #[default]
    Inhaling,
    Exhaling,
}

/// Дышит (`RespiratorComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Respirator {
    #[serde(default = "breath_volume")]
    pub breath_volume: f32,
    #[serde(default = "one")]
    pub ratio: f32,
    /// Между вдохом и выдохом, с.
    #[serde(default = "two_seconds")]
    pub update_interval: f32,
    #[serde(default = "one")]
    pub update_interval_multiplier: f32,
    /// Насыщение: сколько секунд до удушья.
    #[serde(default = "five")]
    pub saturation: f32,
    #[serde(default)]
    pub suffocation_threshold: f32,
    #[serde(default = "five")]
    pub max_saturation: f32,
    #[serde(default = "minus_two")]
    pub min_saturation: f32,
    pub damage: DamageSpecifier,
    pub damage_recovery: DamageSpecifier,
    #[serde(skip)]
    pub suffocation_cycles: i32,
    #[serde(skip, default = "three")]
    pub suffocation_cycle_threshold: i32,
    #[serde(skip)]
    pub status: RespiratorStatus,
    #[serde(skip)]
    pub next_update: Duration,
}

impl Respirator {
    /// Интервал с учётом обмена веществ (`AdjustedUpdateInterval`).
    pub fn adjusted_interval(&self) -> Duration {
        Duration::from_secs_f32(self.update_interval * self.update_interval_multiplier)
    }
}

fn lung_air() -> GasMixture {
    GasMixture::new(6.0).with_temperature(LUNG_AIR_TEMPERATURE)
}

/// Лёгкие (`LungComponent`): воздух вдоха и реагенты из него.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lung {
    #[serde(skip, default = "lung_air")]
    pub air: GasMixture,
    /// Реагенты лёгких (раствор `Lung` в SS14, ADR-0009).
    #[serde(skip)]
    pub reagents: BTreeMap<String, FixedPoint2>,
    /// Значок удушья (`Alert`): у людей — нехватка кислорода, у вокса — азота.
    #[serde(default = "low_oxygen_alert")]
    pub alert: String,
}

fn low_oxygen_alert() -> String {
    "LowOxygen".into()
}

impl Default for Lung {
    fn default() -> Self {
        Self {
            air: lung_air(),
            reagents: BTreeMap::new(),
            alert: low_oxygen_alert(),
        }
    }
}

/// Значок удушья лёгких тела (`OnSuffocation`, `OnStopSuffocating`).
fn suffocation_alert(world: &mut World, body: Entity, show: bool) {
    let alerts: Vec<String> = lungs_of(world, body)
        .into_iter()
        .filter_map(|lung| world.get::<Lung>(lung).map(|l| l.alert.clone()))
        .collect();
    for alert in alerts {
        if show {
            crate::alerts::show_alert(world, body, &alert, None);
        } else {
            crate::alerts::clear_alert(world, body, &alert);
        }
    }
}

/// Перерабатывает реагенты (`MetabolizerComponent`). Здесь — только стадия дыхания.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metabolizer {
    #[serde(default = "one")]
    pub update_interval: f32,
    #[serde(default = "one")]
    pub update_interval_multiplier: f32,
    #[serde(default)]
    pub metabolizer_types: Option<Vec<String>>,
    #[serde(rename = "maxReagents", default = "three")]
    pub max_reagents_processable: i32,
    #[serde(default)]
    pub stages: Vec<String>,
    #[serde(skip)]
    pub next_update: Duration,
}

/// Условие эффекта (`EntityCondition`).
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// Тип метаболизма органа (`MetabolizerTypeCondition`).
    MetabolizerType { types: Vec<String>, inverted: bool },
    /// Не перенесено: эффект с ним не применяется.
    Unsupported,
}

/// Что делает эффект дыхания.
#[derive(Debug, Clone, PartialEq)]
pub enum EffectKind {
    /// Насыщение кислородом (`Oxygenate`).
    Oxygenate { factor: f32 },
    /// Урон или лечение (`HealthChange`).
    HealthChange {
        damage: DamageSpecifier,
        ignore_resistances: bool,
    },
    /// Газ в лёгких: газ → доля (`ModifyLungGas`).
    ModifyLungGas { ratios: Vec<(usize, f32)> },
}

/// Эффект стадии дыхания (`EntityEffect`).
#[derive(Debug, Clone, PartialEq)]
pub struct Effect {
    pub kind: EffectKind,
    pub conditions: Vec<Condition>,
    pub min_scale: f32,
    pub probability: f32,
}

fn number(node: &DataNode, key: &str) -> Option<f32> {
    node.get(key)?.as_str()?.parse().ok()
}

fn flag(node: &DataNode, key: &str) -> Option<bool> {
    let text = node.get(key)?.as_str()?;
    Some(text.eq_ignore_ascii_case("true"))
}

fn tag(node: &DataNode) -> &str {
    node.tag
        .as_deref()
        .and_then(|t| t.strip_prefix("type:"))
        .unwrap_or("")
}

fn parse_condition(node: &DataNode) -> Condition {
    match tag(node) {
        "MetabolizerTypeCondition" => Condition::MetabolizerType {
            types: node
                .get("type")
                .and_then(DataNode::as_seq)
                .map(|s| {
                    s.iter()
                        .filter_map(|t| t.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            inverted: flag(node, "inverted").unwrap_or(false),
        },
        _ => Condition::Unsupported,
    }
}

/// Эффект из данных; `None` — эффект не перенесён.
pub fn parse_effect(node: &DataNode) -> Option<Effect> {
    let kind = match tag(node) {
        "Oxygenate" => EffectKind::Oxygenate {
            factor: number(node, "factor").unwrap_or(1.0),
        },
        "HealthChange" => EffectKind::HealthChange {
            damage: astrocore_data::from_node(node.get("damage")?).ok()?,
            ignore_resistances: flag(node, "ignoreResistances").unwrap_or(true),
        },
        "ModifyLungGas" => EffectKind::ModifyLungGas {
            ratios: node
                .get("ratios")?
                .as_map()?
                .iter()
                .filter_map(|(gas, ratio)| {
                    Some((gas_index(gas)?, ratio.as_str()?.parse::<f32>().ok()?))
                })
                .collect(),
        },
        _ => return None,
    };
    Some(Effect {
        kind,
        conditions: node
            .get("conditions")
            .and_then(DataNode::as_seq)
            .map(|s| s.iter().map(parse_condition).collect())
            .unwrap_or_default(),
        min_scale: number(node, "minScale").unwrap_or(0.0),
        probability: number(node, "probability").unwrap_or(1.0),
    })
}

/// Эффекты стадии дыхания реагентов: `None` — у реагента нет этой стадии.
#[derive(Resource, Debug, Default)]
pub struct RespirationEffects(HashMap<String, Option<Vec<Effect>>>);

/// Эффекты дыхания реагента из прототипа (`ReagentPrototype.Metabolisms`).
pub fn reagent_effects(world: &mut World, reagent: &str) -> Option<Vec<Effect>> {
    if let Some(cached) = world
        .get_resource::<RespirationEffects>()
        .and_then(|c| c.0.get(reagent))
    {
        return cached.clone();
    }
    let effects = world.get_resource::<Prototypes>().and_then(|p| {
        let stage = p
            .resolved_node("reagent", reagent)?
            .get("metabolisms")?
            .get(RESPIRATION_STAGE)?;
        Some(
            stage
                .get("effects")
                .and_then(DataNode::as_seq)
                .map(|s| s.iter().filter_map(parse_effect).collect())
                .unwrap_or_default(),
        )
    });
    world
        .get_resource_or_insert_with(RespirationEffects::default)
        .0
        .insert(reagent.to_owned(), effects.clone());
    effects
}

/// Откуда вдыхать (`InhaleLocationEvent`): баллон кладёт сюда себя.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InhaleLocation {
    pub tank: Option<Entity>,
}

/// Удушье началось или продолжается (`SuffocationEvent`) и закончилось
/// (`StopSuffocatingEvent`). На теле.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Suffocation;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StopSuffocating;

/// Дыхание из баллона: модуль баллонов ставит свои функции — взять объём газа из баллона
/// (`RemoveAirVolume`) и вернуть невдохнутое (`Merge`).
#[derive(Resource, Clone, Copy)]
pub struct TankBreathing {
    pub remove_volume: fn(&mut World, Entity, f32) -> Option<GasMixture>,
    pub give_back: fn(&mut World, Entity, &GasMixture),
}

fn now(world: &World) -> Duration {
    world.resource::<GameTime>().cur_time()
}

/// Лёгкие тела.
pub fn lungs_of(world: &World, body: Entity) -> Vec<Entity> {
    world
        .organs(body)
        .into_iter()
        .filter(|organ| world.get::<Lung>(*organ).is_some())
        .collect()
}

/// Изменить насыщение (`UpdateSaturation`).
pub fn update_saturation(world: &mut World, entity: Entity, amount: f32) {
    if let Some(mut respirator) = world.get_mut::<Respirator>(entity) {
        respirator.saturation = (respirator.saturation + amount)
            .clamp(respirator.min_saturation, respirator.max_saturation);
    }
}

/// Дышит ли (`IsBreathing`): насыщение выше порога и тело в сознании.
pub fn is_breathing(world: &World, entity: Entity) -> bool {
    if world.is_incapacitated(entity) {
        return false;
    }
    world
        .get::<Respirator>(entity)
        .is_some_and(|r| r.saturation > r.suffocation_threshold)
}

/// Воздух лёгких → реагенты (`GasToReagent`).
fn gas_to_reagent(data: &GasData, air: &GasMixture, reagents: &mut BTreeMap<String, FixedPoint2>) {
    for gas in 0..GAS_COUNT {
        let moles = air.moles[gas];
        if moles <= 0.0 {
            continue;
        }
        let Some(reagent) = &data.reagents[gas] else {
            continue;
        };
        let amount = FixedPoint2::from_f32(moles * BREATH_MOLES_TO_REAGENT);
        let entry = reagents.entry(reagent.clone()).or_default();
        *entry += amount;
    }
}

/// Вдох (`Inhale`): объём вдоха из воздуха вокруг или баллона в лёгкие.
pub fn inhale(world: &mut World, data: &GasData, entity: Entity) {
    let Some(volume) = world.get::<Respirator>(entity).map(|r| r.breath_volume) else {
        return;
    };
    let mut location = InhaleLocation::default();
    world.raise(entity, &mut location);
    let gas = match location.tank {
        Some(tank) => {
            let Some(breathing) = world.get_resource::<TankBreathing>().copied() else {
                return;
            };
            (breathing.remove_volume)(world, tank, volume)
        }
        None => crate::barotrauma::with_containing_mixture(world, entity, true, |air| {
            air.remove_volume(volume)
        }),
    };
    let Some(gas) = gas else {
        return;
    };
    // Первые лёгкие забирают весь вдох (`OnGasInhaled`); без лёгких газ возвращается.
    let Some(lung) = lungs_of(world, entity).into_iter().next() else {
        give_back(world, data, entity, location.tank, &gas);
        return;
    };
    if let Some(mut lung) = world.get_mut::<Lung>(lung) {
        let lung = &mut *lung;
        data.merge(&mut lung.air, &gas);
        gas_to_reagent(data, &lung.air, &mut lung.reagents);
    }
}

/// Вернуть невдохнутый газ туда, откуда он взят (`Merge(ev.Gas, gas)`).
fn give_back(
    world: &mut World,
    data: &GasData,
    entity: Entity,
    tank: Option<Entity>,
    gas: &GasMixture,
) {
    match (tank, world.get_resource::<TankBreathing>().copied()) {
        (Some(tank), Some(breathing)) => (breathing.give_back)(world, tank, gas),
        _ => {
            crate::barotrauma::with_containing_mixture(world, entity, true, |air| {
                data.merge(air, gas)
            });
        }
    }
}

/// Выдох (`Exhale`): воздух лёгких наружу, реагенты очищаются.
pub fn exhale(world: &mut World, data: &GasData, entity: Entity) {
    for lung in lungs_of(world, entity) {
        let Some(mut component) = world.get_mut::<Lung>(lung) else {
            continue;
        };
        let air = component.air.clone();
        component.air.clear();
        component.reagents.clear();
        crate::barotrauma::with_containing_mixture(world, entity, true, |outside| {
            data.merge(outside, &air)
        });
    }
}

/// Условия эффекта на органе (`CanMetabolizeEffect`).
fn conditions_hold(world: &World, organ: Entity, conditions: &[Condition]) -> bool {
    let types = world
        .get::<Metabolizer>(organ)
        .and_then(|m| m.metabolizer_types.clone())
        .unwrap_or_default();
    conditions.iter().all(|condition| match condition {
        Condition::MetabolizerType {
            types: wanted,
            inverted,
        } => wanted.iter().any(|t| types.contains(t)) != *inverted,
        Condition::Unsupported => false,
    })
}

/// Применить эффект (`ApplyEffect`): насыщение и урон — телу, газ — лёгким.
fn apply_effect(world: &mut World, organ: Entity, body: Entity, effect: &EffectKind, scale: f32) {
    match effect {
        EffectKind::Oxygenate { factor } => {
            update_saturation(world, body, scale * factor);
        }
        EffectKind::HealthChange {
            damage,
            ignore_resistances,
        } => {
            if world.get::<Damageable>(body).is_some() {
                world.change_damage(body, &damage.scale(scale), *ignore_resistances, false, None);
            }
        }
        EffectKind::ModifyLungGas { ratios } => {
            let Some(mut lung) = world.get_mut::<Lung>(organ) else {
                return;
            };
            for (gas, ratio) in ratios {
                let mut quantity = ratio * scale / BREATH_MOLES_TO_REAGENT;
                if quantity < 0.0 {
                    quantity = quantity.max(-lung.air.moles[*gas]);
                }
                lung.air.adjust_moles(*gas, quantity);
            }
        }
    }
}

/// Переработка реагентов лёгких (`TryMetabolizeStage`, стадия `Respiration`,
/// `MetabolizeAll`).
fn metabolize_lung(world: &mut World, organ: Entity) {
    let Some(lung) = world.get::<Lung>(organ) else {
        return;
    };
    let mut list: Vec<(String, FixedPoint2)> = lung
        .reagents
        .iter()
        .map(|(id, quantity)| (id.clone(), *quantity))
        .collect();
    if list.is_empty() {
        return;
    }
    let max = world
        .get::<Metabolizer>(organ)
        .map_or(3, |m| m.max_reagents_processable);
    // Порядок случайный, как в SS14.
    {
        let mut random = crate::random::random(world);
        for i in (1..list.len()).rev() {
            let j = random.range(0, i as i32 + 1) as usize;
            list.swap(i, j);
        }
    }
    let body = world
        .get::<Organ>(organ)
        .and_then(|o| o.body)
        .unwrap_or(organ);
    let mut processed = 0;
    for (reagent, quantity) in list {
        let Some(effects) = reagent_effects(world, &reagent) else {
            // Без стадии дыхания реагент уходит в кровь; крови пока нет (ADR-0009).
            remove_reagent(world, organ, &reagent, FixedPoint2::from_f32(TRANSFER_RATE));
            continue;
        };
        if processed >= max {
            return;
        }
        let scale = quantity.float();
        for effect in &effects {
            if scale < effect.min_scale {
                continue;
            }
            if crate::random::random(world).next_f32() >= effect.probability {
                continue;
            }
            if !conditions_hold(world, organ, &effect.conditions) {
                continue;
            }
            apply_effect(world, organ, body, &effect.kind, scale);
        }
        if quantity > FixedPoint2::ZERO {
            remove_reagent(world, organ, &reagent, quantity);
            processed += 1;
        }
    }
}

fn remove_reagent(world: &mut World, organ: Entity, reagent: &str, amount: FixedPoint2) {
    let Some(mut lung) = world.get_mut::<Lung>(organ) else {
        return;
    };
    let Some(current) = lung.reagents.get(reagent).copied() else {
        return;
    };
    let left = current - amount;
    if left <= FixedPoint2::ZERO {
        lung.reagents.remove(reagent);
    } else {
        lung.reagents.insert(reagent.to_owned(), left);
    }
}

/// Урон от удушья (`TakeSuffocationDamage`).
fn take_suffocation_damage(world: &mut World, entity: Entity) {
    let Some(respirator) = world.get::<Respirator>(entity).cloned() else {
        return;
    };
    world.change_damage(entity, &respirator.damage, true, false, None);
    if respirator.suffocation_cycles >= respirator.suffocation_cycle_threshold {
        world.raise(entity, &mut Suffocation);
    }
}

/// Удушье прошло (`StopSuffocation`): удушье понемногу лечится.
fn stop_suffocation(world: &mut World, entity: Entity) {
    let Some(recovery) = world
        .get::<Respirator>(entity)
        .map(|r| r.damage_recovery.clone())
    else {
        return;
    };
    world.change_damage(entity, &recovery, false, true, None);
    world.raise(entity, &mut StopSuffocating);
}

fn update(world: &mut World) {
    if !world.contains_resource::<GasData>() {
        crate::atmos::gas_data(world);
    }
    let time = now(world);

    // Лёгкие перерабатывают вдох раньше нового вдоха (`UpdatesAfter MetabolizerSystem`).
    let organs: Vec<Entity> = world
        .query::<(Entity, &Metabolizer, &Lung)>()
        .iter(world)
        .filter(|(_, m, _)| {
            time >= m.next_update && m.stages.iter().any(|s| s == RESPIRATION_STAGE)
        })
        .map(|(e, _, _)| e)
        .collect();
    for organ in organs {
        if let Some(mut metabolizer) = world.get_mut::<Metabolizer>(organ) {
            let interval = Duration::from_secs_f32(
                metabolizer.update_interval * metabolizer.update_interval_multiplier,
            );
            metabolizer.next_update += interval;
        }
        metabolize_lung(world, organ);
    }

    let due: Vec<Entity> = world
        .query::<(Entity, &Respirator)>()
        .iter(world)
        .filter(|(_, r)| time >= r.next_update)
        .map(|(e, _)| e)
        .collect();
    if due.is_empty() {
        return;
    }
    world.resource_scope::<GasData, _>(|world, data| {
        for entity in due {
            let Some(mut respirator) = world.get_mut::<Respirator>(entity) else {
                continue;
            };
            let interval = respirator.adjusted_interval();
            respirator.next_update += interval;
            let seconds = respirator.update_interval;
            if world.is_mob_dead(entity) {
                continue;
            }
            update_saturation(world, entity, -seconds);
            // В критическом состоянии не дышат.
            if !world.is_incapacitated(entity) {
                let status = world.get::<Respirator>(entity).map(|r| r.status);
                match status {
                    Some(RespiratorStatus::Inhaling) => {
                        inhale(world, &data, entity);
                        set_status(world, entity, RespiratorStatus::Exhaling);
                    }
                    Some(RespiratorStatus::Exhaling) => {
                        exhale(world, &data, entity);
                        set_status(world, entity, RespiratorStatus::Inhaling);
                    }
                    None => {}
                }
            }
            let Some(respirator) = world.get::<Respirator>(entity) else {
                continue;
            };
            if respirator.saturation < respirator.suffocation_threshold {
                take_suffocation_damage(world, entity);
                if let Some(mut respirator) = world.get_mut::<Respirator>(entity) {
                    respirator.suffocation_cycles += 1;
                }
                continue;
            }
            stop_suffocation(world, entity);
            if let Some(mut respirator) = world.get_mut::<Respirator>(entity) {
                respirator.suffocation_cycles = 0;
            }
        }
    });
}

fn set_status(world: &mut World, entity: Entity, status: RespiratorStatus) {
    if let Some(mut respirator) = world.get_mut::<Respirator>(entity) {
        respirator.status = status;
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Respirator>();
    world.register_data_component::<Lung>();
    world.register_data_component::<Metabolizer>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Respirator, Suffocation>(|world, entity, _| {
        suffocation_alert(world, entity, true);
    });
    world.subscribe::<Respirator, StopSuffocating>(|world, entity, _| {
        suffocation_alert(world, entity, false);
    });
    world.subscribe::<Respirator, MapInit>(|world, entity, _| {
        let time = now(world);
        if let Some(mut respirator) = world.get_mut::<Respirator>(entity) {
            respirator.next_update = time + respirator.adjusted_interval();
        }
    });
    world.subscribe::<Metabolizer, MapInit>(|world, entity, _| {
        let time = now(world);
        if let Some(mut metabolizer) = world.get_mut::<Metabolizer>(entity) {
            metabolizer.next_update = time
                + Duration::from_secs_f32(
                    metabolizer.update_interval * metabolizer.update_interval_multiplier,
                );
        }
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
