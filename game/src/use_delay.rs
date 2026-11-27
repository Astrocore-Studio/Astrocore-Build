//! Пауза использования предметов: после использования в руке или «использовать» предмет
//! недоступен на время паузы (по умолчанию 1 с). У предмета может быть несколько пауз со
//! своими именами (у дефибриллятора — пауза разряда). Образец — `UseDelayComponent`,
//! `UseDelaySystem` (с `API`) и проверки пауз в `SharedInteractionSystem`
//! (`InteractionActivate`, `UseInHandInteraction`) SS14 @ 2f4a2aad.

use std::collections::BTreeMap;
use std::time::Duration;

use astrocore_ecs::{EventWorldExt, GameTime, LifecycleWorldExt, MapInit, RegistryWorldExt};
use astrocore_net::NetRegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;

/// Пауза по умолчанию (`DefaultId`).
pub const DEFAULT_ID: &str = "default";

/// Одна пауза (`UseDelayInfo`). В данных — `length` в секундах.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "UseDelayData", into = "UseDelayData")]
pub struct UseDelayInfo {
    pub length: Duration,
    pub start_time: Duration,
    pub end_time: Duration,
}

/// Пауза в данных: секунды.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct UseDelayData {
    length: f32,
    start_time: f32,
    end_time: f32,
}

impl From<UseDelayData> for UseDelayInfo {
    fn from(data: UseDelayData) -> Self {
        let seconds = |value: f32| Duration::from_secs_f32(value.max(0.0));
        Self {
            length: seconds(data.length),
            start_time: seconds(data.start_time),
            end_time: seconds(data.end_time),
        }
    }
}

impl From<UseDelayInfo> for UseDelayData {
    fn from(info: UseDelayInfo) -> Self {
        Self {
            length: info.length.as_secs_f32(),
            start_time: info.start_time.as_secs_f32(),
            end_time: info.end_time.as_secs_f32(),
        }
    }
}

fn one() -> f32 {
    1.0
}

/// Паузы предмета (`UseDelayComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UseDelay {
    /// Паузы по именам; в данных — длины своих пауз.
    #[serde(default)]
    pub delays: BTreeMap<String, UseDelayInfo>,
    /// Длина паузы по умолчанию, с; читается при появлении.
    #[serde(default = "one")]
    pub delay: f32,
}

fn now(world: &World) -> Duration {
    world
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default()
}

/// Задать длину паузы, добавив её при надобности (`SetLength`).
pub fn set_length(world: &mut World, entity: Entity, length: Duration, id: &str) {
    if world.get::<UseDelay>(entity).is_none() {
        world.add_comp(
            entity,
            UseDelay {
                delays: BTreeMap::new(),
                delay: 1.0,
            },
        );
    }
    let Some(mut component) = world.get_mut::<UseDelay>(entity) else {
        return;
    };
    component.delays.entry(id.to_owned()).or_default().length = length;
}

/// Идёт ли пауза (`IsDelayed`).
pub fn is_delayed(world: &World, entity: Entity, id: &str) -> bool {
    let time = now(world);
    world
        .get::<UseDelay>(entity)
        .and_then(|c| c.delays.get(id))
        .is_some_and(|d| d.end_time >= time)
}

/// Прервать паузу (`CancelDelay`).
pub fn cancel_delay(world: &mut World, entity: Entity, id: &str) -> bool {
    let time = now(world);
    let Some(mut component) = world.get_mut::<UseDelay>(entity) else {
        return false;
    };
    match component.delays.get_mut(id) {
        Some(delay) if delay.end_time > time => {
            delay.end_time = time;
            true
        }
        _ => false,
    }
}

/// Пауза с этим именем (`TryGetDelayInfo`).
pub fn delay_info(world: &World, entity: Entity, id: &str) -> Option<UseDelayInfo> {
    world.get::<UseDelay>(entity)?.delays.get(id).copied()
}

/// Начать паузу заново (`TryResetDelay`); `check_delayed` — не трогать идущую.
pub fn try_reset_delay(world: &mut World, entity: Entity, check_delayed: bool, id: &str) -> bool {
    if check_delayed && is_delayed(world, entity, id) {
        return false;
    }
    let time = now(world);
    let Some(mut component) = world.get_mut::<UseDelay>(entity) else {
        return false;
    };
    let Some(delay) = component.delays.get_mut(id) else {
        return false;
    };
    delay.start_time = time;
    delay.end_time = time + delay.length;
    true
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<UseDelay>();
    world.register_net_serde::<UseDelay>();
    if !side.is_server() {
        return;
    }
    // Длина паузы по умолчанию из прототипа (`OnMapInit`).
    world.subscribe::<UseDelay, MapInit>(|world, entity, _| {
        let Some(seconds) = world.get::<UseDelay>(entity).map(|c| c.delay) else {
            return;
        };
        set_length(world, entity, Duration::from_secs_f32(seconds), DEFAULT_ID);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{spawn_at, station, world_with};
    use astrocore_ecs::GameWorldExt;

    const PROTOTYPES: &str = r#"
- type: entity
  id: Horn
  components:
  - type: UseDelay
    delay: 0.5
    delays:
      gasTank:
        length: 1.0
"#;

    #[test]
    fn delay_runs_from_reset_and_can_be_cancelled() {
        let mut world = world_with(PROTOTYPES);
        let grid = station(&mut world);
        let horn = spawn_at(&mut world, "Horn", grid, 1, 1);
        assert!(!is_delayed(&world, horn, DEFAULT_ID));
        assert_eq!(
            delay_info(&world, horn, "gasTank").map(|d| d.length),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            delay_info(&world, horn, DEFAULT_ID).map(|d| d.length),
            Some(Duration::from_millis(500))
        );
        assert!(try_reset_delay(&mut world, horn, false, DEFAULT_ID));
        assert!(is_delayed(&world, horn, DEFAULT_ID));
        // Идущую паузу с проверкой не перезапустить.
        assert!(!try_reset_delay(&mut world, horn, true, DEFAULT_ID));
        for _ in 0..20 {
            world.run_tick();
        }
        assert!(!is_delayed(&world, horn, DEFAULT_ID));
        // Своя пауза со своим именем.
        set_length(&mut world, horn, Duration::from_secs(5), "zap");
        try_reset_delay(&mut world, horn, false, "zap");
        assert!(is_delayed(&world, horn, "zap"));
        assert!(!is_delayed(&world, horn, DEFAULT_ID));
        assert!(cancel_delay(&mut world, horn, "zap"));
        world.run_tick();
        assert!(!is_delayed(&world, horn, "zap"));
    }
}
