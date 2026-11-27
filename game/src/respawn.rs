//! Возрождение мёртвых по правилу раунда: игрок из списка правила после смерти через
//! задержку (или сразу) снова входит в раунд на станции, старое тело удаляется. Образец —
//! `RespawnRuleSystem`, `RespawnDeadRuleComponent`, `RespawnTrackerComponent` SS14 @ 2f4a2aad.
//! Команда администратора `respawn` — в `game_ticker`.
//!
//! Нет пока: самоубийства (`SuicideEvent`) — его система ещё не перенесена.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use astrocore_ecs::{
    EventWorldExt, GameTime, GameWorldExt, LifecycleWorldExt, RegistryWorldExt, Update,
};
use astrocore_loc::LocArg;
use astrocore_net::{Actor, NetUserId, Session};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::chat::ChatChannel;
use crate::examine::loc;
use crate::game_ticker::{ActiveGameRule, GameRule};
use crate::mobs::{MobState, MobStateChanged};

/// Правило: мёртвые возрождаются (`RespawnDeadRuleComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RespawnDeadRule {
    /// Возрождать любого умершего, а не только из списка.
    pub always_respawn_dead: bool,
}

/// Кого и когда возродить (`RespawnTrackerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RespawnTracker {
    #[serde(skip)]
    pub players: BTreeSet<NetUserId>,
    /// Задержка, секунды.
    pub respawn_delay: f64,
    #[serde(skip)]
    pub respawn_queue: BTreeMap<NetUserId, Duration>,
    pub delete_body: bool,
}

impl Default for RespawnTracker {
    fn default() -> Self {
        Self {
            players: BTreeSet::new(),
            respawn_delay: 0.0,
            respawn_queue: BTreeMap::new(),
            delete_body: true,
        }
    }
}

fn first_station(world: &mut World) -> Option<Entity> {
    crate::stations::stations(world).into_iter().next()
}

/// Добавить игрока в список (`AddToTracker`).
pub fn add_to_tracker(world: &mut World, user: NetUserId, tracker: Entity) {
    if let Some(mut component) = world.get_mut::<RespawnTracker>(tracker) {
        component.players.insert(user);
    }
}

/// Возродить игрока (`RespawnPlayer`): сразу или в очередь. `true` — поставлен в очередь.
pub fn respawn_player(world: &mut World, body: Entity, session: Entity, tracker: Entity) -> bool {
    let Some(user) = world.get::<Session>(session).map(|s| s.user_id) else {
        return false;
    };
    let Some(state) = world.get::<RespawnTracker>(tracker).cloned() else {
        return false;
    };
    if !state.players.contains(&user) || state.respawn_queue.contains_key(&user) {
        return false;
    }
    if state.respawn_delay <= 0.0 {
        let Some(station) = first_station(world) else {
            return false;
        };
        if state.delete_body {
            world.queue_delete(body);
        }
        crate::game_ticker::make_join_game(world, session, Some(station), None);
        return false;
    }
    let text = loc(
        world,
        "rule-respawn-in-seconds",
        &[("second", LocArg::from(state.respawn_delay))],
    );
    let wrapped = loc(
        world,
        "chat-manager-server-wrap-message",
        &[("message", LocArg::from(text.clone()))],
    );
    crate::chat::chat_message_to_one(
        world,
        ChatChannel::SERVER,
        &text,
        &wrapped,
        None,
        false,
        session,
        None,
    );
    let at = world.resource::<GameTime>().cur_time() + Duration::from_secs_f64(state.respawn_delay);
    if let Some(mut component) = world.get_mut::<RespawnTracker>(tracker) {
        component.respawn_queue.insert(user, at);
    }
    true
}

/// Смерть игрока (`OnMobStateChanged`).
fn on_mob_state_changed(world: &mut World, event: &mut MobStateChanged) {
    if event.new_state != MobState::Dead {
        return;
    }
    let body = event.target;
    let Some(session) = world.get::<Actor>(body).map(|a| a.session) else {
        return;
    };
    let Some(user) = world.get::<Session>(session).map(|s| s.user_id) else {
        return;
    };
    let rules: Vec<(Entity, bool)> = world
        .query_filtered::<(Entity, &RespawnDeadRule), (
            bevy_ecs::query::With<RespawnTracker>,
            bevy_ecs::query::With<GameRule>,
            bevy_ecs::query::With<ActiveGameRule>,
        )>()
        .iter(world)
        .map(|(e, r)| (e, r.always_respawn_dead))
        .collect();
    for (rule, always) in rules {
        if always {
            add_to_tracker(world, user, rule);
        }
        if respawn_player(world, body, session, rule) {
            break;
        }
    }
}

/// Очередь возрождения (`Update`).
fn update(world: &mut World) {
    let Some(station) = first_station(world) else {
        return;
    };
    let now = world.resource::<GameTime>().cur_time();
    let trackers: Vec<(Entity, Vec<NetUserId>)> = world
        .query::<(Entity, &RespawnTracker)>()
        .iter(world)
        .map(|(e, t)| {
            let due = t
                .respawn_queue
                .iter()
                .filter(|(_, time)| **time <= now)
                .map(|(user, _)| *user)
                .collect();
            (e, due)
        })
        .collect();
    for (tracker, due) in trackers {
        for user in due {
            let Some(session) = crate::mind::session_of_user(world, user) else {
                continue;
            };
            if let Some(mind) = crate::mind::mind_of_user(world, user)
                && let Some(owned) = world
                    .get::<crate::mind::Mind>(mind)
                    .and_then(|m| m.owned_entity)
            {
                world.queue_delete(owned);
            }
            crate::game_ticker::make_join_game(world, session, Some(station), None);
            if let Some(mut component) = world.get_mut::<RespawnTracker>(tracker) {
                component.respawn_queue.remove(&user);
            }
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<RespawnDeadRule>();
    world.register_data_component::<RespawnTracker>();
    if !side.is_server() {
        return;
    }
    world.subscribe_broadcast::<MobStateChanged>(on_mob_state_changed);
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
