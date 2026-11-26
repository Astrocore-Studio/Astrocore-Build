use std::collections::BTreeMap;

use astrocore_grid2d::Cell;
use power::{Battery, CableGroup, Cables, Load, PowerState, Supply};

use super::{DT, coordinates};
use crate::{Kind, Layout, State, StorageSample};

#[derive(Default)]
pub(super) struct PowerGrid {
    solver: PowerState,
    generators: BTreeMap<Cell, usize>,
    storages: BTreeMap<Cell, (usize, bool)>,
    doors: BTreeMap<Cell, usize>,
    id_consoles: BTreeMap<Cell, usize>,
    vents: BTreeMap<Cell, (usize, u16)>,
    pipe_machines: BTreeMap<Cell, usize>,
}

impl PowerGrid {
    pub(super) fn rewire(&mut self, layout: &Layout, state: &State) {
        let cables = Cables::build(state.cables.keys().map(|segment| segment.network_key()));
        let mut solver = PowerState::default();
        let previous_generators = std::mem::take(&mut self.generators);
        let previous_storages = std::mem::take(&mut self.storages);
        self.doors.clear();
        self.id_consoles.clear();
        self.vents.clear();
        self.pipe_machines.clear();
        for _ in 0..cables.net_count() {
            solver.add_network();
        }
        for (&cell, generator) in &state.generators {
            if let Kind::Generator { watts, voltage } = layout.prototypes[&generator.prototype].kind
                && let Some(net) = cables.net_at(voltage, coordinates(cell))
            {
                let supply = previous_generators.get(&cell).map_or_else(
                    || Supply::new(f32::from(watts)),
                    |&id| self.solver.supplies[id].clone(),
                );
                self.generators.insert(cell, solver.add_supply(net, supply));
            }
        }
        for (&cell, storage) in &state.storages {
            let Kind::EnergyStorage {
                capacity_joules,
                initial_joules,
                charge_watts,
                supply_watts,
                ports,
            } = layout.prototypes[&storage.prototype].kind
            else {
                continue;
            };
            let previous = previous_storages
                .get(&cell)
                .map(|&(id, _)| &self.solver.batteries[id]);
            let stored = previous.map_or(initial_joules as f32, |battery| battery.current_storage);
            let mut battery = Battery::new(capacity_joules as f32, stored);
            battery.max_charge_rate = charge_watts as f32;
            battery.max_supply = supply_watts as f32;
            battery.enabled = storage.enabled;
            if let Some(previous) = previous {
                battery.supply_ramp_position = previous.supply_ramp_position;
            }
            let (input, output) = ports.nets(&cables, cell, storage.facing);
            let bypassed = input.is_some() && input == output;
            let id = if bypassed {
                solver.add_battery_ports(None, None, battery)
            } else {
                solver.add_battery_ports(input, output, battery)
            };
            self.storages.insert(cell, (id, bypassed));
        }
        for (&cell, door) in &state.doors {
            if let Some(net) = cables.net_at(CableGroup::Apc, coordinates(cell))
                && let Kind::Door { watts } = layout.prototypes[&door.prototype].kind
            {
                let load = solver.add_load(net, Load::new(f32::from(watts)));
                self.doors.insert(cell, load);
            }
        }
        for (&cell, console) in &state.id_consoles {
            if let Some(net) = cables.net_at(CableGroup::Apc, coordinates(cell))
                && let Kind::IdConsole { watts } = layout.prototypes[&console.prototype].kind
            {
                self.id_consoles
                    .insert(cell, solver.add_load(net, Load::new(f32::from(watts))));
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
        for &cell in state.pipe_machines.keys() {
            if let Some(net) = cables.net_at(CableGroup::Apc, coordinates(cell))
                && let Some(watts) = layout.pipes[&cell].shape.power_draw()
            {
                self.pipe_machines
                    .insert(cell, solver.add_load(net, Load::new(f32::from(watts))));
            }
        }
        self.solver = solver;
    }

    pub(super) fn step(&mut self, state: &mut State) {
        for (&cell, &id) in &self.generators {
            self.solver.supplies[id].enabled = state.generators[&cell].enabled;
        }
        for (&cell, &(id, _)) in &self.storages {
            self.solver.batteries[id].enabled = state.storages[&cell].enabled;
        }
        self.solver.tick(DT);
        for (&cell, door) in &mut state.doors {
            door.powered = self
                .doors
                .get(&cell)
                .is_some_and(|&id| self.load_powered(id));
        }
        for (&cell, console) in &mut state.id_consoles {
            console.powered = self
                .id_consoles
                .get(&cell)
                .is_some_and(|&id| self.load_powered(id));
        }
        for (&cell, vent) in &mut state.vents {
            vent.powered = self
                .vents
                .get(&cell)
                .is_some_and(|&(id, _)| self.load_powered(id));
        }
        for (&cell, machine) in &mut state.pipe_machines {
            machine.powered = !machine.requires_power()
                || self
                    .pipe_machines
                    .get(&cell)
                    .is_some_and(|&id| self.load_powered(id));
        }
    }

    pub(super) fn vent_flow(&self, cell: Cell) -> u16 {
        self.vents[&cell].1
    }

    pub(super) fn sample_storage(&self, cell: Cell, capacity: u32) -> Option<StorageSample> {
        let &(id, bypassed) = self.storages.get(&cell)?;
        let battery = &self.solver.batteries[id];
        Some(StorageSample {
            cell,
            joules: (battery.current_storage.round() as u32).min(capacity),
            capacity,
            input_watts: battery
                .charging
                .map(|_| battery.current_receiving.round() as u32),
            output_watts: battery
                .discharging
                .map(|_| battery.current_supply.round() as u32),
            bypassed,
        })
    }

    fn load_powered(&self, id: usize) -> bool {
        let load = &self.solver.loads[id];
        load.receiving_power >= load.desired_power
    }
}
