use astrocore_grid2d::Cell;
use atmos::pipes::{CANISTER_VOLUME, DEFAULT_PIPE_VOLUME, EAST, NORTH, SOUTH, WEST};
use atmos::{Canister, Device, GasMixture, NodeSpec, Scrubber, VentPump};

use super::{Environment, coordinates};
use crate::{Layout, PipeDirection, PipeSample, PipeShape, State};

pub(super) struct PipeNodes {
    first: u64,
    opposite: Option<u64>,
}

impl Environment {
    pub(super) fn install_piping(&mut self, layout: &Layout, state: &State) {
        self.pipe_nodes.clear();
        let mut specs = Vec::new();
        for (index, (&cell, piece)) in layout.pipes.iter().enumerate() {
            let first = index as u64 * 2;
            let opposite = matches!(
                piece.shape,
                PipeShape::Valve | PipeShape::PressurePump { .. }
            )
            .then_some(first + 1);
            let coordinates = coordinates(cell);
            let device = if let Some(second) = opposite {
                for (key, facing) in [(first, piece.facing), (second, piece.facing.opposite())] {
                    specs.push(NodeSpec {
                        key,
                        cell: coordinates,
                        layer: 0,
                        dirs: mask(facing),
                        volume: DEFAULT_PIPE_VOLUME / 2.0,
                    });
                }
                match piece.shape {
                    PipeShape::Valve => Some(Device::Valve {
                        a: first,
                        b: second,
                        open: state.pipe_valves[&cell],
                    }),
                    PipeShape::PressurePump { target_kpa, .. } => Some(Device::PressurePump {
                        inlet: second,
                        outlet: first,
                        target: f32::from(target_kpa),
                        enabled: state.pipe_machines[&cell].enabled,
                    }),
                    _ => unreachable!("у детали один порт"),
                }
            } else {
                let dirs = piece
                    .shape
                    .ports(piece.facing)
                    .fold(0, |dirs, port| dirs | mask(port));
                specs.push(NodeSpec {
                    key: first,
                    cell: coordinates,
                    layer: 0,
                    dirs,
                    volume: DEFAULT_PIPE_VOLUME,
                });
                match piece.shape {
                    PipeShape::PassiveVent => Some(Device::PassiveVent { node: first }),
                    PipeShape::Canister { pressure_kpa } => {
                        Some(connected_canister(first, pressure_kpa))
                    }
                    PipeShape::VentPump { target_kpa, .. } => {
                        let mut vent = VentPump::new(first);
                        vent.external_bound = f32::from(target_kpa);
                        vent.enabled = state.pipe_machines[&cell].enabled;
                        Some(Device::VentPump(vent))
                    }
                    PipeShape::Scrubber {
                        rate_lps,
                        siphoning,
                        ..
                    } => {
                        let mut scrubber = Scrubber::new(first);
                        scrubber.transfer_rate = f32::from(rate_lps);
                        scrubber.siphoning = siphoning;
                        scrubber.enabled = state.pipe_machines[&cell].enabled;
                        Some(Device::Scrubber(scrubber))
                    }
                    _ => None,
                }
            };
            if let Some(device) = device {
                self.air.pipes_mut().add_device(first, coordinates, device);
            }
            self.pipe_nodes.insert(cell, PipeNodes { first, opposite });
        }
        self.air.set_pipe_nodes(&specs);
    }

    pub(crate) fn set_pipe_valve(&mut self, cell: Cell, enabled: bool) {
        let Some(nodes) = self.pipe_nodes.get(&cell) else {
            return;
        };
        if let Some(Device::Valve { open, .. }) = self.air.pipes_mut().device_mut(nodes.first) {
            *open = enabled;
        }
    }

    pub(crate) fn set_pipe_machine(&mut self, cell: Cell, enabled: bool) {
        let Some(nodes) = self.pipe_nodes.get(&cell) else {
            return;
        };
        match self.air.pipes_mut().device_mut(nodes.first) {
            Some(Device::PressurePump {
                enabled: current, ..
            }) => *current = enabled,
            Some(Device::VentPump(vent)) => vent.enabled = enabled,
            Some(Device::Scrubber(scrubber)) => scrubber.enabled = enabled,
            _ => {}
        }
    }

    pub(super) fn sync_pipe_power(&mut self, state: &State) {
        for (&cell, machine) in &state.pipe_machines {
            self.air
                .pipes_mut()
                .set_powered(self.pipe_nodes[&cell].first, machine.powered);
        }
    }

    pub(crate) fn sample_pipe(&self, cell: Cell) -> Option<PipeSample> {
        let nodes = self.pipe_nodes.get(&cell)?;
        let pressure = |key| {
            self.air
                .pipes()
                .air_of(key)
                .map(|air| air.pressure().round().clamp(0.0, f32::from(u16::MAX)) as u16)
        };
        Some(PipeSample {
            cell,
            pressure_kpa: pressure(nodes.first)?,
            opposite_kpa: nodes.opposite.and_then(pressure),
        })
    }
}

fn connected_canister(port: u64, pressure_kpa: u16) -> Device {
    let mut air = GasMixture::standard_air();
    let ratio = f32::from(pressure_kpa) / air.pressure() * CANISTER_VOLUME / air.volume;
    air.multiply(ratio);
    air.volume = CANISTER_VOLUME;
    let mut canister = Canister::new(air);
    canister.port = Some(port);
    Device::Canister(Box::new(canister))
}

fn mask(direction: PipeDirection) -> u8 {
    // Координата Y 2D-карты растёт вверх, а в решателе Север уменьшает вторую координату.
    match direction {
        PipeDirection::North => SOUTH,
        PipeDirection::East => EAST,
        PipeDirection::South => NORTH,
        PipeDirection::West => WEST,
    }
}
