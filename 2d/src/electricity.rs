use astrocore_grid2d::Cell;
use astrocore_grid2d::session::PlayerId;
use power::{CableGroup, Cables};
use serde::{Deserialize, Serialize};

use crate::environment::coordinates;
use crate::{Kind, PipeDirection, Station};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CableSegment {
    pub cell: Cell,
    pub voltage: CableGroup,
}

impl CableSegment {
    pub(crate) fn network_key(self) -> (CableGroup, power::cables::Cell) {
        (self.voltage, coordinates(self.cell))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoragePorts {
    pub input: CableGroup,
    pub output: CableGroup,
}

impl StoragePorts {
    pub fn opposite_sides(self) -> bool {
        self.input == self.output
    }

    pub(crate) fn nets(
        self,
        cables: &Cables,
        cell: Cell,
        facing: PipeDirection,
    ) -> (Option<usize>, Option<usize>) {
        let port = |direction: PipeDirection| {
            let [x, y] = direction.offset();
            Cell::at(cell.x + x, cell.y + y, cell.z)
        };
        let (input, output) = if self.opposite_sides() {
            (port(facing.opposite()), port(facing))
        } else {
            (cell, cell)
        };
        (
            cables.net_at(self.input, coordinates(input)),
            cables.net_at(self.output, coordinates(output)),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Storage {
    pub prototype: String,
    pub facing: PipeDirection,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageSample {
    pub cell: Cell,
    pub joules: u32,
    pub capacity: u32,
    pub input_watts: Option<u32>,
    pub output_watts: Option<u32>,
    pub bypassed: bool,
}

impl Station {
    pub(crate) fn inspect_storage(&mut self, player: PlayerId, cell: Cell) {
        let crew = &self.state.players[&player];
        if crew
            .position
            .cell()
            .distance_on_level(cell)
            .is_none_or(|distance| distance > 1)
        {
            return;
        }
        let Some(storage) = self.state.storages.get(&cell) else {
            return;
        };
        let Kind::EnergyStorage {
            capacity_joules, ..
        } = self.layout.prototypes[&storage.prototype].kind
        else {
            return;
        };
        let Some(sample) = self.environment.sample_storage(cell, capacity_joules) else {
            return;
        };
        let crew = self
            .state
            .players
            .get_mut(&player)
            .expect("игрок существует");
        crew.scan = None;
        crew.pipe_scan = None;
        crew.medical_reading = None;
        crew.storage_scan = Some(sample);
    }
}
