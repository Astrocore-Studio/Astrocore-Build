use std::collections::BTreeMap;

use astrocore_grid2d::Cell;
use power::{CableGroup, Cables, Load, PowerState, Supply};

use super::{DT, coordinates};
use crate::{Kind, Layout, State};

#[derive(Default)]
pub(super) struct PowerGrid {
    solver: PowerState,
    generators: BTreeMap<Cell, usize>,
    doors: BTreeMap<Cell, usize>,
    vents: BTreeMap<Cell, (usize, u16)>,
}

impl PowerGrid {
    pub(super) fn rewire(&mut self, layout: &Layout, state: &State) {
        let cables = Cables::build(
            state
                .cables
                .keys()
                .map(|&cell| (CableGroup::Apc, coordinates(cell))),
        );
        let mut solver = PowerState::default();
        let previous_generators = std::mem::take(&mut self.generators);
        self.doors.clear();
        self.vents.clear();
        for _ in 0..cables.net_count() {
            solver.add_network();
        }
        for (&cell, generator) in &state.generators {
            if let Some(net) = cables.net_at(CableGroup::Apc, coordinates(cell))
                && let Kind::Generator { watts } = layout.prototypes[&generator.prototype].kind
            {
                let supply = previous_generators.get(&cell).map_or_else(
                    || Supply::new(f32::from(watts)),
                    |&id| self.solver.supplies[id].clone(),
                );
                self.generators.insert(cell, solver.add_supply(net, supply));
            }
        }
        for (&cell, door) in &state.doors {
            if let Some(net) = cables.net_at(CableGroup::Apc, coordinates(cell))
                && let Kind::Door { watts } = layout.prototypes[&door.prototype].kind
            {
                let load = solver.add_load(net, Load::new(f32::from(watts)));
                self.doors.insert(cell, load);
            }
        }
        for (&cell, vent) in &state.vents {
            if let Some(net) = cables.net_at(CableGroup::Apc, coordinates(cell))
                && let Kind::Vent {
                    watts, flow_moles, ..
                } = layout.prototypes[&vent.prototype].kind
            {
                let load = solver.add_load(net, Load::new(f32::from(watts)));
                self.vents.insert(cell, (load, flow_moles));
            }
        }
        self.solver = solver;
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
