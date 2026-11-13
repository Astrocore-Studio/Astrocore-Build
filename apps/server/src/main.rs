use std::process::ExitCode;
use std::time::Duration;

use round::{Round, RoundConfig};

const USAGE: &str = "\
server — сервер Astrocore Build (раунд на нейтральном полигоне)

  server --listen 127.0.0.1:7779 --key <hex32> [--tick-hz 20] [--ticks N] [--map карта.ron] [--lobby секунд, 0 = без лобби]
  server --write-manifest <файл.json>   записать манифест рендера для клиента и выйти
";

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") || args.is_empty() {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let content = polygon::content_dir();
    if let Some(path) = arg(args, "--write-manifest") {
        let registry = polygon::load_registry(&content).map_err(|e| e.to_string())?;
        let json = polygon::render_manifest(&content, &registry).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| format!("{path}: {e}"))?;
        println!("манифест рендера записан: {path}");
        return Ok(());
    }
    if let Some(map) = arg(args, "--map") {
        polygon::use_map_file(std::path::Path::new(&map));
    }
    let addr: std::net::SocketAddr = arg(args, "--listen")
        .ok_or("нужен --listen")?
        .parse()
        .map_err(|e| format!("--listen: {e}"))?;
    let key = astrocore_network::secure::parse_hex32(&arg(args, "--key").ok_or("нужен --key")?)
        .ok_or("--key: 64 шестнадцатеричных символа")?;
    let tick_hz: u8 = arg(args, "--tick-hz")
        .map_or(Ok(20), |v| v.parse())
        .map_err(|e| format!("--tick-hz: {e}"))?;
    let max_ticks: Option<u64> = arg(args, "--ticks")
        .map(|v| v.parse().map_err(|e| format!("--ticks: {e}")))
        .transpose()?;

    // Лобби: `--lobby <секунд>` (по умолчанию 120, как `game.lobbyduration`; 0 — без лобби).
    let lobby_secs: f32 = arg(args, "--lobby")
        .map_or(Ok(round::DEFAULT_LOBBY_SECS), |v| v.parse())
        .map_err(|e| format!("--lobby: {e}"))?;
    let config = RoundConfig {
        tick_hz,
        lobby_secs: (lobby_secs > 0.0).then_some(lobby_secs),
        ..RoundConfig::default()
    };
    let mut round = Round::start(1, &content, &config).map_err(|e| e.to_string())?;
    // Initialize map-dependent systems before accepting clients. On Saltern the
    // first tick builds networks and resolves spawners; doing it during the
    // handshake can exceed netcode's connection-request timeout.
    println!("подготовка систем карты…");
    let _ = round.server.tick();
    println!("раунд {} начат; слушаю {addr}", round.number);
    let mut host = astrocore_lightyear::LyGameHost::start(addr, key, round.server, tick_hz);
    println!("Lightyear: {}", host.local_addr());
    let mut steps = 0u64;
    while max_ticks.is_none_or(|m| steps < m) {
        host.step();
        steps += 1;
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
