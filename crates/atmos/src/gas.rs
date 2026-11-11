//! Газы и газовые смеси (`Content.Shared/Atmos`: `Atmospherics.cs`, `GasMixture.cs`).
//!
//! Константы и формулы взяты из SS14 дословно; значения сверены с `Atmospherics.cs` и
//! `Resources/Prototypes/Atmospherics/gases.yml` (коммит `910154a7`).

/// Универсальная газовая постоянная, кПа·л/(К·моль).
pub const R: f32 = 8.314_463;
pub const ONE_ATMOSPHERE: f32 = 101.325;
/// Температура космического микроволнового фона, К: ниже смесь остыть не может.
pub const TCMB: f32 = 2.7;
pub const T0C: f32 = 273.15;
pub const T20C: f32 = 293.15;
pub const TMAX: f32 = 262_144.0;
/// Объём клетки, л (плитка 1 × 1 м на 2,5 м высоты).
pub const CELL_VOLUME: f32 = 2500.0;
/// Объём одного вдоха, л.
pub const BREATH_VOLUME: f32 = 0.5;
pub const MOLES_CELL_STANDARD: f32 = ONE_ATMOSPHERE * CELL_VOLUME / (T20C * R);
pub const OXYGEN_STANDARD: f32 = 0.21;
pub const NITROGEN_STANDARD: f32 = 0.79;
pub const GAS_MIN_MOLES: f32 = 0.000_000_05;
pub const OPEN_HEAT_TRANSFER_COEFFICIENT: f32 = 0.4;
pub const MINIMUM_AIR_RATIO_TO_SUSPEND: f32 = 0.1;
pub const MINIMUM_AIR_RATIO_TO_MOVE: f32 = 0.001;
pub const MINIMUM_AIR_TO_SUSPEND: f32 = MOLES_CELL_STANDARD * MINIMUM_AIR_RATIO_TO_SUSPEND;
pub const MINIMUM_TEMPERATURE_TO_MOVE: f32 = 5.0;
pub const MINIMUM_MOLES_DELTA_TO_MOVE: f32 = MOLES_CELL_STANDARD * MINIMUM_AIR_RATIO_TO_MOVE;
pub const MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND: f32 = 4.0;
pub const MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER: f32 = 0.01;
pub const MINIMUM_HEAT_CAPACITY: f32 = 0.0003;
pub const SPACE_HEAT_CAPACITY: f32 = 7000.0;
/// Опасное давление сверху и снизу, кПа, и пороги предупреждения.
pub const HAZARD_HIGH_PRESSURE: f32 = 550.0;
pub const WARNING_HIGH_PRESSURE: f32 = 0.7 * HAZARD_HIGH_PRESSURE;
pub const HAZARD_LOW_PRESSURE: f32 = 20.0;
pub const WARNING_LOW_PRESSURE: f32 = 2.5 * HAZARD_LOW_PRESSURE;
pub const PRESSURE_DAMAGE_COEFFICIENT: f32 = 4.0;
pub const MAX_HIGH_PRESSURE_DAMAGE: f32 = 4.0;
pub const LOW_PRESSURE_DAMAGE: f32 = 4.0;
pub const NORMAL_BODY_TEMPERATURE: f32 = 37.0;
/// Сколько единиц реагента даёт моль газа при вдохе.
pub const BREATH_MOLES_TO_REAGENT_MULTIPLIER: f32 = 1144.0;
/// Масштаб теплоёмкости (`atmos.heat_scale` по умолчанию).
pub const HEAT_SCALE: f32 = 8.0;
pub const EXCITED_GROUP_BREAKDOWN_CYCLES: u32 = 4;
pub const EXCITED_GROUPS_DISMANTLE_CYCLES: u32 = 16;

/// Порог «есть топливо / окислитель» (`Atmospherics.Epsilon`).
pub const GAS_EPSILON: f32 = 0.5;
/// Огонь (`Atmospherics.cs`).
pub const FIRE_MINIMUM_TEMPERATURE_TO_EXIST: f32 = T0C + 100.0;
pub const FIRE_MINIMUM_TEMPERATURE_TO_SPREAD: f32 = T0C + 150.0;
pub const FIRE_SPREAD_RADIOSITY_SCALE: f32 = 0.85;
pub const FIRE_PLASMA_ENERGY_RELEASED: f32 = 160e3;
pub const FIRE_HYDROGEN_ENERGY_RELEASED: f32 = 284e3;
pub const FIRE_GROWTH_RATE: f32 = 40000.0;

pub const GAS_COUNT: usize = 9;

/// Газы в порядке `Gas` SS14 (он же порядок в `gases.yml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gas {
    Oxygen = 0,
    Nitrogen,
    CarbonDioxide,
    Plasma,
    Tritium,
    WaterVapor,
    Ammonia,
    NitrousOxide,
    Frezon,
}

/// Молярная теплоёмкость газов (`molarHeatCapacity` из `gases.yml`).
pub const MOLAR_HEAT_CAPACITY: [f32; GAS_COUNT] =
    [29.0, 29.0, 37.0, 200.0, 10.0, 40.0, 20.0, 40.0, 600.0];

/// Молярная масса газов, г/моль (`molarMass` из `gases.yml`).
pub const MOLAR_MASS: [f32; GAS_COUNT] = [32.0, 28.0, 44.0, 120.0, 3.0, 18.0, 44.0, 44.0, 50.0];

/// Теплоёмкость с учётом масштаба: `GasMolarHeatCapacities` в SS14 заранее поделены на `HeatScale`.
fn scaled_capacity(gas: usize) -> f32 {
    MOLAR_HEAT_CAPACITY[gas] / HEAT_SCALE
}

/// Газовая смесь: моли по газам, температура, объём; неизменяемая смесь — космос.
#[derive(Debug, Clone, PartialEq)]
pub struct GasMixture {
    pub(crate) moles: [f32; GAS_COUNT],
    pub(crate) temperature: f32,
    pub volume: f32,
    pub(crate) immutable: bool,
    /// `ReactionResults[Fire]`: сколько топлива сгорело в последней реакции горения.
    pub(crate) fire: f32,
}

impl GasMixture {
    pub fn new(volume: f32) -> Self {
        Self {
            moles: [0.0; GAS_COUNT],
            temperature: TCMB,
            volume: volume.max(0.0),
            immutable: false,
            fire: 0.0,
        }
    }

    /// Космос: пустая смесь с температурой реликтового фона, которую нельзя изменить.
    pub fn space() -> Self {
        Self {
            moles: [0.0; GAS_COUNT],
            temperature: TCMB,
            volume: CELL_VOLUME,
            immutable: true,
            fire: 0.0,
        }
    }

    /// Воздух станции: стандартное число молей в клетке, 21% кислорода и 79% азота при 20 °C.
    pub fn standard_air() -> Self {
        let mut m = Self::new(CELL_VOLUME);
        m.temperature = T20C;
        m.moles[Gas::Oxygen as usize] = MOLES_CELL_STANDARD * OXYGEN_STANDARD;
        m.moles[Gas::Nitrogen as usize] = MOLES_CELL_STANDARD * NITROGEN_STANDARD;
        m
    }

    pub fn is_immutable(&self) -> bool {
        self.immutable
    }

    pub fn moles(&self, gas: Gas) -> f32 {
        self.moles[gas as usize]
    }

    pub fn moles_at(&self, index: usize) -> f32 {
        self.moles[index]
    }

    pub fn temperature(&self) -> f32 {
        self.temperature
    }

    pub fn set_temperature(&mut self, t: f32) {
        if !self.immutable && t.is_finite() {
            self.temperature = t.clamp(TCMB, TMAX);
        }
    }

    pub fn set_moles(&mut self, gas: Gas, moles: f32) {
        if !self.immutable && moles.is_finite() && moles >= 0.0 {
            self.moles[gas as usize] = moles;
        }
    }

    /// Добавить (или убавить) молей; ниже нуля не уходит.
    pub fn adjust_moles(&mut self, gas: Gas, delta: f32) {
        if self.immutable || !delta.is_finite() {
            return;
        }
        let m = &mut self.moles[gas as usize];
        *m = (*m + delta).max(0.0);
    }

    pub fn total_moles(&self) -> f32 {
        self.moles.iter().sum()
    }

    /// Давление, кПа: `nRT / V`.
    pub fn pressure(&self) -> f32 {
        if self.volume <= 0.0 {
            return 0.0;
        }
        self.total_moles() * R * self.temperature / self.volume
    }

    /// Теплоёмкость (`GetHeatCapacityCalculation`): пустой космос — условные 7000, иначе сумма по
    /// газам, не меньше минимума.
    pub fn heat_capacity(&self) -> f32 {
        if self.immutable && self.total_moles().abs() < 1e-6 {
            return SPACE_HEAT_CAPACITY;
        }
        let sum: f32 = (0..GAS_COUNT)
            .map(|i| self.moles[i] * scaled_capacity(i))
            .sum();
        sum.max(MINIMUM_HEAT_CAPACITY)
    }

    /// Убрать `moles` молей пропорционально составу (`Remove`).
    pub fn remove(&mut self, moles: f32) -> GasMixture {
        let total = self.total_moles();
        if total <= 0.0 || moles <= 0.0 {
            let mut empty = GasMixture::new(self.volume);
            empty.temperature = self.temperature;
            return empty;
        }
        self.remove_ratio((moles / total).min(1.0))
    }

    /// Масса смеси, г (`GetMass`): моли на молярную массу газа.
    pub fn mass(&self) -> f32 {
        (0..GAS_COUNT).map(|i| self.moles[i] * MOLAR_MASS[i]).sum()
    }

    /// Есть ли топливо (плазма, тритий): `IsMixtureFuel`.
    pub fn is_fuel(&self) -> bool {
        self.moles[Gas::Plasma as usize] + self.moles[Gas::Tritium as usize] > GAS_EPSILON
    }

    /// Есть ли окислитель (кислород): `IsMixtureOxidizer`.
    pub fn is_oxidizer(&self) -> bool {
        self.moles[Gas::Oxygen as usize] > GAS_EPSILON
    }

    pub fn is_ignitable(&self) -> bool {
        self.is_fuel() && self.is_oxidizer()
    }

    pub fn thermal_energy(&self) -> f32 {
        self.temperature * self.heat_capacity()
    }

    pub fn clear(&mut self) {
        if !self.immutable {
            self.moles = [0.0; GAS_COUNT];
        }
    }

    pub fn multiply(&mut self, k: f32) {
        if !self.immutable {
            for m in &mut self.moles {
                *m *= k;
            }
        }
    }

    pub fn copy_from(&mut self, other: &GasMixture) {
        if self.immutable {
            return;
        }
        self.volume = other.volume;
        self.moles = other.moles;
        self.temperature = other.temperature;
    }

    /// Влить `giver` в эту смесь с обменом теплом (`Merge`).
    pub fn merge(&mut self, giver: &GasMixture) {
        if self.immutable {
            return;
        }
        if (self.temperature - giver.temperature).abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
            let (mine, theirs) = (self.heat_capacity(), giver.heat_capacity());
            let combined = mine + theirs;
            if combined > MINIMUM_HEAT_CAPACITY {
                self.temperature = (giver.thermal_energy() + self.thermal_energy()) / combined;
            }
        }
        for i in 0..GAS_COUNT {
            self.moles[i] += giver.moles[i];
        }
    }

    /// Отделить долю смеси (`RemoveRatio`): в отделённой смеси тот же объём и температура.
    pub fn remove_ratio(&mut self, ratio: f32) -> GasMixture {
        let mut removed = GasMixture::new(self.volume);
        removed.temperature = self.temperature;
        if ratio <= 0.0 || !ratio.is_finite() {
            return removed;
        }
        let ratio = ratio.min(1.0);
        for i in 0..GAS_COUNT {
            removed.moles[i] = self.moles[i] * ratio;
            if !self.immutable {
                self.moles[i] -= removed.moles[i];
                if self.moles[i] < GAS_MIN_MOLES || self.moles[i].is_nan() {
                    self.moles[i] = 0.0;
                }
            }
            if removed.moles[i] < GAS_MIN_MOLES || removed.moles[i].is_nan() {
                removed.moles[i] = 0.0;
            }
        }
        removed
    }

    /// Отделить столько литров смеси (`RemoveVolume`): вдох берёт `BREATH_VOLUME`.
    pub fn remove_volume(&mut self, litres: f32) -> GasMixture {
        if self.volume <= 0.0 {
            return GasMixture::new(0.0);
        }
        self.remove_ratio(litres / self.volume)
    }
}

/// Результат сравнения (`GasCompareResult`): можно ли обмениваться газом.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    NoExchange,
    Gas(usize),
    Temperature,
}

/// `CompareExchange`: есть ли смысл обмениваться между двумя архивными смесями.
pub fn compare_exchange(sample: &GasMixture, other: &GasMixture) -> Compare {
    let mut moles = 0.0;
    for i in 0..GAS_COUNT {
        let gas_moles = sample.moles[i];
        let delta = (gas_moles - other.moles[i]).abs();
        if delta > MINIMUM_MOLES_DELTA_TO_MOVE && delta > gas_moles * MINIMUM_AIR_RATIO_TO_MOVE {
            return Compare::Gas(i);
        }
        moles += gas_moles;
    }
    if moles > MINIMUM_MOLES_DELTA_TO_MOVE
        && (sample.temperature - other.temperature).abs() > MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND
    {
        return Compare::Temperature;
    }
    Compare::NoExchange
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_air_is_one_atmosphere_with_the_ss14_mix() {
        let air = GasMixture::standard_air();
        assert!(
            (air.pressure() - ONE_ATMOSPHERE).abs() < 0.01,
            "{}",
            air.pressure()
        );
        assert!((air.moles(Gas::Oxygen) / air.total_moles() - 0.21).abs() < 1e-5);
        assert!(
            (air.total_moles() - 103.9).abs() < 0.5,
            "{}",
            air.total_moles()
        );
    }

    #[test]
    fn a_breath_takes_half_a_litre_of_air() {
        let mut air = GasMixture::standard_air();
        let before = air.total_moles();
        let breath = air.remove_volume(BREATH_VOLUME);
        let expected = ONE_ATMOSPHERE * BREATH_VOLUME / (R * T20C);
        assert!(
            (breath.total_moles() - expected).abs() < 1e-4,
            "{}",
            breath.total_moles()
        );
        assert!((breath.moles(Gas::Oxygen) - expected * 0.21).abs() < 1e-5);
        assert!((before - air.total_moles() - expected).abs() < 1e-3);
    }

    #[test]
    fn space_is_a_vacuum_that_cannot_be_changed_or_filled() {
        let mut space = GasMixture::space();
        assert_eq!(space.pressure(), 0.0);
        space.adjust_moles(Gas::Oxygen, 5.0);
        space.set_temperature(300.0);
        space.merge(&GasMixture::standard_air());
        assert_eq!(space.total_moles(), 0.0);
        assert_eq!(space.temperature(), TCMB);
        assert_eq!(space.heat_capacity(), SPACE_HEAT_CAPACITY);
        let taken = space.remove_ratio(0.5);
        assert_eq!(taken.total_moles(), 0.0);
    }

    #[test]
    fn merging_mixes_temperatures_by_thermal_energy() {
        let mut hot = GasMixture::standard_air();
        hot.set_temperature(400.0);
        let cold = GasMixture::standard_air();
        let mut a = cold.clone();
        a.merge(&hot);
        // Равные смеси: температура посередине.
        assert!(
            (a.temperature() - (400.0 + T20C) / 2.0).abs() < 0.1,
            "{}",
            a.temperature()
        );
        assert!((a.total_moles() - 2.0 * cold.total_moles()).abs() < 1e-3);
    }

    #[test]
    fn compare_exchange_ignores_equal_air_and_notices_differences() {
        let a = GasMixture::standard_air();
        assert_eq!(compare_exchange(&a, &a.clone()), Compare::NoExchange);
        assert!(matches!(
            compare_exchange(&a, &GasMixture::space()),
            Compare::Gas(_)
        ));
        let mut warm = a.clone();
        warm.set_temperature(T20C + 10.0);
        assert_eq!(compare_exchange(&a, &warm), Compare::Temperature);
    }
}
