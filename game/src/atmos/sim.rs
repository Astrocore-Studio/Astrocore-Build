//! Обмен газом между клетками: LINDA (`ProcessCell`, `Share`, `TemperatureShare`), группы
//! возбуждённых клеток и Monstermos (`EqualizePressureInZone`, `ExplosivelyDepressurize`,
//! `FinalizeEq`). Образец — `AtmosphereSystem.LINDA`, `.ExcitedGroup`, `.Monstermos` и
//! `.HighPressureDelta.ConsiderPressureDifference` SS14 @ 2f4a2aad.
//!
//! Функции работают с атмосферой грида, вынутой из мира на время шага (ADR-0007); то, что
//! требует мира (пожарные шлюзы при разгерметизации), — через [`Hooks`].

use super::gas::*;
use super::grid::*;

/// Настройки атмосферы (`CCVars.Atmos*`): значения SS14 по умолчанию.
#[derive(Debug, Clone, Copy, PartialEq, bevy_ecs::resource::Resource)]
pub struct AtmosConfig {
    pub monstermos_equalization: bool,
    pub monstermos_depressurization: bool,
    pub monstermos_rip_tiles: bool,
    pub excited_groups: bool,
    pub excited_groups_space_is_all_consuming: bool,
    pub superconduction: bool,
    pub space_wind: bool,
    pub delta_pressure_damage: bool,
    pub spacing_escape_ratio: f32,
    pub spacing_min_gas: f32,
    pub spacing_max_wind: f32,
    /// Бюджет времени на такт сервера, мс (`atmos.max_process_time`).
    pub max_process_time: f32,
    /// Частота шагов, Гц (`atmos.tickrate`).
    pub tick_rate: f32,
}

impl Default for AtmosConfig {
    fn default() -> Self {
        Self {
            monstermos_equalization: true,
            monstermos_depressurization: true,
            monstermos_rip_tiles: false,
            excited_groups: true,
            excited_groups_space_is_all_consuming: false,
            superconduction: false,
            space_wind: false,
            delta_pressure_damage: true,
            spacing_escape_ratio: 0.15,
            spacing_min_gas: 2.0,
            spacing_max_wind: 500.0,
            max_process_time: 3.0,
            tick_rate: 15.0,
        }
    }
}

/// То, что алгоритмам нужно от мира.
pub trait Hooks {
    /// Пожарные шлюзы на клетках закрываются при разгерметизации (`ConsiderFirelocks`):
    /// `true`, если непроницаемость клеток изменилась.
    fn consider_firelocks(&mut self, _grid: &mut GridAtmosphere, _a: u32, _b: u32) -> bool {
        false
    }
    /// Вид газа клетки устарел (`InvalidateVisuals`).
    fn invalidate_visuals(&mut self, _grid: &GridAtmosphere, _tile: u32) {}
    /// Реакции газа в клетке (`React`).
    fn react(&mut self, _grid: &mut GridAtmosphere, _gas: &GasData, _tile: u32) {}
    /// Сорвать плитку пола при сильном ветре (`HandleDecompressionFloorRip`).
    fn rip_tile(&mut self, _grid: &GridAtmosphere, _tile: u32, _sum: f32) {}
}

/// Без мира: для тестов.
pub struct NoHooks;
impl Hooks for NoHooks {}

/// Две клетки сразу.
pub fn pair_mut(
    tiles: &mut [TileAtmosphere],
    a: u32,
    b: u32,
) -> (&mut TileAtmosphere, &mut TileAtmosphere) {
    let (a, b) = (a as usize, b as usize);
    assert_ne!(a, b);
    if a < b {
        let (left, right) = tiles.split_at_mut(b);
        (&mut left[a], &mut right[0])
    } else {
        let (left, right) = tiles.split_at_mut(a);
        (&mut right[0], &mut left[b])
    }
}

fn has(bits: u8, direction: u8) -> bool {
    direction != 0 && bits & direction == direction
}

// Активные клетки.

/// `AddActiveTile`.
pub fn add_active_tile(grid: &mut GridAtmosphere, tile: u32) {
    let t = grid.tile_mut(tile);
    if t.air.is_none() || t.excited {
        return;
    }
    t.excited = true;
    grid.active_tiles.insert(tile);
}

/// `RemoveActiveTile`.
pub fn remove_active_tile(grid: &mut GridAtmosphere, tile: u32, dispose_group: bool) {
    if !grid.tile(tile).excited {
        return;
    }
    grid.tile_mut(tile).excited = false;
    grid.active_tiles.remove(tile);
    let Some(group) = grid.tile(tile).excited_group else {
        return;
    };
    if dispose_group {
        excited_group_dispose(grid, group);
    } else {
        excited_group_remove_tile(grid, group, tile);
    }
}

// Группы.

/// `ExcitedGroupAddTile`.
pub fn excited_group_add_tile(grid: &mut GridAtmosphere, group: u32, tile: u32) {
    grid.group_mut(group).tiles.push(tile);
    grid.tile_mut(tile).excited_group = Some(group);
    excited_group_reset_cooldowns(grid, group);
}

fn excited_group_remove_tile(grid: &mut GridAtmosphere, group: u32, tile: u32) {
    grid.tile_mut(tile).excited_group = None;
    grid.group_mut(group).tiles.retain(|t| *t != tile);
}

/// `ExcitedGroupMerge`.
pub fn excited_group_merge(grid: &mut GridAtmosphere, ours: u32, other: u32) {
    let (winner, loser) = if grid.group(ours).tiles.len() > grid.group(other).tiles.len() {
        (ours, other)
    } else {
        (other, ours)
    };
    let moved = std::mem::take(&mut grid.group_mut(loser).tiles);
    for tile in &moved {
        grid.tile_mut(*tile).excited_group = Some(winner);
    }
    grid.group_mut(winner).tiles.extend(moved);
    excited_group_dispose(grid, loser);
    excited_group_reset_cooldowns(grid, winner);
}

fn excited_group_reset_cooldowns(grid: &mut GridAtmosphere, group: u32) {
    let group = grid.group_mut(group);
    group.breakdown_cooldown = 0;
    group.dismantle_cooldown = 0;
}

/// Выровнять группу целиком (`ExcitedGroupSelfBreakdown`).
pub fn excited_group_self_breakdown(
    grid: &mut GridAtmosphere,
    gas: &GasData,
    config: &AtmosConfig,
    group: u32,
    hooks: &mut dyn Hooks,
) {
    if grid.group(group).disposed {
        return;
    }
    let tiles = grid.group(group).tiles.clone();
    if tiles.is_empty() {
        excited_group_dispose(grid, group);
        return;
    }
    let mut combined = GasMixture::new(CELL_VOLUME);
    for tile in &tiles {
        let t = grid.tile(*tile);
        let Some(air) = &t.air else {
            continue;
        };
        gas.merge(&mut combined, air);
        if config.excited_groups_space_is_all_consuming && t.space {
            combined.clear();
            break;
        }
    }
    combined.multiply(1.0 / tiles.len() as f32);
    for tile in &tiles {
        if let Some(air) = &mut grid.tile_mut(*tile).air {
            air.copy_from(&combined);
            hooks.invalidate_visuals(grid, *tile);
        }
    }
    grid.group_mut(group).breakdown_cooldown = 0;
}

/// `DeactivateGroupTiles`.
pub fn deactivate_group_tiles(grid: &mut GridAtmosphere, group: u32) {
    let tiles = std::mem::take(&mut grid.group_mut(group).tiles);
    for tile in tiles {
        grid.tile_mut(tile).excited_group = None;
        remove_active_tile(grid, tile, true);
    }
}

/// `ExcitedGroupDispose`.
pub fn excited_group_dispose(grid: &mut GridAtmosphere, group: u32) {
    if grid.group(group).disposed {
        return;
    }
    grid.group_mut(group).disposed = true;
    let tiles = std::mem::take(&mut grid.group_mut(group).tiles);
    for tile in tiles {
        grid.tile_mut(tile).excited_group = None;
    }
    grid.release_group(group);
}

// LINDA.

fn archive(grid: &mut GridAtmosphere, tile: u32, fire_count: i32) {
    let t = grid.tile_mut(tile);
    if let Some(air) = &t.air {
        t.air_archived = Some(air.clone());
    }
    t.archived_cycle = fire_count;
}

fn last_share_check(grid: &mut GridAtmosphere, tile: u32) {
    let t = grid.tile(tile);
    let (Some(_), Some(group)) = (&t.air, t.excited_group) else {
        return;
    };
    let last = t.last_share;
    if last > MINIMUM_AIR_TO_SUSPEND {
        excited_group_reset_cooldowns(grid, group);
    } else if last > MINIMUM_MOLES_DELTA_TO_MOVE {
        grid.group_mut(group).dismantle_cooldown = 0;
    }
}

/// Теплоёмкость архивной смеси (`GetHeatCapacityArchived`).
fn heat_capacity_archived(gas: &GasData, tile: &TileAtmosphere) -> f32 {
    match &tile.air_archived {
        Some(air) => gas.heat_capacity(air),
        None => tile.heat_capacity,
    }
}

/// Обмен теплом (`TemperatureShare`).
pub fn temperature_share(
    gas: &GasData,
    receiver: &mut TileAtmosphere,
    sharer: &mut TileAtmosphere,
    coefficient: f32,
) {
    let (Some(_), Some(_), Some(receiver_archived), Some(sharer_archived)) = (
        &receiver.air,
        &sharer.air,
        &receiver.air_archived,
        &sharer.air_archived,
    ) else {
        return;
    };
    let delta = receiver_archived.temperature() - sharer_archived.temperature();
    if delta.abs() <= MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
        return;
    }
    let heat_capacity = heat_capacity_archived(gas, receiver);
    let sharer_heat_capacity = heat_capacity_archived(gas, sharer);
    if sharer_heat_capacity <= MINIMUM_HEAT_CAPACITY || heat_capacity <= MINIMUM_HEAT_CAPACITY {
        return;
    }
    let heat = coefficient
        * delta
        * (heat_capacity * sharer_heat_capacity / (heat_capacity + sharer_heat_capacity));
    let receiver_air = receiver.air.as_mut().unwrap();
    if !receiver_air.immutable {
        let t = (receiver_air.temperature() - heat / heat_capacity)
            .max(TCMB)
            .abs();
        receiver_air.set_temperature(t);
    }
    let sharer_air = sharer.air.as_mut().unwrap();
    if !sharer_air.immutable {
        let t = (sharer_air.temperature() + heat / sharer_heat_capacity)
            .max(TCMB)
            .abs();
        sharer_air.set_temperature(t);
    }
}

/// Обмен газом двух соседей (`Share`): разница давлений после обмена.
pub fn share(
    gas: &GasData,
    receiver_tile: &mut TileAtmosphere,
    sharer_tile: &mut TileAtmosphere,
    adjacent: usize,
) -> f32 {
    let (Some(receiver_archived), Some(sharer_archived)) = (
        receiver_tile.air_archived.clone(),
        sharer_tile.air_archived.clone(),
    ) else {
        return 0.0;
    };
    let (Some(receiver), Some(sharer)) = (receiver_tile.air.as_mut(), sharer_tile.air.as_mut())
    else {
        return 0.0;
    };
    let temperature_delta = receiver_archived.temperature() - sharer_archived.temperature();
    let abs_temperature_delta = temperature_delta.abs();
    let mut old_heat_capacity = 0.0;
    let mut old_sharer_heat_capacity = 0.0;
    if abs_temperature_delta > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
        old_heat_capacity = gas.heat_capacity(receiver);
        old_sharer_heat_capacity = gas.heat_capacity(sharer);
    }
    let mut heat_capacity_to_sharer = 0.0;
    let mut heat_capacity_sharer_to_this = 0.0;
    let mut moved_moles = 0.0;
    let mut abs_moved_moles = 0.0;
    for i in 0..GAS_COUNT {
        let delta = (receiver.moles[i] - sharer.moles[i]) / (adjacent as f32 + 1.0);
        if delta.abs() < GAS_MIN_MOLES {
            continue;
        }
        if abs_temperature_delta > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
            let gas_heat_capacity = delta * gas.heat_capacities[i];
            if delta > 0.0 {
                heat_capacity_to_sharer += gas_heat_capacity;
            } else {
                heat_capacity_sharer_to_this -= gas_heat_capacity;
            }
        }
        if !receiver.immutable {
            receiver.moles[i] -= delta;
        }
        if !sharer.immutable {
            sharer.moles[i] += delta;
        }
        moved_moles += delta;
        abs_moved_moles += delta.abs();
    }
    receiver_tile.last_share = abs_moved_moles;
    let receiver = receiver_tile.air.as_mut().unwrap();
    let sharer = sharer_tile.air.as_mut().unwrap();
    if abs_temperature_delta > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
        let new_heat_capacity =
            old_heat_capacity + heat_capacity_sharer_to_this - heat_capacity_to_sharer;
        let new_sharer_heat_capacity =
            old_sharer_heat_capacity + heat_capacity_to_sharer - heat_capacity_sharer_to_this;
        if !receiver.immutable && new_heat_capacity > MINIMUM_HEAT_CAPACITY {
            let t = (old_heat_capacity * receiver.temperature()
                - heat_capacity_to_sharer * receiver_archived.temperature()
                + heat_capacity_sharer_to_this * sharer_archived.temperature())
                / new_heat_capacity;
            receiver.set_temperature(t);
        }
        if !sharer.immutable && new_sharer_heat_capacity > MINIMUM_HEAT_CAPACITY {
            let t = (old_sharer_heat_capacity * sharer.temperature()
                - heat_capacity_sharer_to_this * sharer_archived.temperature()
                + heat_capacity_to_sharer * receiver_archived.temperature())
                / new_sharer_heat_capacity;
            sharer.set_temperature(t);
        }
        if old_sharer_heat_capacity.abs() > MINIMUM_HEAT_CAPACITY
            && (new_sharer_heat_capacity / old_sharer_heat_capacity - 1.0).abs() < 0.1
        {
            temperature_share(
                gas,
                receiver_tile,
                sharer_tile,
                OPEN_HEAT_TRANSFER_COEFFICIENT,
            );
        }
    }
    if abs_temperature_delta <= MINIMUM_TEMPERATURE_TO_MOVE
        && moved_moles.abs() <= MINIMUM_MOLES_DELTA_TO_MOVE
    {
        return 0.0;
    }
    let receiver = receiver_tile.air.as_ref().unwrap();
    let sharer = sharer_tile.air.as_ref().unwrap();
    let moles = receiver.total_moles();
    let their_moles = sharer.total_moles();
    (receiver_archived.temperature() * (moles + moved_moles)
        - sharer_archived.temperature() * (their_moles - moved_moles))
        * R
        / receiver.volume
}

/// `ConsiderPressureDifference`.
pub fn consider_pressure_difference(
    grid: &mut GridAtmosphere,
    tile: u32,
    direction: u8,
    difference: f32,
) {
    grid.high_pressure_delta.insert(tile);
    let t = grid.tile_mut(tile);
    t.in_high_pressure_delta = true;
    if difference <= t.pressure_difference {
        return;
    }
    t.pressure_difference = difference;
    t.pressure_direction = direction;
}

/// Обработка активной клетки (`ProcessCell`).
pub fn process_cell(
    grid: &mut GridAtmosphere,
    gas: &GasData,
    config: &AtmosConfig,
    tile: u32,
    fire_count: i32,
    hooks: &mut dyn Hooks,
) {
    if grid.tile(tile).air.is_none() {
        remove_active_tile(grid, tile, true);
        return;
    }
    if grid.tile(tile).archived_cycle < fire_count {
        archive(grid, tile, fire_count);
    }
    grid.tile_mut(tile).current_cycle = fire_count;
    let adjacent_count = grid.tile(tile).adjacent_bits.count_ones() as usize;
    for i in 0..4 {
        let dir = direction(i);
        if !has(grid.tile(tile).adjacent_bits, dir) {
            continue;
        }
        let Some(enemy) = grid.tile(tile).adjacent[i] else {
            continue;
        };
        if grid.tile(enemy).air.is_none() || fire_count <= grid.tile(enemy).current_cycle {
            continue;
        }
        archive(grid, enemy, fire_count);
        let mut should_share = false;
        let (ours, theirs) = (
            grid.tile(tile).excited_group,
            grid.tile(enemy).excited_group,
        );
        if let (true, Some(ours), Some(theirs)) = (config.excited_groups, ours, theirs) {
            if ours != theirs {
                excited_group_merge(grid, ours, theirs);
            }
            should_share = true;
        } else {
            let compare = match (
                &grid.tile(tile).air_archived,
                &grid.tile(enemy).air_archived,
            ) {
                (Some(a), Some(b)) => compare_exchange(a, b),
                _ => GasCompare::NoExchange,
            };
            if compare != GasCompare::NoExchange {
                add_active_tile(grid, enemy);
                if config.excited_groups {
                    let group = match grid
                        .tile(tile)
                        .excited_group
                        .or(grid.tile(enemy).excited_group)
                    {
                        Some(group) => group,
                        None => grid.new_group(),
                    };
                    if grid.tile(tile).excited_group.is_none() {
                        excited_group_add_tile(grid, group, tile);
                    }
                    if grid.tile(enemy).excited_group.is_none() {
                        excited_group_add_tile(grid, group, enemy);
                    }
                }
                should_share = true;
            }
        }
        if should_share {
            let (a, b) = pair_mut(&mut grid.tiles, tile, enemy);
            let difference = share(gas, a, b, adjacent_count);
            if !config.monstermos_equalization {
                if difference >= 0.0 {
                    consider_pressure_difference(grid, tile, dir, difference);
                } else {
                    consider_pressure_difference(grid, enemy, opposite_direction(i), -difference);
                }
            }
            last_share_check(grid, tile);
        }
    }
    if grid.tile(tile).air.is_some() {
        hooks.react(grid, gas, tile);
    }
    hooks.invalidate_visuals(grid, tile);
    // Теплопроводность (`ConsiderSuperconductivity`) — в 5.5б; пока клетка не задерживается.
    let remove = true;
    if config.excited_groups && grid.tile(tile).excited_group.is_none() && remove {
        remove_active_tile(grid, tile, true);
    }
}

// Monstermos.

fn adjust_eq_movement(grid: &mut GridAtmosphere, tile: u32, index: usize, amount: f32) {
    let Some(adj) = grid.tile(tile).adjacent[index] else {
        return;
    };
    grid.tile_mut(tile).monstermos.transfer[index] += amount;
    grid.tile_mut(adj).monstermos.transfer[opposite_index(index)] -= amount;
}

/// Выравнивание давления в зоне (`EqualizePressureInZone`).
pub fn equalize_pressure_in_zone(
    grid: &mut GridAtmosphere,
    gas: &GasData,
    config: &AtmosConfig,
    tile: u32,
    cycle: i32,
    hooks: &mut dyn Hooks,
) {
    if grid.tile(tile).air.is_none() || grid.tile(tile).monstermos.last_cycle >= cycle {
        return;
    }
    grid.tile_mut(tile).monstermos = MonstermosInfo::default();
    let starting_moles = grid.tile(tile).air.as_ref().unwrap().total_moles();
    let mut run_atmos = false;
    for i in 0..4 {
        if !has(grid.tile(tile).adjacent_bits, direction(i)) {
            continue;
        }
        let Some(other) = grid.tile(tile).adjacent[i] else {
            continue;
        };
        let Some(air) = &grid.tile(other).air else {
            continue;
        };
        if (air.total_moles() - starting_moles).abs() > MINIMUM_MOLES_DELTA_TO_MOVE {
            run_atmos = true;
            break;
        }
    }
    if !run_atmos {
        grid.tile_mut(tile).monstermos.last_cycle = cycle;
        return;
    }
    grid.equalization_queue_cycle_control += 1;
    let queue_cycle = grid.equalization_queue_cycle_control;
    let mut total_moles = 0.0;
    let mut tiles: Vec<u32> = vec![tile];
    grid.tile_mut(tile).monstermos.last_queue_cycle = queue_cycle;
    let mut i = 0;
    while i < tiles.len() {
        if i > MONSTERMOS_HARD_TILE_LIMIT {
            break;
        }
        let exploring = tiles[i];
        if i < MONSTERMOS_TILE_LIMIT {
            let moles = grid.tile(exploring).air.as_ref().unwrap().total_moles();
            grid.tile_mut(exploring).monstermos.mole_delta = moles;
            total_moles += moles;
        }
        for j in 0..4 {
            if !has(grid.tile(exploring).adjacent_bits, direction(j)) {
                continue;
            }
            let Some(adj) = grid.tile(exploring).adjacent[j] else {
                continue;
            };
            if grid.tile(adj).air.is_none()
                || grid.tile(adj).monstermos.last_queue_cycle == queue_cycle
            {
                continue;
            }
            grid.tile_mut(adj).monstermos = MonstermosInfo {
                last_queue_cycle: queue_cycle,
                ..MonstermosInfo::default()
            };
            if tiles.len() < MONSTERMOS_HARD_TILE_LIMIT {
                tiles.push(adj);
            }
            if grid.tile(adj).space && config.monstermos_depressurization {
                explosively_depressurize(grid, gas, config, tile, cycle, hooks);
                return;
            }
        }
        i += 1;
    }
    if tiles.len() > MONSTERMOS_TILE_LIMIT {
        for other in &tiles[MONSTERMOS_TILE_LIMIT..] {
            grid.tile_mut(*other).monstermos.last_queue_cycle = 0;
        }
        tiles.truncate(MONSTERMOS_TILE_LIMIT);
    }
    let count = tiles.len();
    let average = total_moles / count as f32;
    let mut givers: Vec<u32> = Vec::new();
    let mut takers: Vec<u32> = Vec::new();
    for other in &tiles {
        let t = grid.tile_mut(*other);
        t.monstermos.last_cycle = cycle;
        t.monstermos.mole_delta -= average;
        if t.monstermos.mole_delta > 0.0 {
            givers.push(*other);
        } else {
            takers.push(*other);
        }
    }
    let log_n = (count as f32).log2();
    if givers.len() as f32 > log_n && takers.len() as f32 > log_n {
        tiles.sort_by(|a, b| {
            grid.tile(*a)
                .monstermos
                .mole_delta
                .total_cmp(&grid.tile(*b).monstermos.mole_delta)
        });
        for other in &tiles {
            let other = *other;
            grid.tile_mut(other).monstermos.fast_done = true;
            if grid.tile(other).monstermos.mole_delta <= 0.0 {
                continue;
            }
            let mut eligible = 0u8;
            let mut eligible_count = 0;
            for j in 0..4 {
                if !has(grid.tile(other).adjacent_bits, direction(j)) {
                    continue;
                }
                let Some(tile2) = grid.tile(other).adjacent[j] else {
                    continue;
                };
                let info = grid.tile(tile2).monstermos;
                if info.fast_done || info.last_queue_cycle != queue_cycle {
                    continue;
                }
                eligible |= direction(j);
                eligible_count += 1;
            }
            if eligible_count == 0 {
                continue;
            }
            let moles_to_move = grid.tile(other).monstermos.mole_delta / eligible_count as f32;
            for j in 0..4 {
                if !has(eligible, direction(j)) {
                    continue;
                }
                adjust_eq_movement(grid, other, j, moles_to_move);
                grid.tile_mut(other).monstermos.mole_delta -= moles_to_move;
                let adj = grid.tile(other).adjacent[j].unwrap();
                grid.tile_mut(adj).monstermos.mole_delta += moles_to_move;
            }
        }
        givers.clear();
        takers.clear();
        for other in &tiles {
            if grid.tile(*other).monstermos.mole_delta > 0.0 {
                givers.push(*other);
            } else {
                takers.push(*other);
            }
        }
    }
    let giving = givers.len() < takers.len();
    let sources = if giving { givers } else { takers };
    for source in sources {
        {
            let info = &mut grid.tile_mut(source).monstermos;
            info.current_transfer_direction = 0;
            info.current_transfer_amount = 0.0;
        }
        grid.equalization_queue_cycle_control += 1;
        let queue_cycle_slow = grid.equalization_queue_cycle_control;
        let mut queue: Vec<u32> = vec![source];
        grid.tile_mut(source).monstermos.last_slow_queue_cycle = queue_cycle_slow;
        let mut qi = 0;
        let done = |grid: &GridAtmosphere| {
            let delta = grid.tile(source).monstermos.mole_delta;
            if giving { delta <= 0.0 } else { delta >= 0.0 }
        };
        while qi < queue.len() {
            if done(grid) {
                break;
            }
            let other = queue[qi];
            for k in 0..4 {
                if !has(grid.tile(other).adjacent_bits, direction(k)) {
                    continue;
                }
                if done(grid) {
                    break;
                }
                let Some(other2) = grid.tile(other).adjacent[k] else {
                    continue;
                };
                if !giving && grid.tile(other2).adjacent_bits == 0 {
                    continue;
                }
                if grid.tile(other2).monstermos.last_queue_cycle != queue_cycle
                    || grid.tile(other2).monstermos.last_slow_queue_cycle == queue_cycle_slow
                {
                    continue;
                }
                queue.push(other2);
                {
                    let info = &mut grid.tile_mut(other2).monstermos;
                    info.last_slow_queue_cycle = queue_cycle_slow;
                    info.current_transfer_direction = opposite_direction(k);
                    info.current_transfer_amount = 0.0;
                }
                let source_delta = grid.tile(source).monstermos.mole_delta;
                let other_delta = grid.tile(other2).monstermos.mole_delta;
                if giving && other_delta < 0.0 {
                    if -other_delta > source_delta {
                        let info = &mut grid.tile_mut(other2).monstermos;
                        info.current_transfer_amount -= source_delta;
                        info.mole_delta += source_delta;
                        grid.tile_mut(source).monstermos.mole_delta = 0.0;
                    } else {
                        let info = &mut grid.tile_mut(other2).monstermos;
                        info.current_transfer_amount += other_delta;
                        info.mole_delta = 0.0;
                        grid.tile_mut(source).monstermos.mole_delta += other_delta;
                    }
                } else if !giving && other_delta > 0.0 {
                    if other_delta > -source_delta {
                        let info = &mut grid.tile_mut(other2).monstermos;
                        info.current_transfer_amount -= source_delta;
                        info.mole_delta += source_delta;
                        grid.tile_mut(source).monstermos.mole_delta = 0.0;
                    } else {
                        let info = &mut grid.tile_mut(other2).monstermos;
                        info.current_transfer_amount += other_delta;
                        info.mole_delta = 0.0;
                        grid.tile_mut(source).monstermos.mole_delta += other_delta;
                    }
                }
            }
            qi += 1;
        }
        for other in queue.iter().rev() {
            let info = grid.tile(*other).monstermos;
            if info.current_transfer_amount == 0.0 || info.current_transfer_direction == 0 {
                continue;
            }
            let Some(index) = direction_index(info.current_transfer_direction) else {
                continue;
            };
            adjust_eq_movement(grid, *other, index, info.current_transfer_amount);
            if let Some(adj) = grid.tile(*other).adjacent[index] {
                grid.tile_mut(adj).monstermos.current_transfer_amount +=
                    info.current_transfer_amount;
            }
            grid.tile_mut(*other).monstermos.current_transfer_amount = 0.0;
        }
    }
    for other in &tiles {
        finalize_eq(grid, gas, *other, hooks);
    }
    for other in &tiles {
        for j in 0..4 {
            if !has(grid.tile(*other).adjacent_bits, direction(j)) {
                continue;
            }
            let Some(other2) = grid.tile(*other).adjacent[j] else {
                continue;
            };
            if grid.tile(other2).adjacent_bits == 0 {
                continue;
            }
            let no_exchange = match (
                &grid.tile(other2).air_archived,
                &grid.tile(tile).air_archived,
            ) {
                (Some(a), Some(b)) => compare_exchange(a, b) == GasCompare::NoExchange,
                _ => false,
            };
            if grid.tile(other2).air.is_some() && no_exchange {
                continue;
            }
            add_active_tile(grid, other2);
            break;
        }
    }
}

/// Перенести накопленные переносы (`FinalizeEq`).
fn finalize_eq(grid: &mut GridAtmosphere, gas: &GasData, tile: u32, hooks: &mut dyn Hooks) {
    let transfer = grid.tile(tile).monstermos.transfer;
    if transfer.iter().all(|amount| *amount == 0.0) {
        return;
    }
    grid.tile_mut(tile).monstermos.transfer = [0.0; 4];
    for i in 0..4 {
        if !has(grid.tile(tile).adjacent_bits, direction(i)) {
            continue;
        }
        let amount = transfer[i];
        let Some(other) = grid.tile(tile).adjacent[i] else {
            continue;
        };
        if grid.tile(other).air.is_none() || amount <= 0.0 {
            continue;
        }
        if grid.tile(tile).air.as_ref().unwrap().total_moles() < amount {
            finalize_eq_neighbors(grid, gas, tile, &transfer, hooks);
        }
        grid.tile_mut(other).monstermos.transfer[opposite_index(i)] = 0.0;
        let removed = grid.tile_mut(tile).air.as_mut().unwrap().remove(amount);
        gas.merge(grid.tile_mut(other).air.as_mut().unwrap(), &removed);
        hooks.invalidate_visuals(grid, tile);
        hooks.invalidate_visuals(grid, other);
        consider_pressure_difference(grid, tile, direction(i), amount);
    }
}

fn finalize_eq_neighbors(
    grid: &mut GridAtmosphere,
    gas: &GasData,
    tile: u32,
    transfer: &[f32; 4],
    hooks: &mut dyn Hooks,
) {
    for (i, amount) in transfer.iter().enumerate() {
        if *amount < 0.0
            && has(grid.tile(tile).adjacent_bits, direction(i))
            && let Some(adj) = grid.tile(tile).adjacent[i]
        {
            finalize_eq(grid, gas, adj, hooks);
        }
    }
}

/// Взрывная разгерметизация (`ExplosivelyDepressurize`).
pub fn explosively_depressurize(
    grid: &mut GridAtmosphere,
    gas: &GasData,
    config: &AtmosConfig,
    tile: u32,
    cycle: i32,
    hooks: &mut dyn Hooks,
) {
    if !config.monstermos_depressurization || grid.tile(tile).air.is_none() {
        return;
    }
    let limit = MONSTERMOS_HARD_TILE_LIMIT;
    grid.equalization_queue_cycle_control += 1;
    let queue_cycle = grid.equalization_queue_cycle_control;
    let mut tiles: Vec<u32> = vec![tile];
    let mut space_tiles: Vec<u32> = Vec::new();
    grid.tile_mut(tile).monstermos = MonstermosInfo {
        last_queue_cycle: queue_cycle,
        ..MonstermosInfo::default()
    };
    let mut i = 0;
    while i < tiles.len() {
        let other = tiles[i];
        grid.tile_mut(other).monstermos.last_cycle = cycle;
        grid.tile_mut(other).monstermos.current_transfer_direction = 0;
        if !grid.tile(other).space {
            for j in 0..4 {
                let Some(other2) = grid.tile(other).adjacent[j] else {
                    continue;
                };
                if grid.tile(other2).air.is_none()
                    || grid.tile(other2).monstermos.last_queue_cycle == queue_cycle
                {
                    continue;
                }
                hooks.consider_firelocks(grid, other, other2);
                if !has(grid.tile(other).adjacent_bits, direction(j)) {
                    continue;
                }
                grid.tile_mut(other2).monstermos = MonstermosInfo {
                    last_queue_cycle: queue_cycle,
                    ..MonstermosInfo::default()
                };
                tiles.push(other2);
                if tiles.len() >= limit {
                    break;
                }
            }
        } else {
            space_tiles.push(other);
            grid.tile_mut(other).pressure_specific_target = Some(other);
        }
        if tiles.len() >= limit || space_tiles.len() >= limit {
            break;
        }
        i += 1;
    }
    grid.equalization_queue_cycle_control += 1;
    let queue_cycle_slow = grid.equalization_queue_cycle_control;
    let mut progression: Vec<u32> = Vec::new();
    for other in &space_tiles {
        progression.push(*other);
        let info = &mut grid.tile_mut(*other).monstermos;
        info.last_slow_queue_cycle = queue_cycle_slow;
        info.current_transfer_direction = 0;
    }
    let mut p = 0;
    while p < progression.len() {
        let other = progression[p];
        for j in 0..4 {
            if !has(grid.tile(other).adjacent_bits, direction(j)) && !grid.tile(other).space {
                continue;
            }
            let Some(tile2) = grid.tile(other).adjacent[j] else {
                continue;
            };
            if grid.tile(tile2).monstermos.last_queue_cycle != queue_cycle
                || grid.tile(tile2).monstermos.last_slow_queue_cycle == queue_cycle_slow
                || grid.tile(tile2).space
            {
                continue;
            }
            let target = grid.tile(other).pressure_specific_target;
            let t2 = grid.tile_mut(tile2);
            t2.monstermos.current_transfer_direction = opposite_direction(j);
            t2.monstermos.current_transfer_amount = 0.0;
            t2.pressure_specific_target = target;
            t2.monstermos.last_slow_queue_cycle = queue_cycle_slow;
            progression.push(tile2);
        }
        p += 1;
    }
    for other in progression.iter().rev().copied() {
        if grid.tile(other).air.is_none() {
            continue;
        }
        let dir = grid.tile(other).monstermos.current_transfer_direction;
        let Some(index) = direction_index(dir) else {
            continue;
        };
        grid.high_pressure_delta.insert(other);
        grid.tile_mut(other).in_high_pressure_delta = true;
        add_active_tile(grid, other);
        let other2 = grid.tile(other).adjacent[index];
        let Some(other2) = other2.filter(|o| grid.tile(*o).air.is_some()) else {
            let air = grid.tile_mut(other).air.as_mut().unwrap();
            air.clear();
            air.set_temperature(TCMB);
            continue;
        };
        let mut sum = grid.tile(other).air.as_ref().unwrap().total_moles();
        if config.spacing_escape_ratio < 1.0 {
            sum *= config.spacing_escape_ratio;
            if sum < config.spacing_min_gas {
                sum = config
                    .spacing_min_gas
                    .min(grid.tile(other).air.as_ref().unwrap().total_moles());
            }
            let current = grid.tile(other).monstermos.current_transfer_amount;
            if sum + current > config.spacing_max_wind {
                sum = config
                    .spacing_min_gas
                    .max(config.spacing_max_wind - current);
            }
        }
        grid.tile_mut(other).monstermos.current_transfer_amount += sum;
        let amount = grid.tile(other).monstermos.current_transfer_amount;
        grid.tile_mut(other2).monstermos.current_transfer_amount += amount;
        grid.tile_mut(other).pressure_difference = amount;
        grid.tile_mut(other).pressure_direction = dir;
        if grid.tile(other2).monstermos.current_transfer_direction == 0 {
            let amount2 = grid.tile(other2).monstermos.current_transfer_amount;
            grid.tile_mut(other2).pressure_difference = amount2;
            grid.tile_mut(other2).pressure_direction = dir;
        }
        let pressure = grid.tile(other).air.as_ref().unwrap().pressure();
        if pressure - sum > config.spacing_min_gas * 0.1 {
            let (a, b) = pair_mut(&mut grid.tiles, other, other2);
            let (air, air2) = (a.air.as_mut().unwrap(), b.air.as_mut().unwrap());
            gas.release_gas_to(air, Some(air2), sum);
            gas.release_gas_to(air2, None, sum * 0.3);
            if air.temperature() > 280.0 {
                let total = air.total_moles();
                let real_temp_loss = (total - sum) / total;
                let t = air.temperature() * (0.9 + 0.1 * real_temp_loss);
                air.set_temperature(t);
            }
        } else {
            let air = grid.tile_mut(other).air.as_mut().unwrap();
            air.clear();
            air.set_temperature(TCMB);
        }
        hooks.invalidate_visuals(grid, other);
        if config.monstermos_rip_tiles {
            hooks.rip_tile(grid, other, amount);
        }
    }
}
