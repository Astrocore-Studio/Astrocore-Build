//! Атмосфера грида: клетки в арене, соседи и наборы номерами (ADR-0007). Образец —
//! `GridAtmosphereComponent`, `TileAtmosphere`, `MonstermosInfo`, `ExcitedGroup` и
//! `AirtightData` SS14 @ 2f4a2aad.

use std::collections::{HashMap, VecDeque};

use astrocore_map::TileCoord;
use bevy_ecs::component::Component;

use super::gas::{GasMixture, MINIMUM_HEAT_CAPACITY, T20C};

/// Направления (`AtmosDirection`): север, юг, восток, запад — биты 1, 2, 4, 8.
pub const NORTH: u8 = 1;
pub const SOUTH: u8 = 2;
pub const EAST: u8 = 4;
pub const WEST: u8 = 8;
pub const ALL_DIRECTIONS: u8 = 15;

/// Бит направления по номеру.
pub const fn direction(index: usize) -> u8 {
    1 << index
}

/// Номер противоположного направления (`ToOppositeIndex`).
pub const fn opposite_index(index: usize) -> usize {
    index ^ 1
}

/// Бит противоположного направления по номеру (`ToOppositeDir`).
pub const fn opposite_direction(index: usize) -> u8 {
    1 << (index ^ 1)
}

/// Номер направления по биту (`ToIndex`); для пустого — `None`.
pub fn direction_index(direction: u8) -> Option<usize> {
    (direction.count_ones() == 1).then(|| direction.trailing_zeros() as usize)
}

/// Соседняя клетка в направлении номер `index`.
pub fn offset(coord: TileCoord, index: usize) -> TileCoord {
    let (dx, dy) = match index {
        0 => (0, 1),
        1 => (0, -1),
        2 => (1, 0),
        _ => (-1, 0),
    };
    TileCoord::new(coord.x + dx, coord.y + dy, coord.z)
}

/// Непроницаемость клетки (`AirtightData`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AirtightData {
    pub blocked_directions: u8,
    pub no_air_when_blocked: bool,
    pub fix_vacuum: bool,
}

/// Данные выравнивания Monstermos (`MonstermosInfo`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MonstermosInfo {
    pub last_cycle: i32,
    pub last_queue_cycle: i64,
    pub last_slow_queue_cycle: i64,
    pub mole_delta: f32,
    /// Переносы по направлениям (`TransferDirections`).
    pub transfer: [f32; 4],
    pub current_transfer_amount: f32,
    pub current_transfer_direction: u8,
    pub fast_done: bool,
}

/// Пламя на клетке (`Hotspot`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Hotspot {
    pub valid: bool,
    pub skipped_first_process: bool,
    pub bypassing: bool,
    pub temperature: f32,
    pub volume: f32,
    /// Вид пламени: 1 — маленькое, 2 — среднее, 3 — охватило клетку.
    pub state: u8,
}

/// Клетка (`TileAtmosphere`).
#[derive(Debug, Clone, PartialEq)]
pub struct TileAtmosphere {
    pub coord: TileCoord,
    pub archived_cycle: i32,
    pub current_cycle: i32,
    pub temperature: f32,
    pub pressure_specific_target: Option<u32>,
    pub pressure_difference: f32,
    pub heat_capacity: f32,
    pub thermal_conductivity: f32,
    pub excited: bool,
    pub space: bool,
    pub adjacent: [Option<u32>; 4],
    pub adjacent_bits: u8,
    pub monstermos: MonstermosInfo,
    pub pressure_direction: u8,
    pub last_pressure_direction: u8,
    pub excited_group: Option<u32>,
    pub air: Option<GasMixture>,
    pub air_archived: Option<GasMixture>,
    pub last_share: f32,
    pub max_fire_temperature_sustained: f32,
    pub hotspot: Hotspot,
    pub map_atmosphere: bool,
    pub no_grid_tile: bool,
    pub trim_queued: bool,
    pub airtight: AirtightData,
    /// Клетка убрана из словаря (обрезана): номер больше не используется.
    pub removed: bool,
    /// В наборах [`GridAtmosphere`]: отметки, чтобы не держать отдельные множества.
    pub in_high_pressure_delta: bool,
    pub in_map_tiles: bool,
}

impl TileAtmosphere {
    pub fn new(coord: TileCoord, air: Option<GasMixture>) -> Self {
        Self {
            coord,
            archived_cycle: 0,
            current_cycle: 0,
            temperature: T20C,
            pressure_specific_target: None,
            pressure_difference: 0.0,
            heat_capacity: MINIMUM_HEAT_CAPACITY,
            thermal_conductivity: 0.05,
            excited: false,
            space: false,
            adjacent: [None; 4],
            adjacent_bits: 0,
            monstermos: MonstermosInfo::default(),
            pressure_direction: 0,
            last_pressure_direction: 0,
            excited_group: None,
            air_archived: air.clone(),
            air,
            last_share: 0.0,
            max_fire_temperature_sustained: 0.0,
            hotspot: Hotspot::default(),
            map_atmosphere: false,
            no_grid_tile: false,
            trim_queued: false,
            airtight: AirtightData::default(),
            removed: false,
            in_high_pressure_delta: false,
            in_map_tiles: false,
        }
    }
}

/// Группа возбуждённых клеток (`ExcitedGroup`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExcitedGroup {
    pub tiles: Vec<u32>,
    pub breakdown_cooldown: i32,
    pub dismantle_cooldown: i32,
    pub disposed: bool,
    /// Поколение номера: номер группы переиспользуется после распада.
    pub generation: u32,
}

/// Упорядоченный набор номеров: вставка в конец, удаление перестановкой с последним.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexSet {
    items: Vec<u32>,
    positions: HashMap<u32, usize>,
}

impl IndexSet {
    pub fn insert(&mut self, item: u32) -> bool {
        if self.positions.contains_key(&item) {
            return false;
        }
        self.positions.insert(item, self.items.len());
        self.items.push(item);
        true
    }

    pub fn remove(&mut self, item: u32) -> bool {
        let Some(position) = self.positions.remove(&item) else {
            return false;
        };
        self.items.swap_remove(position);
        if let Some(moved) = self.items.get(position) {
            self.positions.insert(*moved, position);
        }
        true
    }

    pub fn contains(&self, item: u32) -> bool {
        self.positions.contains_key(&item)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.items.iter().copied()
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.positions.clear();
    }
}

/// Шаг обработки (`AtmosphereProcessingState`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProcessingState {
    #[default]
    Revalidate,
    TileEqualize,
    ActiveTiles,
    ExcitedGroups,
    HighPressureDelta,
    DeltaPressure,
    Hotspots,
    Superconductivity,
    PipeNet,
    AtmosDevices,
}

/// Атмосфера грида (`GridAtmosphereComponent`). Не сетевой и не компонент данных.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct GridAtmosphere {
    pub simulated: bool,
    pub update_counter: i32,
    pub equalization_queue_cycle_control: i64,
    pub tiles: Vec<TileAtmosphere>,
    pub index: HashMap<TileCoord, u32>,
    pub active_tiles: IndexSet,
    pub excited_groups: Vec<ExcitedGroup>,
    pub live_groups: IndexSet,
    free_groups: Vec<u32>,
    pub map_tiles: IndexSet,
    pub high_pressure_delta: IndexSet,
    pub hotspot_tiles: IndexSet,
    /// Клетки, вид которых устарел (`InvalidateVisuals`).
    pub visuals_dirty: IndexSet,
    pub possibly_disconnected: Vec<u32>,
    /// Клетки, которые надо пересчитать (`InvalidatedCoords`): порядок вставки без повторов.
    pub invalidated: Vec<TileCoord>,
    pub invalidated_set: std::collections::HashSet<TileCoord>,
    pub current_run_invalidated: VecDeque<u32>,
    pub current_run_tiles: VecDeque<u32>,
    pub current_run_groups: VecDeque<(u32, u32)>,
    pub state: ProcessingState,
    pub processing_paused: bool,
    pub timer: f32,
}

impl Default for GridAtmosphere {
    fn default() -> Self {
        Self {
            simulated: true,
            update_counter: 1,
            equalization_queue_cycle_control: 0,
            tiles: Vec::new(),
            index: HashMap::new(),
            active_tiles: IndexSet::default(),
            excited_groups: Vec::new(),
            live_groups: IndexSet::default(),
            free_groups: Vec::new(),
            map_tiles: IndexSet::default(),
            high_pressure_delta: IndexSet::default(),
            hotspot_tiles: IndexSet::default(),
            visuals_dirty: IndexSet::default(),
            possibly_disconnected: Vec::new(),
            invalidated: Vec::new(),
            invalidated_set: Default::default(),
            current_run_invalidated: VecDeque::new(),
            current_run_tiles: VecDeque::new(),
            current_run_groups: VecDeque::new(),
            state: ProcessingState::default(),
            processing_paused: false,
            timer: 0.0,
        }
    }
}

impl GridAtmosphere {
    /// Номер клетки.
    pub fn tile_index(&self, coord: TileCoord) -> Option<u32> {
        self.index.get(&coord).copied()
    }

    pub fn tile(&self, index: u32) -> &TileAtmosphere {
        &self.tiles[index as usize]
    }

    pub fn tile_mut(&mut self, index: u32) -> &mut TileAtmosphere {
        &mut self.tiles[index as usize]
    }

    /// Клетка по месту.
    pub fn tile_at(&self, coord: TileCoord) -> Option<&TileAtmosphere> {
        self.tile_index(coord).map(|i| self.tile(i))
    }

    /// Пометить клетку для пересчёта (`InvalidateTile`).
    pub fn invalidate(&mut self, coord: TileCoord) {
        if self.invalidated_set.insert(coord) {
            self.invalidated.push(coord);
        }
    }

    /// Клетка по месту; новая создаётся (`GetOrNewTile`).
    pub fn get_or_new(&mut self, coord: TileCoord, invalidate_new: bool) -> u32 {
        if let Some(index) = self.tile_index(coord) {
            return index;
        }
        if invalidate_new {
            self.invalidate(coord);
        }
        let index = self.tiles.len() as u32;
        self.tiles.push(TileAtmosphere::new(coord, None));
        self.index.insert(coord, index);
        index
    }

    /// Вставить клетку с воздухом (чтение карты).
    pub fn insert_tile(&mut self, coord: TileCoord, air: GasMixture) -> u32 {
        let index = self.get_or_new(coord, false);
        let tile = self.tile_mut(index);
        tile.air_archived = Some(air.clone());
        tile.air = Some(air);
        index
    }

    /// Убрать клетку из словаря (`Tiles.Remove`).
    pub fn remove_tile(&mut self, index: u32) {
        let coord = self.tile(index).coord;
        self.index.remove(&coord);
        self.tile_mut(index).removed = true;
    }

    /// Новая группа (`new ExcitedGroup()`).
    pub fn new_group(&mut self) -> u32 {
        let id = match self.free_groups.pop() {
            Some(id) => {
                let group = &mut self.excited_groups[id as usize];
                let generation = group.generation + 1;
                *group = ExcitedGroup {
                    generation,
                    ..ExcitedGroup::default()
                };
                id
            }
            None => {
                self.excited_groups.push(ExcitedGroup::default());
                self.excited_groups.len() as u32 - 1
            }
        };
        self.live_groups.insert(id);
        id
    }

    pub fn group(&self, id: u32) -> &ExcitedGroup {
        &self.excited_groups[id as usize]
    }

    pub fn group_mut(&mut self, id: u32) -> &mut ExcitedGroup {
        &mut self.excited_groups[id as usize]
    }

    /// Номер группы освобождается (`ExcitedGroupDispose`, вторая половина).
    pub(super) fn release_group(&mut self, id: u32) {
        self.live_groups.remove(id);
        self.free_groups.push(id);
    }

    /// Клетки с воздухом.
    pub fn live_tiles(&self) -> impl Iterator<Item = (u32, &TileAtmosphere)> {
        self.tiles
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.removed)
            .map(|(i, t)| (i as u32, t))
    }
}
