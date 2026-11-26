//! Сервер игры: мир с картой раунда и вход игроков.

use astrocore_console::{Config, ConfigWorldExt};
use astrocore_ecs::{EventWorldExt, GameWorldExt, LifecycleWorldExt, MetaData};
use astrocore_map::{
    MapCoordinates, MapLoadReport, MapLoadWorldExt, SpawnWorldExt, TransformWorldExt,
};
use astrocore_math::Vec2;
use astrocore_movement::MovementServerPlugin;
use astrocore_net::{
    NetServerPlugin, PlayerConnected, PlayerDisconnected, Session, SessionWorldExt,
};
use astrocore_physics::PhysicsPlugin;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::content::{self, MAP, PLAYER_PROTOTYPE};

/// Где появляются вошедшие игроки.
#[derive(Resource, Debug, Clone, Copy)]
pub struct ArrivalPoint(pub MapCoordinates);

/// Тела игроков по сессиям: тело удаляется, когда игрок выходит.
#[derive(Resource, Debug, Default)]
struct Bodies(std::collections::HashMap<Entity, Entity>);

/// Итог запуска сервера.
pub struct ServerStart {
    pub world: World,
    pub map: MapLoadReport,
    pub prototype_errors: usize,
}

/// Точка появления: первая сущность-маркер позднего входа, иначе любая точка появления.
fn arrival_point(world: &mut World, report: &MapLoadReport) -> MapCoordinates {
    let mut markers: Vec<(bool, Entity)> = world
        .query::<(Entity, &MetaData)>()
        .iter(world)
        .filter_map(|(entity, meta)| {
            let prototype = meta.prototype.as_deref()?;
            prototype
                .starts_with("SpawnPoint")
                .then_some((prototype != "SpawnPointLatejoin", entity))
        })
        .collect();
    markers.sort_unstable();
    markers
        .first()
        .and_then(|(_, entity)| world.map_coordinates(*entity))
        .unwrap_or_else(|| {
            let id = report.maps.first().map(|(_, id)| *id).unwrap_or_default();
            MapCoordinates::new(id, Vec2::ZERO)
        })
}

/// Создаёт мир сервера: плагины движка, контент, карта раунда и правила входа игроков.
pub fn start() -> Result<ServerStart, String> {
    content::register_names();
    let (resources, ss14) = content::mount_resources()?;
    let mut world = World::new();
    world.add_plugin(NetServerPlugin);
    world.add_plugin(PhysicsPlugin::SERVER);
    world.add_plugin(MovementServerPlugin);
    world.add_plugin(astrocore_audio::AudioPlugin);
    world.add_plugin(astrocore_light::LightPlugin);
    crate::interaction::install(&mut world, true);
    content::register_cvars(&mut world);
    if let Ok(text) = std::fs::read_to_string("server_config.toml") {
        world
            .load_config(&text)
            .map_err(|error| format!("server_config.toml: {error}"))?;
    }
    let report = content::load_content(&mut world, &resources, &ss14);
    for error in report.errors.iter().take(20) {
        eprintln!("прототипы: {error}");
    }

    let map = world.resource::<Config>().get_text(&MAP);
    let text = resources
        .read_to_string(&map)
        .map_err(|error| format!("карта {map}: {error}"))?;
    let node =
        astrocore_data::parse_yaml(&text).map_err(|error| format!("карта {map}: {error}"))?;
    let loaded = world
        .load_map(&node, true)
        .map_err(|error| format!("карта {map}: {error}"))?;
    for error in loaded.errors.iter().take(20) {
        eprintln!("карта: {error}");
    }
    let arrival = arrival_point(&mut world, &loaded);
    world.insert_resource(ArrivalPoint(arrival));
    world.init_resource::<Bodies>();

    // Вошедший игрок получает тело у точки появления.
    world.subscribe_broadcast::<PlayerConnected>(|world, event| {
        let prototype = world.resource::<Config>().get_text(&PLAYER_PROTOTYPE);
        let arrival = world.resource::<ArrivalPoint>().0;
        match world.spawn_on_map(&prototype, arrival) {
            Ok(body) => {
                let name = world.get::<Session>(event.session).map(|s| s.name.clone());
                if let Some(mut meta) = world.get_mut::<MetaData>(body) {
                    meta.name = name;
                }
                world.attach_player(event.session, body);
                world.resource_mut::<Bodies>().0.insert(event.session, body);
            }
            Err(error) => eprintln!("игрок не получил тело {prototype}: {error}"),
        }
    });
    world.subscribe_broadcast::<PlayerDisconnected>(|world, event| {
        if let Some(body) = world.resource_mut::<Bodies>().0.remove(&event.session) {
            world.queue_delete(body);
        }
    });

    Ok(ServerStart {
        world,
        map: loaded,
        prototype_errors: report.errors.len(),
    })
}
