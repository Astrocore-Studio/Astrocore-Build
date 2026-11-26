//! Игра Astrocore на ядре сущностей движка (ADR-0015): сервер и клиент читают прототипы,
//! карты и картинки Space Station 14 из каталога ресурсов.
//!
//! Сейчас игрок входит на карту, ходит и видит станцию. Игровые системы SS14 переезжают
//! сюда по плану `docs/ROADMAP.md` (фазы 3–7); сценарий Relay Two на первой 2D-основе живёт
//! в каталоге `2d/` до конца переезда.

pub mod content;
pub mod input_contexts;
pub mod interaction;
pub mod server;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod smooth;

/// Адрес сервера по умолчанию.
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:7783";

/// Адрес из переменной окружения `ASTRO_ADDR` или адрес по умолчанию.
pub fn configured_address() -> Result<std::net::SocketAddr, String> {
    let text = std::env::var("ASTRO_ADDR").unwrap_or_else(|_| DEFAULT_ADDRESS.to_owned());
    text.parse()
        .map_err(|_| format!("ASTRO_ADDR: «{text}» — не адрес вида 127.0.0.1:7783"))
}

/// Ключ соединений из переменной окружения `ASTRO_KEY` (64 шестнадцатеричных символа).
/// Без переменной — общеизвестный ключ для игры на своём компьютере.
pub fn configured_key() -> Result<[u8; 32], String> {
    match std::env::var("ASTRO_KEY") {
        Ok(text) => astrocore_transport::parse_hex32(&text)
            .ok_or_else(|| "ASTRO_KEY: нужны 64 шестнадцатеричных символа".to_owned()),
        Err(_) => Ok([0x41; 32]),
    }
}
