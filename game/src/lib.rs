//! Игра Astrocore на ядре сущностей движка (ADR-0015): сервер и клиент читают прототипы,
//! карты и картинки Space Station 14 из каталога ресурсов.
//!
//! Сейчас игрок входит на карту, ходит и видит станцию. Игровые системы SS14 переезжают
//! сюда по плану `docs/ROADMAP.md` (фазы 3–7); сценарий Relay Two на первой 2D-основе живёт
//! в каталоге `2d/` до конца переезда.

pub mod access;
pub mod action_blocker;
pub mod actions;
pub mod activatable_ui;
pub mod admin_commands;
pub mod admin_logs;
pub mod admin_logs_eui;
pub mod admin_manager;
pub mod admin_system;
pub mod admins;
pub mod airtight;
pub mod ame;
pub mod anchorable;
pub mod atmos;
pub mod atmos_alarms;
pub mod atmos_devices;
pub mod atmos_portables;
pub mod bans;
pub mod barotrauma;
pub mod body;
pub mod bwoink;
pub mod cables;
pub mod character_editor;
pub mod chat;
pub mod combat;
pub mod connection;
pub mod construction;
pub mod container_fill;
pub mod containment;
pub mod content;
pub mod damage;
pub mod damage_other_on_hit;
pub mod database;
pub mod decals;
pub mod destructible;
pub mod device_linking;
pub mod device_network;
pub mod do_after;
pub mod doors;
pub mod electrocution;
pub mod entity_storage;
pub mod examine;
pub mod explosion;
pub mod firelock;
pub mod fixed_point;
pub mod flammable;
pub mod flatpack;
pub mod floor_tile;
pub mod friction;
pub mod game_maps;
pub mod game_ticker;
pub mod gas_analyzer;
pub mod gas_tank;
pub mod ghost;
pub mod ghost_roles;
pub mod gravity;
pub mod guns;
pub mod hands;
pub mod healing;
pub mod id_console;
pub mod input_contexts;
pub mod interaction;
pub mod internals;
pub mod inventory;
pub mod item;
pub mod item_slots;
pub mod lights;
pub mod lock;
pub mod map_commands;
pub mod mapping_autosave;
pub mod markings;
pub mod mind;
pub mod mobs;
pub mod movement_speed;
pub mod multipart;
pub mod network_configurator;
pub mod node_container;
pub mod ordered;
pub mod particle_accelerator;
pub mod pipes;
pub mod placement;
pub mod popups;
pub mod power;
pub mod preferences;
pub mod profiles;
pub mod projectiles;
pub mod pulling;
pub mod radiation;
pub mod radio;
pub mod random;
pub mod rate_limit;
pub mod respawn;
pub mod respiration;
pub mod round_end;
pub mod server;
pub mod signal_devices;
pub mod singularity;
pub mod stack;
pub mod standing;
pub mod station_spawning;
pub mod stations;
pub mod status_effects;
pub mod storage;
pub mod tag;
pub mod temperature;
pub mod throwing;
pub mod tools;
pub mod verbs;
pub mod virtual_item;
pub mod voting;
pub mod weldable;
pub mod whitelist;
pub mod wires;

#[cfg(test)]
pub(crate) mod testing;

#[cfg(feature = "client")]
pub mod action_bar;
#[cfg(feature = "client")]
pub mod admin_ui;
#[cfg(feature = "client")]
pub mod ahelp_ui;
#[cfg(feature = "client")]
pub mod alarm_visuals;
#[cfg(feature = "client")]
pub mod body_visuals;
#[cfg(feature = "client")]
pub mod cable_visuals;
#[cfg(feature = "client")]
pub mod character_ui;
#[cfg(feature = "client")]
pub mod chat_ui;
#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod clothing_visuals;
#[cfg(feature = "client")]
pub mod construction_ui;
#[cfg(feature = "client")]
pub mod decal_visuals;
#[cfg(feature = "client")]
pub mod dev_console;
#[cfg(feature = "client")]
pub mod door_visuals;
#[cfg(feature = "client")]
pub mod explosion_visuals;
#[cfg(feature = "client")]
pub mod fire_visuals;
#[cfg(feature = "client")]
pub mod gas_overlay_visuals;
#[cfg(feature = "client")]
pub mod ghost_ui;
#[cfg(feature = "client")]
pub mod hands_hud;
#[cfg(feature = "client")]
pub mod inhands;
#[cfg(feature = "client")]
pub mod inventory_hud;
#[cfg(feature = "client")]
pub mod light_visuals;
#[cfg(feature = "client")]
pub mod lobby_ui;
#[cfg(feature = "client")]
pub mod mapping_ui;
#[cfg(feature = "client")]
pub mod mob_visuals;
#[cfg(feature = "client")]
pub mod network_configurator_ui;
#[cfg(feature = "client")]
pub mod overlays;
#[cfg(feature = "client")]
pub mod power_ui;
#[cfg(feature = "client")]
pub mod smooth;
#[cfg(feature = "client")]
pub mod stack_visuals;
#[cfg(feature = "client")]
pub mod storage_hud;
#[cfg(feature = "client")]
pub mod storage_visuals;
#[cfg(feature = "client")]
pub mod vote_ui;
#[cfg(feature = "client")]
pub mod wires_ui;

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
    damage::install(world, side);
    standing::install(world, side);
    mobs::install(world, side);
    healing::install(world, side);
    stack::install(world, side);
    tools::install(world, side);
    virtual_item::install(world, side);
    pulling::install(world, side);
    actions::install(world, side);
    combat::install(world, side);
    body::install(world, side);
    damage_other_on_hit::install(world, side);
    map_commands::install(world, side);
    mapping_autosave::install(world, side);
    placement::install(world, side);
    decals::install(world, side);
    node_container::install(world, side);
    power::install(world, side);
    container_fill::install(world, side);
    item_slots::install(world, side);
    flatpack::install(world, side);
    ame::install(world, side);
    cables::install(world, side);
    access::install(world, side);
    lock::install(world, side);
    lights::install(world, side);
    gravity::install(world, side);
    projectiles::install(world, side);
    guns::install(world, side);
    containment::install(world, side);
    gas_tank::install(world, side);
    radiation::install(world, side);
    singularity::install(world, side);
    destructible::install(world, side);
    explosion::install(world, side);
    multipart::install(world, side);
    particle_accelerator::install(world, side);
    id_console::install(world, side);
    airtight::install(world, side);
    atmos::install(world, side);
    barotrauma::install(world, side);
    temperature::install(world, side);
    flammable::install(world, side);
    respiration::install(world, side);
    internals::install(world, side);
    pipes::install(world, side);
    database::install(world, side);
    bans::install(world);
    admin_logs::install(world, side);
    rate_limit::install(world, side);
    admin_manager::install(world, side);
    admin_commands::install(world, side);
    admin_system::install(world, side);
    admin_logs_eui::install(world, side);
    bwoink::install(world, side);
    profiles::install(world);
    markings::install(world);
    preferences::install(world, side);
    stations::install(world, side);
    station_spawning::install(world);
    game_maps::install(world, side);
    game_ticker::install(world, side);
    voting::install(world, side);
    round_end::install(world, side);
    mind::install(world, side);
    ghost::install(world, side);
    ghost_roles::install(world, side);
    respawn::install(world, side);
    chat::install(world, side);
    radio::install(world, side);
    connection::install(world, side);
    device_network::install(world, side);
    device_linking::install(world, side);
    network_configurator::install(world, side);
    signal_devices::install(world, side);
    atmos_devices::install(world, side);
    atmos_portables::install(world, side);
    gas_analyzer::install(world, side);
    atmos_alarms::install(world, side);
    firelock::install(world, side);
    weldable::install(world, side);
    status_effects::install(world, side);
    electrocution::install(world, side);
    doors::install(world, side);
    wires::install(world, side);
    construction::install(world, side);
    anchorable::install(world, side);
    floor_tile::install(world, side);
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
