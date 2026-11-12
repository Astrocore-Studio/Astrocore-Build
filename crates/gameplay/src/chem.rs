//! Реагенты, метаболизм и реакции из данных SS14 (`Resources/Prototypes/Reagents`,
//! `Recipes/Reactions`, снимок `910154a7`; `scripts/gen_chem.py`).
//!
//! Реагент: название, цвет и метаболизм по стадиям — скорость и список эффектов. Эффект:
//! вид, порог `minScale`, вероятность `prob` и условия (`ReagentCondition`, `MetabolizerTypeCondition`,
//! `MobStateCondition`, `TemperatureCondition`). Перенесены `HealthChange`, `EvenHealthChange`,
//! `DistributedHealthChange`, `Satiate`, `ModifyBleed`, `ModifyBloodLevel`, `Oxygenate`, `ModifyLungGas`
//! и `AdjustReagent`; остальные эффекты (дрожь, рвота, эмоции, статусы) отброшены генератором.
//!
//! Реакции (`ChemicalReactionSystem.FullyReactSolution`): пока в растворе есть всё нужное, самая
//! приоритетная реакция расходует реагенты (катализаторы остаются) и добавляет продукты; не больше 20
//! проходов. Перенесены только реакции без смесителя и без нагрева (комнатная температура).

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use atmos::Gas;
use prototypes::{Data, PrototypeRegistry};

use crate::reagents::{SatiationKind, Solution, Stage};

/// Условие эффекта.
#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    /// Реагента в метаболизируемом растворе от `min` до `max` (`max < 0` — без верхнего предела).
    Reagent {
        reagent: String,
        min: f32,
        max: f32,
    },
    MetabolizerType {
        types: Vec<String>,
        inverted: bool,
    },
    MobState {
        state: String,
        inverted: bool,
    },
    Temperature {
        min: f32,
        max: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// `HealthChange`: урон по видам (отрицательный — лечение).
    Health(Vec<(String, f32)>),
    /// `EvenHealthChange`: лечение группы поровну по видам с уроном.
    Even(Vec<(String, f32)>),
    /// `DistributedHealthChange`: лечение группы пропорционально урону.
    Distributed(Vec<(String, f32)>),
    Satiate {
        kind: SatiationKind,
        factor: f32,
    },
    Bleed(f32),
    Blood(f32),
    Oxygenate(f32),
    LungGas(Vec<(Gas, f32)>),
    /// `AdjustReagent`: изменить количество другого реагента в том же растворе.
    Adjust {
        reagent: String,
        amount: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Eff {
    pub kind: Kind,
    pub min_scale: f32,
    pub prob: f32,
    pub conds: Vec<Cond>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub rate: f32,
    pub effects: Vec<Eff>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reagent {
    pub id: String,
    pub name: String,
    pub color: String,
    pub group: String,
    pub works_on_dead: bool,
    pub metabolisms: BTreeMap<Stage, Entry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reaction {
    pub id: String,
    pub priority: i32,
    pub quantized: bool,
    pub reactants: Vec<(String, f32, bool)>,
    pub products: Vec<(String, f32)>,
}

/// Реагенты и реакции мира.
#[derive(Debug, Default, PartialEq)]
pub struct ReagentTable {
    pub reagents: BTreeMap<String, Reagent>,
    /// Реакции в порядке `ReactionPrototype.CompareTo`: приоритет по убыванию, меньше продуктов первыми.
    pub reactions: Vec<Reaction>,
}

/// Ссылка на таблицу в теле: сравнивается по указателю.
#[derive(Debug, Clone)]
pub struct TableRef(pub Arc<ReagentTable>);

impl PartialEq for TableRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

fn f(d: &Data, key: &str, default: f32) -> f32 {
    d.get(key)
        .and_then(Data::as_f64)
        .map_or(default, |v| v as f32)
}

fn pairs(d: Option<&Data>) -> Vec<(String, f32)> {
    d.and_then(Data::as_map)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32)))
                .collect()
        })
        .unwrap_or_default()
}

fn gas_of(name: &str) -> Option<Gas> {
    Some(match name {
        "Oxygen" => Gas::Oxygen,
        "Nitrogen" => Gas::Nitrogen,
        "CarbonDioxide" => Gas::CarbonDioxide,
        "Plasma" => Gas::Plasma,
        "Tritium" => Gas::Tritium,
        "WaterVapor" => Gas::WaterVapor,
        "Ammonia" => Gas::Ammonia,
        "NitrousOxide" => Gas::NitrousOxide,
        "Frezon" => Gas::Frezon,
        _ => return None,
    })
}

fn parse_cond(d: &Data) -> Option<Cond> {
    Some(match d.get("c")?.as_str()? {
        "reagent" => Cond::Reagent {
            reagent: d.get("reagent")?.as_str()?.to_string(),
            min: f(d, "min", 0.0),
            max: f(d, "max", -1.0),
        },
        "mtype" => Cond::MetabolizerType {
            types: d
                .get("types")?
                .as_list()?
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect(),
            inverted: d.get("inverted").and_then(Data::as_bool).unwrap_or(false),
        },
        "mobstate" => Cond::MobState {
            state: d.get("state")?.as_str()?.to_string(),
            inverted: d.get("inverted").and_then(Data::as_bool).unwrap_or(false),
        },
        "temp" => Cond::Temperature {
            min: f(d, "min", 0.0),
            max: f(d, "max", -1.0),
        },
        _ => return None,
    })
}

fn parse_eff(d: &Data) -> Option<Eff> {
    let kind = match d.get("k")?.as_str()? {
        "health" => Kind::Health(pairs(d.get("damage"))),
        "even" => Kind::Even(pairs(d.get("damage"))),
        "dist" => Kind::Distributed(pairs(d.get("damage"))),
        "satiate" => Kind::Satiate {
            kind: if d.get("type")?.as_str()? == "Thirst" {
                SatiationKind::Thirst
            } else {
                SatiationKind::Hunger
            },
            factor: f(d, "factor", 1.0),
        },
        "bleed" => Kind::Bleed(f(d, "amount", 0.0)),
        "blood" => Kind::Blood(f(d, "amount", 0.0)),
        "oxygenate" => Kind::Oxygenate(f(d, "factor", 1.0)),
        "lunggas" => Kind::LungGas(
            pairs(d.get("ratios"))
                .into_iter()
                .filter_map(|(g, v)| Some((gas_of(&g)?, v)))
                .collect(),
        ),
        "adjust" => Kind::Adjust {
            reagent: d.get("reagent")?.as_str()?.to_string(),
            amount: f(d, "amount", 0.0),
        },
        _ => return None,
    };
    Some(Eff {
        kind,
        min_scale: f(d, "minScale", 0.0),
        prob: f(d, "prob", 1.0),
        conds: d
            .get("conds")
            .and_then(Data::as_list)
            .map(|l| l.iter().filter_map(parse_cond).collect())
            .unwrap_or_default(),
    })
}

impl ReagentTable {
    /// Таблица из реестра прототипов (`Reagent…` и `Reaction…`).
    pub fn from_registry(registry: &PrototypeRegistry) -> Self {
        let mut table = Self::default();
        let ids: Vec<String> = registry.ids().map(str::to_string).collect();
        for id in ids {
            let Ok(r) = registry.spawnable(&id) else {
                continue;
            };
            if let Some(c) = r.component("Reagent")
                && let Some(rid) = id.strip_prefix("Reagent")
            {
                let mut metabolisms = BTreeMap::new();
                if let Some(m) = c.get("metabolisms").and_then(Data::as_map) {
                    for (stage, entry) in m {
                        let stage = match stage.as_str() {
                            "Bloodstream" => Stage::Bloodstream,
                            "Digestion" => Stage::Digestion,
                            "Respiration" => Stage::Respiration,
                            _ => continue,
                        };
                        metabolisms.insert(
                            stage,
                            Entry {
                                rate: f(entry, "rate", 0.5),
                                effects: entry
                                    .get("effects")
                                    .and_then(Data::as_list)
                                    .map(|l| l.iter().filter_map(parse_eff).collect())
                                    .unwrap_or_default(),
                            },
                        );
                    }
                }
                let s = |k: &str| {
                    c.get(k)
                        .and_then(Data::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                table.reagents.insert(
                    rid.to_string(),
                    Reagent {
                        id: rid.to_string(),
                        name: s("name"),
                        color: s("color"),
                        group: s("group"),
                        works_on_dead: c.get("deadWorks").and_then(Data::as_bool).unwrap_or(false),
                        metabolisms,
                    },
                );
            }
            if let Some(c) = r.component("Reaction") {
                let reactants = c
                    .get("reactants")
                    .and_then(Data::as_map)
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| {
                                (
                                    k.clone(),
                                    f(v, "amount", 1.0),
                                    v.get("catalyst").and_then(Data::as_bool).unwrap_or(false),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                table.reactions.push(Reaction {
                    id: id.clone(),
                    priority: c.get("priority").and_then(Data::as_int).unwrap_or(0) as i32,
                    quantized: c.get("quantized").and_then(Data::as_bool).unwrap_or(false),
                    reactants,
                    products: pairs(c.get("products")),
                });
            }
        }
        table.reactions.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(a.products.len().cmp(&b.products.len()))
                .then(a.id.cmp(&b.id))
        });
        table
    }

    /// Встроенная таблица из `content/prototypes/reagents.ron` и `reactions.ron` (для тел без модуля).
    pub fn builtin() -> Arc<Self> {
        static TABLE: OnceLock<Arc<ReagentTable>> = OnceLock::new();
        TABLE
            .get_or_init(|| {
                let mut reg = PrototypeRegistry::new();
                for (text, name) in [
                    (
                        include_str!("../../../content/prototypes/reagents.ron"),
                        "reagents",
                    ),
                    (
                        include_str!("../../../content/prototypes/reactions.ron"),
                        "reactions",
                    ),
                ] {
                    let _ = reg.load_ron_str(text, name);
                }
                Arc::new(Self::from_registry(&reg))
            })
            .clone()
    }

    pub fn metabolism(&self, reagent: &str, stage: Stage) -> Option<&Entry> {
        self.reagents.get(reagent)?.metabolisms.get(&stage)
    }

    /// `FullyReactSolution`: пока что-то реагирует, до 20 проходов. Возвращает, была ли хоть одна реакция.
    pub fn react(&self, solution: &mut Solution) -> bool {
        let mut any = false;
        for _ in 0..20 {
            let mut reacted = false;
            for r in &self.reactions {
                let Some(units) = Self::units(r, solution) else {
                    continue;
                };
                for (id, amount, catalyst) in &r.reactants {
                    if !catalyst {
                        solution.remove(id, units * amount);
                    }
                }
                for (id, amount) in &r.products {
                    // Продукты занимают место, освобождённое реагентами; лишнее не помещается.
                    solution.add(id, units * amount);
                }
                reacted = true;
                break;
            }
            if !reacted {
                break;
            }
            any = true;
        }
        any
    }

    /// `CanReact`: сколько «единиц» реакции возможно (`None` — нельзя).
    fn units(r: &Reaction, s: &Solution) -> Option<f32> {
        let mut lowest = f32::MAX;
        for (id, amount, catalyst) in &r.reactants {
            let have = s.amount(id);
            if have <= 0.0 {
                return None;
            }
            if *catalyst {
                if r.quantized && have < *amount {
                    return None;
                }
                continue;
            }
            lowest = lowest.min(have / amount);
        }
        if r.quantized {
            lowest = lowest.floor();
        }
        (lowest > 0.0 && lowest < f32::MAX).then_some(lowest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_table_has_medicine_and_reactions() {
        let t = ReagentTable::builtin();
        assert!(t.reagents.len() > 300, "{}", t.reagents.len());
        let bica = t.metabolism("Bicaridine", Stage::Bloodstream).unwrap();
        assert_eq!(bica.rate, 0.5);
        assert!(matches!(bica.effects[0].kind, Kind::Even(_)));
        assert!(t.reactions.len() > 150);
    }

    #[test]
    fn reactions_consume_reactants_keep_catalysts_and_make_products() {
        let t = ReagentTable::builtin();
        // Дилоксин: кремний + азот + калий → 3 дилоксина (в SS14 — Dylovene).
        let mut s = Solution::new(100.0);
        s.add("Silicon", 10.0);
        s.add("Nitrogen", 10.0);
        s.add("Potassium", 10.0);
        assert!(t.react(&mut s));
        assert!(
            (s.amount("Dylovene") - 30.0).abs() < 1e-3,
            "{:?}",
            s.contents
        );
        assert!(s.amount("Silicon") < 1e-3);
        // Нет всех компонентов — ничего не происходит.
        let mut s = Solution::new(100.0);
        s.add("Silicon", 10.0);
        assert!(!t.react(&mut s));
    }
}
