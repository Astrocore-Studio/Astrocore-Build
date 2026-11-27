//! Сервер игры: мир с картой раунда и вход игроков.

use astrocore_console::{Config, ConfigWorldExt};
use astrocore_ecs::{EventWorldExt, GameWorldExt, LifecycleWorldExt, MetaData};
use astrocore_map::{MapCoordinates, MapLoadReport, MapLoadWorldExt, MapWorldExt, SpawnWorldExt};
use astrocore_math::Vec2;
use astrocore_movement::MovementServerPlugin;
use astrocore_net::{NetServerPlugin, PlayerDisconnected, Session, SessionWorldExt};
use astrocore_physics::PhysicsPlugin;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::content::{self, MAP, PLAYER_PROTOTYPE};

/// Место появления вошедших игроков для проверок без человека: тело ставится сюда, а не
/// на точку появления станции.
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

/// Тело вошедшего игрока: должность станции и точка появления, для проверок — заданное
/// место; без станции и профиля — прототип игрока по умолчанию.
fn spawn_player(
    world: &mut World,
    user: astrocore_net::NetUserId,
    profile: Option<&crate::profiles::HumanoidCharacterProfile>,
) -> Option<Entity> {
    use crate::stations::{pick_best_available_job, stations, try_assign_job};
    let station = stations(world).first().copied();
    let job = match (station, profile) {
        (Some(station), Some(profile)) => {
            let job = pick_best_available_job(
                world,
                station,
                &profile.job_priorities,
                true,
                &Default::default(),
            );
            if let Some(job) = &job {
                try_assign_job(world, station, job, user);
            }
            job
        }
        _ => None,
    };
    if let Some(ArrivalPoint(point)) = world.get_resource::<ArrivalPoint>().copied() {
        let at = world.snap_to_grid_or_map(point)?;
        return crate::station_spawning::spawn_player_mob(
            world,
            at,
            job.as_deref(),
            profile,
            station,
        );
    }
    if (profile.is_some() || station.is_some())
        && let Some(body) = crate::station_spawning::spawn_player_character_on_station(
            world,
            station,
            job.as_deref(),
            profile,
            true,
        )
    {
        return Some(body);
    }
    let prototype = world.resource::<Config>().get_text(&PLAYER_PROTOTYPE);
    let map = world
        .query::<&astrocore_map::Map>()
        .iter(world)
        .map(|m| m.id)
        .min()?;
    world
        .spawn_on_map(&prototype, MapCoordinates::new(map, Vec2::ZERO))
        .ok()
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
    crate::install_content(&mut world, crate::Side::Server);
    content::register_cvars(&mut world);
    if let Ok(text) = std::fs::read_to_string("server_config.toml") {
        world
            .load_config(&text)
            .map_err(|error| format!("server_config.toml: {error}"))?;
    }
    // База данных (`ServerDbManager.Init`): без неё сервер не запускается, как в SS14.
    let database = crate::database::Database::from_config(world.resource::<Config>())?;
    // Раунд в базе с запуска сервера, пока нет `GameTicker` (6.3): к нему привязан журнал.
    let db = database.db().clone();
    let round = database.block_on(async {
        let server = db.server_id().await?;
        db.add_new_round(server, &[]).await
    })?;
    world.insert_resource(database);
    crate::admin_logs::round_starting(&mut world, round);
    let report = content::load_content(&mut world, &resources, &ss14);
    for error in report.errors.iter().take(20) {
        eprintln!("прототипы: {error}");
    }

    // Карта раунда — прототип `gameMap` или путь файла (`GameMapManager`).
    let setting = world.resource::<Config>().get_text(&MAP);
    let game_map = world
        .get_resource::<astrocore_prototypes::Prototypes>()
        .and_then(|p| crate::stations::game_map(p, &setting));
    let map = game_map
        .as_ref()
        .map_or(setting, |m| crate::stations::resource_path(&m.map_path));
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
    if let Some(game_map) = &game_map {
        crate::stations::setup_stations(&mut world, game_map, &loaded.grids, None);
    }
    world.init_resource::<Bodies>();

    // Игрок с загруженными настройками сразу входит в раунд, как при позднем входе
    // (`SpawnPlayer`): лучшая свободная должность станции по приоритетам, точка позднего
    // входа, снаряжение должности. Лобби и раунд — `GameTicker` (6.3).
    world.subscribe_broadcast::<crate::preferences::PreferencesLoaded>(|world, event| {
        if world.resource::<Bodies>().0.contains_key(&event.session) {
            return;
        }
        let profile = world
            .resource::<crate::preferences::PreferencesCache>()
            .selected_profile(event.user_id)
            .cloned();
        let body = spawn_player(world, event.user_id, profile.as_ref());
        match body {
            Some(body) => {
                if profile.is_none() {
                    let name = world.get::<Session>(event.session).map(|s| s.name.clone());
                    if let Some(mut meta) = world.get_mut::<MetaData>(body) {
                        meta.name = name;
                    }
                }
                world.attach_player(event.session, body);
                world.resource_mut::<Bodies>().0.insert(event.session, body);
            }
            None => eprintln!("игрок не получил тело"),
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
