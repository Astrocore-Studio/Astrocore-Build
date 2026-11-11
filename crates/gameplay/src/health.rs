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
];
pub const GROUPS: &[(&str, &[&str])] = &[
    ("Brute", &["Blunt", "Slash", "Piercing"]),
    ("Burn", &["Heat", "Cold", "Shock"]),
    ("Toxin", &["Poison"]),
    ("Airloss", &["Asphyxiation", "Bloodloss"]),
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
/// Кровотечение от колющего и режущего урона: сила кровотечения на единицу урона.
pub const BLEED_PER_DAMAGE: f32 = 0.15;
/// На сколько в секунду кровотечение затухает само.
pub const BLEED_DECAY_PER_S: f32 = 0.05;
/// Потеря крови: урон `Bloodloss` в секунду на единицу кровотечения.
pub const BLOODLOSS_PER_BLEED: f32 = 1.0;
/// Удушье в безвоздушной области, урон в секунду, и восстановление на воздухе.
pub const SUFFOCATION_PER_S: f32 = 1.0;
pub const BREATHING_RECOVERY_PER_S: f32 = 1.0;

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
    /// Сила кровотечения.
    #[serde(default)]
    bleed: f32,
    /// Дышать нечем (персонаж в безвоздушной области).
    #[serde(default)]
    pub suffocating: bool,
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

    pub fn part_damage(&self, part: &str) -> f32 {
        self.parts.get(part).copied().unwrap_or(0.0)
    }

    pub fn part_disabled(&self, part: &str) -> bool {
        self.part_damage(part) >= LIMB_DISABLED_AT
    }

    pub fn bleed(&self) -> f32 {
        self.bleed
    }

    /// Остановить кровотечение (перевязка).
    pub fn stop_bleed(&mut self, amount: f32) {
        self.bleed = (self.bleed - amount.max(0.0)).max(0.0);
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
    pub fn apply(&mut self, spec: &DamageSpec) {
        self.apply_to(None, spec);
    }

    /// Применить урон к части тела. Неизвестные виды игнорируются, результат по каждому виду не
    /// уходит ниже нуля. Мёртвого вылечить нельзя. Колющий и режущий урон вызывает кровотечение.
    pub fn apply_to(&mut self, part: Option<&str>, spec: &DamageSpec) {
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
            let local = !matches!(kind.as_str(), "Asphyxiation" | "Bloodloss");
            if *amount > 0.0 {
                if local && let Some(p) = part.filter(|p| PARTS.contains(p)) {
                    *self.parts.entry(p.to_string()).or_insert(0.0) += amount;
                }
                if matches!(kind.as_str(), "Slash" | "Piercing") {
                    self.bleed += amount * BLEED_PER_DAMAGE;
                }
            } else if local {
                self.heal_parts(-amount);
            }
        }
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

    /// Шаг времени: кровотечение теряет кровь и затухает, удушье растёт или проходит. Возвращает
    /// `true`, если что-то изменилось.
    pub fn tick(&mut self, dt: f32) -> bool {
        let mut changed = false;
        if self.bleed > 0.0 {
            let loss = self.bleed * BLOODLOSS_PER_BLEED * dt;
            self.bleed = (self.bleed - BLEED_DECAY_PER_S * dt).max(0.0);
            if self.state() != MobState::Dead {
                let spec = DamageSpec::from([("Bloodloss".to_string(), loss)]);
                self.apply_to(None, &spec);
            }
            changed = true;
        }
        let air = self.damage_of("Asphyxiation");
        if self.state() != MobState::Dead {
            if self.suffocating {
                let spec = DamageSpec::from([("Asphyxiation".to_string(), SUFFOCATION_PER_S * dt)]);
                self.apply_to(None, &spec);
                changed = true;
            } else if air > 0.0 {
                let spec = DamageSpec::from([(
                    "Asphyxiation".to_string(),
                    -BREATHING_RECOVERY_PER_S * dt,
                )]);
                self.apply_to(None, &spec);
                changed = true;
            }
        }
        changed
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
    pub bleed: f32,
    #[serde(default)]
    pub suffocating: bool,
}

pub const HEALTH_COMPONENT: &str = "build.health";
pub const HEALTH_MAX_BYTES: usize = 1024;
/// Состояние здоровья для всех: один байт (`MobState::to_byte`).
pub const MOBSTATE_COMPONENT: &str = "build.mobstate";

impl Health {
    pub fn view(&self) -> HealthView {
        HealthView {
            damage: self.damage.clone(),
            state: self.state(),
            parts: self.parts.clone(),
            bleed: self.bleed,
            suffocating: self.suffocating,
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
    /// На сколько применение уменьшает кровотечение.
    pub stop_bleed: f32,
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

    #[test]
    fn cuts_bleed_the_bleeding_drains_and_fades_and_a_bandage_stops_it() {
        let mut h = Health::default();
        h.apply_to(Some("Torso"), &spec(&[("Slash", 20.0)]));
        assert!((h.bleed() - 3.0).abs() < 1e-5);
        for _ in 0..20 {
            h.tick(0.05);
        }
        assert!(h.damage_of("Bloodloss") > 2.5, "за секунду теряется кровь");
        assert!(h.bleed() < 3.0, "кровотечение затухает");
        h.stop_bleed(10.0);
        let before = h.damage_of("Bloodloss");
        h.tick(1.0);
        assert_eq!(h.damage_of("Bloodloss"), before);
        // Тупой урон не кровоточит.
        let mut b = Health::default();
        b.apply_to(Some("Torso"), &spec(&[("Blunt", 50.0)]));
        assert_eq!(b.bleed(), 0.0);
    }

    #[test]
    fn suffocation_grows_without_air_and_passes_with_it() {
        let mut h = Health {
            suffocating: true,
            ..Health::default()
        };
        for _ in 0..20 {
            h.tick(0.1);
        }
        assert!((h.damage_of("Asphyxiation") - 2.0).abs() < 1e-4);
        h.suffocating = false;
        for _ in 0..30 {
            h.tick(0.1);
        }
        assert_eq!(h.damage_of("Asphyxiation"), 0.0);
    }
}
