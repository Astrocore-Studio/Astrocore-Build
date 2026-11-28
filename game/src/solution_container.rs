//! Растворы сущностей (ADR-0017): основной раствор — компонент `Solution` на самой сущности,
//! дополнительные — сущности-растворы в контейнере `solutions` под `SolutionManager`
//! (создаются при появлении по списку прототипов или по запросу `ensure_solution`). Раствор
//! ищется по имени; любое изменение проходит `update_chemicals`: реакции, перелив сверх
//! вместимости, событие изменения хозяину, вид заполнения. Компоненты-возможности указывают,
//! какой раствор можно доливать, сливать, набирать, вводить, осматривать. Осмотр — объём,
//! вид основного реагента и узнаваемые реагенты; очки химика добавляют глагол «Раствор» с
//! полным составом и температурой. Образец — `SharedSolutionContainerSystem` (с
//! `Capabilities`, `Relays`), `SolutionComponent`, `SolutionManagerComponent`,
//! `ContainedSolutionComponent`, `*SolutionComponent` возможностей,
//! `ExaminableSolutionComponent`, `SolutionScannerSystem` SS14 @ 2f4a2aad.
//!
//! Нет пока: закрытых ёмкостей (`Openable`), реакций (7.2б).

use astrocore_ecs::{ComponentInit, EventWorldExt, LifecycleWorldExt, MapInit, RegistryWorldExt};
use astrocore_loc::{EntityNameWorldExt, LocArg};
use astrocore_map::{ContainerKind, ContainerWorldExt, EntityCoordinates, SpawnWorldExt};
use astrocore_math::Vec2;
use astrocore_net::{
    Actor, AppearanceWorldExt, NetEntity, NetEventWorldExt, NetRegistryWorldExt, NetTarget,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::examine::{ExamineMessage, Examined, loc};
use crate::fixed_point::FixedPoint2;
use crate::hands::HandsWorldExt;
use crate::inventory::{Inventory, InventoryWorldExt, SlotFlags};
use crate::solution::{ReagentCatalog, ReagentId, Solution, reagents};
use crate::verbs::{GetVerbs, Verb, VerbKind};

/// Имя основного раствора по умолчанию (`DefaultSolutionId`).
pub const DEFAULT_SOLUTION_ID: &str = "solution";
/// Контейнер растворов (`DefaultContainerId`).
pub const DEFAULT_CONTAINER_ID: &str = "solutions";
/// Прототип раствора по умолчанию (`DefaultSolution`).
pub const DEFAULT_SOLUTION_PROTOTYPE: &str = "Solution";

/// Ключи вида заполнения (`SolutionContainerVisuals`).
pub const VISUALS_FILL_FRACTION: &str = "enum.SolutionContainerVisuals.FillFraction";
pub const VISUALS_COLOR: &str = "enum.SolutionContainerVisuals.Color";
pub const VISUALS_SOLUTION_NAME: &str = "enum.SolutionContainerVisuals.SolutionName";
pub const VISUALS_BASE_OVERRIDE: &str = "enum.SolutionContainerVisuals.BaseOverride";

fn default_solution_id() -> String {
    DEFAULT_SOLUTION_ID.into()
}

/// Раствор сущности (`SolutionComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolutionComponent {
    #[serde(default = "default_solution_id")]
    pub id: String,
    #[serde(default)]
    pub solution: Solution,
}

fn default_container() -> String {
    DEFAULT_CONTAINER_ID.into()
}

/// Дополнительные растворы (`SolutionManagerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolutionManager {
    #[serde(default = "default_container")]
    pub container: String,
    /// Прототипы растворов, которые появятся при `MapInit`.
    #[serde(default, rename = "solutions")]
    pub solution_ents: Option<Vec<String>>,
}

impl Default for SolutionManager {
    fn default() -> Self {
        Self {
            container: default_container(),
            solution_ents: None,
        }
    }
}

macro_rules! capability {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
        pub struct $name {
            #[serde(default = "default_capability_solution")]
            pub solution: String,
        }
    };
}

fn default_capability_solution() -> String {
    "default".into()
}

capability!(
    /// Можно набрать шприцем (`DrawableSolutionComponent`).
    DrawableSolution
);
capability!(
    /// Можно ввести шприцем (`InjectableSolutionComponent`).
    InjectableSolution
);
capability!(
    /// Смешивается мешалкой (`MixableSolutionComponent`).
    MixableSolution
);
capability!(
    /// Встаёт в химраздатчик (`FitsInDispenserComponent`).
    FitsInDispenser
);

/// Можно долить (`RefillableSolutionComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefillableSolution {
    #[serde(default = "default_capability_solution")]
    pub solution: String,
    #[serde(default)]
    pub max_refill: Option<FixedPoint2>,
    /// С.
    #[serde(default)]
    pub refill_time: f32,
}

/// Можно слить (`DrainableSolutionComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DrainableSolution {
    #[serde(default = "default_capability_solution")]
    pub solution: String,
    /// С.
    #[serde(default)]
    pub drain_time: f32,
}

/// Можно вылить целиком (`DumpableSolutionComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DumpableSolution {
    #[serde(default = "default_capability_solution")]
    pub solution: String,
    #[serde(default)]
    pub unlimited: bool,
}

fn yes() -> bool {
    true
}
fn loc_volume() -> String {
    "examinable-solution-on-examine-volume".into()
}
fn loc_physical() -> String {
    "shared-solution-container-component-on-examine-main-text".into()
}
fn loc_recognizable() -> String {
    "examinable-solution-has-recognizable-chemicals".into()
}

/// Раствор виден при осмотре (`ExaminableSolutionComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExaminableSolution {
    #[serde(default = "default_capability_solution")]
    pub solution: String,
    /// Только в руках.
    #[serde(default)]
    pub held_only: bool,
    /// Точный объём вместо «наполовину полон».
    #[serde(default)]
    pub exact_volume: bool,
    #[serde(default = "yes")]
    pub examinable_while_closed: bool,
    #[serde(default = "loc_volume")]
    pub loc_volume: String,
    #[serde(default = "loc_physical")]
    pub loc_physical_quality: String,
    #[serde(default = "loc_recognizable")]
    pub loc_recognizable_reagents: String,
}

fn white() -> String {
    "#FFFFFF".into()
}
fn fill_layer() -> String {
    "enum.SolutionContainerLayers.Fill".into()
}
fn base_layer() -> String {
    "enum.SolutionContainerLayers.Base".into()
}
fn overlay_layer() -> String {
    "enum.SolutionContainerLayers.Overlay".into()
}
fn glass() -> String {
    "transformable-container-component-glass".into()
}

/// Вид заполнения ёмкости (`SolutionContainerVisualsComponent`): слой с уровнем заполнения
/// цвета раствора.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionContainerVisuals {
    #[serde(default)]
    pub max_fill_levels: i32,
    #[serde(default)]
    pub fill_base_name: Option<String>,
    #[serde(default = "fill_layer")]
    pub layer: String,
    #[serde(default = "base_layer")]
    pub base_layer: String,
    #[serde(default = "overlay_layer")]
    pub overlay_layer: String,
    #[serde(default = "yes")]
    pub change_color: bool,
    #[serde(default)]
    pub empty_sprite_name: Option<String>,
    #[serde(default = "white")]
    pub empty_sprite_color: String,
    #[serde(default)]
    pub metamorphic: bool,
    #[serde(default)]
    pub metamorphic_default_sprite: Option<astrocore_data::DataNode>,
    #[serde(default = "glass")]
    pub metamorphic_name_full: String,
    #[serde(default)]
    pub solution_name: Option<String>,
    #[serde(default)]
    pub initial_description: String,
    #[serde(default)]
    pub in_hands_fill_base_name: Option<String>,
    #[serde(default)]
    pub in_hands_max_fill_levels: i32,
    #[serde(default)]
    pub equipped_fill_base_name: Option<String>,
    #[serde(default)]
    pub equipped_max_fill_levels: i32,
}

/// Видит полный состав раствора (`SolutionScannerComponent`): очки химика, борги.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SolutionScanner {}

/// Раствор изменился (`SolutionChangedEvent`). На хозяине раствора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolutionChanged {
    pub solution: Entity,
}

/// Раствор перелился (`SolutionOverflowEvent`). На сущности раствора.
#[derive(Debug, Clone)]
pub struct SolutionOverflow {
    pub overflow: Solution,
    pub handled: bool,
}

/// Можно ли получить раствор (`SolutionAccessAttemptEvent`). На хозяине.
#[derive(Debug, Clone)]
pub struct SolutionAccessAttempt {
    pub solution_name: String,
    pub cancelled: bool,
}

/// Раствор берётся у другой сущности (`GetConnectedContainerEvent`). На сущности.
#[derive(Debug, Clone, Default)]
pub struct GetConnectedContainer {
    pub container_entity: Option<Entity>,
}

/// Реакции раствора: ставит модуль реакций (7.2б). Ресурс `World`.
#[derive(Resource, Clone, Copy)]
pub struct ReactionHook(pub fn(&mut World, Entity));

/// Хозяин раствора: сущность, в контейнере растворов которой лежит раствор, иначе сама
/// (`GetSolutionOwner`).
pub fn solution_owner(world: &World, solution: Entity) -> Entity {
    let Some(holder) = world.containing_container(solution) else {
        return solution;
    };
    match world.get::<SolutionManager>(holder.owner) {
        Some(manager) if *holder.id == *manager.container => holder.owner,
        _ => solution,
    }
}

/// Сущность раствора по имени (`TryGetSolution`).
pub fn try_get_solution(world: &mut World, entity: Entity, name: &str) -> Option<Entity> {
    let mut connected = GetConnectedContainer::default();
    world.raise(entity, &mut connected);
    let entity = connected.container_entity.unwrap_or(entity);
    if world
        .get::<SolutionComponent>(entity)
        .is_some_and(|s| s.id == name)
    {
        return Some(entity);
    }
    let container = world.get::<SolutionManager>(entity)?.container.clone();
    let found = world
        .container(entity, &container)?
        .entities()
        .iter()
        .copied()
        .find(|e| {
            world
                .get::<SolutionComponent>(*e)
                .is_some_and(|s| s.id == name)
        })?;
    let mut attempt = SolutionAccessAttempt {
        solution_name: name.to_owned(),
        cancelled: false,
    };
    world.raise(entity, &mut attempt);
    (!attempt.cancelled).then_some(found)
}

/// Все растворы сущности: свой и в контейнере (`EnumerateSolutions`).
pub fn enumerate_solutions(world: &World, entity: Entity) -> Vec<(String, Entity)> {
    let mut list = Vec::new();
    if let Some(own) = world.get::<SolutionComponent>(entity) {
        list.push((own.id.clone(), entity));
    }
    if let Some(manager) = world.get::<SolutionManager>(entity)
        && let Some(container) = world.container(entity, &manager.container)
    {
        for e in container.entities() {
            if let Some(solution) = world.get::<SolutionComponent>(*e) {
                list.push((solution.id.clone(), *e));
            }
        }
    }
    list
}

/// Раствор сущности раствора.
pub fn solution(world: &World, solution_entity: Entity) -> Option<&Solution> {
    world
        .get::<SolutionComponent>(solution_entity)
        .map(|s| &s.solution)
}

/// Раствор по имени копией.
pub fn solution_named(world: &mut World, entity: Entity, name: &str) -> Option<(Entity, Solution)> {
    let soln = try_get_solution(world, entity, name)?;
    Some((soln, solution(world, soln)?.clone()))
}

/// Изменить раствор и провести изменение (`UpdateChemicals`).
pub fn modify<R>(
    world: &mut World,
    soln: Entity,
    change: impl FnOnce(&mut Solution, &ReagentCatalog) -> R,
) -> Option<R> {
    let catalog = reagents(world);
    let result = {
        let mut component = world.get_mut::<SolutionComponent>(soln)?;
        change(&mut component.solution, &catalog)
    };
    update_chemicals(world, soln, true);
    Some(result)
}

/// После изменения: реакции, перелив, событие, вид (`UpdateChemicals`).
pub fn update_chemicals(world: &mut World, soln: Entity, needs_reactions: bool) {
    let can_react = solution(world, soln).is_some_and(|s| s.can_react);
    if needs_reactions
        && can_react
        && let Some(hook) = world.get_resource::<ReactionHook>().copied()
    {
        (hook.0)(world, soln);
    }
    let overflow = world.get_mut::<SolutionComponent>(soln).and_then(|mut c| {
        let over = c.solution.volume - c.solution.max_volume;
        (over > FixedPoint2::ZERO).then(|| c.solution.split(over))
    });
    if let Some(overflow) = overflow {
        world.raise(
            soln,
            &mut SolutionOverflow {
                overflow,
                handled: false,
            },
        );
    }
    let owner = solution_owner(world, soln);
    world.raise(owner, &mut SolutionChanged { solution: soln });
    update_appearance(world, owner, soln);
}

/// Вид заполнения (`UpdateAppearance`): только у сущностей с `SolutionContainerVisuals`.
fn update_appearance(world: &mut World, owner: Entity, soln: Entity) {
    if world.get::<SolutionContainerVisuals>(owner).is_none() {
        return;
    }
    let Some(component) = world.get::<SolutionComponent>(soln).cloned() else {
        return;
    };
    let catalog = reagents(world);
    let solution = &component.solution;
    world.set_appearance(owner, VISUALS_FILL_FRACTION, solution.fill_fraction());
    world.set_appearance(owner, VISUALS_COLOR, solution.color(&catalog).to_hex());
    world.set_appearance(owner, VISUALS_SOLUTION_NAME, component.id.clone());
    if let Some(primary) = solution.primary_reagent() {
        world.set_appearance(owner, VISUALS_BASE_OVERRIDE, primary.prototype.clone());
    }
}

/// Отделить часть (`SplitSolution`).
pub fn split_solution(world: &mut World, soln: Entity, quantity: FixedPoint2) -> Solution {
    modify(world, soln, |s, _| s.split(quantity)).unwrap_or_default()
}

/// Отделить без перечисленных (`SplitSolutionWithout`).
pub fn split_solution_without(
    world: &mut World,
    soln: Entity,
    quantity: FixedPoint2,
    excluded: &[&str],
) -> Solution {
    modify(world, soln, |s, _| s.split_without(quantity, excluded)).unwrap_or_default()
}

/// Очистить (`RemoveAllSolution`).
pub fn remove_all_solution(world: &mut World, soln: Entity) {
    if solution(world, soln).is_some_and(|s| !s.is_empty()) {
        modify(world, soln, |s, _| s.remove_all());
    }
}

/// Вместимость (`SetCapacity`).
pub fn set_capacity(world: &mut World, soln: Entity, capacity: FixedPoint2) {
    if solution(world, soln).is_some_and(|s| s.max_volume != capacity) {
        modify(world, soln, |s, _| s.max_volume = capacity);
    }
}

/// Добавить реагент сколько влезет; `true` — влез весь (`TryAddReagent`). Возвращает и
/// принятое количество.
pub fn try_add_reagent(
    world: &mut World,
    soln: Entity,
    id: ReagentId,
    quantity: FixedPoint2,
    temperature: Option<f32>,
) -> (bool, FixedPoint2) {
    let Some(available) = solution(world, soln).map(Solution::available_volume) else {
        return (false, FixedPoint2::ZERO);
    };
    let accepted = if available > quantity {
        quantity
    } else {
        available
    };
    if accepted <= FixedPoint2::ZERO {
        return (quantity == FixedPoint2::ZERO, FixedPoint2::ZERO);
    }
    modify(world, soln, |s, catalog| match temperature {
        None => s.add_reagent(id, accepted),
        Some(t) => s.add_reagent_with_temperature(id, accepted, t, catalog),
    });
    (accepted == quantity, accepted)
}

/// Убрать реагент (`RemoveReagent`).
pub fn remove_reagent(
    world: &mut World,
    soln: Entity,
    id: &ReagentId,
    quantity: FixedPoint2,
) -> FixedPoint2 {
    let removed = world
        .get_mut::<SolutionComponent>(soln)
        .map_or(FixedPoint2::ZERO, |mut c| {
            c.solution.remove_reagent(id, quantity, false)
        });
    if removed > FixedPoint2::ZERO {
        update_chemicals(world, soln, true);
    }
    removed
}

/// Перенести часть чужого раствора (`TryTransferSolution`).
pub fn try_transfer_solution(
    world: &mut World,
    soln: Entity,
    source: &mut Solution,
    quantity: FixedPoint2,
) -> bool {
    let Some(available) = solution(world, soln).map(Solution::available_volume) else {
        return false;
    };
    let quantity = quantity.min(available).min(source.volume);
    if quantity <= FixedPoint2::ZERO {
        return false;
    }
    let taken = source.split(quantity);
    modify(world, soln, |s, catalog| s.add_solution(&taken, catalog));
    true
}

/// Влить, если влезет весь (`TryAddSolution`).
pub fn try_add_solution(world: &mut World, soln: Entity, add: &Solution) -> bool {
    if add.is_empty() {
        return true;
    }
    if solution(world, soln).is_none_or(|s| add.volume > s.available_volume()) {
        return false;
    }
    force_add_solution(world, soln, add);
    true
}

/// Влить сколько влезет (`AddSolution`); возвращает влитое.
pub fn add_solution(world: &mut World, soln: Entity, add: &Solution) -> FixedPoint2 {
    if add.is_empty() {
        return FixedPoint2::ZERO;
    }
    let Some(available) = solution(world, soln).map(Solution::available_volume) else {
        return FixedPoint2::ZERO;
    };
    let quantity = add.volume.min(available).max(FixedPoint2::ZERO);
    modify(world, soln, |s, catalog| {
        if quantity < add.volume {
            s.add_solution(&add.clone().split(quantity), catalog);
        } else {
            s.add_solution(add, catalog);
        }
    });
    quantity
}

/// Влить всё сверх вместимости (`ForceAddSolution`).
pub fn force_add_solution(world: &mut World, soln: Entity, add: &Solution) -> bool {
    if add.is_empty() {
        return false;
    }
    modify(world, soln, |s, catalog| s.add_solution(add, catalog)).is_some()
}

/// Влить и отдать перелитое сверх порога (`TryMixAndOverflow`).
pub fn try_mix_and_overflow(
    world: &mut World,
    soln: Entity,
    add: &Solution,
    overflow_threshold: FixedPoint2,
) -> Option<Solution> {
    let max = solution(world, soln)?.max_volume;
    if add.is_empty() || overflow_threshold > max {
        return None;
    }
    modify(world, soln, |s, catalog| {
        s.add_solution(add, catalog);
        let over = (s.volume - overflow_threshold).max(FixedPoint2::ZERO);
        s.split(over)
    })
}

/// Убрать каждого реагента поровну (`RemoveEachReagent`).
pub fn remove_each_reagent(world: &mut World, soln: Entity, quantity: FixedPoint2) -> Solution {
    if quantity <= FixedPoint2::ZERO {
        return Solution::default();
    }
    modify(world, soln, |s, _| {
        let mut removed = Solution::default();
        for i in (0..s.contents.len()).rev() {
            let Some((id, _)) = s.contents.get(i).cloned() else {
                continue;
            };
            let amount = s.remove_reagent(&id, quantity, false);
            removed.add_reagent(id, amount);
        }
        removed
    })
    .unwrap_or_default()
}

/// Температура (`SetTemperature`).
pub fn set_temperature(world: &mut World, soln: Entity, temperature: f32) {
    if solution(world, soln).is_some_and(|s| s.temperature != temperature) {
        modify(world, soln, |s, _| s.temperature = temperature);
    }
}

/// Тепловая энергия (`SetThermalEnergy`).
pub fn set_thermal_energy(world: &mut World, soln: Entity, energy: f32) {
    modify(world, soln, |s, catalog| {
        let heat = s.heat_capacity(catalog);
        s.temperature = if heat == 0.0 { 0.0 } else { energy / heat };
    });
}

/// Добавить тепло (`AddThermalEnergy`).
pub fn add_thermal_energy(world: &mut World, soln: Entity, energy: f32) {
    if energy == 0.0 {
        return;
    }
    modify(world, soln, |s, catalog| {
        let heat = s.heat_capacity(catalog);
        if heat != 0.0 {
            s.temperature += energy / heat;
        }
    });
}

/// Добавить тепло в пределах (`AddThermalEnergyClamped`).
pub fn add_thermal_energy_clamped(
    world: &mut World,
    soln: Entity,
    energy: f32,
    min: f32,
    max: f32,
) {
    if energy == 0.0 {
        return;
    }
    modify(world, soln, |s, catalog| {
        let heat = s.heat_capacity(catalog);
        s.temperature = (s.temperature + energy / heat).clamp(min, max);
    });
}

/// Всего реагента во всех растворах (`GetTotalPrototypeQuantity`).
pub fn total_prototype_quantity(world: &World, owner: Entity, reagent: &str) -> FixedPoint2 {
    enumerate_solutions(world, owner)
        .into_iter()
        .filter_map(|(_, e)| solution(world, e))
        .fold(FixedPoint2::ZERO, |sum, s| {
            sum + s.prototype_quantity(reagent)
        })
}

/// Создать раствор-сущность в контейнере растворов.
fn create_solution(
    world: &mut World,
    owner: Entity,
    container: &str,
    prototype: &str,
) -> Option<Entity> {
    world.ensure_container(owner, container, ContainerKind::List);
    let entity = world
        .spawn_attached(prototype, EntityCoordinates::new(owner, Vec2::ZERO))
        .ok()?;
    world
        .force_insert_into_container(entity, owner, container)
        .ok()?;
    Some(entity)
}

/// Растворы из списка прототипов (`InitializeManager`).
fn initialize_manager(world: &mut World, owner: Entity) {
    let Some(manager) = world.get::<SolutionManager>(owner).cloned() else {
        return;
    };
    world.ensure_container(owner, &manager.container, ContainerKind::List);
    let Some(list) = manager.solution_ents else {
        return;
    };
    if let Some(mut manager) = world.get_mut::<SolutionManager>(owner) {
        manager.solution_ents = None;
    }
    for prototype in list {
        create_solution(world, owner, &manager.container, &prototype);
    }
}

/// Раствор с этим именем, при надобности созданный пустым; `true` — уже был
/// (`EnsureSolution`).
pub fn ensure_solution(world: &mut World, owner: Entity, name: &str) -> Option<(Entity, bool)> {
    if world
        .get::<SolutionComponent>(owner)
        .is_some_and(|s| s.id == name)
    {
        return Some((owner, true));
    }
    if world.get::<SolutionManager>(owner).is_none() {
        world.add_comp(owner, SolutionManager::default());
    }
    if world
        .get::<SolutionManager>(owner)
        .is_some_and(|m| m.solution_ents.is_some())
    {
        initialize_manager(world, owner);
    }
    if let Some(found) = try_get_solution(world, owner, name) {
        return Some((found, true));
    }
    let container = world.get::<SolutionManager>(owner)?.container.clone();
    let created = create_solution(world, owner, &container, DEFAULT_SOLUTION_PROTOTYPE)?;
    if let Some(mut component) = world.get_mut::<SolutionComponent>(created) {
        component.id = name.to_owned();
    }
    Some((created, false))
}

macro_rules! capability_getter {
    ($fn_name:ident, $component:ident) => {
        /// Раствор возможности по её имени (`TryGet…Solution`).
        pub fn $fn_name(world: &mut World, entity: Entity) -> Option<Entity> {
            let name = world.get::<$component>(entity)?.solution.clone();
            try_get_solution(world, entity, &name)
        }
    };
}

capability_getter!(refillable_solution, RefillableSolution);
capability_getter!(drainable_solution, DrainableSolution);
capability_getter!(dumpable_solution, DumpableSolution);
capability_getter!(drawable_solution, DrawableSolution);
capability_getter!(injectable_solution, InjectableSolution);
capability_getter!(fits_in_dispenser_solution, FitsInDispenser);
capability_getter!(mixable_solution, MixableSolution);

/// Процент заполнения (`PercentFull`).
pub fn percent_full(solution: &Solution) -> f32 {
    if solution.max_volume == FixedPoint2::ZERO {
        0.0
    } else {
        solution.fill_fraction() * 100.0
    }
}

/// Как назвать объём при осмотре (`ExaminedVolumeDisplay`), как ключ выбора в строке.
pub fn examined_volume(
    examinable: &ExaminableSolution,
    solution: &Solution,
    examiner_name: &str,
) -> &'static str {
    if examinable.exact_volume {
        return "exact";
    }
    match percent_full(solution) as i32 {
        100 => "full",
        67.. => "mostlyfull",
        34.. => {
            // Одни видят наполовину полный, другие — наполовину пустой, но всегда одно.
            match examiner_name.chars().next() {
                Some(c) if c.to_lowercase().next().is_some_and(|c| c > 'm') => "halffull",
                Some(_) => "halfempty",
                None => "halffull",
            }
        }
        1.. => "mostlyempty",
        _ => "empty",
    }
}

fn hex_no_alpha(color: astrocore_math::Color) -> String {
    let hex = color.to_hex();
    hex.chars().take(7).collect()
}

/// Скрыт ли раствор от осматривающего (`CanSeeHiddenSolution`).
fn can_see(
    world: &World,
    entity: Entity,
    examinable: &ExaminableSolution,
    examiner: Entity,
) -> bool {
    !examinable.held_only || world.held_items(examiner).contains(&entity)
}

/// Осмотр раствора (`OnExamineSolution`).
fn on_examined(world: &mut World, entity: Entity, event: &mut Examined) {
    let Some(examinable) = world.get::<ExaminableSolution>(entity).cloned() else {
        return;
    };
    if !event.is_in_details_range || !can_see(world, entity, &examinable, event.examiner) {
        return;
    }
    let Some((_, solution)) = solution_named(world, entity, &examinable.solution) else {
        return;
    };
    let catalog = reagents(world);
    let primary = solution
        .primary_reagent()
        .and_then(|id| catalog.get(&id.prototype).cloned());
    let Some(primary) = primary else {
        let text = loc(
            world,
            &examinable.loc_volume,
            &[("fillLevel", "empty".into())],
        );
        event.push_markup(text, 0);
        return;
    };
    let examiner_name = world.entity_name(event.examiner);
    let fill = examined_volume(&examinable, &solution, &examiner_name);
    let text = loc(
        world,
        &examinable.loc_volume,
        &[
            ("fillLevel", fill.into()),
            ("current", solution.volume.to_string().into()),
            ("max", solution.max_volume.to_string().into()),
        ],
    );
    event.push_markup(text, 0);
    let color = solution.color(&catalog);
    let desc = loc(world, &primary.physical_desc, &[]);
    let text = loc(
        world,
        &examinable.loc_physical_quality,
        &[
            ("color", hex_no_alpha(color).into()),
            (
                "colorName",
                crate::color_naming::describe(world, color).into(),
            ),
            ("desc", desc.into()),
            ("chemCount", LocArg::from(solution.contents.len() as i64)),
        ],
    );
    event.push_markup(text, 0);
    let recognized: Vec<String> = sorted_reagents(world, &solution, &catalog)
        .into_iter()
        .filter_map(|(id, _)| {
            let info = catalog.get(&id)?;
            info.recognizable.then(|| {
                let name = loc(world, &info.name, &[]);
                loc(
                    world,
                    "examinable-solution-recognized",
                    &[
                        ("color", hex_no_alpha(info.color).into()),
                        ("chemical", name.into()),
                    ],
                )
            })
        })
        .collect();
    if recognized.is_empty() {
        return;
    }
    let list = crate::tools::format_list(world, &recognized);
    let text = loc(
        world,
        &examinable.loc_recognizable_reagents,
        &[("recognizedString", list.into())],
    );
    event.push_markup(text, 0);
}

/// Реагенты по убыванию количества, затем по названию.
fn sorted_reagents(
    world: &World,
    solution: &Solution,
    catalog: &ReagentCatalog,
) -> Vec<(String, FixedPoint2)> {
    let mut list = solution.reagent_prototypes();
    let name = |id: &str| {
        catalog
            .get(id)
            .map_or_else(|| id.to_owned(), |r| loc(world, &r.name, &[]))
    };
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| name(&a.0).cmp(&name(&b.0))));
    list
}

/// Видит ли полный состав: сам сканер или сканер в слоте глаз (`SolutionScanEvent`).
fn can_scan(world: &World, user: Entity) -> bool {
    if world.get::<SolutionScanner>(user).is_some() {
        return true;
    }
    let Some(inventory) = world.get::<Inventory>(user) else {
        return false;
    };
    inventory
        .slots
        .iter()
        .filter(|slot| slot.slot_flags.intersects(SlotFlags::EYES))
        .filter_map(|slot| world.slot_entity(user, &slot.name))
        .any(|item| world.get::<SolutionScanner>(item).is_some())
}

/// Полный состав (`GetSolutionExamine`).
pub fn solution_examine_lines(world: &World, solution: &Solution) -> Vec<String> {
    if solution.is_empty() {
        return vec![loc(world, "scannable-solution-empty-container", &[])];
    }
    let catalog = crate::solution::reagents_of(world);
    let mut lines = vec![loc(world, "scannable-solution-main-text", &[])];
    for (id, quantity) in sorted_reagents(world, solution, &catalog) {
        let (name, color) = catalog
            .get(&id)
            .map_or((id.clone(), "#ffffff".to_owned()), |r| {
                (loc(world, &r.name, &[]), hex_no_alpha(r.color))
            });
        lines.push(loc(
            world,
            "scannable-solution-chemical",
            &[
                ("type", name.into()),
                ("color", color.into()),
                ("amount", quantity.to_string().into()),
            ],
        ));
    }
    lines.push(loc(
        world,
        "scannable-solution-temperature",
        &[(
            "temperature",
            LocArg::from(solution.temperature.round() as i64),
        )],
    ));
    lines
}

/// Глагол «Раствор» для очков химика (`OnSolutionExaminableVerb`).
fn scan_verb(world: &mut World, entity: Entity, event: &mut GetVerbs) {
    if event.kind != VerbKind::Examine || !event.can_interact || !event.can_access {
        return;
    }
    let user = event.user;
    if !can_scan(world, user) {
        return;
    }
    let Some(examinable) = world.get::<ExaminableSolution>(entity).cloned() else {
        return;
    };
    if try_get_solution(world, entity, &examinable.solution).is_none()
        || !can_see(world, entity, &examinable, user)
    {
        return;
    }
    let name = examinable.solution.clone();
    let mut verb = Verb::new(
        VerbKind::Examine,
        loc(world, "scannable-solution-verb-text", &[]),
        move |world| {
            let Some((_, solution)) = solution_named(world, entity, &name) else {
                return;
            };
            let lines = solution_examine_lines(world, &solution);
            let (Some(session), Some(net)) = (
                world.get::<Actor>(user).map(|a| a.session),
                world.get::<NetEntity>(entity).copied(),
            ) else {
                return;
            };
            let response = ExamineMessage::Response {
                target: net,
                name: world.entity_name(entity),
                lines,
            };
            world.send_net_event(NetTarget::Session(session), &response);
        },
    );
    verb.message = Some(loc(world, "scannable-solution-verb-message", &[]));
    verb.icon = Some("Interface/VerbIcons/drink.svg.192dpi.png".into());
    event.verbs.push(verb);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component_as::<SolutionComponent>("Solution");
    world
        .resource_mut::<astrocore_ecs::ComponentRegistry>()
        .set_inheritance("Solution", &["solution"], &[]);
    world.register_data_component::<SolutionManager>();
    world.register_data_component::<RefillableSolution>();
    world.register_data_component::<DrainableSolution>();
    world.register_data_component::<DumpableSolution>();
    world.register_data_component::<DrawableSolution>();
    world.register_data_component::<InjectableSolution>();
    world.register_data_component::<MixableSolution>();
    world.register_data_component::<FitsInDispenser>();
    world.register_data_component::<ExaminableSolution>();
    world.register_data_component::<SolutionScanner>();
    world.register_data_component::<SolutionContainerVisuals>();
    world.register_net_serde::<SolutionContainerVisuals>();
    world.register_net_serde::<SolutionComponent>();
    world.register_net_serde::<ExaminableSolution>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<SolutionManager, ComponentInit>(|world, entity, _| {
        let container = world
            .get::<SolutionManager>(entity)
            .map(|m| m.container.clone());
        if let Some(container) = container {
            world.ensure_container(entity, &container, ContainerKind::List);
        }
    });
    world.subscribe::<SolutionManager, MapInit>(|world, entity, _| {
        initialize_manager(world, entity)
    });
    world.subscribe::<SolutionComponent, MapInit>(|world, entity, _| {
        update_chemicals(world, entity, true);
    });
    world.subscribe::<ExaminableSolution, Examined>(on_examined);
    world.subscribe::<ExaminableSolution, GetVerbs>(scan_verb);
}

#[cfg(test)]
mod tests;
