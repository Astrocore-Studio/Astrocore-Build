//! Команды карт, телепорта и маппинга. Образец — `Robust.Shared/Console/Commands/MapCommands.cs`,
//! `Robust.Server/Console/Commands/MapCommands.cs`, `Robust.Shared/Map/Commands/MapPausingCommands.cs`,
//! `Robust.Shared/Console/Commands/TeleportCommands.cs` Robust @ 1afd692 и
//! `Content.Server/Mapping/MappingCommand.cs`, `Content.Server/Administration/Commands/AGhostCommand.cs`
//! SS14 @ 2f4a2aad.
//!
//! Файлы карт `savemap` пишет в каталог данных пользователя (`UserData` Robust, настройка
//! `game.user_data`); `loadmap` и `mapping` ищут файл сначала там, затем в ресурсах игры.
//! Права: игрок с той же машины, что и сервер, получает все команды
//! (`console.loginlocal` Robust); остальным команды закрыты до администрирования (фаза 6).

use std::path::PathBuf;

use astrocore_console::{
    CVarDef, CVarFlags, Config, ConfigWorldExt, ConsoleCommand, ConsoleShell, ConsoleWorldExt,
};
use astrocore_data::{parse_yaml, to_yaml_string};
use astrocore_ecs::{LifecycleWorldExt, MetaData};
use astrocore_map::{
    EntityCoordinates, Map, MapCoordinates, MapId, MapLoadOptions, MapLoadWorldExt, MapSaveOptions,
    MapSaveWorldExt, MapWorldExt, Maps, SpawnWorldExt, TransformWorldExt,
};
use astrocore_math::{Angle, Vec2};
use astrocore_net::{NetEntities, NetEntity, ServerNet, Session, SessionWorldExt};
use astrocore_resources::{Resources, normalize_path};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::Side;

/// Каталог данных пользователя: сюда `savemap` пишет карты.
pub const USER_DATA: CVarDef<&str> = CVarDef::new(
    "game.user_data",
    "data",
    CVarFlags::SERVERONLY.union(CVarFlags::ARCHIVE),
    "Каталог данных пользователя: сюда сохраняются карты.",
);

/// Игрок с той же машины, что и сервер, получает все команды (`console.loginlocal` Robust).
pub const LOGIN_LOCAL: CVarDef<bool> = CVarDef::new(
    "console.loginlocal",
    true,
    CVarFlags::SERVERONLY.union(CVarFlags::ARCHIVE),
    "Игрок с той же машины, что и сервер, получает все команды.",
);

/// Прототип админ-призрака (`ServerGameTicker.AdminObserverPrototypeName`).
pub const ADMIN_OBSERVER: &str = "AdminObserver";

/// Админ-призрак и тело, в которое он возвращается.
#[derive(Component, Debug, Clone, Copy)]
pub struct AdminGhost {
    pub body: Option<Entity>,
}

fn map_id(shell: &mut ConsoleShell, text: &str) -> Option<MapId> {
    match text.parse::<u32>() {
        Ok(id) => Some(MapId(id)),
        Err(_) => {
            shell.write_error(format!("«{text}» — не номер карты"));
            None
        }
    }
}

fn existing_map(world: &World, shell: &mut ConsoleShell, text: &str) -> Option<(MapId, Entity)> {
    let id = map_id(shell, text)?;
    match world.map_entity(id) {
        Some(entity) => Some((id, entity)),
        None => {
            shell.write_error(format!("карты {} нет", id.0));
            None
        }
    }
}

fn number(shell: &mut ConsoleShell, text: &str) -> Option<f32> {
    match text.parse::<f32>() {
        Ok(value) => Some(value),
        Err(_) => {
            shell.write_error(format!("«{text}» — не число"));
            None
        }
    }
}

fn flag(shell: &mut ConsoleShell, text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => {
            shell.write_error(format!("«{text}» — не true или false"));
            None
        }
    }
}

fn map_ids(world: &World) -> Vec<String> {
    let mut ids: Vec<u32> = world
        .get_resource::<Maps>()
        .map(|maps| maps.iter().map(|(id, _)| id.0).collect())
        .unwrap_or_default();
    ids.sort_unstable();
    ids.into_iter().map(|id| id.to_string()).collect()
}

fn complete_map_id(world: &World, _: &ConsoleShell, args: &[String]) -> Vec<String> {
    if args.len() == 1 {
        map_ids(world)
    } else {
        Vec::new()
    }
}

fn next_map_id(world: &World) -> MapId {
    let max = world
        .get_resource::<Maps>()
        .and_then(|maps| maps.iter().map(|(id, _)| id.0).max())
        .unwrap_or(0);
    MapId(max + 1)
}

/// Файл в каталоге данных пользователя.
fn user_file(world: &World, path: &str) -> Result<PathBuf, String> {
    let relative = normalize_path(path).map_err(|error| error.to_string())?;
    if relative.is_empty() {
        return Err("пустой путь".into());
    }
    let root = world.resource::<Config>().get_text(&USER_DATA);
    Ok(PathBuf::from(root).join(relative))
}

/// Текст файла карты: из данных пользователя, иначе из ресурсов игры.
fn read_map_file(world: &World, path: &str) -> Result<String, String> {
    if let Ok(file) = user_file(world, path)
        && let Ok(text) = std::fs::read_to_string(&file)
    {
        return Ok(text);
    }
    world
        .get_resource::<Resources>()
        .ok_or_else(|| format!("файл {path} не найден"))?
        .read_to_string(path)
        .map_err(|error| error.to_string())
}

/// Загружает файл карты под номером `id` без инициализации.
fn load_map_file(world: &mut World, id: MapId, path: &str) -> Result<usize, String> {
    let text = read_map_file(world, path)?;
    let node = parse_yaml(&text).map_err(|error| format!("{path}: {error}"))?;
    let report = world
        .load_map_with(
            &node,
            MapLoadOptions {
                initialize: false,
                map_id: Some(id),
            },
        )
        .map_err(|error| format!("{path}: {error}"))?;
    if world.map_entity(id).is_none() {
        return Err(format!("{path}: в файле нет карты"));
    }
    Ok(report.entities)
}

/// Сущность, которой управляет игрок оболочки.
fn attached(world: &World, shell: &mut ConsoleShell) -> Option<Entity> {
    let entity = shell
        .player
        .and_then(|session| world.get::<Session>(session))
        .and_then(|session| session.attached);
    if entity.is_none() {
        shell.write_error("у игрока нет сущности: команду выполняет игрок в игре");
    }
    entity
}

/// Перемещает сущность в точку карты: на грид под точкой или на саму карту.
fn teleport(world: &mut World, entity: Entity, map: MapId, position: Vec2) -> Result<(), String> {
    let map_entity = world.map_entity(map).ok_or("карты нет")?;
    let coordinates = match world.find_grid_at(MapCoordinates::new(map, position)) {
        Some(grid) => {
            let local = world
                .world_transform(grid)
                .map_or(position, |grid| grid.to_local(position));
            EntityCoordinates::new(grid, local)
        }
        None => EntityCoordinates::new(map_entity, position),
    };
    world
        .set_coordinates(entity, coordinates, None)
        .map_err(|error| error.to_string())
}

/// Сущность по сетевому номеру или по имени игрока.
pub(crate) fn find_target(world: &mut World, text: &str) -> Option<Entity> {
    if let Ok(number) = text.parse::<u32>()
        && let Some(entity) = world.resource::<NetEntities>().entity(NetEntity(number))
        && world.get::<Map>(entity).is_none()
    {
        return Some(entity);
    }
    let sessions: Vec<Entity> = world.resource::<ServerNet>().sessions().collect();
    sessions.into_iter().find_map(|session| {
        let state = world.get::<Session>(session)?;
        (state.name == text).then_some(state.attached).flatten()
    })
}

// ---------------------------------------------------------------- карты

fn addmap(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let Some(id) = args.first().and_then(|text| map_id(shell, text)) else {
        shell.write_line("addmap <номер> [без инициализации]");
        return;
    };
    let preinit = match args.get(1) {
        Some(text) => match flag(shell, text) {
            Some(value) => value,
            None => return,
        },
        None => false,
    };
    match world.create_map_with_id(id, !preinit) {
        Ok(_) => shell.write_line(format!("карта {} создана", id.0)),
        Err(error) => shell.write_error(error.to_string()),
    }
}

/// Сдвиг и поворот загруженной карты (`ApplyTransform` загрузчика Robust): сущности прямо
/// на карте поворачиваются вокруг её начала и сдвигаются; карта-грид не трогается.
pub fn apply_load_transform(world: &mut World, id: MapId, offset: Vec2, rotation: Angle) {
    if rotation.0 == 0.0 && offset == Vec2::ZERO {
        return;
    }
    let Some(map) = world.map_entity(id) else {
        return;
    };
    if world.get::<astrocore_map::MapGrid>(map).is_some() {
        return;
    }
    let children: Vec<Entity> = world
        .get::<bevy_ecs::hierarchy::Children>(map)
        .map(|c| c.iter().copied().collect())
        .unwrap_or_default();
    for child in children {
        let Some((position, local)) = world
            .get::<astrocore_map::Transform>(child)
            .map(|t| (t.local_position(), t.local_rotation()))
        else {
            continue;
        };
        let _ = world.set_local_position(child, rotation.rotate(position) + offset);
        let _ = world.set_local_rotation(child, Angle(local.0 + rotation.0));
    }
}

/// `loadmap` Robust: номер новой карты, путь, сдвиг и поворот (радианы).
fn loadmap(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    if args.len() < 2 || args.len() > 5 {
        shell.write_line("loadmap <номер> <путь> [x] [y] [поворот, радианы]");
        return;
    }
    let Some(id) = map_id(shell, &args[0]) else {
        return;
    };
    if world.map_entity(id).is_some() {
        shell.write_error(format!("карта {} уже есть", id.0));
        return;
    }
    let mut values = [0.0f32; 3];
    for (value, text) in values.iter_mut().zip(&args[2..]) {
        match number(shell, text) {
            Some(parsed) => *value = parsed,
            None => return,
        }
    }
    let [x, y, rotation] = values;
    let path = &args[1];
    match load_map_file(world, id, path) {
        Ok(entities) => {
            apply_load_transform(world, id, Vec2::new(x, y), Angle(rotation));
            shell.write_line(format!(
                "карта {} загружена из {path}: сущностей {entities}",
                id.0
            ));
        }
        Err(error) => shell.write_error(error),
    }
}

fn savemap(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    if args.len() < 2 || args.len() > 3 {
        shell.write_line("savemap <номер> <путь> [force]");
        return;
    }
    let Some((id, map)) = existing_map(world, shell, &args[0]) else {
        return;
    };
    let force = match args.get(2) {
        Some(text) => match flag(shell, text) {
            Some(value) => value,
            None => return,
        },
        None => false,
    };
    match save_map_file(world, map, &args[1], force) {
        Ok((file, entities)) => shell.write_line(format!(
            "карта {} сохранена в {}: сущностей {entities}",
            id.0,
            file.display(),
        )),
        Err(SaveError::Initialized(error)) => shell.write_error(format!(
            "{error}; сохранить всё равно: savemap <номер> <путь> true"
        )),
        Err(SaveError::Other(error)) => shell.write_error(error),
    }
}

/// Задать поле компонента записи сущности `uid` в документе карты: `None` убирает поле.
/// Компонента нет в записи — он добавляется, если поле задаётся.
pub fn set_saved_component_field(
    document: &mut astrocore_data::DataNode,
    uid: u32,
    component: &str,
    field: &str,
    value: Option<astrocore_data::DataNode>,
) {
    use astrocore_data::{DataMap, DataNode};
    let uid = uid.to_string();
    let Some(section) = document
        .as_map_mut()
        .and_then(|root| root.get_mut("entities"))
        .and_then(DataNode::as_seq_mut)
    else {
        return;
    };
    let entry = section
        .iter_mut()
        .filter_map(|group| group.as_map_mut()?.get_mut("entities")?.as_seq_mut())
        .flat_map(|entities| entities.iter_mut())
        .find(|entity| {
            entity
                .as_map()
                .and_then(|m| m.get("uid"))
                .and_then(DataNode::as_str)
                == Some(uid.as_str())
        });
    let Some(entry) = entry.and_then(DataNode::as_map_mut) else {
        return;
    };
    if entry.get("components").is_none() {
        entry.insert("components", DataNode::seq(Vec::new()));
    }
    let Some(components) = entry.get_mut("components").and_then(DataNode::as_seq_mut) else {
        return;
    };
    let existing = components.iter_mut().find(|c| {
        c.as_map()
            .and_then(|m| m.get("type"))
            .and_then(DataNode::as_str)
            == Some(component)
    });
    match (existing.and_then(DataNode::as_map_mut), value) {
        (Some(map), Some(value)) => {
            map.insert(field, value);
        }
        (Some(map), None) => {
            map.remove(field);
        }
        (None, Some(value)) => {
            let mut map = DataMap::new();
            map.insert("type", DataNode::plain(component));
            map.insert(field, value);
            components.push(DataNode::map(map));
        }
        (None, None) => {}
    }
}

/// Почему карта не сохранилась.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// Карта уже инициализирована, а сохранение без `force`.
    Initialized(String),
    Other(String),
}

/// Сохраняет карту в файл данных пользователя (`TrySaveMap`): путь и число сущностей.
pub fn save_map_file(
    world: &mut World,
    map: Entity,
    path: &str,
    force: bool,
) -> Result<(PathBuf, usize), SaveError> {
    let file = user_file(world, path).map_err(SaveError::Other)?;
    crate::decals::write_for_save(world, map);
    crate::atmos::write_for_save(world, map);
    let mut saved = world
        .save_map(
            map,
            MapSaveOptions {
                allow_initialized: force,
            },
        )
        .map_err(|error| SaveError::Initialized(error.to_string()))?;
    // Ссылки на сущности, изменённые в игре (ADR-0014): списки устройств и связи сигналов.
    crate::device_network::write_references(world, &mut saved);
    crate::device_linking::write_references(world, &mut saved);
    let text = to_yaml_string(&saved.document);
    file.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&file, text))
        .map_err(|error| SaveError::Other(format!("{}: {error}", file.display())))?;
    Ok((file, saved.entities))
}

fn rmmap(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let [id] = args else {
        shell.write_line("rmmap <номер>");
        return;
    };
    let Some((id, map)) = existing_map(world, shell, id) else {
        return;
    };
    world.delete_entity(map);
    shell.write_line(format!("карта {} удалена", id.0));
}

fn set_paused(world: &mut World, shell: &mut ConsoleShell, args: &[String], paused: bool) {
    let [id] = args else {
        shell.write_line("pausemap <номер>");
        return;
    };
    if let Some((_, map)) = existing_map(world, shell, id) {
        world.set_map_paused(map, paused);
    }
}

fn pausemap(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    set_paused(world, shell, args, true);
}

fn unpausemap(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    set_paused(world, shell, args, false);
}

fn querymappaused(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let [id] = args else {
        shell.write_line("querymappaused <номер>");
        return;
    };
    if let Some((_, map)) = existing_map(world, shell, id) {
        let paused = world.get::<Map>(map).is_some_and(|m| m.paused);
        shell.write_line(paused.to_string());
    }
}

fn mapinit(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let [id] = args else {
        shell.write_line("mapinit <номер>");
        return;
    };
    let Some((_, map)) = existing_map(world, shell, id) else {
        return;
    };
    if world.get::<Map>(map).is_some_and(|m| m.initialized) {
        shell.write_error("карта уже инициализирована");
        return;
    }
    world.initialize_map(map);
}

fn lsmap(world: &mut World, shell: &mut ConsoleShell, _args: &[String]) {
    let mut maps: Vec<(MapId, Entity)> = world
        .get_resource::<Maps>()
        .map(|maps| maps.iter().collect())
        .unwrap_or_default();
    maps.sort_unstable_by_key(|(id, _)| id.0);
    for (id, entity) in maps {
        let Some(map) = world.get::<Map>(entity) else {
            continue;
        };
        let name = world
            .get::<MetaData>(entity)
            .and_then(|meta| meta.name.clone())
            .unwrap_or_default();
        let grids: Vec<String> = map
            .grids()
            .iter()
            .map(|grid| net_id(world, *grid))
            .collect();
        shell.write_line(format!(
            "{}: {name}, init: {}, paused: {}, nent: {}, grids: {}",
            id.0,
            map.initialized,
            map.paused,
            net_id(world, entity),
            grids.join(",")
        ));
    }
}

/// Номер сущности для команд: сетевой, если он есть.
fn net_id(world: &World, entity: Entity) -> String {
    world
        .get::<NetEntity>(entity)
        .map_or_else(|| format!("e{}", entity.index()), |net| net.0.to_string())
}

// ---------------------------------------------------------------- телепорт

fn tp(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    if args.len() < 2 || args.len() > 3 {
        shell.write_line("tp <x> <y> [карта]");
        return;
    }
    let Some(entity) = attached(world, shell) else {
        return;
    };
    let (Some(x), Some(y)) = (number(shell, &args[0]), number(shell, &args[1])) else {
        return;
    };
    let map = match args.get(2) {
        Some(text) => match existing_map(world, shell, text) {
            Some((id, _)) => id,
            None => return,
        },
        None => match world.map_coordinates(entity) {
            Some(coordinates) => coordinates.map,
            None => return shell.write_error("сущность не на карте: укажите карту"),
        },
    };
    match teleport(world, entity, map, Vec2::new(x, y)) {
        Ok(()) => shell.write_line(format!("телепорт на {}:{x},{y}", map.0)),
        Err(error) => shell.write_error(error),
    }
}

fn tpto(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let Some((target, victims)) = args.split_first() else {
        shell.write_line("tpto <куда: номер или игрок> [кого...]");
        return;
    };
    let Some(target) = find_target(world, target) else {
        shell.write_error(format!("«{target}» — нет такой сущности или игрока"));
        return;
    };
    let Some(destination) = world.map_coordinates(target) else {
        shell.write_error("цель не на карте");
        return;
    };
    let mut moving = Vec::new();
    if victims.is_empty() {
        match attached(world, shell) {
            Some(entity) => moving.push(entity),
            None => return,
        }
    }
    for victim in victims {
        match find_target(world, victim) {
            Some(entity) if entity != target => moving.push(entity),
            Some(_) => {}
            None => shell.write_error(format!("«{victim}» — нет такой сущности или игрока")),
        }
    }
    for entity in moving {
        if let Err(error) = teleport(world, entity, destination.map, destination.position) {
            shell.write_error(error);
        }
    }
}

fn tpgrid(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    if args.len() < 3 || args.len() > 4 {
        shell.write_line("tpgrid <грид> <x> <y> [карта]");
        return;
    }
    let grid = args[0]
        .parse::<u32>()
        .ok()
        .and_then(|number| world.resource::<NetEntities>().entity(NetEntity(number)))
        .filter(|entity| {
            world.get::<astrocore_map::MapGrid>(*entity).is_some()
                && world.get::<Map>(*entity).is_none()
        });
    let Some(grid) = grid else {
        shell.write_error(format!("«{}» — не грид", args[0]));
        return;
    };
    let (Some(x), Some(y)) = (number(shell, &args[1]), number(shell, &args[2])) else {
        return;
    };
    let map = match args.get(3) {
        Some(text) => match existing_map(world, shell, text) {
            Some((_, map)) => map,
            None => return,
        },
        None => match world
            .map_coordinates(grid)
            .and_then(|c| world.map_entity(c.map))
        {
            Some(map) => map,
            None => return shell.write_error("грид не на карте"),
        },
    };
    if let Err(error) =
        world.set_coordinates(grid, EntityCoordinates::new(map, Vec2::new(x, y)), None)
    {
        shell.write_error(error.to_string());
    }
}

// ---------------------------------------------------------------- маппинг

/// Игрок становится админ-призраком или возвращается в своё тело.
fn aghost_toggle(world: &mut World, session: Entity) -> Result<Entity, String> {
    let body = world
        .get::<Session>(session)
        .and_then(|state| state.attached)
        .ok_or("у игрока нет сущности")?;
    if let Some(ghost) = world.get::<AdminGhost>(body).copied() {
        let Some(original) = ghost.body.filter(|e| world.get_entity(*e).is_ok()) else {
            return Ok(body);
        };
        world.attach_player(session, original);
        world.queue_delete(body);
        return Ok(original);
    }
    let coordinates = world.coordinates(body).ok_or("сущность не на карте")?;
    let ghost = world
        .spawn_attached(ADMIN_OBSERVER, coordinates)
        .map_err(|error| error.to_string())?;
    world
        .entity_mut(ghost)
        .insert(AdminGhost { body: Some(body) });
    let name = world
        .get::<Session>(session)
        .map(|state| state.name.clone());
    if let Some(mut meta) = world.get_mut::<MetaData>(ghost) {
        meta.name = name;
    }
    world.attach_player(session, ghost);
    Ok(ghost)
}

fn aghost(world: &mut World, shell: &mut ConsoleShell, _args: &[String]) {
    let Some(session) = shell.player else {
        shell.write_error("команду выполняет игрок");
        return;
    };
    if let Err(error) = aghost_toggle(world, session) {
        shell.write_error(error);
    }
}

fn mapping(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    let Some(session) = shell.player else {
        shell.write_error("команду выполняет игрок");
        return;
    };
    if args.len() > 2 {
        shell.write_line("mapping [номер] [путь]");
        return;
    }
    let id = match args.first() {
        Some(text) => match map_id(shell, text) {
            Some(id) => id,
            None => return,
        },
        None => next_map_id(world),
    };
    if id.0 == 0 {
        shell.write_error("номер карты 0 занят под «нигде»");
        return;
    }
    if world.map_entity(id).is_some() {
        shell.write_error(format!("карта {} уже есть", id.0));
        return;
    }
    match args.get(1) {
        Some(path) => {
            if let Err(error) = load_map_file(world, id, path) {
                shell.write_error(error);
                return;
            }
        }
        None => {
            if let Err(error) = world.create_map_with_id(id, false) {
                shell.write_error(error.to_string());
                return;
            }
            if let Some(map) = world.map_entity(id) {
                world.set_map_paused(map, true);
            }
        }
    }
    // Маппер ходит админ-призраком: он движется и на карте на паузе.
    let is_ghost = world
        .get::<Session>(session)
        .and_then(|state| state.attached)
        .is_some_and(|body| world.get::<AdminGhost>(body).is_some());
    let entity = if is_ghost {
        world
            .get::<Session>(session)
            .and_then(|state| state.attached)
    } else {
        match aghost_toggle(world, session) {
            Ok(ghost) => Some(ghost),
            Err(error) => {
                shell.write_error(error);
                None
            }
        }
    };
    if let Some(entity) = entity
        && let Err(error) = teleport(world, entity, id, Vec2::ZERO)
    {
        shell.write_error(error);
    }
    // Автосохранение карты маппинга (`ToggleAutosave` в `MappingCommand`).
    let name = args.get(1).map_or("NEWMAP", String::as_str);
    crate::mapping_autosave::toggle_autosave(world, id, Some(name));
    match args.get(1) {
        Some(path) => shell.write_line(format!("маппинг карты {} из {path}", id.0)),
        None => shell.write_line(format!("маппинг новой карты {}", id.0)),
    }
}

fn command(
    name: &'static str,
    description: &'static str,
    help: &'static str,
    execute: astrocore_console::CommandFn,
    maps: bool,
) -> ConsoleCommand {
    ConsoleCommand {
        name,
        description,
        help,
        execute,
        complete: maps.then_some(complete_map_id as astrocore_console::CompletionFn),
    }
}

/// Добавить газ на клетку грида (`AddGasCommand` SS14 @ 2f4a2aad).
fn addgas(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    if args.len() < 5 {
        return;
    }
    let (Ok(x), Ok(y)) = (args[0].parse::<i32>(), args[1].parse::<i32>()) else {
        return;
    };
    // Грид по номеру, как его печатает `lsmap`.
    let grids: Vec<Entity> = world
        .query::<&Map>()
        .iter(world)
        .flat_map(|map| map.grids().to_vec())
        .collect();
    let grid = grids
        .into_iter()
        .find(|grid| net_id(world, *grid) == args[2]);
    // `TryParseGasID`: имя газа или номер.
    let gas = args[3]
        .parse::<usize>()
        .ok()
        .filter(|g| *g < crate::atmos::gas::GAS_COUNT)
        .or_else(|| crate::atmos::gas::gas_index(&args[3]));
    let (Some(gas), Ok(moles)) = (gas, args[4].parse::<f32>()) else {
        return;
    };
    let Some(grid) = grid.filter(|g| world.get::<astrocore_map::MapGrid>(*g).is_some()) else {
        shell.write_error(format!(
            "«{}» — нет такой сущности или это не грид",
            args[2]
        ));
        return;
    };
    let coord = astrocore_map::TileCoord::new(x, y, 0);
    if crate::atmos::with_tile_mixture(world, grid, coord, |air| air.adjust_moles(gas, moles))
        .is_none()
    {
        shell.write_line("Нет клетки с воздухом.");
    }
}

/// Взрыв (`ExplosionCommand` SS14 @ 2f4a2aad):
/// `explosion <сила> [наклон] [предел] [x y [карта]] [вид]`; без точки — у игрока.
fn explosion(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    if args.is_empty() || args.len() == 4 || args.len() > 7 {
        shell.write_error("неверное число аргументов");
        return;
    }
    let float =
        |i: usize, default: f32| args.get(i).map_or(Some(default), |v| v.parse::<f32>().ok());
    let (Some(intensity), Some(slope), Some(max)) = (float(0, 0.0), float(1, 5.0), float(2, 100.0))
    else {
        shell.write_error("не число");
        return;
    };
    let coords = if args.len() > 4 {
        let (Some(x), Some(y)) = (float(3, 0.0), float(4, 0.0)) else {
            shell.write_error("не координаты");
            return;
        };
        let map = match args.get(5) {
            Some(id) => match id.parse::<u32>() {
                Ok(id) => MapId(id),
                Err(_) => {
                    shell.write_error("не номер карты");
                    return;
                }
            },
            None => match attached(world, shell).and_then(|e| world.map_coordinates(e)) {
                Some(at) => at.map,
                None => return,
            },
        };
        MapCoordinates::new(map, Vec2::new(x, y))
    } else {
        let Some(at) = attached(world, shell).and_then(|e| world.map_coordinates(e)) else {
            return;
        };
        at
    };
    let kind = args.get(6).map_or("Default", String::as_str);
    if crate::explosion::explosion_prototype(world, kind).is_none() {
        shell.write_error(format!("нет вида взрыва «{kind}»"));
        return;
    }
    crate::explosion::queue_explosion(
        world,
        coords,
        kind,
        intensity,
        slope,
        max,
        None,
        1.0,
        i32::MAX,
        true,
    );
}

pub fn install(world: &mut World, side: Side) {
    if !side.is_server() {
        return;
    }
    world.register_cvar(&USER_DATA);
    world.register_cvar(&LOGIN_LOCAL);
    for command in [
        command(
            "addmap",
            "Создаёт карту.",
            "addmap <номер> [без инициализации]",
            addmap,
            false,
        ),
        command(
            "loadmap",
            "Загружает карту из файла.",
            "loadmap <номер> <путь> [x] [y] [поворот, радианы]",
            loadmap,
            false,
        ),
        command(
            "savemap",
            "Сохраняет карту в файл данных пользователя.",
            "savemap <номер> <путь> [force]",
            savemap,
            true,
        ),
        command("rmmap", "Удаляет карту.", "rmmap <номер>", rmmap, true),
        command(
            "pausemap",
            "Ставит карту на паузу.",
            "pausemap <номер>",
            pausemap,
            true,
        ),
        command(
            "unpausemap",
            "Снимает карту с паузы.",
            "unpausemap <номер>",
            unpausemap,
            true,
        ),
        command(
            "querymappaused",
            "Стоит ли карта на паузе.",
            "querymappaused <номер>",
            querymappaused,
            true,
        ),
        command(
            "mapinit",
            "Инициализирует карту (MapInit).",
            "mapinit <номер>",
            mapinit,
            true,
        ),
        command(
            "addgas",
            "Добавляет газ на клетку.",
            "addgas <x> <y> <грид> <газ> <моли>",
            addgas,
            false,
        ),
        command("lsmap", "Список карт.", "lsmap", lsmap, false),
        command(
            "explosion",
            "Взрыв в точке или у игрока.",
            "explosion <сила> [наклон] [предел] [x y [карта]] [вид]",
            explosion,
            false,
        ),
        command(
            "tp",
            "Телепорт в точку карты.",
            "tp <x> <y> [карта]",
            tp,
            false,
        ),
        command(
            "tpto",
            "Телепорт к сущности или игроку.",
            "tpto <куда> [кого...]",
            tpto,
            false,
        ),
        command(
            "tpgrid",
            "Переносит грид в точку карты.",
            "tpgrid <грид> <x> <y> [карта]",
            tpgrid,
            false,
        ),
        command(
            "aghost",
            "Админ-призрак: выйти из тела или вернуться.",
            "aghost",
            aghost,
            false,
        ),
        command(
            "mapping",
            "Маппинг: новая или загруженная карта на паузе, игрок — админ-призрак.",
            "mapping [номер] [путь]",
            mapping,
            false,
        ),
    ] {
        world.register_command(command);
    }
}

#[cfg(test)]
mod tests;
