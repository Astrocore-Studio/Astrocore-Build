//! Реакции газов и пламя на клетках. Образец — `GasReactionPrototype`, реакции
//! `PlasmaFireReaction`, `TritiumFireReaction`, `FrezonCoolantReaction`,
//! `FrezonProductionReaction`, `AmmoniaOxygenReaction`, `N2ODecompositionReaction`,
//! `AtmosphereSystem.React` и `AtmosphereSystem.Hotspot` SS14 @ 2f4a2aad.

use astrocore_data::DataNode;
use astrocore_prototypes::Prototypes;
use bevy_ecs::resource::Resource;

use super::gas::*;
use super::grid::{GridAtmosphere, Hotspot};
use super::sim::add_active_tile;

/// Действие реакции (`IGasReactionEffect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionEffect {
    PlasmaFire,
    TritiumFire,
    FrezonCoolant,
    FrezonProduction,
    AmmoniaOxygen,
    N2ODecomposition,
}

/// Реакция (`GasReactionPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct GasReaction {
    pub id: String,
    pub priority: i32,
    pub minimum_requirements: [f32; GAS_COUNT],
    pub minimum_temperature: f32,
    pub maximum_temperature: f32,
    pub minimum_energy: f32,
    pub effects: Vec<ReactionEffect>,
}

/// Реакции по убыванию приоритета (`CacheGases`). Ресурс.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct GasReactions(pub Vec<GasReaction>);

impl GasReactions {
    pub fn from_prototypes(prototypes: &Prototypes) -> Self {
        let mut reactions: Vec<GasReaction> = prototypes
            .ids("gasReaction")
            .filter_map(|id| {
                let node = prototypes.resolved_node("gasReaction", id)?;
                let number = |key: &str| {
                    node.get(key)
                        .and_then(DataNode::as_str)
                        .and_then(|v| v.parse::<f32>().ok())
                };
                let mut minimum_requirements = [0.0; GAS_COUNT];
                if let Some(map) = node.get("minimumRequirements").and_then(DataNode::as_map) {
                    for (name, value) in map.iter() {
                        if let (Some(gas), Some(value)) =
                            (gas_index(name), value.as_str().and_then(|v| v.parse().ok()))
                        {
                            minimum_requirements[gas] = value;
                        }
                    }
                }
                let effects = node
                    .get("effects")
                    .and_then(DataNode::as_seq)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| match item.type_tag()? {
                                "PlasmaFireReaction" => Some(ReactionEffect::PlasmaFire),
                                "TritiumFireReaction" => Some(ReactionEffect::TritiumFire),
                                "FrezonCoolantReaction" => Some(ReactionEffect::FrezonCoolant),
                                "FrezonProductionReaction" => {
                                    Some(ReactionEffect::FrezonProduction)
                                }
                                "AmmoniaOxygenReaction" => Some(ReactionEffect::AmmoniaOxygen),
                                "N2ODecompositionReaction" => {
                                    Some(ReactionEffect::N2ODecomposition)
                                }
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(GasReaction {
                    id: id.to_owned(),
                    priority: number("priority").map_or(i32::MIN, |p| p as i32),
                    minimum_requirements,
                    minimum_temperature: number("minimumTemperature").unwrap_or(TCMB),
                    maximum_temperature: number("maximumTemperature").unwrap_or(f32::MAX),
                    minimum_energy: number("minimumEnergy").unwrap_or(0.0),
                    effects,
                })
            })
            .collect();
        reactions.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.id.cmp(&b.id)));
        Self(reactions)
    }
}

/// Итог реакции (`ReactionResult`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReactionResult {
    pub reacting: bool,
    pub stop: bool,
}

/// Реакция в клетке просит поджечь её (`HotspotExpose` из реакций огня).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Exposure {
    pub temperature: f32,
    pub volume: f32,
}

fn plasma_fire(gas: &GasData, mixture: &mut GasMixture, exposures: &mut Vec<Exposure>) -> bool {
    let mut energy_released = 0.0;
    let old_heat_capacity = gas.heat_capacity(mixture);
    let temperature = mixture.temperature();
    mixture.reaction_fire = 0.0;
    let temperature_scale = if temperature > PLASMA_UPPER_TEMPERATURE {
        1.0
    } else {
        (temperature - PLASMA_MINIMUM_BURN_TEMPERATURE)
            / (PLASMA_UPPER_TEMPERATURE - PLASMA_MINIMUM_BURN_TEMPERATURE)
    };
    if temperature_scale > 0.0 {
        let oxygen_burn_rate = OXYGEN_BURN_RATE_BASE - temperature_scale;
        let oxygen = mixture.moles[OXYGEN];
        let plasma = mixture.moles[PLASMA];
        let oxy_ratio = oxygen / plasma;
        let supersaturation = ((oxy_ratio - SUPER_SATURATION_ENDS)
            / (SUPER_SATURATION_THRESHOLD - SUPER_SATURATION_ENDS))
            .clamp(0.0, 1.0);
        let mut burn_rate = if oxygen > plasma * PLASMA_OXYGEN_FULLBURN {
            plasma * temperature_scale / PLASMA_BURN_RATE_DELTA
        } else {
            temperature_scale * (oxygen / PLASMA_OXYGEN_FULLBURN) / PLASMA_BURN_RATE_DELTA
        };
        if burn_rate > MINIMUM_HEAT_CAPACITY {
            burn_rate = burn_rate.min(plasma.min(oxygen / oxygen_burn_rate));
            mixture.set_moles(PLASMA, plasma - burn_rate);
            mixture.set_moles(OXYGEN, oxygen - burn_rate * oxygen_burn_rate);
            mixture.adjust_moles(TRITIUM, burn_rate * supersaturation);
            mixture.adjust_moles(CARBON_DIOXIDE, burn_rate * (1.0 - supersaturation));
            energy_released += FIRE_PLASMA_ENERGY_RELEASED * burn_rate;
            energy_released /= HEAT_SCALE;
            mixture.reaction_fire += burn_rate * (1.0 + oxygen_burn_rate);
        }
    }
    if energy_released > 0.0 {
        let new_heat_capacity = gas.heat_capacity(mixture);
        if new_heat_capacity > MINIMUM_HEAT_CAPACITY {
            mixture.set_temperature(
                (temperature * old_heat_capacity + energy_released) / new_heat_capacity,
            );
        }
    }
    if mixture.temperature() > FIRE_MINIMUM_TEMPERATURE_TO_EXIST {
        exposures.push(Exposure {
            temperature: mixture.temperature(),
            volume: mixture.volume,
        });
    }
    mixture.reaction_fire != 0.0
}

fn tritium_fire(gas: &GasData, mixture: &mut GasMixture, exposures: &mut Vec<Exposure>) -> bool {
    let mut energy_released = 0.0;
    let old_heat_capacity = gas.heat_capacity(mixture);
    let temperature = mixture.temperature();
    mixture.reaction_fire = 0.0;
    let initial_trit = mixture.moles[TRITIUM];
    let burned_fuel;
    if mixture.moles[OXYGEN] < initial_trit
        || MINIMUM_TRITIUM_OXYBURN_ENERGY > temperature * old_heat_capacity * HEAT_SCALE
    {
        burned_fuel = (mixture.moles[OXYGEN] / TRITIUM_BURN_OXY_FACTOR).min(initial_trit);
        mixture.adjust_moles(TRITIUM, -burned_fuel);
        mixture.adjust_moles(OXYGEN, -burned_fuel / TRITIUM_BURN_FUEL_RATIO);
    } else {
        burned_fuel = initial_trit.min(mixture.moles[OXYGEN] / TRITIUM_BURN_FUEL_RATIO)
            / TRITIUM_BURN_TRIT_FACTOR;
        mixture.adjust_moles(TRITIUM, -burned_fuel);
        mixture.adjust_moles(OXYGEN, -burned_fuel / TRITIUM_BURN_FUEL_RATIO);
        energy_released +=
            FIRE_HYDROGEN_ENERGY_RELEASED * burned_fuel * (TRITIUM_BURN_TRIT_FACTOR - 1.0);
    }
    if burned_fuel > 0.0 {
        energy_released += FIRE_HYDROGEN_ENERGY_RELEASED * burned_fuel;
        mixture.adjust_moles(WATER_VAPOR, burned_fuel);
        mixture.reaction_fire += burned_fuel;
    }
    energy_released /= HEAT_SCALE;
    if energy_released > 0.0 {
        let new_heat_capacity = gas.heat_capacity(mixture);
        if new_heat_capacity > MINIMUM_HEAT_CAPACITY {
            mixture.set_temperature(
                (temperature * old_heat_capacity + energy_released) / new_heat_capacity,
            );
        }
    }
    if mixture.temperature() > FIRE_MINIMUM_TEMPERATURE_TO_EXIST {
        exposures.push(Exposure {
            temperature: mixture.temperature(),
            volume: mixture.volume,
        });
    }
    mixture.reaction_fire != 0.0
}

fn frezon_coolant(gas: &GasData, mixture: &mut GasMixture) -> bool {
    let old_heat_capacity = gas.heat_capacity(mixture);
    let temperature = mixture.temperature();
    let mut energy_modifier = 1.0;
    let mut scale = (temperature - FREZON_COOL_LOWER_TEMPERATURE)
        / (FREZON_COOL_MID_TEMPERATURE - FREZON_COOL_LOWER_TEMPERATURE);
    if scale > 1.0 {
        energy_modifier = scale.min(FREZON_COOL_MAXIMUM_ENERGY_MODIFIER);
        scale = 1.0;
    }
    if scale <= 0.0 {
        return false;
    }
    let nitrogen = mixture.moles[NITROGEN];
    let frezon = mixture.moles[FREZON];
    let burn_rate = frezon * scale / FREZON_COOL_RATE_MODIFIER;
    let mut energy_released = 0.0;
    if burn_rate > MINIMUM_HEAT_CAPACITY {
        let nit = (burn_rate * FREZON_NITROGEN_COOL_RATIO).min(nitrogen);
        let fre = burn_rate.min(frezon);
        mixture.adjust_moles(NITROGEN, -nit);
        mixture.adjust_moles(FREZON, -fre);
        mixture.adjust_moles(NITROUS_OXIDE, nit + fre);
        energy_released = burn_rate * FREZON_COOL_ENERGY_RELEASED * energy_modifier;
    }
    energy_released /= HEAT_SCALE;
    if energy_released >= 0.0 {
        return false;
    }
    let new_heat_capacity = gas.heat_capacity(mixture);
    if new_heat_capacity > MINIMUM_HEAT_CAPACITY {
        mixture.set_temperature(
            (temperature * old_heat_capacity + energy_released) / new_heat_capacity,
        );
    }
    true
}

fn frezon_production(mixture: &mut GasMixture) -> bool {
    let n2 = mixture.moles[NITROGEN];
    let oxy = mixture.moles[OXYGEN];
    let trit = mixture.moles[TRITIUM];
    let efficiency = mixture.temperature() / FREZON_PRODUCTION_MAX_EFFICIENCY_TEMPERATURE;
    let loss = 1.0 - efficiency;
    let catalyst_limit = n2 * (FREZON_PRODUCTION_NITROGEN_RATIO / efficiency);
    let oxy_limit = oxy.min(catalyst_limit) / FREZON_PRODUCTION_TRIT_RATIO;
    let trit_burned = oxy_limit.min(trit);
    let oxy_burned = trit_burned * FREZON_PRODUCTION_TRIT_RATIO;
    let oxy_conversion = oxy_burned / FREZON_PRODUCTION_CONVERSION_RATE;
    let trit_conversion = trit_burned / FREZON_PRODUCTION_CONVERSION_RATE;
    let total = oxy_conversion + trit_conversion;
    mixture.adjust_moles(OXYGEN, -oxy_conversion);
    mixture.adjust_moles(TRITIUM, -trit_conversion);
    mixture.adjust_moles(FREZON, total * efficiency);
    mixture.adjust_moles(NITROGEN, total * loss);
    true
}

fn ammonia_oxygen(mixture: &mut GasMixture) -> bool {
    let ammonia = mixture.moles[AMMONIA];
    let oxygen = mixture.moles[OXYGEN];
    let total = mixture.total_moles();
    let rate = (ammonia / total).powi(2) * (oxygen / total).powi(2);
    let delta = ammonia / AMMONIA_OXYGEN_REACTION_RATE * 2.0 * rate;
    if delta <= 0.0 || ammonia - delta < 0.0 {
        return false;
    }
    mixture.adjust_moles(AMMONIA, -delta);
    mixture.adjust_moles(OXYGEN, -delta);
    mixture.adjust_moles(NITROUS_OXIDE, delta / 2.0);
    mixture.adjust_moles(WATER_VAPOR, delta * 1.5);
    true
}

fn n2o_decomposition(mixture: &mut GasMixture) -> bool {
    let n2o = mixture.moles[NITROUS_OXIDE];
    let burned = n2o / N2O_DECOMPOSITION_RATE;
    if burned <= 0.0 || n2o - burned < 0.0 {
        return false;
    }
    mixture.adjust_moles(NITROUS_OXIDE, -burned);
    mixture.adjust_moles(NITROGEN, burned);
    mixture.adjust_moles(OXYGEN, burned / 2.0);
    true
}

/// Реакции смеси (`React`). Поджоги клетки собираются в `exposures`.
pub fn react(
    gas: &GasData,
    reactions: &GasReactions,
    mixture: &mut GasMixture,
    exposures: &mut Vec<Exposure>,
) -> ReactionResult {
    let mut result = ReactionResult::default();
    let temperature = mixture.temperature();
    let energy = gas.thermal_energy(mixture);
    for reaction in &reactions.0 {
        if energy < reaction.minimum_energy
            || temperature < reaction.minimum_temperature
            || temperature > reaction.maximum_temperature
        {
            continue;
        }
        if (0..GAS_COUNT).any(|i| mixture.moles[i] < reaction.minimum_requirements[i]) {
            continue;
        }
        for effect in &reaction.effects {
            let reacted = match effect {
                ReactionEffect::PlasmaFire => plasma_fire(gas, mixture, exposures),
                ReactionEffect::TritiumFire => tritium_fire(gas, mixture, exposures),
                ReactionEffect::FrezonCoolant => frezon_coolant(gas, mixture),
                ReactionEffect::FrezonProduction => frezon_production(mixture),
                ReactionEffect::AmmoniaOxygen => ammonia_oxygen(mixture),
                ReactionEffect::N2ODecomposition => n2o_decomposition(mixture),
            };
            result.reacting |= reacted;
        }
        if result.stop {
            break;
        }
    }
    result
}

/// Поджечь клетку (`HotspotExpose`). `soh` — поддержать уже горящее пламя.
pub fn hotspot_expose(
    grid: &mut GridAtmosphere,
    gas: &GasData,
    tile: u32,
    temperature: f32,
    volume: f32,
    soh: bool,
) {
    let Some(air) = &grid.tile(tile).air else {
        return;
    };
    if !gas.is_oxidizer(air) {
        return;
    }
    let flammable = gas.is_fuel(air);
    let update_counter = grid.update_counter;
    let t = grid.tile_mut(tile);
    if t.hotspot.valid {
        if soh && flammable {
            t.hotspot.temperature = t.hotspot.temperature.max(temperature);
            t.hotspot.volume = t.hotspot.volume.max(volume);
        }
        return;
    }
    if temperature > PLASMA_MINIMUM_BURN_TEMPERATURE && flammable {
        t.hotspot = Hotspot {
            volume: volume * 25.0,
            temperature,
            skipped_first_process: t.current_cycle > update_counter,
            valid: true,
            state: 1,
            bypassing: false,
        };
        add_active_tile(grid, tile);
        grid.hotspot_tiles.insert(tile);
    }
}

/// Реакции воздуха клетки и поджог от них (`React(tile.Air, tile)` в `ProcessCell`).
pub fn react_tile(grid: &mut GridAtmosphere, gas: &GasData, reactions: &GasReactions, tile: u32) {
    let Some(mut air) = grid.tile_mut(tile).air.take() else {
        return;
    };
    let mut exposures = Vec::new();
    react(gas, reactions, &mut air, &mut exposures);
    grid.tile_mut(tile).air = Some(air);
    for exposure in exposures {
        hotspot_expose(
            grid,
            gas,
            tile,
            exposure.temperature,
            exposure.volume,
            false,
        );
    }
}

/// Пламя забирает газ клетки и растёт (`PerformHotspotExposure`). Возвращает пламя для
/// события `TileFire`.
pub fn perform_hotspot_exposure(
    grid: &mut GridAtmosphere,
    gas: &GasData,
    reactions: &GasReactions,
    tile: u32,
) -> Option<(f32, f32)> {
    let t = grid.tile_mut(tile);
    if !t.hotspot.valid {
        return None;
    }
    let air = t.air.as_mut()?;
    t.hotspot.bypassing = t.hotspot.skipped_first_process && t.hotspot.volume > air.volume * 0.95;
    if t.hotspot.bypassing {
        t.hotspot.volume = air.reaction_fire * FIRE_GROWTH_RATE;
        t.hotspot.temperature = air.temperature();
    } else {
        let mut affected = air.remove_volume(t.hotspot.volume);
        affected.set_temperature(t.hotspot.temperature);
        let mut exposures = Vec::new();
        react(gas, reactions, &mut affected, &mut exposures);
        t.hotspot.temperature = affected.temperature();
        t.hotspot.volume = affected.reaction_fire * FIRE_GROWTH_RATE;
        gas.merge(air, &affected);
    }
    Some((t.hotspot.temperature, t.hotspot.volume))
}
