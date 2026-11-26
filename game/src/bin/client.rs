//! Клиент игры.
//!
//! `cargo station` — игра в одиночку: сервер работает в процессе клиента.
//! `cargo client -- --connect 203.0.113.5:7783 --name Urist` — вход на выделенный сервер.
//! Управление: WASD или стрелки, Shift — шаг.
//!
//! Для проверки без человека: `--capture кадр.png` снимает кадр и завершает программу,
//! `--walk` ведёт персонажа вправо.

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
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        host: false,
        connect: None,
        name: "Urist".to_owned(),
        capture: None,
        walk: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{arg}: нужно значение — {what}"));
        match arg.as_str() {
            "--host" => options.host = true,
            "--walk" => options.walk = true,
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

fn hold_right(mut game: ResMut<Game>) {
    if let Some(mut input) = game.0.get_resource_mut::<LocalInput>() {
        input.buttons = MoveButtons::RIGHT;
    }
}

fn run() -> Result<(), String> {
    let options = options()?;
    let transport: Box<dyn ClientTransport> = if options.host {
        let start = server::start()?;
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
    .insert_resource(GameResources(resources))
    .insert_non_send(Link(transport))
    .add_plugins(ClientPlugin);
    if options.walk {
        app.add_systems(Update, hold_right);
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
