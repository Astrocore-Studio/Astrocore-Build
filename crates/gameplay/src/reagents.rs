//! Реагенты и их метаболизм (`Resources/Prototypes/Reagents`, `Content.Shared/Metabolism`).
//!
//! Перенесены реагенты дыхания и пищеварения, с которыми работает билд: кислород, плазма,
//! углекислый газ, азот; вода, питательные вещества, белок, витамины, сахар. Эффекты
//! (`Satiate`, `Oxygenate`, `ModifyLungGas`, `HealthChange`, `EvenHealthChange`) и их числа взяты
//! из прототипов SS14 (коммит `910154a7`).
//!
//! Не перенесены: метаболиты, химия лекарств и реакции между реагентами.

use std::collections::BTreeMap;

use atmos::{BREATH_MOLES_TO_REAGENT_MULTIPLIER, Gas};

/// Скорость метаболизма реагента по умолчанию (`MetabolismRate`).
pub const DEFAULT_METABOLISM_RATE: f32 = 0.5;
/// Сила `Satiate` по умолчанию (`Factor`).
pub const DEFAULT_SATIATE_FACTOR: f32 = 1.5;

/// Раствор: количество каждого реагента и предельный объём.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Solution {
    pub contents: BTreeMap<String, f32>,
    pub max_volume: f32,
}

impl Solution {
    pub fn new(max_volume: f32) -> Self {
        Self {
            contents: BTreeMap::new(),
            max_volume,
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
        for (k, v) in &part.contents {
            self.add(k, *v);
        }
        rest
    }
}

/// Стадии метаболизма.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Respiration,
    Digestion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SatiationKind {
    Hunger,
    Thirst,
}

/// Эффекты метаболизма; `scale` — доля обработанного реагента (`quantity` для дыхания, иначе
/// `removed / rate`).
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// `Oxygenate`: насыщение кислородом на `factor × scale`.
    Oxygenate { factor: f32 },
    /// `ModifyLungGas`: обмен газа в лёгких, `ratio × scale / BreathMolesToReagentMultiplier` молей.
    ModifyLungGas(&'static [(Gas, f32)]),
    /// `HealthChange`: урон (в виде «вид, число») на `scale`; `min_scale` — порог срабатывания.
    HealthChange {
        damage: &'static [(&'static str, f32)],
        min_scale: f32,
    },
    /// `EvenHealthChange`: урон (отрицательный — лечение) поровну по видам группы, с вероятностью.
    EvenHealthChange {
        group: &'static [&'static str],
        amount: f32,
        probability: f32,
    },
    /// `ModifyBleed`: изменение силы кровотечения на `amount × scale`.
    ModifyBleed(f32),
    /// `ModifyBloodLevel`: добавить (или убавить) крови на `amount × scale`, не выше нормы.
    ModifyBloodLevel(f32),
    /// `Satiate` с необязательным условием «в растворе есть реагент не меньше `min`».
    Satiate {
        kind: SatiationKind,
        factor: f32,
        needs: Option<(&'static str, f32)>,
    },
}

pub struct Metabolism {
    pub stage: Stage,
    pub rate: f32,
    pub effects: &'static [Effect],
}

pub struct Reagent {
    pub id: &'static str,
    pub metabolisms: &'static [Metabolism],
}

const BRUTE: &[&str] = &["Blunt", "Slash", "Piercing"];
const BURN: &[&str] = &["Heat", "Cold", "Shock"];

pub const REAGENTS: &[Reagent] = &[
    Reagent {
        id: "Oxygen",
        metabolisms: &[Metabolism {
            stage: Stage::Respiration,
            rate: DEFAULT_METABOLISM_RATE,
            effects: &[
                Effect::Oxygenate { factor: 1.0 },
                Effect::ModifyLungGas(&[(Gas::CarbonDioxide, 1.0), (Gas::Oxygen, -1.0)]),
            ],
        }],
    },
    Reagent {
        id: "CarbonDioxide",
        metabolisms: &[Metabolism {
            stage: Stage::Respiration,
            rate: DEFAULT_METABOLISM_RATE,
            effects: &[
                Effect::HealthChange {
                    damage: &[("Poison", 0.8)],
                    min_scale: 0.5,
                },
                // Углекислый газ вытесняет кислород из крови.
                Effect::Oxygenate { factor: -4.0 },
            ],
        }],
    },
    Reagent {
        id: "Plasma",
        metabolisms: &[Metabolism {
            stage: Stage::Respiration,
            rate: DEFAULT_METABOLISM_RATE,
            effects: &[Effect::HealthChange {
                damage: &[("Poison", 0.1)],
                min_scale: 0.0,
            }],
        }],
    },
    Reagent {
        id: "Nitrogen",
        metabolisms: &[],
    },
    Reagent {
        id: "Nutriment",
        metabolisms: &[Metabolism {
            stage: Stage::Digestion,
            rate: 1.0,
            effects: &[Effect::Satiate {
                kind: SatiationKind::Hunger,
                factor: 3.0,
                needs: None,
            }],
        }],
    },
    Reagent {
        id: "Water",
        metabolisms: &[Metabolism {
            stage: Stage::Digestion,
            rate: DEFAULT_METABOLISM_RATE,
            effects: &[Effect::Satiate {
                kind: SatiationKind::Thirst,
                factor: 4.0,
                needs: None,
            }],
        }],
    },
    Reagent {
        id: "Protein",
        metabolisms: &[Metabolism {
            stage: Stage::Digestion,
            rate: DEFAULT_METABOLISM_RATE,
            effects: &[
                Effect::EvenHealthChange {
                    group: BRUTE,
                    amount: -0.4,
                    probability: 0.5,
                },
                Effect::ModifyBloodLevel(1.0),
                Effect::Satiate {
                    kind: SatiationKind::Hunger,
                    factor: DEFAULT_SATIATE_FACTOR,
                    needs: None,
                },
            ],
        }],
    },
    Reagent {
        id: "Vitamin",
        metabolisms: &[Metabolism {
            stage: Stage::Digestion,
            rate: DEFAULT_METABOLISM_RATE,
            effects: &[
                Effect::EvenHealthChange {
                    group: BRUTE,
                    amount: -0.5,
                    probability: 0.5,
                },
                Effect::EvenHealthChange {
                    group: BURN,
                    amount: -0.5,
                    probability: 0.5,
                },
                Effect::ModifyBleed(-0.25),
                Effect::Satiate {
                    kind: SatiationKind::Hunger,
                    factor: DEFAULT_SATIATE_FACTOR,
                    needs: None,
                },
            ],
        }],
    },
    Reagent {
        id: "Sugar",
        metabolisms: &[Metabolism {
            stage: Stage::Digestion,
            rate: DEFAULT_METABOLISM_RATE,
            effects: &[Effect::Satiate {
                kind: SatiationKind::Hunger,
                factor: 1.0,
                // Насыщает, только если съеден вместе с питательными веществами.
                needs: Some(("Nutriment", 0.1)),
            }],
        }],
    },
];

pub fn reagent(id: &str) -> Option<&'static Reagent> {
    REAGENTS.iter().find(|r| r.id == id)
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
