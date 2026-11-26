use std::collections::BTreeMap;

use astrocore_grid2d::session::{PlayerId, ReplicatedState};
use astrocore_grid2d::{Cell, Tile};
use serde::{Deserialize, Serialize};

use crate::{
    CableSegment, Crew, Door, Generator, Item, Outcome, Phase, PipeMachine, Recovery, State,
    Storage, Vent,
};

#[derive(Serialize, Deserialize)]
pub struct StateDelta {
    players: MapDelta<PlayerId, Crew>,
    doors: MapDelta<Cell, Door>,
    generators: MapDelta<Cell, Generator>,
    storages: MapDelta<Cell, Storage>,
    vents: MapDelta<Cell, Vent>,
    pipe_valves: MapDelta<Cell, bool>,
    pipe_machines: MapDelta<Cell, PipeMachine>,
    items: MapDelta<u16, Item>,
    cables: MapDelta<CableSegment, u32>,
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
            storages: MapDelta::diff(&self.storages, &baseline.storages),
            vents: MapDelta::diff(&self.vents, &baseline.vents),
            pipe_valves: MapDelta::diff(&self.pipe_valves, &baseline.pipe_valves),
            pipe_machines: MapDelta::diff(&self.pipe_machines, &baseline.pipe_machines),
            items: MapDelta::diff(&self.items, &baseline.items),
            cables: MapDelta::diff(&self.cables, &baseline.cables),
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
        delta.storages.apply(&mut self.storages);
        delta.vents.apply(&mut self.vents);
        delta.pipe_valves.apply(&mut self.pipe_valves);
        delta.pipe_machines.apply(&mut self.pipe_machines);
        delta.items.apply(&mut self.items);
        delta.cables.apply(&mut self.cables);
        delta.terrain.apply(&mut self.terrain);
        self.terrain_revision = delta.terrain_revision;
        self.phase = delta.phase;
        self.round = delta.round;
        self.seconds_left = delta.seconds_left;
        self.recovery = delta.recovery;
        self.outcome = delta.outcome;
    }
}
