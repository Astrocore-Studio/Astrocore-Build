use astrocore_grid2d::session::PlayerId;
use serde::{Deserialize, Serialize};

use crate::{Crew, Job, Phase, State, Station};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Participation {
    Choosing,
    Ready,
    Observing,
    InRound,
}

impl Crew {
    pub fn on_duty(&self) -> bool {
        self.participation == Participation::InRound
    }
}

impl State {
    pub fn remaining_slots(&self, role: usize, job: &Job) -> Option<u16> {
        let limit = job.slots?;
        let used = self
            .players
            .values()
            .filter(|crew| {
                usize::from(crew.role) == role
                    && matches!(
                        crew.participation,
                        Participation::Ready | Participation::InRound
                    )
            })
            .count();
        Some(usize::from(limit).saturating_sub(used) as u16)
    }
}

impl Station {
    pub(crate) fn request_join(&mut self, player: PlayerId) {
        let crew = &self.state.players[&player];
        if crew.participation == Participation::Ready {
            self.state
                .players
                .get_mut(&player)
                .expect("игрок существует")
                .participation = Participation::Choosing;
            return;
        }
        let role = usize::from(crew.role);
        if self.state.remaining_slots(role, &self.layout.roles[role]) == Some(0) {
            return;
        }
        match self.state.phase {
            Phase::Lobby => {
                self.state
                    .players
                    .get_mut(&player)
                    .expect("игрок существует")
                    .participation = Participation::Ready;
            }
            Phase::Active => self.spawn_for_job(player),
            Phase::Debrief => {}
        }
    }

    pub(crate) fn spawn_for_job(&mut self, player: PlayerId) {
        let role = self.state.players[&player].role;
        let preferred = &self.layout.roles[usize::from(role)].spawns;
        let spawns = &self.layout.spawns;
        let common = spawns.iter().filter(|cell| {
            self.layout
                .roles
                .iter()
                .all(|job| !job.spawns.contains(cell))
        });
        let position = preferred
            .iter()
            .chain(common)
            .chain(spawns)
            .find(|&&cell| {
                self.state
                    .players
                    .values()
                    .filter(|crew| crew.on_duty())
                    .all(|crew| !crew.position.overlaps(cell))
            })
            .or_else(|| preferred.first())
            .unwrap_or(&spawns[0])
            .center();
        let mut crew = Crew::new(position);
        crew.role = role;
        crew.participation = Participation::InRound;
        self.state.players.insert(player, crew);
        self.equip(player);
    }
}
