//! Состояние существа: живо, в критическом состоянии или мертво, — и пороги урона, которые
//! его меняют. Образец — `Content.Shared/Mobs` SS14 @ 2f4a2aad (`MobStateComponent`,
//! `MobThresholdsComponent`, `MobStateSystem`, `MobThresholdSystem`).

use std::collections::{BTreeMap, BTreeSet};

use astrocore_ecs::{ComponentInit, EventWorldExt, RegistryWorldExt};
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::action_blocker::{
    ConsciousAttempt, DropAttempt, PickupAttempt, ThrowAttempt, UpdateCanMove, UseAttempt,
    update_can_move,
};
use crate::damage::{DamageChanged, Damageable};
use crate::fixed_point::FixedPoint2;
use crate::inventory::{EquipAttempt, UnequipAttempt};
use crate::standing::{StandAttempt, StandingWorldExt};

/// Ключ внешнего вида: состояние (`MobStateVisuals.State`).
pub const VISUALS_STATE: &str = "enum.MobStateVisuals.State";

/// Состояние (`MobState`); порядок — от лучшего к худшему.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum MobState {
    Invalid,
    #[default]
    Alive,
    Critical,
    Dead,
}

impl MobState {
    pub fn name(self) -> &'static str {
        match self {
            MobState::Invalid => "Invalid",
            MobState::Alive => "Alive",
            MobState::Critical => "Critical",
            MobState::Dead => "Dead",
        }
    }
}

fn all_states() -> BTreeSet<MobState> {
    [MobState::Alive, MobState::Critical, MobState::Dead]
        .into_iter()
        .collect()
}

/// Текущее состояние. В данных — компонент `MobState`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobStateComponent {
    #[serde(default)]
    pub current_state: MobState,
    #[serde(default = "all_states")]
    pub allowed_states: BTreeSet<MobState>,
}

impl Default for MobStateComponent {
    fn default() -> Self {
        Self {
            current_state: MobState::Alive,
            allowed_states: all_states(),
        }
    }
}

fn yes() -> bool {
    true
}

fn alerts() -> BTreeMap<MobState, String> {
    [
        (MobState::Alive, "HumanHealth"),
        (MobState::Critical, "HumanCrit"),
        (MobState::Dead, "HumanDead"),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.to_owned()))
    .collect()
}

fn health_category() -> String {
    "Health".into()
}

/// Пороги урона по состояниям. В данных — компонент `MobThresholds`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobThresholds {
    #[serde(default)]
    pub thresholds: BTreeMap<FixedPoint2, MobState>,
    #[serde(default = "yes")]
    pub triggers_alerts: bool,
    #[serde(default = "alerts")]
    pub state_alert_dict: BTreeMap<MobState, String>,
    #[serde(default = "health_category")]
    pub health_alert_category: String,
    #[serde(default = "yes")]
    pub show_overlays: bool,
    #[serde(default)]
    pub allow_revives: bool,
}

impl Default for MobThresholds {
    fn default() -> Self {
        Self {
            thresholds: BTreeMap::new(),
            triggers_alerts: true,
            state_alert_dict: alerts(),
            health_alert_category: health_category(),
            show_overlays: true,
            allow_revives: false,
        }
    }
}

/// Состояние сменилось (`MobStateChangedEvent`). На сущности и широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct MobStateChanged {
    pub target: Entity,
    pub old_state: MobState,
    pub new_state: MobState,
    pub origin: Option<Entity>,
}

/// Состояния на `World`.
pub trait MobStateWorldExt {
    fn mob_state(&self, entity: Entity) -> Option<MobState>;
    fn is_mob_alive(&self, entity: Entity) -> bool;
    fn is_mob_critical(&self, entity: Entity) -> bool;
    fn is_mob_dead(&self, entity: Entity) -> bool;
    /// В критическом состоянии или мертво (`IsIncapacitated`).
    fn is_incapacitated(&self, entity: Entity) -> bool;
    /// Сменить состояние, если оно разрешено (`ChangeMobState`).
    fn change_mob_state(&mut self, entity: Entity, state: MobState, origin: Option<Entity>);
    /// Порог урона состояния (`TryGetThresholdForState`).
    fn threshold_for_state(&self, entity: Entity, state: MobState) -> Option<FixedPoint2>;
    /// Порог, с которого существо не может действовать: крит, иначе смерть.
    fn incap_threshold(&self, entity: Entity) -> Option<FixedPoint2>;
    /// Задать порог состояния (`SetMobStateThreshold`).
    fn set_mob_state_threshold(&mut self, entity: Entity, damage: FixedPoint2, state: MobState);
    /// Проверить пороги заново (`VerifyThresholds`).
    fn verify_thresholds(&mut self, entity: Entity);
}

impl MobStateWorldExt for World {
    fn mob_state(&self, entity: Entity) -> Option<MobState> {
        self.get::<MobStateComponent>(entity)
            .map(|m| m.current_state)
    }

    fn is_mob_alive(&self, entity: Entity) -> bool {
        self.mob_state(entity) == Some(MobState::Alive)
    }

    fn is_mob_critical(&self, entity: Entity) -> bool {
        self.mob_state(entity) == Some(MobState::Critical)
    }

    fn is_mob_dead(&self, entity: Entity) -> bool {
        self.mob_state(entity) == Some(MobState::Dead)
    }

    fn is_incapacitated(&self, entity: Entity) -> bool {
        matches!(
            self.mob_state(entity),
            Some(MobState::Critical | MobState::Dead)
        )
    }

    fn change_mob_state(&mut self, entity: Entity, state: MobState, origin: Option<Entity>) {
        let Some(component) = self.get::<MobStateComponent>(entity) else {
            return;
        };
        let old = component.current_state;
        if old == state || !component.allowed_states.contains(&state) {
            return;
        }
        on_exit(self, entity, old);
        if let Some(mut component) = self.get_mut::<MobStateComponent>(entity) {
            component.current_state = state;
        }
        on_enter(self, entity, state);
        let mut event = MobStateChanged {
            target: entity,
            old_state: old,
            new_state: state,
            origin,
        };
        self.raise_and_broadcast(entity, &mut event);
    }

    fn threshold_for_state(&self, entity: Entity, state: MobState) -> Option<FixedPoint2> {
        self.get::<MobThresholds>(entity)?
            .thresholds
            .iter()
            .find(|(_, s)| **s == state)
            .map(|(damage, _)| *damage)
    }

    fn incap_threshold(&self, entity: Entity) -> Option<FixedPoint2> {
        self.threshold_for_state(entity, MobState::Critical)
            .or_else(|| self.threshold_for_state(entity, MobState::Dead))
    }

    fn set_mob_state_threshold(&mut self, entity: Entity, damage: FixedPoint2, state: MobState) {
        let Some(mut thresholds) = self.get_mut::<MobThresholds>(entity) else {
            return;
        };
        thresholds.thresholds.retain(|_, s| *s != state);
        thresholds.thresholds.insert(damage, state);
        self.verify_thresholds(entity);
    }

    fn verify_thresholds(&mut self, entity: Entity) {
        check_thresholds(self, entity, None);
    }
}

/// Разрешить или запретить оживление мёртвого порогами (`SetAllowRevives`).
pub fn set_allow_revives(world: &mut World, entity: Entity, allow: bool) {
    let Some(mut thresholds) = world.get_mut::<MobThresholds>(entity) else {
        return;
    };
    thresholds.allow_revives = allow;
    world.verify_thresholds(entity);
}

/// Наибольший пройденный порог задаёт состояние; мёртвого возвращает только разрешённое
/// оживление (`CheckThresholds`, `TriggerThreshold`).
fn check_thresholds(world: &mut World, entity: Entity, origin: Option<Entity>) {
    let (Some(thresholds), Some(state), Some(total)) = (
        world.get::<MobThresholds>(entity),
        world.mob_state(entity),
        world.get::<Damageable>(entity).map(|d| d.damage.total()),
    ) else {
        return;
    };
    let reached = thresholds
        .thresholds
        .iter()
        .rev()
        .find(|(threshold, _)| total >= **threshold)
        .map(|(_, state)| *state);
    let allow_revives = thresholds.allow_revives;
    if let Some(new_state) = reached
        && new_state != state
        && (state != MobState::Dead || allow_revives)
    {
        world.change_mob_state(entity, new_state, origin);
    }
}

fn on_exit(world: &mut World, entity: Entity, state: MobState) {
    if matches!(state, MobState::Critical | MobState::Dead) {
        world.stand(entity, false);
    }
}

fn on_enter(world: &mut World, entity: Entity, state: MobState) {
    update_can_move(world, entity);
    match state {
        MobState::Alive => {
            world.stand(entity, false);
        }
        MobState::Critical | MobState::Dead => {
            world.down(entity, true, false);
            world.raise(entity, &mut crate::standing::DropHandItems);
        }
        MobState::Invalid => return,
    }
    world.set_appearance(entity, VISUALS_STATE, state.name().to_owned());
}

/// Следующее, худшее состояние по порогам (`TryGetNextState`).
pub fn next_state(world: &World, entity: Entity, state: MobState) -> Option<MobState> {
    world
        .get::<MobThresholds>(entity)?
        .thresholds
        .values()
        .copied()
        .filter(|candidate| *candidate > state)
        .min()
}

/// Значок здоровья по состоянию (`UpdateAlerts`): у живого — уровень по доле урона до
/// следующего порога.
fn update_alerts(world: &mut World, entity: Entity) {
    let (Some(thresholds), Some(state)) = (
        world.get::<MobThresholds>(entity).cloned(),
        world.mob_state(entity),
    ) else {
        return;
    };
    if !thresholds.triggers_alerts {
        return;
    }
    let Some(alert_type) = thresholds.state_alert_dict.get(&state).cloned() else {
        eprintln!("нет значка здоровья для состояния {}", state.name());
        return;
    };
    let Some(alert) = world
        .get_resource::<astrocore_prototypes::Prototypes>()
        .and_then(|p| crate::alerts::alert_prototype(p, &alert_type))
    else {
        return;
    };
    if !alert.supports_severity() {
        crate::alerts::show_alert(world, entity, &alert_type, None);
        return;
    }
    let mut severity = alert.min_severity();
    let total = world
        .get::<Damageable>(entity)
        .map(|d| d.damage.total())
        .unwrap_or_default();
    if let Some(next) = next_state(world, entity, state)
        && let Some(threshold) = world.threshold_for_state(entity, next)
        && threshold > FixedPoint2::default()
    {
        let percentage = (total.float() / threshold.float()).clamp(0.0, 1.0);
        let (min, max) = (
            f32::from(alert.min_severity()),
            f32::from(alert.max_severity),
        );
        severity = (min + (max - min) * percentage).round() as i16;
    }
    crate::alerts::show_alert(world, entity, &alert_type, Some(severity));
}

/// Крит и смерть отменяют действие (`CheckAct`).
fn blocks(world: &World, entity: Entity) -> bool {
    world.is_incapacitated(entity)
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component_as::<MobStateComponent>("MobState");
    world.register_data_component::<MobThresholds>();
    world.register_net_serde::<MobStateComponent>();
    world.register_net_serde::<MobThresholds>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<MobStateComponent, ConsciousAttempt>(|world, entity, event| {
        event.cancelled |= blocks(world, entity);
    });
    world.subscribe::<MobStateComponent, UseAttempt>(|world, entity, event| {
        event.cancelled |= blocks(world, entity);
    });
    world.subscribe::<MobStateComponent, ThrowAttempt>(|world, entity, event| {
        event.cancelled |= event.user == entity && blocks(world, entity);
    });
    world.subscribe::<MobStateComponent, DropAttempt>(|world, entity, event| {
        event.cancelled |= blocks(world, entity);
    });
    world.subscribe::<MobStateComponent, PickupAttempt>(|world, entity, event| {
        event.cancelled |= event.user == entity && blocks(world, entity);
    });
    world.subscribe::<MobStateComponent, crate::action_blocker::AttackAttempt>(
        |world, entity, event| {
            event.cancelled |= blocks(world, entity);
        },
    );
    world.subscribe::<MobStateComponent, UpdateCanMove>(|world, entity, event| {
        event.cancelled |= blocks(world, entity);
    });
    world.subscribe::<MobStateComponent, StandAttempt>(|world, entity, event| {
        event.cancelled |= blocks(world, entity);
    });
    world.subscribe::<MobStateComponent, EquipAttempt>(|world, entity, event| {
        if event.user == entity && blocks(world, entity) {
            event.cancelled = true;
        }
    });
    world.subscribe::<MobStateComponent, UnequipAttempt>(|world, entity, event| {
        if event.user == entity && blocks(world, entity) {
            event.cancelled = true;
        }
    });
    world.subscribe::<MobThresholds, DamageChanged>(|world, entity, event| {
        check_thresholds(world, entity, event.origin);
        update_alerts(world, entity);
    });
    world.subscribe::<MobThresholds, ComponentInit>(|world, entity, _| {
        check_thresholds(world, entity, None);
        update_alerts(world, entity);
    });
    world.subscribe::<MobThresholds, MobStateChanged>(|world, entity, _| {
        update_alerts(world, entity);
    });
}

#[cfg(test)]
mod tests;
