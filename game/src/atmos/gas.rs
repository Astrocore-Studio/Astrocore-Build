//! Газовые смеси и их физика. Образец — `GasMixture`, `Atmospherics`,
//! `SharedAtmosphereSystem.Gases` и `AtmosphereSystem.Gases` SS14 @ 2f4a2aad.

use astrocore_data::{DataMap, DataNode};
use astrocore_prototypes::Prototypes;
use bevy_ecs::resource::Resource;

/// Газовая постоянная (`R`).
pub const R: f32 = 8.314_463;
pub const ONE_ATMOSPHERE: f32 = 101.325;
/// Температура космоса (`TCMB`).
pub const TCMB: f32 = 2.7;
pub const T0C: f32 = 273.15;
pub const T20C: f32 = 293.15;
pub const TMAX: f32 = 262_144.0;
pub const CELL_VOLUME: f32 = 2500.0;
pub const MOLES_CELL_STANDARD: f32 = ONE_ATMOSPHERE * CELL_VOLUME / (T20C * R);
pub const G_TO_KG: f32 = 0.001;
pub const GAS_MIN_MOLES: f32 = 0.000_000_05;
pub const OPEN_HEAT_TRANSFER_COEFFICIENT: f32 = 0.4;
pub const MINIMUM_AIR_RATIO_TO_SUSPEND: f32 = 0.1;
pub const MINIMUM_AIR_RATIO_TO_MOVE: f32 = 0.001;
pub const MINIMUM_AIR_TO_SUSPEND: f32 = MOLES_CELL_STANDARD * MINIMUM_AIR_RATIO_TO_SUSPEND;
pub const MINIMUM_TEMPERATURE_TO_MOVE: f32 = 5.0;
pub const MINIMUM_MOLES_DELTA_TO_MOVE: f32 = MOLES_CELL_STANDARD * MINIMUM_AIR_RATIO_TO_MOVE;
pub const MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND: f32 = 4.0;
pub const MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER: f32 = 0.01;
pub const MINIMUM_TEMPERATURE_START_SUPERCONDUCTION: f32 = T20C + 400.0;
pub const MINIMUM_HEAT_CAPACITY: f32 = 0.0003;
pub const SPACE_HEAT_CAPACITY: f32 = 7000.0;
pub const EXCITED_GROUP_BREAKDOWN_CYCLES: i32 = 4;
pub const EXCITED_GROUPS_DISMANTLE_CYCLES: i32 = 16;
pub const MONSTERMOS_HARD_TILE_LIMIT: usize = 2000;
pub const MONSTERMOS_TILE_LIMIT: usize = 200;
/// Делитель теплоёмкостей (`atmos.heat_scale`).
pub const HEAT_SCALE: f32 = 8.0;

/// Число газов (`TotalNumberOfGases`).
pub const GAS_COUNT: usize = 9;

/// Газы по порядку (`Gas`): имена прототипов `gas`.
pub const GAS_NAMES: [&str; GAS_COUNT] = [
    "Oxygen",
    "Nitrogen",
    "CarbonDioxide",
    "Plasma",
    "Tritium",
    "WaterVapor",
    "Ammonia",
    "NitrousOxide",
    "Frezon",
];

pub const OXYGEN: usize = 0;
pub const NITROGEN: usize = 1;
pub const CARBON_DIOXIDE: usize = 2;
pub const PLASMA: usize = 3;
pub const TRITIUM: usize = 4;
pub const WATER_VAPOR: usize = 5;
pub const AMMONIA: usize = 6;
pub const NITROUS_OXIDE: usize = 7;
pub const FREZON: usize = 8;

/// Номер газа по имени прототипа.
pub fn gas_index(name: &str) -> Option<usize> {
    GAS_NAMES.iter().position(|gas| *gas == name)
}

/// Свойства газов из прототипов `gas` (`GasPrototype`). Ресурс.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct GasData {
    /// Молярные теплоёмкости, делённые на [`HEAT_SCALE`].
    pub heat_capacities: [f32; GAS_COUNT],
    pub molar_masses: [f32; GAS_COUNT],
    pub fuel: [bool; GAS_COUNT],
    pub oxidizer: [bool; GAS_COUNT],
    pub colors: [Option<String>; GAS_COUNT],
}

impl Default for GasData {
    /// Значения прототипов SS14 @ 2f4a2aad на случай, если прототипов нет.
    fn default() -> Self {
        let heat = [29.0, 29.0, 37.0, 200.0, 10.0, 40.0, 20.0, 40.0, 600.0];
        let mut heat_capacities = [0.0; GAS_COUNT];
        for (i, value) in heat.iter().enumerate() {
            heat_capacities[i] = value / HEAT_SCALE;
        }
        Self {
            heat_capacities,
            molar_masses: [32.0, 28.0, 44.0, 120.0, 3.0, 18.0, 44.0, 44.0, 50.0],
            fuel: [false, false, false, true, true, false, false, false, false],
            oxidizer: [true, false, false, false, false, false, false, false, false],
            colors: Default::default(),
        }
    }
}

impl GasData {
    /// Из прототипов `gas` (`InitializeGases`).
    pub fn from_prototypes(prototypes: &Prototypes) -> Self {
        let mut data = Self::default();
        let number =
            |node: &DataNode, key: &str| -> Option<f32> { node.get(key)?.as_str()?.parse().ok() };
        let flag = |node: &DataNode, key: &str| -> bool {
            node.get(key)
                .and_then(DataNode::as_str)
                .is_some_and(|v| v.eq_ignore_ascii_case("true"))
        };
        for (i, name) in GAS_NAMES.iter().enumerate() {
            let Some(node) = prototypes.resolved_node("gas", name) else {
                continue;
            };
            if let Some(heat) = number(node, "molarHeatCapacity") {
                data.heat_capacities[i] = heat / HEAT_SCALE;
            }
            if let Some(mass) = number(node, "molarMass") {
                data.molar_masses[i] = mass;
            }
            data.fuel[i] = flag(node, "isFuel");
            data.oxidizer[i] = flag(node, "isOxidizer");
            data.colors[i] = node
                .get("color")
                .and_then(DataNode::as_str)
                .map(str::to_owned);
        }
        data
    }

    /// Теплоёмкость смеси (`GetHeatCapacity`, со шкалой): у пустого космоса — постоянная.
    pub fn heat_capacity(&self, mixture: &GasMixture) -> f32 {
        self.heat_capacity_of(&mixture.moles, mixture.immutable)
    }

    /// `GetHeatCapacityCalculation`.
    pub fn heat_capacity_of(&self, moles: &[f32; GAS_COUNT], space: bool) -> f32 {
        let total: f32 = moles.iter().sum();
        if space && total.abs() < 0.000_01 {
            return SPACE_HEAT_CAPACITY;
        }
        let sum: f32 = moles
            .iter()
            .zip(self.heat_capacities.iter())
            .map(|(m, c)| m * c)
            .sum();
        sum.max(MINIMUM_HEAT_CAPACITY)
    }

    /// Тепловая энергия (`GetThermalEnergy`).
    pub fn thermal_energy(&self, mixture: &GasMixture) -> f32 {
        mixture.temperature() * self.heat_capacity(mixture)
    }

    /// Масса, кг (`GetMass`).
    pub fn mass(&self, mixture: &GasMixture) -> f32 {
        let sum: f32 = mixture
            .moles
            .iter()
            .zip(self.molar_masses.iter())
            .map(|(m, mass)| m * mass)
            .sum();
        sum * G_TO_KG
    }

    /// Слить `giver` в `receiver` (`Merge`).
    pub fn merge(&self, receiver: &mut GasMixture, giver: &GasMixture) {
        if receiver.immutable {
            return;
        }
        if (receiver.temperature() - giver.temperature()).abs()
            > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER
        {
            let receiver_heat = self.heat_capacity(receiver);
            let giver_heat = self.heat_capacity(giver);
            let combined = receiver_heat + giver_heat;
            if combined > MINIMUM_HEAT_CAPACITY {
                receiver.set_temperature(
                    (giver.temperature() * giver_heat + receiver.temperature() * receiver_heat)
                        / combined,
                );
            }
        }
        for i in 0..GAS_COUNT {
            receiver.moles[i] += giver.moles[i];
        }
    }

    /// Выпустить газ до давления (`ReleaseGasTo`); `output` — `None`: в никуда.
    pub fn release_gas_to(
        &self,
        mixture: &mut GasMixture,
        output: Option<&mut GasMixture>,
        target_pressure: f32,
    ) -> bool {
        let output_pressure = output.as_ref().map_or(0.0, |o| o.pressure());
        let input_pressure = mixture.pressure();
        if output_pressure >= target_pressure.min(input_pressure - 10.0) {
            return false;
        }
        if mixture.total_moles() <= 0.0 || mixture.temperature() <= 0.0 {
            return false;
        }
        let delta =
            (target_pressure - output_pressure).min((input_pressure - output_pressure) / 2.0);
        let volume = output.as_ref().map_or(CELL_VOLUME, |o| o.volume);
        let moles = delta * volume / (mixture.temperature() * R);
        let removed = mixture.remove(moles);
        if let Some(output) = output {
            self.merge(output, &removed);
        }
        true
    }
}

/// Газовая смесь (`GasMixture`).
#[derive(Debug, Clone, PartialEq)]
pub struct GasMixture {
    pub moles: [f32; GAS_COUNT],
    temperature: f32,
    pub volume: f32,
    /// Неизменная (космос, атмосфера карты).
    pub immutable: bool,
}

impl Default for GasMixture {
    fn default() -> Self {
        Self {
            moles: [0.0; GAS_COUNT],
            temperature: TCMB,
            volume: 0.0,
            immutable: false,
        }
    }
}

impl GasMixture {
    /// Пустая смесь объёма `volume`.
    pub fn new(volume: f32) -> Self {
        Self {
            volume: volume.max(0.0),
            ..Self::default()
        }
    }

    /// Космос (`SpaceGas`).
    pub fn space() -> Self {
        Self {
            volume: CELL_VOLUME,
            immutable: true,
            ..Self::default()
        }
    }

    /// Обычный воздух станции: 21 % кислорода, 79 % азота при 20 °C.
    pub fn standard_air(volume: f32) -> Self {
        let mut air = Self::new(volume);
        let scale = volume / CELL_VOLUME;
        air.moles[OXYGEN] = MOLES_CELL_STANDARD * 0.21 * scale;
        air.moles[NITROGEN] = MOLES_CELL_STANDARD * 0.79 * scale;
        air.temperature = T20C;
        air
    }

    pub fn temperature(&self) -> f32 {
        self.temperature
    }

    /// Температура в пределах [`TCMB`, `TMAX`]; у неизменной смеси не меняется.
    pub fn set_temperature(&mut self, value: f32) {
        if !self.immutable {
            self.temperature = value.clamp(TCMB, TMAX);
        }
    }

    /// Температура без проверок — для чтения карты.
    pub fn with_temperature(mut self, value: f32) -> Self {
        self.temperature = value;
        self
    }

    pub fn total_moles(&self) -> f32 {
        self.moles.iter().sum()
    }

    /// Давление, кПа (`Pressure`).
    pub fn pressure(&self) -> f32 {
        if self.volume <= 0.0 {
            return 0.0;
        }
        self.total_moles() * R * self.temperature / self.volume
    }

    pub fn set_moles(&mut self, gas: usize, moles: f32) {
        if !self.immutable && moles.is_finite() && moles >= 0.0 {
            self.moles[gas] = moles;
        }
    }

    pub fn adjust_moles(&mut self, gas: usize, moles: f32) {
        if !self.immutable && moles.is_finite() {
            self.moles[gas] = (self.moles[gas] + moles).max(0.0);
        }
    }

    /// Забрать `amount` молей (`Remove`).
    pub fn remove(&mut self, amount: f32) -> GasMixture {
        let total = self.total_moles();
        self.remove_ratio(amount / total)
    }

    /// Забрать долю (`RemoveRatio`).
    pub fn remove_ratio(&mut self, ratio: f32) -> GasMixture {
        let mut removed = GasMixture::new(self.volume);
        removed.temperature = self.temperature;
        if ratio <= 0.0 || ratio.is_nan() {
            return removed;
        }
        let ratio = ratio.min(1.0);
        for i in 0..GAS_COUNT {
            removed.moles[i] = self.moles[i] * ratio;
            if !self.immutable {
                self.moles[i] -= removed.moles[i];
            }
            if !self.immutable && (self.moles[i] < GAS_MIN_MOLES || self.moles[i].is_nan()) {
                self.moles[i] = 0.0;
            }
            if removed.moles[i] < GAS_MIN_MOLES || removed.moles[i].is_nan() {
                removed.moles[i] = 0.0;
            }
        }
        removed
    }

    /// Скопировать другую смесь (`CopyFrom`).
    pub fn copy_from(&mut self, sample: &GasMixture) {
        if self.immutable {
            return;
        }
        self.volume = sample.volume;
        self.moles = sample.moles;
        self.set_temperature(sample.temperature);
    }

    pub fn clear(&mut self) {
        if !self.immutable {
            self.moles = [0.0; GAS_COUNT];
        }
    }

    pub fn multiply(&mut self, factor: f32) {
        if !self.immutable {
            for moles in &mut self.moles {
                *moles *= factor;
            }
        }
    }

    /// Из записи карты (`GasMixture` с `GasArraySerializer`): `moles` — словарь имён или
    /// список чисел.
    pub fn from_node(node: &DataNode) -> Option<Self> {
        let number = |key: &str| -> Option<f32> { node.get(key)?.as_str()?.parse().ok() };
        let mut mixture = Self::new(number("volume").unwrap_or(0.0));
        mixture.temperature = number("temperature").unwrap_or(TCMB);
        if let Some(moles) = node.get("moles") {
            if let Some(map) = moles.as_map() {
                for (name, value) in map.iter() {
                    if let (Some(gas), Some(value)) = (
                        gas_index(name),
                        value.as_str().and_then(|v| v.parse::<f32>().ok()),
                    ) {
                        mixture.moles[gas] = value;
                    }
                }
            } else if let Some(list) = moles.as_seq() {
                for (gas, value) in list.iter().take(GAS_COUNT).enumerate() {
                    mixture.moles[gas] = value.as_str().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                }
            }
        }
        mixture.immutable = node
            .get("immutable")
            .and_then(DataNode::as_str)
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        Some(mixture)
    }

    /// Запись карты в формате SS14: ненулевые газы словарём по именам.
    pub fn to_node(&self) -> DataNode {
        let mut map = DataMap::new();
        if self.temperature != TCMB {
            map.insert("temperature", DataNode::plain(float_text(self.temperature)));
        }
        map.insert("volume", DataNode::plain(float_text(self.volume)));
        let mut moles = DataMap::new();
        for (gas, value) in self.moles.iter().enumerate() {
            if *value != 0.0 {
                moles.insert(GAS_NAMES[gas], DataNode::plain(float_text(*value)));
            }
        }
        map.insert("moles", DataNode::map(moles));
        if self.immutable {
            map.insert("immutable", DataNode::plain("True"));
        }
        DataNode::map(map)
    }
}

/// Число как его пишет SS14: кратчайшая запись, без лишней дробной части.
pub fn float_text(value: f32) -> String {
    let text = format!("{value}");
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

/// Чем отличаются смеси соседних клеток (`GasCompareResult`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GasCompare {
    NoExchange,
    TemperatureExchange,
    Gas(usize),
}

/// Есть ли что выравнивать (`CompareExchange`).
pub fn compare_exchange(sample: &GasMixture, other: &GasMixture) -> GasCompare {
    let mut moles = 0.0;
    for i in 0..GAS_COUNT {
        let gas = sample.moles[i];
        let delta = (gas - other.moles[i]).abs();
        if delta > MINIMUM_MOLES_DELTA_TO_MOVE && delta > gas * MINIMUM_AIR_RATIO_TO_MOVE {
            return GasCompare::Gas(i);
        }
        moles += gas;
    }
    if moles > MINIMUM_MOLES_DELTA_TO_MOVE
        && (sample.temperature - other.temperature).abs() > MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND
    {
        return GasCompare::TemperatureExchange;
    }
    GasCompare::NoExchange
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_air_is_one_atmosphere() {
        let air = GasMixture::standard_air(CELL_VOLUME);
        assert!((air.pressure() - ONE_ATMOSPHERE).abs() < 0.01);
        assert!((air.moles[OXYGEN] - 21.824_879).abs() < 0.001);
    }

    #[test]
    fn merge_mixes_temperatures_by_heat_capacity() {
        let data = GasData::default();
        let mut cold = GasMixture::standard_air(CELL_VOLUME);
        let mut hot = GasMixture::standard_air(CELL_VOLUME);
        hot.set_temperature(T20C + 100.0);
        cold.set_temperature(T20C);
        data.merge(&mut cold, &hot);
        assert!((cold.temperature() - (T20C + 50.0)).abs() < 0.01);
        assert!((cold.pressure() - 2.0 * ONE_ATMOSPHERE * (T20C + 50.0) / T20C).abs() < 0.1);
    }

    #[test]
    fn remove_ratio_and_space_heat_capacity() {
        let data = GasData::default();
        let mut air = GasMixture::standard_air(CELL_VOLUME);
        let half = air.remove_ratio(0.5);
        assert!((half.total_moles() - air.total_moles()).abs() < 0.001);
        assert_eq!(
            data.heat_capacity(&GasMixture::space()),
            SPACE_HEAT_CAPACITY
        );
        let mut space = GasMixture::space();
        space.adjust_moles(OXYGEN, 10.0);
        assert_eq!(space.total_moles(), 0.0);
    }

    #[test]
    fn map_record_round_trip() {
        let node = astrocore_data::parse_yaml(
            "temperature: 293.15\nvolume: 2500\nmoles:\n  Oxygen: 21.824879\n  Nitrogen: 82.10312\n",
        )
        .unwrap();
        let mixture = GasMixture::from_node(&node).unwrap();
        assert_eq!(mixture.moles[NITROGEN], 82.10312);
        assert_eq!(GasMixture::from_node(&mixture.to_node()).unwrap(), mixture);
        assert_eq!(float_text(2500.0), "2500");
    }
}
