use std::collections::{BTreeMap, BTreeSet};

use astrocore_grid2d::session::TICK_HZ;
use astrocore_grid2d::{Cell, Grid, Tile};

use crate::content::{Kind, Layout};
use crate::{AirSample, Crew, State};

const DT: f32 = 1.0 / TICK_HZ as f32;
const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const MIN_BREATHABLE_OXYGEN_KPA: f32 = 16.0;
const SUFFOCATION_DAMAGE: u8 = 6;

pub(crate) struct Environment {
    air: atmos::Grid,
    hull: atmos::Layout,
    closed: BTreeSet<Cell>,
    power: power::PowerState,
    cables: power::Cables,
    generators: BTreeMap<Cell, usize>,
    doors: BTreeMap<Cell, usize>,
    vents: BTreeMap<Cell, (usize, u16)>,
    breathing_ticks: u32,
}

impl Environment {
    pub fn new(layout: &Layout, cables: &BTreeSet<Cell>, state: &mut State) -> Self {
        let mut result = Self {
            air: atmos::Grid::new(),
            hull: atmos::Layout::default(),
            closed: BTreeSet::new(),
            power: power::PowerState::default(),
            cables: power::Cables::build(
                cables
                    .iter()
                    .map(|&cell| (power::CableGroup::Apc, coordinates(cell))),
            ),
            generators: BTreeMap::new(),
            doors: BTreeMap::new(),
            vents: BTreeMap::new(),
            breathing_ticks: 0,
        };
        result.reset(layout, state);
        result
    }

    pub fn reset(&mut self, layout: &Layout, state: &mut State) {
        self.air = atmos::Grid::new();
        self.hull = hull_layout(&layout.grid);
        self.apply_seals(closed_doors(state));
        self.breathing_ticks = 0;

        self.power = power::PowerState::default();
        self.generators.clear();
        self.doors.clear();
        self.vents.clear();
        for _ in 0..self.cables.net_count() {
            self.power.add_network();
        }
        for (&cell, generator) in &state.generators {
            if let Some(net) = self
                .cables
                .net_at(power::CableGroup::Apc, coordinates(cell))
                && let Kind::Generator { watts } = layout.prototypes[&generator.prototype].kind
            {
                let supply = self
                    .power
                    .add_supply(net, power::Supply::new(f32::from(watts)));
                self.generators.insert(cell, supply);
            }
        }
        for (&cell, door) in &state.doors {
            if let Some(net) = self
                .cables
                .net_at(power::CableGroup::Apc, coordinates(cell))
                && let Kind::Door { watts } = layout.prototypes[&door.prototype].kind
            {
                let load = self.power.add_load(net, power::Load::new(f32::from(watts)));
                self.doors.insert(cell, load);
            }
        }
        for (&cell, vent) in &state.vents {
            if let Some(net) = self
                .cables
                .net_at(power::CableGroup::Apc, coordinates(cell))
                && let Kind::Vent {
                    watts, flow_moles, ..
                } = layout.prototypes[&vent.prototype].kind
            {
                let load = self.power.add_load(net, power::Load::new(f32::from(watts)));
                self.vents.insert(cell, (load, flow_moles));
            }
        }
        self.update_power(state);
    }

    pub fn tick(&mut self, state: &mut State) {
        let closed = closed_doors(state);
        if self.closed != closed {
            self.apply_seals(closed);
        }
        self.air.advance(DT);
        self.update_power(state);
        self.feed_vents(state);
        self.breathing_ticks += 1;
        if self.breathing_ticks == TICK_HZ {
            self.breathing_ticks = 0;
            for crew in state.players.values_mut() {
                self.breathe(crew);
            }
        }
    }

    pub fn rebuild_hull(&mut self, grid: &Grid, state: &State) {
        let next = hull_layout(grid);
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

    fn update_power(&mut self, state: &mut State) {
        for (&cell, &id) in &self.generators {
            self.power.supplies[id].enabled = state.generators[&cell].enabled;
        }
        self.power.tick(DT);
        for (&cell, door) in &mut state.doors {
            door.powered = self
                .doors
                .get(&cell)
                .is_some_and(|&id| self.load_powered(id));
        }
        for (&cell, vent) in &mut state.vents {
            vent.powered = self
                .vents
                .get(&cell)
                .is_some_and(|&(id, _)| self.load_powered(id));
        }
    }

    fn load_powered(&self, id: usize) -> bool {
        let load = &self.power.loads[id];
        load.receiving_power >= load.desired_power
    }

    fn feed_vents(&mut self, state: &mut State) {
        for (&cell, vent) in &mut state.vents {
            if !vent.enabled || !vent.powered || vent.reserve_moles <= 0.0 {
                continue;
            }
            let (_, flow) = self.vents[&cell];
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

fn hull_layout(grid: &Grid) -> atmos::Layout {
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
    hull
}

fn closed_doors(state: &State) -> BTreeSet<Cell> {
    state
        .doors
        .iter()
        .filter_map(|(&cell, door)| (!door.open).then_some(cell))
        .collect()
}

fn coordinates(cell: Cell) -> (i32, i32) {
    (cell.x, cell.y)
}

fn neighbors(cell: Cell) -> impl Iterator<Item = Cell> {
    SIDES
        .into_iter()
        .map(move |(x, y)| Cell::new(cell.x + x, cell.y + y))
}
