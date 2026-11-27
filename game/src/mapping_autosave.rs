//! Автосохранение карт маппинга: карта, которую строят командой `mapping`, раз в
//! `mapping.autosave_interval` секунд сохраняется в `<mapping.autosave_dir>/<имя файла>/`
//! данных пользователя с отметкой времени в имени. Инициализированная карта из
//! автосохранения выбывает. Команда `toggleautosave` включает и выключает его вручную.
//! Образец — `MappingSystem` и `CCVars.Mapping` SS14 @ 2f4a2aad.
//!
//! Нет пока автосохранения отдельных гридов: загрузки грида (`loadgrid`) ещё нет.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use astrocore_console::{
    CVarChanged, CVarDef, CVarFlags, Config, ConfigWorldExt, ConsoleCommand, ConsoleShell,
    ConsoleWorldExt,
};
use astrocore_ecs::{EventWorldExt, GameWorldExt, Update};
use astrocore_map::{Map, MapId, MapWorldExt};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::Side;

pub const AUTOSAVE_ENABLED: CVarDef<bool> = CVarDef::new(
    "mapping.autosave",
    true,
    CVarFlags::SERVERONLY,
    "Включает ли режим маппинга автосохранение.",
);
pub const AUTOSAVE_INTERVAL: CVarDef<f32> = CVarDef::new(
    "mapping.autosave_interval",
    600.0,
    CVarFlags::SERVERONLY,
    "Пауза между автосохранениями, секунды.",
);
pub const AUTOSAVE_DIRECTORY: CVarDef<&str> = CVarDef::new(
    "mapping.autosave_dir",
    "Autosaves",
    CVarFlags::SERVERONLY,
    "Каталог автосохранений в данных пользователя.",
);

/// Карты на автосохранении (`_currentlyAutosaving`): карта → (когда, имя файла).
#[derive(Resource, Debug, Default)]
pub struct MappingAutosaves {
    pub maps: BTreeMap<Entity, (Instant, String)>,
}

fn next_time(world: &World) -> Instant {
    Instant::now() + Duration::from_secs_f32(world.cvar(&AUTOSAVE_INTERVAL).max(0.0))
}

fn initialized(world: &World, map: Entity) -> bool {
    world.get::<Map>(map).is_none_or(|m| m.initialized)
}

/// Включить или выключить автосохранение карты (`ToggleAutosave`): уже сохраняемая
/// выключается; без пути — только выключение.
pub fn toggle_autosave(world: &mut World, id: MapId, path: Option<&str>) {
    if !world.cvar(&AUTOSAVE_ENABLED) {
        return;
    }
    let Some(map) = world.map_entity(id) else {
        return;
    };
    let mut saves = world.get_resource_or_insert_with(MappingAutosaves::default);
    if saves.maps.remove(&map).is_some() {
        return;
    }
    let Some(path) = path else {
        return;
    };
    if initialized(world, map) {
        eprintln!("автосохранение: карта {} уже инициализирована", id.0);
        return;
    }
    // Имя файла без каталогов (`Path.GetFileName`).
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned();
    let next = next_time(world);
    world
        .resource_mut::<MappingAutosaves>()
        .maps
        .insert(map, (next, name));
}

/// Имя файла автосохранения по времени, мс UTC: `ГГГГ-М-ДД_ЧЧ.ММ.СС-AUTO.yml`, как в SS14
/// (там время местное, здесь — UTC).
pub fn autosave_file_name(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    // Дата по числу дней от 1970-01-01 (алгоритм Хиннанта `civil_from_days`).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year}-{month}-{day:02}_{:02}.{:02}.{:02}-AUTO.yml",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// Сохранить карты, чьё время пришло (`Update`).
pub fn update(world: &mut World) {
    if !world.cvar(&AUTOSAVE_ENABLED) {
        return;
    }
    let now = Instant::now();
    let due: Vec<(Entity, String)> = match world.get_resource::<MappingAutosaves>() {
        Some(saves) => saves
            .maps
            .iter()
            .filter(|(_, (time, _))| *time < now)
            .map(|(map, (_, name))| (*map, name.clone()))
            .collect(),
        None => return,
    };
    for (map, name) in due {
        if initialized(world, map) {
            eprintln!("автосохранение: карта удалена или инициализирована, больше не сохраняется");
            world.resource_mut::<MappingAutosaves>().maps.remove(&map);
            continue;
        }
        let next = next_time(world);
        world
            .resource_mut::<MappingAutosaves>()
            .maps
            .insert(map, (next, name.clone()));
        let directory = world.resource::<Config>().get_text(&AUTOSAVE_DIRECTORY);
        let path = format!(
            "{directory}/{name}/{}",
            autosave_file_name(crate::database::now_ms())
        );
        match crate::map_commands::save_map_file(world, map, &path, false) {
            Ok((file, _)) => eprintln!("автосохранение карты {name}: {}", file.display()),
            Err(error) => eprintln!("автосохранение карты {name}: {error:?}"),
        }
    }
}

fn toggleautosave_command(world: &mut World, shell: &mut ConsoleShell, args: &[String]) {
    if args.is_empty() || args.len() > 2 {
        shell.write_error("toggleautosave <карта> [путь, если включить]");
        return;
    }
    let Ok(id) = args[0].parse::<u32>() else {
        shell.write_error(format!("«{}» — не номер карты", args[0]));
        return;
    };
    toggle_autosave(world, MapId(id), args.get(1).map(String::as_str));
}

pub fn install(world: &mut World, side: Side) {
    if !side.is_server() {
        return;
    }
    world.register_cvar(&AUTOSAVE_ENABLED);
    world.register_cvar(&AUTOSAVE_INTERVAL);
    world.register_cvar(&AUTOSAVE_DIRECTORY);
    world.init_resource::<MappingAutosaves>();
    world.register_command(ConsoleCommand {
        name: "toggleautosave",
        description: "Включает и выключает автосохранение карты.",
        help: "toggleautosave <карта> [путь, если включить]",
        execute: toggleautosave_command,
        complete: None,
    });
    // Выключенное автосохранение забывает карты (`SetAutosaveEnabled`).
    world.subscribe_broadcast::<CVarChanged>(|world, event| {
        if event.name == AUTOSAVE_ENABLED.name && !world.cvar(&AUTOSAVE_ENABLED) {
            world.resource_mut::<MappingAutosaves>().maps.clear();
        }
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
