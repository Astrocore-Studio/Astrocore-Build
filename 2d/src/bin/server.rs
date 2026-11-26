use astrocore_build_2d::{endpoint, server};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let (address, key) = endpoint()?;
    let mut host = server::start(address, key)?;
    println!("Astrocore 2D: {}", host.local_addr());
    server::run(&mut host, || true)
}
