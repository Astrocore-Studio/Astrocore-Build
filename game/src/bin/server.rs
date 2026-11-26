//! Выделенный сервер игры.

use std::process::ExitCode;
use std::time::Instant;

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
    let mut next = Instant::now();
    let mut players = 0;
    loop {
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

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
