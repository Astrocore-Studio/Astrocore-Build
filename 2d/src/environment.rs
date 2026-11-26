mod power_grid;

use std::collections::BTreeSet;

use astrocore_grid2d::session::TICK_HZ;
use astrocore_grid2d::{Cell, Grid, Tile};

use crate::content::{Layout, LevelLink};
use crate::{AirSample, Crew, State};
use power_grid::PowerGrid;

const DT: f32 = 1.0 / TICK_HZ as f32;
const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const MIN_BREATHABLE_OXYGEN_KPA: f32 = 16.0;
const SUFFOCATION_DAMAGE: u8 = 6;

pub(crate) struct Environment {
    workers: rayon::ThreadPool,
    air: atmos::Grid,
    hull: atmos::Layout,
    closed: BTreeSet<Cell>,
    power: PowerGrid,
    breathing_ticks: u32,
}

impl Environment {
    pub fn new(
        layout: &Layout,
        cables: &BTreeSet<Cell>,
        state: &mut State,
    ) -> Result<Self, String> {
        let workers = rayon::ThreadPoolBuilder::new()
            .thread_name(|index| format!("astrocore-sim-{index}"))
            .build()
            .map_err(|error| format!("не удалось запустить потоки симуляции: {error}"))?;
        let mut result = Self {
            workers,
            air: atmos::Grid::new(),
            hull: atmos::Layout::default(),
            closed: BTreeSet::new(),
            power: PowerGrid::new(cables),
            breathing_ticks: 0,
        };
        result.reset(layout, state);
        Ok(result)
    }

    pub fn reset(&mut self, layout: &Layout, state: &mut State) {
        self.air = atmos::Grid::new();
        self.hull = hull_layout(&layout.grid, &layout.links);
        self.apply_seals(closed_doors(state));
        self.breathing_ticks = 0;

        self.power.reset(layout, state);
    }

    pub fn tick(&mut self, state: &mut State) {
        let closed = closed_doors(state);
        if self.closed != closed {
            self.apply_seals(closed);
        }
        self.workers.in_place_scope(|scope| {
            let air = &mut self.air;
            scope.spawn(move |_| air.advance(DT));
            self.power.step(state);
        });
        self.feed_vents(state);
        self.breathing_ticks += 1;
        if self.breathing_ticks == TICK_HZ {
            self.breathing_ticks = 0;
            for crew in state.players.values_mut() {
                self.breathe(crew);
            }
        }
    }

    pub fn rebuild_hull(&mut self, grid: &Grid, links: &[LevelLink], state: &State) {
        let next = hull_layout(grid, links);
        let added: Vec<_> = next
            .cells
            .keys()
            .filter(|cell| !self.hull.cells.contains_key(cell))
            .copied()
            .collect();
        self.hull = next;
        self.apply_seals(closed_doors(state));
        // Укладка пола в космосе не создаёт воздух.
        for cell in added {
            self.air.with_air(cell, atmos::GasMixture::clear);
        }
    }

    fn apply_seals(&mut self, closed: BTreeSet<Cell>) {
        let mut layout = self.hull.clone();
        for &cell in &closed {
            for neighbor in neighbors(cell) {
                layout
                    .blocked
                    .insert(atmos::edge(coordinates(cell), coordinates(neighbor)));
            }
        }
        self.air.set_layout(layout);
        self.closed = closed;
    }

    fn feed_vents(&mut self, state: &mut State) {
        for (&cell, vent) in &mut state.vents {
            if !vent.enabled || !vent.powered || vent.reserve_moles <= 0.0 {
                continue;
            }
            let flow = self.power.vent_flow(cell);
            let mut source = atmos::GasMixture::standard_air();
            let room_pressure = self.air.pressure(coordinates(cell));
            let headroom =
                (1.0 - room_pressure / source.pressure()).max(0.0) * source.total_moles();
            let transfer = (f32::from(flow) * DT).min(vent.reserve_moles).min(headroom);
            if transfer > 0.0 {
                let air = source.remove(transfer);
                self.air.merge_into(coordinates(cell), &air);
                vent.reserve_moles -= air.total_moles();
            }
        }
    }

    pub fn pressure(&self, cell: Cell) -> (f32, bool) {
        let air = self.air.air(coordinates(cell));
        let pressure = air.pressure();
        let partial_oxygen =
            pressure * air.moles(atmos::Gas::Oxygen) / air.total_moles().max(f32::EPSILON);
        (pressure, partial_oxygen >= MIN_BREATHABLE_OXYGEN_KPA)
    }

    pub fn sample(&self, cell: Cell) -> AirSample {
        let air = self.air.air(coordinates(cell));
        let oxygen = 100.0 * air.moles(atmos::Gas::Oxygen) / air.total_moles().max(f32::EPSILON);
        AirSample {
            cell,
            pressure_kpa: air.pressure().round().clamp(0.0, f32::from(u16::MAX)) as u16,
            oxygen_percent: oxygen.round().clamp(0.0, 100.0) as u8,
            temperature_k: air.temperature().round().clamp(0.0, f32::from(u16::MAX)) as u16,
        }
    }

    fn breathe(&mut self, crew: &mut Crew) {
        let cell = coordinates(crew.position.cell());
        let (pressure, breathable) = self.pressure(crew.position.cell());
        crew.pressure_kpa = pressure.round().clamp(0.0, f32::from(u16::MAX)) as u16;
        if crew.health == 0 {
            return;
        }
        if !breathable {
            crew.health = crew.health.saturating_sub(SUFFOCATION_DAMAGE);
        } else {
            self.air.with_air(cell, |air| {
                let consumed = 0.012_f32.min(air.moles(atmos::Gas::Oxygen));
                air.adjust_moles(atmos::Gas::Oxygen, -consumed);
                air.adjust_moles(atmos::Gas::CarbonDioxide, consumed);
            });
        }
    }
}

fn hull_layout(grid: &Grid, links: &[LevelLink]) -> atmos::Layout {
    let mut hull = atmos::Layout::default();
    for (cell, tile) in grid.cells() {
        if tile == Tile::Floor {
            let point = coordinates(cell);
            hull.cells.insert(point, atmos::CellKind::Air);
            for neighbor in neighbors(cell) {
                if grid.tile(neighbor) == Tile::Wall {
                    hull.blocked
                        .insert(atmos::edge(point, coordinates(neighbor)));
                }
            }
        }
    }
    hull.links.extend(
        links
            .iter()
            .map(|link| (coordinates(link.lower), coordinates(link.upper))),
    );
    hull
}

fn closed_doors(state: &State) -> BTreeSet<Cell> {
    state
        .doors
        .iter()
        .filter_map(|(&cell, door)| (!door.open).then_some(cell))
        .collect()
}

pub(crate) fn coordinates(cell: Cell) -> (i32, i32) {
    atmos::floor_cell(i32::from(cell.z), cell.x, cell.y)
}

fn neighbors(cell: Cell) -> impl Iterator<Item = Cell> {
    SIDES
        .into_iter()
        .map(move |(x, y)| Cell::at(cell.x + x, cell.y + y, cell.z))
}
