use astrocore_grid2d::Cell;
use astrocore_grid2d::session::PlayerId;
use atmos::Gas;
use serde::{Deserialize, Serialize};

use crate::{Kind, MobState, Phase, State, Station};

pub const MAX_FILTER_RATE_LPS: u16 = 1000;

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
    Canister {
        pressure_kpa: u16,
    },
    PressurePump {
        watts: u16,
        target_kpa: u16,
    },
    VentPump {
        watts: u16,
        target_kpa: u16,
    },
    Scrubber {
        watts: u16,
        rate_lps: u16,
        #[serde(default)]
        siphoning: bool,
    },
    Filter {
        rate_lps: u16,
        #[serde(default)]
        gas: Option<Gas>,
        #[serde(default)]
        mirrored: bool,
    },
}

impl PipeShape {
    pub fn ports(self, facing: PipeDirection) -> impl Iterator<Item = PipeDirection> {
        let straight = [Some(facing), Some(facing.opposite()), None, None];
        match self {
            Self::Straight | Self::Valve | Self::PressurePump { .. } => straight,
            Self::Bend => [Some(facing), Some(facing.clockwise()), None, None],
            Self::Tee => [straight[0], straight[1], Some(facing.clockwise()), None],
            Self::Filter { .. } => [straight[0], straight[1], self.filter_branch(facing), None],
            Self::Cross => [
                straight[0],
                straight[1],
                Some(facing.clockwise()),
                Some(facing.opposite().clockwise()),
            ],
            Self::PassiveVent
            | Self::Canister { .. }
            | Self::VentPump { .. }
            | Self::Scrubber { .. } => [Some(facing), None, None, None],
        }
        .into_iter()
        .flatten()
    }

    pub(crate) fn power_draw(self) -> Option<u16> {
        match self {
            Self::PressurePump { watts, .. }
            | Self::VentPump { watts, .. }
            | Self::Scrubber { watts, .. } => Some(watts),
            _ => None,
        }
    }

    pub(crate) fn filter_branch(self, facing: PipeDirection) -> Option<PipeDirection> {
        match self {
            Self::Filter {
                mirrored: false, ..
            } => Some(facing.clockwise()),
            Self::Filter { mirrored: true, .. } => Some(facing.opposite().clockwise()),
            _ => None,
        }
    }

    #[cfg(feature = "client")]
    pub(crate) fn has_switch(self) -> bool {
        matches!(self, Self::Valve | Self::Filter { .. }) || self.power_draw().is_some()
    }

    pub(crate) fn valid(self) -> bool {
        match self {
            Self::Canister { pressure_kpa } => (1..=10_132).contains(&pressure_kpa),
            Self::PressurePump { watts, target_kpa } | Self::VentPump { watts, target_kpa } => {
                watts > 0 && (1..=atmos::pipes::MAX_OUTPUT_PRESSURE as u16).contains(&target_kpa)
            }
            Self::Scrubber {
                watts, rate_lps, ..
            } => watts > 0 && (1..=atmos::pipes::MAX_TRANSFER_RATE as u16).contains(&rate_lps),
            Self::Filter { rate_lps, .. } => rate_lps <= MAX_FILTER_RATE_LPS,
            _ => true,
        }
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
    pub filter: Option<FilterSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterSample {
    pub branch_kpa: u16,
    pub gas: Option<Gas>,
    pub inlet_percent: u8,
    pub outlet_percent: u8,
    pub branch_percent: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterSettings {
    pub gas: Option<Gas>,
    pub rate_lps: u16,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum FilterControl {
    Toggle,
    SetGas(Option<Gas>),
    SetRate(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeMachine {
    pub enabled: bool,
    pub powered: bool,
    pub filter: Option<FilterSettings>,
}

impl PipeMachine {
    pub(crate) fn requires_power(&self) -> bool {
        self.filter.is_none()
    }

    pub(crate) fn from_piece(piece: PipePiece) -> Option<Self> {
        let filter = match piece.shape {
            PipeShape::Filter { rate_lps, gas, .. } => Some(FilterSettings { rate_lps, gas }),
            _ => None,
        };
        (filter.is_some() || piece.shape.power_draw().is_some()).then_some(Self {
            enabled: !piece.starts_off,
            powered: filter.is_some(),
            filter,
        })
    }
}

impl State {
    pub fn filter_in_reach(&self, player: PlayerId, cell: Cell) -> Option<&PipeMachine> {
        let crew = self.players.get(&player)?;
        if self.phase != Phase::Active
            || !crew.on_duty()
            || crew.damage.state() != MobState::Alive
            || crew
                .position
                .cell()
                .distance_on_level(cell)
                .is_none_or(|distance| distance > 1)
        {
            return None;
        }
        self.pipe_machines
            .get(&cell)
            .filter(|machine| machine.filter.is_some())
    }
}

impl Station {
    pub(crate) fn control_filter(&mut self, player: PlayerId, cell: Cell, control: FilterControl) {
        if self.state.filter_in_reach(player, cell).is_none() {
            return;
        }
        let machine = self
            .state
            .pipe_machines
            .get_mut(&cell)
            .expect("фильтр в зоне доступа");
        let settings = machine
            .filter
            .as_mut()
            .expect("настройки фильтра существуют");
        match control {
            FilterControl::Toggle => machine.enabled = !machine.enabled,
            FilterControl::SetGas(gas) => settings.gas = gas,
            FilterControl::SetRate(rate) => settings.rate_lps = rate.min(MAX_FILTER_RATE_LPS),
        }
        self.environment.set_pipe_machine(cell, *machine);
    }

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
        crew.storage_scan = None;
        crew.medical_reading = None;
        crew.pipe_scan = Some(sample);
    }
}
