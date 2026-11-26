//! Атмосфера (ADR-0007): воздух клеток гридов, выравнивание давления и температуры,
//! разгерметизация. Образец — `AtmosphereSystem` (`Processing`, `GridAtmosphere`, `Map`,
//! `Utils`), `AirtightSystem`, `TileAtmosCollectionSerializer` SS14 @ 2f4a2aad.

pub mod fire;
pub mod gas;
pub mod grid;
pub mod overlay;
pub mod sim;

use std::collections::HashMap;
use std::time::Instant;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_data::{DataMap, DataNode};
use astrocore_ecs::{
    ComponentInit, EntityTerminating, EventWorldExt, GameTime, GameWorldExt, Paused, Update,
};
use astrocore_map::{
    AnchorStateChanged, Map, MapGrid, MapLoaded, MapPreserved, TileChanged, TileCoord,
    TileDefinitions, Transform,
};
use astrocore_prototypes::Prototypes;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::Side;
use crate::airtight::{AirBlockedDirection, Airtight, AirtightChanged};

pub use gas::{GasData, GasMixture};
pub use grid::GridAtmosphere;
use grid::Hotspot;
use grid::*;
pub use sim::AtmosConfig;
use sim::*;

/// Проверка бюджета времени — раз в столько клеток (`LagCheckIterations`).
const LAG_CHECK_ITERATIONS: usize = 30;
const INVALID_COORDINATES_LAG_CHECK_ITERATIONS: usize = 50;
const SPACE_WIND_SOUND_COOLDOWN_CYCLES: i32 = 75;

/// Свойства тайла для атмосферы (`ContentTileDefinition`: `isSpace`, `thermalConductivity`,
/// `heatCapacity`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileAtmosDef {
    pub map_atmosphere: bool,
    pub thermal_conductivity: f32,
    pub heat_capacity: f32,
}

impl Default for TileAtmosDef {
    fn default() -> Self {
        Self {
            map_atmosphere: false,
            thermal_conductivity: 0.04,
            heat_capacity: gas::MINIMUM_HEAT_CAPACITY,
        }
    }
}

/// Свойства тайлов по номеру определения. Ресурс, заполняется при первом обращении.
#[derive(Resource, Debug, Default)]
pub struct TileAtmosDefs(HashMap<u16, TileAtmosDef>);

/// Ход обработки по гридам (`_currentRunAtmosphere`, `_simulationPaused`). Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct AtmosRun {
    paused: bool,
    grids: Vec<Entity>,
    index: usize,
    space_wind_sound_cooldown: i32,
    hotspot_sound_cooldown: i32,
}

/// Свойства газов (ресурс создаётся из прототипов при первом обращении).
pub fn gas_data(world: &mut World) -> GasData {
    if let Some(data) = world.get_resource::<GasData>() {
        return data.clone();
    }
    let data = world
        .get_resource::<Prototypes>()
        .map(GasData::from_prototypes)
        .unwrap_or_default();
    world.insert_resource(data.clone());
    data
}

fn tile_def(world: &mut World, type_id: u16) -> TileAtmosDef {
    if let Some(def) = world
        .get_resource::<TileAtmosDefs>()
        .and_then(|d| d.0.get(&type_id))
    {
        return *def;
    }
    let name = world
        .get_resource::<TileDefinitions>()
        .and_then(|d| d.get(type_id))
        .map(|d| d.id.clone());
    let mut def = TileAtmosDef::default();
    if let (Some(name), Some(prototypes)) = (name, world.get_resource::<Prototypes>())
        && let Some(node) = prototypes.resolved_node("tile", &name)
    {
        let number = |key: &str| {
            node.get(key)
                .and_then(DataNode::as_str)
                .and_then(|v| v.parse::<f32>().ok())
        };
        def.map_atmosphere = node
            .get("isSpace")
            .and_then(DataNode::as_str)
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        if let Some(value) = number("thermalConductivity") {
            def.thermal_conductivity = value;
        }
        if let Some(value) = number("heatCapacity") {
            def.heat_capacity = value;
        }
    }
    world
        .get_resource_or_insert_with(TileAtmosDefs::default)
        .0
        .insert(type_id, def);
    def
}

// Непроницаемость.

fn direction_from_name(name: &str) -> u8 {
    match name.trim() {
        "North" => NORTH,
        "South" => SOUTH,
        "East" => EAST,
        "West" => WEST,
        "NorthEast" => NORTH | EAST,
        "NorthWest" => NORTH | WEST,
        "SouthEast" => SOUTH | EAST,
        "SouthWest" => SOUTH | WEST,
        "All" => ALL_DIRECTIONS,
        "Invalid" => 0,
        other => other.parse().unwrap_or(0),
    }
}

/// Направления из данных (`airBlockedDirection`); без поля — все.
pub fn initial_blocked_directions(airtight: &Airtight) -> u8 {
    match &airtight.air_blocked_direction {
        None => ALL_DIRECTIONS,
        Some(AirBlockedDirection::One(name)) => name
            .split(',')
            .fold(0, |bits, n| bits | direction_from_name(n)),
        Some(AirBlockedDirection::Many(names)) => names
            .iter()
            .fold(0, |bits, n| bits | direction_from_name(n)),
    }
}

/// Повернуть направления (`AirtightSystem.Rotate`): угол SS14 — юг 0, восток π/2.
fn rotate_directions(directions: u8, rotation: f32) -> u8 {
    if directions == ALL_DIRECTIONS || rotation == 0.0 {
        return directions;
    }
    // Номер четверти: юг 0, восток 1, север 2, запад 3.
    let quarter_of = |bit: u8| match bit {
        SOUTH => 0,
        EAST => 1,
        NORTH => 2,
        _ => 3,
    };
    let bit_of = |quarter: i32| match quarter.rem_euclid(4) {
        0 => SOUTH,
        1 => EAST,
        2 => NORTH,
        _ => WEST,
    };
    let turn = (rotation / std::f32::consts::FRAC_PI_2).round() as i32;
    [NORTH, SOUTH, EAST, WEST]
        .into_iter()
        .filter(|bit| directions & bit != 0)
        .fold(0, |acc, bit| acc | bit_of(quarter_of(bit) + turn))
}

/// Непроницаемость клетки по привязанным сущностям (`GetAirtightData`).
fn airtight_data(world: &World, grid: Entity, coord: TileCoord) -> AirtightData {
    let Some(map_grid) = world.get::<MapGrid>(grid) else {
        return AirtightData::default();
    };
    let mut data = AirtightData::default();
    for entity in map_grid.anchored_at(coord) {
        let Some(airtight) = world.get::<Airtight>(*entity) else {
            continue;
        };
        data.fix_vacuum |= airtight.fix_vacuum;
        if !airtight.air_blocked {
            continue;
        }
        let rotation = if airtight.rotate_air_blocked {
            world
                .get::<Transform>(*entity)
                .map_or(0.0, |t| t.local_rotation().0)
        } else {
            0.0
        };
        data.blocked_directions |=
            rotate_directions(initial_blocked_directions(airtight), rotation);
        data.no_air_when_blocked |= airtight.no_air_when_fully_air_blocked;
    }
    data
}

/// `UpdateAirtightData`.
fn update_airtight_data(world: &World, grid_entity: Entity, grid: &mut GridAtmosphere, tile: u32) {
    let old = grid.tile(tile).airtight.blocked_directions;
    let data = if grid.tile(tile).no_grid_tile {
        AirtightData::default()
    } else {
        airtight_data(world, grid_entity, grid.tile(tile).coord)
    };
    grid.tile_mut(tile).airtight = data;
    if data.blocked_directions != old
        && let Some(group) = grid.tile(tile).excited_group
    {
        excited_group_dispose(grid, group);
    }
}

// Пересчёт клеток.

fn has_grid_tile(world: &World, grid: Entity, coord: TileCoord) -> Option<u16> {
    let tile = world.get::<MapGrid>(grid)?.tile(coord);
    (tile.type_id != 0).then_some(tile.type_id)
}

fn queue_tile_trim(grid: &mut GridAtmosphere, tile: u32) {
    if !grid.tile(tile).trim_queued {
        grid.tile_mut(tile).trim_queued = true;
        grid.possibly_disconnected.push(tile);
    }
    for i in 0..4 {
        let coord = offset(grid.tile(tile).coord, i);
        if let Some(adj) = grid.tile_index(coord)
            && grid.tile(adj).no_grid_tile
            && !grid.tile(adj).trim_queued
        {
            grid.tile_mut(adj).trim_queued = true;
            grid.possibly_disconnected.push(adj);
        }
    }
}

fn remove_map_atmos(grid: &mut GridAtmosphere, tile: u32) {
    grid.map_tiles.remove(tile);
    let t = grid.tile_mut(tile);
    t.map_atmosphere = false;
    t.in_map_tiles = false;
    t.air = None;
    t.air_archived = None;
    t.archived_cycle = 0;
    t.last_share = 0.0;
    t.space = false;
}

/// `UpdateTileData`.
fn update_tile_data(world: &mut World, grid_entity: Entity, grid: &mut GridAtmosphere, tile: u32) {
    let coord = grid.tile(tile).coord;
    let map_atmosphere = match has_grid_tile(world, grid_entity, coord) {
        Some(type_id) => {
            let def = tile_def(world, type_id);
            let t = grid.tile_mut(tile);
            t.thermal_conductivity = def.thermal_conductivity;
            t.heat_capacity = def.heat_capacity;
            t.no_grid_tile = false;
            def.map_atmosphere
        }
        None => {
            let t = grid.tile_mut(tile);
            t.thermal_conductivity = 0.5;
            t.heat_capacity = f32::INFINITY;
            if !t.no_grid_tile {
                t.no_grid_tile = true;
                queue_tile_trim(grid, tile);
            }
            true
        }
    };
    update_airtight_data(world, grid_entity, grid, tile);
    if map_atmosphere {
        if !grid.tile(tile).map_atmosphere {
            // Атмосфера карты (`MapAtmosphereComponent`) — космос, пока карты без неё.
            let t = grid.tile_mut(tile);
            t.air = Some(GasMixture::space());
            t.space = true;
            t.map_atmosphere = true;
            t.in_map_tiles = true;
            grid.map_tiles.insert(tile);
        }
        return;
    }
    if grid.tile(tile).map_atmosphere {
        remove_map_atmos(grid, tile);
    }
}

/// `ShuntTileVolume`: воздух закрытой клетки уходит соседям.
fn shunt_tile_volume(grid: &mut GridAtmosphere, gas_data: &GasData, tile: u32) {
    let neighbours: Vec<u32> = grid
        .tile(tile)
        .adjacent
        .iter()
        .flatten()
        .copied()
        .filter(|adj| grid.tile(*adj).air.is_some())
        .collect();
    if !neighbours.is_empty() {
        let ratio = 1.0 / neighbours.len() as f32;
        let mix = grid
            .tile_mut(tile)
            .air
            .as_mut()
            .unwrap()
            .remove_ratio(ratio);
        for adj in neighbours {
            gas_data.merge(grid.tile_mut(adj).air.as_mut().unwrap(), &mix);
        }
    }
    let t = grid.tile_mut(tile);
    t.air = None;
    t.air_archived = None;
    t.archived_cycle = 0;
    t.last_share = 0.0;
}

/// `GridFixTileVacuum`: новая клетка берёт воздух у соседей.
fn grid_fix_tile_vacuum(grid: &mut GridAtmosphere, gas_data: &GasData, tile: u32) {
    {
        let t = grid.tile_mut(tile);
        t.air_archived = None;
        t.archived_cycle = 0;
    }
    let neighbours: Vec<u32> = grid
        .tile(tile)
        .adjacent
        .iter()
        .flatten()
        .copied()
        .filter(|adj| grid.tile(*adj).air.is_some())
        .collect();
    if neighbours.is_empty() {
        return;
    }
    let ratio = 1.0 / (neighbours.len() as f32 + 1.0);
    let mut total_temperature = 0.0;
    for adj in &neighbours {
        // Как в SS14: температура клетки, а не её воздуха.
        total_temperature += grid.tile(*adj).temperature;
        let mix = grid
            .tile_mut(*adj)
            .air
            .as_mut()
            .unwrap()
            .remove_ratio(ratio);
        gas_data.merge(grid.tile_mut(tile).air.as_mut().unwrap(), &mix);
    }
    grid.tile_mut(tile)
        .air
        .as_mut()
        .unwrap()
        .set_temperature(total_temperature / neighbours.len() as f32);
}

/// `UpdateTileAir`.
fn update_tile_air(grid: &mut GridAtmosphere, gas_data: &GasData, tile: u32, volume: f32) {
    if grid.tile(tile).map_atmosphere {
        return;
    }
    let data = grid.tile(tile).airtight;
    if data.blocked_directions == ALL_DIRECTIONS && data.no_air_when_blocked {
        if grid.tile(tile).air.is_some() {
            shunt_tile_volume(grid, gas_data, tile);
        }
        return;
    }
    if grid.tile(tile).air.is_some() {
        return;
    }
    grid.tile_mut(tile).air = Some(GasMixture::new(volume).with_temperature(gas::T20C));
    if data.fix_vacuum {
        grid_fix_tile_vacuum(grid, gas_data, tile);
    }
}

/// `UpdateAdjacentTiles`.
fn update_adjacent_tiles(grid: &mut GridAtmosphere, tile: u32, activate: bool) {
    let blocked = grid.tile(tile).airtight.blocked_directions;
    if activate {
        add_active_tile(grid, tile);
    }
    grid.tile_mut(tile).adjacent_bits = 0;
    for i in 0..4 {
        let dir = direction(i);
        let coord = offset(grid.tile(tile).coord, i);
        let adjacent = if !grid.tile(tile).no_grid_tile {
            grid.get_or_new(coord, true)
        } else if let Some(adjacent) = grid.tile_index(coord) {
            adjacent
        } else {
            grid.tile_mut(tile).adjacent_bits &= !dir;
            grid.tile_mut(tile).adjacent[i] = None;
            continue;
        };
        let adjacent_blocked = grid.tile(adjacent).airtight.blocked_directions;
        if activate {
            add_active_tile(grid, adjacent);
        }
        let opposite = opposite_index(i);
        let opposite_dir = direction(opposite);
        if adjacent_blocked & opposite_dir != 0 || blocked & dir != 0 {
            grid.tile_mut(tile).adjacent_bits &= !dir;
            grid.tile_mut(adjacent).adjacent_bits &= !opposite_dir;
            grid.tile_mut(tile).adjacent[i] = None;
            grid.tile_mut(adjacent).adjacent[opposite] = None;
        } else {
            grid.tile_mut(tile).adjacent_bits |= dir;
            grid.tile_mut(adjacent).adjacent_bits |= opposite_dir;
            grid.tile_mut(tile).adjacent[i] = Some(adjacent);
            grid.tile_mut(adjacent).adjacent[opposite] = Some(tile);
        }
        let a = grid.tile(adjacent);
        if a.adjacent_bits & a.monstermos.current_transfer_direction
            != a.monstermos.current_transfer_direction
        {
            grid.tile_mut(adjacent)
                .monstermos
                .current_transfer_direction = 0;
        }
    }
    let t = grid.tile(tile);
    if t.adjacent_bits & t.monstermos.current_transfer_direction
        != t.monstermos.current_transfer_direction
    {
        grid.tile_mut(tile).monstermos.current_transfer_direction = 0;
    }
}

/// `TrimDisconnectedMapTiles`.
fn trim_disconnected_map_tiles(world: &World, grid_entity: Entity, grid: &mut GridAtmosphere) {
    let candidates = std::mem::take(&mut grid.possibly_disconnected);
    for tile in candidates {
        grid.tile_mut(tile).trim_queued = false;
        if !grid.tile(tile).no_grid_tile || grid.tile(tile).removed {
            continue;
        }
        let coord = grid.tile(tile).coord;
        let connected =
            (0..4).any(|i| has_grid_tile(world, grid_entity, offset(coord, i)).is_some());
        if !connected {
            remove_active_tile(grid, tile, true);
            grid.map_tiles.remove(tile);
            grid.remove_tile(tile);
        }
    }
}

// Шаги.

struct WorldHooks<'a> {
    reactions: &'a fire::GasReactions,
}

impl Hooks for WorldHooks<'_> {
    fn react(&mut self, grid: &mut GridAtmosphere, gas: &GasData, tile: u32) {
        fire::react_tile(grid, gas, self.reactions, tile);
    }
}

/// Реакции газов (ресурс создаётся из прототипов при первом обращении).
pub fn gas_reactions(world: &mut World) -> fire::GasReactions {
    if let Some(reactions) = world.get_resource::<fire::GasReactions>() {
        return reactions.clone();
    }
    let reactions = world
        .get_resource::<Prototypes>()
        .map(fire::GasReactions::from_prototypes)
        .unwrap_or_default();
    world.insert_resource(reactions.clone());
    reactions
}

const HOTSPOT_SOUND_COOLDOWN_CYCLES: i32 = 200;

/// Пламя на клетке горит или гаснет (`ProcessHotspot`). Возвращает пламя для события
/// `TileFire`.
fn process_hotspot(
    grid: &mut GridAtmosphere,
    gas_data: &GasData,
    reactions: &fire::GasReactions,
    tile: u32,
    effects: &mut HotspotEffects,
) {
    if !grid.tile(tile).hotspot.valid {
        grid.hotspot_tiles.remove(tile);
        return;
    }
    add_active_tile(grid, tile);
    if !grid.tile(tile).hotspot.skipped_first_process {
        grid.tile_mut(tile).hotspot.skipped_first_process = true;
        return;
    }
    if let Some(group) = grid.tile(tile).excited_group {
        let group = grid.group_mut(group);
        group.breakdown_cooldown = 0;
        group.dismantle_cooldown = 0;
    }
    let t = grid.tile(tile);
    let ignitable = t
        .air
        .as_ref()
        .is_some_and(|air| gas_data.is_fuel(air) && gas_data.is_oxidizer(air));
    if t.hotspot.temperature < gas::FIRE_MINIMUM_TEMPERATURE_TO_EXIST
        || t.hotspot.volume <= 1.0
        || !ignitable
    {
        grid.tile_mut(tile).hotspot = Hotspot::default();
        grid.visuals_dirty.insert(tile);
        return;
    }
    if let Some((temperature, volume)) =
        fire::perform_hotspot_exposure(grid, gas_data, reactions, tile)
    {
        effects
            .fires
            .push((grid.tile(tile).coord, temperature, volume));
    }
    if grid.tile(tile).hotspot.bypassing {
        grid.tile_mut(tile).hotspot.state = 3;
        effects.burnt.push(grid.tile(tile).coord);
        let air_temperature = grid
            .tile(tile)
            .air
            .as_ref()
            .map_or(0.0, |a| a.temperature());
        if air_temperature > gas::FIRE_MINIMUM_TEMPERATURE_TO_SPREAD {
            let radiated = air_temperature * gas::FIRE_SPREAD_RADIOSITY_SCALE;
            let adjacent = grid.tile(tile).adjacent;
            for other in adjacent.into_iter().flatten() {
                if !grid.tile(other).hotspot.valid {
                    fire::hotspot_expose(
                        grid,
                        gas_data,
                        other,
                        radiated,
                        gas::CELL_VOLUME / 4.0,
                        false,
                    );
                }
            }
        }
    } else {
        let volume = grid.tile(tile).hotspot.volume;
        grid.tile_mut(tile).hotspot.state = if volume > gas::CELL_VOLUME * 0.4 {
            2
        } else {
            1
        };
    }
    let t = grid.tile_mut(tile);
    if t.hotspot.temperature > t.max_fire_temperature_sustained {
        t.max_fire_temperature_sustained = t.hotspot.temperature;
    }
    let state = t.hotspot.state;
    let coord = t.coord;
    if effects.sound_cooldown == 0 {
        effects.sounds.push((coord, state));
    }
    effects.sound_cooldown += 1;
    if effects.sound_cooldown > HOTSPOT_SOUND_COOLDOWN_CYCLES {
        effects.sound_cooldown = 0;
    }
}

/// Что пламя сделает с миром после шага.
#[derive(Default)]
struct HotspotEffects {
    fires: Vec<(TileCoord, f32, f32)>,
    burnt: Vec<TileCoord>,
    sounds: Vec<(TileCoord, u8)>,
    sound_cooldown: i32,
}

/// Пламя коснулось сущности на клетке (`TileFireEvent`). Поднимается на сущности.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileFire {
    pub temperature: f32,
    pub volume: f32,
}

fn apply_hotspot_effects(world: &mut World, grid_entity: Entity, effects: HotspotEffects) {
    for (coord, temperature, volume) in effects.fires {
        let bounds =
            astrocore_math::Box2::centered(coord.center(), astrocore_math::Vec2::splat(1.0));
        let entities = astrocore_physics::PhysicsWorldExt::entities_intersecting(
            world,
            grid_entity,
            coord.z,
            bounds,
        );
        for entity in entities {
            world.raise(
                entity,
                &mut TileFire {
                    temperature,
                    volume,
                },
            );
        }
    }
    let burnt: Vec<String> = world
        .get_resource::<Prototypes>()
        .map(|p| {
            let mut ids: Vec<String> = p
                .ids("decal")
                .filter(|id| {
                    p.resolved_node("decal", id)
                        .and_then(|n| n.get("tags"))
                        .and_then(DataNode::as_seq)
                        .is_some_and(|tags| tags.iter().any(|t| t.as_str() == Some("burnt")))
                })
                .map(str::to_owned)
                .collect();
            ids.sort();
            ids
        })
        .unwrap_or_default();
    for coord in effects.burnt {
        if burnt.is_empty() {
            break;
        }
        let position = astrocore_math::Vec2::new(coord.x as f32, coord.y as f32);
        let existing = world
            .get::<crate::decals::GridDecals>(grid_entity)
            .map(|d| {
                d.decals
                    .values()
                    .filter(|decal| burnt.contains(&decal.id))
                    .filter(|decal| {
                        (position - decal.coordinates - astrocore_math::Vec2::splat(0.5)).length()
                            <= 0.75
                    })
                    .count()
            })
            .unwrap_or(0);
        if existing >= 4 {
            continue;
        }
        let index = crate::random::random(world).range(0, burnt.len() as i32) as usize;
        let decal = crate::decals::Decal {
            coordinates: position,
            id: burnt[index].clone(),
            color: None,
            angle: 0.0,
            z_index: 0,
            cleanable: true,
        };
        crate::decals::place(world, grid_entity, position, decal);
    }
    for (coord, state) in effects.sounds {
        let at = astrocore_map::EntityCoordinates::new(grid_entity, coord.center());
        if let Some(map) = astrocore_map::TransformWorldExt::to_map_coordinates(world, at) {
            let state = f32::from(state.max(1));
            let sound = SoundSpecifier::Collection {
                collection: "AtmosHotspot".into(),
                params: astrocore_audio::AudioParams {
                    volume: -5.0 + 5.0 * state,
                    variation: Some(0.15 / state),
                    ..Default::default()
                },
            };
            world.play_at(&sound, map, None);
        }
    }
}

fn out_of_time(start: Instant, config: &AtmosConfig) -> bool {
    start.elapsed().as_secs_f32() * 1000.0 >= config.max_process_time
}

/// `ProcessRevalidate`.
fn process_revalidate(
    world: &mut World,
    grid_entity: Entity,
    grid: &mut GridAtmosphere,
    gas_data: &GasData,
    config: &AtmosConfig,
    start: Instant,
) -> bool {
    let volume = gas::CELL_VOLUME;
    if !grid.processing_paused {
        grid.current_run_invalidated.clear();
        let coords = std::mem::take(&mut grid.invalidated);
        grid.invalidated_set.clear();
        for coord in coords {
            let tile = grid.get_or_new(coord, false);
            grid.current_run_invalidated.push_back(tile);
            update_tile_data(world, grid_entity, grid, tile);
        }
        if out_of_time(start, config) {
            return false;
        }
    }
    let mut number = 0;
    while let Some(tile) = grid.current_run_invalidated.pop_front() {
        if grid.tile(tile).removed {
            continue;
        }
        update_adjacent_tiles(grid, tile, true);
        update_tile_air(grid, gas_data, tile, volume);
        number += 1;
        if number < INVALID_COORDINATES_LAG_CHECK_ITERATIONS {
            continue;
        }
        number = 0;
        if out_of_time(start, config) {
            return false;
        }
    }
    trim_disconnected_map_tiles(world, grid_entity, grid);
    true
}

fn queue_run_tiles(grid: &mut GridAtmosphere, from: &IndexSet) {
    grid.current_run_tiles.clear();
    grid.current_run_tiles.extend(from.iter());
}

/// Общий обход очереди клеток с проверкой бюджета.
fn run_tiles(
    grid: &mut GridAtmosphere,
    config: &AtmosConfig,
    start: Instant,
    mut step: impl FnMut(&mut GridAtmosphere, u32),
) -> bool {
    let mut number = 0;
    while let Some(tile) = grid.current_run_tiles.pop_front() {
        if !grid.tile(tile).removed {
            step(grid, tile);
        }
        number += 1;
        if number < LAG_CHECK_ITERATIONS {
            continue;
        }
        number = 0;
        if out_of_time(start, config) {
            return false;
        }
    }
    true
}

/// `ProcessExcitedGroups`.
fn process_excited_groups(
    grid: &mut GridAtmosphere,
    gas_data: &GasData,
    reactions: &fire::GasReactions,
    config: &AtmosConfig,
    start: Instant,
) -> bool {
    if !grid.processing_paused {
        grid.current_run_groups.clear();
        let groups: Vec<(u32, u32)> = grid
            .live_groups
            .iter()
            .map(|id| (id, grid.group(id).generation))
            .collect();
        grid.current_run_groups.extend(groups);
    }
    let mut number = 0;
    while let Some((id, generation)) = grid.current_run_groups.pop_front() {
        let group = grid.group(id);
        if group.disposed || group.generation != generation {
            continue;
        }
        let group = grid.group_mut(id);
        group.breakdown_cooldown += 1;
        group.dismantle_cooldown += 1;
        if group.breakdown_cooldown > gas::EXCITED_GROUP_BREAKDOWN_CYCLES {
            excited_group_self_breakdown(grid, gas_data, config, id, &mut WorldHooks { reactions });
        } else if grid.group(id).dismantle_cooldown > gas::EXCITED_GROUPS_DISMANTLE_CYCLES {
            deactivate_group_tiles(grid, id);
        }
        number += 1;
        if number < LAG_CHECK_ITERATIONS {
            continue;
        }
        number = 0;
        if out_of_time(start, config) {
            return false;
        }
    }
    true
}

/// `ProcessHighPressureDelta`: звук ветра; толкание предметов выключено (`space_wind`).
fn process_high_pressure_delta(
    world: &mut World,
    grid_entity: Entity,
    grid: &mut GridAtmosphere,
    config: &AtmosConfig,
    start: Instant,
) -> bool {
    if !grid.processing_paused {
        let set = grid.high_pressure_delta.clone();
        queue_run_tiles(grid, &set);
    }
    let mut sounds: Vec<(TileCoord, f32)> = Vec::new();
    let mut cooldown = world.resource::<AtmosRun>().space_wind_sound_cooldown;
    let done = run_tiles(grid, config, start, |grid, tile| {
        let t = grid.tile(tile);
        if t.pressure_difference > 15.0 && cooldown == 0 {
            sounds.push((t.coord, t.pressure_difference));
        }
        cooldown += 1;
        if cooldown > SPACE_WIND_SOUND_COOLDOWN_CYCLES {
            cooldown = 0;
        }
        let t = grid.tile_mut(tile);
        t.pressure_difference = 0.0;
        t.last_pressure_direction = t.pressure_direction;
        t.pressure_direction = 0;
        t.pressure_specific_target = None;
        t.in_high_pressure_delta = false;
        grid.high_pressure_delta.remove(tile);
    });
    world.resource_mut::<AtmosRun>().space_wind_sound_cooldown = cooldown;
    for (coord, difference) in sounds {
        let at = astrocore_map::EntityCoordinates::new(grid_entity, coord.center());
        if let Some(map) = astrocore_map::TransformWorldExt::to_map_coordinates(world, at) {
            let sound = SoundSpecifier::Collection {
                collection: "SpaceWind".into(),
                params: astrocore_audio::AudioParams {
                    volume: (difference / 10.0).clamp(10.0, 100.0),
                    variation: Some(0.125),
                    ..Default::default()
                },
            };
            world.play_at(&sound, map, None);
        }
    }
    done
}

enum Completion {
    Return,
    Continue,
    Finished,
}

/// Один шаг обработки грида (`ProcessAtmosphere`).
fn process_atmosphere(
    world: &mut World,
    grid_entity: Entity,
    frame_time: f32,
    gas_data: &GasData,
    config: &AtmosConfig,
    start: Instant,
) -> Completion {
    let Some(mut grid) = world
        .get_mut::<GridAtmosphere>(grid_entity)
        .map(|mut g| std::mem::take(&mut *g))
    else {
        return Completion::Continue;
    };
    let reactions = gas_reactions(world);
    let result = step(
        world,
        grid_entity,
        &mut grid,
        frame_time,
        gas_data,
        &reactions,
        config,
        start,
    );
    if let Some(mut slot) = world.get_mut::<GridAtmosphere>(grid_entity) {
        *slot = grid;
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn step(
    world: &mut World,
    grid_entity: Entity,
    grid: &mut GridAtmosphere,
    frame_time: f32,
    gas_data: &GasData,
    reactions: &fire::GasReactions,
    config: &AtmosConfig,
    start: Instant,
) -> Completion {
    let atmos_time = 1.0 / config.tick_rate;
    grid.timer += frame_time;
    if grid.timer < atmos_time {
        return Completion::Continue;
    }
    grid.timer -= atmos_time;
    let cycle = grid.update_counter;
    let (done, next) = match grid.state {
        ProcessingState::Revalidate => (
            process_revalidate(world, grid_entity, grid, gas_data, config, start),
            if config.monstermos_equalization {
                ProcessingState::TileEqualize
            } else {
                ProcessingState::ActiveTiles
            },
        ),
        ProcessingState::TileEqualize => {
            if !grid.processing_paused {
                let set = grid.active_tiles.clone();
                queue_run_tiles(grid, &set);
            }
            (
                run_tiles(grid, config, start, |grid, tile| {
                    equalize_pressure_in_zone(
                        grid,
                        gas_data,
                        config,
                        tile,
                        cycle,
                        &mut WorldHooks { reactions },
                    )
                }),
                ProcessingState::ActiveTiles,
            )
        }
        ProcessingState::ActiveTiles => {
            if !grid.processing_paused {
                let set = grid.active_tiles.clone();
                queue_run_tiles(grid, &set);
            }
            (
                run_tiles(grid, config, start, |grid, tile| {
                    process_cell(
                        grid,
                        gas_data,
                        config,
                        tile,
                        cycle,
                        &mut WorldHooks { reactions },
                    )
                }),
                if config.excited_groups {
                    ProcessingState::ExcitedGroups
                } else {
                    ProcessingState::HighPressureDelta
                },
            )
        }
        ProcessingState::ExcitedGroups => (
            process_excited_groups(grid, gas_data, reactions, config, start),
            ProcessingState::HighPressureDelta,
        ),
        ProcessingState::HighPressureDelta => (
            process_high_pressure_delta(world, grid_entity, grid, config, start),
            if config.delta_pressure_damage {
                ProcessingState::DeltaPressure
            } else {
                ProcessingState::Hotspots
            },
        ),
        // Давление на конструкции, огонь, теплопроводность — 5.5б; трубы и устройства — 5.6.
        ProcessingState::DeltaPressure => (true, ProcessingState::Hotspots),
        ProcessingState::Hotspots => {
            if !grid.processing_paused {
                let set = grid.hotspot_tiles.clone();
                queue_run_tiles(grid, &set);
            }
            let mut effects = HotspotEffects {
                sound_cooldown: world.resource::<AtmosRun>().hotspot_sound_cooldown,
                ..HotspotEffects::default()
            };
            let done = run_tiles(grid, config, start, |grid, tile| {
                process_hotspot(grid, gas_data, reactions, tile, &mut effects)
            });
            world.resource_mut::<AtmosRun>().hotspot_sound_cooldown = effects.sound_cooldown;
            apply_hotspot_effects(world, grid_entity, effects);
            (
                done,
                if config.superconduction {
                    ProcessingState::Superconductivity
                } else {
                    ProcessingState::PipeNet
                },
            )
        }
        ProcessingState::Superconductivity => (true, ProcessingState::PipeNet),
        ProcessingState::PipeNet => {
            // `PipeNet.Update`: реакции в воздухе труб грида.
            for net in crate::pipes::nets_on(world, grid_entity) {
                let mut nets = world.resource_mut::<crate::pipes::PipeNets>();
                if let Some(pipe) = nets.nets.get_mut(&net) {
                    let mut exposures = Vec::new();
                    fire::react(gas_data, reactions, &mut pipe.air, &mut exposures);
                }
            }
            (true, ProcessingState::AtmosDevices)
        }
        ProcessingState::AtmosDevices => {
            // Устройства работают с воздухом клеток: атмосфера грида на время шага
            // возвращается в мир.
            let dt = real_atmos_time(config);
            if let Some(mut slot) = world.get_mut::<GridAtmosphere>(grid_entity) {
                std::mem::swap(&mut *slot, grid);
            }
            for device in crate::atmos_devices::devices_on(world, grid_entity) {
                crate::atmos_devices::update_device(world, device, dt);
            }
            if let Some(mut slot) = world.get_mut::<GridAtmosphere>(grid_entity) {
                std::mem::swap(&mut *slot, grid);
            }
            (true, ProcessingState::Revalidate)
        }
    };
    if !done {
        grid.processing_paused = true;
        return Completion::Return;
    }
    grid.processing_paused = false;
    let finished = grid.state == ProcessingState::AtmosDevices;
    grid.state = next;
    if finished {
        grid.update_counter += 1;
        Completion::Finished
    } else {
        Completion::Continue
    }
}

/// Время между шагами устройств (`RealAtmosTime`): число включённых шагов × шаг.
pub fn real_atmos_time(config: &AtmosConfig) -> f32 {
    let mut states = 10;
    for enabled in [
        config.monstermos_equalization,
        config.excited_groups,
        config.delta_pressure_damage,
        config.superconduction,
    ] {
        if !enabled {
            states -= 1;
        }
    }
    states as f32 / config.tick_rate
}

/// Такт атмосферы (`UpdateProcessing`).
pub fn update(world: &mut World) {
    let start = Instant::now();
    let frame_time = world.resource::<GameTime>().frame_time();
    let config = *world.get_resource_or_insert_with(AtmosConfig::default);
    let gas_data = gas_data(world);
    world.init_resource::<AtmosRun>();
    if !world.resource::<AtmosRun>().paused {
        let mut grids: Vec<Entity> = world
            .query::<(Entity, &GridAtmosphere)>()
            .iter(world)
            .filter(|(_, g)| g.simulated)
            .map(|(e, _)| e)
            .collect();
        grids.sort();
        let mut run = world.resource_mut::<AtmosRun>();
        run.grids = grids;
        run.index = 0;
    }
    world.resource_mut::<AtmosRun>().paused = true;
    loop {
        let (index, grids_len) = {
            let run = world.resource::<AtmosRun>();
            (run.index, run.grids.len())
        };
        if index >= grids_len {
            break;
        }
        let grid = world.resource::<AtmosRun>().grids[index];
        let valid = world.get_entity(grid).is_ok()
            && world.get::<Paused>(grid).is_none()
            && world.get::<Transform>(grid).and_then(|t| t.map()).is_some();
        if valid {
            match process_atmosphere(world, grid, frame_time, &gas_data, &config, start) {
                Completion::Return => return,
                Completion::Continue | Completion::Finished => {}
            }
        }
        world.resource_mut::<AtmosRun>().index += 1;
    }
    world.resource_mut::<AtmosRun>().paused = false;
}

// Доступ для других систем.

/// Пометить клетку для пересчёта (`InvalidateTile`).
pub fn invalidate_tile(world: &mut World, grid: Entity, coord: TileCoord) {
    if let Some(mut atmos) = world.get_mut::<GridAtmosphere>(grid) {
        atmos.invalidate(coord);
    }
}

/// Пересчитать всё (`InvalidateAllTiles`).
pub fn invalidate_all_tiles(world: &mut World, grid: Entity) {
    let mut coords: Vec<TileCoord> = world
        .get::<GridAtmosphere>(grid)
        .map(|a| a.index.keys().copied().collect())
        .unwrap_or_default();
    if let Some(map_grid) = world.get::<MapGrid>(grid) {
        let levels: Vec<i16> = map_grid.levels().collect();
        for z in levels {
            coords.extend(map_grid.tiles_on_level(z).map(|(coord, _)| coord));
        }
    }
    coords.sort_by_key(|c| (c.z, c.y, c.x));
    if let Some(mut atmos) = world.get_mut::<GridAtmosphere>(grid) {
        for coord in coords {
            atmos.invalidate(coord);
        }
    }
}

/// Дать гриду атмосферу (без воздуха: клетки получат пустой воздух при пересчёте).
pub fn ensure_grid_atmosphere(world: &mut World, grid: Entity) {
    if world.get::<GridAtmosphere>(grid).is_none() {
        world.entity_mut(grid).insert(GridAtmosphere::default());
        invalidate_all_tiles(world, grid);
    }
}

/// Воздух клетки (`GetTileMixture`).
pub fn tile_mixture(world: &World, grid: Entity, coord: TileCoord) -> Option<&GasMixture> {
    world
        .get::<GridAtmosphere>(grid)?
        .tile_at(coord)?
        .air
        .as_ref()
}

/// Изменить воздух клетки и разбудить её (`GetTileMixture(excite: true)`).
pub fn with_tile_mixture<R>(
    world: &mut World,
    grid: Entity,
    coord: TileCoord,
    f: impl FnOnce(&mut GasMixture) -> R,
) -> Option<R> {
    let mut atmos = world.get_mut::<GridAtmosphere>(grid)?;
    let index = atmos.tile_index(coord)?;
    let result = f(atmos.tile_mut(index).air.as_mut()?);
    add_active_tile(&mut atmos, index);
    Some(result)
}

/// Клетка — космос (`IsTileSpace`).
pub fn is_tile_space(world: &World, grid: Entity, coord: TileCoord) -> bool {
    world
        .get::<GridAtmosphere>(grid)
        .and_then(|a| a.tile_at(coord))
        .is_none_or(|t| t.space)
}

// Файл карты.

/// Воздух из записи `GridAtmosphere` (`TileAtmosCollectionSerializer.Read`).
pub fn read_tiles(node: &DataNode) -> Vec<(TileCoord, GasMixture)> {
    let number = |node: Option<&DataNode>| -> Option<i64> { node?.as_str()?.parse().ok() };
    let version = number(node.get("version")).unwrap_or(1);
    let mut tiles = Vec::new();
    let parse_coord = |text: &str| -> Option<(i32, i32)> {
        let (x, y) = text.split_once(',')?;
        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
    };
    if version == 1 {
        let unique: Vec<GasMixture> = node
            .get("uniqueMixes")
            .and_then(DataNode::as_seq)
            .map(|items| items.iter().filter_map(GasMixture::from_node).collect())
            .unwrap_or_default();
        if let Some(map) = node.get("tiles").and_then(DataNode::as_map) {
            for (key, value) in map.iter() {
                let (Some((x, y)), Some(mix)) = (
                    parse_coord(key),
                    value.as_str().and_then(|v| v.parse::<usize>().ok()),
                ) else {
                    continue;
                };
                if let Some(air) = unique.get(mix) {
                    tiles.push((TileCoord::new(x, y, 0), air.clone()));
                }
            }
        }
        return tiles;
    }
    let Some(data) = node.get("data") else {
        return tiles;
    };
    let chunk_size = number(data.get("chunkSize")).unwrap_or(4) as i32;
    let unique: Vec<GasMixture> = data
        .get("uniqueMixes")
        .and_then(DataNode::as_seq)
        .map(|items| items.iter().filter_map(GasMixture::from_node).collect())
        .unwrap_or_default();
    let Some(chunks) = data.get("tiles").and_then(DataNode::as_map) else {
        return tiles;
    };
    for (key, value) in chunks.iter() {
        let Some((cx, cy)) = parse_coord(key) else {
            continue;
        };
        let Some(masks) = value.as_map() else {
            continue;
        };
        for (mix, mask) in masks.iter() {
            let (Ok(mix), Some(mask)) = (
                mix.parse::<usize>(),
                mask.as_str().and_then(|v| v.parse::<u32>().ok()),
            ) else {
                continue;
            };
            let Some(air) = unique.get(mix) else {
                continue;
            };
            for y in 0..chunk_size {
                for x in 0..chunk_size {
                    if mask & (1 << (x + y * chunk_size)) == 0 {
                        continue;
                    }
                    tiles.push((
                        TileCoord::new(x + cx * chunk_size, y + cy * chunk_size, 0),
                        air.clone(),
                    ));
                }
            }
        }
    }
    tiles
}

/// Запись `GridAtmosphere` версии 2 (`TileAtmosCollectionSerializer.Write`).
pub fn write_tiles(atmos: &GridAtmosphere) -> DataNode {
    const CHUNK: i32 = 4;
    let mut unique: Vec<GasMixture> = Vec::new();
    let mut chunks: std::collections::BTreeMap<(i32, i32), std::collections::BTreeMap<usize, u32>> =
        Default::default();
    let mut tiles: Vec<(TileCoord, &GasMixture)> = atmos
        .live_tiles()
        .filter(|(_, t)| t.coord.z == 0)
        .filter_map(|(_, t)| t.air.as_ref().map(|air| (t.coord, air)))
        .collect();
    tiles.sort_by_key(|(c, _)| (c.y, c.x));
    for (coord, air) in tiles {
        let index = match unique.iter().position(|u| u == air) {
            Some(index) => index,
            None => {
                unique.push(air.clone());
                unique.len() - 1
            }
        };
        let chunk = (coord.x.div_euclid(CHUNK), coord.y.div_euclid(CHUNK));
        let (lx, ly) = (coord.x.rem_euclid(CHUNK), coord.y.rem_euclid(CHUNK));
        *chunks.entry(chunk).or_default().entry(index).or_default() |= 1 << (lx + ly * CHUNK);
    }
    let mut data = DataMap::new();
    data.insert("chunkSize", DataNode::plain(CHUNK.to_string()));
    if !unique.is_empty() {
        data.insert(
            "uniqueMixes",
            DataNode::seq(unique.iter().map(GasMixture::to_node).collect()),
        );
    }
    if !chunks.is_empty() {
        let mut map = DataMap::new();
        for ((x, y), masks) in chunks {
            let mut chunk = DataMap::new();
            for (mix, mask) in masks {
                chunk.insert(mix.to_string(), DataNode::plain(mask.to_string()));
            }
            map.insert(format!("{x},{y}"), DataNode::map(chunk));
        }
        data.insert("tiles", DataNode::map(map));
    }
    let mut node = DataMap::new();
    node.insert("type", DataNode::plain("GridAtmosphere"));
    node.insert("version", DataNode::plain("2"));
    node.insert("data", DataNode::map(data));
    DataNode::map(node)
}

fn preserved_atmosphere(world: &World, grid: Entity) -> Option<DataNode> {
    world
        .get::<MapPreserved>(grid)?
        .components
        .iter()
        .find(|node| node.type_name() == Some("GridAtmosphere"))
        .cloned()
}

/// Файл загружен: гриды с записью `GridAtmosphere` получают атмосферу с воздухом из неё.
fn adopt(world: &mut World, loaded: &MapLoaded) {
    for &grid in &loaded.grids {
        let Some(node) = preserved_atmosphere(world, grid) else {
            continue;
        };
        let mut atmos = GridAtmosphere::default();
        for (coord, air) in read_tiles(&node) {
            atmos.insert_tile(coord, air);
        }
        world.entity_mut(grid).insert(atmos);
        invalidate_all_tiles(world, grid);
    }
}

/// Перед сохранением: запись `GridAtmosphere` гридов карты — по текущему воздуху.
pub fn write_for_save(world: &mut World, map: Entity) {
    let Some(grids) = world.get::<Map>(map).map(|m| m.grids().to_vec()) else {
        return;
    };
    for grid in grids {
        let Some(node) = world.get::<GridAtmosphere>(grid).map(write_tiles) else {
            continue;
        };
        let mut preserved = world.get::<MapPreserved>(grid).cloned().unwrap_or_default();
        match preserved
            .components
            .iter_mut()
            .find(|n| n.type_name() == Some("GridAtmosphere"))
        {
            Some(existing) => *existing = node,
            None => preserved.components.push(node),
        }
        world.entity_mut(grid).insert(preserved);
    }
}

// События мира.

fn airtight_position(world: &World, entity: Entity) -> Option<(Entity, TileCoord)> {
    let transform = world.get::<Transform>(entity)?;
    let grid = transform.grid()?;
    Some((
        grid,
        TileCoord::containing(transform.local_position(), transform.level()),
    ))
}

fn invalidate_airtight(world: &mut World, entity: Entity) {
    if let Some((grid, coord)) = airtight_position(world, entity) {
        invalidate_tile(world, grid, coord);
    }
}

/// Поджечь клетку (`HotspotExpose`): искра, сварка, горящий предмет.
pub fn hotspot_expose_at(
    world: &mut World,
    grid: Entity,
    coord: TileCoord,
    temperature: f32,
    volume: f32,
    soh: bool,
) {
    let gas_data = gas_data(world);
    if let Some(mut atmos) = world.get_mut::<GridAtmosphere>(grid)
        && let Some(index) = atmos.tile_index(coord)
    {
        fire::hotspot_expose(&mut atmos, &gas_data, index, temperature, volume, soh);
    }
}

/// Горит ли клетка (`IsHotspotActive`).
pub fn is_hotspot_active(world: &World, grid: Entity, coord: TileCoord) -> bool {
    world
        .get::<GridAtmosphere>(grid)
        .and_then(|a| a.tile_at(coord))
        .is_some_and(|t| t.hotspot.valid)
}

/// Потушить клетку (`HotspotExtinguish`).
pub fn hotspot_extinguish(world: &mut World, grid: Entity, coord: TileCoord) {
    if let Some(mut atmos) = world.get_mut::<GridAtmosphere>(grid)
        && let Some(index) = atmos.tile_index(coord)
    {
        atmos.tile_mut(index).hotspot = Hotspot::default();
        add_active_tile(&mut atmos, index);
    }
}

pub fn install(world: &mut World, side: Side) {
    overlay::install(world, side.is_server());
    if !side.is_server() {
        return;
    }
    world.init_resource::<AtmosRun>();
    world.subscribe_broadcast::<MapLoaded>(|world, event| adopt(world, event));
    // `AirtightSystem`: изменения непроницаемости пересчитывают клетку.
    world.subscribe::<Airtight, AirtightChanged>(|world, entity, _| {
        invalidate_airtight(world, entity)
    });
    world.subscribe::<Airtight, ComponentInit>(|world, entity, _| {
        invalidate_airtight(world, entity)
    });
    world.subscribe::<Airtight, AnchorStateChanged>(|world, entity, _| {
        invalidate_airtight(world, entity)
    });
    world.subscribe::<Airtight, EntityTerminating>(|world, entity, _| {
        invalidate_airtight(world, entity)
    });
    world.subscribe::<MapGrid, TileChanged>(|world, grid, event| {
        let coords: Vec<TileCoord> = event.changes.iter().map(|c| c.coord).collect();
        for coord in coords {
            invalidate_tile(world, grid, coord);
        }
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
