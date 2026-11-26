use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Instant;

use astrocore_grid2d::session::{Host, TICK_PERIOD};

use crate::Station;

pub fn start(address: SocketAddr, key: [u8; 32]) -> Result<Host<Station>, String> {
    let directory = std::env::var_os("ASTRO_2D_CONTENT").map(PathBuf::from);
    Host::start(address, key, Station::load(directory.as_deref())?)
}

pub fn run(host: &mut Host<Station>, mut running: impl FnMut() -> bool) -> Result<(), String> {
    let mut deadline = Instant::now();
    while running() {
        host.step()?;
        deadline += TICK_PERIOD;
        if let Some(delay) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(delay);
        } else {
            deadline = Instant::now();
        }
    }
    Ok(())
}

#[cfg(feature = "client")]
mod local {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::sync_channel;
    use std::thread::{Builder, JoinHandle};

    pub struct LocalServer {
        pub address: SocketAddr,
        running: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl LocalServer {
        pub fn launch(address: SocketAddr, key: [u8; 32]) -> Result<Self, String> {
            let running = Arc::new(AtomicBool::new(true));
            let proceed = Arc::clone(&running);
            let (ready, result) = sync_channel(1);
            let worker = Builder::new()
                .name("astrocore-local-server".into())
                .spawn(move || {
                    let mut host = match super::start(address, key) {
                        Ok(host) => host,
                        Err(error) => {
                            let _ = ready.send(Err(error));
                            return;
                        }
                    };
                    println!("Astrocore 2D: {}", host.local_addr());
                    if ready.send(Ok(host.local_addr())).is_ok()
                        && let Err(error) =
                            super::run(&mut host, || proceed.load(Ordering::Relaxed))
                    {
                        eprintln!("Сервер Astrocore 2D: {error}");
                    }
                })
                .map_err(|error| format!("Не удалось запустить сервер: {error}"))?;

            match result.recv() {
                Ok(Ok(mut address)) => {
                    if address.ip().is_unspecified() {
                        let loopback = if address.is_ipv4() {
                            IpAddr::V4(Ipv4Addr::LOCALHOST)
                        } else {
                            IpAddr::V6(Ipv6Addr::LOCALHOST)
                        };
                        address.set_ip(loopback);
                    }
                    Ok(Self {
                        address,
                        running,
                        worker: Some(worker),
                    })
                }
                Ok(Err(error)) => {
                    let _ = worker.join();
                    Err(error)
                }
                Err(_) => {
                    let _ = worker.join();
                    Err("сервер завершился до подключения клиента".into())
                }
            }
        }
    }

    impl Drop for LocalServer {
        fn drop(&mut self) {
            self.running.store(false, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

#[cfg(feature = "client")]
pub use local::LocalServer;
