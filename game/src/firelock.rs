//! Пожарные шлюзы: закрываются по тревоге (и открываются, когда она прошла), раз в секунду
//! проверяют, держат ли перепад давления или температуры между соседними клетками, —
//! тогда горят огни, а открыть можно только с доступом; без питания не открываются;
//! голыми руками без инструмента не взламываются, держащий шлюз взламывается дольше.
//! Образец — `FirelockComponent`, `SharedFirelockSystem` и `FirelockSystem` SS14
//! @ 2f4a2aad.
//!
//! Нет пока: стыковочных шлюзов шаттлов (`DockingComponent`) — они всегда «не держат».

use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, Update};
use astrocore_light::PointLight;
use astrocore_map::{MapGrid, TileCoord, Transform, TransformWorldExt};
use astrocore_net::AppearanceWorldExt;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::Side;
use crate::atmos::grid::{offset, opposite_direction};
use crate::atmos::{entity_blocked_directions, tile_mixture};
use crate::atmos_alarms::{AtmosAlarm, AtmosAlarmType, AtmosAlarmable};
use crate::doors::{
    BeforeDoorOpened, BeforePry, Door, DoorState, Firelock, GetPryTimeModifier, Pried,
};
use crate::popups::{PopupType, PopupWorldExt};
use crate::power::{PowerChanged, is_powered};

/// Огни закрытой двери (`DoorVisuals.ClosedLights`).
pub const CLOSED_LIGHTS: &str = "enum.DoorVisuals.ClosedLights";
/// Держит давление (`FirelockVisuals.PressureWarning`).
pub const PRESSURE_WARNING: &str = "enum.FirelockVisuals.PressureWarning";
/// Держит огонь (`FirelockVisuals.TemperatureWarning`).
pub const TEMPERATURE_WARNING: &str = "enum.FirelockVisuals.TemperatureWarning";
/// Проверка раз в 30 тактов (`UpdateInterval`).
const UPDATE_INTERVAL: u32 = 30;

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

fn door_state(world: &World, entity: Entity) -> Option<DoorState> {
    world.get::<Door>(entity).map(|d| d.state)
}

/// Аварийно закрыть (`EmergencyPressureStop`): открытая дверь сразу становится твёрдой.
pub fn emergency_pressure_stop(world: &mut World, entity: Entity) -> bool {
    let Some(firelock) = world.get::<Firelock>(entity).cloned() else {
        return false;
    };
    if door_state(world, entity) != Some(DoorState::Open)
        || firelock
            .emergency_close_cooldown
            .is_some_and(|until| now(world) < until)
    {
        return false;
    }
    if !crate::doors::try_close(world, entity, None) {
        return false;
    }
    crate::doors::on_partial_close(world, entity)
}

/// Клетка шлюза на гриде.
fn cell_of(world: &World, entity: Entity) -> Option<(Entity, TileCoord)> {
    let transform = world.get::<Transform>(entity)?;
    let grid = transform.grid()?;
    if world.parent(entity) != Some(grid) {
        return None;
    }
    Some((
        grid,
        TileCoord::containing(transform.local_position(), transform.level()),
    ))
}

/// Есть ли на клетке непроницаемая сущность, перекрывающая направление (`HasAirtightBlocker`).
fn has_airtight_blocker(world: &World, grid: Entity, cell: TileCoord, direction: u8) -> bool {
    let Some(map_grid) = world.get::<MapGrid>(grid) else {
        return false;
    };
    map_grid
        .anchored_at(cell)
        .iter()
        .any(|e| entity_blocked_directions(world, *e) & direction == direction)
}

/// Держит ли шлюз давление и огонь (`CheckPressureAndFire`): разница давлений и
/// температур между клетками по разные стороны больше порогов.
pub fn check_pressure_and_fire(world: &World, entity: Entity) -> (bool, bool) {
    let Some(firelock) = world.get::<Firelock>(entity) else {
        return (false, false);
    };
    let blocked = entity_blocked_directions(world, entity);
    if blocked == 0 {
        return (false, false);
    }
    let Some((grid, cell)) = cell_of(world, entity) else {
        return (false, false);
    };
    if world.get::<crate::atmos::GridAtmosphere>(grid).is_none() {
        return (false, false);
    }
    let (mut min_p, mut max_p) = (f32::MAX, f32::MIN);
    let (mut min_t, mut max_t) = (f32::MAX, f32::MIN);
    let (mut pressure, mut fire) = (false, false);
    for index in 0..4 {
        let direction = 1u8 << index;
        if blocked & direction == 0 {
            continue;
        }
        let neighbour = offset(cell, index);
        if let Some(gas) = tile_mixture(world, grid, neighbour) {
            if has_airtight_blocker(world, grid, neighbour, opposite_direction(index)) {
                continue;
            }
            min_p = min_p.min(gas.pressure());
            max_p = max_p.max(gas.pressure());
            min_t = min_t.min(gas.temperature());
            max_t = max_t.max(gas.temperature());
        }
        pressure |= max_p - min_p > firelock.pressure_threshold;
        fire |= max_t - min_t > firelock.temperature_threshold;
        if pressure && fire {
            return (pressure, fire);
        }
    }
    if blocked == 0b1111 {
        return (pressure, fire);
    }
    // Шлюз пропускает часть направлений: своя клетка тоже сторона.
    let (p, t) = tile_mixture(world, grid, cell)
        .map_or((0.0, 0.0), |gas| (gas.pressure(), gas.temperature()));
    min_p = min_p.min(p);
    max_p = max_p.max(p);
    min_t = min_t.min(t);
    max_t = max_t.max(t);
    pressure |= max_p - min_p > firelock.pressure_threshold;
    fire |= max_t - min_t > firelock.temperature_threshold;
    (pressure, fire)
}

fn update(world: &mut World, mut ticks: bevy_ecs::system::Local<u32>) {
    *ticks += 1;
    if *ticks < UPDATE_INTERVAL {
        return;
    }
    *ticks = 0;
    let firelocks: Vec<Entity> = world
        .query::<(Entity, &Firelock, &Door)>()
        .iter(world)
        .map(|(e, _, _)| e)
        .collect();
    for entity in firelocks {
        let danger = world
            .get::<AtmosAlarmable>(entity)
            .is_some_and(|a| a.last_alarm_state == AtmosAlarmType::Danger);
        if danger && is_powered(world, entity) && door_state(world, entity) == Some(DoorState::Open)
        {
            emergency_pressure_stop(world, entity);
        }
        if !matches!(
            door_state(world, entity),
            Some(DoorState::Closed | DoorState::Denying)
        ) {
            continue;
        }
        if world.get::<crate::airtight::Airtight>(entity).is_none()
            || world.get::<astrocore_net::Appearance>(entity).is_none()
        {
            continue;
        }
        let (pressure, fire) = check_pressure_and_fire(world, entity);
        world.set_appearance(entity, CLOSED_LIGHTS, fire || pressure);
        if let Some(mut firelock) = world.get_mut::<Firelock>(entity) {
            firelock.temperature = fire;
            firelock.pressure = pressure;
        }
        world.set_appearance(entity, PRESSURE_WARNING, pressure);
        world.set_appearance(entity, TEMPERATURE_WARNING, fire);
        if let Some(mut light) = world.get_mut::<PointLight>(entity)
            && light.enabled != (fire || pressure)
        {
            light.enabled = fire || pressure;
        }
    }
}

/// Предупредить взламывающего или открывающего (`WarnPlayer`).
fn warn_player(world: &mut World, entity: Entity, user: Entity) {
    let Some(firelock) = world.get::<Firelock>(entity).cloned() else {
        return;
    };
    let key = if firelock.temperature {
        "firelock-component-is-holding-fire-message"
    } else if firelock.pressure {
        "firelock-component-is-holding-pressure-message"
    } else {
        return;
    };
    let text = crate::examine::loc(world, key, &[]);
    world.popup_entity_for(&text, entity, user, PopupType::MediumCaution);
}

/// Вид огней по состоянию (`UpdateVisuals`).
fn update_visuals(world: &mut World, entity: Entity) {
    let Some(state) = door_state(world, entity) else {
        return;
    };
    if world.get::<astrocore_net::Appearance>(entity).is_none() {
        return;
    }
    let lights = matches!(state, DoorState::Closed | DoorState::Denying)
        && world
            .get::<Firelock>(entity)
            .is_some_and(Firelock::is_locked);
    world.set_appearance(entity, CLOSED_LIGHTS, lights);
}

pub fn install(world: &mut World, side: Side) {
    if !side.is_server() {
        return;
    }
    world.subscribe::<Firelock, MapInit>(|world, entity, _| {
        let powered = is_powered(world, entity);
        if let Some(mut firelock) = world.get_mut::<Firelock>(entity) {
            firelock.powered = powered;
        }
        update_visuals(world, entity);
    });
    world.subscribe::<Firelock, PowerChanged>(|world, entity, event| {
        if let Some(mut firelock) = world.get_mut::<Firelock>(entity) {
            firelock.powered = event.powered;
        }
    });
    // Тревога: опасность закрывает, норма открывает закрытый (`OnAtmosAlarm`).
    world.subscribe::<Firelock, AtmosAlarm>(|world, entity, event| {
        if !is_powered(world, entity) {
            return;
        }
        match event.alarm_type {
            AtmosAlarmType::Normal => {
                if door_state(world, entity) == Some(DoorState::Closed) {
                    crate::doors::try_open(world, entity, None, false);
                }
            }
            AtmosAlarmType::Danger => {
                emergency_pressure_stop(world, entity);
            }
            _ => {}
        }
    });
    // Открыть: без питания нельзя, держащий — только с доступом (`OnBeforeDoorOpened`).
    world.subscribe::<Firelock, BeforeDoorOpened>(|world, entity, event| {
        let Some(firelock) = world.get::<Firelock>(entity).cloned() else {
            return;
        };
        let override_access = event
            .user
            .is_some_and(|user| crate::access::is_allowed(world, user, entity));
        if !firelock.powered || (!override_access && firelock.is_locked()) {
            event.cancelled = true;
        } else if let Some(user) = event.user {
            warn_player(world, entity, user);
        }
    });
    world.subscribe::<Firelock, BeforePry>(|world, entity, event| {
        let powered = world.get::<Firelock>(entity).is_some_and(|f| f.powered);
        if event.cancelled || !powered || event.strong_pry || event.pry_powered {
            return;
        }
        event.cancelled = true;
    });
    world.subscribe::<Firelock, GetPryTimeModifier>(|world, entity, event| {
        warn_player(world, entity, event.user);
        if let Some(firelock) = world.get::<Firelock>(entity)
            && firelock.is_locked()
        {
            event.pry_time_modifier *= firelock.locked_pry_time_modifier;
        }
    });
    world.subscribe::<Firelock, Pried>(|world, entity, _| {
        let time = now(world);
        if let Some(mut firelock) = world.get_mut::<Firelock>(entity) {
            firelock.emergency_close_cooldown =
                Some(time + firelock.emergency_close_cooldown_duration);
        }
    });
    world.subscribe::<Firelock, crate::doors::DoorStateChanged>(|world, entity, _| {
        update_visuals(world, entity);
    });
    world.subscribe::<Firelock, crate::examine::Examined>(|world, entity, event| {
        let Some(firelock) = world.get::<Firelock>(entity).cloned() else {
            return;
        };
        if firelock.pressure {
            let text =
                crate::examine::loc(world, "firelock-component-examine-pressure-warning", &[]);
            event.push_markup(text, 0);
        }
        if firelock.temperature {
            let text =
                crate::examine::loc(world, "firelock-component-examine-temperature-warning", &[]);
            event.push_markup(text, 0);
        }
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
