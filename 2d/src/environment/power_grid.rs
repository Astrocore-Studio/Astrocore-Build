use std::collections::{BTreeMap, BTreeSet};

use astrocore_grid2d::Cell;
use power::{CableGroup, Cables, Load, PowerState, Supply};

use super::{DT, coordinates};
use crate::{Kind, Layout, State};

pub(super) struct PowerGrid {
    solver: PowerState,
    cables: Cables,
    generators: BTreeMap<Cell, usize>,
    doors: BTreeMap<Cell, usize>,
    vents: BTreeMap<Cell, (usize, u16)>,
}

impl PowerGrid {
    pub(super) fn new(cables: &BTreeSet<Cell>) -> Self {
        Self {
            solver: PowerState::default(),
            cables: Cables::build(
                cables
                    .iter()
                    .map(|&cell| (CableGroup::Apc, coordinates(cell))),
            ),
            generators: BTreeMap::new(),
            doors: BTreeMap::new(),
            vents: BTreeMap::new(),
        }
    }

    pub(super) fn reset(&mut self, layout: &Layout, state: &mut State) {
        self.solver = PowerState::default();
        self.generators.clear();
        self.doors.clear();
        self.vents.clear();
        for _ in 0..self.cables.net_count() {
            self.solver.add_network();
        }
        for (&cell, generator) in &state.generators {
            if let Some(net) = self.cables.net_at(CableGroup::Apc, coordinates(cell))
                && let Kind::Generator { watts } = layout.prototypes[&generator.prototype].kind
            {
                let supply = self.solver.add_supply(net, Supply::new(f32::from(watts)));
                self.generators.insert(cell, supply);
            }
        }
        for (&cell, door) in &state.doors {
            if let Some(net) = self.cables.net_at(CableGroup::Apc, coordinates(cell))
                && let Kind::Door { watts } = layout.prototypes[&door.prototype].kind
            {
                let load = self.solver.add_load(net, Load::new(f32::from(watts)));
                self.doors.insert(cell, load);
            }
        }
        for (&cell, vent) in &state.vents {
            if let Some(net) = self.cables.net_at(CableGroup::Apc, coordinates(cell))
                && let Kind::Vent {
                    watts, flow_moles, ..
                } = layout.prototypes[&vent.prototype].kind
            {
                let load = self.solver.add_load(net, Load::new(f32::from(watts)));
                self.vents.insert(cell, (load, flow_moles));
            }
        }
        self.step(state);
    }

    pub(super) fn step(&mut self, state: &mut State) {
        for (&cell, &id) in &self.generators {
            self.solver.supplies[id].enabled = state.generators[&cell].enabled;
        }
        self.solver.tick(DT);
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

    pub(super) fn vent_flow(&self, cell: Cell) -> u16 {
        self.vents[&cell].1
    }

    fn load_powered(&self, id: usize) -> bool {
        let load = &self.solver.loads[id];
        load.receiving_power >= load.desired_power
    }
}
