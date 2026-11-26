//! Клиент игры.
//!
//! `cargo station` — игра в одиночку: сервер работает в процессе клиента.
//! `cargo client -- --connect 203.0.113.5:7783 --name Urist` — вход на выделенный сервер.
//! Управление — как в SS14: WASD, Shift — шаг, E — использовать устройство рядом.
//!
//! Для проверки без человека: `--capture кадр.png` снимает кадр и завершает программу,
//! `--walk` ведёт персонажа вправо, `--use-nearest` (только с `--host`) ставит игрока рядом
//! с устройством, у которого есть интерфейс, и нажимает на нём «использовать в мире»,
//! `--examine-nearest` осматривает ближайшую сущность и показывает всплывающее сообщение,
//! `--storage-nearest` (только с `--host`) ставит игрока у ящика с инструментами, кладёт в него
//! инструменты, открывает его окно и ближайший шкаф, `--hurt` (только с `--host`) через секунду
//! наносит игроку смертельный урон.

use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use astrocore_client::{Capture, ClientPlugin, Game, GameResources, Link};
use astrocore_game::client::{self, LocalHost};
use astrocore_game::{configured_address, configured_key, server};
use astrocore_movement::{LocalInput, MoveButtons};
use astrocore_net::ClientTransport;
use astrocore_transport::ClientLink;
use bevy::prelude::*;
use bevy::window::WindowResolution;

struct Options {
    host: bool,
    connect: Option<String>,
    name: String,
    capture: Option<String>,
    walk: bool,
    use_nearest: bool,
    examine_nearest: bool,
    storage_nearest: bool,
    hurt: bool,
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        host: false,
        connect: None,
        name: "Urist".to_owned(),
        capture: None,
        walk: false,
        use_nearest: false,
        examine_nearest: false,
        storage_nearest: false,
        hurt: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{arg}: нужно значение — {what}"));
        match arg.as_str() {
            "--host" => options.host = true,
            "--walk" => options.walk = true,
            "--use-nearest" => options.use_nearest = true,
            "--examine-nearest" => options.examine_nearest = true,
            "--storage-nearest" => options.storage_nearest = true,
            "--hurt" => options.hurt = true,
            "--connect" => options.connect = Some(value("адрес сервера")?),
            "--name" => options.name = value("имя игрока")?,
            "--capture" => options.capture = Some(value("файл кадра")?),
            other => {
                return Err(format!(
                    "неизвестный параметр {other}. Параметры: --host, --connect <адрес>, --name <имя>, --capture <файл>, --walk"
                ));
            }
        }
    }
    Ok(options)
}

/// Проверка без человека: точка появления переносится к первому устройству с интерфейсом.
fn stand_by_device(server: &mut bevy_ecs::world::World) {
    use astrocore_game::activatable_ui::ActivatableUi;
    use astrocore_map::{MapCoordinates, TransformWorldExt};
    let mut devices = server.query_filtered::<bevy_ecs::entity::Entity, (
        With<ActivatableUi>,
        With<astrocore_ui::UserInterface>,
    )>();
    let mut found: Vec<_> = devices.iter(server).collect();
    found.sort_unstable();
    if let Some(at) = found
        .first()
        .and_then(|device| server.map_coordinates(*device))
    {
        let beside = MapCoordinates {
            position: at.position + astrocore_math::Vec2::new(0.0, -1.0),
            ..at
        };
        server.insert_resource(server::ArrivalPoint(beside));
    }
}

/// Проверка без человека: через секунду после появления игрок нажимает «использовать в
/// мире» на ближайшем устройстве с интерфейсом.
fn use_nearest(mut game: ResMut<Game>, mut frames: Local<u32>) {
    use astrocore_game::activatable_ui::ActivatableUi;
    use astrocore_game::interaction::functions::ACTIVATE_IN_WORLD;
    use astrocore_input::{BoundKeyState, InputWorldExt};
    use astrocore_map::TransformWorldExt;
    let world = &mut game.0;
    let Some(player) = world.resource::<astrocore_net::ClientNet>().attached else {
        return;
    };
    *frames += 1;
    if *frames != 45 {
        return;
    }
    let Some(here) = world.map_coordinates(player) else {
        return;
    };
    let mut devices = world.query_filtered::<bevy_ecs::entity::Entity, With<ActivatableUi>>();
    let nearest = devices
        .iter(world)
        .filter_map(|device| {
            Some((
                world
                    .map_coordinates(device)?
                    .position
                    .distance(here.position),
                device,
            ))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, device)) = nearest {
        let at = world.map_coordinates(device).map(|at| at.position);
        world.send_input(ACTIVATE_IN_WORLD, BoundKeyState::Down, at, Some(device));
    }
}

/// Проверка без человека: осмотр ближайшей сущности и всплывающее сообщение над игроком.
fn examine_nearest(mut game: ResMut<Game>, mut frames: Local<u32>) {
    use astrocore_game::examine::ExamineMessage;
    use astrocore_map::TransformWorldExt;
    use astrocore_net::{NetEntity, NetEventWorldExt};
    let world = &mut game.0;
    let Some(player) = world.resource::<astrocore_net::ClientNet>().attached else {
        return;
    };
    *frames += 1;
    if *frames != 45 {
        return;
    }
    let Some(here) = world.map_coordinates(player) else {
        return;
    };
    let mut candidates = world
        .query_filtered::<(bevy_ecs::entity::Entity, &NetEntity), With<astrocore_input::Clickable>>(
        );
    let nearest = candidates
        .iter(world)
        .filter(|(entity, _)| *entity != player)
        .filter_map(|(entity, net)| {
            Some((
                world
                    .map_coordinates(entity)?
                    .position
                    .distance(here.position),
                *net,
            ))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, net)) = nearest {
        world.send_net_event_to_server(&ExamineMessage::Request(net));
    }
}

/// Проверка без человека: всплывающее сообщение над игроком перед снимком.
fn popup_before_capture(mut game: ResMut<Game>, mut frames: Local<u32>) {
    use astrocore_game::popups::{Popup, PopupPlace, PopupQueue, PopupType};
    let world = &mut game.0;
    let Some(player) = world.resource::<astrocore_net::ClientNet>().attached else {
        return;
    };
    *frames += 1;
    if *frames != 80 {
        return;
    }
    world.resource_mut::<PopupQueue>().0.push(Popup {
        message: "Проверка всплывающего сообщения".into(),
        kind: PopupType::Medium,
        place: PopupPlace::Entity(player),
    });
}

/// Проверка без человека: игрок появляется у хранилища с окном, лежащего на полу.
fn stand_by_storage(server: &mut bevy_ecs::world::World) {
    use astrocore_ecs::GameWorldExt;
    use astrocore_game::storage::Storage;
    use astrocore_map::{ContainerWorldExt, MapCoordinates, TransformWorldExt};
    let mut storages = server.query_filtered::<bevy_ecs::entity::Entity, (
        With<Storage>,
        With<astrocore_game::item::Item>,
        With<astrocore_ui::UserInterface>,
    )>();
    let mut found: Vec<_> = storages
        .iter(server)
        .filter(|storage| server.containing_container(*storage).is_none())
        .collect();
    found.sort_unstable();
    if let Some(at) = found
        .first()
        .and_then(|storage| server.map_coordinates(*storage))
    {
        let beside = MapCoordinates {
            position: at.position + astrocore_math::Vec2::new(0.0, -1.0),
            ..at
        };
        server.insert_resource(server::ArrivalPoint(beside));
    }
    server.add_game_systems(astrocore_ecs::Update, open_storage_nearest);
}

/// Через секунду после появления сервер кладёт инструменты в ближайшее хранилище, открывает
/// игроку его окно и открывает ближайший шкаф.
fn open_storage_nearest(world: &mut bevy_ecs::world::World, mut ticks: Local<u32>) {
    use astrocore_ecs::LifecycleWorldExt;
    use astrocore_game::entity_storage::{EntityStorage, EntityStorageWorldExt};
    use astrocore_game::storage::{STORAGE_UI_KEY, Storage, StorageWorldExt};
    use astrocore_map::{ContainerWorldExt, SpawnWorldExt, TransformWorldExt};
    use astrocore_ui::UiWorldExt;
    let Some(player) = world
        .query_filtered::<bevy_ecs::entity::Entity, With<astrocore_net::Actor>>()
        .iter(world)
        .next()
    else {
        return;
    };
    *ticks += 1;
    if *ticks != 30 {
        return;
    }
    let Some(here) = world.map_coordinates(player) else {
        return;
    };
    let nearest = |world: &mut bevy_ecs::world::World, entities: Vec<bevy_ecs::entity::Entity>| {
        entities
            .into_iter()
            .filter(|entity| world.containing_container(*entity).is_none())
            .filter_map(|entity| {
                let distance = world
                    .map_coordinates(entity)?
                    .position
                    .distance(here.position);
                Some((distance, entity))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, entity)| entity)
    };
    let storages: Vec<_> = world
        .query_filtered::<bevy_ecs::entity::Entity, (With<Storage>, With<astrocore_ui::UserInterface>)>()
        .iter(world)
        .collect();
    if let Some(storage) = nearest(world, storages) {
        for tool in ["Crowbar", "Wrench", "Screwdriver", "Wirecutter", "Welder"] {
            if let Some(at) = world.coordinates(storage)
                && let Ok(item) = world.spawn_attached(tool, at)
                && !world.storage_insert(storage, item, None, false)
            {
                world.delete_entity(item);
            }
        }
        if !world.open_ui(storage, STORAGE_UI_KEY, player) {
            eprintln!("окно хранилища не открылось");
        }
    }
    let lockers: Vec<_> = world
        .query_filtered::<bevy_ecs::entity::Entity, With<EntityStorage>>()
        .iter(world)
        .collect();
    if let Some(locker) = nearest(world, lockers) {
        world.open_storage(locker, None);
    }
}

/// Проверка без человека: через секунду игрок получает смертельный тупой урон.
fn hurt_player(world: &mut bevy_ecs::world::World, mut ticks: Local<u32>) {
    use astrocore_game::damage::{DamageSpecifier, DamageableWorldExt};
    use astrocore_game::fixed_point::FixedPoint2;
    let Some(player) = world
        .query_filtered::<bevy_ecs::entity::Entity, With<astrocore_net::Actor>>()
        .iter(world)
        .next()
    else {
        return;
    };
    *ticks += 1;
    if *ticks == 30 {
        let damage = DamageSpecifier::new([("Blunt", FixedPoint2::new(500))]);
        world.change_damage(player, &damage, true, true, None);
    }
}

fn hold_right(mut game: ResMut<Game>) {
    if let Some(mut input) = game.0.get_resource_mut::<LocalInput>() {
        input.buttons = MoveButtons::RIGHT;
    }
}

fn run() -> Result<(), String> {
    let options = options()?;
    let transport: Box<dyn ClientTransport> = if options.host {
        let mut start = server::start()?;
        if options.use_nearest {
            stand_by_device(&mut start.world);
        }
        if options.storage_nearest {
            stand_by_storage(&mut start.world);
        }
        if options.hurt {
            use astrocore_ecs::GameWorldExt;
            start
                .world
                .add_game_systems(astrocore_ecs::Update, hurt_player);
        }
        println!(
            "Игра в одиночку: сущностей на карте {}.",
            start.map.entities
        );
        Box::new(LocalHost {
            server: start.world,
        })
    } else {
        let address = match &options.connect {
            Some(text) => text
                .parse()
                .map_err(|_| format!("--connect: «{text}» — не адрес вида 127.0.0.1:7783"))?,
            None => configured_address()?,
        };
        // Номер клиента различает соединения на сервере.
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |time| time.as_nanos() as u64 | 1);
        println!("Соединение с {address}…");
        Box::new(ClientLink::connect(
            address,
            id,
            configured_key()?,
            u32::from(astrocore_ecs::DEFAULT_TICK_RATE),
        )?)
    };
    let (world, resources) = client::start(&options.name)?;
    let bindings = client::bindings(&resources);

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Astrocore".into(),
                    resolution: WindowResolution::new(1280, 720),
                    ..default()
                }),
                ..default()
            })
            .set(ImagePlugin::default_nearest()),
    )
    .insert_resource(ClearColor(Color::BLACK))
    .insert_resource(Game(world))
    .insert_resource(bindings)
    .insert_resource(GameResources(resources))
    .insert_non_send(Link(transport))
    .add_plugins(ClientPlugin)
    .add_plugins(astrocore_game::hands_hud::HandsHudPlugin)
    .add_plugins(astrocore_game::overlays::OverlaysPlugin)
    .add_plugins(astrocore_game::inventory_hud::InventoryHudPlugin)
    .add_plugins(astrocore_game::storage_hud::StorageHudPlugin);
    if options.walk {
        app.add_systems(Update, hold_right);
    }
    if options.use_nearest {
        app.add_systems(Update, use_nearest);
    }
    if options.examine_nearest {
        app.add_systems(Update, (examine_nearest, popup_before_capture));
    }
    if let Some(path) = options.capture {
        app.insert_resource(Capture {
            path: path.into(),
            after_frames: 90,
        });
    }
    app.run();
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
