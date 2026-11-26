use std::collections::BTreeSet;

use astrocore_grid2d::Cell;
use astrocore_grid2d::session::{PlayerId, TICK_HZ};
use serde::{Deserialize, Serialize};

use crate::{Crew, Kind, State, Station};

pub(crate) const MAX_ACCESS_TAGS: usize = 32;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessReader {
    #[serde(default)]
    pub access: Vec<Vec<String>>,
    #[serde(default)]
    pub deny: Vec<String>,
}

impl AccessReader {
    pub(crate) fn valid(&self) -> bool {
        self.access.len() <= 32
            && self.access.iter().all(|group| valid_tags(group))
            && valid_tags(&self.deny)
    }

    pub(crate) fn allows(&self, tags: &BTreeSet<&str>) -> bool {
        !self.deny.iter().any(|tag| tags.contains(tag.as_str()))
            && (self.access.is_empty()
                || self
                    .access
                    .iter()
                    .any(|group| group.iter().all(|tag| tags.contains(tag.as_str()))))
    }
}

pub(crate) fn valid_tags(tags: &[String]) -> bool {
    tags.len() <= MAX_ACCESS_TAGS
        && tags
            .iter()
            .all(|tag| !tag.is_empty() && tag.len() <= 48 && tag.trim() == tag)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JobCredential {
    pub(crate) prototype: String,
    #[serde(default)]
    pub(crate) access: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityCard {
    pub assignment: String,
    pub access: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DoorFailure {
    NoPower,
    AccessDenied,
    Obstructed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoorNotice {
    pub cell: Cell,
    pub failure: DoorFailure,
    ticks_left: u8,
}

impl DoorNotice {
    pub(crate) fn advance(self, here: Cell) -> Option<Self> {
        (self.ticks_left > 1
            && here
                .distance_on_level(self.cell)
                .is_some_and(|distance| distance <= 1))
        .then(|| Self {
            ticks_left: self.ticks_left - 1,
            ..self
        })
    }
}

impl State {
    fn credential_tags<'a>(&'a self, crew: &Crew) -> BTreeSet<&'a str> {
        crew.hands
            .into_iter()
            .flatten()
            .chain(crew.id_card)
            .filter_map(|id| self.items.get(&id)?.identity.as_ref())
            .flat_map(|card| card.access.iter().map(String::as_str))
            .collect()
    }
}

impl Station {
    pub(crate) fn toggle_door(&mut self, player: PlayerId, cell: Cell) {
        let door = &self.state.doors[&cell];
        let crew = &self.state.players[&player];
        let failure = if !door.powered {
            Some(DoorFailure::NoPower)
        } else if self.layout.prototypes[&door.prototype]
            .access_reader
            .as_ref()
            .is_some_and(|reader| !reader.allows(&self.state.credential_tags(crew)))
        {
            Some(DoorFailure::AccessDenied)
        } else if door.open
            && self
                .state
                .players
                .values()
                .any(|crew| crew.on_duty() && crew.position.overlaps(cell))
        {
            Some(DoorFailure::Obstructed)
        } else {
            None
        };
        let crew = self
            .state
            .players
            .get_mut(&player)
            .expect("игрок существует");
        crew.door_notice = failure.map(|failure| DoorNotice {
            cell,
            failure,
            ticks_left: 2 * TICK_HZ as u8,
        });
        if failure.is_none() {
            let door = self
                .state
                .doors
                .get_mut(&cell)
                .expect("устройство является шлюзом");
            door.open = !door.open;
        }
    }

    pub(crate) fn toggle_id_card(&mut self, player: PlayerId) {
        let crew = self
            .state
            .players
            .get_mut(&player)
            .expect("игрок существует");
        let hand = &mut crew.hands[usize::from(crew.active_hand)];
        if hand.is_none()
            || hand.is_some_and(|id| {
                self.layout.prototypes[&self.state.items[&id].prototype].kind == Kind::IdCard
            }) && crew.id_card.is_none()
        {
            std::mem::swap(hand, &mut crew.id_card);
        }
    }
}
