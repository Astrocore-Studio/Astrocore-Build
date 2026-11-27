//! Проверка электросети на карте раунда: сервер с картой работает `секунды` игрового
//! времени, затем печатает сети, источники, аккумуляторы и долю запитанных устройств.
//!
//! `cargo run --release -p astrocore-game --example check_power [-- 10]`

use std::collections::BTreeMap;
use std::time::Instant;

use astrocore_ecs::{GameTime, GameWorldExt, MetaData};
use astrocore_game::node_container::NodeGroups;
use astrocore_game::power::{
    ApcPowerReceiver, Battery, PowerNetworkBattery, PowerSupplier, is_powered,
};
use astrocore_game::server;
use bevy_ecs::entity::Entity;

fn main() -> Result<(), String> {
    let seconds: f32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    let mut start = server::start_with(&[("game.lobbyenabled", "false")])?;
    let world = &mut start.world;
    let ticks = (seconds * f32::from(world.resource::<GameTime>().tick_rate)) as u32;
    let started = Instant::now();
    for _ in 0..ticks {
        world.run_tick();
    }
    let per_tick = started.elapsed() / ticks.max(1);
    let total = world.query::<&MetaData>().iter(world).count();
    println!(
        "Сущностей в мире: {total} (в файле карты {})",
        start.entities
    );
    let groups = world.resource::<NodeGroups>();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for group in &groups.groups {
        *kinds.entry(group.kind.clone()).or_default() += 1;
    }
    println!("Группы узлов: {kinds:?}");
    let supply: f32 = world
        .query::<&PowerSupplier>()
        .iter(world)
        .map(|s| s.current_supply)
        .sum();
    println!("Источники отдают {supply:.0} Вт");
    let mut stored = 0.0;
    let mut capacity = 0.0;
    for (battery, _) in world
        .query::<(&Battery, &PowerNetworkBattery)>()
        .iter(world)
    {
        stored += battery.last_charge;
        capacity += battery.max_charge;
    }
    println!(
        "Аккумуляторы: {:.1} из {:.1} МДж",
        stored / 1e6,
        capacity / 1e6
    );
    let receivers: Vec<Entity> = world
        .query::<(Entity, &ApcPowerReceiver)>()
        .iter(world)
        .map(|(e, _)| e)
        .collect();
    let mut dark: BTreeMap<String, usize> = BTreeMap::new();
    let mut lit = 0;
    for &entity in &receivers {
        if is_powered(world, entity) {
            lit += 1;
        } else {
            let proto = world
                .get::<MetaData>(entity)
                .and_then(|m| m.prototype.as_deref().map(str::to_owned))
                .unwrap_or_default();
            *dark.entry(proto).or_default() += 1;
        }
    }
    println!("Запитано устройств: {lit} из {}", receivers.len());
    // Почему устройства без питания.
    let links = world
        .resource::<astrocore_game::power::ApcLinks>()
        .provider_of
        .clone();
    let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for &entity in receivers.iter().filter(|e| !is_powered(world, **e)) {
        let receiver = world.get::<ApcPowerReceiver>(entity).unwrap();
        let reason = if receiver.power_disabled {
            "выключено"
        } else if receiver.power_load <= 0.0 {
            "нагрузка 0 Вт (лампа без лампочки)"
        } else if !links.contains_key(&entity) {
            "нет удлинителя АПЦ рядом"
        } else {
            "мало питания"
        };
        *reasons.entry(reason).or_default() += 1;
    }
    println!("Причины: {reasons:?}");
    let mut dark: Vec<_> = dark.into_iter().collect();
    dark.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    for (proto, count) in dark.iter().take(12) {
        println!("  без питания: {proto} × {count}");
    }
    println!("Такт сервера: {per_tick:.1?}");
    let gravity: Vec<(bool, bool)> = world
        .query::<&astrocore_game::gravity::Gravity>()
        .iter(world)
        .map(|g| (g.enabled, g.inherent))
        .collect();
    let generators: Vec<(bool, f32)> = world
        .query::<(
            &astrocore_game::gravity::GravityGenerator,
            &astrocore_game::gravity::PowerCharge,
        )>()
        .iter(world)
        .map(|(g, c)| (g.gravity_active, c.charge))
        .collect();
    println!("Гравитация (вкл, своя): {gravity:?}; генераторы (работает, заряд): {generators:?}");
    Ok(())
}
