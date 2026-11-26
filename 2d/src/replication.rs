use std::collections::BTreeMap;

use astrocore_grid2d::session::{PlayerId, ReplicatedState};
use astrocore_grid2d::{Cell, Tile};
use serde::{Deserialize, Serialize};

use crate::{Crew, Door, Generator, Item, Outcome, Phase, Recovery, State, Vent};

#[derive(Serialize, Deserialize)]
pub struct StateDelta {
    players: MapDelta<PlayerId, Crew>,
    doors: MapDelta<Cell, Door>,
    generators: MapDelta<Cell, Generator>,
    vents: MapDelta<Cell, Vent>,
    items: MapDelta<u16, Item>,
    terrain: MapDelta<Cell, Tile>,
    terrain_revision: u32,
    phase: Phase,
    round: u32,
    seconds_left: u32,
    recovery: Option<Recovery>,
    outcome: Option<Outcome>,
}

#[derive(Serialize, Deserialize)]
struct MapDelta<Key, Value> {
    changed: Vec<(Key, Value)>,
    removed: Vec<Key>,
}

impl<Key: Ord + Copy, Value: Clone + PartialEq> MapDelta<Key, Value> {
    fn diff(current: &BTreeMap<Key, Value>, baseline: &BTreeMap<Key, Value>) -> Self {
        let changed = current
            .iter()
            .filter(|&(key, value)| baseline.get(key) != Some(value))
            .map(|(&key, value)| (key, value.clone()))
            .collect();
        let removed = baseline
            .keys()
            .filter(|key| !current.contains_key(key))
            .copied()
            .collect();
        Self { changed, removed }
    }

    fn apply(self, current: &mut BTreeMap<Key, Value>) {
        for key in self.removed {
            current.remove(&key);
        }
        current.extend(self.changed);
    }
}

impl ReplicatedState for State {
    type Delta = StateDelta;

    fn diff(&self, baseline: &Self) -> StateDelta {
        StateDelta {
            players: MapDelta::diff(&self.players, &baseline.players),
            doors: MapDelta::diff(&self.doors, &baseline.doors),
            generators: MapDelta::diff(&self.generators, &baseline.generators),
            vents: MapDelta::diff(&self.vents, &baseline.vents),
            items: MapDelta::diff(&self.items, &baseline.items),
            terrain: MapDelta::diff(&self.terrain, &baseline.terrain),
            terrain_revision: self.terrain_revision,
            phase: self.phase,
            round: self.round,
            seconds_left: self.seconds_left,
            recovery: self.recovery.clone(),
            outcome: self.outcome,
        }
    }

    fn apply(&mut self, delta: StateDelta) {
        delta.players.apply(&mut self.players);
        delta.doors.apply(&mut self.doors);
        delta.generators.apply(&mut self.generators);
        delta.vents.apply(&mut self.vents);
        delta.items.apply(&mut self.items);
        delta.terrain.apply(&mut self.terrain);
        self.terrain_revision = delta.terrain_revision;
        self.phase = delta.phase;
        self.round = delta.round;
        self.seconds_left = delta.seconds_left;
        self.recovery = delta.recovery;
        self.outcome = delta.outcome;
    }
}
