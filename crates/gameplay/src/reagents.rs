//! Реагенты и их метаболизм (`Resources/Prototypes/Reagents`, `Content.Shared/Metabolism`).
//!
//! Здесь раствор (`Solution`) и газ ↔ реагент; сами реагенты, метаболизм и реакции — в `chem.rs`.

use std::collections::BTreeMap;

use atmos::{BREATH_MOLES_TO_REAGENT_MULTIPLIER, Gas};

/// Комнатная температура раствора по умолчанию, К (`Solution.Temperature`).
pub const ROOM_TEMPERATURE_K: f32 = 293.15;

/// Раствор: количество каждого реагента, предельный объём и температура. Теплоёмкость — сумма
/// `количество × удельная теплоёмкость`, а у всех реагентов SS14 `specificHeat` равна 1, поэтому
/// теплоёмкость раствора равна его объёму.
#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    pub contents: BTreeMap<String, f32>,
    pub max_volume: f32,
    pub temperature: f32,
    /// `canReact`: идут ли в растворе реакции (у таблеток выключено).
    pub can_react: bool,
}

impl Default for Solution {
    fn default() -> Self {
        Self::new(0.0)
    }
}

impl Solution {
    pub fn new(max_volume: f32) -> Self {
        Self {
            contents: BTreeMap::new(),
            max_volume,
            temperature: ROOM_TEMPERATURE_K,
            can_react: true,
        }
    }

    /// `GetHeatCapacity`.
    pub fn heat_capacity(&self) -> f32 {
        self.volume()
    }

    /// `GetThermalEnergy`.
    pub fn thermal_energy(&self) -> f32 {
        self.heat_capacity() * self.temperature
    }

    /// `AddThermalEnergy`: температура меняется на `энергия / теплоёмкость`.
    pub fn add_thermal_energy(&mut self, joules: f32) {
        let cap = self.heat_capacity();
        if cap > 0.0 && joules != 0.0 {
            self.temperature += joules / cap;
        }
    }

    pub fn volume(&self) -> f32 {
        self.contents.values().sum()
    }

    pub fn available(&self) -> f32 {
        (self.max_volume - self.volume()).max(0.0)
    }

    pub fn amount(&self, reagent: &str) -> f32 {
        self.contents.get(reagent).copied().unwrap_or(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.volume() <= 1e-4
    }

    /// Добавить реагент; сверх предела не добавляется. Возвращает, сколько принято.
    pub fn add(&mut self, reagent: &str, amount: f32) -> f32 {
        let take = amount.min(self.available()).max(0.0);
        if take > 0.0 {
            *self.contents.entry(reagent.to_string()).or_insert(0.0) += take;
        }
        take
    }

    pub fn remove(&mut self, reagent: &str, amount: f32) {
        if let Some(v) = self.contents.get_mut(reagent) {
            *v -= amount;
            if *v <= 1e-5 {
                self.contents.remove(reagent);
            }
        }
    }

    /// Сколько в растворе реагентов, подходящих под `pick`.
    pub fn quantity_where(&self, pick: impl Fn(&str) -> bool) -> f32 {
        self.contents
            .iter()
            .filter(|(r, _)| pick(r))
            .map(|(_, q)| q)
            .sum()
    }

    /// Отделить до `amount` единиц только из реагентов под `pick` (`SplitSolutionWithOnly`; с
    /// отрицанием — `SplitSolutionWithout`), пропорционально их количеству.
    pub fn split_where(&mut self, amount: f32, pick: impl Fn(&str) -> bool) -> Solution {
        let total = self.quantity_where(&pick);
        let mut out = Solution::new(amount.max(0.0));
        out.temperature = self.temperature;
        if total <= 0.0 || amount <= 0.0 {
            return out;
        }
        let ratio = (amount / total).min(1.0);
        let names: Vec<String> = self.contents.keys().filter(|r| pick(r)).cloned().collect();
        for name in names {
            let part = self.amount(&name) * ratio;
            out.contents.insert(name.clone(), part);
            self.remove(&name, part);
        }
        out
    }

    /// Отделить `amount` единиц пропорционально составу (`SplitSolution`).
    pub fn split(&mut self, amount: f32) -> Solution {
        let total = self.volume();
        let mut out = Solution::new(amount.max(0.0));
        out.temperature = self.temperature;
        if total <= 0.0 || amount <= 0.0 {
            return out;
        }
        let ratio = (amount / total).min(1.0);
        let names: Vec<String> = self.contents.keys().cloned().collect();
        for name in names {
            let part = self.amount(&name) * ratio;
            out.contents.insert(name.clone(), part);
            self.remove(&name, part);
        }
        out
    }

    /// Влить другой раствор целиком (сколько поместится).
    pub fn merge(&mut self, other: &Solution) -> Solution {
        let room = self.available();
        let mut rest = other.clone();
        let part = rest.split(room.min(other.volume()));
        // Температура смеси сохраняет тепловую энергию (`AddSolution`).
        let energy = self.thermal_energy() + part.thermal_energy();
        for (k, v) in &part.contents {
            self.add(k, *v);
        }
        let cap = self.heat_capacity();
        if cap > 0.0 {
            self.temperature = energy / cap;
        }
        rest
    }
}

/// Стадии метаболизма.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Respiration,
    Digestion,
    /// Кровь: метаболизирует сердце (`Bloodstream` в SS14).
    Bloodstream,
    /// Метаболиты: то, что ушло из крови; перерабатывает печень (`Metabolites`).
    Metabolites,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SatiationKind {
    Hunger,
    Thirst,
}

/// Реагент, в который переходит газ при вдохе (`reagent:` в `gases.yml`).
pub fn gas_reagent(gas: Gas) -> Option<&'static str> {
    match gas {
        Gas::Oxygen => Some("Oxygen"),
        Gas::Nitrogen => Some("Nitrogen"),
        Gas::CarbonDioxide => Some("CarbonDioxide"),
        Gas::Plasma => Some("Plasma"),
        _ => None,
    }
}

/// Газ → реагенты лёгких (`GasToReagent`): `moles × 1144` единиц на каждый газ.
pub fn gas_to_reagents(gas: &atmos::GasMixture, into: &mut Solution) {
    for g in [Gas::Oxygen, Gas::Nitrogen, Gas::CarbonDioxide, Gas::Plasma] {
        let moles = gas.moles(g);
        if moles <= 0.0 {
            continue;
        }
        if let Some(r) = gas_reagent(g) {
            into.add(r, moles * BREATH_MOLES_TO_REAGENT_MULTIPLIER);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_keeps_the_proportions_and_the_total() {
        let mut s = Solution::new(100.0);
        s.add("Nutriment", 30.0);
        s.add("Protein", 10.0);
        s.add("Vitamin", 10.0);
        let bite = s.split(5.0);
        assert!((bite.volume() - 5.0).abs() < 1e-4);
        assert!((bite.amount("Nutriment") - 3.0).abs() < 1e-4);
        assert!((s.volume() - 45.0).abs() < 1e-3);
    }

    #[test]
    fn a_solution_does_not_overflow_and_a_breath_of_air_makes_five_units_of_oxygen() {
        let mut s = Solution::new(10.0);
        assert_eq!(s.add("Water", 25.0), 10.0);
        assert_eq!(s.available(), 0.0);
        let mut air = atmos::GasMixture::standard_air();
        let breath = air.remove_volume(atmos::BREATH_VOLUME);
        let mut lung = Solution::new(100.0);
        gas_to_reagents(&breath, &mut lung);
        assert!(
            (lung.amount("Oxygen") - 5.0).abs() < 0.05,
            "{}",
            lung.amount("Oxygen")
        );
        assert!(lung.amount("Nitrogen") > 15.0);
    }
}
