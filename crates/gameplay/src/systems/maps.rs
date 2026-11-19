//! Карты мира (ADR-0012 движка) и команды консоли по SS14: `lsmap`, `mapping`, `loadmap`,
//! `savemap`, `rmmap`, `pausemap`, `unpausemap`, `querymappaused`, `mapinit`.
//!
//! Несколько карт живут в одном мире одновременно: карта 0 — станция раунда, остальные
//! загружаются и создаются на ходу. Карта `k` лежит на высоте `k × 1000` м со своими этажами
//! (`maps::floor_id`) и идентификаторами (`maps::retag`). Маппинг идёт на замороженной (на паузе)
//! и неинициализированной карте, как в SS14: в файл ложится чистый авторский контент.

use astrocore_api::HookContext;
use astrocore_core::NetworkEntityId;
use astrocore_math::glam::Vec3;
use astrocore_world::{MapId, WorldCommand};
use maps::{MapFile, map_def};

use crate::admin;
use crate::module::State;

/// Номера карт: 0…999 (этажи карт укладываются в номера этажей мира и сетку атмосферы).
pub const MAX_MAP_ID: u32 = 999;

/// Номер карты из аргумента.
pub fn parse_map(arg: &str) -> Result<MapId, String> {
    match arg.parse::<u32>() {
        Ok(n) if n <= MAX_MAP_ID => Ok(MapId(n)),
        _ => Err(format!("«{arg}» — не номер карты (0…{MAX_MAP_ID}).")),
    }
}

/// Карта, на которой стоит точка мира.
pub fn map_at(p: Vec3) -> MapId {
    maps::map_at_y(p.y)
}

/// Точка появления на карте: заданная при загрузке или центр клетки (0, 0) над уровнем 0.
pub fn spawn_of(st: &State, map: MapId) -> Vec3 {
    if map == MapId(0) {
        return st
            .spawn
            .unwrap_or(Vec3::new(0.5, maps::FLOOR_TOP_M + 0.05, 0.5));
    }
    st.map_spawns.get(&map).copied().unwrap_or(Vec3::new(
        0.5,
        maps::map_base_y(map) + maps::FLOOR_TOP_M + 0.05,
        0.5,
    ))
}

fn read_map(st: &State, name: &str) -> Result<MapFile, String> {
    if !admin::valid_map_name(name) {
        return Err(format!(
            "Имя «{name}»: латиница, цифры, «_» и «-», до {} символов.",
            admin::MAP_NAME_MAX
        ));
    }
    let dir = st
        .map_dir
        .as_ref()
        .ok_or("Карты на этом сервере выключены (нет папки карт).")?;
    MapFile::load(&dir.join(format!("{name}.ron")))
        .map_err(|e| format!("Карта «{name}» не читается: {e}"))
}

fn free_map(ctx: &HookContext<'_>, map: MapId) -> Result<(), String> {
    if ctx.world.map(map).is_some() {
        return Err(format!("Карта {} уже есть.", map.0));
    }
    Ok(())
}

/// Положить файл карты картой `map` (команды мира уходят в очередь хука).
fn place_file(
    st: &mut State,
    ctx: &mut HookContext<'_>,
    map: MapId,
    file: &MapFile,
) -> Result<(), String> {
    let resolve = polygon::item_resolver(&st.registry).map_err(|e| e.to_string())?;
    let items = file
        .item_commands(map, resolve)
        .map_err(|name| format!("В карте предмет «{name}», которого нет в прототипах."))?;
    for c in file.structure_commands(map).into_iter().chain(items) {
        ctx.request(c);
    }
    let s = file.spawn;
    st.map_spawns
        .insert(map, Vec3::new(s[0], s[1] + maps::map_base_y(map), s[2]));
    Ok(())
}

pub fn lsmap(ctx: &HookContext<'_>) -> String {
    ctx.world
        .maps()
        .iter()
        .map(|m| {
            format!(
                "{} | {} | {} | {}",
                m.id.0,
                m.name,
                if m.paused {
                    "на паузе"
                } else {
                    "идёт"
                },
                if m.initialized {
                    "инициализирована"
                } else {
                    "не инициализирована"
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `mapping <карта> [имя]`: новая замороженная неинициализированная карта (пустая или из файла),
/// маппер — админ-призраком на ней.
pub fn mapping(
    st: &mut State,
    ctx: &mut HookContext<'_>,
    actor: NetworkEntityId,
    args: &[&str],
) -> Result<String, String> {
    let (map, name) = match args {
        [m] => (parse_map(m)?, None),
        [m, name] => (parse_map(m)?, Some(*name)),
        _ => return Err("Использование: mapping <карта> [имя]".into()),
    };
    free_map(ctx, map)?;
    let file = name.map(|n| read_map(st, n)).transpose()?;
    let title = name.unwrap_or("новая карта");
    ctx.request(WorldCommand::DefineMap(map_def(map, title, true, false)));
    match &file {
        Some(f) => place_file(st, ctx, map, f)?,
        None => {
            st.map_spawns.remove(&map);
        }
    }
    become_admin_ghost(st, ctx, actor);
    ctx.teleport(actor, spawn_of(st, map));
    Ok(match name {
        Some(n) => format!("Карта {} из «{n}»: на паузе, вы на ней призраком.", map.0),
        None => format!("Пустая карта {}: на паузе, вы на ней призраком.", map.0),
    })
}

/// `loadmap <карта> <имя>`: карта из файла под номером; идёт и инициализирована.
pub fn loadmap(st: &mut State, ctx: &mut HookContext<'_>, args: &[&str]) -> Result<String, String> {
    let [m, name] = args else {
        return Err("Использование: loadmap <карта> <имя>".into());
    };
    let map = parse_map(m)?;
    free_map(ctx, map)?;
    let file = read_map(st, name)?;
    ctx.request(WorldCommand::DefineMap(map_def(map, name, false, true)));
    place_file(st, ctx, map, &file)?;
    Ok(format!("Карта «{name}» загружена картой {}.", map.0))
}

/// `savemap <карта> <имя> [force]`: снимок карты в файл; инициализированную — только с `force`.
pub fn savemap(st: &State, ctx: &HookContext<'_>, args: &[&str]) -> Result<String, String> {
    let (m, name, force) = match args {
        [m, name] => (*m, *name, false),
        [m, name, f] => (*m, *name, *f == "force" || *f == "true"),
        _ => return Err("Использование: savemap <карта> <имя> [force]".into()),
    };
    let map = parse_map(m)?;
    let def = ctx.world.map(map).ok_or(format!("Карты {} нет.", map.0))?;
    if !admin::valid_map_name(name) {
        return Err(format!(
            "Имя «{name}»: латиница, цифры, «_» и «-», до {} символов.",
            admin::MAP_NAME_MAX
        ));
    }
    if def.initialized && !force {
        return Err(format!(
            "Карта {} инициализирована: в файл попадёт то, что наиграла игра. Сохранить всё равно — savemap {} {name} force.",
            map.0, map.0
        ));
    }
    let dir = st
        .map_dir
        .as_ref()
        .ok_or("Сохранение карт на этом сервере выключено.")?;
    let spawn = spawn_of(st, map).to_array();
    let (file, skipped) = MapFile::from_map(name, spawn, ctx.world, map, |id| {
        st.ss14_names.get(id).cloned()
    });
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.ron"));
    file.save(&path).map_err(|e| e.to_string())?;
    println!("карта {} сохранена: {}", map.0, path.display());
    let mut text = format!(
        "Карта {} сохранена: {} (элементов {}, предметов {}, зон {}).",
        map.0,
        path.display(),
        file.elements.len(),
        file.items.len(),
        file.zones.len()
    );
    if skipped > 0 {
        text.push_str(&format!(" Без прототипа пропущено: {skipped}."));
    }
    Ok(text)
}

/// `rmmap <карта>`: удалить карту (кроме станции); кто был на ней — на станцию.
pub fn rmmap(st: &mut State, ctx: &mut HookContext<'_>, args: &[&str]) -> Result<String, String> {
    let [m] = args else {
        return Err("Использование: rmmap <карта>".into());
    };
    let map = parse_map(m)?;
    if map == MapId(0) {
        return Err("Карту 0 (станцию раунда) удалить нельзя.".into());
    }
    if ctx.world.map(map).is_none() {
        return Err(format!("Карты {} нет.", map.0));
    }
    let home = spawn_of(st, MapId(0));
    let here: Vec<NetworkEntityId> = ctx
        .players()
        .filter(|p| ctx.position_of(*p).is_some_and(|at| map_at(at) == map))
        .collect();
    for p in here {
        ctx.teleport(p, home);
    }
    ctx.request(WorldCommand::RemoveMap { id: map });
    st.map_spawns.remove(&map);
    Ok(format!("Карта {} удалена.", map.0))
}

/// `pausemap`, `unpausemap`, `mapinit`: флаги карты.
pub fn set_flags(
    ctx: &mut HookContext<'_>,
    args: &[&str],
    paused: Option<bool>,
    initialize: bool,
) -> Result<String, String> {
    let [m] = args else {
        return Err("Нужен номер карты.".into());
    };
    let map = parse_map(m)?;
    let mut def = ctx.world.map(map).ok_or(format!("Карты {} нет.", map.0))?;
    if let Some(p) = paused {
        def.paused = p;
    }
    if initialize {
        def.initialized = true;
        def.paused = false;
    }
    let text = format!(
        "Карта {}: {}, {}.",
        map.0,
        if def.paused {
            "на паузе"
        } else {
            "идёт"
        },
        if def.initialized {
            "инициализирована"
        } else {
            "не инициализирована"
        }
    );
    ctx.request(WorldCommand::DefineMap(def));
    Ok(text)
}

pub fn querymappaused(ctx: &HookContext<'_>, args: &[&str]) -> Result<String, String> {
    let [m] = args else {
        return Err("Использование: querymappaused <карта>".into());
    };
    let map = parse_map(m)?;
    let def = ctx.world.map(map).ok_or(format!("Карты {} нет.", map.0))?;
    Ok(if def.paused { "true" } else { "false" }.into())
}

/// Админ-призрак без тела, если ещё не призрак (как `aghost` SS14 при входе в маппинг).
fn become_admin_ghost(st: &mut State, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
    if st
        .lobby
        .as_ref()
        .is_some_and(|l| l.in_lobby.contains(&actor))
    {
        st.lobby_observe(ctx, actor);
    }
    let clock = st.clock;
    let a = st.actor(actor);
    a.admin_ghost = true;
    if !a.ghost {
        a.ghost = true;
        a.respawn_at = clock;
        ctx.set_ghost(actor, true);
    }
    st.publish(ctx, actor);
}
