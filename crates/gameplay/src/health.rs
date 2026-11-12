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
    "Blunt",
    "Slash",
    "Piercing",
    "Heat",
    "Cold",
    "Shock",
    "Poison",
    "Asphyxiation",
    "Bloodloss",
    "Caustic",
    "Radiation",
    "Cellular",
];
pub const GROUPS: &[(&str, &[&str])] = &[
    ("Brute", &["Blunt", "Slash", "Piercing"]),
    ("Burn", &["Heat", "Cold", "Shock", "Caustic"]),
    ("Toxin", &["Poison", "Radiation"]),
    ("Airloss", &["Asphyxiation", "Bloodloss"]),
    ("Genetic", &["Cellular"]),
];

/// Части тела: урон оружия приходится на одну из них. В SS14 урон по частям тела не делится —
/// это решение билда.
pub const PARTS: &[&str] = &[
    "Head", "Torso", "LeftArm", "RightArm", "LeftLeg", "RightLeg",
];
/// Рука или нога перестают работать при таком уроне на части.
pub const LIMB_DISABLED_AT: f32 = 50.0;
/// Смертельный урон на голове и на торсе.
pub const HEAD_FATAL_AT: f32 = 100.0;
pub const TORSO_FATAL_AT: f32 = 150.0;

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
    /// Урон по частям тела (только травмы и ожоги от ударов; потеря крови и удушье не локальны).
    #[serde(default)]
    parts: BTreeMap<String, f32>,
    /// Множители получаемого урона от статусов (`DamageModifierStatusEffect`).
    #[serde(skip)]
    modifiers: BTreeMap<String, f32>,
}

impl Health {
    /// Заменить множители урона (раз в шаг тела по действующим статусам).
    pub fn set_modifiers(&mut self, modifiers: impl IntoIterator<Item = (String, f32)>) {
        self.modifiers = modifiers.into_iter().collect();
    }

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

    pub fn part_damage(&self, part: &str) -> f32 {
        self.parts.get(part).copied().unwrap_or(0.0)
    }

    pub fn part_disabled(&self, part: &str) -> bool {
        self.part_damage(part) >= LIMB_DISABLED_AT
    }

    pub fn state(&self) -> MobState {
        let t = self.total();
        if self.part_damage("Head") >= HEAD_FATAL_AT || self.part_damage("Torso") >= TORSO_FATAL_AT
        {
            return MobState::Dead;
        }
        if t >= DEAD_AT {
            MobState::Dead
        } else if t >= CRITICAL_AT {
            MobState::Critical
        } else {
            MobState::Alive
        }
    }

    /// Применить урон или лечение к телу целиком (урон по частям не меняется, лечение снимает
    /// урон с самых повреждённых частей).
    pub fn apply(&mut self, spec: &DamageSpec) -> DamageSpec {
        self.apply_to(None, spec)
    }

    /// Применить урон к части тела. Неизвестные виды игнорируются, результат по каждому виду не
    /// уходит ниже нуля. Мёртвого вылечить нельзя. Возвращает фактический прирост урона по видам
    /// (только положительный): по нему кровеносная система считает кровотечение.
    pub fn apply_to(&mut self, part: Option<&str>, spec: &DamageSpec) -> DamageSpec {
        self.apply_inner(part, spec, false)
    }

    /// Как `apply`, но лечит и мёртвого (разряд дефибриллятора: `TryChangeDamage` без проверки состояния).
    pub fn apply_forced(&mut self, spec: &DamageSpec) -> DamageSpec {
        self.apply_inner(None, spec, true)
    }

    fn apply_inner(&mut self, part: Option<&str>, spec: &DamageSpec, revive: bool) -> DamageSpec {
        let mut dealt = DamageSpec::new();
        let dead = !revive && self.state() == MobState::Dead;
        for (kind, amount) in spec {
            if !KINDS.contains(&kind.as_str()) || !amount.is_finite() {
                continue;
            }
            let amount = &(if *amount > 0.0 {
                amount * self.modifiers.get(kind).copied().unwrap_or(1.0)
            } else {
                *amount
            });
            if dead && *amount < 0.0 {
                continue;
            }
            let v = (self.damage_of(kind) + amount).max(0.0);
            let before = self.damage_of(kind);
            if v > before {
                dealt.insert(kind.clone(), v - before);
            }
            if v == 0.0 {
                self.damage.remove(kind);
            } else {
                self.damage.insert(kind.clone(), v);
            }
            let local = !matches!(kind.as_str(), "Asphyxiation" | "Bloodloss");
            if *amount > 0.0 {
                if local && let Some(p) = part.filter(|p| PARTS.contains(p)) {
                    *self.parts.entry(p.to_string()).or_insert(0.0) += amount;
                }
            } else if local {
                self.heal_parts(-amount);
            }
        }
        dealt
    }

    /// Лечение снимает урон с самых повреждённых частей по очереди.
    fn heal_parts(&mut self, mut amount: f32) {
        while amount > 0.0 {
            let Some((name, v)) = self
                .parts
                .iter()
                .map(|(n, v)| (n.clone(), *v))
                .filter(|(_, v)| *v > 0.0)
                .max_by(|a, b| a.1.total_cmp(&b.1))
            else {
                return;
            };
            let take = amount.min(v);
            amount -= take;
            if v - take <= 0.0 {
                self.parts.remove(&name);
            } else {
                self.parts.insert(name, v - take);
            }
        }
    }
}

/// Что видит игрок о своём здоровье.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthView {
    pub damage: BTreeMap<String, f32>,
    pub state: MobState,
    #[serde(default)]
    pub parts: BTreeMap<String, f32>,
    #[serde(default)]
    pub suffocating: bool,
    /// Игрок мёртв и летает призраком.
    #[serde(default)]
    pub ghost: bool,
    /// Сколько секунд до возможности возродиться (на момент рассылки).
    #[serde(default)]
    pub respawn_in: f32,
}

pub const HEALTH_COMPONENT: &str = "build.health";
pub const HEALTH_MAX_BYTES: usize = 1024;
/// Состояние здоровья для всех: один байт (`MobState::to_byte`).
pub const MOBSTATE_COMPONENT: &str = "build.mobstate";
/// Призрак ли персонаж: один байт, видно всем (клиенты прячут призраков от живых).
pub const GHOST_COMPONENT: &str = "build.ghost";
/// Через сколько секунд после смерти призрак может возродиться (выбор билда).
pub const RESPAWN_DELAY_S: f32 = 30.0;
/// Прототип тела, остающегося на месте смерти.
pub const CORPSE_PROTOTYPE: &str = "ss14:corpse";

impl Health {
    pub fn view(&self) -> HealthView {
        HealthView {
            damage: self.damage.clone(),
            state: self.state(),
            parts: self.parts.clone(),
            suffocating: false,
            ghost: false,
            respawn_in: 0.0,
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
    /// Изменение кровотечения за применение (`bloodlossModifier`; отрицательное — останавливает).
    pub bleed_modifier: f32,
    /// Сколько раз можно применить (размер стопки).
    pub uses: u32,
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
    fn apply_reports_the_damage_actually_dealt() {
        let mut h = Health::default();
        let dealt = h.apply_to(Some("Torso"), &spec(&[("Slash", 20.0), ("Blunt", -5.0)]));
        assert_eq!(dealt.get("Slash"), Some(&20.0));
        assert!(!dealt.contains_key("Blunt"));
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

    #[test]
    fn parts_take_the_blow_limbs_fail_and_the_head_is_fatal() {
        let mut h = Health::default();
        h.apply_to(Some("LeftArm"), &spec(&[("Blunt", 55.0)]));
        assert!(h.part_disabled("LeftArm") && !h.part_disabled("RightArm"));
        assert_eq!(h.state(), MobState::Alive);
        h.apply_to(Some("Head"), &spec(&[("Blunt", 100.0)]));
        assert_eq!(
            h.state(),
            MobState::Dead,
            "голова 100 — смерть при общем уроне 155"
        );
        // Лечение идёт с самой повреждённой части.
        let mut h = Health::default();
        h.apply_to(Some("Torso"), &spec(&[("Blunt", 30.0)]));
        h.apply_to(Some("LeftLeg"), &spec(&[("Blunt", 10.0)]));
        h.apply(&spec(&[("Blunt", -20.0)]));
        assert_eq!(h.part_damage("Torso"), 10.0);
        assert_eq!(h.part_damage("LeftLeg"), 10.0);
    }
}
