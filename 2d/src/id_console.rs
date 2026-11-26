use std::collections::BTreeSet;

use astrocore_grid2d::Cell;
use astrocore_grid2d::session::PlayerId;
use serde::{Deserialize, Serialize};

use crate::access::MAX_ACCESS_TAGS;
use crate::{IdentityCard, Kind, Layout, MobState, Phase, State, Station};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CardSlot {
    Authority,
    Target,
}

impl CardSlot {
    pub const fn index(self) -> usize {
        match self {
            Self::Authority => 0,
            Self::Target => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum IdConsoleControl {
    Transfer {
        slot: CardSlot,
        expected_card: Option<u16>,
        expected_hand: Option<u16>,
    },
    SetAccess {
        authority: u16,
        target: u16,
        level: u8,
        granted: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdConsole {
    pub prototype: String,
    pub powered: bool,
    pub cards: [Option<u16>; 2],
}

impl State {
    pub fn id_console_in_reach(&self, player: PlayerId, cell: Cell) -> Option<&IdConsole> {
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
        self.id_consoles.get(&cell)
    }

    pub fn console_authority(&self, layout: &Layout, console: &IdConsole) -> Option<&[String]> {
        let item = self
            .items
            .get(&console.cards[CardSlot::Authority.index()]?)?;
        let access = item
            .identity
            .as_ref()
            .map_or(&[][..], |card| card.access.as_slice());
        let reader = &layout.prototypes[&console.prototype].access_reader;
        reader
            .as_ref()
            .is_none_or(|reader| {
                let tags: BTreeSet<_> = access.iter().map(String::as_str).collect();
                reader.allows(&tags)
            })
            .then_some(access)
    }
}

impl Station {
    pub(crate) fn control_id_console(
        &mut self,
        player: PlayerId,
        cell: Cell,
        control: IdConsoleControl,
    ) {
        let Some(console) = self.state.id_console_in_reach(player, cell) else {
            return;
        };
        match control {
            IdConsoleControl::Transfer {
                slot,
                expected_card,
                expected_hand,
            } => {
                let crew = &self.state.players[&player];
                let hand = usize::from(crew.active_hand);
                if console.cards[slot.index()] != expected_card
                    || crew.hands[hand] != expected_hand
                    || expected_card.is_some() && expected_hand.is_some()
                    || expected_hand.is_some_and(|id| {
                        self.layout.prototypes[&self.state.items[&id].prototype].kind
                            != Kind::IdCard
                    })
                {
                    return;
                }
                self.state
                    .players
                    .get_mut(&player)
                    .expect("игрок существует")
                    .hands[hand] = expected_card;
                self.state
                    .id_consoles
                    .get_mut(&cell)
                    .expect("консоль существует")
                    .cards[slot.index()] = expected_hand;
            }
            IdConsoleControl::SetAccess {
                authority,
                target,
                level,
                granted,
            } => {
                if !console.powered
                    || authority == target
                    || console.cards != [Some(authority), Some(target)]
                {
                    return;
                }
                let Some(tag) = self.layout.prototypes[&console.prototype]
                    .access_levels
                    .get(usize::from(level))
                else {
                    return;
                };
                if !self
                    .state
                    .console_authority(&self.layout, console)
                    .is_some_and(|access| access.contains(tag))
                {
                    return;
                }
                let item = self
                    .state
                    .items
                    .get_mut(&target)
                    .expect("целевая карта существует");
                if granted {
                    let card = item.identity.get_or_insert_with(|| IdentityCard {
                        assignment: String::new(),
                        access: Vec::new(),
                    });
                    if card.access.len() < MAX_ACCESS_TAGS && !card.access.contains(tag) {
                        card.access.push(tag.clone());
                    }
                } else if let Some(card) = &mut item.identity {
                    card.access.retain(|access| access != tag);
                }
            }
        }
    }
}
