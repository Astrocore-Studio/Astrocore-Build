use astrocore_grid2d::Cell;
use astrocore_grid2d::session::PlayerId;
use serde::{Deserialize, Serialize};

use crate::{Kind, Station};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipeDirection {
    North,
    #[default]
    East,
    South,
    West,
}

impl PipeDirection {
    pub const fn clockwise(self) -> Self {
        match self {
            Self::North => Self::East,
            Self::East => Self::South,
            Self::South => Self::West,
            Self::West => Self::North,
        }
    }

    pub const fn opposite(self) -> Self {
        self.clockwise().clockwise()
    }

    pub const fn offset(self) -> [i32; 2] {
        match self {
            Self::North => [0, 1],
            Self::East => [1, 0],
            Self::South => [0, -1],
            Self::West => [-1, 0],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipeShape {
    Straight,
    Bend,
    Tee,
    Cross,
    Valve,
    PassiveVent,
    Canister { pressure_kpa: u16 },
}

impl PipeShape {
    pub fn ports(self, facing: PipeDirection) -> impl Iterator<Item = PipeDirection> {
        let straight = [Some(facing), Some(facing.opposite()), None, None];
        match self {
            Self::Straight | Self::Valve => straight,
            Self::Bend => [Some(facing), Some(facing.clockwise()), None, None],
            Self::Tee => [straight[0], straight[1], Some(facing.clockwise()), None],
            Self::Cross => [
                straight[0],
                straight[1],
                Some(facing.clockwise()),
                Some(facing.opposite().clockwise()),
            ],
            Self::PassiveVent | Self::Canister { .. } => [Some(facing), None, None, None],
        }
        .into_iter()
        .flatten()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PipePiece {
    pub shape: PipeShape,
    pub facing: PipeDirection,
    pub starts_off: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeSample {
    pub cell: Cell,
    pub pressure_kpa: u16,
    pub opposite_kpa: Option<u16>,
}

impl Station {
    pub(crate) fn scan_pipe(&mut self, player: PlayerId, cell: Cell) {
        let crew = &self.state.players[&player];
        let Some(tool) = crew.hands[usize::from(crew.active_hand)] else {
            return;
        };
        let kind = self.layout.prototypes[&self.state.items[&tool].prototype].kind;
        if !matches!(kind, Kind::Item { analyzer: true, .. })
            || crew
                .position
                .cell()
                .distance_on_level(cell)
                .is_none_or(|distance| distance > 1)
            || self.state.doors.get(&cell).is_some_and(|door| !door.open)
        {
            return;
        }
        let Some(sample) = self.environment.sample_pipe(cell) else {
            return;
        };
        let crew = self
            .state
            .players
            .get_mut(&player)
            .expect("игрок существует");
        crew.scan = None;
        crew.medical_reading = None;
        crew.pipe_scan = Some(sample);
    }
}
