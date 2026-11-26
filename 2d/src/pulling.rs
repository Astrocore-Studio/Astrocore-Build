use astrocore_grid2d::Cell;
use astrocore_grid2d::session::PlayerId;

use crate::{Crew, MobState, Station};

const PULL_SPACING: f32 = 0.65;
const PULL_RANGE: f32 = 1.60;
pub(crate) const PULL_SPEED_RATIO: f32 = 0.95;

impl Station {
    pub(crate) fn start_pull(&mut self, player: PlayerId, target: Cell) {
        let actor = &self.state.players[&player];
        if actor.pulling.is_some()
            || actor
                .position
                .cell()
                .distance_on_level(target)
                .is_none_or(|distance| distance > 1)
            || self.state.doors.get(&target).is_some_and(|door| !door.open)
        {
            return;
        }
        let patient = self.state.players.iter().find_map(|(&id, patient)| {
            (id != player
                && patient.position.cell() == target
                && can_pull(actor, patient)
                && !self
                    .state
                    .players
                    .values()
                    .any(|crew| crew.pulling == Some(id)))
            .then_some(id)
        });
        if let Some(patient) = patient {
            self.state
                .players
                .get_mut(&player)
                .expect("игрок существует")
                .pulling = Some(patient);
        }
    }

    pub(crate) fn move_pulled(&mut self) {
        let movements: Vec<_> = self
            .state
            .players
            .values()
            .filter_map(|actor| {
                let id = actor.pulling?;
                let patient = self.state.players.get(&id)?;
                if !can_pull(actor, patient) {
                    return None;
                }
                let position = self.terrain.move_body_towards(
                    patient.position,
                    actor.position,
                    PULL_SPACING,
                    |cell| self.state.doors.get(&cell).is_some_and(|door| !door.open),
                );
                Some((id, position))
            })
            .collect();
        for (id, position) in movements {
            let patient = self.state.players.get_mut(&id).expect("пациент существует");
            if patient.position.cell() != position.cell() {
                patient.medical_reading = None;
                patient.pipe_scan = None;
            }
            patient.position = position;
        }
    }

    pub(crate) fn prune_pulls(&mut self) {
        let released: Vec<_> = self
            .state
            .players
            .iter()
            .filter_map(|(&player, actor)| {
                let target = actor.pulling?;
                self.state
                    .players
                    .get(&target)
                    .is_none_or(|patient| !can_pull(actor, patient))
                    .then_some(player)
            })
            .collect();
        for player in released {
            self.state
                .players
                .get_mut(&player)
                .expect("игрок существует")
                .pulling = None;
        }
    }

    pub(crate) fn change_deck(&mut self, player: PlayerId, up: bool) {
        let actor = &self.state.players[&player];
        let cell = actor.position.cell();
        let destination = self.layout.links.iter().find_map(|link| {
            if up && cell == link.lower {
                Some(link.upper)
            } else if !up && cell == link.upper {
                Some(link.lower)
            } else {
                None
            }
        });
        let Some(destination) = destination else {
            return;
        };
        let patient = actor.pulling.filter(|id| {
            self.state
                .players
                .get(id)
                .is_some_and(|patient| can_pull(actor, patient))
        });
        if patient.is_some_and(|id| !self.state.players[&id].position.overlaps(cell)) {
            return;
        }
        self.state
            .players
            .get_mut(&player)
            .expect("игрок существует")
            .pulling = patient;
        for id in std::iter::once(player).chain(patient) {
            let crew = self
                .state
                .players
                .get_mut(&id)
                .expect("участник перехода существует");
            crew.position = destination.center();
            crew.scan = None;
            crew.pipe_scan = None;
            crew.medical_reading = None;
        }
    }
}

fn can_pull(actor: &Crew, patient: &Crew) -> bool {
    actor.on_duty()
        && patient.on_duty()
        && actor.damage.state() == MobState::Alive
        && patient.damage.state() != MobState::Alive
        && actor.hands.contains(&None)
        && actor.position.z == patient.position.z
        && (actor.position.x - patient.position.x).hypot(actor.position.y - patient.position.y)
            <= PULL_RANGE
}
