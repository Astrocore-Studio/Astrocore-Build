//! Проверка систем контента на данных SS14: загружает все прототипы референса, создаёт по
//! одной сущности каждого прототипа и печатает, что из компонентов игры не разобралось.
//!
//! `cargo run --release -p astrocore-game --example check_content`
//!
//! Каталог `Resources` SS14 — как у игры: `ASTRO_SS14_RESOURCES` или референс рядом с Build.

use std::collections::BTreeMap;
use std::time::Instant;

use astrocore_ecs::{ComponentRegistry, GameWorldExt};
use astrocore_game::{Side, content, install_content};
use astrocore_map::{MapCoordinates, SpawnWorldExt};
use astrocore_math::Vec2;
use astrocore_movement::MovementServerPlugin;
use astrocore_net::NetServerPlugin;
use astrocore_physics::PhysicsPlugin;
use astrocore_prototypes::Prototypes;

fn main() -> Result<(), String> {
    content::register_names();
    let (resources, ss14) = content::mount_resources()?;
    let mut world = bevy_ecs::world::World::new();
    world.add_plugin(NetServerPlugin);
    world.add_plugin(PhysicsPlugin::SERVER);
    world.add_plugin(MovementServerPlugin);
    world.add_plugin(astrocore_audio::AudioPlugin);
    world.add_plugin(astrocore_light::LightPlugin);
    install_content(&mut world, Side::Server);
    let started = Instant::now();
    let report = content::load_content(&mut world, &resources, &ss14);
    println!(
        "Прототипы: {} файлов за {:.1?}, ошибок {}",
        report.files,
        started.elapsed(),
        report.errors.len()
    );
    for error in report.errors.iter().take(10) {
        println!(
            "  ошибка: {}:{}: {}",
            error.source, error.line, error.message
        );
    }

    // Поля компонентов игры и движка, которые данные задают, а мы не читаем.
    let ours: Vec<String> = world
        .resource::<ComponentRegistry>()
        .iter()
        .map(|info| info.name.to_string())
        .collect();
    let mut unknown: Vec<(&String, &usize)> = report
        .unknown_fields
        .iter()
        .filter(|(field, _)| {
            ours.iter()
                .any(|name| field.split('.').next() == Some(name.as_str()))
        })
        .collect();
    unknown.sort_by(|a, b| b.1.cmp(a.1));
    println!("Непрочитанные поля наших компонентов: {}", unknown.len());
    for (field, count) in unknown.iter().take(40) {
        println!("  {field}: {count}");
    }

    // Каждый прототип сущности создаётся на пустой карте.
    let (_, map) = astrocore_map::MapWorldExt::create_map(&mut world, true);
    let ids: Vec<String> = world
        .resource::<Prototypes>()
        .entities()
        .map(|proto| proto.id.to_string())
        .collect();
    let started = Instant::now();
    let mut failed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (index, id) in ids.iter().enumerate() {
        let at = MapCoordinates::new(
            map,
            Vec2::new((index % 200) as f32 * 2.0, (index / 200) as f32 * 2.0),
        );
        if let Err(error) = world.spawn_on_map(id, at) {
            failed
                .entry(error.to_string())
                .or_default()
                .push(id.clone());
        }
    }
    world.run_tick();
    println!(
        "Сущности: создано {} из {} за {:.1?}",
        ids.len() - failed.values().map(Vec::len).sum::<usize>(),
        ids.len(),
        started.elapsed()
    );
    for (error, protos) in failed.iter().take(10) {
        println!(
            "  {} × {error}: {}",
            protos.len(),
            protos
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}
