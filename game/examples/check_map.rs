//! Проверка сохранения карты на настоящей карте SS14: загружает карту раунда без
//! инициализации, сохраняет её, загружает сохранённое в новый мир и сохраняет снова. Обе
//! записи должны совпасть, а число сущностей и тайлов — остаться прежним. Файл грида
//! (`category: Grid`, например шаттл) загружается на пустую карту и сохраняется как грид.
//!
//! `cargo run --release -p astrocore-game --example check_map [-- Maps/saltern.yml [out.yml]]`
//!
//! С `out.yml` первая запись ложится в `out.yml`, повторная — в `out.yml.again`.

use std::time::Instant;

use astrocore_data::{DataNode, parse_yaml, to_yaml_string};
use astrocore_ecs::GameWorldExt;
use astrocore_game::{Side, content, install_content};
use astrocore_map::{MapGrid, MapLoadWorldExt, MapSaveOptions, MapSaveWorldExt, MapWorldExt};
use astrocore_math::{Angle, Vec2};
use astrocore_movement::MovementServerPlugin;
use astrocore_net::NetServerPlugin;
use astrocore_physics::PhysicsPlugin;
use bevy_ecs::world::World;

/// Мир сервера с контентом, но без карты.
fn content_world() -> Result<World, String> {
    let (resources, ss14) = content::mount_resources()?;
    let mut world = World::new();
    world.add_plugin(NetServerPlugin);
    world.add_plugin(PhysicsPlugin::SERVER);
    world.add_plugin(MovementServerPlugin);
    world.add_plugin(astrocore_audio::AudioPlugin);
    world.add_plugin(astrocore_light::LightPlugin);
    install_content(&mut world, Side::Server);
    content::load_content(&mut world, &resources, &ss14);
    Ok(world)
}

/// Загружает документ без инициализации и сохраняет первую карту.
fn round_trip(document: &DataNode) -> Result<(String, usize, usize), String> {
    let mut world = content_world()?;
    let started = Instant::now();
    let grid_file = document
        .get("meta")
        .and_then(|meta| meta.get("category"))
        .and_then(DataNode::as_str)
        == Some("Grid");
    let (loaded, root) = if grid_file {
        let (_, id) = world.create_map(false);
        let loaded = world
            .load_grid(document, id, Vec2::ZERO, Angle(0.0))
            .map_err(|error| error.to_string())?;
        let grid = *loaded.grids.first().ok_or("в файле нет грида")?;
        (loaded, grid)
    } else {
        let loaded = world
            .load_map(document, false)
            .map_err(|error| error.to_string())?;
        let map = loaded.maps.first().ok_or("в файле нет карты")?.0;
        (loaded, map)
    };
    for error in loaded.errors.iter().take(10) {
        println!("  карта: {error}");
    }
    let tiles: usize = loaded
        .grids
        .iter()
        .filter_map(|grid| world.get::<MapGrid>(*grid))
        .map(|grid| grid.tile_count() as usize)
        .sum();
    astrocore_game::decals::write_for_save(&mut world, root);
    astrocore_game::atmos::write_for_save(&mut world, root);
    let saved = if grid_file {
        world.save_grid(root, MapSaveOptions::default())
    } else {
        world.save_map(root, MapSaveOptions::default())
    };
    let mut saved = saved.map_err(|error| error.to_string())?;
    // Ссылки на устройства, как в `savemap` (ADR-0014): без правок поля остаются как в файле.
    astrocore_game::device_network::write_references(&mut world, &mut saved);
    astrocore_game::device_linking::write_references(&mut world, &mut saved);
    println!(
        "  загружено {} сущностей, сохранено {}, тайлов {tiles} за {:.1?}",
        loaded.entities,
        saved.entities,
        started.elapsed()
    );
    Ok((to_yaml_string(&saved.document), saved.entities, tiles))
}

fn main() -> Result<(), String> {
    content::register_names();
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "Maps/saltern.yml".to_owned());
    let out = args.next();
    let (resources, _) = content::mount_resources()?;
    let text = resources
        .read_to_string(&path)
        .map_err(|error| format!("{path}: {error}"))?;
    let original = parse_yaml(&text).map_err(|error| format!("{path}: {error}"))?;

    println!("{path}:");
    let (first, entities, tiles) = round_trip(&original)?;
    println!("Сохранённая копия:");
    let reparsed = parse_yaml(&first).map_err(|error| format!("сохранённая карта: {error}"))?;
    let (second, entities_again, tiles_again) = round_trip(&reparsed)?;
    if let Some(out) = out {
        std::fs::write(&out, &first).map_err(|error| format!("{out}: {error}"))?;
        let again = format!("{out}.again");
        std::fs::write(&again, &second).map_err(|error| format!("{again}: {error}"))?;
    }

    if (entities, tiles) != (entities_again, tiles_again) {
        return Err(format!(
            "после повторной загрузки сущностей {entities_again} вместо {entities}, тайлов {tiles_again} вместо {tiles}"
        ));
    }
    if first != second {
        let line = first
            .lines()
            .zip(second.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        return Err(format!(
            "повторное сохранение отличается со строки {}",
            line + 1
        ));
    }
    println!("Повторное сохранение совпадает с первым.");
    Ok(())
}
