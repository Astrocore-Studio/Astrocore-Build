//! Здоровье и урон по образцу SS14: накопленный урон по видам, пороги состояния (`MobState`),
//! оружие ближнего боя и лечение предметами.
//!
//! Виды урона и группы — из `Resources/Prototypes/Damage` (`Brute`, `Burn`, `Toxin`); пороги
//! человека — критическое состояние с 100 единиц урона, смерть с 200.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Состояние здоровья: пороги суммарного урона (`MobThresholds` человека в SS14).
pub const CRITICAL_AT: f32 = 100.0;
pub const DEAD_AT: f32 = 200.0;
/// Урон без оружия: удар кулаком (`Blunt 5`).
pub const FIST_DAMAGE: f32 = 5.0;
/// Темп удара без оружия и дальность ближнего боя, м (`MeleeWeaponComponent` по умолчанию).
pub const DEFAULT_ATTACK_RATE: f32 = 1.5;
pub const MELEE_RANGE_M: f32 = 1.5;

/// Виды урона, с которыми работает билд, и группы для показа.
pub const KINDS: &[&str] = &[
    "Blunt", "Slash", "Piercing", "Heat", "Cold", "Shock", "Poison",
];
pub const GROUPS: &[(&str, &[&str])] = &[
    ("Brute", &["Blunt", "Slash", "Piercing"]),
    ("Burn", &["Heat", "Cold", "Shock"]),
    ("Toxin", &["Poison"]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MobState {
    Alive,
    Critical,
    Dead,
}

impl MobState {
    /// Может ли персонаж действовать: двигаться, использовать руки.
    pub fn can_act(self) -> bool {
        self == Self::Alive
    }

    pub fn to_byte(self) -> u8 {
        match self {
            Self::Alive => 0,
            Self::Critical => 1,
            Self::Dead => 2,
        }
    }

    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Alive,
            1 => Self::Critical,
            _ => Self::Dead,
        }
    }
}

/// Урон (положительные числа) или лечение (отрицательные) по видам.
pub type DamageSpec = BTreeMap<String, f32>;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Health {
    damage: BTreeMap<String, f32>,
}

impl Health {
    pub fn damage_of(&self, kind: &str) -> f32 {
        self.damage.get(kind).copied().unwrap_or(0.0)
    }

    pub fn total(&self) -> f32 {
        self.damage.values().sum()
    }

    pub fn group_total(&self, group: &str) -> f32 {
        GROUPS
            .iter()
            .find(|(g, _)| *g == group)
            .map_or(0.0, |(_, kinds)| {
                kinds.iter().map(|k| self.damage_of(k)).sum()
            })
    }

    pub fn state(&self) -> MobState {
        let t = self.total();
        if t >= DEAD_AT {
            MobState::Dead
        } else if t >= CRITICAL_AT {
            MobState::Critical
        } else {
            MobState::Alive
        }
    }

    /// Применить урон или лечение. Неизвестные виды игнорируются, результат по каждому виду не
    /// уходит ниже нуля. Мёртвого вылечить нельзя (воскрешение — отдельная механика).
    pub fn apply(&mut self, spec: &DamageSpec) {
        let dead = self.state() == MobState::Dead;
        for (kind, amount) in spec {
            if !KINDS.contains(&kind.as_str()) || !amount.is_finite() {
                continue;
            }
            if dead && *amount < 0.0 {
                continue;
            }
            let v = (self.damage_of(kind) + amount).max(0.0);
            if v == 0.0 {
                self.damage.remove(kind);
            } else {
                self.damage.insert(kind.clone(), v);
            }
        }
    }
}

/// Что видит игрок о своём здоровье.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthView {
    pub damage: BTreeMap<String, f32>,
    pub state: MobState,
}

pub const HEALTH_COMPONENT: &str = "build.health";
pub const HEALTH_MAX_BYTES: usize = 512;
/// Состояние здоровья для всех: один байт (`MobState::to_byte`).
pub const MOBSTATE_COMPONENT: &str = "build.mobstate";

impl Health {
    pub fn view(&self) -> HealthView {
        HealthView {
            damage: self.damage.clone(),
            state: self.state(),
        }
    }
}

pub fn encode_health(v: &HealthView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_health(bytes: &[u8]) -> Option<HealthView> {
    if bytes.len() > HEALTH_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Оружие ближнего боя предмета.
#[derive(Debug, Clone, PartialEq)]
pub struct Melee {
    pub damage: DamageSpec,
    pub rate: f32,
}

/// Лечащий предмет: расходуется целиком за одно применение.
#[derive(Debug, Clone, PartialEq)]
pub struct Healing {
    pub damage: DamageSpec,
    pub delay: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(items: &[(&str, f32)]) -> DamageSpec {
        items.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    #[test]
    fn thresholds_follow_the_total_and_healing_cannot_go_below_zero() {
        let mut h = Health::default();
        assert_eq!(h.state(), MobState::Alive);
        h.apply(&spec(&[("Blunt", 60.0), ("Heat", 40.0)]));
        assert_eq!(h.state(), MobState::Critical);
        assert_eq!(h.group_total("Brute"), 60.0);
        h.apply(&spec(&[("Blunt", -100.0)]));
        assert_eq!(h.damage_of("Blunt"), 0.0);
        assert_eq!(h.state(), MobState::Alive);
        h.apply(&spec(&[("Slash", 200.0)]));
        assert_eq!(h.state(), MobState::Dead);
    }

    #[test]
    fn the_dead_are_not_healed_and_junk_is_ignored() {
        let mut h = Health::default();
        h.apply(&spec(&[("Piercing", 250.0)]));
        h.apply(&spec(&[("Piercing", -250.0)]));
        assert_eq!(h.state(), MobState::Dead);
        let mut h = Health::default();
        h.apply(&spec(&[("Magic", 50.0), ("Blunt", f32::NAN)]));
        assert_eq!(h.total(), 0.0);
    }

    #[test]
    fn the_wire_form_round_trips() {
        let mut h = Health::default();
        h.apply(&spec(&[("Heat", 12.5)]));
        assert_eq!(decode_health(&encode_health(&h.view())), Some(h.view()));
        assert_eq!(
            MobState::from_byte(MobState::Critical.to_byte()),
            MobState::Critical
        );
        assert!(decode_health(&[0u8; 2000]).is_none());
    }
}
