//! Воздушные сигнализации и датчики (`AtmosMonitorSystem`, `AtmosAlarmThreshold`, `AirAlarmSystem`,
//! `FirelockSystem`).
//!
//! Монитор измеряет воздух своей клетки по порогам станции (`airalarms.yml`): температура, давление и
//! доля каждого газа. Состояние — самое тяжёлое из сработавших: `Normal`, `Warning`, `Danger`. Монитор
//! сообщает группе — сигнализация и устройства из её списка (`DeviceList` карты SS14): датчики,
//! вентили, скрубберы, огнезапоры. Состояние группы — самое тяжёлое среди её мониторов.
//!
//! Реакция группы при смене состояния: огнезапоры закрываются при `Danger` и открываются при
//! `Normal` (`FirelockSystem.OnAtmosAlarm`); вентили при `Danger` выключаются, при `Normal`
//! включаются (`GasVentPumpSystem.OnAtmosAlarm`); автоматический режим сигнализации ставит
//! скрубберам `WideFiltering` при `Danger` (чистят и соседние клетки) и `Filtering` иначе.

use atmos::{Gas, GasMixture};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum AlarmState {
    #[default]
    Normal,
    Warning,
    Danger,
}

/// `AtmosAlarmThreshold`: границы и доли предупреждения.
#[derive(Debug, Clone, Copy, Default)]
pub struct Threshold {
    pub upper: Option<f32>,
    pub lower: Option<f32>,
    /// `upperWarnAround` / `lowerWarnAround`: предупреждение на `граница × доля`.
    pub upper_warn: Option<f32>,
    pub lower_warn: Option<f32>,
}

impl Threshold {
    const fn new(upper: Option<f32>, lower: Option<f32>, uw: Option<f32>, lw: Option<f32>) -> Self {
        Self {
            upper,
            lower,
            upper_warn: uw,
            lower_warn: lw,
        }
    }

    /// `CheckThreshold`.
    pub fn check(&self, value: f32) -> AlarmState {
        if self.upper.is_some_and(|u| value >= u) || self.lower.is_some_and(|l| value <= l) {
            return AlarmState::Danger;
        }
        let upper_warning = self.upper.zip(self.upper_warn).map(|(u, p)| u * p);
        let lower_warning = self.lower.zip(self.lower_warn).map(|(l, p)| l * p);
        if upper_warning.is_some_and(|w| value >= w) || lower_warning.is_some_and(|w| value <= w) {
            return AlarmState::Warning;
        }
        AlarmState::Normal
    }
}

/// Пороги станции из `airalarms.yml`.
pub const TEMPERATURE: Threshold = Threshold::new(Some(393.15), Some(193.15), Some(0.8), Some(1.1));
pub const PRESSURE: Threshold = Threshold::new(Some(550.0), Some(85.0), Some(0.7), Some(1.05));

/// Порог по доле газа в смеси (`gasThresholdPrototypes` сигнализации и датчика).
pub fn gas_threshold(gas: Gas) -> Threshold {
    match gas {
        Gas::Oxygen => Threshold::new(Some(0.30), Some(0.10), Some(0.8), Some(1.5)),
        Gas::Nitrogen => Threshold::new(None, Some(0.10), None, Some(1.5)),
        Gas::CarbonDioxide => Threshold::new(Some(0.006), None, Some(0.5), None),
        Gas::Plasma => Threshold::new(Some(0.005), None, Some(0.5), None),
        Gas::Tritium => Threshold::new(Some(0.004), None, Some(0.5), None),
        Gas::WaterVapor => Threshold::new(Some(1.5), None, Some(0.5), None),
        Gas::Ammonia => Threshold::new(Some(0.05), None, Some(0.5), None),
        Gas::NitrousOxide => Threshold::new(Some(0.01), None, Some(0.5), None),
        // `danger`: любой замеченный фрезон — тревога.
        Gas::Frezon => Threshold::new(Some(0.0001), None, None, None),
    }
}

const ALL_GASES: [Gas; 9] = [
    Gas::Oxygen,
    Gas::Nitrogen,
    Gas::CarbonDioxide,
    Gas::Plasma,
    Gas::Tritium,
    Gas::WaterVapor,
    Gas::Ammonia,
    Gas::NitrousOxide,
    Gas::Frezon,
];

/// `AtmosMonitorSystem.UpdateState`: состояние воздуха по температуре, давлению и газам.
pub fn evaluate(air: &GasMixture) -> AlarmState {
    let mut state = TEMPERATURE
        .check(air.temperature())
        .max(PRESSURE.check(air.pressure()));
    let total = air.total_moles();
    if total > 0.0 {
        for g in ALL_GASES {
            state = state.max(gas_threshold(g).check(air.moles(g) / total));
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn station_air_is_normal_and_a_vacuum_or_a_fire_is_danger() {
        assert_eq!(evaluate(&GasMixture::standard_air()), AlarmState::Normal);
        assert_eq!(
            evaluate(&GasMixture::space()),
            AlarmState::Danger,
            "вакуум: давление ниже 85 кПа"
        );
        let mut hot = GasMixture::standard_air();
        hot.set_temperature(420.0);
        assert_eq!(evaluate(&hot), AlarmState::Danger);
        let mut plasma = GasMixture::standard_air();
        plasma.adjust_moles(Gas::Plasma, 5.0);
        assert_eq!(
            evaluate(&plasma),
            AlarmState::Danger,
            "плазма выше 0,5% в смеси"
        );
    }

    #[test]
    fn the_warning_band_sits_between_normal_and_the_bound() {
        // Давление: опасно ≥ 550 или ≤ 85; предупреждение ≥ 385 или ≤ 89,25.
        assert_eq!(PRESSURE.check(101.3), AlarmState::Normal);
        assert_eq!(PRESSURE.check(400.0), AlarmState::Warning);
        assert_eq!(PRESSURE.check(87.0), AlarmState::Warning);
        assert_eq!(PRESSURE.check(560.0), AlarmState::Danger);
        assert_eq!(PRESSURE.check(80.0), AlarmState::Danger);
        // Температура: предупреждение при 0,8 × 393,15 ≈ 314,5 К.
        assert_eq!(TEMPERATURE.check(300.0), AlarmState::Normal);
        assert_eq!(TEMPERATURE.check(330.0), AlarmState::Warning);
        // Кислород: опасно ниже 10%, предупреждение ниже 15%.
        assert_eq!(gas_threshold(Gas::Oxygen).check(0.21), AlarmState::Normal);
        assert_eq!(gas_threshold(Gas::Oxygen).check(0.14), AlarmState::Warning);
        assert_eq!(gas_threshold(Gas::Oxygen).check(0.05), AlarmState::Danger);
    }
}
