//! Сетка атмосферы: обмен газом между соседними клетками по алгоритму LINDA и группы
//! возбуждения (`AtmosphereSystem.LINDA.cs`, `AtmosphereSystem.ExcitedGroup.cs`,
//! `AtmosphereSystem.Processing.cs` из SS14).
//!
//! Не перенесено (в README Build сказано явно): выравнивание Monstermos (`atmos.monstermos_*`),
//! реакции и огонь, трубы и приборы, космический ветер (по умолчанию выключен в SS14),
//! сверхпроводимость (выключена по умолчанию).

use std::collections::{BTreeMap, BTreeSet};

use crate::gas::*;

/// Частота шагов атмосферы, Гц (`atmos.tickrate`).
pub const ATMOS_TICK_RATE: f32 = 15.0;
pub const ATMOS_TIME: f32 = 1.0 / ATMOS_TICK_RATE;

/// Стадии одного цикла в порядке SS14 при настройках по умолчанию, кроме `TileEqualize`
/// (Monstermos не перенесён). Каждая стадия занимает один шаг по `ATMOS_TIME`; обмен газом
/// выполняется на стадии `ActiveTiles`, поэтому полный проход по клеткам идёт раз в `STATES.len()`
/// шагов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Revalidate,
    ActiveTiles,
    ExcitedGroups,
    HighPressureDelta,
    DeltaPressure,
    Hotspots,
    PipeNet,
    AtmosDevices,
}

const STATES: [State; 8] = [
    State::Revalidate,
    State::ActiveTiles,
    State::ExcitedGroups,
    State::HighPressureDelta,
    State::DeltaPressure,
    State::Hotspots,
    State::PipeNet,
    State::AtmosDevices,
];

/// Клетка сетки: плоскость XZ, шаг 1 м.
pub type Cell = (i32, i32);

/// Что в клетке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    /// Помещение с воздухом.
    Air,
    /// Космос: пустая неизменяемая смесь (решётка, дыра в обшивке, область без атмосферы).
    Space,
}

/// Ребро между двумя соседними клетками (с упорядоченными концами).
pub type Edge = (Cell, Cell);

pub fn edge(a: Cell, b: Cell) -> Edge {
    if a <= b { (a, b) } else { (b, a) }
}

/// Описание геометрии для атмосферы: какие клетки есть и какие рёбра непроницаемы.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    pub cells: BTreeMap<Cell, CellKind>,
    /// Рёбра, закрытые стеной, окном или закрытой дверью.
    pub blocked: BTreeSet<Edge>,
}

struct Tile {
    cell: Cell,
    space: bool,
    air: GasMixture,
    archived: Option<GasMixture>,
    archived_cycle: i64,
    current_cycle: i64,
    last_share: f32,
    excited: bool,
    group: Option<usize>,
    /// Соседи: север (z−1), юг (z+1), восток (x+1), запад (x−1).
    adjacent: [Option<usize>; 4],
}

const DIRS: [(i32, i32); 4] = [(0, -1), (0, 1), (1, 0), (-1, 0)];

struct Group {
    tiles: Vec<usize>,
    dismantle_cooldown: u32,
    breakdown_cooldown: u32,
}

/// Атмосфера одного этажа.
pub struct Grid {
    tiles: Vec<Tile>,
    index: BTreeMap<Cell, usize>,
    active: BTreeSet<usize>,
    groups: Vec<Option<Group>>,
    layout: Layout,
    timer: f32,
    state: usize,
    cycle: i64,
}

impl Default for Grid {
    fn default() -> Self {
        Self::new()
    }
}

impl Grid {
    pub fn new() -> Self {
        Self {
            tiles: Vec::new(),
            index: BTreeMap::new(),
            active: BTreeSet::new(),
            groups: Vec::new(),
            layout: Layout::default(),
            timer: 0.0,
            state: 0,
            cycle: 0,
        }
    }

    /// Номер завершённых циклов (`UpdateCounter`).
    pub fn cycles(&self) -> i64 {
        self.cycle
    }

    pub fn active_tiles(&self) -> usize {
        self.active.len()
    }

    pub fn contains(&self, cell: Cell) -> bool {
        self.index.contains_key(&cell)
    }

    /// Воздух клетки; вне сетки — космос.
    pub fn air(&self, cell: Cell) -> GasMixture {
        self.index
            .get(&cell)
            .map_or_else(GasMixture::space, |&i| self.tiles[i].air.clone())
    }

    pub fn pressure(&self, cell: Cell) -> f32 {
        self.air(cell).pressure()
    }

    pub fn is_space(&self, cell: Cell) -> bool {
        self.index.get(&cell).is_none_or(|&i| self.tiles[i].space)
    }

    /// Сумма молей по всей сетке (без космоса): для проверки сохранения.
    pub fn total_moles(&self) -> f32 {
        self.tiles
            .iter()
            .filter(|t| !t.space)
            .map(|t| t.air.total_moles())
            .sum()
    }

    /// Взять смесь из клетки (вдох): `None`, если клетки нет или в ней космос.
    pub fn remove_volume(&mut self, cell: Cell, litres: f32) -> Option<GasMixture> {
        let &i = self.index.get(&cell)?;
        self.excite(i);
        Some(self.tiles[i].air.remove_volume(litres))
    }

    /// Влить смесь в клетку (выдох, возврат непереваренного вдоха).
    pub fn merge_into(&mut self, cell: Cell, gas: &GasMixture) {
        if let Some(&i) = self.index.get(&cell) {
            self.excite(i);
            self.tiles[i].air.merge(gas);
        }
    }

    fn excite(&mut self, i: usize) {
        self.add_active(i);
    }

    /// Построить или перестроить сетку под новую геометрию. Воздух уцелевших клеток сохраняется,
    /// новые клетки получают стандартный воздух, космос — пустоту. Клетки, у которых изменилась
    /// проходимость, и их соседи снова становятся активными (`UpdateAdjacentTiles(activate)`).
    pub fn set_layout(&mut self, layout: Layout) {
        let old: BTreeMap<Cell, GasMixture> = self
            .tiles
            .iter()
            .filter(|t| !t.space)
            .map(|t| (t.cell, t.air.clone()))
            .collect();
        let old_blocked = std::mem::take(&mut self.layout.blocked);
        let first = self.tiles.is_empty();
        self.tiles.clear();
        self.index.clear();
        self.active.clear();
        self.groups.clear();

        // Клетки сетки.
        for (&cell, &kind) in &layout.cells {
            let space = kind == CellKind::Space;
            let air = if space {
                GasMixture::space()
            } else {
                old.get(&cell)
                    .cloned()
                    .unwrap_or_else(GasMixture::standard_air)
            };
            self.push_tile(cell, space, air);
        }
        // Космос за краем сетки: по клетке на каждое открытое соседнее место.
        let cells: Vec<Cell> = layout.cells.keys().copied().collect();
        for cell in cells {
            for (dx, dz) in DIRS {
                let n = (cell.0 + dx, cell.1 + dz);
                if !self.index.contains_key(&n) && !layout.blocked.contains(&edge(cell, n)) {
                    self.push_tile(n, true, GasMixture::space());
                }
            }
        }
        // Соседство.
        for i in 0..self.tiles.len() {
            let cell = self.tiles[i].cell;
            for (d, (dx, dz)) in DIRS.iter().enumerate() {
                let n = (cell.0 + dx, cell.1 + dz);
                let adj = self
                    .index
                    .get(&n)
                    .copied()
                    .filter(|_| !layout.blocked.contains(&edge(cell, n)));
                self.tiles[i].adjacent[d] = adj;
            }
        }
        // Активные: при первой сборке — всё, что граничит с чем-то отличным; иначе — рёбра, что
        // изменились.
        let changed: BTreeSet<Cell> = layout
            .blocked
            .symmetric_difference(&old_blocked)
            .flat_map(|(a, b)| [*a, *b])
            .collect();
        for i in 0..self.tiles.len() {
            let t = &self.tiles[i];
            let touches_space = t.adjacent.iter().flatten().any(|&n| self.tiles[n].space);
            let new_tile = !old.contains_key(&t.cell) && !t.space;
            if (first && touches_space) || changed.contains(&t.cell) || new_tile {
                self.add_active(i);
                for n in self.tiles[i].adjacent.into_iter().flatten() {
                    self.add_active(n);
                }
            }
        }
        self.layout = layout;
    }

    fn push_tile(&mut self, cell: Cell, space: bool, air: GasMixture) {
        let i = self.tiles.len();
        self.tiles.push(Tile {
            cell,
            space,
            air,
            archived: None,
            archived_cycle: -1,
            current_cycle: -1,
            last_share: 0.0,
            excited: false,
            group: None,
            adjacent: [None; 4],
        });
        self.index.insert(cell, i);
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Двигать время атмосферы: один шаг стадии на каждые `ATMOS_TIME`.
    pub fn advance(&mut self, dt: f32) {
        self.timer += dt;
        while self.timer >= ATMOS_TIME {
            self.timer -= ATMOS_TIME;
            self.step();
        }
    }

    fn step(&mut self) {
        match STATES[self.state] {
            State::ActiveTiles => self.process_active_tiles(),
            State::ExcitedGroups => self.process_excited_groups(),
            // Остальные стадии SS14 заняты тем, что здесь не перенесено; они сохраняют длину цикла.
            State::Revalidate
            | State::HighPressureDelta
            | State::DeltaPressure
            | State::Hotspots
            | State::PipeNet
            | State::AtmosDevices => {}
        }
        self.state += 1;
        if self.state == STATES.len() {
            self.state = 0;
            self.cycle += 1;
        }
    }

    /// Выполнить подряд `n` полных циклов (для тестов и прогрева).
    pub fn run_cycles(&mut self, n: usize) {
        for _ in 0..n * STATES.len() {
            self.step();
        }
    }

    fn add_active(&mut self, i: usize) {
        if self.tiles[i].excited {
            return;
        }
        self.tiles[i].excited = true;
        self.active.insert(i);
    }

    fn remove_active(&mut self, i: usize, dispose_group: bool) {
        if !self.tiles[i].excited {
            return;
        }
        self.tiles[i].excited = false;
        self.active.remove(&i);
        let Some(g) = self.tiles[i].group else { return };
        if dispose_group {
            self.group_dispose(g);
        } else {
            self.tiles[i].group = None;
            if let Some(Some(group)) = self.groups.get_mut(g) {
                group.tiles.retain(|&t| t != i);
            }
        }
    }

    fn archive(&mut self, i: usize) {
        let t = &mut self.tiles[i];
        t.archived = Some(t.air.clone());
        t.archived_cycle = self.cycle;
    }

    fn process_active_tiles(&mut self) {
        let run: Vec<usize> = self.active.iter().copied().collect();
        for i in run {
            self.process_cell(i);
        }
    }

    /// `ProcessCell` из LINDA.
    fn process_cell(&mut self, i: usize) {
        if self.tiles[i].archived_cycle < self.cycle {
            self.archive(i);
        }
        self.tiles[i].current_cycle = self.cycle;
        let adjacent_count = self.tiles[i].adjacent.iter().flatten().count();
        for d in 0..4 {
            let Some(e) = self.tiles[i].adjacent[d] else {
                continue;
            };
            if self.cycle <= self.tiles[e].current_cycle {
                continue;
            }
            self.archive(e);
            let mut share = false;
            let (gi, ge) = (self.tiles[i].group, self.tiles[e].group);
            if let (Some(a), Some(b)) = (gi, ge) {
                if a != b {
                    self.group_merge(a, b);
                }
                share = true;
            } else if self.compare(i, e) != Compare::NoExchange {
                self.add_active(e);
                let group = match (self.tiles[i].group, self.tiles[e].group) {
                    (Some(g), _) | (None, Some(g)) => g,
                    (None, None) => {
                        self.groups.push(Some(Group {
                            tiles: Vec::new(),
                            dismantle_cooldown: 0,
                            breakdown_cooldown: 0,
                        }));
                        self.groups.len() - 1
                    }
                };
                if self.tiles[i].group.is_none() {
                    self.group_add(group, i);
                }
                if self.tiles[e].group.is_none() {
                    self.group_add(group, e);
                }
                share = true;
            }
            if share {
                self.share(i, e, adjacent_count);
                self.last_share_check(i);
            }
        }
        let remove = true;
        if self.tiles[i].group.is_none() && remove {
            self.remove_active(i, true);
        }
    }

    fn compare(&self, a: usize, b: usize) -> Compare {
        match (&self.tiles[a].archived, &self.tiles[b].archived) {
            (Some(x), Some(y)) => compare_exchange(x, y),
            _ => Compare::NoExchange,
        }
    }

    fn last_share_check(&mut self, i: usize) {
        let Some(g) = self.tiles[i].group else { return };
        let last = self.tiles[i].last_share;
        if last > MINIMUM_AIR_TO_SUSPEND {
            self.group_reset(g);
        } else if last > MINIMUM_MOLES_DELTA_TO_MOVE
            && let Some(Some(group)) = self.groups.get_mut(g)
        {
            group.dismantle_cooldown = 0;
        }
    }

    /// `Share`: выровнять смеси между двумя клетками. Возвращает разность давлений.
    fn share(&mut self, recv: usize, shar: usize, adjacent: usize) -> f32 {
        let (Some(ra), Some(sa)) = (
            self.tiles[recv].archived.clone(),
            self.tiles[shar].archived.clone(),
        ) else {
            return 0.0;
        };
        let (a, b) = two_mut(&mut self.tiles, recv, shar);
        let temperature_delta = ra.temperature() - sa.temperature();
        let abs_temperature_delta = temperature_delta.abs();
        let consider = abs_temperature_delta > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER;
        let (old_hc, old_shc) = if consider {
            (a.air.heat_capacity(), b.air.heat_capacity())
        } else {
            (0.0, 0.0)
        };
        let (mut hc_to_sharer, mut hc_sharer_to_this) = (0.0, 0.0);
        let (mut moved, mut abs_moved) = (0.0f32, 0.0f32);
        for (i, capacity) in MOLAR_HEAT_CAPACITY.iter().enumerate() {
            let this_v = a.air.moles[i];
            let sharer_v = b.air.moles[i];
            let delta = (this_v - sharer_v) / (adjacent as f32 + 1.0);
            if delta.abs() < GAS_MIN_MOLES {
                continue;
            }
            if consider {
                let cap = delta * capacity / HEAT_SCALE;
                if delta > 0.0 {
                    hc_to_sharer += cap;
                } else {
                    hc_sharer_to_this -= cap;
                }
            }
            if !a.air.immutable {
                a.air.moles[i] -= delta;
            }
            if !b.air.immutable {
                b.air.moles[i] += delta;
            }
            moved += delta;
            abs_moved += delta.abs();
        }
        a.last_share = abs_moved;
        if consider {
            let new_hc = old_hc + hc_sharer_to_this - hc_to_sharer;
            let new_shc = old_shc + hc_to_sharer - hc_sharer_to_this;
            if !a.air.immutable && new_hc > MINIMUM_HEAT_CAPACITY {
                let t = (old_hc * a.air.temperature - hc_to_sharer * ra.temperature()
                    + hc_sharer_to_this * sa.temperature())
                    / new_hc;
                a.air.temperature = t.clamp(TCMB, TMAX);
            }
            if !b.air.immutable && new_shc > MINIMUM_HEAT_CAPACITY {
                let t = (old_shc * b.air.temperature - hc_sharer_to_this * sa.temperature()
                    + hc_to_sharer * ra.temperature())
                    / new_shc;
                b.air.temperature = t.clamp(TCMB, TMAX);
            }
            if old_shc.abs() > MINIMUM_HEAT_CAPACITY && (new_shc / old_shc - 1.0).abs() < 0.1 {
                temperature_share(a, b, &ra, &sa, OPEN_HEAT_TRANSFER_COEFFICIENT);
            }
        }
        if abs_temperature_delta <= MINIMUM_TEMPERATURE_TO_MOVE
            && moved.abs() <= MINIMUM_MOLES_DELTA_TO_MOVE
        {
            return 0.0;
        }
        let (moles, their) = (a.air.total_moles(), b.air.total_moles());
        (ra.temperature() * (moles + moved) - sa.temperature() * (their - moved)) * R / a.air.volume
    }

    // ---- группы возбуждения -------------------------------------------------------------------

    fn group_add(&mut self, g: usize, tile: usize) {
        if let Some(Some(group)) = self.groups.get_mut(g) {
            group.tiles.push(tile);
            group.dismantle_cooldown = 0;
            group.breakdown_cooldown = 0;
            self.tiles[tile].group = Some(g);
        }
    }

    fn group_reset(&mut self, g: usize) {
        if let Some(Some(group)) = self.groups.get_mut(g) {
            group.dismantle_cooldown = 0;
            group.breakdown_cooldown = 0;
        }
    }

    fn group_merge(&mut self, ours: usize, other: usize) {
        let our_size = self.groups[ours].as_ref().map_or(0, |g| g.tiles.len());
        let other_size = self.groups[other].as_ref().map_or(0, |g| g.tiles.len());
        let (winner, loser) = if our_size > other_size {
            (ours, other)
        } else {
            (other, ours)
        };
        let moved = self.groups[loser]
            .as_mut()
            .map(|g| std::mem::take(&mut g.tiles))
            .unwrap_or_default();
        for &t in &moved {
            self.tiles[t].group = Some(winner);
        }
        if let Some(Some(w)) = self.groups.get_mut(winner) {
            w.tiles.extend(moved);
        }
        self.group_dispose(loser);
        self.group_reset(winner);
    }

    fn group_dispose(&mut self, g: usize) {
        let Some(group) = self.groups.get_mut(g).and_then(Option::take) else {
            return;
        };
        for t in group.tiles {
            self.tiles[t].group = None;
        }
    }

    fn group_self_breakdown(&mut self, g: usize) {
        let tiles = match self.groups[g].as_ref() {
            Some(group) => group.tiles.clone(),
            None => return,
        };
        if tiles.is_empty() {
            self.group_dispose(g);
            return;
        }
        let mut combined = GasMixture::new(CELL_VOLUME);
        for &t in &tiles {
            combined.merge(&self.tiles[t].air);
        }
        combined.multiply(1.0 / tiles.len() as f32);
        for &t in &tiles {
            self.tiles[t].air.copy_from(&combined);
        }
        if let Some(Some(group)) = self.groups.get_mut(g) {
            group.breakdown_cooldown = 0;
        }
    }

    fn deactivate_group(&mut self, g: usize) {
        let tiles = self.groups[g]
            .as_mut()
            .map(|group| std::mem::take(&mut group.tiles))
            .unwrap_or_default();
        for t in tiles {
            self.tiles[t].group = None;
            self.remove_active(t, true);
        }
    }

    fn process_excited_groups(&mut self) {
        for g in 0..self.groups.len() {
            let Some(group) = self.groups[g].as_mut() else {
                continue;
            };
            group.breakdown_cooldown += 1;
            group.dismantle_cooldown += 1;
            let (breakdown, dismantle) = (group.breakdown_cooldown, group.dismantle_cooldown);
            if breakdown > EXCITED_GROUP_BREAKDOWN_CYCLES {
                self.group_self_breakdown(g);
            } else if dismantle > EXCITED_GROUPS_DISMANTLE_CYCLES {
                self.deactivate_group(g);
            }
        }
    }
}

fn two_mut(tiles: &mut [Tile], a: usize, b: usize) -> (&mut Tile, &mut Tile) {
    debug_assert_ne!(a, b);
    if a < b {
        let (l, r) = tiles.split_at_mut(b);
        (&mut l[a], &mut r[0])
    } else {
        let (l, r) = tiles.split_at_mut(a);
        (&mut r[0], &mut l[b])
    }
}

/// `TemperatureShare`: теплопроводность между клетками по архивным смесям.
fn temperature_share(
    a: &mut Tile,
    b: &mut Tile,
    ra: &GasMixture,
    sa: &GasMixture,
    coefficient: f32,
) {
    let delta = ra.temperature() - sa.temperature();
    if delta.abs() <= MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
        return;
    }
    let (hc, shc) = (ra.heat_capacity(), sa.heat_capacity());
    if shc > MINIMUM_HEAT_CAPACITY && hc > MINIMUM_HEAT_CAPACITY {
        let heat = coefficient * delta * (hc * shc / (hc + shc));
        if !a.air.immutable {
            a.air.temperature = (a.air.temperature - heat / hc).max(TCMB).abs();
        }
        if !b.air.immutable {
            b.air.temperature = (b.air.temperature + heat / shc).max(TCMB).abs();
        }
    }
}

#[cfg(test)]
mod tests;
