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
    /// `BloodLevelCondition`: доля крови от нормы.
    BloodLevel {
        min: f32,
        max: f32,
    },
    /// `SatiationCondition`.
    Satiation {
        kind: SatiationKind,
        min: f32,
        max: f32,
    },
    /// `TagCondition`: у тел в билде тегов нет, так что `inverted` выполняется всегда.
    Tag {
        inverted: bool,
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
    /// `PopupMessage`: случайная фраза владельцу тела.
    Popup {
        messages: Vec<String>,
        large: bool,
    },
    /// `Emote`: звук из набора эмоции (чата в билде нет).
    Emote {
        emote: String,
        sounds: Vec<String>,
    },
    /// `Vomit`: рвота, меняет сытость и жажду.
    Vomit {
        thirst: f32,
        hunger: f32,
        sounds: Vec<String>,
    },
    /// `AdjustTemperature`: тепло телу, Дж.
    Temp(f32),
    /// `ModifyStatusEffect` и `GenericStatusEffect`: время статуса (отрицательное — бессрочно).
    Status {
        name: String,
        op: StatusOp,
        time: f32,
        delay: f32,
    },
    /// `MovementSpeedModifier`: статус со множителем скорости.
    Speed {
        name: String,
        walk: f32,
        op: StatusOp,
        time: f32,
    },
    /// `Flammable`: горючесть тела растёт (`Multiplier`, у горящего — `MultiplierOnExisting`).
    Flammable {
        mult: f32,
        existing: f32,
    },
    Ignite,
    /// `Electrocute`: удар током с оглушением.
    Electrocute {
        time: f32,
        damage: f32,
    },
    /// `ModifyKnockdown`.
    Knockdown {
        op: StatusOp,
        time: f32,
    },
    /// `CleanBloodstream`: вымыть из крови лишнее, кроме `excluded`.
    Clean {
        rate: f32,
        excluded: String,
    },
}

/// `StatusEffectMetabolismType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusOp {
    Update,
    Add,
    Remove,
    Set,
}

impl StatusOp {
    fn parse(s: &str) -> Self {
        match s {
            "Add" => Self::Add,
            "Remove" => Self::Remove,
            "Set" => Self::Set,
            _ => Self::Update,
        }
    }
}

/// Прототип статуса (`StatusEffect…`): что он делает с телом.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusDef {
    pub flags: Vec<String>,
    /// Множители получаемого урона по видам (`DamageModifierStatusEffect`).
    pub damage: Vec<(String, f32)>,
    /// `HemophiliaStatusEffect.bleedReductionMultiplier` и `bleedAmountMultiplier`.
    pub bleed_reduction: f32,
    pub bleed_amount: f32,
}

impl StatusDef {
    pub fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
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
    /// `metabolites`: сколько какого реагента уходит в следующий раствор за единицу переработанного.
    pub metabolites: Vec<(String, f32)>,
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
    /// `minTemp` и `maxTemp`, К.
    pub min_temp: f32,
    pub max_temp: f32,
    /// `conserveEnergy`: после реакции тепловая энергия раствора сохраняется.
    pub conserve_energy: bool,
    /// `requiredMixerCategories`: реакция идёт только при смесителе, умеющем всё перечисленное.
    pub mixers: Vec<String>,
}

/// Реагенты и реакции мира.
#[derive(Debug, Default, PartialEq)]
pub struct ReagentTable {
    pub reagents: BTreeMap<String, Reagent>,
    /// Реакции в порядке `ReactionPrototype.CompareTo`: приоритет по убыванию, меньше продуктов первыми.
    pub reactions: Vec<Reaction>,
    /// Статусы по имени без префикса `StatusEffect`.
    pub statuses: BTreeMap<String, StatusDef>,
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

fn strings(d: Option<&Data>) -> Vec<String> {
    d.and_then(Data::as_list)
        .map(|l| {
            l.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
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
        "bloodlevel" => Cond::BloodLevel {
            min: f(d, "min", 0.5),
            max: f(d, "max", f32::MAX),
        },
        "satiation" => Cond::Satiation {
            kind: if d.get("type")?.as_str()? == "Thirst" {
                SatiationKind::Thirst
            } else {
                SatiationKind::Hunger
            },
            min: f(d, "min", 0.0),
            max: f(d, "max", f32::MAX),
        },
        "tag" => Cond::Tag {
            inverted: d.get("inverted").and_then(Data::as_bool).unwrap_or(false),
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
        "popup" => Kind::Popup {
            messages: strings(d.get("messages")),
            large: d.get("large").and_then(Data::as_bool).unwrap_or(false),
        },
        "emote" => Kind::Emote {
            emote: d.get("emote")?.as_str()?.to_string(),
            sounds: strings(d.get("sounds")),
        },
        "vomit" => Kind::Vomit {
            thirst: f(d, "thirst", -8.0),
            hunger: f(d, "hunger", -8.0),
            sounds: strings(d.get("sounds")),
        },
        "temp" => Kind::Temp(f(d, "amount", 0.0)),
        "status" => Kind::Status {
            name: d.get("name")?.as_str()?.to_string(),
            op: StatusOp::parse(d.get("op")?.as_str()?),
            time: f(d, "time", 2.0),
            delay: f(d, "delay", 0.0),
        },
        "speed" => Kind::Speed {
            name: d.get("name")?.as_str()?.to_string(),
            walk: f(d, "walk", 1.0),
            op: StatusOp::parse(d.get("op")?.as_str()?),
            time: f(d, "time", 2.0),
        },
        "flammable" => Kind::Flammable {
            mult: f(d, "mult", 0.05),
            existing: f(d, "existing", -1.0),
        },
        "ignite" => Kind::Ignite,
        "electrocute" => Kind::Electrocute {
            time: f(d, "time", 2.0),
            damage: f(d, "damage", 5.0),
        },
        "knockdown" => Kind::Knockdown {
            op: StatusOp::parse(d.get("op")?.as_str()?),
            time: f(d, "time", 2.0),
        },
        "clean" => Kind::Clean {
            rate: f(d, "rate", 3.0),
            excluded: d
                .get("excluded")
                .and_then(Data::as_str)
                .unwrap_or_default()
                .to_string(),
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
                            "Metabolites" => Stage::Metabolites,
                            _ => continue,
                        };
                        metabolisms.insert(
                            stage,
                            Entry {
                                rate: f(entry, "rate", 0.5),
                                metabolites: pairs(entry.get("metabolites")),
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
            if let Some(c) = r.component("Status")
                && let Some(name) = id.strip_prefix("StatusEffect")
            {
                table.statuses.insert(
                    name.to_string(),
                    StatusDef {
                        flags: strings(c.get("flags")),
                        damage: pairs(c.get("damage")),
                        bleed_reduction: f(c, "bleedReduction", 1.0),
                        bleed_amount: f(c, "bleedAmount", 1.0),
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
                    min_temp: f(c, "minTemp", 0.0),
                    max_temp: f(c, "maxTemp", f32::MAX),
                    conserve_energy: c
                        .get("conserveEnergy")
                        .and_then(Data::as_bool)
                        .unwrap_or(true),
                    mixers: c
                        .get("mixers")
                        .and_then(Data::as_list)
                        .map(|l| {
                            l.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
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
                    (
                        include_str!("../../../content/prototypes/statuses.ron"),
                        "statuses",
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
        self.react_with(solution, None)
    }

    /// То же со смесителем: `mixer` — виды смешивания, которые он умеет (`ReactionMixerComponent.ReactionTypes`);
    /// реакции с `requiredMixerCategories` идут, только если смеситель умеет все их.
    pub fn react_with(&self, solution: &mut Solution, mixer: Option<&[String]>) -> bool {
        if !solution.can_react {
            return false;
        }
        let mut any = false;
        for _ in 0..20 {
            let mut reacted = false;
            for r in &self.reactions {
                if solution.temperature < r.min_temp || solution.temperature > r.max_temp {
                    continue;
                }
                if !r.mixers.is_empty()
                    && !mixer.is_some_and(|m| r.mixers.iter().all(|c| m.contains(c)))
                {
                    continue;
                }
                let Some(units) = Self::units(r, solution) else {
                    continue;
                };
                let energy = solution.thermal_energy();
                for (id, amount, catalyst) in &r.reactants {
                    if !catalyst {
                        solution.remove(id, units * amount);
                    }
                }
                for (id, amount) in &r.products {
                    // Продукты занимают место, освобождённое реагентами; лишнее не помещается.
                    solution.add(id, units * amount);
                }
                if r.conserve_energy {
                    let cap = solution.heat_capacity();
                    if cap > 0.0 {
                        solution.temperature = energy / cap;
                    }
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
