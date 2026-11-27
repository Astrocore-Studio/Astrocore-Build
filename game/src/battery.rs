//! Заряд батарей предметов: заряд меняется линейно со скоростью (`ChargeRate`) от времени
//! последней записи, скорость собирается событием (самозарядка прибавляет, потребление
//! батарейки устройством — отнимает), изменения заряда и переходы «полна / пуста / между»
//! объявляются событиями. Сетевые аккумуляторы (подстанции, АПЦ) живут со скоростью 0: их
//! заряд пишет решатель сети. Образец — `SharedBatterySystem`, `BatteryComponent`,
//! `BatterySelfRechargerComponent`, `ExaminableBatteryComponent`, `ChargeEvents` SS14 @
//! 2f4a2aad.
//!
//! Нет пока: удара ЭМИ (`EmpPulseEvent`), цены по заряду, вида заряда (`BatteryVisuals`).

use std::time::Duration;

use astrocore_ecs::{
    ComponentStartup, EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update,
};
use astrocore_net::NetRegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::damage::Rejuvenate;
use crate::examine::{Examined, loc};
use crate::power::Battery;

/// Полна, пуста или между (`BatteryState`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatteryState {
    Full,
    Empty,
    #[default]
    Neither,
}

/// Заряд изменился (`ChargeChangedEvent`). На батарее.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChargeChanged {
    pub current_charge: f32,
    pub delta: f32,
    pub current_charge_rate: f32,
    pub max_charge: f32,
}

/// Батарея стала полной, пустой или ни той ни другой (`BatteryStateChangedEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryStateChanged {
    pub old_state: BatteryState,
    pub new_state: BatteryState,
}

/// Собрать скорость заряда (`RefreshChargeRateEvent`). На батарее; батарейка передаёт его
/// устройству, в слоте которого стоит.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefreshChargeRate {
    pub max_charge: f32,
    pub new_charge_rate: f32,
}

/// Самозарядка (`BatterySelfRechargerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatterySelfRecharger {
    /// Дж/с.
    #[serde(default)]
    pub auto_recharge_rate: f32,
    /// Пауза самозарядки после траты, с.
    #[serde(default)]
    pub auto_recharge_pause_time: f32,
    /// Когда пауза кончится; `None` — не ждём.
    #[serde(skip, default = "zero")]
    pub next_auto_recharge: Option<Duration>,
}

fn zero() -> Option<Duration> {
    Some(Duration::ZERO)
}

/// Заряд виден при осмотре (`ExaminableBatteryComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExaminableBattery {}

fn now(world: &World) -> Duration {
    world
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default()
}

/// Заряд в момент `time` (`GetCharge`).
pub fn charge_at(battery: &Battery, time: Duration) -> f32 {
    let dt = time.saturating_sub(battery.last_update).as_secs_f32();
    (battery.last_charge + dt * battery.charge_rate).clamp(0.0, battery.max_charge.max(0.0))
}

/// Текущий заряд батареи; нет батареи — 0.
pub fn charge(world: &World, entity: Entity) -> f32 {
    world
        .get::<Battery>(entity)
        .map_or(0.0, |b| charge_at(b, now(world)))
}

/// Доля заряда (`GetChargeLevel`).
pub fn charge_level(world: &World, entity: Entity) -> f32 {
    match world.get::<Battery>(entity) {
        Some(battery) if battery.max_charge > 0.0 => charge(world, entity) / battery.max_charge,
        _ => 0.0,
    }
}

/// Записать новый заряд и объявить изменение.
fn write_charge(world: &mut World, entity: Entity, value: f32) -> f32 {
    let time = now(world);
    let Some(mut battery) = world.get_mut::<Battery>(entity) else {
        return 0.0;
    };
    let old = charge_at(&battery, time);
    let new = value.clamp(0.0, battery.max_charge.max(0.0));
    let delta = new - old;
    if delta == 0.0 {
        return 0.0;
    }
    battery.last_charge = new;
    battery.last_update = time;
    let mut event = ChargeChanged {
        current_charge: new,
        delta,
        current_charge_rate: battery.charge_rate,
        max_charge: battery.max_charge,
    };
    world.raise(entity, &mut event);
    update_state(world, entity);
    delta
}

/// Изменить заряд на `amount`, в пределах ёмкости; вернуть, на сколько изменился
/// (`ChangeCharge`). Трата ставит паузу самозарядки.
pub fn change_charge(world: &mut World, entity: Entity, amount: f32) -> f32 {
    if amount == 0.0 {
        return 0.0;
    }
    let old = charge(world, entity);
    let delta = write_charge(world, entity, old + amount);
    if delta != 0.0 {
        try_set_charge_cooldown(world, entity);
    }
    delta
}

/// Потратить заряд (`UseCharge`).
pub fn use_charge(world: &mut World, entity: Entity, amount: f32) -> f32 {
    if amount <= 0.0 {
        return 0.0;
    }
    change_charge(world, entity, -amount)
}

/// Потратить, только если заряда хватает (`TryUseCharge`).
pub fn try_use_charge(world: &mut World, entity: Entity, amount: f32) -> bool {
    if world.get::<Battery>(entity).is_none() || amount > charge(world, entity) {
        return false;
    }
    use_charge(world, entity, amount);
    true
}

/// Задать заряд (`SetCharge`).
pub fn set_charge(world: &mut World, entity: Entity, value: f32) {
    write_charge(world, entity, value);
}

/// Обновить «полна / пуста / между» (`UpdateState`).
pub fn update_state(world: &mut World, entity: Entity) {
    let time = now(world);
    let Some(mut battery) = world.get_mut::<Battery>(entity) else {
        return;
    };
    let charge = charge_at(&battery, time);
    let new_state = if charge >= battery.max_charge {
        BatteryState::Full
    } else if charge == 0.0 {
        BatteryState::Empty
    } else {
        BatteryState::Neither
    };
    let old_state = battery.state;
    if old_state == new_state {
        return;
    }
    battery.state = new_state;
    world.raise(
        entity,
        &mut BatteryStateChanged {
            old_state,
            new_state,
        },
    );
}

/// Пересобрать скорость заряда, не меняя текущего заряда (`RefreshChargeRate`).
pub fn refresh_charge_rate(world: &mut World, entity: Entity) -> f32 {
    let time = now(world);
    let Some(mut battery) = world.get_mut::<Battery>(entity) else {
        return 0.0;
    };
    battery.last_charge = charge_at(&battery, time);
    battery.last_update = time;
    let mut event = RefreshChargeRate {
        max_charge: battery.max_charge,
        new_charge_rate: 0.0,
    };
    world.raise(entity, &mut event);
    let Some(mut battery) = world.get_mut::<Battery>(entity) else {
        return 0.0;
    };
    battery.charge_rate = event.new_charge_rate;
    let mut changed = ChargeChanged {
        current_charge: battery.last_charge,
        delta: 0.0,
        current_charge_rate: battery.charge_rate,
        max_charge: battery.max_charge,
    };
    world.raise(entity, &mut changed);
    event.new_charge_rate
}

/// Поставить паузу самозарядки после траты (`TrySetChargeCooldown`).
pub fn try_set_charge_cooldown(world: &mut World, entity: Entity) {
    let time = now(world);
    let Some(recharger) = world.get::<BatterySelfRecharger>(entity) else {
        return;
    };
    if recharger.auto_recharge_pause_time <= 0.0 {
        return;
    }
    let pause = Duration::from_secs_f32(recharger.auto_recharge_pause_time);
    if recharger
        .next_auto_recharge
        .is_some_and(|next| time + pause <= next)
    {
        return;
    }
    if let Some(mut recharger) = world.get_mut::<BatterySelfRecharger>(entity) {
        recharger.next_auto_recharge = Some(time + pause);
    }
    refresh_charge_rate(world, entity);
}

/// Конец паузы самозарядки и переходы «полна / пуста» при идущем заряде (`Update`).
fn update(world: &mut World) {
    let time = now(world);
    let ready: Vec<Entity> = world
        .query::<(Entity, &BatterySelfRecharger, &Battery)>()
        .iter(world)
        .filter(|(_, r, _)| r.next_auto_recharge.is_some_and(|next| time >= next))
        .map(|(e, _, _)| e)
        .collect();
    for entity in ready {
        if let Some(mut recharger) = world.get_mut::<BatterySelfRecharger>(entity) {
            recharger.next_auto_recharge = None;
        }
        refresh_charge_rate(world, entity);
    }
    let changing: Vec<Entity> = world
        .query::<(Entity, &Battery)>()
        .iter(world)
        .filter(|(_, b)| b.charge_rate != 0.0)
        .map(|(e, _)| e)
        .collect();
    for entity in changing {
        update_state(world, entity);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<BatterySelfRecharger>();
    world.register_data_component::<ExaminableBattery>();
    world.register_net_serde::<BatterySelfRecharger>();
    if !side.is_server() {
        return;
    }
    // Заряд на старте (`OnMapInit`).
    world.subscribe::<Battery, MapInit>(|world, entity, _| {
        let starting = world
            .get::<Battery>(entity)
            .map_or(0.0, |b| b.starting_charge);
        set_charge(world, entity, starting);
        refresh_charge_rate(world, entity);
    });
    world.subscribe::<Battery, Rejuvenate>(|world, entity, _| {
        let max = world.get::<Battery>(entity).map_or(0.0, |b| b.max_charge);
        set_charge(world, entity, max);
    });
    world.subscribe::<BatterySelfRecharger, RefreshChargeRate>(|world, entity, event| {
        let time = now(world);
        let Some(recharger) = world.get::<BatterySelfRecharger>(entity) else {
            return;
        };
        if recharger.next_auto_recharge.is_some_and(|next| time < next) {
            return;
        }
        event.new_charge_rate += recharger.auto_recharge_rate;
    });
    world.subscribe::<BatterySelfRecharger, ComponentStartup>(|world, entity, _| {
        refresh_charge_rate(world, entity);
    });
    world.subscribe::<ExaminableBattery, Examined>(|world, entity, event| {
        if !event.is_in_details_range {
            return;
        }
        let percent = (100.0 * charge_level(world, entity)) as i64;
        let text = loc(
            world,
            "examinable-battery-component-examine-detail",
            &[
                ("percent", percent.into()),
                ("markupPercentColor", "green".into()),
            ],
        );
        event.push_markup(text, 0);
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
