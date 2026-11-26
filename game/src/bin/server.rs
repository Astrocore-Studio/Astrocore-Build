//! Выделенный сервер игры.

use std::process::ExitCode;
use std::time::Instant;

use std::sync::mpsc;

use astrocore_console::{ConsoleShell, ConsoleWorldExt};
use astrocore_ecs::{GameTime, GameWorldExt};
use astrocore_game::{configured_address, configured_key, server};
use astrocore_net::ServerNet;
use astrocore_transport::ServerLink;

fn run() -> Result<(), String> {
    let address = configured_address()?;
    let key = configured_key()?;
    let started = Instant::now();
    let mut start = server::start()?;
    let period = start.world.resource::<GameTime>().tick_period();
    let tick_rate = u32::from(start.world.resource::<GameTime>().tick_rate);
    let mut link = ServerLink::start(address, key, tick_rate);
    println!(
        "Сервер слушает {}: карта загружена за {:.1?}, сущностей {}, ошибок в прототипах {}.",
        link.local_addr(),
        started.elapsed(),
        start.map.entities,
        start.prototype_errors
    );
    let console = stdin_lines();
    let mut next = Instant::now();
    let mut players = 0;
    loop {
        // Строки терминала — команды консоли сервера (`IConsoleHost` Robust).
        while let Ok(line) = console.try_recv() {
            run_command(&mut start.world, &line);
        }
        link.receive(&mut start.world);
        start.world.run_tick();
        link.send(&mut start.world);
        let now_players = start.world.resource::<ServerNet>().sessions().count();
        if now_players != players {
            players = now_players;
            println!("Игроков: {players}");
        }
        next += period;
        match next.checked_duration_since(Instant::now()) {
            Some(wait) => std::thread::sleep(wait),
            // Сервер не успевает: не копим долг, идём от текущего времени.
            None => next = Instant::now(),
        }
    }
}

/// Строки стандартного ввода из отдельного потока: чтение не задерживает такты.
fn stdin_lines() -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else {
                break;
            };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    receiver
}

fn run_command(world: &mut bevy_ecs::world::World, line: &str) {
    if line.trim().is_empty() {
        return;
    }
    let mut shell = ConsoleShell::server();
    if let Err(error) = world.execute_command(&mut shell, line) {
        shell.write_error(error.to_string());
    }
    for line in shell.output {
        println!("{line}");
    }
    for line in shell.errors {
        eprintln!("{line}");
    }
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
