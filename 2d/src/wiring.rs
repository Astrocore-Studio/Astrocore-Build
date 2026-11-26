use astrocore_grid2d::session::{PlayerId, TICK_HZ};
use astrocore_grid2d::{Cell, Position, Tile};

use crate::content::MAX_CABLES;
use crate::{CableGroup, CableSegment, Item, Kind, MAX_ROUND_ITEMS, MobState, Station};

pub(crate) struct PendingCut {
    cable: CableSegment,
    revision: u32,
    tool: u16,
    position: Position,
}

impl Station {
    pub(crate) fn lay_cable(&mut self, player: PlayerId, cell: Cell) {
        let crew = &self.state.players[&player];
        let hand = usize::from(crew.active_hand);
        let Some(id) = crew.hands[hand] else { return };
        let item = &self.state.items[&id];
        let Kind::CableCoil { voltage, .. } = self.layout.prototypes[&item.prototype].kind else {
            return;
        };
        let cable = CableSegment { cell, voltage };
        if item.materials == 0
            || self.state.cables.len() >= MAX_CABLES
            || self.state.cables.contains_key(&cable)
            || !self.can_reach_cable(crew.position, cell)
        {
            return;
        }
        self.cable_revision += 1;
        self.state.cables.insert(cable, self.cable_revision);
        if item.materials == 1 {
            self.state.items.remove(&id);
            self.state
                .players
                .get_mut(&player)
                .expect("игрок существует")
                .hands[hand] = None;
        } else {
            self.state
                .items
                .get_mut(&id)
                .expect("моток существует")
                .materials -= 1;
        }
    }

    pub(crate) fn start_cut(&mut self, player: PlayerId, cable: CableSegment) {
        let crew = &self.state.players[&player];
        let Some(tool) = crew.hands[usize::from(crew.active_hand)] else {
            return;
        };
        let Some(&revision) = self.state.cables.get(&cable) else {
            return;
        };
        let prototype = &self.state.items[&tool].prototype;
        if self.layout.prototypes[prototype].kind != Kind::Wirecutters
            || self.pending_cuts.contains_key(&player)
            || self.cable_coil_prototype(cable.voltage).is_none()
            || !self.can_reach_cable(crew.position, cable.cell)
        {
            return;
        }
        self.pending_cuts.insert(
            player,
            PendingCut {
                cable,
                revision,
                tool,
                position: crew.position,
            },
        );
        self.state
            .players
            .get_mut(&player)
            .expect("игрок существует")
            .cutting_cable = Some(TICK_HZ as u8);
    }

    pub(crate) fn advance_cable_cuts(&mut self) {
        let mut pending = std::mem::take(&mut self.pending_cuts);
        pending.retain(|&player, cut| {
            let crew = &self.state.players[&player];
            let valid = crew.position == cut.position
                && crew.damage.state() == MobState::Alive
                && crew.hands[usize::from(crew.active_hand)] == Some(cut.tool)
                && self.state.cables.get(&cut.cable) == Some(&cut.revision)
                && self.can_reach_cable(crew.position, cut.cable.cell);
            let next = crew.cutting_cable.and_then(|ticks| ticks.checked_sub(1));
            let remaining = next.filter(|&ticks| ticks > 0 && valid);
            self.state
                .players
                .get_mut(&player)
                .expect("игрок существует")
                .cutting_cable = remaining;
            if valid && next == Some(0) {
                self.finish_cut(cut.cable);
            }
            remaining.is_some()
        });
        self.pending_cuts = pending;
    }

    fn finish_cut(&mut self, cable: CableSegment) {
        if self.state.items.len() >= MAX_ROUND_ITEMS {
            return;
        }
        let Some(next) = self.next_item.checked_add(1) else {
            return;
        };
        let Some(prototype) = self.cable_coil_prototype(cable.voltage).cloned() else {
            return;
        };
        self.state.items.insert(
            self.next_item,
            Item {
                prototype,
                ground: Some(cable.cell),
                materials: 1,
                contents: Vec::new(),
                identity: None,
            },
        );
        self.next_item = next;
        self.state.cables.remove(&cable);
        self.cable_revision += 1;
    }

    fn can_reach_cable(&self, position: Position, cell: Cell) -> bool {
        position
            .cell()
            .distance_on_level(cell)
            .is_some_and(|distance| distance <= 1)
            && self.terrain.tile(cell) == Tile::Floor
            && self.state.doors.get(&cell).is_none_or(|door| door.open)
    }

    fn cable_coil_prototype(&self, voltage: CableGroup) -> Option<&String> {
        self.layout.prototypes.iter().find_map(|(id, prototype)| {
            matches!(prototype.kind, Kind::CableCoil { voltage: coil, .. } if coil == voltage)
                .then_some(id)
        })
    }
}
