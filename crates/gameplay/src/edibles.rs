//! Еда и напитки по образцу `EdibleComponent` и `OpenableComponent` SS14: раствор в предмете,
//! укус переносит `transferAmount` единиц в желудок за `delay` секунд, пустая еда исчезает, а
//! бутылка пустая остаётся.

use prototypes::{Data, Resolved};

use crate::reagents::Solution;

/// Данные `Edible` прототипа.
#[derive(Debug, Clone, PartialEq)]
pub struct EdibleSpec {
    /// Сколько единиц переходит в желудок за раз (`TransferAmount`, по умолчанию 5).
    pub transfer: f32,
    /// Время одного укуса, с (`Delay`, по умолчанию 1).
    pub delay: f32,
    /// Исчезает ли предмет, когда раствор кончился (`DestroyOnEmpty`).
    pub destroy_on_empty: bool,
}

impl EdibleSpec {
    pub fn from_resolved(r: &Resolved) -> Option<Self> {
        let c = r.component("Edible")?;
        let f = |key: &str, default: f32| {
            c.get(key)
                .and_then(Data::as_f64)
                .map_or(default, |v| v.max(0.0) as f32)
        };
        Some(Self {
            transfer: f("transferAmount", 5.0),
            delay: f("delay", 1.0),
            destroy_on_empty: c
                .get("destroyOnEmpty")
                .and_then(Data::as_bool)
                .unwrap_or(true),
        })
    }
}

/// Раствор по умолчанию из `Solution: (maxVol, reagents)`.
pub fn solution_template(r: &Resolved) -> Option<Solution> {
    let c = r.component("Solution")?;
    let max = c
        .get("maxVol")
        .and_then(Data::as_f64)
        .map_or(30.0, |v| v.max(0.0) as f32);
    let mut s = Solution::new(max);
    if let Some(m) = c.get("reagents").and_then(Data::as_map) {
        for (k, v) in m {
            if let Some(q) = v.as_f64() {
                s.add(k, q as f32);
            }
        }
    }
    Some(s)
}

/// Закрыт ли предмет вначале (`Openable: (opened: false)`); `None` — не открывается.
pub fn openable_closed(r: &Resolved) -> Option<bool> {
    let c = r.component("Openable")?;
    Some(!c.get("opened").and_then(Data::as_bool).unwrap_or(false))
}
