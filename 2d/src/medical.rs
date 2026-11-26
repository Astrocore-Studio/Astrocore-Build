use astrocore_grid2d::session::{PlayerId, TICK_HZ};
use astrocore_grid2d::{Cell, Position};
use serde::{Deserialize, Serialize};

use crate::{Kind, Station};

const CRITICAL_AT: u16 = 100;
const DEAD_AT: u16 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobState {
    Alive,
    Critical,
    Dead,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Damage {
    #[serde(default)]
    pub blunt: u16,
    #[serde(default)]
    pub asphyxiation: u16,
}

impl Damage {
    pub fn total(self) -> u16 {
        self.blunt.saturating_add(self.asphyxiation)
    }

    pub fn state(self) -> MobState {
        match self.total() {
            DEAD_AT.. => MobState::Dead,
            CRITICAL_AT.. => MobState::Critical,
            _ => MobState::Alive,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MedicalProgress {
    pub scanning: bool,
    pub ticks_left: u16,
}

#[derive(Clone, Copy)]
enum Procedure {
    Scan,
    FirstAid(u8),
    Restore(Damage),
}

impl Procedure {
    fn supports(self, damage: Damage) -> bool {
        if matches!(self, Self::Scan) {
            return true;
        }
        damage.state() != MobState::Dead && self.heal(damage) != damage
    }

    fn heal(self, damage: Damage) -> Damage {
        match self {
            Self::Scan => damage,
            Self::FirstAid(amount) => {
                let amount = u16::from(amount);
                let blunt = amount.min(damage.blunt);
                Damage {
                    blunt: damage.blunt - blunt,
                    asphyxiation: damage.asphyxiation.saturating_sub(amount - blunt),
                }
            }
            Self::Restore(restores) => Damage {
                blunt: damage.blunt.saturating_sub(restores.blunt),
                asphyxiation: damage.asphyxiation.saturating_sub(restores.asphyxiation),
            },
        }
    }
}

pub(crate) struct PendingMedical {
    patient: PlayerId,
    tool: u16,
    actor_position: Position,
    patient_position: Position,
    procedure: Procedure,
}

impl Station {
    pub(crate) fn start_medical(
        &mut self,
        player: PlayerId,
        target: Option<Cell>,
        tool: u16,
        kind: Kind,
    ) {
        let (procedure, delay) = match kind {
            Kind::Item {
                healing: healing @ 1..,
                ..
            } => (Procedure::FirstAid(healing), 1),
            Kind::Medicine {
                restores,
                delay_seconds,
            } => (Procedure::Restore(restores), delay_seconds),
            Kind::HealthAnalyzer => (Procedure::Scan, 1),
            _ => return,
        };
        if self.pending_medical.contains_key(&player) {
            return;
        }
        let actor = &self.state.players[&player];
        let patient = if let Some(target) = target {
            if !actor
                .position
                .cell()
                .distance_on_level(target)
                .is_some_and(|distance| distance <= 1)
                || self.state.doors.get(&target).is_some_and(|door| !door.open)
            {
                return;
            }
            let Some((&patient, _)) = self.state.players.iter().find(|(id, crew)| {
                **id != player
                    && crew.on_duty()
                    && crew.position.cell() == target
                    && procedure.supports(crew.damage)
            }) else {
                return;
            };
            patient
        } else {
            player
        };
        let patient_crew = &self.state.players[&patient];
        if !procedure.supports(patient_crew.damage) {
            return;
        }
        let scanning = matches!(procedure, Procedure::Scan);
        let self_penalty = if patient == player && !scanning { 2 } else { 1 };
        let task = PendingMedical {
            patient,
            tool,
            actor_position: actor.position,
            patient_position: patient_crew.position,
            procedure,
        };
        let actor = self
            .state
            .players
            .get_mut(&player)
            .expect("игрок существует");
        actor.scan = None;
        actor.medical_reading = None;
        actor.medical_progress = Some(MedicalProgress {
            scanning,
            ticks_left: u16::from(delay) * self_penalty * TICK_HZ as u16,
        });
        self.pending_medical.insert(player, task);
    }

    pub(crate) fn advance_medical(&mut self) {
        let mut pending = std::mem::take(&mut self.pending_medical);
        pending.retain(|&player, task| {
            let Some(actor) = self.state.players.get_mut(&player) else {
                return false;
            };
            let hand = usize::from(actor.active_hand);
            let valid_actor = actor.damage.state() == MobState::Alive
                && actor.position == task.actor_position
                && actor.hands[hand] == Some(task.tool);
            let Some(progress) = actor.medical_progress.as_mut() else {
                return false;
            };
            progress.ticks_left -= 1;
            let finished = progress.ticks_left == 0;
            let target_cell = task.patient_position.cell();
            let patient_damage = self
                .state
                .players
                .get(&task.patient)
                .filter(|patient| {
                    patient.position == task.patient_position
                        && task.procedure.supports(patient.damage)
                        && self
                            .state
                            .doors
                            .get(&target_cell)
                            .is_none_or(|door| door.open)
                })
                .map(|patient| patient.damage);
            if valid_actor && patient_damage.is_some() && !finished {
                return true;
            }
            let actor = self
                .state
                .players
                .get_mut(&player)
                .expect("игрок существует");
            actor.medical_progress = None;
            if let (true, Some(damage)) = (valid_actor, patient_damage) {
                if matches!(task.procedure, Procedure::Scan) {
                    actor.medical_reading = Some(damage);
                } else {
                    actor.hands[hand] = None;
                    self.state.items.remove(&task.tool);
                    self.state
                        .players
                        .get_mut(&task.patient)
                        .expect("пациент существует")
                        .damage = task.procedure.heal(damage);
                }
            }
            false
        });
        self.pending_medical = pending;
    }
}
