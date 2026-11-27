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
//! наносит игроку смертельный урон, `--dress` (только с `--host`) надевает на игрока
//! комбинезон, ботинки и рюкзак, `--console "<команда>"` (можно несколько раз) после входа
//! открывает консоль и выполняет команды на сервере, `--palette entities:<поиск>` или
//! `tiles:<поиск>` открывает палитру маппинга и берёт первую найденную строку, курсор при
//! этом стоит на две клетки правее игрока; `--construct <рецепт>` открывает меню строительства
//! и показывает призрак рецепта на две клетки ниже игрока.

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
    /// Через сколько кадров снимать (`--capture-after`), по умолчанию 90.
    capture_after: u32,
    walk: bool,
    walk_up: bool,
    use_nearest: bool,
    /// Прототип устройства для `--use-nearest` (`--device`).
    device: Option<String>,
    examine_nearest: bool,
    storage_nearest: bool,
    hurt: bool,
    ignite: bool,
    /// Проверка вида пожарного шлюза: игрок у шлюза, который держит давление и огонь.
    firelock: bool,
    dress: bool,
    console: Vec<String>,
    palette: Option<String>,
    construct: Option<String>,
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        host: false,
        connect: None,
        name: "Urist".to_owned(),
        capture: None,
        capture_after: 90,
        walk: false,
        walk_up: false,
        use_nearest: false,
        device: None,
        examine_nearest: false,
        storage_nearest: false,
        hurt: false,
        ignite: false,
        firelock: false,
        dress: false,
        console: Vec::new(),
        palette: None,
        construct: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{arg}: нужно значение — {what}"));
        match arg.as_str() {
            "--host" => options.host = true,
            "--walk" => options.walk = true,
            "--walk-up" => options.walk_up = true,
            "--use-nearest" => options.use_nearest = true,
            "--device" => options.device = Some(value("прототип устройства")?),
            "--examine-nearest" => options.examine_nearest = true,
            "--storage-nearest" => options.storage_nearest = true,
            "--hurt" => options.hurt = true,
            "--ignite" => options.ignite = true,
            "--firelock" => options.firelock = true,
            "--dress" => options.dress = true,
            "--connect" => options.connect = Some(value("адрес сервера")?),
            "--name" => options.name = value("имя игрока")?,
            "--capture" => options.capture = Some(value("файл кадра")?),
            "--capture-after" => {
                options.capture_after = value("число кадров")?
                    .parse()
                    .map_err(|_| "--capture-after: нужно число кадров".to_owned())?
            }
            "--console" => options.console.push(value("строка команды")?),
            "--palette" => options.palette = Some(value("палитра:поиск")?),
            "--construct" => options.construct = Some(value("рецепт")?),
            other => {
                return Err(format!(
                    "неизвестный параметр {other}. Параметры: --host, --connect <адрес>, --name <имя>, --capture <файл>, --walk, --walk-up"
                ));
            }
        }
    }
    Ok(options)
}

/// Проверка без человека: точка появления переносится к первому устройству с интерфейсом.
fn stand_by_device(server: &mut bevy_ecs::world::World, prototype: Option<&str>) {
    use astrocore_game::activatable_ui::ActivatableUi;
    use astrocore_map::{MapCoordinates, TransformWorldExt};
    let mut devices = server.query_filtered::<bevy_ecs::entity::Entity, (
        With<ActivatableUi>,
        With<astrocore_ui::UserInterface>,
    )>();
    let mut found: Vec<_> = devices.iter(server).collect();
    if let Some(prototype) = prototype {
        found.retain(|device| {
            server
                .get::<astrocore_ecs::MetaData>(*device)
                .and_then(|m| m.prototype.as_deref())
                == Some(prototype)
        });
    }
    found.sort_unstable();
    if let Some((device, at)) = found
        .first()
        .and_then(|device| Some((*device, server.map_coordinates(*device)?)))
    {
        // Перед лицевой стороной устройства: настенные смотрят «вниз» в своём повороте.
        let rotation = server.world_transform(device).map_or(0.0, |t| t.rotation.0);
        let front = astrocore_math::Vec2::new(rotation.sin(), -rotation.cos());
        let beside = MapCoordinates {
            position: at.position + front,
            ..at
        };
        server.insert_resource(server::ArrivalPoint(beside));
    }
}

/// Проверка без человека: через секунду после появления игрок нажимает «использовать в
/// мире» на ближайшем устройстве с интерфейсом.
/// Прототип устройства для `--use-nearest` (`--device`).
#[derive(Resource, Default)]
struct DeviceFilter(Option<String>);

fn use_nearest(mut game: ResMut<Game>, filter: Res<DeviceFilter>, mut frames: Local<u32>) {
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
        .filter(|device| {
            filter.0.as_deref().is_none_or(|wanted| {
                world
                    .get::<astrocore_ecs::MetaData>(*device)
                    .and_then(|m| m.prototype.as_deref())
                    == Some(wanted)
            })
        })
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

/// Проверка без человека: через секунду игрок горит (четыре «стопки» огня).
fn ignite_player(world: &mut bevy_ecs::world::World, mut ticks: Local<u32>) {
    let Some(player) = world
        .query_filtered::<bevy_ecs::entity::Entity, With<astrocore_net::Actor>>()
        .iter(world)
        .next()
    else {
        return;
    };
    *ticks += 1;
    if *ticks == 30 {
        astrocore_game::flammable::set_fire_stacks(world, player, 4.0, true);
    }
}

/// Пожарный шлюз для `--firelock`.
#[derive(Resource)]
struct HeldFirelock(bevy_ecs::entity::Entity);

/// Клетки по обе стороны шлюза: север и юг.
fn firelock_sides(
    world: &bevy_ecs::world::World,
    firelock: bevy_ecs::entity::Entity,
) -> Option<(
    bevy_ecs::entity::Entity,
    astrocore_map::TileCoord,
    astrocore_map::TileCoord,
)> {
    use astrocore_map::{TileCoord, Transform};
    let transform = world.get::<Transform>(firelock)?;
    let grid = transform.grid()?;
    let cell = TileCoord::containing(transform.local_position(), transform.level());
    Some((
        grid,
        TileCoord::new(cell.x, cell.y + 1, cell.z),
        TileCoord::new(cell.x, cell.y - 1, cell.z),
    ))
}

/// Проверка без человека: игрок появляется рядом с первым пожарным шлюзом, у которого
/// с севера и с юга воздух.
fn stand_by_firelock(server: &mut bevy_ecs::world::World) -> Option<bevy_ecs::entity::Entity> {
    use astrocore_game::doors::Firelock;
    use astrocore_map::{MapCoordinates, TransformWorldExt};
    let mut firelocks: Vec<_> = server
        .query_filtered::<bevy_ecs::entity::Entity, With<Firelock>>()
        .iter(server)
        .collect();
    firelocks.sort_unstable();
    let mut doors =
        server.query_filtered::<bevy_ecs::entity::Entity, With<astrocore_game::doors::Door>>();
    let doors: Vec<_> = doors.iter(server).collect();
    let firelock = firelocks.into_iter().find(|f| {
        let at = server.map_coordinates(*f).map(|c| c.position);
        // Без другой двери на той же клетке: шлюз над пожарным закрыл бы его.
        let alone = doors
            .iter()
            .filter(|d| *d != f)
            .all(|d| server.map_coordinates(*d).map(|c| c.position) != at);
        alone
            && firelock_sides(server, *f).is_some_and(|(grid, north, south)| {
                astrocore_game::atmos::tile_mixture(server, grid, north).is_some()
                    && astrocore_game::atmos::tile_mixture(server, grid, south).is_some()
            })
    })?;
    let at = server.map_coordinates(firelock)?;
    let beside = MapCoordinates {
        position: at.position + astrocore_math::Vec2::new(1.0, 0.0),
        ..at
    };
    server.insert_resource(server::ArrivalPoint(beside));
    Some(firelock)
}

/// Через секунду шлюз закрывается; к северу — горячий воздух, к югу — вакуум.
fn hold_firelock(world: &mut bevy_ecs::world::World, mut ticks: Local<u32>) {
    use astrocore_game::atmos::GasMixture;
    use astrocore_game::atmos::gas::CELL_VOLUME;
    let firelock = world.resource::<HeldFirelock>().0;
    *ticks += 1;
    if *ticks < 30 {
        return;
    }
    if *ticks == 30 {
        astrocore_game::firelock::emergency_pressure_stop(world, firelock);
    }
    let Some((grid, north, south)) = firelock_sides(world, firelock) else {
        return;
    };
    astrocore_game::atmos::with_tile_mixture(world, grid, north, |tile| {
        tile.copy_from(&GasMixture::standard_air(CELL_VOLUME));
        tile.set_temperature(700.0);
    });
    astrocore_game::atmos::with_tile_mixture(world, grid, south, |tile| tile.clear());
}

/// Проверка без человека: через секунду игрок одет.
fn dress_player(world: &mut bevy_ecs::world::World, mut ticks: Local<u32>) {
    use astrocore_game::inventory::{EquipOptions, InventoryWorldExt};
    use astrocore_map::{SpawnWorldExt, TransformWorldExt};
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
    for (prototype, slot) in [
        ("ClothingUniformJumpsuitColorGrey", "jumpsuit"),
        ("ClothingShoesColorBlack", "shoes"),
        ("ClothingBackpack", "back"),
    ] {
        let Some(at) = world.coordinates(player) else {
            return;
        };
        if let Ok(item) = world.spawn_attached(prototype, at) {
            let options = EquipOptions {
                silent: true,
                force: true,
                delay: false,
            };
            world.try_equip(player, player, item, slot, options);
        }
    }
}

/// Команды консоли для проверки без человека: выполняются через полсекунды после входа.
#[derive(Resource)]
struct StartupCommands(Vec<String>);

fn run_startup_commands(
    mut game: ResMut<Game>,
    mut console: ResMut<astrocore_game::dev_console::DevConsole>,
    mut commands: ResMut<StartupCommands>,
    mut frames: Local<u32>,
) {
    if commands.0.is_empty()
        || game
            .0
            .resource::<astrocore_net::ClientNet>()
            .attached
            .is_none()
    {
        return;
    }
    *frames += 1;
    if *frames < 30 {
        return;
    }
    console.open = true;
    for line in std::mem::take(&mut commands.0) {
        console.history.push(line.clone());
        astrocore_net::RemoteConsoleWorldExt::execute_remote_command(&mut game.0, &line);
    }
}

/// Тайлы игры по идентификаторам.
fn game_world_tiles(app: &App) -> Vec<String> {
    app.world()
        .resource::<Game>()
        .0
        .resource::<astrocore_map::TileDefinitions>()
        .iter()
        .map(|definition| definition.id.clone())
        .collect()
}

fn hold_right(mut game: ResMut<Game>) {
    if let Some(mut input) = game.0.get_resource_mut::<LocalInput>() {
        input.buttons = MoveButtons::RIGHT;
    }
}

fn hold_up(mut game: ResMut<Game>) {
    if let Some(mut input) = game.0.get_resource_mut::<LocalInput>() {
        input.buttons = MoveButtons::UP;
    }
}

fn run() -> Result<(), String> {
    let options = options()?;
    let transport: Box<dyn ClientTransport> = if options.host {
        let mut start = server::start()?;
        if options.use_nearest {
            stand_by_device(&mut start.world, options.device.as_deref());
        }
        if options.storage_nearest {
            stand_by_storage(&mut start.world);
        }
        if options.dress {
            use astrocore_ecs::GameWorldExt;
            start
                .world
                .add_game_systems(astrocore_ecs::Update, dress_player);
        }
        if options.hurt {
            use astrocore_ecs::GameWorldExt;
            start
                .world
                .add_game_systems(astrocore_ecs::Update, hurt_player);
        }
        if options.ignite {
            use astrocore_ecs::GameWorldExt;
            start
                .world
                .add_game_systems(astrocore_ecs::Update, ignite_player);
        }
        if options.firelock {
            use astrocore_ecs::GameWorldExt;
            if let Some(firelock) = stand_by_firelock(&mut start.world) {
                start.world.insert_resource(HeldFirelock(firelock));
                start
                    .world
                    .add_game_systems(astrocore_ecs::Update, hold_firelock);
            }
        }
        println!(
            "Игра в одиночку: сущностей на карте {}.",
            start.map.entities
        );
        Box::new(LocalHost::new(start.world))
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
    .add_plugins(astrocore_game::storage_hud::StorageHudPlugin)
    .add_plugins(astrocore_game::action_bar::ActionBarPlugin)
    .add_plugins(astrocore_game::dev_console::DevConsolePlugin)
    .add_plugins(astrocore_game::mapping_ui::MappingUiPlugin)
    .add_plugins(astrocore_game::construction_ui::ConstructionUiPlugin)
    .add_plugins(astrocore_game::decal_visuals::DecalVisualsPlugin)
    .add_plugins(astrocore_game::power_ui::PowerUiPlugin)
    .add_plugins(astrocore_game::wires_ui::WiresUiPlugin);
    if options.walk {
        app.add_systems(Update, hold_right);
    }
    if options.walk_up {
        app.add_systems(Update, hold_up);
    }
    if options.use_nearest {
        app.insert_resource(DeviceFilter(options.device.clone()));
        app.add_systems(Update, use_nearest);
    }
    if options.examine_nearest {
        app.add_systems(Update, (examine_nearest, popup_before_capture));
    }
    if let Some(palette) = &options.palette {
        use astrocore_game::mapping_ui::{DemoCursor, MappingTools, Palette, Tool};
        let (kind, search) = palette.split_once(':').unwrap_or((palette, ""));
        let tiles = kind == "tiles";
        let decals = kind == "decals";
        let first = if decals {
            Some(Tool::Decal(search.to_owned()))
        } else if tiles {
            let definitions = game_world_tiles(&app);
            definitions
                .into_iter()
                .find(|id| id.to_lowercase().contains(&search.to_lowercase()))
                .map(Tool::Tile)
        } else {
            Some(Tool::Entity(search.to_owned()))
        };
        let tools = MappingTools {
            palette: Some(if decals {
                Palette::Decals
            } else if tiles {
                Palette::Tiles
            } else {
                Palette::Entities
            }),
            search: search.to_owned(),
            tool: first.unwrap_or_default(),
            version: 1,
            ..MappingTools::default()
        };
        app.world_mut()
            .resource_mut::<Game>()
            .0
            .insert_resource(tools);
        app.insert_resource(DemoCursor(Vec2::new(2.0, 0.0)));
    }
    if let Some(search) = &options.construct {
        use astrocore_game::construction_ui::ConstructionMenu;
        app.world_mut()
            .resource_mut::<Game>()
            .0
            .insert_resource(ConstructionMenu {
                open: true,
                search: search.clone(),
                placing: Some(search.clone()),
                version: 1,
                ..ConstructionMenu::default()
            });
        app.insert_resource(astrocore_game::mapping_ui::DemoCursor(Vec2::new(0.0, -2.0)));
    }
    if !options.console.is_empty() {
        app.insert_resource(StartupCommands(options.console.clone()));
        app.add_systems(Update, run_startup_commands);
    }
    if let Some(path) = options.capture {
        app.insert_resource(Capture {
            path: path.into(),
            after_frames: options.capture_after,
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
