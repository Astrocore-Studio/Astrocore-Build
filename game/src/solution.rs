//! Раствор: список «реагент → количество» в сотых долях с наибольшим объёмом, температурой и
//! флагом «может реагировать» (ADR-0017). Деление, удаление и масштабирование — по правилам
//! SS14 в сотых, с округлением в пользу берущего; слияние растворов разной температуры
//! сохраняет тепло по теплоёмкостям реагентов; основной реагент и смешанный цвет. Каталог
//! реагентов собирается из прототипов `reagent`. Образец — `Solution`, `ReagentId`,
//! `ReagentQuantity`, `ReagentData`, `DnaData`, `ReagentPrototype` SS14 @ 2f4a2aad.

use std::collections::HashMap;
use std::sync::Arc;

use astrocore_math::Color;
use astrocore_prototypes::Prototypes;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::fixed_point::FixedPoint2;

/// Температура раствора по умолчанию, К.
pub const DEFAULT_TEMPERATURE: f32 = 293.15;

/// Данные при реагенте (`ReagentData`); в SS14 — только ДНК (`DnaData`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ReagentData {
    #[serde(rename = "DnaData")]
    Dna {
        #[serde(rename = "dNA", alias = "dna", alias = "DNA", default)]
        dna: String,
    },
}

/// Реагент с данными (`ReagentId`): реагенты с разными данными — разные записи раствора.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReagentId {
    pub prototype: String,
    pub data: Vec<ReagentData>,
}

impl ReagentId {
    pub fn new(prototype: impl Into<String>) -> Self {
        Self {
            prototype: prototype.into(),
            data: Vec::new(),
        }
    }

    /// Равенство данных без учёта порядка (`Equals`).
    fn same(&self, other: &ReagentId) -> bool {
        self.prototype == other.prototype
            && self.data.len() == other.data.len()
            && self.data.iter().all(|d| other.data.contains(d))
    }
}

impl std::fmt::Display for ReagentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.prototype)
    }
}

/// Количество реагента (`ReagentQuantity`). В данных: `ReagentId`, `Quantity`, `data`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReagentQuantity {
    #[serde(rename = "ReagentId")]
    pub reagent: String,
    #[serde(rename = "Quantity")]
    pub quantity: FixedPoint2,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<ReagentData>,
}

impl ReagentQuantity {
    pub fn new(reagent: impl Into<String>, quantity: FixedPoint2) -> Self {
        Self {
            reagent: reagent.into(),
            quantity,
            data: Vec::new(),
        }
    }

    pub fn id(&self) -> ReagentId {
        ReagentId {
            prototype: self.reagent.clone(),
            data: self.data.clone(),
        }
    }
}

fn yes() -> bool {
    true
}
fn default_temperature() -> f32 {
    DEFAULT_TEMPERATURE
}

/// Раствор в данных.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SolutionData {
    #[serde(default)]
    reagents: Vec<ReagentQuantity>,
    #[serde(default, rename = "maxVol")]
    max_vol: FixedPoint2,
    #[serde(default = "yes")]
    can_react: bool,
    #[serde(default = "default_temperature")]
    temperature: f32,
}

/// Раствор (`Solution`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "SolutionData", into = "SolutionData")]
pub struct Solution {
    pub contents: Vec<(ReagentId, FixedPoint2)>,
    pub volume: FixedPoint2,
    pub max_volume: FixedPoint2,
    pub can_react: bool,
    pub temperature: f32,
}

impl Default for Solution {
    fn default() -> Self {
        Self {
            contents: Vec::new(),
            volume: FixedPoint2::ZERO,
            max_volume: FixedPoint2::ZERO,
            can_react: true,
            temperature: DEFAULT_TEMPERATURE,
        }
    }
}

impl From<SolutionData> for Solution {
    /// Объём — сумма; вместимость 0 — по объёму (`AfterDeserialization`).
    fn from(data: SolutionData) -> Self {
        let mut solution = Solution {
            max_volume: data.max_vol,
            can_react: data.can_react,
            temperature: data.temperature,
            ..Default::default()
        };
        for reagent in data.reagents {
            let id = reagent.id();
            solution.add_reagent(id, reagent.quantity);
        }
        if solution.max_volume == FixedPoint2::ZERO {
            solution.max_volume = solution.volume;
        }
        solution
    }
}

impl From<Solution> for SolutionData {
    fn from(solution: Solution) -> Self {
        Self {
            reagents: solution
                .contents
                .into_iter()
                .map(|(id, quantity)| ReagentQuantity {
                    reagent: id.prototype,
                    quantity,
                    data: id.data,
                })
                .collect(),
            max_vol: solution.max_volume,
            can_react: solution.can_react,
            temperature: solution.temperature,
        }
    }
}

impl Solution {
    /// Раствор из одного реагента (`Solution(prototype, quantity)`).
    pub fn of(prototype: &str, quantity: FixedPoint2) -> Self {
        let mut solution = Self::default();
        solution.add_reagent(ReagentId::new(prototype), quantity);
        solution
    }

    /// Раствор из списка с вместимостью по объёму (`Solution(IEnumerable<ReagentQuantity>)`).
    pub fn from_reagents(reagents: impl IntoIterator<Item = (ReagentId, FixedPoint2)>) -> Self {
        let mut solution = Self::default();
        for (id, quantity) in reagents {
            solution.add_reagent(id, quantity);
        }
        solution.max_volume = solution.volume;
        solution
    }

    /// Свободный объём (`AvailableVolume`).
    pub fn available_volume(&self) -> FixedPoint2 {
        self.max_volume - self.volume
    }

    /// Доля заполнения; без вместимости — 1 (`FillFraction`).
    pub fn fill_fraction(&self) -> f32 {
        if self.max_volume == FixedPoint2::ZERO {
            1.0
        } else {
            self.volume.float() / self.max_volume.float()
        }
    }

    pub fn can_add_solution(&self, other: &Solution) -> bool {
        other.volume <= self.available_volume()
    }

    pub fn is_empty(&self) -> bool {
        self.volume == FixedPoint2::ZERO
    }

    /// Теплоёмкость по реагентам (`GetHeatCapacity`).
    pub fn heat_capacity(&self, reagents: &ReagentCatalog) -> f32 {
        self.contents
            .iter()
            .map(|(id, quantity)| quantity.float() * reagents.specific_heat(&id.prototype))
            .sum()
    }

    /// Тепловая энергия (`GetThermalEnergy`).
    pub fn thermal_energy(&self, reagents: &ReagentCatalog) -> f32 {
        self.heat_capacity(reagents) * self.temperature
    }

    pub fn contains_prototype(&self, prototype: &str) -> bool {
        self.contents
            .iter()
            .any(|(id, _)| id.prototype == prototype)
    }

    /// Количество реагента с данными (`GetReagentQuantity`).
    pub fn reagent_quantity(&self, id: &ReagentId) -> FixedPoint2 {
        self.contents
            .iter()
            .find(|(other, _)| other.same(id))
            .map_or(FixedPoint2::ZERO, |(_, q)| *q)
    }

    /// Всего реагента любого вида данных (`GetTotalPrototypeQuantity`).
    pub fn prototype_quantity(&self, prototype: &str) -> FixedPoint2 {
        self.contents
            .iter()
            .filter(|(id, _)| id.prototype == prototype)
            .fold(FixedPoint2::ZERO, |sum, (_, q)| sum + *q)
    }

    /// Реагента больше всех, последний из равных (`GetPrimaryReagentId`).
    pub fn primary_reagent(&self) -> Option<&ReagentId> {
        let mut best: Option<&(ReagentId, FixedPoint2)> = None;
        for entry in &self.contents {
            if best.is_none_or(|b| entry.1 >= b.1) {
                best = Some(entry);
            }
        }
        best.map(|(id, _)| id)
    }

    /// Добавить реагент (`AddReagent`).
    pub fn add_reagent(&mut self, id: ReagentId, quantity: FixedPoint2) {
        if quantity <= FixedPoint2::ZERO {
            return;
        }
        self.volume += quantity;
        if let Some(entry) = self.contents.iter_mut().find(|(other, _)| other.same(&id)) {
            entry.1 += quantity;
            return;
        }
        self.contents.push((id, quantity));
    }

    /// Добавить реагент своей температуры с сохранением тепла (`AddReagent(proto, quantity,
    /// temperature)`).
    pub fn add_reagent_with_temperature(
        &mut self,
        id: ReagentId,
        quantity: FixedPoint2,
        temperature: f32,
        reagents: &ReagentCatalog,
    ) {
        let heat = self.heat_capacity(reagents);
        let specific = reagents.specific_heat(&id.prototype);
        // В SS14 тепло добавляемого берётся по удельной теплоёмкости без умножения на
        // количество.
        let energy = self.temperature * heat + temperature * specific;
        self.add_reagent(id, quantity);
        let heat = self.heat_capacity(reagents);
        self.temperature = if heat == 0.0 { 0.0 } else { energy / heat };
    }

    /// Умножить на целое (`ScaleSolution(int)`).
    pub fn scale_int(&mut self, scale: i32) {
        if scale == 1 {
            return;
        }
        if scale <= 0 {
            self.remove_all();
            return;
        }
        self.volume = self.volume * scale;
        for entry in &mut self.contents {
            entry.1 = entry.1 * scale;
        }
    }

    /// Умножить на дробное (`ScaleSolution(float)`).
    pub fn scale(&mut self, scale: f32) {
        if scale == 1.0 {
            return;
        }
        if scale == 0.0 {
            self.remove_all();
            return;
        }
        self.volume = FixedPoint2::ZERO;
        for i in (0..self.contents.len()).rev() {
            let quantity = self.contents[i].1.scale(scale);
            if quantity == FixedPoint2::ZERO {
                self.contents.swap_remove(i);
            } else {
                self.contents[i].1 = quantity;
                self.volume += quantity;
            }
        }
    }

    /// Довести объём до `target` (`ScaleTo`).
    pub fn scale_to(&mut self, target: FixedPoint2) {
        if self.volume == FixedPoint2::ZERO || self.volume == target {
            return;
        }
        if target == FixedPoint2::ZERO {
            self.remove_all();
            return;
        }
        let nearest = (target.float() / self.volume.float()).ceil() as i32;
        self.scale_int(nearest);
        let overflow = self.volume - target;
        self.remove_solution(overflow);
    }

    /// Убрать реагент; `ignore_data` — любого вида данных (`RemoveReagent`). Возвращает
    /// убранное.
    pub fn remove_reagent(
        &mut self,
        id: &ReagentId,
        quantity: FixedPoint2,
        ignore_data: bool,
    ) -> FixedPoint2 {
        if quantity <= FixedPoint2::ZERO {
            return FixedPoint2::ZERO;
        }
        let matches = |other: &ReagentId| {
            if ignore_data {
                other.prototype == id.prototype
            } else {
                other.same(id)
            }
        };
        // Индексы с конца, чтобы удаление с подстановкой было безопасным.
        let mut indices: Vec<usize> = Vec::new();
        let mut total: i64 = 0;
        for (i, (other, q)) in self.contents.iter().enumerate() {
            if matches(other) {
                total += i64::from(q.0);
                indices.insert(0, i);
            }
        }
        if total <= 0 {
            return FixedPoint2::ZERO;
        }
        let mut removed = FixedPoint2::ZERO;
        for index in indices {
            let current = self.contents[index].1;
            // Округление тянет к тому, чтобы забрать больше.
            let split = FixedPoint2::from_cents(
                (i64::from(quantity.0) * i64::from(current.0) / total) as i32,
            );
            let left = current - split;
            if left <= FixedPoint2::ZERO {
                self.contents.swap_remove(index);
                self.volume -= current;
                removed += current;
                continue;
            }
            self.contents[index].1 = left;
            self.volume -= split;
            removed += split;
        }
        removed
    }

    /// Убрать реагент по прототипу без данных.
    pub fn remove_prototype(&mut self, prototype: &str, quantity: FixedPoint2) -> FixedPoint2 {
        self.remove_reagent(&ReagentId::new(prototype), quantity, false)
    }

    /// Очистить (`RemoveAllSolution`).
    pub fn remove_all(&mut self) {
        self.contents.clear();
        self.volume = FixedPoint2::ZERO;
    }

    /// Отделить часть, пропорционально составу (`SplitSolution`).
    pub fn split(&mut self, to_take: FixedPoint2) -> Solution {
        if to_take <= FixedPoint2::ZERO {
            return Solution::default();
        }
        if to_take >= self.volume {
            let taken = self.clone();
            self.remove_all();
            return taken;
        }
        let original = self.volume;
        let mut effective = i64::from(self.volume.0);
        let mut taken = Solution {
            temperature: self.temperature,
            ..Default::default()
        };
        let mut remaining = i64::from(to_take.0);
        for i in (0..self.contents.len()).rev() {
            let (id, quantity) = self.contents[i].clone();
            let split = remaining * i64::from(quantity.0) / effective;
            if split <= 0 {
                effective -= i64::from(quantity.0);
                continue;
            }
            let split_quantity = FixedPoint2::from_cents(split as i32);
            let left = quantity - split_quantity;
            if left > FixedPoint2::ZERO {
                self.contents[i].1 = left;
            } else {
                self.contents.swap_remove(i);
            }
            taken.contents.push((id, split_quantity));
            self.volume -= split_quantity;
            remaining -= split;
            effective -= i64::from(quantity.0);
        }
        taken.volume = original - self.volume;
        taken
    }

    /// Отделить, не трогая перечисленных реагентов (`SplitSolutionWithout`).
    pub fn split_without(&mut self, to_take: FixedPoint2, excluded: &[&str]) -> Solution {
        let mut kept = Vec::new();
        for prototype in excluded {
            if let Some(index) = self
                .contents
                .iter()
                .position(|(id, _)| id.prototype == *prototype)
            {
                let (id, quantity) = self.contents[index].clone();
                self.remove_reagent(&id, quantity, false);
                kept.push((id, quantity));
            }
        }
        let taken = self.split(to_take);
        for (id, quantity) in kept {
            self.add_reagent(id, quantity);
        }
        taken
    }

    /// Отделить только перечисленные реагенты (`SplitSolutionWithOnly`).
    pub fn split_with_only(&mut self, to_take: FixedPoint2, included: &[&str]) -> Solution {
        let mut kept = Vec::new();
        for i in (0..self.contents.len()).rev() {
            if included.contains(&self.contents[i].0.prototype.as_str()) {
                continue;
            }
            let (id, quantity) = self.contents[i].clone();
            self.remove_reagent(&id, quantity, false);
            kept.push((id, quantity));
        }
        let taken = self.split(to_take);
        for (id, quantity) in kept {
            self.add_reagent(id, quantity);
        }
        taken
    }

    /// Убрать объём, пропорционально составу (`RemoveSolution`).
    pub fn remove_solution(&mut self, to_take: FixedPoint2) {
        self.split(to_take);
    }

    /// Влить раствор с сохранением тепла (`AddSolution`).
    pub fn add_solution(&mut self, other: &Solution, reagents: &ReagentCatalog) {
        if other.volume <= FixedPoint2::ZERO {
            return;
        }
        let close = (other.temperature - self.temperature).abs()
            <= 1e-7 * other.temperature.abs().max(self.temperature.abs()).max(1.0);
        let energy = if close {
            0.0
        } else {
            self.thermal_energy(reagents) + other.thermal_energy(reagents)
        };
        for (id, quantity) in &other.contents {
            self.add_reagent(id.clone(), *quantity);
        }
        if !close {
            let heat = self.heat_capacity(reagents);
            self.temperature = if heat == 0.0 { 0.0 } else { energy / heat };
        }
    }

    /// Смешанный цвет без перечисленных реагентов (`GetColorWithout`).
    pub fn color_without(&self, reagents: &ReagentCatalog, without: &[&str]) -> Color {
        if self.volume == FixedPoint2::ZERO {
            return Color::TRANSPARENT;
        }
        let mut mix: Option<Color> = None;
        let mut running = FixedPoint2::ZERO;
        for (id, quantity) in &self.contents {
            if without.contains(&id.prototype.as_str()) {
                continue;
            }
            running += *quantity;
            let Some(color) = reagents.get(&id.prototype).map(|r| r.color) else {
                continue;
            };
            mix = Some(match mix {
                None => color,
                Some(mix) => {
                    let t = quantity.float() / running.float();
                    lerp(mix, color, t)
                }
            });
        }
        mix.unwrap_or(Color::TRANSPARENT)
    }

    /// Смешанный цвет (`GetColor`).
    pub fn color(&self, reagents: &ReagentCatalog) -> Color {
        self.color_without(reagents, &[])
    }

    /// Задать данные всем реагентам (`SetReagentData`).
    pub fn set_reagent_data(&mut self, data: Vec<ReagentData>) {
        let old = std::mem::take(&mut self.contents);
        self.volume = FixedPoint2::ZERO;
        for (mut id, quantity) in old {
            id.data = data.clone();
            self.add_reagent(id, quantity);
        }
    }

    /// Количество по прототипам, слитое по данным (`GetReagentPrototypes`).
    pub fn reagent_prototypes(&self) -> Vec<(String, FixedPoint2)> {
        let mut list: Vec<(String, FixedPoint2)> = Vec::new();
        for (id, quantity) in &self.contents {
            match list.iter_mut().find(|(p, _)| *p == id.prototype) {
                Some(entry) => entry.1 += *quantity,
                None => list.push((id.prototype.clone(), *quantity)),
            }
        }
        list
    }
}

fn lerp(a: Color, b: Color, t: f32) -> Color {
    Color::rgba(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
}

/// Реагент из прототипа (`ReagentPrototype`): то, что нужно растворам, осмотру и окнам.
#[derive(Debug, Clone, PartialEq)]
pub struct ReagentInfo {
    pub id: String,
    /// Ключ названия.
    pub name: String,
    pub group: String,
    /// Ключ описания.
    pub desc: String,
    /// Ключ описания на вид.
    pub physical_desc: String,
    pub color: Color,
    pub specific_heat: f32,
    pub boiling_point: Option<f32>,
    pub melting_point: Option<f32>,
    pub recognizable: bool,
    pub metabolism_rate: Option<FixedPoint2>,
    pub price_per_unit: f32,
}

/// Реагенты из прототипов. Ресурс `World`.
#[derive(Resource, Debug, Default, Clone)]
pub struct ReagentCatalog {
    reagents: HashMap<String, ReagentInfo>,
}

impl ReagentCatalog {
    pub fn build(prototypes: &Prototypes) -> Self {
        let text = |node: &astrocore_data::DataNode, key: &str| -> Option<String> {
            node.get(key)
                .and_then(|n| astrocore_data::from_node::<String>(n).ok())
        };
        let number = |node: &astrocore_data::DataNode, key: &str| -> Option<f32> {
            node.get(key)
                .and_then(|n| astrocore_data::from_node::<f32>(n).ok())
        };
        let reagents = prototypes
            .ids("reagent")
            .filter_map(|id| {
                let node = prototypes.resolved_node("reagent", id)?;
                let color = text(node, "color")
                    .and_then(|c| Color::parse(&c))
                    .unwrap_or(Color::WHITE);
                Some((
                    id.to_owned(),
                    ReagentInfo {
                        id: id.to_owned(),
                        name: text(node, "name").unwrap_or_default(),
                        group: text(node, "group").unwrap_or_else(|| "Unknown".into()),
                        desc: text(node, "desc").unwrap_or_default(),
                        physical_desc: text(node, "physicalDesc").unwrap_or_default(),
                        color,
                        specific_heat: number(node, "specificHeat").unwrap_or(1.0),
                        boiling_point: number(node, "boilingPoint"),
                        melting_point: number(node, "meltingPoint"),
                        recognizable: node
                            .get("recognizable")
                            .and_then(|n| astrocore_data::from_node::<bool>(n).ok())
                            .unwrap_or(false),
                        metabolism_rate: None,
                        price_per_unit: number(node, "pricePerUnit").unwrap_or(0.0),
                    },
                ))
            })
            .collect();
        Self { reagents }
    }

    pub fn get(&self, id: &str) -> Option<&ReagentInfo> {
        self.reagents.get(id)
    }

    /// Удельная теплоёмкость; неизвестный реагент — 1.
    pub fn specific_heat(&self, id: &str) -> f32 {
        self.get(id).map_or(1.0, |r| r.specific_heat)
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.reagents.keys().map(String::as_str)
    }
}

#[derive(Resource)]
struct SharedReagents(Arc<ReagentCatalog>);

/// Каталог реагентов мира; собирается при первом обращении.
pub fn reagents(world: &mut World) -> Arc<ReagentCatalog> {
    if let Some(shared) = world.get_resource::<SharedReagents>() {
        return shared.0.clone();
    }
    let built = Arc::new(
        world
            .get_resource::<Prototypes>()
            .map(ReagentCatalog::build)
            .unwrap_or_default(),
    );
    world.insert_resource(SharedReagents(built.clone()));
    built
}

/// Каталог без изменения мира (клиентские окна): готовый или собранный заново.
pub fn reagents_of(world: &World) -> Arc<ReagentCatalog> {
    if let Some(shared) = world.get_resource::<SharedReagents>() {
        return shared.0.clone();
    }
    Arc::new(
        world
            .get_resource::<Prototypes>()
            .map(ReagentCatalog::build)
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests;
