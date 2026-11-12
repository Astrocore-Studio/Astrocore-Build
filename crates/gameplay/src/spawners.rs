//! Спавнеры карты (`ConditionalSpawnerSystem`, `EntityTable`, снимок SS14 `910154a7`).
//!
//! При старте раунда каждый спавнер заменяется предметами: `EntityTableSpawner` берёт таблицу
//! (`ent` — предмет и количество, `group` — один из детей с весом, `all` — все дети, `nested` —
//! другая таблица; у любого узла есть `rolls` и `prob`), `RandomSpawner`/`ConditionalSpawner` —
//! один прототип из списка (редкий список с `rareChance`, общий шанс `chance`). Предмет ложится
//! в случайной точке квадрата `offset` вокруг спавнера. Предметы, прототипа которых нет в Build
//! (мобы и т. п.), не создаются.

use std::collections::BTreeMap;

use prototypes::Data;

use crate::disposal::Rng;

/// Число или диапазон (`NumberSelector`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Count {
    Const(u32),
    Range(u32, u32),
}

impl Count {
    fn parse(d: Option<&Data>) -> Self {
        match d {
            Some(Data::Int(n)) => Self::Const((*n).max(0) as u32),
            Some(m @ Data::Map(_)) => {
                let a = m.get("min").and_then(Data::as_int).unwrap_or(1);
                let b = m.get("max").and_then(Data::as_int).unwrap_or(a);
                Self::Range(a.max(0) as u32, b.max(0) as u32)
            }
            _ => Self::Const(1),
        }
    }

    /// `RangeNumberSelector`: включительно с обеих сторон.
    fn get(self, rng: &mut Rng) -> u32 {
        match self {
            Self::Const(n) => n,
            Self::Range(a, b) => {
                let (lo, hi) = (a.min(b), a.max(b));
                lo + ((rng.next_f32() * (hi - lo + 1) as f32) as u32).min(hi - lo)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Ent { id: String, amount: Count },
    Group(Vec<Node>),
    All(Vec<Node>),
    Nested(String),
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: Kind,
    pub weight: f32,
    pub prob: f32,
    pub rolls: Count,
}

impl Node {
    pub fn parse(d: &Data) -> Option<Self> {
        let kind = match d.get("k")?.as_str()? {
            "ent" => Kind::Ent {
                id: d.get("id")?.as_str()?.to_string(),
                amount: Count::parse(d.get("amount")),
            },
            "group" | "all" => {
                let children = d
                    .get("children")?
                    .as_list()?
                    .iter()
                    .filter_map(Self::parse)
                    .collect();
                if d.get("k")?.as_str()? == "group" {
                    Kind::Group(children)
                } else {
                    Kind::All(children)
                }
            }
            "nested" => Kind::Nested(d.get("table")?.as_str()?.to_string()),
            "none" => Kind::None,
            _ => return None,
        };
        Some(Self {
            kind,
            weight: d.get("weight").and_then(Data::as_f64).unwrap_or(1.0) as f32,
            prob: d.get("prob").and_then(Data::as_f64).unwrap_or(1.0) as f32,
            rolls: Count::parse(d.get("rolls")),
        })
    }

    /// `EntityTableSelector.GetSpawns`.
    pub fn spawns(&self, tables: &BTreeMap<String, Node>, rng: &mut Rng, out: &mut Vec<String>) {
        for _ in 0..self.rolls.get(rng) {
            if rng.next_f32() >= self.prob && self.prob < 1.0 {
                continue;
            }
            match &self.kind {
                Kind::Ent { id, amount } => {
                    for _ in 0..amount.get(rng) {
                        out.push(id.clone());
                    }
                }
                Kind::Group(children) => {
                    let total: f32 = children.iter().map(|c| c.weight).sum();
                    if total <= 0.0 {
                        continue;
                    }
                    let mut roll = rng.next_f32() * total;
                    for c in children {
                        if roll < c.weight {
                            c.spawns(tables, rng, out);
                            break;
                        }
                        roll -= c.weight;
                    }
                }
                Kind::All(children) => {
                    for c in children {
                        c.spawns(tables, rng, out);
                    }
                }
                Kind::Nested(id) => {
                    if let Some(t) = tables.get(id) {
                        t.spawns(tables, rng, out);
                    }
                }
                Kind::None => {}
            }
        }
    }
}

/// Описание спавнера.
#[derive(Debug, Clone, PartialEq)]
pub enum Spawner {
    Table {
        offset: f32,
        table: Node,
    },
    Random {
        offset: f32,
        chance: f32,
        rare_chance: f32,
        prototypes: Vec<String>,
        rare: Vec<String>,
    },
}

impl Spawner {
    pub fn parse(d: &Data) -> Option<Self> {
        let offset = d.get("offset").and_then(Data::as_f64).unwrap_or(0.2) as f32;
        if let Some(t) = d.get("table") {
            return Some(Self::Table {
                offset,
                table: Node::parse(t)?,
            });
        }
        let list = |k: &str| -> Vec<String> {
            d.get(k)
                .and_then(Data::as_list)
                .map(|l| {
                    l.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        Some(Self::Random {
            offset,
            chance: d.get("chance").and_then(Data::as_f64).unwrap_or(1.0) as f32,
            rare_chance: d.get("rareChance").and_then(Data::as_f64).unwrap_or(0.05) as f32,
            prototypes: list("prototypes"),
            rare: list("rarePrototypes"),
        })
    }

    pub fn offset(&self) -> f32 {
        match self {
            Self::Table { offset, .. } | Self::Random { offset, .. } => *offset,
        }
    }

    /// Что создаёт спавнер на этот раз (имена прототипов SS14).
    pub fn roll(&self, tables: &BTreeMap<String, Node>, rng: &mut Rng) -> Vec<String> {
        let mut out = Vec::new();
        match self {
            Self::Table { table, .. } => table.spawns(tables, rng, &mut out),
            Self::Random {
                chance,
                rare_chance,
                prototypes,
                rare,
                ..
            } => {
                let list =
                    if !rare.is_empty() && (*rare_chance >= 1.0 || rng.next_f32() < *rare_chance) {
                        rare
                    } else if *chance >= 1.0 || rng.next_f32() < *chance {
                        prototypes
                    } else {
                        return out;
                    };
                if !list.is_empty() {
                    let i = ((rng.next_f32() * list.len() as f32) as usize).min(list.len() - 1);
                    out.push(list[i].clone());
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(id: &str) -> Node {
        Node {
            kind: Kind::Ent {
                id: id.into(),
                amount: Count::Const(1),
            },
            weight: 1.0,
            prob: 1.0,
            rolls: Count::Const(1),
        }
    }

    fn node(kind: Kind) -> Node {
        Node {
            kind,
            weight: 1.0,
            prob: 1.0,
            rolls: Count::Const(1),
        }
    }

    #[test]
    fn a_group_picks_one_child_by_weight() {
        let mut heavy = ent("Common");
        heavy.weight = 95.0;
        let mut rare = ent("Rare");
        rare.weight = 5.0;
        let g = node(Kind::Group(vec![heavy, rare]));
        let mut rng = Rng(3);
        let (mut common, mut rarec) = (0, 0);
        for _ in 0..2000 {
            let mut out = Vec::new();
            g.spawns(&BTreeMap::new(), &mut rng, &mut out);
            assert_eq!(out.len(), 1);
            if out[0] == "Common" {
                common += 1;
            } else {
                rarec += 1;
            }
        }
        assert!(rarec > 40 && rarec < 180, "{rarec}");
        assert_eq!(common + rarec, 2000);
    }

    #[test]
    fn all_nested_and_probability_combine() {
        let tables = BTreeMap::from([("T".to_string(), node(Kind::All(vec![ent("A"), ent("B")])))]);
        let mut n = node(Kind::Nested("T".into()));
        n.prob = 0.5;
        let mut rng = Rng(8);
        let mut got = 0;
        for _ in 0..1000 {
            let mut out = Vec::new();
            n.spawns(&tables, &mut rng, &mut out);
            assert!(out.is_empty() || out == ["A", "B"]);
            got += usize::from(!out.is_empty());
        }
        assert!(got > 400 && got < 600, "{got}");
    }

    #[test]
    fn amounts_and_rolls_accept_ranges() {
        let n = Node {
            kind: Kind::Ent {
                id: "X".into(),
                amount: Count::Range(2, 4),
            },
            weight: 1.0,
            prob: 1.0,
            rolls: Count::Const(2),
        };
        let mut rng = Rng(1);
        for _ in 0..200 {
            let mut out = Vec::new();
            n.spawns(&BTreeMap::new(), &mut rng, &mut out);
            assert!((4..=8).contains(&out.len()), "{}", out.len());
        }
    }

    #[test]
    fn a_random_spawner_uses_the_rare_list_with_its_chance() {
        let s = Spawner::Random {
            offset: 0.2,
            chance: 1.0,
            rare_chance: 0.25,
            prototypes: vec!["Common".into()],
            rare: vec!["Rare".into()],
        };
        let mut rng = Rng(5);
        let rare = (0..2000)
            .filter(|_| s.roll(&BTreeMap::new(), &mut rng) == ["Rare"])
            .count();
        assert!(rare > 400 && rare < 600, "{rare}");
    }

    #[test]
    fn data_parses_into_a_table() {
        let d = Data::Map(BTreeMap::from([
            ("k".to_string(), Data::Str("group".into())),
            (
                "children".to_string(),
                Data::List(vec![Data::Map(BTreeMap::from([
                    ("k".to_string(), Data::Str("ent".into())),
                    ("id".to_string(), Data::Str("Wrench".into())),
                ]))]),
            ),
        ]));
        let n = Node::parse(&d).unwrap();
        let mut out = Vec::new();
        n.spawns(&BTreeMap::new(), &mut Rng(2), &mut out);
        assert_eq!(out, ["Wrench"]);
    }
}
