//! Дыхание: раз в 2 секунды вдох или выдох; вдох идёт в воздух лёгких и превращается в
//! реагенты раствора лёгких «Lung» (100 ед., без реакций), выдох возвращает воздух наружу
//! и очищает раствор. Раствор перерабатывает метаболизм лёгких (стадия `Respiration`,
//! модуль `metabolism`) по эффектам реагентов SS14: кислород насыщает (`Oxygenate`),
//! плазма и углекислый газ ранят, кислород в лёгких становится углекислым газом
//! (`ModifyLungGas`). Без насыщения — удушье. Образец — `RespiratorComponent`,
//! `RespiratorSystem`, `LungComponent`, `LungSystem`, эффекты `Oxygenate`, `ModifyLungGas`,
//! условие `BreathingCondition` SS14 @ 2f4a2aad.
//!
//! Нет пока: остальных эффектов дыхания (сон, опьянение, рвота, эмоции), вздоха при
//! удушье (`Gasp`, чат).

use std::sync::Arc;
use std::time::Duration;

use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::schedule::IntoScheduleConfigs;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::atmos::gas::{GAS_COUNT, gas_index};
use crate::atmos::{GasData, GasMixture};
use crate::body::BodyWorldExt;
use crate::damage::{DamageSpecifier, DamageableWorldExt};
use crate::entity_effects::{ConditionKind, EffectKind, EffectsWorldExt, number};
use crate::fixed_point::FixedPoint2;
use crate::mobs::MobStateWorldExt;
use crate::solution::ReagentId;
use crate::solution_container::{self as containers, SolutionComponent};

/// Объём вдоха (`Atmospherics.BreathVolume`), л.
pub const BREATH_VOLUME: f32 = 0.5;
/// Моли газа → количество реагента (`BreathMolesToReagentMultiplier`).
pub const BREATH_MOLES_TO_REAGENT: f32 = 1144.0;
/// Температура воздуха лёгких по умолчанию (`Atmospherics.NormalBodyTemperature`).
const LUNG_AIR_TEMPERATURE: f32 = 37.0;
/// Стадия метаболизма лёгких.
pub const RESPIRATION_STAGE: &str = "Respiration";
/// Раствор лёгких по умолчанию.
pub const LUNG_SOLUTION: &str = "Lung";
/// Вместимость раствора лёгких (`LungSystem.OnMapInit`).
const LUNG_SOLUTION_VOLUME: i32 = 100;

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
    /// Раствор лёгких, в который воздух превращается в реагенты (`SolutionName`).
    #[serde(default = "lung_solution")]
    pub solution_name: String,
    /// Значок удушья (`Alert`): у людей — нехватка кислорода, у вокса — азота.
    #[serde(default = "low_oxygen_alert")]
    pub alert: String,
}

fn low_oxygen_alert() -> String {
    "LowOxygen".into()
}

fn lung_solution() -> String {
    LUNG_SOLUTION.into()
}

impl Default for Lung {
    fn default() -> Self {
        Self {
            air: lung_air(),
            solution_name: lung_solution(),
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

/// Воздух лёгких → реагенты раствора лёгких (`GasToReagent`).
fn gas_to_reagent(world: &mut World, data: &GasData, lung: Entity) {
    let Some(component) = world.get::<Lung>(lung) else {
        return;
    };
    let air = component.air.clone();
    let name = component.solution_name.clone();
    let Some(soln) = containers::try_get_solution(world, lung, &name) else {
        return;
    };
    if let Some(mut solution) = world.get_mut::<SolutionComponent>(soln) {
        for gas in 0..GAS_COUNT {
            let moles = air.moles[gas];
            if moles <= 0.0 {
                continue;
            }
            let Some(reagent) = &data.reagents[gas] else {
                continue;
            };
            let amount = FixedPoint2::from_f32(moles * BREATH_MOLES_TO_REAGENT);
            solution
                .solution
                .add_reagent(ReagentId::new(reagent.clone()), amount);
        }
    }
    containers::update_chemicals(world, soln, true);
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
    if let Some(mut component) = world.get_mut::<Lung>(lung) {
        data.merge(&mut component.air, &gas);
    }
    gas_to_reagent(world, data, lung);
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
        let name = component.solution_name.clone();
        crate::barotrauma::with_containing_mixture(world, entity, true, |outside| {
            data.merge(outside, &air)
        });
        if let Some(soln) = containers::try_get_solution(world, lung, &name) {
            containers::remove_all_solution(world, soln);
        }
    }
}

/// Насыщение кислородом (`Oxygenate`): у дышащего тела.
#[derive(Debug)]
struct Oxygenate {
    factor: f32,
}

impl EffectKind for Oxygenate {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        update_saturation(world, target, scale * self.factor);
    }
}

/// Газ в воздухе лёгких: газ → доля (`ModifyLungGas`). На лёгких.
#[derive(Debug)]
struct ModifyLungGas {
    ratios: Vec<(usize, f32)>,
}

impl EffectKind for ModifyLungGas {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        let Some(mut lung) = world.get_mut::<Lung>(target) else {
            return;
        };
        for (gas, ratio) in &self.ratios {
            let mut quantity = ratio * scale / BREATH_MOLES_TO_REAGENT;
            if quantity < 0.0 {
                quantity = quantity.max(-lung.air.moles[*gas]);
            }
            lung.air.adjust_moles(*gas, quantity);
        }
    }
}

/// Дышит ли тело (`BreathingCondition`).
#[derive(Debug)]
struct BreathingCondition;

impl ConditionKind for BreathingCondition {
    fn check(&self, world: &mut World, target: Entity, _: Option<Entity>) -> bool {
        world.get::<Respirator>(target).is_some() && is_breathing(world, target)
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
    world.register_effect("Oxygenate", |node| {
        Some(Arc::new(Oxygenate {
            factor: number(node, "factor").unwrap_or(1.0),
        }))
    });
    world.register_effect("ModifyLungGas", |node| {
        let ratios = node
            .get("ratios")?
            .as_map()?
            .iter()
            .filter_map(|(gas, ratio)| {
                Some((gas_index(gas)?, ratio.as_str()?.trim().parse().ok()?))
            })
            .collect();
        Some(Arc::new(ModifyLungGas { ratios }))
    });
    world.register_condition("BreathingCondition", |_| Some(Arc::new(BreathingCondition)));
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
    // Раствор лёгких: 100 ед., без реакций (`LungSystem.OnMapInit`).
    world.subscribe::<Lung, MapInit>(|world, lung, _| {
        let Some(name) = world.get::<Lung>(lung).map(|l| l.solution_name.clone()) else {
            return;
        };
        let Some((soln, _)) = containers::ensure_solution(world, lung, &name) else {
            return;
        };
        if let Some(mut solution) = world.get_mut::<SolutionComponent>(soln) {
            solution.solution.max_volume = FixedPoint2::new(LUNG_SOLUTION_VOLUME);
            solution.solution.can_react = false;
        }
    });
    // Лёгкие перерабатывают вдох раньше нового вдоха (`UpdatesAfter MetabolizerSystem`).
    world.add_game_systems(Update, update.after(crate::metabolism::update));
}

#[cfg(test)]
mod tests;
