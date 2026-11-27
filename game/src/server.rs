//! Сервер игры: мир с контентом, базой данных и раундом (`GameTicker`, ADR-0012).

use astrocore_console::{Config, ConfigWorldExt};
use astrocore_ecs::{GameWorldExt, MetaData};
use astrocore_map::MapCoordinates;
use astrocore_movement::MovementServerPlugin;
use astrocore_net::NetServerPlugin;
use astrocore_physics::PhysicsPlugin;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::content;

/// Место появления вошедших игроков для проверок без человека: тело ставится сюда, а не
/// на точку появления станции.
#[derive(Resource, Debug, Clone, Copy)]
pub struct ArrivalPoint(pub MapCoordinates);

/// Итог запуска сервера.
pub struct ServerStart {
    pub world: World,
    /// Сущностей после запуска: с картой, если раунд начался сразу.
    pub entities: usize,
    pub prototype_errors: usize,
}

/// Создаёт мир сервера: плагины движка, контент, база данных и первый раунд.
pub fn start() -> Result<ServerStart, String> {
    start_with(&[])
}

/// То же с настройками поверх `server_config.toml` (например, без лобби для игры в одиночку).
pub fn start_with(settings: &[(&str, &str)]) -> Result<ServerStart, String> {
    content::register_names();
    let (resources, ss14) = content::mount_resources()?;
    let mut world = World::new();
    world.add_plugin(NetServerPlugin);
    world.add_plugin(PhysicsPlugin::SERVER);
    world.add_plugin(MovementServerPlugin);
    world.add_plugin(astrocore_audio::AudioPlugin);
    world.add_plugin(astrocore_light::LightPlugin);
    crate::install_content(&mut world, crate::Side::Server);
    content::register_cvars(&mut world);
    if let Ok(text) = std::fs::read_to_string("server_config.toml") {
        world
            .load_config(&text)
            .map_err(|error| format!("server_config.toml: {error}"))?;
    }
    for (name, value) in settings {
        world
            .set_cvar_text(name, value)
            .map_err(|error| format!("{name}: {error}"))?;
    }
    // База данных (`ServerDbManager.Init`): без неё сервер не запускается, как в SS14.
    let database = crate::database::Database::from_config(world.resource::<Config>())?;
    world.insert_resource(database);
    let report = content::load_content(&mut world, &resources, &ss14);
    for error in report.errors.iter().take(20) {
        eprintln!("прототипы: {error}");
    }
    // Карту раунда `GameTicker` читает из ресурсов при старте раунда.
    world.insert_resource(resources);
    crate::game_ticker::post_initialize(&mut world);
    let entities = world.query::<&MetaData>().iter(&world).count();
    Ok(ServerStart {
        world,
        entities,
        prototype_errors: report.errors.len(),
    })
}
