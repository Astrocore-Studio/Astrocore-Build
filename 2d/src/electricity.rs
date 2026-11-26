use astrocore_grid2d::Cell;
use astrocore_grid2d::session::PlayerId;
use power::{CableGroup, Cables};
use serde::{Deserialize, Serialize};

use crate::environment::coordinates;
use crate::{Kind, PipeDirection, Station};

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

pub(crate) fn storage_ports(
    cables: &Cables,
    cell: Cell,
    facing: PipeDirection,
) -> (Option<usize>, Option<usize>) {
    let port = |direction: PipeDirection| {
        let [x, y] = direction.offset();
        let connector = Cell::at(cell.x + x, cell.y + y, cell.z);
        cables.net_at(CableGroup::Apc, coordinates(connector))
    };
    (port(facing.opposite()), port(facing))
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
