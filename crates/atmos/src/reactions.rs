//! Газовые реакции (`Resources/Prototypes/Atmospherics/reactions.yml` и `Content.Server/Atmos/Reactions`).
//!
//! Формулы перенесены дословно; энергия делится на `HEAT_SCALE`, как в `React(..., heatScale)`.

use crate::gas::*;

/// Температура для `PlasmaFireReaction` и `TritiumFireReaction` (`FireMinimumTemperatureToExist`).
pub const PLASMA_MINIMUM_BURN_TEMPERATURE: f32 = T0C + 100.0;
pub const PLASMA_UPPER_TEMPERATURE: f32 = T0C + 1370.0;
pub const PLASMA_OXYGEN_FULLBURN: f32 = 10.0;
pub const PLASMA_BURN_RATE_DELTA: f32 = 9.0;
pub const OXYGEN_BURN_RATE_BASE: f32 = 1.4;
pub const SUPER_SATURATION_THRESHOLD: f32 = 96.0;
pub const SUPER_SATURATION_ENDS: f32 = SUPER_SATURATION_THRESHOLD / 3.0;
pub const MINIMUM_TRITIUM_OXYBURN_ENERGY: f32 = 143_000.0;
pub const TRITIUM_BURN_OXY_FACTOR: f32 = 100.0;
pub const TRITIUM_BURN_TRIT_FACTOR: f32 = 10.0;
pub const TRITIUM_BURN_FUEL_RATIO: f32 = 2.0;
pub const FREZON_COOL_LOWER_TEMPERATURE: f32 = 23.15;
pub const FREZON_COOL_MID_TEMPERATURE: f32 = 373.15;
pub const FREZON_COOL_MAXIMUM_ENERGY_MODIFIER: f32 = 10.0;
pub const FREZON_NITROGEN_COOL_RATIO: f32 = 5.0;
pub const FREZON_COOL_ENERGY_RELEASED: f32 = -600_000.0;
pub const FREZON_COOL_RATE_MODIFIER: f32 = 20.0;
pub const FREZON_PRODUCTION_MAX_EFFICIENCY_TEMPERATURE: f32 = 73.15;
pub const FREZON_PRODUCTION_NITROGEN_RATIO: f32 = 10.0;
pub const FREZON_PRODUCTION_TRIT_RATIO: f32 = 50.0;
pub const FREZON_PRODUCTION_CONVERSION_RATE: f32 = 50.0;
pub const N2O_DECOMPOSITION_RATE: f32 = 2.0;
pub const AMMONIA_OXYGEN_REACTION_RATE: f32 = 10.0;

/// Какая реакция (порядок `GasReactionPrototype` по убыванию приоритета).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    FrezonProduction,
    AmmoniaOxygen,
    FrezonCoolant,
    N2oDecomposition,
    TritiumFire,
    PlasmaFire,
}

/// Описание реакции: `priority`, температурные границы и минимальные количества газов.
pub struct ReactionSpec {
    pub id: Reaction,
    pub priority: i32,
    pub min_temperature: f32,
    pub max_temperature: f32,
    pub min_requirements: &'static [(Gas, f32)],
}

/// Реакции в порядке выполнения (`Array.Sort(b.Priority.CompareTo(a.Priority))`, устойчиво).
pub const REACTIONS: [ReactionSpec; 6] = [
    ReactionSpec {
        id: Reaction::FrezonProduction,
        priority: 2,
        min_temperature: 0.0,
        max_temperature: 73.15,
        min_requirements: &[
            (Gas::Oxygen, 0.01),
            (Gas::Nitrogen, 0.01),
            (Gas::Tritium, 0.01),
        ],
    },
    ReactionSpec {
        id: Reaction::AmmoniaOxygen,
        priority: 2,
        min_temperature: 323.149,
        max_temperature: f32::INFINITY,
        min_requirements: &[(Gas::Oxygen, 0.01), (Gas::Ammonia, 0.01)],
    },
    ReactionSpec {
        id: Reaction::FrezonCoolant,
        priority: 1,
        min_temperature: 23.15,
        max_temperature: f32::INFINITY,
        min_requirements: &[(Gas::Nitrogen, 0.01), (Gas::Frezon, 0.01)],
    },
    ReactionSpec {
        id: Reaction::N2oDecomposition,
        priority: 0,
        min_temperature: 850.0,
        max_temperature: f32::INFINITY,
        min_requirements: &[(Gas::NitrousOxide, 0.01)],
    },
    ReactionSpec {
        id: Reaction::TritiumFire,
        priority: -1,
        min_temperature: 373.149,
        max_temperature: f32::INFINITY,
        min_requirements: &[(Gas::Oxygen, 0.01), (Gas::Tritium, 0.01)],
    },
    ReactionSpec {
        id: Reaction::PlasmaFire,
        priority: -2,
        min_temperature: 373.149,
        max_temperature: f32::INFINITY,
        min_requirements: &[(Gas::Oxygen, 0.01), (Gas::Plasma, 0.01)],
    },
];

/// Итог `AtmosphereSystem.React`: прошла ли хоть одна реакция и нужно ли зажечь клетку.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ReactOutcome {
    pub reacting: bool,
    /// `HotspotExpose(location, temperature, volume)`, если реакция горения идёт в клетке.
    pub expose: Option<(f32, f32)>,
}

/// Выполнить все подходящие реакции над смесью. `in_tile` — смесь принадлежит клетке
/// (тогда реакции горения зажигают `Hotspot`).
pub fn react(mix: &mut GasMixture, in_tile: bool) -> ReactOutcome {
    let mut out = ReactOutcome::default();
    if mix.immutable {
        return out;
    }
    let temperature = mix.temperature;
    for spec in &REACTIONS {
        if temperature < spec.min_temperature || temperature > spec.max_temperature {
            continue;
        }
        if spec
            .min_requirements
            .iter()
            .any(|&(g, req)| mix.moles(g) < req)
        {
            continue;
        }
        let reacting = match spec.id {
            Reaction::PlasmaFire => plasma_fire(mix, in_tile, &mut out),
            Reaction::TritiumFire => tritium_fire(mix, in_tile, &mut out),
            Reaction::FrezonCoolant => frezon_coolant(mix),
            Reaction::FrezonProduction => frezon_production(mix),
            Reaction::AmmoniaOxygen => ammonia_oxygen(mix),
            Reaction::N2oDecomposition => n2o_decomposition(mix),
        };
        out.reacting |= reacting;
    }
    out
}

fn apply_energy(mix: &mut GasMixture, old_temperature: f32, old_capacity: f32, energy: f32) {
    let new_capacity = mix.heat_capacity();
    if new_capacity > MINIMUM_HEAT_CAPACITY {
        mix.set_temperature((old_temperature * old_capacity + energy) / new_capacity);
    }
}

fn plasma_fire(mix: &mut GasMixture, in_tile: bool, out: &mut ReactOutcome) -> bool {
    let mut energy = 0.0;
    let old_capacity = mix.heat_capacity();
    let temperature = mix.temperature;
    mix.fire = 0.0;
    let temperature_scale = if temperature > PLASMA_UPPER_TEMPERATURE {
        1.0
    } else {
        (temperature - PLASMA_MINIMUM_BURN_TEMPERATURE)
            / (PLASMA_UPPER_TEMPERATURE - PLASMA_MINIMUM_BURN_TEMPERATURE)
    };
    if temperature_scale > 0.0 {
        let oxygen_burn_rate = OXYGEN_BURN_RATE_BASE - temperature_scale;
        let oxygen = mix.moles(Gas::Oxygen);
        let plasma = mix.moles(Gas::Plasma);
        let oxy_ratio = oxygen / plasma;
        let supersaturation = ((oxy_ratio - SUPER_SATURATION_ENDS)
            / (SUPER_SATURATION_THRESHOLD - SUPER_SATURATION_ENDS))
            .clamp(0.0, 1.0);
        let mut plasma_burn_rate = if oxygen > plasma * PLASMA_OXYGEN_FULLBURN {
            plasma * temperature_scale / PLASMA_BURN_RATE_DELTA
        } else {
            temperature_scale * (oxygen / PLASMA_OXYGEN_FULLBURN) / PLASMA_BURN_RATE_DELTA
        };
        if plasma_burn_rate > MINIMUM_HEAT_CAPACITY {
            plasma_burn_rate = plasma_burn_rate.min(plasma.min(oxygen / oxygen_burn_rate));
            mix.set_moles(Gas::Plasma, plasma - plasma_burn_rate);
            mix.set_moles(Gas::Oxygen, oxygen - plasma_burn_rate * oxygen_burn_rate);
            mix.adjust_moles(Gas::Tritium, plasma_burn_rate * supersaturation);
            mix.adjust_moles(
                Gas::CarbonDioxide,
                plasma_burn_rate * (1.0 - supersaturation),
            );
            energy += FIRE_PLASMA_ENERGY_RELEASED * plasma_burn_rate;
            energy /= HEAT_SCALE;
            mix.fire += plasma_burn_rate * (1.0 + oxygen_burn_rate);
        }
    }
    if energy > 0.0 {
        apply_energy(mix, temperature, old_capacity, energy);
    }
    if in_tile && mix.temperature > FIRE_MINIMUM_TEMPERATURE_TO_EXIST {
        out.expose = Some((mix.temperature, mix.volume));
    }
    mix.fire != 0.0
}

fn tritium_fire(mix: &mut GasMixture, in_tile: bool, out: &mut ReactOutcome) -> bool {
    let mut energy = 0.0;
    let old_capacity = mix.heat_capacity();
    let temperature = mix.temperature;
    mix.fire = 0.0;
    let initial_trit = mix.moles(Gas::Tritium);
    let burned = if mix.moles(Gas::Oxygen) < initial_trit
        || MINIMUM_TRITIUM_OXYBURN_ENERGY > temperature * old_capacity * HEAT_SCALE
    {
        let mut b = mix.moles(Gas::Oxygen) / TRITIUM_BURN_OXY_FACTOR;
        if b > initial_trit {
            b = initial_trit;
        }
        mix.adjust_moles(Gas::Tritium, -b);
        mix.adjust_moles(Gas::Oxygen, -b / TRITIUM_BURN_FUEL_RATIO);
        b
    } else {
        let b = initial_trit.min(mix.moles(Gas::Oxygen) / TRITIUM_BURN_FUEL_RATIO)
            / TRITIUM_BURN_TRIT_FACTOR;
        mix.adjust_moles(Gas::Tritium, -b);
        mix.adjust_moles(Gas::Oxygen, -b / TRITIUM_BURN_FUEL_RATIO);
        energy += FIRE_HYDROGEN_ENERGY_RELEASED * b * (TRITIUM_BURN_TRIT_FACTOR - 1.0);
        b
    };
    if burned > 0.0 {
        energy += FIRE_HYDROGEN_ENERGY_RELEASED * burned;
        mix.adjust_moles(Gas::WaterVapor, burned);
        mix.fire += burned;
    }
    energy /= HEAT_SCALE;
    if energy > 0.0 {
        apply_energy(mix, temperature, old_capacity, energy);
    }
    if in_tile && mix.temperature > FIRE_MINIMUM_TEMPERATURE_TO_EXIST {
        out.expose = Some((mix.temperature, mix.volume));
    }
    mix.fire != 0.0
}

fn frezon_coolant(mix: &mut GasMixture) -> bool {
    let old_capacity = mix.heat_capacity();
    let temperature = mix.temperature;
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
    let initial_nit = mix.moles(Gas::Nitrogen);
    let initial_frezon = mix.moles(Gas::Frezon);
    let burn_rate = initial_frezon * scale / FREZON_COOL_RATE_MODIFIER;
    let mut energy = 0.0;
    if burn_rate > MINIMUM_HEAT_CAPACITY {
        let nit = (burn_rate * FREZON_NITROGEN_COOL_RATIO).min(initial_nit);
        let frezon = burn_rate.min(initial_frezon);
        mix.adjust_moles(Gas::Nitrogen, -nit);
        mix.adjust_moles(Gas::Frezon, -frezon);
        mix.adjust_moles(Gas::NitrousOxide, nit + frezon);
        energy = burn_rate * FREZON_COOL_ENERGY_RELEASED * energy_modifier;
    }
    energy /= HEAT_SCALE;
    if energy >= 0.0 {
        return false;
    }
    apply_energy(mix, temperature, old_capacity, energy);
    true
}

fn frezon_production(mix: &mut GasMixture) -> bool {
    let n2 = mix.moles(Gas::Nitrogen);
    let oxy = mix.moles(Gas::Oxygen);
    let trit = mix.moles(Gas::Tritium);
    let efficiency = mix.temperature / FREZON_PRODUCTION_MAX_EFFICIENCY_TEMPERATURE;
    let loss = 1.0 - efficiency;
    let catalyst_limit = n2 * (FREZON_PRODUCTION_NITROGEN_RATIO / efficiency);
    let oxy_limit = oxy.min(catalyst_limit) / FREZON_PRODUCTION_TRIT_RATIO;
    let trit_burned = oxy_limit.min(trit);
    let oxy_burned = trit_burned * FREZON_PRODUCTION_TRIT_RATIO;
    let oxy_conversion = oxy_burned / FREZON_PRODUCTION_CONVERSION_RATE;
    let trit_conversion = trit_burned / FREZON_PRODUCTION_CONVERSION_RATE;
    let total = oxy_conversion + trit_conversion;
    mix.adjust_moles(Gas::Oxygen, -oxy_conversion);
    mix.adjust_moles(Gas::Tritium, -trit_conversion);
    mix.adjust_moles(Gas::Frezon, total * efficiency);
    mix.adjust_moles(Gas::Nitrogen, total * loss);
    true
}

fn ammonia_oxygen(mix: &mut GasMixture) -> bool {
    let ammonia = mix.moles(Gas::Ammonia);
    let oxygen = mix.moles(Gas::Oxygen);
    let total = mix.total_moles();
    let rate = (ammonia / total).powi(2) * (oxygen / total).powi(2);
    let delta = ammonia / AMMONIA_OXYGEN_REACTION_RATE * 2.0 * rate;
    if delta <= 0.0 || ammonia - delta < 0.0 {
        return false;
    }
    mix.adjust_moles(Gas::Ammonia, -delta);
    mix.adjust_moles(Gas::Oxygen, -delta);
    mix.adjust_moles(Gas::NitrousOxide, delta / 2.0);
    mix.adjust_moles(Gas::WaterVapor, delta * 1.5);
    true
}

fn n2o_decomposition(mix: &mut GasMixture) -> bool {
    let n2o = mix.moles(Gas::NitrousOxide);
    let burned = n2o / N2O_DECOMPOSITION_RATE;
    if burned <= 0.0 || n2o - burned < 0.0 {
        return false;
    }
    mix.adjust_moles(Gas::NitrousOxide, -burned);
    mix.adjust_moles(Gas::Nitrogen, burned);
    mix.adjust_moles(Gas::Oxygen, burned / 2.0);
    true
}
