//! Игра Astrocore на ядре сущностей движка (ADR-0015): сервер и клиент читают прототипы,
//! карты и картинки Space Station 14 из каталога ресурсов.
//!
//! Сейчас игрок входит на карту, ходит и видит станцию. Игровые системы SS14 переезжают
//! сюда по плану `docs/ROADMAP.md` (фазы 3–7); сценарий Relay Two на первой 2D-основе живёт
//! в каталоге `2d/` до конца переезда.

pub mod action_blocker;
pub mod activatable_ui;
pub mod content;
pub mod do_after;
pub mod entity_storage;
pub mod examine;
pub mod friction;
pub mod grammar;
pub mod hands;
pub mod input_contexts;
pub mod interaction;
pub mod inventory;
pub mod item;
pub mod ordered;
pub mod popups;
pub mod server;
pub mod storage;
pub mod tag;
pub mod throwing;
pub mod verbs;
pub mod whitelist;

#[cfg(test)]
pub(crate) mod testing;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod clothing_visuals;
#[cfg(feature = "client")]
pub mod hands_hud;
#[cfg(feature = "client")]
pub mod inhands;
#[cfg(feature = "client")]
pub mod inventory_hud;
#[cfg(feature = "client")]
pub mod overlays;
#[cfg(feature = "client")]
pub mod smooth;
#[cfg(feature = "client")]
pub mod storage_hud;
#[cfg(feature = "client")]
pub mod storage_visuals;

use bevy_ecs::world::World;

/// Сторона, на которой работает мир игры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Решает исход действий.
    Server,
    /// Показывает мир и передаёт намерения игрока.
    Client,
}

impl Side {
    pub fn is_server(self) -> bool {
        self == Side::Server
    }
}

/// Системы контента SS14 (ADR-0005): компоненты регистрируются одинаково на обеих сторонах,
/// логика, меняющая мир, — только на сервере.
pub fn install_content(world: &mut World, side: Side) {
    tag::install(world);
    item::install(world);
    action_blocker::install(world);
    interaction::install(world, side);
    activatable_ui::install(world, side);
    hands::install(world, side);
    throwing::install(world, side);
    friction::install(world, side);
    popups::install(world);
    do_after::install(world, side);
    verbs::install(world);
    examine::install(world, side);
    inventory::install(world, side);
    storage::install(world, side);
    entity_storage::install(world, side);
}

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
