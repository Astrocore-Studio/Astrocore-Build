//! Выбор карты раунда: карта из `game.map` (если задана) или выбранная голосованием и
//! командой; иначе — по правилам: из пула карт (`game.map_pool`, у режима — свой пул)
//! подходящие по числу игроков, а если таких нет — запасные (`fallback`). С ротацией
//! (`game.map_rotation`) берётся карта, сыгранная давнее всех среди последних
//! `game.map_memory_depth`; без неё — случайная. Выбор забывается при перезапуске раунда.
//! Образец — `GameMapManager`, `GameMapPoolPrototype`, `ForceMapCommand`,
//! `ServerGameTicker.IsMapEligible`, `ValidateMap` SS14 @ 2f4a2aad.
//!
//! Отличия: `game.map` по умолчанию — `Saltern` (в SS14 пусто), а значение может быть и
//! путём файла карты; условий карт (`conditions`, например праздничных) нет — в пулах SS14
//! их нет; постоянства карты между раундами (`UsePersistence`) нет.

use std::collections::VecDeque;

use astrocore_console::{
    CVarChanged, CVarDef, CVarFlags, Config, ConfigWorldExt, ConsoleCommand, ConsoleShell,
    ConsoleWorldExt,
};
use astrocore_data::DataNode;
use astrocore_ecs::EventWorldExt;
use astrocore_loc::LocArg;
use astrocore_net::Session;
use astrocore_prototypes::Prototypes;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::Side;
use crate::examine::loc;
use crate::stations::{GameMap, game_map};

pub const GAME_MAP_POOL: CVarDef<&str> = CVarDef::new(
    "game.map_pool",
    "DefaultMapPool",
    CVarFlags::SERVERONLY,
    "Пул карт, из которого выбирается карта раунда.",
);
pub const GAME_MAP_MEMORY_DEPTH: CVarDef<i32> = CVarDef::new(
    "game.map_memory_depth",
    16,
    CVarFlags::SERVERONLY,
    "Сколько последних карт помнит ротация.",
);
pub const GAME_MAP_ROTATION: CVarDef<bool> = CVarDef::new(
    "game.map_rotation",
    true,
    CVarFlags::SERVERONLY,
    "Ротация карт: давно не игранная, иначе — случайная.",
);

/// Выбор карты. Ресурс сервера.
#[derive(Resource, Debug, Default, Clone)]
pub struct GameMaps {
    /// Сыгранные карты, старые первыми (`_previousMaps`).
    pub previous: VecDeque<String>,
    /// Выбранная голосованием или правилами (`_selectedMap`); не меняется во время раунда.
    pub selected: Option<String>,
}

fn prototypes(world: &World) -> Option<&Prototypes> {
    world.get_resource::<Prototypes>()
}

fn player_count(world: &mut World) -> i32 {
    world.query::<&Session>().iter(world).count() as i32
}

/// Карты пула (`AllVotableMaps`): пул режима, иначе — из `game.map_pool`.
pub fn all_votable_maps(world: &World) -> Vec<GameMap> {
    let preset = crate::game_ticker::preset_of(
        world,
        world
            .get_resource::<crate::game_ticker::GameTicker>()
            .and_then(|t| t.preset.clone())
            .as_deref(),
    );
    let pool = preset
        .and_then(|p| p.map_pool)
        .unwrap_or_else(|| world.resource::<Config>().get_text(&GAME_MAP_POOL));
    let Some(prototypes) = prototypes(world) else {
        return Vec::new();
    };
    let Some(node) = prototypes.resolved_node("gameMapPool", &pool) else {
        eprintln!("пул карт {pool} не найден");
        return Vec::new();
    };
    node.get("maps")
        .and_then(DataNode::as_seq)
        .unwrap_or_default()
        .iter()
        .filter_map(DataNode::as_str)
        .filter_map(|id| {
            let map = game_map(prototypes, id);
            if map.is_none() {
                eprintln!("карта {id} пула {pool} не найдена");
            }
            map
        })
        .collect()
}

/// Все карты (`AllMaps`).
pub fn all_maps(world: &World) -> Vec<GameMap> {
    let Some(prototypes) = prototypes(world) else {
        return Vec::new();
    };
    let mut ids: Vec<&str> = prototypes.ids("gameMap").collect();
    ids.sort_unstable();
    ids.into_iter()
        .filter_map(|id| game_map(prototypes, id))
        .collect()
}

/// Подходит ли карта (`IsMapEligible`): число игроков и пул режима.
fn is_map_eligible(world: &mut World, map: &GameMap) -> bool {
    let players = player_count(world);
    if map.max_players < players || map.min_players > players {
        return false;
    }
    // `ServerGameTicker.IsMapEligible`: карта из пула режима, если он у режима есть.
    let preset_pool = crate::game_ticker::preset_of(
        world,
        world
            .get_resource::<crate::game_ticker::GameTicker>()
            .and_then(|t| t.preset.clone())
            .as_deref(),
    )
    .and_then(|p| p.map_pool);
    match preset_pool {
        Some(pool) => prototypes(world)
            .and_then(|p| p.resolved_node("gameMapPool", &pool))
            .and_then(|n| n.get("maps"))
            .and_then(DataNode::as_seq)
            .is_some_and(|maps| maps.iter().any(|m| m.as_str() == Some(&map.id))),
        None => true,
    }
}

/// Подходящие сейчас карты (`CurrentlyEligibleMaps`); нет таких — запасные.
pub fn currently_eligible_maps(world: &mut World) -> Vec<GameMap> {
    let maps: Vec<GameMap> = all_votable_maps(world)
        .into_iter()
        .filter(|map| is_map_eligible(world, map))
        .collect();
    if maps.is_empty() {
        all_maps(world).into_iter().filter(|m| m.fallback).collect()
    } else {
        maps
    }
}

/// Карта из `game.map`, если это прототип `gameMap` (`_configSelectedMap`).
fn config_map(world: &World) -> Option<GameMap> {
    let setting = world.resource::<Config>().get_text(&crate::content::MAP);
    if setting.is_empty() {
        return None;
    }
    game_map(prototypes(world)?, &setting)
}

/// Выбранная карта (`GetSelectedMap`): из настройки, иначе выбранная.
pub fn selected_map(world: &World) -> Option<GameMap> {
    config_map(world).or_else(|| {
        let id = world.get_resource::<GameMaps>()?.selected.clone()?;
        game_map(prototypes(world)?, &id)
    })
}

/// Есть ли такая карта (`CheckMapExists`).
pub fn check_map_exists(world: &World, id: &str) -> bool {
    prototypes(world).is_some_and(|p| game_map(p, id).is_some())
}

/// Выбрать карту (`SelectMap`).
pub fn select_map(world: &mut World, id: &str) -> bool {
    if !check_map_exists(world, id) {
        return false;
    }
    world
        .get_resource_or_insert_with(GameMaps::default)
        .selected = Some(id.to_owned());
    true
}

/// Забыть выбор (`ClearSelectedMap`).
pub fn clear_selected_map(world: &mut World) {
    world
        .get_resource_or_insert_with(GameMaps::default)
        .selected = None;
}

/// Случайная подходящая карта (`SelectMapRandom`).
pub fn select_map_random(world: &mut World) {
    let maps = currently_eligible_maps(world);
    if maps.is_empty() {
        return;
    }
    let index = crate::random::random(world).range(0, maps.len() as i32) as usize;
    world
        .get_resource_or_insert_with(GameMaps::default)
        .selected = Some(maps[index].id.clone());
}

/// Насколько давно карта сыграна (`GetMapRotationQueuePriority`): чем больше, тем давнее;
/// не игранная — глубина памяти.
fn rotation_priority(world: &World, id: &str) -> usize {
    let depth = world.cvar(&GAME_MAP_MEMORY_DEPTH).max(0) as usize;
    world
        .get_resource::<GameMaps>()
        .and_then(|maps| maps.previous.iter().rev().position(|m| m == id))
        .unwrap_or(depth)
}

/// Первая в очереди ротации (`GetFirstInRotationQueue`): давнее всех сыгранная, при равенстве
/// — первая по идентификатору.
pub fn first_in_rotation_queue(world: &mut World) -> Option<GameMap> {
    let eligible = currently_eligible_maps(world);
    let best = eligible
        .iter()
        .map(|m| rotation_priority(world, &m.id))
        .max()?;
    eligible
        .into_iter()
        .filter(|m| rotation_priority(world, &m.id) == best)
        .min_by(|a, b| a.id.cmp(&b.id))
}

/// Запомнить сыгранную (`EnqueueMap`).
fn enqueue_map(world: &mut World, id: &str) {
    let depth = world.cvar(&GAME_MAP_MEMORY_DEPTH).max(0) as usize;
    let mut maps = world.get_resource_or_insert_with(GameMaps::default);
    maps.previous.push_back(id.to_owned());
    while maps.previous.len() > depth {
        maps.previous.pop_front();
    }
}

/// Карта по правилам (`SelectMapByConfigRules`): ротация или случайная.
pub fn select_map_by_config_rules(world: &mut World) {
    if world.cvar(&GAME_MAP_ROTATION) {
        let Some(map) = first_in_rotation_queue(world) else {
            return;
        };
        world
            .get_resource_or_insert_with(GameMaps::default)
            .selected = Some(map.id.clone());
        enqueue_map(world, &map.id);
    } else {
        select_map_random(world);
    }
}

/// Режим сменился: выбранная карта не из его пула — случайная (`ValidateMap`).
pub fn validate_map(world: &mut World) {
    let Some(map) = selected_map(world) else {
        return;
    };
    let pool = crate::game_ticker::preset_of(
        world,
        world
            .get_resource::<crate::game_ticker::GameTicker>()
            .and_then(|t| t.preset.clone())
            .as_deref(),
    )
    .and_then(|p| p.map_pool);
    let Some(pool) = pool else {
        return;
    };
    let in_pool = prototypes(world)
        .and_then(|p| p.resolved_node("gameMapPool", &pool))
        .and_then(|n| n.get("maps"))
        .and_then(DataNode::as_seq)
        .is_some_and(|maps| maps.iter().any(|m| m.as_str() == Some(&map.id)));
    if !in_pool {
        select_map_random(world);
    }
}

/// Начальная память ротации (`Initialize`): карты пула в случайном порядке.
fn fill_rotation_memory(world: &mut World) {
    let mut maps: Vec<String> = all_votable_maps(world).into_iter().map(|m| m.id).collect();
    // Перемешивание Фишера — Йейтса генератором мира.
    for i in (1..maps.len()).rev() {
        let j = crate::random::random(world).range(0, i as i32 + 1) as usize;
        maps.swap(i, j);
    }
    let depth = world.cvar(&GAME_MAP_MEMORY_DEPTH).max(0) as usize;
    let mut state = world.get_resource_or_insert_with(GameMaps::default);
    state.previous = maps.into_iter().take(depth).collect();
}

/// `forcemap <карта>`: карта следующего раунда в `game.map`; пустая строка снимает выбор.
fn forcemap(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let [name] = args else {
        shell.write_line(loc(world, "shell-need-exactly-one-argument", &[]));
        return;
    };
    if !name.is_empty() && !check_map_exists(world, name) {
        shell.write_line(loc(
            world,
            "cmd-forcemap-map-not-found",
            &[("map", LocArg::from(name.as_str()))],
        ));
        return;
    }
    if let Err(error) = world.set_cvar_text(crate::content::MAP.name, name) {
        shell.write_error(error.to_string());
        return;
    }
    if name.is_empty() {
        shell.write_line(loc(world, "cmd-forcemap-cleared", &[]));
    } else {
        shell.write_line(loc(
            world,
            "cmd-forcemap-success",
            &[("map", LocArg::from(name.as_str()))],
        ));
    }
}

pub fn install(world: &mut World, side: Side) {
    if !side.is_server() {
        return;
    }
    world.register_cvar(&crate::content::MAP);
    world.register_cvar(&GAME_MAP_POOL);
    world.register_cvar(&GAME_MAP_MEMORY_DEPTH);
    world.register_cvar(&GAME_MAP_ROTATION);
    world.init_resource::<GameMaps>();
    world.register_command(ConsoleCommand {
        name: "forcemap",
        description: "Задаёт карту следующего раунда; пустая строка снимает выбор.",
        help: "forcemap <карта>",
        execute: forcemap,
        complete: None,
    });
    // Меньшая глубина памяти — лишнее забывается.
    world.subscribe_broadcast::<CVarChanged>(|world, event| {
        if event.name != GAME_MAP_MEMORY_DEPTH.name {
            return;
        }
        let depth = world.cvar(&GAME_MAP_MEMORY_DEPTH).max(0) as usize;
        let mut maps = world.resource_mut::<GameMaps>();
        while maps.previous.len() > depth {
            maps.previous.pop_front();
        }
    });
    // Прототипы загружаются после установки: память ротации заполняется с первым раундом.
    world.subscribe_broadcast::<crate::game_ticker::RoundRestartCleanup>(|world, _| {
        clear_selected_map(world);
    });
}

/// Заполнить память ротации, если она пуста (при первом выборе карты).
pub fn ensure_rotation_memory(world: &mut World) {
    let empty = world
        .get_resource::<GameMaps>()
        .is_none_or(|m| m.previous.is_empty());
    let filled = world.get_resource::<GameMapsFilled>().is_some_and(|f| f.0);
    if empty && !filled {
        fill_rotation_memory(world);
        world.insert_resource(GameMapsFilled(true));
    }
}

/// Память ротации уже заполнялась.
#[derive(Resource, Debug, Default)]
struct GameMapsFilled(bool);

#[cfg(test)]
mod tests;
