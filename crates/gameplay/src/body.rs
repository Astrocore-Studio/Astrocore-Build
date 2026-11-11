//! Физиология персонажа по образцу SS14: дыхание (`RespiratorSystem`, `LungSystem`), метаболизм
//! (`MetabolizerSystem`), сытость (`SatiationSystem`), баротравма (`BarotraumaSystem`).
//!
//! Числа и порядок действий — из исходников SS14 (коммит `910154a7`):
//!
//! * дыхание: раз в 2 с насыщение падает на 2; вдох и выдох чередуются; вдох берёт 0,5 л воздуха
//!   из клетки, переводит газы в реагенты лёгких (1144 ед. на моль), раз в секунду лёгкие
//!   метаболизируют всё (кислород → насыщение +1 за единицу, углекислый газ в лёгких); выдох
//!   возвращает содержимое лёгких в клетку;
//! * насыщение от −2 до 5; ниже 0 — урон `Asphyxiation` 1 за каждые 2 с, выше — лечение 1;
//! * пищеварение: раз в секунду желудок метаболизирует до 3 реагентов (`Satiate`);
//! * сытость замедляет и ранит так же, как компоненты `SatiationSpeedModifier` и `SatiationDamage`:
//!   на «Starving» и «Parched» скорость 0,75 (`BaseHungerThirstSlowdown`); при нуле сытости раз в
//!   секунду идёт урон `Poison` (значение — выбор билда: у человека в прототипах SS14 этой версии
//!   `SatiationDamage` не задан);
//! * кровь (`BloodstreamSystem`): 600 ед., раз в 3 с восстанавливается на 1 ед. к норме; кровотечение
//!   (`BleedAmount`, до 10) уносит столько же крови и само слабеет на 0,33; урон даёт кровотечение
//!   по коэффициентам `BloodlossHuman` (тупой 0,08, колющий 0,2, режущий 0,25, ожог −0,5, остальное
//!   0); ниже 90% крови идёт урон `Bloodloss` 0,5 / (0,1 + доля), выше — лечение на долю;
//! * баротравма раз в секунду: давление ≤ 20 кПа — `Blunt 0,5 × 4` и `Heat 0,1 × 4`, ≥ 550 кПа —
//!   тот же урон с множителем `min((p/550 − 1) × 4, 4)`; предупреждения при 50 и 385 кПа.

use atmos::{BREATH_MOLES_TO_REAGENT_MULTIPLIER, BREATH_VOLUME};
use atmos::{
    GasMixture, HAZARD_HIGH_PRESSURE, HAZARD_LOW_PRESSURE, LOW_PRESSURE_DAMAGE,
    MAX_HIGH_PRESSURE_DAMAGE, NORMAL_BODY_TEMPERATURE, ONE_ATMOSPHERE, PRESSURE_DAMAGE_COEFFICIENT,
    WARNING_HIGH_PRESSURE, WARNING_LOW_PRESSURE,
};
use serde::{Deserialize, Serialize};

use crate::health::{DamageSpec, MobState};
use crate::reagents::{Effect, SatiationKind, Solution, Stage, gas_to_reagents, reagent};

/// Через сколько секунд обновляется дыхание (`UpdateInterval` дыхательной системы).
pub const RESPIRATION_INTERVAL_S: f32 = 2.0;
pub const SATURATION_MAX: f32 = 5.0;
pub const SATURATION_MIN: f32 = -2.0;
pub const SUFFOCATION_CYCLE_THRESHOLD: u32 = 3;
/// Шаг метаболизма, с (`MetabolizerComponent.UpdateInterval`).
pub const METABOLISM_INTERVAL_S: f32 = 1.0;
/// Сколько реагентов за шаг обрабатывает орган (`maxReagents`).
pub const MAX_REAGENTS_PER_STEP: usize = 3;
pub const LUNG_VOLUME_L: f32 = 6.0;
pub const LUNG_SOLUTION_MAX: f32 = 100.0;
pub const STOMACH_SOLUTION_MAX: f32 = 150.0;
/// Урон баротравмы в секунду на единицу множителя (`Barotrauma` в `species_base.yml`).
pub const BAROTRAUMA_BLUNT: f32 = 0.5;
pub const BAROTRAUMA_HEAT: f32 = 0.1;
/// Пока урон баротравмы не достигнет этого, давление продолжает ранить (`MaxDamage`).
pub const BAROTRAUMA_MAX_DAMAGE: f32 = 200.0;
pub const BAROTRAUMA_INTERVAL_S: f32 = 1.0;
/// Множитель скорости при «Starving» и «Parched» (`BaseHungerThirstSlowdown`).
pub const STARVING_SPEED: f32 = 0.75;
/// Урон от голода и жажды при нулевой сытости, раз в секунду (`SatiationDamageComponent.Frequency`).
pub const STARVATION_DAMAGE: f32 = 1.0;
pub const STARVATION_INTERVAL_S: f32 = 1.0;
/// Кровь (`BloodstreamComponent` и прототип `Bloodstream` у `BaseMob`).
pub const BLOOD_REFERENCE: f32 = 600.0;
pub const BLOOD_MAX_MODIFIER: f32 = 1.2;
pub const BLOOD_UPDATE_S: f32 = 3.0;
pub const BLOOD_REFRESH: f32 = 1.0;
pub const BLEED_REDUCTION: f32 = 0.33;
pub const MAX_BLEED: f32 = 10.0;
pub const BLOODLOSS_THRESHOLD: f32 = 0.9;
pub const BLOODLOSS_DAMAGE: f32 = 0.5;
pub const BLOODLOSS_HEAL: f32 = 1.0;
/// Коэффициенты `BloodlossHuman`: сколько кровотечения даёт единица урона данного вида.
pub fn bleed_coefficient(kind: &str) -> f32 {
    match kind {
        "Blunt" => 0.08,
        "Piercing" => 0.2,
        "Slash" => 0.25,
        "Heat" => -0.5,
        _ => 0.0,
    }
}

/// Одна величина сытости: параметры взяты из `NormalSatiationHunger` и `NormalSatiationThirst`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SatiationSpec {
    pub maximum: f32,
    pub base_rate: f32,
    /// Пороги по убыванию: (значение, модификатор скорости, номер предупреждения 0/1/2).
    pub levels: [(f32, f32, u8); 4],
    pub start_min: f32,
    pub start_max: f32,
}

pub const HUNGER: SatiationSpec = SatiationSpec {
    maximum: 200.0,
    base_rate: -0.016_666_667,
    // Overfed 200, Okay 150, Peckish 100 (предупреждение 1), Starving 50 (предупреждение 2).
    levels: [
        (200.0, 1.2, 0),
        (150.0, 1.0, 0),
        (100.0, 0.8, 1),
        (50.0, 0.6, 2),
    ],
    start_min: 100.0,
    start_max: 150.0,
};

pub const THIRST: SatiationSpec = SatiationSpec {
    maximum: 600.0,
    base_rate: -0.1,
    // Overhydrated 600, Okay 450, Thirsty 300 (1), Parched 150 (2).
    levels: [
        (600.0, 1.2, 0),
        (450.0, 1.0, 0),
        (300.0, 0.8, 1),
        (150.0, 0.6, 2),
    ],
    start_min: 300.0,
    start_max: 450.0,
};

impl SatiationSpec {
    /// Порог, в который попало значение (`TryGetValueByThreshold`): значение между двумя порогами
    /// относится к верхнему; выше всех порогов — ни к одному.
    fn level(&self, value: f32) -> Option<(f32, f32, u8)> {
        if value > self.levels[0].0 {
            return None;
        }
        let mut upper = self.levels[0];
        for lower in &self.levels[1..] {
            if value > lower.0 {
                return Some(upper);
            }
            upper = *lower;
        }
        Some(upper)
    }

    pub fn modifier(&self, value: f32) -> f32 {
        self.level(value).map_or(1.0, |l| l.1)
    }

    /// Предупреждение: 0 — нет, 1 — «хочется есть/пить», 2 — «голод/жажда».
    pub fn alert(&self, value: f32) -> u8 {
        self.level(value).map_or(0, |l| l.2)
    }

    pub fn start_value(&self, seed: u32) -> f32 {
        let t = (seed.wrapping_mul(2_654_435_761) >> 8) as f32 / 16_777_216.0;
        self.start_min + (self.start_max - self.start_min) * t
    }
}

/// Давление для предупреждений: уровень и сторона.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PressureAlert {
    #[default]
    None,
    Low(u8),
    High(u8),
}

/// Что видит игрок о своём теле (то, что показывают алерты SS14).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Vitals {
    pub hunger: f32,
    pub thirst: f32,
    pub hunger_alert: u8,
    pub thirst_alert: u8,
    pub pressure_alert: PressureAlert,
    pub pressure_kpa: f32,
    /// Кислородное голодание: после трёх тактов удушья (`LowOxygen`).
    pub low_oxygen: bool,
    pub saturation: f32,
    /// Множитель скорости ходьбы от голода и жажды.
    #[serde(default = "one")]
    pub speed: f32,
    /// Доля крови от нормы (1,0 — норма) и сила кровотечения.
    #[serde(default = "one")]
    pub blood: f32,
    #[serde(default)]
    pub bleed: f32,
}

fn one() -> f32 {
    1.0
}

pub const VITALS_COMPONENT: &str = "build.vitals";
pub const VITALS_MAX_BYTES: usize = 512;

pub fn encode_vitals(v: &Vitals) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_vitals(bytes: &[u8]) -> Option<Vitals> {
    if bytes.len() > VITALS_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Среда вокруг персонажа: откуда брать воздух и куда его возвращать.
pub trait Environment {
    /// Взять `litres` воздуха из клетки; `None`, если дышать нечем (клетки нет).
    fn take_breath(&mut self, litres: f32) -> Option<GasMixture>;
    /// Вернуть воздух (выдох или то, что не усвоено).
    fn give_back(&mut self, gas: &GasMixture);
    /// Давление в клетке, кПа.
    fn pressure(&self) -> f32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Breath {
    Inhaling,
    Exhaling,
}

/// Результат шага: накопленный урон (отрицательные числа — лечение).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Outcome {
    pub damage: DamageSpec,
    /// Вид и число изменений здоровья, которые надо перерассылать.
    pub changed: bool,
}

impl Outcome {
    fn add(&mut self, kind: &str, amount: f32) {
        *self.damage.entry(kind.to_string()).or_insert(0.0) += amount;
        self.changed = true;
    }
}

/// Тело: лёгкие, желудок, сытость и таймеры.
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    pub saturation: f32,
    breath: Breath,
    respiration_timer: f32,
    suffocation_cycles: u32,
    lung_air: GasMixture,
    lung: Solution,
    lung_timer: f32,
    pub stomach: Solution,
    stomach_timer: f32,
    pub hunger: f32,
    pub thirst: f32,
    barotrauma_timer: f32,
    pub pressure_alert: PressureAlert,
    pub pressure_kpa: f32,
    pub low_oxygen: bool,
    starvation_timer: f32,
    blood: f32,
    bleed: f32,
    blood_timer: f32,
    rng: u32,
}

impl Body {
    pub fn new(seed: u32) -> Self {
        let mut lung_air = GasMixture::new(LUNG_VOLUME_L);
        lung_air.set_temperature(NORMAL_BODY_TEMPERATURE + 273.15);
        Self {
            saturation: SATURATION_MAX,
            breath: Breath::Inhaling,
            respiration_timer: RESPIRATION_INTERVAL_S,
            suffocation_cycles: 0,
            lung_air,
            lung: Solution::new(LUNG_SOLUTION_MAX),
            lung_timer: METABOLISM_INTERVAL_S,
            stomach: Solution::new(STOMACH_SOLUTION_MAX),
            stomach_timer: METABOLISM_INTERVAL_S,
            hunger: HUNGER.start_value(seed),
            thirst: THIRST.start_value(seed ^ 0x9e37_79b9),
            barotrauma_timer: BAROTRAUMA_INTERVAL_S,
            pressure_alert: PressureAlert::None,
            pressure_kpa: ONE_ATMOSPHERE,
            low_oxygen: false,
            starvation_timer: STARVATION_INTERVAL_S,
            blood: BLOOD_REFERENCE,
            bleed: 0.0,
            blood_timer: BLOOD_UPDATE_S,
            rng: seed | 1,
        }
    }

    pub fn vitals(&self) -> Vitals {
        Vitals {
            hunger: self.hunger,
            thirst: self.thirst,
            hunger_alert: HUNGER.alert(self.hunger),
            thirst_alert: THIRST.alert(self.thirst),
            pressure_alert: self.pressure_alert,
            pressure_kpa: self.pressure_kpa,
            low_oxygen: self.low_oxygen,
            saturation: self.saturation,
            speed: self.speed(),
            blood: self.blood / BLOOD_REFERENCE,
            bleed: self.bleed,
        }
    }

    /// `TryModifyBleedAmount`: изменить силу кровотечения, не выходя за 0…10.
    pub fn modify_bleed(&mut self, amount: f32) {
        self.bleed = (self.bleed + amount).clamp(0.0, MAX_BLEED);
    }

    /// `TryModifyBloodLevel`: добавить или убавить крови; добавлять можно только до нормы.
    pub fn modify_blood(&mut self, amount: f32) {
        self.blood = if amount > 0.0 {
            (self.blood + amount).min(self.blood.max(BLOOD_REFERENCE))
        } else {
            (self.blood + amount).max(0.0)
        };
    }

    /// `BloodstreamSystem.OnDamageChanged`: полученный урон открывает кровотечение; сильный удар
    /// сразу выбивает часть крови (вероятность — доля урона к 25, потеря — пятая часть).
    pub fn on_damage(&mut self, dealt: &DamageSpec) {
        let total: f32 = dealt.iter().map(|(k, v)| bleed_coefficient(k) * v).sum();
        if total == 0.0 {
            return;
        }
        self.modify_bleed(total);
        if total > 0.0 && self.random() < (total / 25.0).clamp(0.0, 1.0) {
            self.blood = (self.blood - total / 5.0).max(0.0);
        }
    }

    fn tick_bleed(&mut self) {
        if self.bleed <= 0.0 {
            return;
        }
        self.blood = (self.blood - self.bleed).max(0.0);
        self.modify_bleed(-BLEED_REDUCTION);
    }

    /// Раз в 3 секунды: кровь тянется к норме, кровотечение уносит её, малокровие ранит.
    fn bloodstream(&mut self, mob: MobState, out: &mut Outcome) {
        if mob == MobState::Dead {
            self.tick_bleed();
            return;
        }
        if self.blood < BLOOD_REFERENCE {
            self.blood = (self.blood + BLOOD_REFRESH).min(BLOOD_REFERENCE);
        } else if self.blood > BLOOD_REFERENCE {
            self.blood = (self.blood - BLOOD_REFRESH).max(BLOOD_REFERENCE);
        }
        self.tick_bleed();
        let level = (self.blood / BLOOD_REFERENCE).min(BLOOD_MAX_MODIFIER);
        if level < BLOODLOSS_THRESHOLD {
            out.add("Bloodloss", BLOODLOSS_DAMAGE / (0.1 + level));
        } else {
            out.add("Bloodloss", -BLOODLOSS_HEAL * level);
        }
        out.changed = true;
    }

    /// Множитель скорости: каждая сытость на «Starving»/«Parched» даёт 0,75, множители перемножаются.
    pub fn speed(&self) -> f32 {
        let slow = |v: f32, spec: &SatiationSpec| {
            if spec.alert(v) == 2 {
                STARVING_SPEED
            } else {
                1.0
            }
        };
        slow(self.hunger, &HUNGER) * slow(self.thirst, &THIRST)
    }

    fn random(&mut self) -> f32 {
        // xorshift32: достаточно для вероятностей эффектов.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / 16_777_216.0
    }

    /// Съеденное попадает в желудок; сверх его объёма не принимается. Возвращает непринятое.
    pub fn swallow(&mut self, food: &Solution) -> Solution {
        self.stomach.merge(food)
    }

    /// Шаг времени. `mob` — состояние здоровья, `barotrauma_damage` — накопленный урон от давления
    /// (`Blunt` + `Heat`), чтобы честно остановиться на пределе.
    pub fn tick(
        &mut self,
        dt: f32,
        env: &mut dyn Environment,
        mob: MobState,
        barotrauma_damage: f32,
    ) -> Outcome {
        let mut out = Outcome::default();
        self.tick_satiation(dt, mob);

        self.respiration_timer -= dt;
        while self.respiration_timer <= 0.0 {
            self.respiration_timer += RESPIRATION_INTERVAL_S;
            self.respirate(env, mob, &mut out);
        }
        self.lung_timer -= dt;
        while self.lung_timer <= 0.0 {
            self.lung_timer += METABOLISM_INTERVAL_S;
            self.metabolize(Stage::Respiration, mob, &mut out);
        }
        self.stomach_timer -= dt;
        while self.stomach_timer <= 0.0 {
            self.stomach_timer += METABOLISM_INTERVAL_S;
            self.metabolize(Stage::Digestion, mob, &mut out);
        }
        self.blood_timer -= dt;
        while self.blood_timer <= 0.0 {
            self.blood_timer += BLOOD_UPDATE_S;
            self.bloodstream(mob, &mut out);
        }
        self.starvation_timer -= dt;
        while self.starvation_timer <= 0.0 {
            self.starvation_timer += STARVATION_INTERVAL_S;
            if mob != MobState::Dead {
                for v in [self.hunger, self.thirst] {
                    if v <= 0.0 {
                        out.add("Poison", STARVATION_DAMAGE);
                    }
                }
            }
        }
        self.barotrauma_timer -= dt;
        while self.barotrauma_timer <= 0.0 {
            self.barotrauma_timer += BAROTRAUMA_INTERVAL_S;
            self.barotrauma(env, mob, barotrauma_damage, &mut out);
        }
        out
    }

    fn tick_satiation(&mut self, dt: f32, mob: MobState) {
        if mob == MobState::Dead {
            return;
        }
        for (value, spec) in [(&mut self.hunger, &HUNGER), (&mut self.thirst, &THIRST)] {
            let rate = spec.base_rate * spec.modifier(*value);
            *value = (*value + rate * dt).clamp(0.0, spec.maximum);
        }
    }

    /// `RespiratorSystem.Update` для одного срабатывания.
    fn respirate(&mut self, env: &mut dyn Environment, mob: MobState, out: &mut Outcome) {
        if mob == MobState::Dead {
            return;
        }
        self.saturation =
            (self.saturation - RESPIRATION_INTERVAL_S).clamp(SATURATION_MIN, SATURATION_MAX);
        // В критическом состоянии не дышат.
        if mob == MobState::Alive {
            match self.breath {
                Breath::Inhaling => {
                    self.inhale(env);
                    self.breath = Breath::Exhaling;
                }
                Breath::Exhaling => {
                    self.exhale(env);
                    self.breath = Breath::Inhaling;
                }
            }
        }
        if self.saturation < 0.0 {
            out.add("Asphyxiation", 1.0);
            if self.suffocation_cycles >= SUFFOCATION_CYCLE_THRESHOLD {
                self.low_oxygen = true;
            }
            self.suffocation_cycles += 1;
        } else {
            out.add("Asphyxiation", -1.0);
            self.suffocation_cycles = 0;
            self.low_oxygen = false;
        }
    }

    fn inhale(&mut self, env: &mut dyn Environment) {
        let Some(gas) = env.take_breath(BREATH_VOLUME) else {
            return;
        };
        self.lung_air.merge(&gas);
        gas_to_reagents(&gas, &mut self.lung);
    }

    fn exhale(&mut self, env: &mut dyn Environment) {
        env.give_back(&self.lung_air);
        self.lung_air.clear();
        self.lung.contents.clear();
    }

    /// Метаболизм одной стадии: лёгкие обрабатывают всё сразу, желудок — по скорости реагента.
    fn metabolize(&mut self, stage: Stage, mob: MobState, out: &mut Outcome) {
        let mut names: Vec<String> = match stage {
            Stage::Respiration => self.lung.contents.keys().cloned().collect(),
            Stage::Digestion => self.stomach.contents.keys().cloned().collect(),
        };
        // Случайный порядок, чтобы не зависеть от алфавита (как в SS14).
        for i in (1..names.len()).rev() {
            let j = (self.random() * (i + 1) as f32) as usize;
            names.swap(i, j.min(i));
        }
        let mut processed = 0;
        for name in names {
            let quantity = match stage {
                Stage::Respiration => self.lung.amount(&name),
                Stage::Digestion => self.stomach.amount(&name),
            };
            let entry =
                reagent(&name).and_then(|r| r.metabolisms.iter().find(|m| m.stage == stage));
            let Some(entry) = entry else {
                // Нет метаболизма на этой стадии: лёгкие всё равно очищаются при выдохе, желудок
                // держит такое вещество (как и любой нерастворимый остаток).
                continue;
            };
            let rate = if stage == Stage::Respiration {
                quantity
            } else {
                entry.rate
            };
            let to_remove = rate.clamp(0.0, quantity);
            if processed >= MAX_REAGENTS_PER_STEP {
                break;
            }
            let scale = if stage == Stage::Respiration {
                to_remove
            } else {
                to_remove / rate
            };
            if mob != MobState::Dead {
                for effect in entry.effects {
                    self.apply_effect(effect, scale, stage, out);
                }
            }
            if to_remove > 0.0 {
                match stage {
                    Stage::Respiration => self.lung.remove(&name, to_remove),
                    Stage::Digestion => self.stomach.remove(&name, to_remove),
                }
                processed += 1;
            }
        }
    }

    fn apply_effect(&mut self, effect: &Effect, scale: f32, stage: Stage, out: &mut Outcome) {
        match effect {
            Effect::Oxygenate { factor } => {
                self.saturation =
                    (self.saturation + scale * factor).clamp(SATURATION_MIN, SATURATION_MAX);
            }
            Effect::ModifyLungGas(ratios) => {
                for (gas, ratio) in ratios.iter() {
                    let mut q = ratio * scale / BREATH_MOLES_TO_REAGENT_MULTIPLIER;
                    if q < 0.0 {
                        q = q.max(-self.lung_air.moles(*gas));
                    }
                    self.lung_air.adjust_moles(*gas, q);
                }
            }
            Effect::HealthChange { damage, min_scale } => {
                if scale < *min_scale {
                    return;
                }
                for (kind, amount) in *damage {
                    out.add(kind, amount * scale);
                }
            }
            Effect::EvenHealthChange {
                group,
                amount,
                probability,
            } => {
                if self.random() >= *probability {
                    return;
                }
                let each = amount * scale / group.len() as f32;
                for kind in *group {
                    out.add(kind, each);
                }
            }
            Effect::ModifyBleed(amount) => self.modify_bleed(amount * scale),
            Effect::ModifyBloodLevel(amount) => self.modify_blood(amount * scale),
            Effect::Satiate {
                kind,
                factor,
                needs,
            } => {
                if let Some((reagent, min)) = needs {
                    let have = match stage {
                        Stage::Digestion => self.stomach.amount(reagent),
                        Stage::Respiration => self.lung.amount(reagent),
                    };
                    if have < *min {
                        return;
                    }
                }
                let (value, spec) = match kind {
                    SatiationKind::Hunger => (&mut self.hunger, &HUNGER),
                    SatiationKind::Thirst => (&mut self.thirst, &THIRST),
                };
                *value = (*value + factor * scale).clamp(0.0, spec.maximum);
            }
        }
    }

    /// `BarotraumaSystem.Update` для одного срабатывания.
    fn barotrauma(
        &mut self,
        env: &mut dyn Environment,
        mob: MobState,
        taken: f32,
        out: &mut Outcome,
    ) {
        let pressure = env.pressure().max(1.0);
        self.pressure_kpa = pressure;
        if taken >= BAROTRAUMA_MAX_DAMAGE || mob == MobState::Dead {
            return;
        }
        // Защитной экипировки у билда нет: ощущаемое давление равно давлению среды, но «низкое»
        // не выше атмосферы, «высокое» не ниже (`GetFeltLowPressure`/`GetFeltHighPressure`).
        let felt = if pressure <= WARNING_LOW_PRESSURE {
            pressure.min(ONE_ATMOSPHERE)
        } else if pressure >= WARNING_HIGH_PRESSURE {
            pressure.max(ONE_ATMOSPHERE)
        } else {
            pressure
        };
        let previous = self.pressure_alert;
        if felt <= HAZARD_LOW_PRESSURE {
            let k = LOW_PRESSURE_DAMAGE;
            out.add("Blunt", BAROTRAUMA_BLUNT * k);
            out.add("Heat", BAROTRAUMA_HEAT * k);
            self.pressure_alert = PressureAlert::Low(2);
        } else if felt >= HAZARD_HIGH_PRESSURE {
            let k = (((felt / HAZARD_HIGH_PRESSURE) - 1.0) * PRESSURE_DAMAGE_COEFFICIENT)
                .min(MAX_HIGH_PRESSURE_DAMAGE);
            out.add("Blunt", BAROTRAUMA_BLUNT * k);
            out.add("Heat", BAROTRAUMA_HEAT * k);
            self.pressure_alert = PressureAlert::High(2);
        } else {
            self.pressure_alert = if felt <= WARNING_LOW_PRESSURE {
                PressureAlert::Low(1)
            } else if felt >= WARNING_HIGH_PRESSURE {
                PressureAlert::High(1)
            } else {
                PressureAlert::None
            };
        }
        if self.pressure_alert != previous {
            out.changed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atmos::{CellKind, Grid, Layout, edge};

    /// Простая среда на одной клетке сетки.
    struct Cell<'a>(&'a mut Grid, (i32, i32));

    impl Environment for Cell<'_> {
        fn take_breath(&mut self, litres: f32) -> Option<GasMixture> {
            self.0.remove_volume(self.1, litres)
        }
        fn give_back(&mut self, gas: &GasMixture) {
            self.0.merge_into(self.1, gas);
        }
        fn pressure(&self) -> f32 {
            self.0.pressure(self.1)
        }
    }

    fn sealed(w: i32, h: i32) -> Grid {
        let mut l = Layout::default();
        for x in 0..w {
            for z in 0..h {
                l.cells.insert((x, z), CellKind::Air);
            }
        }
        for x in 0..w {
            l.blocked.insert(edge((x, 0), (x, -1)));
            l.blocked.insert(edge((x, h - 1), (x, h)));
        }
        for z in 0..h {
            l.blocked.insert(edge((0, z), (-1, z)));
            l.blocked.insert(edge((w - 1, z), (w, z)));
        }
        let mut g = Grid::new();
        g.set_layout(l);
        g
    }

    fn run(body: &mut Body, grid: &mut Grid, seconds: f32) -> DamageSpec {
        let mut total = DamageSpec::new();
        let mut t = 0.0;
        while t < seconds {
            let out = body.tick(0.1, &mut Cell(grid, (0, 0)), MobState::Alive, 0.0);
            for (k, v) in out.damage {
                *total.entry(k).or_insert(0.0) += v;
            }
            t += 0.1;
        }
        total
    }

    #[test]
    fn a_person_breathes_station_air_forever_without_harm() {
        let mut g = sealed(1, 1);
        let mut b = Body::new(7);
        let dmg = run(&mut b, &mut g, 120.0);
        assert!(dmg.get("Asphyxiation").copied().unwrap_or(0.0) <= 0.0);
        // Насыщение качается между 1 и 5: на вдох +5, а каждые 2 с уходит 2 (как в SS14).
        assert!(b.saturation >= 1.0, "{}", b.saturation);
        assert!(!b.low_oxygen);
        assert_eq!(b.pressure_alert, PressureAlert::None);
    }

    #[test]
    fn breathing_turns_oxygen_into_carbon_dioxide_in_a_sealed_cell() {
        let mut g = sealed(1, 1);
        let o2 = g.air((0, 0)).moles(atmos::Gas::Oxygen);
        let mut b = Body::new(3);
        run(&mut b, &mut g, 120.0);
        let air = g.air((0, 0));
        assert!(
            air.moles(atmos::Gas::CarbonDioxide) > 0.01,
            "{}",
            air.moles(atmos::Gas::CarbonDioxide)
        );
        assert!(air.moles(atmos::Gas::Oxygen) < o2);
        // Один моль кислорода уходит в один моль углекислого газа: всего молей не меньше.
        assert!((air.total_moles() - atmos::MOLES_CELL_STANDARD).abs() < 0.5);
    }

    #[test]
    fn without_air_the_person_suffocates_on_the_ss14_schedule() {
        let mut g = sealed(1, 1);
        // Откачиваем воздух.
        g.remove_volume((0, 0), atmos::CELL_VOLUME);
        let mut b = Body::new(1);
        let early = run(&mut b, &mut g, 5.0);
        assert!(
            early.get("Asphyxiation").copied().unwrap_or(0.0) <= 0.0,
            "5 → 3 → 1 пока не страшно"
        );
        let later = run(&mut b, &mut g, 20.0);
        let a = later.get("Asphyxiation").copied().unwrap_or(0.0);
        assert!(a >= 8.0, "после трёх тактов — по 1 за каждые 2 с: {a}");
        assert!(b.low_oxygen, "после трёх тактов появляется предупреждение");
    }

    #[test]
    fn a_vacuum_hurts_by_barotrauma_every_second() {
        let mut g = sealed(1, 1);
        g.remove_volume((0, 0), atmos::CELL_VOLUME);
        let mut b = Body::new(1);
        let dmg = run(&mut b, &mut g, 10.0);
        let blunt = dmg.get("Blunt").copied().unwrap_or(0.0);
        let heat = dmg.get("Heat").copied().unwrap_or(0.0);
        assert!((blunt - 20.0).abs() < 2.5, "0,5 × 4 в секунду: {blunt}");
        assert!((heat - 4.0).abs() < 0.6, "{heat}");
        assert_eq!(b.pressure_alert, PressureAlert::Low(2));
    }

    #[test]
    fn high_pressure_scales_with_pressure_and_the_warning_comes_first() {
        let mut g = sealed(1, 1);
        let mut extra = GasMixture::standard_air();
        extra.multiply(9.0); // 10 атмосфер
        g.merge_into((0, 0), &extra);
        let mut b = Body::new(1);
        let dmg = run(&mut b, &mut g, 10.0);
        assert!(dmg.get("Blunt").copied().unwrap_or(0.0) > 10.0);
        assert_eq!(b.pressure_alert, PressureAlert::High(2));
        // Умеренное давление (4 атм ≈ 405 кПа): только предупреждение.
        let mut g = sealed(1, 1);
        let mut more = GasMixture::standard_air();
        more.multiply(3.0);
        g.merge_into((0, 0), &more);
        let mut b = Body::new(1);
        let dmg = run(&mut b, &mut g, 5.0);
        assert!(!dmg.contains_key("Blunt"));
        assert_eq!(b.pressure_alert, PressureAlert::High(1));
    }

    #[test]
    fn hunger_and_thirst_fall_by_the_ss14_rates_and_levels_match_the_thresholds() {
        let mut g = sealed(1, 1);
        let mut b = Body::new(5);
        b.hunger = 149.0;
        b.thirst = 449.0;
        run(&mut b, &mut g, 60.0);
        // Okay: модификатор 1,0 — −1 в минуту голод; жажда −6 в минуту.
        assert!((149.0 - b.hunger - 1.0).abs() < 0.2, "{}", b.hunger);
        assert!((449.0 - b.thirst - 6.0).abs() < 0.5, "{}", b.thirst);
        assert_eq!(HUNGER.alert(120.0), 0);
        assert_eq!(HUNGER.alert(100.0), 1);
        assert_eq!(HUNGER.alert(60.0), 1);
        assert_eq!(HUNGER.alert(50.0), 2);
        assert_eq!(THIRST.alert(300.0), 1);
        assert_eq!(THIRST.alert(150.0), 2);
        assert_eq!(HUNGER.modifier(190.0), 1.2);
        assert_eq!(HUNGER.modifier(10.0), 0.6);
        assert!((HUNGER.start_value(1)..=HUNGER.start_max).contains(&HUNGER.start_value(1)));
    }

    #[test]
    fn cuts_bleed_the_blood_drains_slowly_and_low_blood_hurts() {
        let mut g = sealed(1, 1);
        let mut b = Body::new(9);
        // Режущий удар в 20: кровотечение 5 (0,25 × 20); сильный удар может выбить часть крови сразу.
        b.on_damage(&DamageSpec::from([("Slash".to_string(), 20.0)]));
        assert!((b.bleed - 5.0).abs() < 1e-5);
        let start = b.blood;
        run(&mut b, &mut g, 9.0);
        assert!(
            b.blood < start - 8.0,
            "кровь уходит: {} → {}",
            start,
            b.blood
        );
        assert!(b.bleed < 5.0, "кровотечение слабеет на 0,33 за такт");
        // Ожог прижигает.
        let mut c = Body::new(9);
        c.modify_bleed(2.0);
        c.on_damage(&DamageSpec::from([("Heat".to_string(), 10.0)]));
        assert!(
            c.bleed < 0.01,
            "10 ожога × −0,5 снимают кровотечение: {}",
            c.bleed
        );
        // Тупой урон почти не кровоточит.
        let mut d = Body::new(9);
        d.on_damage(&DamageSpec::from([("Blunt".to_string(), 50.0)]));
        assert!((d.bleed - 4.0).abs() < 1e-5);
        // Малокровие: ниже 90% идёт Bloodloss 0,5 / (0,1 + доля).
        let mut e = Body::new(9);
        e.blood = 0.5 * BLOOD_REFERENCE;
        let dmg = run(&mut e, &mut g, 3.1);
        let loss = dmg.get("Bloodloss").copied().unwrap_or(0.0);
        assert!((loss - 0.5 / 0.6).abs() < 0.05, "{loss}");
        // В норме идёт лечение Bloodloss.
        let mut f = Body::new(9);
        let dmg = run(&mut f, &mut g, 3.1);
        assert!(dmg.get("Bloodloss").copied().unwrap_or(0.0) < 0.0);
    }

    #[test]
    fn protein_restores_blood_and_vitamin_slows_bleeding() {
        let mut g = sealed(1, 1);
        let mut b = Body::new(9);
        b.blood = 500.0;
        b.modify_bleed(3.0);
        let mut meal = Solution::new(100.0);
        meal.add("Protein", 4.0);
        meal.add("Vitamin", 4.0);
        b.swallow(&meal);
        run(&mut b, &mut g, 10.0);
        assert!(b.stomach.is_empty());
        // Белок добавляет по единице крови за единицу переваренного (4 ед.), кровотечение же уносит больше.
        assert!(b.bleed < 3.0 - 0.33 * 3.0 + 0.1, "{}", b.bleed);
    }

    #[test]
    fn starving_slows_you_down_and_zero_satiation_poisons() {
        let mut g = sealed(1, 1);
        let mut b = Body::new(5);
        b.hunger = 120.0;
        b.thirst = 400.0;
        assert_eq!(b.speed(), 1.0);
        b.hunger = 50.0;
        assert_eq!(b.speed(), STARVING_SPEED);
        b.thirst = 150.0;
        assert!(
            (b.speed() - 0.5625).abs() < 1e-6,
            "голод и жажда перемножаются"
        );
        b.hunger = 0.0;
        b.thirst = 0.0;
        let dmg = run(&mut b, &mut g, 5.0);
        assert!(
            (dmg.get("Poison").copied().unwrap_or(0.0) - 10.0).abs() < 2.1,
            "{dmg:?}"
        );
    }

    #[test]
    fn eating_digests_nutriment_into_hunger_and_water_into_thirst() {
        let mut g = sealed(1, 1);
        let mut b = Body::new(5);
        b.hunger = 60.0;
        b.thirst = 200.0;
        let mut bite = Solution::new(100.0);
        bite.add("Nutriment", 3.0);
        bite.add("Protein", 1.0);
        bite.add("Water", 5.0);
        assert!(b.swallow(&bite).is_empty());
        let drained = run(&mut b, &mut g, 30.0);
        assert!(b.stomach.is_empty(), "всё переварено: {:?}", b.stomach);
        // Питательное вещество: 3 ед. при скорости 1 → 3 с × 3 = 9; белок 1 ед. → 2 с × 1,5 = 3.
        assert!(b.hunger > 60.0 + 9.0, "{}", b.hunger);
        // Вода: 5 ед. при скорости 0,5 → 10 с × 4 = 40.
        assert!(b.thirst > 200.0 + 40.0 - 4.0, "{}", b.thirst);
        let _ = drained;
    }

    #[test]
    fn a_full_stomach_takes_no_more() {
        let mut b = Body::new(1);
        let mut big = Solution::new(1000.0);
        big.add("Water", 400.0);
        let rest = b.swallow(&big);
        assert!((b.stomach.volume() - STOMACH_SOLUTION_MAX).abs() < 1e-3);
        assert!((rest.volume() - 250.0).abs() < 1e-2);
    }
}
