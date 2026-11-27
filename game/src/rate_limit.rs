//! Ограничение частоты действий игрока: за период — не больше заданного числа действий по
//! ключу (чат, ahelp, взаимодействия). Превысивший узнаёт об этом один раз за период,
//! запись уходит в журнал администратора, администраторы получают оповещение не чаще
//! заданной задержки. Образец — `PlayerRateLimitManager`, `SharedPlayerRateLimitManager`,
//! `RateLimitRegistration` SS14 @ 2f4a2aad.
//!
//! Время — настоящее (`RealTime`), не игровое: пауза и замедление игры ограничение не
//! меняют. Ограничение только на сервере; на клиенте действия не считаются, как в SS14.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use astrocore_console::{CVarDef, ConfigWorldExt};
use astrocore_ecs::EventWorldExt;
use astrocore_net::{PlayerDisconnected, Session};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};

/// Что делать с действием (`RateLimitStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitStatus {
    Allowed,
    Blocked,
}

/// Реакция на превышение: сессия игрока.
pub type RateLimitAction = fn(&mut World, Entity);

/// Правило ключа (`RateLimitRegistration`).
#[derive(Clone, Copy)]
pub struct RateLimitRegistration {
    pub period: CVarDef<f32>,
    pub count: CVarDef<i32>,
    /// Сказать игроку, что он ограничен: раз за период.
    pub player_limited: Option<RateLimitAction>,
    /// Задержка между оповещениями администраторов, секунды; отрицательная — не оповещать.
    pub admin_announce_delay: Option<CVarDef<i32>>,
    pub admin_announce: Option<RateLimitAction>,
    pub log_type: LogType,
}

#[derive(Debug, Clone, Copy)]
struct Datum {
    count_expires: Instant,
    count: i32,
    announced: bool,
    next_admin_announce: Option<Instant>,
}

/// Правила и счётчики. Ресурс сервера.
#[derive(Resource, Default)]
pub struct RateLimits {
    registrations: HashMap<&'static str, RateLimitRegistration>,
    data: HashMap<Entity, HashMap<&'static str, Datum>>,
}

/// Зарегистрировать ключ (`Register`). Настройки регистрирует владелец ключа; повторная
/// регистрация ключа — ошибка программы.
pub fn register_rate_limit(
    world: &mut World,
    key: &'static str,
    registration: RateLimitRegistration,
) {
    assert_eq!(
        registration.admin_announce.is_some(),
        registration.admin_announce_delay.is_some(),
        "оповещение администраторов задаётся вместе с задержкой"
    );
    let previous = world
        .get_resource_or_insert_with(RateLimits::default)
        .registrations
        .insert(key, registration);
    assert!(previous.is_none(), "ключ ограничения «{key}» уже есть");
}

/// Учесть действие игрока (`CountAction`).
pub fn count_action(world: &mut World, session: Entity, key: &'static str) -> RateLimitStatus {
    let Some(registration) = world
        .get_resource::<RateLimits>()
        .and_then(|r| r.registrations.get(key).copied())
    else {
        panic!("ключ ограничения «{key}» не зарегистрирован");
    };
    let period = Duration::from_secs_f32(world.cvar(&registration.period).max(0.0));
    let limit = world.cvar(&registration.count);
    let announce_delay = registration
        .admin_announce_delay
        .map(|d| world.cvar(&d))
        .filter(|d| *d >= 0)
        .map(|d| Duration::from_secs(d as u64));
    let time = Instant::now();
    let (announce_admins, announce_player) = {
        let mut limits = world.resource_mut::<RateLimits>();
        let datum = limits
            .data
            .entry(session)
            .or_default()
            .entry(key)
            .or_insert(Datum {
                count_expires: time,
                count: 0,
                announced: false,
                next_admin_announce: None,
            });
        if datum.count_expires <= time {
            // Период истёк — начать новый.
            datum.count_expires = time + period;
            datum.count = 0;
            datum.announced = false;
        }
        datum.count += 1;
        if datum.count <= limit {
            return RateLimitStatus::Allowed;
        }
        let announce_admins = match announce_delay {
            Some(delay) if datum.next_admin_announce.is_none_or(|next| next < time) => {
                datum.next_admin_announce = Some(time + delay);
                true
            }
            _ => false,
        };
        let announce_player = !datum.announced;
        datum.announced = true;
        (announce_admins, announce_player)
    };
    if announce_admins && let Some(action) = registration.admin_announce {
        action(world, session);
    }
    if announce_player {
        if let Some(action) = registration.player_limited {
            action(world, session);
        }
        let name = world
            .get::<Session>(session)
            .map(|s| (s.name.clone(), s.user_id));
        let (text, players) = match name {
            Some((name, user)) => (
                format!("Player {name} breached '{key}' rate limit "),
                vec![user],
            ),
            None => (format!("Player breached '{key}' rate limit "), Vec::new()),
        };
        add_admin_log(
            world,
            registration.log_type,
            LogImpact::Medium,
            &text,
            &[],
            &players,
        );
    }
    RateLimitStatus::Blocked
}

pub fn install(world: &mut World, side: Side) {
    if !side.is_server() {
        return;
    }
    world.init_resource::<RateLimits>();
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        let session = event.session;
        world.resource_mut::<RateLimits>().data.remove(&session);
    });
}

#[cfg(test)]
mod tests;
