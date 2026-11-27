//! Радиация: раз в секунду каждый источник светит на каждый приёмник по прямой; сила падает
//! с расстоянием (`slope`) и на каждой клетке с преградой (`RadiationBlocker`). Приёмник
//! получает `Irradiated`: живое — урон «Radiation», коллектор — ток из газа баллона.
//! Образец — `RadiationSystem` (`GridCast`, `Blockers`), `SharedRadiationSystem`,
//! `DamageableSystem.OnIrradiated` и `RadiationCollectorSystem` SS14 @ 2f4a2aad.
//!
//! Нет пока: отладочного вида лучей, счётчика Гейгера, защиты контейнерами.

use std::collections::{BTreeMap, HashMap};

use astrocore_console::{CVarDef, CVarFlags, ConfigWorldExt};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update};
use astrocore_map::{
    AnchorStateChanged, ContainerWorldExt, EntInsertedIntoContainer, EntRemovedFromContainer,
    TileCoord, Transform, TransformWorldExt,
};
use astrocore_math::Vec2;
use astrocore_net::AppearanceWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::damage::{DamageSpecifier, Damageable, DamageableWorldExt};
use crate::doors::{DoorState, DoorStateChanged};
use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::interaction::ActivateInWorld;
use crate::popups::{PopupType, PopupWorldExt};
use crate::power::PowerSupplier;

/// Слабее этого излучение не считается (`radiation.min_intensity`).
pub const MIN_INTENSITY: CVarDef<f32> = CVarDef::new(
    "radiation.min_intensity",
    0.1,
    CVarFlags::SERVER,
    "Наименьшая сила излучения.",
);
/// Секунд между расчётами (`radiation.gridcast.update_rate`).
pub const UPDATE_RATE: CVarDef<f32> = CVarDef::new(
    "radiation.gridcast.update_rate",
    1.0,
    CVarFlags::SERVER,
    "Секунд между расчётами излучения.",
);
/// Дальше этого излучение не доходит (`radiation.gridcast.max_distance`).
pub const MAX_DISTANCE: CVarDef<f32> = CVarDef::new(
    "radiation.gridcast.max_distance",
    50.0,
    CVarFlags::SERVER,
    "Дальность излучения, м.",
);

/// Источник излучения (`RadiationSourceComponent`): рад/с на расстоянии ноль, убывание на
/// метр (`slope`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadiationSource {
    #[serde(default = "one")]
    pub intensity: f32,
    #[serde(default = "half")]
    pub slope: f32,
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// Приёмник излучения (`RadiationReceiverComponent`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadiationReceiver {
    #[serde(skip)]
    pub current_radiation: f32,
}

/// Преграда в клетке (`RadiationBlockerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadiationBlocker {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, rename = "resistance")]
    pub rad_resistance: f32,
    #[serde(skip)]
    pub current_position: Option<(Entity, TileCoord)>,
}

/// Сопротивление клеток гридов (`RadiationGridResistanceComponent`). Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct RadiationResistance {
    pub per_tile: HashMap<(Entity, i32, i32), f32>,
}

/// Коллектор излучения (`RadiationCollectorComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadiationCollector {
    #[serde(default = "one")]
    pub charge_modifier: f32,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub radiation_reactive_gases: Vec<ReactiveGas>,
    #[serde(skip)]
    pub power_ticks_left: u32,
}

/// Газ, который коллектор превращает в ток (`RadiationReactiveGas`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactiveGas {
    pub reactant_prototype: String,
    #[serde(default = "one")]
    pub power_generation_efficiency: f32,
    #[serde(default)]
    pub reactant_breakdown_rate: f32,
    #[serde(default)]
    pub byproduct: Option<String>,
    #[serde(default)]
    pub molar_ratio: f32,
}

fn one() -> f32 {
    1.0
}
fn half() -> f32 {
    0.5
}
fn yes() -> bool {
    true
}

/// Облучение (`OnIrradiatedEvent`). На приёмнике.
#[derive(Debug, Clone, Copy)]
pub struct Irradiated {
    /// Секунд облучения.
    pub frame_time: f32,
    pub rads_per_second: f32,
    pub origin: Option<Entity>,
}

impl Irradiated {
    pub fn total_rads(&self) -> f32 {
        self.rads_per_second * self.frame_time
    }
}

/// Контейнер для баллона коллектора (`GasTankContainer`).
pub const GAS_TANK_CONTAINER: &str = "gas_tank";
/// Вид коллектора (`RadiationCollectorVisuals`).
pub const COLLECTOR_STATE: &str = "enum.RadiationCollectorVisuals.VisualState";
pub const COLLECTOR_TANK: &str = "enum.RadiationCollectorVisuals.TankInserted";
pub const COLLECTOR_PRESSURE: &str = "enum.RadiationCollectorVisuals.PressureState";

/// Сила источника (`SetIntensity`).
pub fn set_intensity(world: &mut World, entity: Entity, intensity: f32) {
    if let Some(mut source) = world.get_mut::<RadiationSource>(entity) {
        source.intensity = intensity;
    }
}

/// Изменить силу источника, не ниже нуля (`ChangeIntensity`).
pub fn change_intensity(world: &mut World, entity: Entity, delta: f32) {
    if let Some(mut source) = world.get_mut::<RadiationSource>(entity) {
        source.intensity = (source.intensity + delta).max(0.0);
    }
}

// Преграды.

fn cell_of(world: &World, entity: Entity) -> Option<(Entity, TileCoord)> {
    let transform = world.get::<Transform>(entity)?;
    if !transform.anchored() {
        return None;
    }
    let grid = transform.grid()?;
    Some((
        grid,
        TileCoord::containing(transform.local_position(), transform.level()),
    ))
}

fn remove_tile(world: &mut World, entity: Entity) {
    let Some(blocker) = world.get::<RadiationBlocker>(entity).cloned() else {
        return;
    };
    let Some((grid, cell)) = blocker.current_position else {
        return;
    };
    let mut map = world.get_resource_or_insert_with(RadiationResistance::default);
    let key = (grid, cell.x, cell.y);
    if let Some(value) = map.per_tile.get_mut(&key) {
        *value -= blocker.rad_resistance;
        if *value <= 0.0 {
            map.per_tile.remove(&key);
        }
    }
    if let Some(mut blocker) = world.get_mut::<RadiationBlocker>(entity) {
        blocker.current_position = None;
    }
}

fn add_tile(world: &mut World, entity: Entity) {
    remove_tile(world, entity);
    let Some(blocker) = world.get::<RadiationBlocker>(entity).cloned() else {
        return;
    };
    if !blocker.enabled || blocker.rad_resistance <= 0.0 {
        return;
    }
    let Some((grid, cell)) = cell_of(world, entity) else {
        return;
    };
    *world
        .get_resource_or_insert_with(RadiationResistance::default)
        .per_tile
        .entry((grid, cell.x, cell.y))
        .or_default() += blocker.rad_resistance;
    if let Some(mut blocker) = world.get_mut::<RadiationBlocker>(entity) {
        blocker.current_position = Some((grid, cell));
    }
}

/// Включить или выключить преграду (`SetEnabled`).
pub fn set_blocker_enabled(world: &mut World, entity: Entity, enabled: bool) {
    let Some(mut blocker) = world.get_mut::<RadiationBlocker>(entity) else {
        return;
    };
    if blocker.enabled == enabled {
        return;
    }
    blocker.enabled = enabled;
    if enabled {
        add_tile(world, entity);
    } else {
        remove_tile(world, entity);
    }
}

// Расчёт.

/// Клетки отрезка от клетки к клетке (`GridLineEnumerator`), включая концы.
pub fn grid_line(from: (i32, i32), to: (i32, i32)) -> Vec<(i32, i32)> {
    let (mut x, mut y) = from;
    let dx = (to.0 - x).abs();
    let dy = -(to.1 - y).abs();
    let sx = if x < to.0 { 1 } else { -1 };
    let sy = if y < to.1 { 1 } else { -1 };
    let mut error = dx + dy;
    let mut cells = vec![(x, y)];
    while (x, y) != to {
        let twice = 2 * error;
        if twice >= dy {
            error += dy;
            x += sx;
        }
        if twice <= dx {
            error += dx;
            y += sy;
        }
        cells.push((x, y));
    }
    cells
}

/// Сила излучения источника у приёмника (`Irradiate`, `Gridcast`).
fn irradiate(
    world: &World,
    source: (Entity, f32, f32, Vec2),
    destination: Entity,
    destination_at: Vec2,
    min: f32,
    max_distance: f32,
) -> f32 {
    let (source_entity, intensity, slope, source_at) = source;
    let distance = (destination_at - source_at).length();
    if distance > max_distance {
        return 0.0;
    }
    let mut rads = intensity - slope * distance;
    if rads < min {
        return 0.0;
    }
    let Some(resistance) = world.get_resource::<RadiationResistance>() else {
        return rads;
    };
    if resistance.per_tile.is_empty() {
        return rads;
    }
    // Гриды, по которым идёт луч: свои у источника и приёмника.
    let mut grids: Vec<Entity> = [source_entity, destination]
        .iter()
        .filter_map(|e| world.get::<Transform>(*e).and_then(Transform::grid))
        .collect();
    grids.dedup();
    for grid in grids {
        let Some(placement) = world.world_transform(grid) else {
            continue;
        };
        let a = placement.to_local(source_at);
        let b = placement.to_local(destination_at);
        let from = (a.x.floor() as i32, a.y.floor() as i32);
        let to = (b.x.floor() as i32, b.y.floor() as i32);
        for (x, y) in grid_line(from, to) {
            if let Some(value) = resistance.per_tile.get(&(grid, x, y)) {
                rads -= value;
                if rads <= min {
                    return 0.0;
                }
            }
        }
    }
    rads
}

fn update(world: &mut World, mut accumulator: bevy_ecs::system::Local<f32>) {
    let rate = world.cvar(&UPDATE_RATE);
    *accumulator += world.resource::<GameTime>().frame_time();
    if *accumulator < rate {
        return;
    }
    *accumulator = 0.0;
    let min = world.cvar(&MIN_INTENSITY);
    let max_distance = world.cvar(&MAX_DISTANCE);
    let source_entities: Vec<(Entity, RadiationSource)> = world
        .query::<(Entity, &RadiationSource)>()
        .iter(world)
        .filter(|(_, s)| s.enabled)
        .map(|(e, s)| (e, *s))
        .collect();
    let mut sources = Vec::new();
    for (entity, source) in source_entities {
        let (Some(at), None) = (
            world.map_coordinates(entity),
            world.containing_container(entity),
        ) else {
            continue;
        };
        let count = crate::stack::StackWorldExt::stack_count(world, entity) as f32;
        sources.push((entity, source.intensity * count, source.slope, at));
    }
    let receivers: Vec<Entity> = world
        .query::<(Entity, &RadiationReceiver)>()
        .iter(world)
        .map(|(e, _)| e)
        .collect();
    let mut totals = Vec::new();
    for receiver in receivers {
        let Some(at) = world.map_coordinates(receiver) else {
            continue;
        };
        let rads: f32 = sources
            .iter()
            .filter(|(e, _, _, s)| *e != receiver && s.map == at.map)
            .map(|(e, intensity, slope, s)| {
                irradiate(
                    world,
                    (*e, *intensity, *slope, s.position),
                    receiver,
                    at.position,
                    min,
                    max_distance,
                )
            })
            .sum();
        totals.push((receiver, rads));
    }
    for (receiver, rads) in totals {
        if let Some(mut component) = world.get_mut::<RadiationReceiver>(receiver) {
            component.current_radiation = rads;
        }
        if rads > 0.0 {
            world.raise(
                receiver,
                &mut Irradiated {
                    frame_time: rate,
                    rads_per_second: rads,
                    origin: None,
                },
            );
        }
    }
    collectors_post_sync(world);
}

// Коллектор.

fn tank_of(world: &World, collector: Entity) -> Option<Entity> {
    world
        .container(collector, GAS_TANK_CONTAINER)?
        .entities()
        .first()
        .copied()
        .filter(|t| crate::gas_tank::tank_air(world, *t).is_some())
}

fn pressure_state(world: &World, tank: Option<Entity>) -> i64 {
    let one_atmosphere = crate::atmos::gas::ONE_ATMOSPHERE;
    match tank
        .and_then(|t| crate::gas_tank::tank_air(world, t))
        .map(|a| a.pressure())
    {
        Some(p) if p >= 6.66 * one_atmosphere => 3,
        Some(p) if p >= 3.33 * one_atmosphere => 2,
        Some(p) if p >= 10.0 => 1,
        _ => 0,
    }
}

fn update_tank_appearance(world: &mut World, collector: Entity) {
    let tank = tank_of(world, collector);
    world.set_appearance(collector, COLLECTOR_TANK, tank.is_some());
    let state = pressure_state(world, tank);
    world.set_appearance(collector, COLLECTOR_PRESSURE, state);
}

fn update_machine_appearance(world: &mut World, collector: Entity) {
    let enabled = world
        .get::<RadiationCollector>(collector)
        .is_some_and(|c| c.enabled);
    world.set_appearance(
        collector,
        COLLECTOR_STATE,
        if enabled { "Active" } else { "Deactive" },
    );
}

/// Включить или выключить коллектор (`SetCollectorEnabled`).
pub fn set_collector_enabled(
    world: &mut World,
    collector: Entity,
    enabled: bool,
    user: Option<Entity>,
) {
    let Some(mut component) = world.get_mut::<RadiationCollector>(collector) else {
        return;
    };
    component.enabled = enabled;
    if user.is_some() {
        let key = if enabled {
            "radiation-collector-component-use-on"
        } else {
            "radiation-collector-component-use-off"
        };
        let text = loc(world, key, &[]);
        world.popup_entity(&text, collector, PopupType::Small);
    }
    update_machine_appearance(world, collector);
}

/// Ток из облучённого газа (`OnRadiation`).
fn on_collector_irradiated(world: &mut World, collector: Entity, event: &mut Irradiated) {
    let Some(component) = world.get::<RadiationCollector>(collector).cloned() else {
        return;
    };
    if !component.enabled || component.radiation_reactive_gases.is_empty() {
        return;
    }
    let Some(tank) = tank_of(world, collector) else {
        return;
    };
    let total = event.total_rads();
    let charge = crate::gas_tank::with_tank_air(world, tank, |air| {
        let mut charge = 0.0;
        for reactive in &component.radiation_reactive_gases {
            let Some(index) = crate::atmos::gas::gas_index(&reactive.reactant_prototype) else {
                continue;
            };
            let moles = air.moles[index];
            let delta = total * moles * reactive.reactant_breakdown_rate;
            let temperature_mod = 1.5 * air.temperature() / (150.0 + air.temperature());
            charge += total
                * moles
                * component.charge_modifier
                * reactive.power_generation_efficiency
                * temperature_mod;
            if delta > 0.0 {
                air.adjust_moles(index, -delta.min(moles));
            }
            if let Some(byproduct) = reactive
                .byproduct
                .as_deref()
                .and_then(crate::atmos::gas::gas_index)
            {
                air.adjust_moles(byproduct, delta * reactive.molar_ratio);
            }
        }
        charge
    })
    .unwrap_or(0.0);
    // Ток держится две секунды после облучения (`PowerTicksLeft`): у нас расчёт раз в секунду.
    if let Some(mut collector_component) = world.get_mut::<RadiationCollector>(collector) {
        collector_component.power_ticks_left = 2;
    }
    if let Some(mut supplier) = world.get_mut::<PowerSupplier>(collector) {
        supplier.supply_rate = charge;
    }
    let state = pressure_state(world, Some(tank));
    world.set_appearance(collector, COLLECTOR_PRESSURE, state);
}

/// Без облучения ток пропадает (`PostSync`).
fn collectors_post_sync(world: &mut World) {
    let collectors: Vec<Entity> = world
        .query::<(Entity, &RadiationCollector)>()
        .iter(world)
        .map(|(e, _)| e)
        .collect();
    for collector in collectors {
        let mut stop = false;
        if let Some(mut component) = world.get_mut::<RadiationCollector>(collector) {
            if component.power_ticks_left > 0 {
                component.power_ticks_left -= 1;
            } else {
                stop = true;
            }
        }
        if stop && let Some(mut supplier) = world.get_mut::<PowerSupplier>(collector) {
            supplier.supply_rate = 0.0;
        }
    }
}

/// Облучение живого (`DamageableSystem.OnIrradiated`): урон «Radiation» в радах.
fn on_damageable_irradiated(world: &mut World, entity: Entity, event: &mut Irradiated) {
    let Some(types) = world
        .get::<Damageable>(entity)
        .map(|d| d.radiation_damage_types.clone())
    else {
        return;
    };
    let value = FixedPoint2::from_f32(event.total_rads());
    let damage = DamageSpecifier {
        types: types
            .into_iter()
            .map(|t| (t, value))
            .collect::<BTreeMap<_, _>>(),
    };
    world.change_damage(entity, &damage, false, false, event.origin);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<RadiationSource>();
    world.register_data_component::<RadiationReceiver>();
    world.register_data_component::<RadiationBlocker>();
    world.register_data_component::<RadiationCollector>();
    if !side.is_server() {
        return;
    }
    world.register_cvar(&MIN_INTENSITY);
    world.register_cvar(&UPDATE_RATE);
    world.register_cvar(&MAX_DISTANCE);
    world.init_resource::<RadiationResistance>();
    world.subscribe::<RadiationBlocker, MapInit>(|world, entity, _| add_tile(world, entity));
    world.subscribe::<RadiationBlocker, AnchorStateChanged>(|world, entity, event| {
        if event.anchored {
            add_tile(world, entity);
        } else {
            remove_tile(world, entity);
        }
    });
    world.subscribe::<RadiationBlocker, astrocore_ecs::EntityTerminating>(|world, entity, _| {
        remove_tile(world, entity);
    });
    world.subscribe::<RadiationBlocker, DoorStateChanged>(|world, entity, event| {
        match event.state {
            DoorState::Open => set_blocker_enabled(world, entity, false),
            DoorState::Closed => set_blocker_enabled(world, entity, true),
            _ => {}
        }
    });
    world.subscribe::<Damageable, Irradiated>(on_damageable_irradiated);
    world.subscribe::<RadiationCollector, Irradiated>(on_collector_irradiated);
    world.subscribe::<RadiationCollector, MapInit>(|world, collector, _| {
        update_tank_appearance(world, collector);
        update_machine_appearance(world, collector);
    });
    world.subscribe::<RadiationCollector, EntInsertedIntoContainer>(|world, collector, _| {
        update_tank_appearance(world, collector);
    });
    world.subscribe::<RadiationCollector, EntRemovedFromContainer>(|world, collector, _| {
        update_tank_appearance(world, collector);
    });
    world.subscribe::<RadiationCollector, ActivateInWorld>(|world, collector, event| {
        if !event.complex || event.handled {
            return;
        }
        let enabled = world
            .get::<RadiationCollector>(collector)
            .is_some_and(|c| c.enabled);
        set_collector_enabled(world, collector, !enabled, Some(event.user));
        event.handled = true;
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
