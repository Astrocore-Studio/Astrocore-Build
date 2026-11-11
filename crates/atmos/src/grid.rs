//! Сетка атмосферы: обмен газом между соседними клетками по алгоритму LINDA и группы
//! возбуждения (`AtmosphereSystem.LINDA.cs`, `AtmosphereSystem.ExcitedGroup.cs`,
//! `AtmosphereSystem.Processing.cs` из SS14).
//!
//! Выравнивание давления и взрывная разгерметизация Monstermos (`AtmosphereSystem.Monstermos.cs`)
//! включены, как в SS14 по умолчанию. Не перенесено (в README Build сказано явно): реакции и огонь,
//! трубы и приборы, космический ветер (по умолчанию выключен в SS14), сверхпроводимость.

use std::collections::{BTreeMap, BTreeSet};

use crate::gas::*;
use crate::reactions;

/// Частота шагов атмосферы, Гц (`atmos.tickrate`).
pub const ATMOS_TICK_RATE: f32 = 15.0;
pub const ATMOS_TIME: f32 = 1.0 / ATMOS_TICK_RATE;

/// Пределы выравнивания Monstermos (`Atmospherics.MonstermosTileLimit`, `...HardTileLimit`).
pub const MONSTERMOS_TILE_LIMIT: usize = 200;
pub const MONSTERMOS_HARD_TILE_LIMIT: usize = 2000;
/// Параметры разгерметизации по умолчанию (`atmos.mmos_spacing_speed`, `mmos_min_gas`, `mmos_max_wind`).
pub const SPACING_ESCAPE_RATIO: f32 = 0.15;
pub const SPACING_MIN_GAS: f32 = 2.0;
pub const SPACING_MAX_WIND: f32 = 500.0;

/// Стадии одного цикла в порядке SS14 при настройках по умолчанию. Каждая стадия занимает один шаг по `ATMOS_TIME`; обмен газом
/// выполняется на стадии `ActiveTiles`, поэтому полный проход по клеткам идёт раз в `STATES.len()`
/// шагов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Revalidate,
    TileEqualize,
    ActiveTiles,
    ExcitedGroups,
    HighPressureDelta,
    DeltaPressure,
    Hotspots,
    PipeNet,
    AtmosDevices,
}

const STATES: [State; 9] = [
    State::Revalidate,
    State::TileEqualize,
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
    /// Вертикальные связи между этажами (лестницы): нижняя клетка, верхняя клетка. Газ идёт по ним
    /// как по обычному ребру. Этажи разнесены по `z` на `FLOOR_STRIDE`, см. [`floor_cell`].
    pub links: BTreeSet<(Cell, Cell)>,
    /// Стартовый состав воздуха клеток, у которых он не стандартный (помещение с плазмой).
    /// Применяется только к клеткам, которых раньше в сетке не было.
    pub fills: BTreeMap<Cell, Fill>,
}

/// Нестандартный воздух: стандартный воздух станции плюс добавка газов.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fill {
    /// Сколько молей каждого газа добавить к стандартному воздуху (в стандартной клетке).
    pub extra: Vec<(Gas, f32)>,
    /// Температура смеси, К; `0` — как у стандартного воздуха.
    pub temperature: f32,
}

impl Fill {
    pub fn mixture(&self) -> GasMixture {
        let mut m = GasMixture::standard_air();
        for &(gas, moles) in &self.extra {
            m.adjust_moles(gas, moles);
        }
        if self.temperature > 0.0 {
            m.set_temperature(self.temperature);
        }
        m
    }
}

/// Шаг по `z` между этажами в общей сетке: клетки разных этажей не бывают соседями по плоскости.
pub const FLOOR_STRIDE: i32 = 1_000_000;

/// Клетка этажа `floor` в общей сетке.
pub fn floor_cell(floor: i32, x: i32, z: i32) -> Cell {
    (x, z + floor * FLOOR_STRIDE)
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
    adjacent: [Option<usize>; ND],
    mm: Mm,
    hotspot: Hotspot,
}

/// `Hotspot`: газовый огонь на клетке.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Hotspot {
    pub valid: bool,
    pub skipped_first_process: bool,
    /// Огонь заполнил клетку и горит прямо в её воздухе.
    pub bypassing: bool,
    pub temperature: f32,
    pub volume: f32,
    /// Размер огня для отрисовки: 1 — искра, 2 — пламя, 3 — пожар на всю клетку.
    pub state: u8,
}

/// Огонь на клетке, как его видят другие системы (`TileFireEvent`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fire {
    pub cell: Cell,
    pub temperature: f32,
    pub volume: f32,
    pub state: u8,
}

/// `MonstermosInfo`: рабочие данные выравнивания клетки.
#[derive(Clone, Copy, Default)]
struct Mm {
    last_cycle: i64,
    last_queue_cycle: i64,
    last_slow_queue_cycle: i64,
    mole_delta: f32,
    transfer: [f32; ND],
    current_transfer_amount: f32,
    current_transfer_dir: Option<usize>,
    fast_done: bool,
}

/// Число направлений: север, юг, восток, запад, вверх, вниз (противоположное — `d ^ 1`).
const ND: usize = 6;
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
    hotspots: BTreeSet<usize>,
    groups: Vec<Option<Group>>,
    layout: Layout,
    timer: f32,
    state: usize,
    cycle: i64,
    queue_cycle_control: i64,
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
            hotspots: BTreeSet::new(),
            groups: Vec::new(),
            layout: Layout::default(),
            timer: 0.0,
            state: 0,
            cycle: 0,
            queue_cycle_control: 0,
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
        let old_hotspots: BTreeMap<Cell, Hotspot> = self
            .hotspots
            .iter()
            .map(|&i| (self.tiles[i].cell, self.tiles[i].hotspot))
            .collect();
        let old_blocked = std::mem::take(&mut self.layout.blocked);
        let old_links = std::mem::take(&mut self.layout.links);
        let first = self.tiles.is_empty();
        self.tiles.clear();
        self.index.clear();
        self.queue_cycle_control = 0;
        self.active.clear();
        self.hotspots.clear();
        self.groups.clear();

        // Клетки сетки.
        for (&cell, &kind) in &layout.cells {
            let space = kind == CellKind::Space;
            let air = if space {
                GasMixture::space()
            } else {
                old.get(&cell).cloned().unwrap_or_else(|| {
                    layout
                        .fills
                        .get(&cell)
                        .map_or_else(GasMixture::standard_air, Fill::mixture)
                })
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
        // Вертикальные связи.
        for &(lower, upper) in &layout.links {
            if let (Some(&a), Some(&b)) = (self.index.get(&lower), self.index.get(&upper)) {
                self.tiles[a].adjacent[4] = Some(b);
                self.tiles[b].adjacent[5] = Some(a);
            }
        }
        // Активные: при первой сборке — всё, что граничит с чем-то отличным; иначе — рёбра, что
        // изменились.
        let mut changed: BTreeSet<Cell> = layout
            .blocked
            .symmetric_difference(&old_blocked)
            .flat_map(|(a, b)| [*a, *b])
            .collect();
        changed.extend(
            layout
                .links
                .symmetric_difference(&old_links)
                .flat_map(|(a, b)| [*a, *b]),
        );
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
        for (cell, hotspot) in old_hotspots {
            if let Some(&i) = self.index.get(&cell)
                && !self.tiles[i].space
            {
                self.tiles[i].hotspot = hotspot;
                self.hotspots.insert(i);
                self.add_active(i);
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
            adjacent: [None; ND],
            mm: Mm::default(),
            hotspot: Hotspot::default(),
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
            State::TileEqualize => self.process_tile_equalize(),
            State::ActiveTiles => self.process_active_tiles(),
            State::Hotspots => self.process_hotspots(),
            State::ExcitedGroups => self.process_excited_groups(),
            // Остальные стадии SS14 заняты тем, что здесь не перенесено; они сохраняют длину цикла.
            State::Revalidate
            | State::HighPressureDelta
            | State::DeltaPressure
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
        for d in 0..ND {
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
        let outcome = reactions::react(&mut self.tiles[i].air, true);
        if let Some((temperature, volume)) = outcome.expose {
            self.hotspot_expose(i, temperature, volume, false);
        }
        let remove = true;
        if self.tiles[i].group.is_none() && remove {
            self.remove_active(i, true);
        }
    }

    /// `HotspotExpose`: зажечь клетку, если в ней есть окислитель и топливо. С `soh` существующий
    /// огонь раздувается до заданных температуры и объёма.
    fn hotspot_expose(&mut self, i: usize, temperature: f32, volume: f32, soh: bool) {
        if self.tiles[i].space || !self.tiles[i].air.is_oxidizer() {
            return;
        }
        let flammable = self.tiles[i].air.is_fuel();
        if self.tiles[i].hotspot.valid {
            if soh && flammable {
                let h = &mut self.tiles[i].hotspot;
                h.temperature = h.temperature.max(temperature);
                h.volume = h.volume.max(volume);
            }
            return;
        }
        if temperature > reactions::PLASMA_MINIMUM_BURN_TEMPERATURE && flammable {
            self.tiles[i].hotspot = Hotspot {
                valid: true,
                skipped_first_process: self.tiles[i].current_cycle > self.cycle,
                bypassing: false,
                temperature,
                volume: volume * 25.0,
                state: 1,
            };
            self.add_active(i);
            self.hotspots.insert(i);
        }
    }

    /// Поджечь клетку снаружи (искра, сварка, зажигалка).
    pub fn ignite(&mut self, cell: Cell, temperature: f32, volume: f32) {
        if let Some(&i) = self.index.get(&cell) {
            self.hotspot_expose(i, temperature, volume, true);
        }
    }

    /// Горит ли клетка и чем.
    pub fn hotspot(&self, cell: Cell) -> Option<Hotspot> {
        let &i = self.index.get(&cell)?;
        Some(self.tiles[i].hotspot).filter(|h| h.valid)
    }

    /// Все клетки с огнём.
    pub fn fires(&self) -> Vec<Fire> {
        self.hotspots
            .iter()
            .filter_map(|&i| {
                let t = &self.tiles[i];
                t.hotspot.valid.then_some(Fire {
                    cell: t.cell,
                    temperature: t.hotspot.temperature,
                    volume: t.hotspot.volume,
                    state: t.hotspot.state,
                })
            })
            .collect()
    }

    /// Мгновенно изменить воздух клетки (для тестов, взрывов, содержимого баллонов).
    pub fn with_air<R>(&mut self, cell: Cell, f: impl FnOnce(&mut GasMixture) -> R) -> Option<R> {
        let &i = self.index.get(&cell)?;
        self.add_active(i);
        Some(f(&mut self.tiles[i].air))
    }

    fn process_hotspots(&mut self) {
        let run: Vec<usize> = self.hotspots.iter().copied().collect();
        for i in run {
            self.process_hotspot(i);
        }
    }

    /// `ProcessHotspot`.
    fn process_hotspot(&mut self, i: usize) {
        if !self.tiles[i].hotspot.valid {
            self.hotspots.remove(&i);
            return;
        }
        self.add_active(i);
        if !self.tiles[i].hotspot.skipped_first_process {
            self.tiles[i].hotspot.skipped_first_process = true;
            return;
        }
        if let Some(g) = self.tiles[i].group {
            self.group_reset(g);
        }
        let h = self.tiles[i].hotspot;
        if h.temperature < FIRE_MINIMUM_TEMPERATURE_TO_EXIST
            || h.volume <= 1.0
            || self.tiles[i].space
            || !self.tiles[i].air.is_ignitable()
        {
            self.tiles[i].hotspot = Hotspot::default();
            return;
        }
        self.perform_hotspot_exposure(i);
        if self.tiles[i].hotspot.bypassing {
            self.tiles[i].hotspot.state = 3;
            let air_temperature = self.tiles[i].air.temperature();
            if air_temperature > FIRE_MINIMUM_TEMPERATURE_TO_SPREAD {
                let radiated = air_temperature * FIRE_SPREAD_RADIOSITY_SCALE;
                for e in self.tiles[i].adjacent.into_iter().flatten() {
                    if !self.tiles[e].hotspot.valid {
                        self.hotspot_expose(e, radiated, CELL_VOLUME / 4.0, false);
                    }
                }
            }
        } else {
            let h = &mut self.tiles[i].hotspot;
            h.state = if h.volume > CELL_VOLUME * 0.4 { 2 } else { 1 };
        }
    }

    /// `PerformHotspotExposure`: огонь тратит топливо либо во всём воздухе клетки, либо в доле
    /// объёма, равной размеру огня.
    fn perform_hotspot_exposure(&mut self, i: usize) {
        if self.tiles[i].space || !self.tiles[i].hotspot.valid {
            return;
        }
        let air_volume = self.tiles[i].air.volume;
        let h = self.tiles[i].hotspot;
        let bypassing = h.skipped_first_process && h.volume > air_volume * 0.95;
        self.tiles[i].hotspot.bypassing = bypassing;
        if bypassing {
            let t = &mut self.tiles[i];
            t.hotspot.volume = t.air.fire * FIRE_GROWTH_RATE;
            t.hotspot.temperature = t.air.temperature();
        } else {
            let mut affected = self.tiles[i].air.remove_volume(h.volume);
            affected.set_temperature(h.temperature);
            reactions::react(&mut affected, true);
            let t = &mut self.tiles[i];
            t.hotspot.temperature = affected.temperature();
            t.hotspot.volume = affected.fire * FIRE_GROWTH_RATE;
            t.air.merge(&affected);
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

    // ---- Monstermos ---------------------------------------------------------------------------

    fn process_tile_equalize(&mut self) {
        let run: Vec<usize> = self.active.iter().copied().collect();
        for i in run {
            self.equalize_pressure_in_zone(i);
        }
    }

    fn mole_total(&self, i: usize) -> f32 {
        self.tiles[i].air.total_moles()
    }

    /// `EqualizePressureInZone`: выровнять количество газа по связной области вокруг клетки,
    /// двигая газ по кратчайшим путям от избытка к нехватке; рядом с космосом — взрывная
    /// разгерметизация.
    fn equalize_pressure_in_zone(&mut self, start: usize) {
        let cycle = self.cycle;
        if self.tiles[start].mm.last_cycle >= cycle {
            return;
        }
        self.tiles[start].mm = Mm::default();
        let starting = self.mole_total(start);
        let mut run_atmos = false;
        for d in 0..ND {
            let Some(o) = self.tiles[start].adjacent[d] else {
                continue;
            };
            if (self.mole_total(o) - starting).abs() > MINIMUM_MOLES_DELTA_TO_MOVE {
                run_atmos = true;
                break;
            }
        }
        if !run_atmos {
            self.tiles[start].mm.last_cycle = cycle;
            return;
        }
        self.queue_cycle_control += 1;
        let queue_cycle = self.queue_cycle_control;
        let mut total_moles = 0.0f32;
        let mut eq: Vec<usize> = vec![start];
        self.tiles[start].mm.last_queue_cycle = queue_cycle;
        let mut i = 0;
        while i < eq.len() {
            if i > MONSTERMOS_HARD_TILE_LIMIT {
                break;
            }
            let exploring = eq[i];
            if i < MONSTERMOS_TILE_LIMIT {
                let moles = self.mole_total(exploring);
                self.tiles[exploring].mm.mole_delta = moles;
                total_moles += moles;
            }
            for d in 0..ND {
                let Some(adj) = self.tiles[exploring].adjacent[d] else {
                    continue;
                };
                if self.tiles[adj].mm.last_queue_cycle == queue_cycle {
                    continue;
                }
                self.tiles[adj].mm = Mm {
                    last_queue_cycle: queue_cycle,
                    ..Mm::default()
                };
                if eq.len() < MONSTERMOS_HARD_TILE_LIMIT {
                    eq.push(adj);
                }
                if self.tiles[adj].space {
                    // Кто-то открыл шлюз в космос.
                    self.explosively_depressurize(start);
                    return;
                }
            }
            i += 1;
        }
        let mut tile_count = eq.len();
        if tile_count > MONSTERMOS_TILE_LIMIT {
            for &t in &eq[MONSTERMOS_TILE_LIMIT..] {
                self.tiles[t].mm.last_queue_cycle = 0;
            }
            tile_count = MONSTERMOS_TILE_LIMIT;
            eq.truncate(tile_count);
        }
        let average = total_moles / tile_count as f32;
        let mut givers: Vec<usize> = Vec::new();
        let mut takers: Vec<usize> = Vec::new();
        for &t in &eq {
            self.tiles[t].mm.last_cycle = cycle;
            self.tiles[t].mm.mole_delta -= average;
            if self.tiles[t].mm.mole_delta > 0.0 {
                givers.push(t);
            } else {
                takers.push(t);
            }
        }
        let log_n = (tile_count as f32).log2();
        // Быстрый проход O(n log n): может не сработать, но ускорит следующий.
        if givers.len() as f32 > log_n && takers.len() as f32 > log_n {
            eq.sort_by(|a, b| {
                self.tiles[*a]
                    .mm
                    .mole_delta
                    .total_cmp(&self.tiles[*b].mm.mole_delta)
            });
            for t in eq.clone() {
                self.tiles[t].mm.fast_done = true;
                if self.tiles[t].mm.mole_delta <= 0.0 {
                    continue;
                }
                let mut eligible: Vec<usize> = Vec::new();
                for d in 0..ND {
                    let Some(t2) = self.tiles[t].adjacent[d] else {
                        continue;
                    };
                    if self.tiles[t2].mm.fast_done
                        || self.tiles[t2].mm.last_queue_cycle != queue_cycle
                    {
                        continue;
                    }
                    eligible.push(d);
                }
                if eligible.is_empty() {
                    continue;
                }
                let to_move = self.tiles[t].mm.mole_delta / eligible.len() as f32;
                for d in eligible {
                    self.adjust_eq_movement(t, d, to_move);
                    self.tiles[t].mm.mole_delta -= to_move;
                    if let Some(o) = self.tiles[t].adjacent[d] {
                        self.tiles[o].mm.mole_delta += to_move;
                    }
                }
            }
            givers.clear();
            takers.clear();
            for &t in &eq {
                if self.tiles[t].mm.mole_delta > 0.0 {
                    givers.push(t);
                } else {
                    takers.push(t);
                }
            }
        }
        // Медленная часть: до O(n²); идём от меньшего списка.
        if givers.len() < takers.len() {
            for &giver in &givers {
                self.tiles[giver].mm.current_transfer_dir = None;
                self.tiles[giver].mm.current_transfer_amount = 0.0;
                self.queue_cycle_control += 1;
                let slow = self.queue_cycle_control;
                let mut queue: Vec<usize> = vec![giver];
                self.tiles[giver].mm.last_slow_queue_cycle = slow;
                let mut qi = 0;
                while qi < queue.len() {
                    if self.tiles[giver].mm.mole_delta <= 0.0 {
                        break;
                    }
                    let other = queue[qi];
                    for k in 0..ND {
                        let Some(_) = self.tiles[other].adjacent[k] else {
                            continue;
                        };
                        if self.tiles[giver].mm.mole_delta <= 0.0 {
                            break;
                        }
                        let o2 = self.tiles[other].adjacent[k].unwrap_or(other);
                        if self.tiles[o2].mm.last_queue_cycle != queue_cycle
                            || self.tiles[o2].mm.last_slow_queue_cycle == slow
                        {
                            continue;
                        }
                        queue.push(o2);
                        self.tiles[o2].mm.last_slow_queue_cycle = slow;
                        self.tiles[o2].mm.current_transfer_dir = Some(k ^ 1);
                        self.tiles[o2].mm.current_transfer_amount = 0.0;
                        let delta2 = self.tiles[o2].mm.mole_delta;
                        if delta2 < 0.0 {
                            let gd = self.tiles[giver].mm.mole_delta;
                            if -delta2 > gd {
                                self.tiles[o2].mm.current_transfer_amount -= gd;
                                self.tiles[o2].mm.mole_delta += gd;
                                self.tiles[giver].mm.mole_delta = 0.0;
                            } else {
                                self.tiles[o2].mm.current_transfer_amount += delta2;
                                self.tiles[giver].mm.mole_delta += delta2;
                                self.tiles[o2].mm.mole_delta = 0.0;
                            }
                        }
                    }
                    qi += 1;
                }
                self.settle_queue(&queue);
            }
        } else {
            for &taker in &takers {
                self.tiles[taker].mm.current_transfer_dir = None;
                self.tiles[taker].mm.current_transfer_amount = 0.0;
                self.queue_cycle_control += 1;
                let slow = self.queue_cycle_control;
                let mut queue: Vec<usize> = vec![taker];
                self.tiles[taker].mm.last_slow_queue_cycle = slow;
                let mut qi = 0;
                while qi < queue.len() {
                    if self.tiles[taker].mm.mole_delta >= 0.0 {
                        break;
                    }
                    let other = queue[qi];
                    for k in 0..ND {
                        let Some(o2) = self.tiles[other].adjacent[k] else {
                            continue;
                        };
                        if self.tiles[taker].mm.mole_delta >= 0.0 {
                            break;
                        }
                        if self.tiles[o2].adjacent.iter().all(Option::is_none)
                            || self.tiles[o2].mm.last_queue_cycle != queue_cycle
                            || self.tiles[o2].mm.last_slow_queue_cycle == slow
                        {
                            continue;
                        }
                        queue.push(o2);
                        self.tiles[o2].mm.last_slow_queue_cycle = slow;
                        self.tiles[o2].mm.current_transfer_dir = Some(k ^ 1);
                        self.tiles[o2].mm.current_transfer_amount = 0.0;
                        let delta2 = self.tiles[o2].mm.mole_delta;
                        if delta2 > 0.0 {
                            let td = self.tiles[taker].mm.mole_delta;
                            if delta2 > -td {
                                self.tiles[o2].mm.current_transfer_amount -= td;
                                self.tiles[o2].mm.mole_delta += td;
                                self.tiles[taker].mm.mole_delta = 0.0;
                            } else {
                                self.tiles[o2].mm.current_transfer_amount += delta2;
                                self.tiles[taker].mm.mole_delta += delta2;
                                self.tiles[o2].mm.mole_delta = 0.0;
                            }
                        }
                    }
                    qi += 1;
                }
                self.settle_queue(&queue);
            }
        }
        for &t in &eq {
            self.finalize_eq(t);
        }
        for &t in &eq {
            for d in 0..ND {
                let Some(o2) = self.tiles[t].adjacent[d] else {
                    continue;
                };
                if self.tiles[o2].adjacent.iter().all(Option::is_none) {
                    continue;
                }
                if self.compare(o2, start) == Compare::NoExchange {
                    continue;
                }
                self.add_active(o2);
                break;
            }
        }
    }

    /// Обратный проход по очереди: накопленные потоки уходят по направлению к источнику.
    fn settle_queue(&mut self, queue: &[usize]) {
        for &t in queue.iter().rev() {
            let (amount, dir) = (
                self.tiles[t].mm.current_transfer_amount,
                self.tiles[t].mm.current_transfer_dir,
            );
            if amount == 0.0 {
                continue;
            }
            let Some(d) = dir else { continue };
            self.adjust_eq_movement(t, d, amount);
            if let Some(o) = self.tiles[t].adjacent[d] {
                self.tiles[o].mm.current_transfer_amount += amount;
            }
            self.tiles[t].mm.current_transfer_amount = 0.0;
        }
    }

    fn adjust_eq_movement(&mut self, t: usize, d: usize, amount: f32) {
        let Some(adj) = self.tiles[t].adjacent[d] else {
            return;
        };
        self.tiles[t].mm.transfer[d] += amount;
        self.tiles[adj].mm.transfer[d ^ 1] -= amount;
    }

    /// `FinalizeEq`: перенести накопленные потоки между клетками.
    fn finalize_eq(&mut self, t: usize) {
        let mut dirs = [0.0f32; ND];
        let mut has = false;
        #[allow(clippy::needless_range_loop)]
        for d in 0..ND {
            let amount = self.tiles[t].mm.transfer[d];
            if amount == 0.0 {
                continue;
            }
            dirs[d] = amount;
            self.tiles[t].mm.transfer[d] = 0.0;
            has = true;
        }
        if !has {
            return;
        }
        for (d, &amount) in dirs.iter().enumerate() {
            let Some(o) = self.tiles[t].adjacent[d] else {
                continue;
            };
            if amount <= 0.0 {
                continue;
            }
            if self.tiles[t].air.total_moles() < amount {
                // Не хватает газа: сначала дорабатываем соседей, у которых отрицательные потоки.
                #[allow(clippy::needless_range_loop)]
                for k in 0..ND {
                    if dirs[k] < 0.0
                        && let Some(n) = self.tiles[t].adjacent[k]
                    {
                        self.finalize_eq(n);
                    }
                }
            }
            self.tiles[o].mm.transfer[d ^ 1] = 0.0;
            let total = self.tiles[t].air.total_moles();
            let removed = self.tiles[t].air.remove_ratio(amount / total.max(1e-12));
            self.tiles[o].air.merge(&removed);
        }
    }

    /// `ExplosivelyDepressurize`: газ из комнаты уходит к пробоине волной; клетки у пробоины
    /// пустеют быстрее всего.
    fn explosively_depressurize(&mut self, start: usize) {
        let cycle = self.cycle;
        self.queue_cycle_control += 1;
        let queue_cycle = self.queue_cycle_control;
        let limit = MONSTERMOS_HARD_TILE_LIMIT;
        let mut tiles: Vec<usize> = vec![start];
        let mut space_tiles: Vec<usize> = Vec::new();
        self.tiles[start].mm = Mm {
            last_queue_cycle: queue_cycle,
            ..Mm::default()
        };
        let mut i = 0;
        while i < tiles.len() {
            let other = tiles[i];
            self.tiles[other].mm.last_cycle = cycle;
            self.tiles[other].mm.current_transfer_dir = None;
            if !self.tiles[other].space {
                for j in 0..ND {
                    let Some(o2) = self.tiles[other].adjacent[j] else {
                        continue;
                    };
                    if self.tiles[o2].mm.last_queue_cycle == queue_cycle {
                        continue;
                    }
                    self.tiles[o2].mm = Mm {
                        last_queue_cycle: queue_cycle,
                        ..Mm::default()
                    };
                    tiles.push(o2);
                    if tiles.len() >= limit {
                        break;
                    }
                }
            } else {
                space_tiles.push(other);
            }
            if tiles.len() < limit && space_tiles.len() < limit {
                i += 1;
                continue;
            }
            break;
        }
        self.queue_cycle_control += 1;
        let slow = self.queue_cycle_control;
        let mut order: Vec<usize> = Vec::new();
        for &t in &space_tiles {
            order.push(t);
            self.tiles[t].mm.last_slow_queue_cycle = slow;
            self.tiles[t].mm.current_transfer_dir = None;
        }
        // Из помещения от пробоины внутрь.
        let mut oi = 0;
        while oi < order.len() {
            let other = order[oi];
            for j in 0..ND {
                if self.tiles[other].adjacent[j].is_none() && !self.tiles[other].space {
                    continue;
                }
                let Some(t2) = self.tiles[other].adjacent[j] else {
                    continue;
                };
                if self.tiles[t2].mm.last_queue_cycle != queue_cycle
                    || self.tiles[t2].mm.last_slow_queue_cycle == slow
                    || self.tiles[t2].space
                {
                    continue;
                }
                self.tiles[t2].mm.current_transfer_dir = Some(j ^ 1);
                self.tiles[t2].mm.current_transfer_amount = 0.0;
                self.tiles[t2].mm.last_slow_queue_cycle = slow;
                order.push(t2);
            }
            oi += 1;
        }
        // Обратно к пробоине от краёв залитой области.
        for k in (0..order.len()).rev() {
            let other = order[k];
            let Some(dir) = self.tiles[other].mm.current_transfer_dir else {
                continue;
            };
            self.add_active(other);
            let Some(other2) = self.tiles[other].adjacent[dir] else {
                self.tiles[other].air.clear();
                self.tiles[other].air.set_temperature(TCMB);
                continue;
            };
            let mut sum = self.tiles[other].air.total_moles();
            if SPACING_ESCAPE_RATIO < 1.0 {
                sum *= SPACING_ESCAPE_RATIO;
                if sum < SPACING_MIN_GAS {
                    sum = SPACING_MIN_GAS.min(self.tiles[other].air.total_moles());
                }
                let carried = self.tiles[other].mm.current_transfer_amount;
                if sum + carried > SPACING_MAX_WIND {
                    sum = SPACING_MIN_GAS.max(SPACING_MAX_WIND - carried);
                }
            }
            self.tiles[other].mm.current_transfer_amount += sum;
            let carried = self.tiles[other].mm.current_transfer_amount;
            self.tiles[other2].mm.current_transfer_amount += carried;
            let enough = self.tiles[other].air.pressure() - sum > SPACING_MIN_GAS * 0.1;
            if enough {
                let (a, b) = two_mut(&mut self.tiles, other, other2);
                release_gas_to(&mut a.air, Some(&mut b.air), sum);
                release_gas_to(&mut b.air, None, sum * 0.3);
                if a.air.temperature() > 280.0 {
                    let total = a.air.total_moles();
                    let real_loss = (total - sum) / total;
                    let t = a.air.temperature() * (0.9 + 0.1 * real_loss);
                    a.air.set_temperature(t);
                }
            } else {
                self.tiles[other].air.clear();
                self.tiles[other].air.set_temperature(TCMB);
            }
        }
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

/// `ReleaseGasTo`: выпустить газ из `mixture` в `output` (или в никуда) до давления `target`.
fn release_gas_to(mixture: &mut GasMixture, output: Option<&mut GasMixture>, target: f32) -> bool {
    let out_pressure = output.as_ref().map_or(0.0, |o| o.pressure());
    let in_pressure = mixture.pressure();
    if out_pressure >= target.min(in_pressure - 10.0) {
        return false;
    }
    if mixture.total_moles() <= 0.0 || mixture.temperature() <= 0.0 {
        return false;
    }
    let delta = (target - out_pressure).min((in_pressure - out_pressure) / 2.0);
    let volume = output.as_ref().map_or(CELL_VOLUME, |o| o.volume);
    let transfer = delta * volume / (mixture.temperature() * R);
    let removed = mixture.remove_ratio(transfer / mixture.total_moles());
    if let Some(o) = output {
        o.merge(&removed);
    }
    true
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
