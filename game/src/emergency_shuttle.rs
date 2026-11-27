//! Шаттл эвакуации и ЦК: при создании станции загружается грид ЦК (`/Maps/centcomm.yml`) на
//! свою карту, а на ней — шаттл эвакуации станции (`/Maps/Shuttles/emergency.yml`, в 500 м от
//! ЦК). По окончании отсчёта конца раунда шаттл стыкуется с самым большим гридом станции —
//! к шлюзам `DockEmergency`, к другим шлюзам или рядом (стоянка дольше: ×1,6667 и ×2), а без
//! станции — «удачи»; объявление со стороной света и ближайшим маяком. Через
//! `shuttle.emergency_dock_time` секунд шаттл улетает: за `shuttle.emergency_authorize_time`
//! объявляется запуск, за время разгона начинается прыжок к ЦК, при отлёте — объявление,
//! через время перехода (60–180 с, кратно 10) и 5 с — конец раунда. Досрочный запуск, команды
//! `dockemergencyshuttle`, `launchemergencyshuttle`, `delayroundend`. Образец —
//! `EmergencyShuttleSystem`, `EmergencyShuttleSystem.Console` (таймер, запуск),
//! `StationCentcommComponent`, `StationEmergencyShuttleComponent`,
//! `EmergencyShuttleComponent`, `DockEmergencyShuttleCommand`,
//! `LaunchEmergencyShuttleCommand`, `DelayRoundEndCommand`, `CCVars.Shuttle` SS14 @ 2f4a2aad.
//!
//! Шаттл не летает, а переносится (ADR-0015 Build). Буфер 5 с после перехода работает как
//! задумано в SS14 (там он теряется: `TimeSpan.Milliseconds` от 5 с — ноль). Нет пока:
//! спасательных капсул, экранов таймера, запрета пилотирования и защиты грида шаттла,
//! назначений FTL для других шаттлов, отладочного показа места стыковки.

use std::time::Duration;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_console::{
    CVarChanged, CVarDef, CVarFlags, ConfigWorldExt, ConsoleCommand, ConsoleShell, ConsoleWorldExt,
};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, LifecycleWorldExt, MetaData, Update};
use astrocore_loc::LocArg;
use astrocore_map::{EntityCoordinates, Map, MapGrid, MapWorldExt, Transform, TransformWorldExt};
use astrocore_math::{Angle, Vec2};
use astrocore_net::NetTarget;
use astrocore_ui::{BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::examine::loc;
use crate::game_ticker::{self, GameRunLevel};
use crate::popups::{PopupType, PopupWorldExt};
use crate::{docking, round_end, shuttles, stations};

pub const EMERGENCY_SHUTTLE_ENABLED: CVarDef<bool> = CVarDef::new(
    "shuttle.emergency",
    true,
    CVarFlags::SERVERONLY,
    "Есть ли шаттл эвакуации; без него раунд кончается сразу по окончании отсчёта.",
);
pub const EMERGENCY_SHUTTLE_DOCK_TIME: CVarDef<f32> = CVarDef::new(
    "shuttle.emergency_dock_time",
    180.0,
    CVarFlags::SERVERONLY,
    "Сколько секунд шаттл эвакуации стоит у станции.",
);
pub const DOCK_TIME_MULTIPLIER_OTHER_DOCK: CVarDef<f32> = CVarDef::new(
    "shuttle.emergency_dock_time_multiplier_other_dock",
    1.6667,
    CVarFlags::SERVERONLY,
    "Во сколько раз дольше стоянка у неприоритетного шлюза.",
);
pub const DOCK_TIME_MULTIPLIER_NO_DOCK: CVarDef<f32> = CVarDef::new(
    "shuttle.emergency_dock_time_multiplier_no_dock",
    2.0,
    CVarFlags::SERVERONLY,
    "Во сколько раз дольше стоянка, если шаттл не пристыковался.",
);
pub const EMERGENCY_SHUTTLE_AUTHORIZE_TIME: CVarDef<f32> = CVarDef::new(
    "shuttle.emergency_authorize_time",
    10.0,
    CVarFlags::SERVERONLY,
    "За сколько секунд до отлёта объявляется запуск; столько остаётся после досрочного.",
);
pub const EMERGENCY_SHUTTLE_MIN_TRANSIT_TIME: CVarDef<f32> = CVarDef::new(
    "shuttle.emergency_transit_time_min",
    60.0,
    CVarFlags::SERVERONLY,
    "Наименьшее время перехода шаттла эвакуации к ЦК, секунды.",
);
pub const EMERGENCY_SHUTTLE_MAX_TRANSIT_TIME: CVarDef<f32> = CVarDef::new(
    "shuttle.emergency_transit_time_max",
    180.0,
    CVarFlags::SERVERONLY,
    "Наибольшее время перехода шаттла эвакуации к ЦК, секунды.",
);

pub const EMERGENCY_EARLY_LAUNCH_ALLOWED: CVarDef<bool> = CVarDef::new(
    "shuttle.emergency_early_launch_allowed",
    false,
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Можно ли досрочно запустить шаттл эвакуации с его консоли.",
);

pub const EMERGENCY_CONSOLE_UI_KEY: &str = "enum.EmergencyConsoleUiKey.Key";
/// Уровень доступа «отменить все разрешения» (`EmergencyRepealAllAccess`).
const EMERGENCY_REPEAL_ALL_ACCESS: &str = "EmergencyShuttleRepealAll";

/// Тег шлюзов станции для шаттла эвакуации (`DockTag`).
pub const DOCK_TAG: &str = "DockEmergency";
/// Промежуток между шаттлами у ЦК (`ShuttleSpawnBuffer`).
const SHUTTLE_SPAWN_BUFFER: f32 = 1.0;
/// Сколько после перехода до конца раунда (`_bufferTime`).
const BUFFER_TIME: Duration = Duration::from_secs(5);
const DANGER_COLOR: &str = "#FF0000";
const ANNOUNCE_START_SOUND: &str = "/Audio/Misc/notice1.ogg";

fn default_centcomm_map() -> String {
    "/Maps/centcomm.yml".into()
}

/// У станции есть ЦК (`StationCentcommComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StationCentcomm {
    /// Где на карте ЦК встанет следующий шаттл.
    pub shuttle_index: f32,
    pub map: String,
    /// Грид ЦК.
    #[serde(skip)]
    pub entity: Option<Entity>,
    #[serde(skip)]
    pub map_entity: Option<Entity>,
}

impl Default for StationCentcomm {
    fn default() -> Self {
        Self {
            shuttle_index: 0.0,
            map: default_centcomm_map(),
            entity: None,
            map_entity: None,
        }
    }
}

/// У станции есть шаттл эвакуации (`StationEmergencyShuttleComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StationEmergencyShuttle {
    #[serde(skip)]
    pub emergency_shuttle: Option<Entity>,
    pub emergency_shuttle_path: String,
    pub docked_announcement: String,
    pub docked_audio: SoundSpecifier,
    pub nearby_announcement: String,
    pub nearby_audio: SoundSpecifier,
    pub failure_announcement: String,
    pub failure_audio: SoundSpecifier,
    pub launch_extended_message: String,
}

impl Default for StationEmergencyShuttle {
    fn default() -> Self {
        Self {
            emergency_shuttle: None,
            emergency_shuttle_path: "/Maps/Shuttles/emergency.yml".into(),
            docked_announcement: "emergency-shuttle-docked".into(),
            docked_audio: SoundSpecifier::path("/Audio/Announcements/shuttle_dock.ogg"),
            nearby_announcement: "emergency-shuttle-nearby".into(),
            nearby_audio: SoundSpecifier::path(ANNOUNCE_START_SOUND),
            failure_announcement: "emergency-shuttle-good-luck".into(),
            failure_audio: SoundSpecifier::path(ANNOUNCE_START_SOUND),
            launch_extended_message: "emergency-shuttle-extended".into(),
        }
    }
}

/// Грид — шаттл эвакуации (`EmergencyShuttleComponent`).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct EmergencyShuttle;

/// Консоль шаттла эвакуации (`EmergencyShuttleConsoleComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EmergencyShuttleConsole {
    /// Карты, разрешившие запуск, и имя карты в момент разрешения (`AuthorizedEntities`).
    #[serde(skip)]
    pub authorized: Vec<(Entity, String)>,
    pub authorizations_required: i32,
}

impl Default for EmergencyShuttleConsole {
    fn default() -> Self {
        Self {
            authorized: Vec::new(),
            authorizations_required: 3,
        }
    }
}

/// Состояние окна консоли (`EmergencyConsoleBoundUserInterfaceState`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EmergencyConsoleState {
    /// Когда шаттл улетит после досрочного запуска, по часам игры.
    pub early_launch_time: Option<Duration>,
    pub authorizations: Vec<String>,
    pub authorizations_required: i32,
}

/// Действия в окне консоли (`EmergencyShuttleAuthorizeMessage`, `EmergencyShuttleRepealMessage`,
/// `EmergencyShuttleRepealAllMessage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmergencyConsoleMessage {
    Authorize,
    Repeal,
    RepealAll,
}

/// Состояние эвакуации раунда. Ресурс сервера.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct Evacuation {
    /// Сколько секунд до отлёта (`_consoleAccumulator`); `f32::MIN` — шаттл не прибыл.
    pub console_accumulator: f32,
    /// Время перехода к ЦК (`TransitTime`).
    pub transit_time: f32,
    /// Шаттл у станции (`EmergencyShuttleArrived`).
    pub arrived: bool,
    /// Досрочный запуск разрешён (`EarlyLaunchAuthorized`).
    pub early_launch_authorized: bool,
    /// Прыжок к ЦК начат (`_launchedShuttles`).
    pub launched: bool,
    /// Шаттлы улетели (`ShuttlesLeft`).
    pub left: bool,
    /// Запуск объявлен (`_announced`).
    pub announced: bool,
    /// Когда кончится раунд после отлёта (`Timer.Spawn`); `None` — не назначено или отложено.
    pub round_end_at: Option<Duration>,
    /// Время прошлого такта.
    pub last_update: Option<Duration>,
}

impl Default for Evacuation {
    fn default() -> Self {
        Self {
            console_accumulator: f32::MIN,
            transit_time: 0.0,
            arrived: false,
            early_launch_authorized: false,
            launched: false,
            left: false,
            announced: false,
            round_end_at: None,
            last_update: None,
        }
    }
}

/// Досрочный запуск разрешён (`EmergencyShuttleAuthorizedEvent`). Широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct EmergencyShuttleAuthorized;

/// Чем кончилась стыковка (`ShuttleDockResultType`), от лучшего к худшему.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShuttleDockResultType {
    PriorityDock,
    OtherDock,
    NoDock,
    GoodLuck,
}

/// Стыковка шаттла станции (`ShuttleDockResult`).
#[derive(Debug, Clone, PartialEq)]
pub struct ShuttleDockResult {
    pub station: Entity,
    pub target_grid: Option<Entity>,
    pub result_type: ShuttleDockResultType,
    pub config: Option<docking::DockingConfig>,
}

fn now(world: &World) -> Duration {
    world
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default()
}

fn state(world: &mut World) -> bevy_ecs::world::Mut<'_, Evacuation> {
    world.get_resource_or_insert_with(Evacuation::default)
}

fn enabled(world: &World) -> bool {
    world.cvar(&EMERGENCY_SHUTTLE_ENABLED)
}

fn exists(world: &World, entity: Option<Entity>) -> bool {
    entity.is_some_and(|e| world.get_entity(e).is_ok())
}

fn log(world: &mut World, impact: LogImpact, message: &str) {
    add_admin_log(world, LogType::EmergencyShuttle, impact, message, &[], &[]);
}

fn station_name(world: &World, station: Entity) -> String {
    stations::entity_name(world, station)
}

fn with_component<C: Component>(world: &mut World) -> Vec<Entity> {
    let mut list: Vec<Entity> = world
        .query_filtered::<Entity, bevy_ecs::query::With<C>>()
        .iter(world)
        .collect();
    list.sort();
    list
}

/// Шаттл эвакуации у станции (`EmergencyShuttleArrived`).
pub fn emergency_shuttle_arrived(world: &World) -> bool {
    world
        .get_resource::<Evacuation>()
        .is_some_and(|s| s.arrived)
}

/// Шаттл эвакуации станции.
pub fn emergency_shuttle_of(world: &World, station: Entity) -> Option<Entity> {
    world
        .get::<StationEmergencyShuttle>(station)
        .and_then(|s| s.emergency_shuttle)
}

/// Карты ЦК (`GetCentcommMaps`).
pub fn centcomm_maps(world: &mut World) -> Vec<Entity> {
    let mut maps: Vec<Entity> = with_component::<StationCentcomm>(world)
        .into_iter()
        .filter_map(|station| world.get::<StationCentcomm>(station)?.map_entity)
        .collect();
    maps.sort();
    maps.dedup();
    maps
}

/// Сущность на шаттле эвакуации, когда он уже у станции (`IsTargetEscaping`).
pub fn is_target_escaping(world: &World, target: Entity) -> bool {
    emergency_shuttle_arrived(world)
        && world
            .get::<Transform>(target)
            .and_then(Transform::grid)
            .is_some_and(|grid| world.get::<EmergencyShuttle>(grid).is_some())
}

// ЦК и шаттлы.

/// Удалить ЦК станции (`ClearCentcomm`).
fn clear_centcomm(world: &mut World, station: Entity) {
    let Some(centcomm) = world.get::<StationCentcomm>(station).cloned() else {
        return;
    };
    for entity in [centcomm.entity, centcomm.map_entity].into_iter().flatten() {
        if world.get_entity(entity).is_ok() {
            world.queue_delete(entity);
        }
    }
    if let Some(mut stored) = world.get_mut::<StationCentcomm>(station) {
        stored.entity = None;
        stored.map_entity = None;
    }
}

/// Загрузить ЦК станции на новую карту (`AddCentcomm`); уже загруженная другой станцией ЦК
/// используется общая.
pub fn add_centcomm(world: &mut World, station: Entity) {
    let Some(centcomm) = world.get::<StationCentcomm>(station).cloned() else {
        return;
    };
    // Грид ЦК задан заранее — только карта.
    if exists(world, centcomm.entity) {
        let map = centcomm
            .entity
            .and_then(|e| world.get::<Transform>(e))
            .and_then(Transform::map);
        if let Some(mut stored) = world.get_mut::<StationCentcomm>(station) {
            stored.map_entity = map;
        }
        return;
    }
    if centcomm.map_entity.is_some() || centcomm.entity.is_some() {
        eprintln!("ЦК станции {station:?} уже добавлена");
        return;
    }
    for other in with_component::<StationCentcomm>(world) {
        if other == station {
            continue;
        }
        let Some(found) = world.get::<StationCentcomm>(other).cloned() else {
            continue;
        };
        if !exists(world, found.map_entity) || !exists(world, found.entity) {
            if found.map_entity.is_some() || found.entity.is_some() {
                eprintln!("ЦК станции {other:?} испорчена");
                clear_centcomm(world, other);
            }
            continue;
        }
        if let Some(mut stored) = world.get_mut::<StationCentcomm>(station) {
            stored.map_entity = found.map_entity;
            stored.entity = found.entity;
            stored.shuttle_index = found.shuttle_index;
        }
        return;
    }
    if centcomm.map.is_empty() {
        eprintln!("у станции {station:?} нет карты ЦК");
        return;
    }
    let (map, map_id) = world.create_map(true);
    let path = stations::resource_path(&centcomm.map);
    let grid =
        match crate::map_commands::load_grid_file(world, map_id, &path, Vec2::ZERO, Angle::ZERO) {
            Ok((grid, _)) => grid,
            Err(error) => {
                eprintln!("ЦК: {error}");
                world.queue_delete(map);
                return;
            }
        };
    if world.get::<Transform>(grid).and_then(Transform::map) != Some(map) {
        eprintln!("грид ЦК не на своей карте");
        world.queue_delete(map);
        return;
    }
    let name = loc(world, "map-name-centcomm", &[]);
    if let Some(mut meta) = world.get_mut::<MetaData>(map) {
        meta.name = Some(name);
    }
    if let Some(mut stored) = world.get_mut::<StationCentcomm>(station) {
        stored.map_entity = Some(map);
        stored.entity = Some(grid);
    }
}

/// Загрузить шаттл эвакуации станции у её ЦК (`AddEmergencyShuttle`).
pub fn add_emergency_shuttle(world: &mut World, station: Entity) {
    if !enabled(world) {
        return;
    }
    let (Some(shuttle), Some(centcomm)) = (
        world.get::<StationEmergencyShuttle>(station).cloned(),
        world.get::<StationCentcomm>(station).cloned(),
    ) else {
        return;
    };
    if let Some(existing) = shuttle.emergency_shuttle {
        if world.get_entity(existing).is_ok() {
            eprintln!("у станции {station:?} уже есть шаттл эвакуации");
            return;
        }
        if let Some(mut stored) = world.get_mut::<StationEmergencyShuttle>(station) {
            stored.emergency_shuttle = None;
        }
    }
    let Some(map_id) = centcomm
        .map_entity
        .and_then(|map| world.get::<Map>(map))
        .map(|map| map.id)
    else {
        eprintln!("шаттл эвакуации станции {station:?}: ЦК не загружена");
        return;
    };
    let path = stations::resource_path(&shuttle.emergency_shuttle_path);
    // Достаточно далеко от ЦК (как в SS14, без проверки границ).
    let offset = Vec2::new(500.0 + centcomm.shuttle_index, 0.0);
    let grid = match crate::map_commands::load_grid_file(world, map_id, &path, offset, Angle::ZERO)
    {
        Ok((grid, _)) => grid,
        Err(error) => {
            eprintln!("шаттл эвакуации станции {station:?}: {error}");
            return;
        }
    };
    let width = docking::grid_local_aabb(world, grid).map_or(0.0, |a| a.size().x);
    let index = centcomm.shuttle_index + width + SHUTTLE_SPAWN_BUFFER;
    // Общий счёт для всех станций с той же ЦК.
    for other in with_component::<StationCentcomm>(world) {
        if let Some(mut stored) = world.get_mut::<StationCentcomm>(other)
            && stored.map_entity == centcomm.map_entity
        {
            stored.shuttle_index = index;
        }
    }
    if let Some(mut stored) = world.get_mut::<StationEmergencyShuttle>(station) {
        stored.emergency_shuttle = Some(grid);
    }
    world.entity_mut(grid).insert(EmergencyShuttle);
}

/// Станция создана (`MapInit` ЦК, `StationPostInitEvent`): ЦК, затем шаттл.
fn on_station_initialized(world: &mut World, station: Entity) {
    if !enabled(world) {
        return;
    }
    if world.get::<StationCentcomm>(station).is_some() {
        add_centcomm(world, station);
    }
    if world.get::<StationEmergencyShuttle>(station).is_some() {
        add_emergency_shuttle(world, station);
    }
}

/// Шаттл включили посреди раунда (`SetupEmergencyShuttle`).
fn setup_emergency_shuttle(world: &mut World) {
    for station in with_component::<StationCentcomm>(world) {
        add_centcomm(world, station);
    }
    for station in with_component::<StationEmergencyShuttle>(world) {
        add_emergency_shuttle(world, station);
    }
}

/// Шаттл выключили (`CleanupEmergencyShuttle`): ЦК удаляется.
fn cleanup_emergency_shuttle(world: &mut World) {
    for station in with_component::<StationCentcomm>(world) {
        clear_centcomm(world, station);
        world.entity_mut(station).remove::<StationCentcomm>();
    }
}

// Прибытие.

/// Самый большой по площади грид станции (`GetLargestGrid`).
pub fn largest_grid(world: &mut World, station: Entity) -> Option<Entity> {
    let grids = world.get::<stations::StationData>(station)?.grids.clone();
    let mut best: Option<(Entity, f32)> = None;
    for grid in grids {
        if world.get::<MapGrid>(grid).is_none() {
            continue;
        }
        let area = docking::grid_local_aabb(world, grid).map_or(0.0, |a| a.size().x * a.size().y);
        if best.is_none_or(|(_, largest)| area >= largest) {
            best = Some((grid, area));
        }
    }
    best.map(|(grid, _)| grid)
}

/// Пристыковать шаттл станции (`DockSingleEmergencyShuttle`).
pub fn dock_single_emergency_shuttle(
    world: &mut World,
    station: Entity,
) -> Option<ShuttleDockResult> {
    let shuttle = emergency_shuttle_of(world, station);
    let Some(shuttle) = shuttle.filter(|s| world.get::<MapGrid>(*s).is_some()) else {
        eprintln!("шаттл эвакуации станции {station:?} не загружен");
        return None;
    };
    let name = station_name(world, station);
    let Some(target) = largest_grid(world, station) else {
        log(
            world,
            LogImpact::High,
            &format!("Emergency shuttle {name} unable to dock with station {name}"),
        );
        return Some(ShuttleDockResult {
            station,
            target_grid: None,
            result_type: ShuttleDockResultType::GoodLuck,
            config: None,
        });
    };
    let config = shuttles::try_ftl_dock(world, shuttle, target, Some(DOCK_TAG));
    let result_type = match &config {
        Some(config) => {
            log(
                world,
                LogImpact::High,
                &format!("Emergency shuttle {name} docked with stations"),
            );
            if docking::is_config_priority(world, config, Some(DOCK_TAG)) {
                ShuttleDockResultType::PriorityDock
            } else {
                ShuttleDockResultType::OtherDock
            }
        }
        None => {
            log(
                world,
                LogImpact::High,
                &format!("Emergency shuttle {name} unable to find a valid docking port for {name}"),
            );
            ShuttleDockResultType::NoDock
        }
    };
    Some(ShuttleDockResult {
        station,
        target_grid: Some(target),
        result_type,
        config,
    })
}

/// Объявить о прибытии (`AnnounceShuttleDock`).
pub fn announce_shuttle_dock(world: &mut World, result: &ShuttleDockResult, extended: bool) {
    let Some(settings) = world
        .get::<StationEmergencyShuttle>(result.station)
        .cloned()
    else {
        return;
    };
    let (Some(target), Some(shuttle)) = (result.target_grid, settings.emergency_shuttle) else {
        let text = loc(world, &settings.failure_announcement, &[]);
        crate::chat::dispatch_station_announcement(world, result.station, &text, None, false, None);
        world.play_global(&settings.failure_audio, NetTarget::All);
        return;
    };
    let angle = docking::get_angle(world, shuttle, target).unwrap_or_default();
    let direction = crate::examine::format_direction(world, angle.direction());
    let location = world
        .map_coordinates(shuttle)
        .map(|coordinates| crate::ghost::nearest_beacon_string(world, coordinates))
        .map(|text| crate::examine::strip_markup(&text))
        .unwrap_or_default();
    let extended_text = if extended {
        loc(world, &settings.launch_extended_message, &[])
    } else {
        String::new()
    };
    let (key, audio) = if result.result_type == ShuttleDockResultType::NoDock {
        (&settings.nearby_announcement, &settings.nearby_audio)
    } else {
        (&settings.docked_announcement, &settings.docked_audio)
    };
    let time = format!("{:.0}", state(world).console_accumulator);
    let text = loc(
        world,
        key,
        &[
            ("time", LocArg::from(time)),
            ("direction", LocArg::from(direction)),
            ("location", LocArg::from(location)),
            ("extended", LocArg::from(extended_text)),
        ],
    );
    crate::chat::dispatch_station_announcement(world, result.station, &text, None, false, None);
    world.play_global(audio, NetTarget::All);
}

/// Пригнать шаттлы к станциям и начать отсчёт до отлёта (`DockEmergencyShuttle`); без шаттла
/// эвакуации раунд кончается.
pub fn dock_emergency_shuttle(world: &mut World) {
    if emergency_shuttle_arrived(world) {
        return;
    }
    if !enabled(world) {
        round_end::end_round(world, None);
        return;
    }
    let dock_time = world.cvar(&EMERGENCY_SHUTTLE_DOCK_TIME);
    {
        let mut state = state(world);
        state.console_accumulator = dock_time;
        state.arrived = true;
    }
    world.resource_mut::<round_end::RoundEnd>().shuttle_arrived = true;
    let results: Vec<ShuttleDockResult> = with_component::<StationEmergencyShuttle>(world)
        .into_iter()
        .filter_map(|station| dock_single_emergency_shuttle(world, station))
        .collect();
    // Таймер общий: по худшему результату.
    let multiplier = match results.iter().map(|r| r.result_type).max() {
        Some(ShuttleDockResultType::OtherDock) => world.cvar(&DOCK_TIME_MULTIPLIER_OTHER_DOCK),
        Some(ShuttleDockResultType::NoDock) => world.cvar(&DOCK_TIME_MULTIPLIER_NO_DOCK),
        _ => 1.0,
    };
    state(world).console_accumulator *= multiplier;
    for result in &results {
        announce_shuttle_dock(world, result, multiplier > 1.0);
    }
    world.raise_broadcast(&mut round_end::RoundEndChanged);
}

// Отлёт.

/// Объявить запуск (`AnnounceLaunch`).
fn announce_launch(world: &mut World) {
    if state(world).announced {
        return;
    }
    state(world).announced = true;
    let time = format!("{:.0}", state(world).console_accumulator);
    let text = loc(
        world,
        "emergency-shuttle-launch-time",
        &[("consoleAccumulator", LocArg::from(time))],
    );
    crate::chat::dispatch_global_announcement(world, &text, None, false, Some(DANGER_COLOR.into()));
    world.play_global(&SoundSpecifier::path(ANNOUNCE_START_SOUND), NetTarget::All);
}

/// Досрочный запуск (`EarlyLaunch`): до отлёта остаётся `shuttle.emergency_authorize_time`.
pub fn early_launch(world: &mut World) -> bool {
    let authorize = world.cvar(&EMERGENCY_SHUTTLE_AUTHORIZE_TIME);
    let state_now = state(world).clone();
    if state_now.early_launch_authorized
        || !state_now.arrived
        || state_now.console_accumulator <= authorize
    {
        return false;
    }
    log(
        world,
        LogImpact::High,
        "Emergency shuttle launch authorized",
    );
    {
        let mut state = state(world);
        state.console_accumulator = authorize;
        state.early_launch_authorized = true;
    }
    world.raise_broadcast(&mut EmergencyShuttleAuthorized);
    announce_launch(world);
    update_all_emergency_consoles(world);
    world.raise_broadcast(&mut round_end::RoundEndChanged);
    true
}

/// Отложить конец раунда после отлёта (`DelayEmergencyRoundEnd`).
pub fn delay_emergency_round_end(world: &mut World) -> bool {
    state(world).round_end_at.take().is_some()
}

/// Начать прыжок шаттлов к ЦК.
fn launch_shuttles(world: &mut World) {
    let (startup, transit) = {
        let state = state(world);
        (state.console_accumulator, state.transit_time)
    };
    for station in with_component::<StationEmergencyShuttle>(world) {
        let Some(shuttle) =
            emergency_shuttle_of(world, station).filter(|s| world.get::<MapGrid>(*s).is_some())
        else {
            continue;
        };
        let Some(centcomm) = world.get::<StationCentcomm>(station).cloned() else {
            continue;
        };
        if let Some(target) = centcomm.entity.filter(|e| world.get_entity(*e).is_ok()) {
            shuttles::ftl_to_dock(world, shuttle, target, Some(startup), Some(transit), None);
            continue;
        }
        if let Some(map) = centcomm.map_entity.filter(|e| world.get_entity(*e).is_ok()) {
            let (angle, distance) = {
                let mut random = crate::random::random(world);
                (
                    random.next_f32() * std::f32::consts::TAU,
                    random.next_f32() * 1000.0,
                )
            };
            let position = Angle(angle).rotate(Vec2::new(distance, 0.0));
            shuttles::ftl_to_coordinates(
                world,
                shuttle,
                EntityCoordinates::new(map, position),
                Angle::ZERO,
                Some(startup),
                Some(transit),
            );
        }
    }
}

/// Такт таймера отлёта (`UpdateEmergencyConsole`).
pub fn update_emergency_console(world: &mut World, frame_time: f32) {
    let startup = world.cvar(&shuttles::FTL_STARTUP_TIME);
    let travel = world.cvar(&shuttles::FTL_TRAVEL_TIME);
    let authorize = world.cvar(&EMERGENCY_SHUTTLE_AUTHORIZE_TIME);
    let transit = state(world).transit_time;
    // Пока шаттл не прибыл, таймер стоит.
    let min_time = (-(transit - (startup + travel + 1.0))).min(0.0);
    if state(world).console_accumulator < min_time {
        return;
    }
    state(world).console_accumulator -= frame_time;
    let current = state(world).clone();
    if !current.launched
        && current.console_accumulator <= authorize
        && !current.early_launch_authorized
    {
        announce_launch(world);
    }
    if !current.launched && current.console_accumulator <= startup {
        state(world).launched = true;
        launch_shuttles(world);
    }
    if !current.left && current.console_accumulator <= 0.0 {
        state(world).left = true;
        let text = loc(
            world,
            "emergency-shuttle-left",
            &[("transitTime", LocArg::from(format!("{transit:.0}")))],
        );
        crate::chat::dispatch_global_announcement(world, &text, None, true, None);
        let at = now(world) + Duration::from_secs_f32(transit.max(0.0)) + BUFFER_TIME;
        state(world).round_end_at = Some(at);
    }
}

/// Сброс к новому раунду (`CleanupEmergencyConsole`): время перехода случайно, кратно 10.
fn cleanup_emergency_console(world: &mut World) {
    let min = world.cvar(&EMERGENCY_SHUTTLE_MIN_TRANSIT_TIME);
    let max = world.cvar(&EMERGENCY_SHUTTLE_MAX_TRANSIT_TIME).max(min);
    let roll = crate::random::random(world).next_f32();
    let transit = ((min + (max - min) * roll) / 10.0).round() * 10.0;
    world.insert_resource(Evacuation {
        transit_time: transit,
        ..Evacuation::default()
    });
}

/// Такт: таймер вне лобби, конец раунда после перехода.
pub fn update(world: &mut World) {
    let time = now(world);
    let last = state(world).last_update.replace(time);
    let frame_time = last.map_or(0.0, |last| time.saturating_sub(last).as_secs_f32());
    if game_ticker::run_level(world) != GameRunLevel::PreRoundLobby {
        update_emergency_console(world, frame_time);
    }
    if state(world).round_end_at.is_some_and(|at| at <= time) {
        state(world).round_end_at = None;
        round_end::end_round(world, None);
    }
}

// Консоль досрочного запуска.

fn emergency_consoles(world: &mut World) -> Vec<Entity> {
    with_component::<EmergencyShuttleConsole>(world)
}

/// Обновить окно консоли (`UpdateConsoleState`).
pub fn update_console_state(world: &mut World, console: Entity) {
    let Some(comp) = world.get::<EmergencyShuttleConsole>(console).cloned() else {
        return;
    };
    let state = state(world).clone();
    let early_launch_time = state
        .early_launch_authorized
        .then(|| now(world) + Duration::from_secs_f32(state.console_accumulator.max(0.0)));
    world.set_ui_state(
        console,
        EMERGENCY_CONSOLE_UI_KEY,
        &EmergencyConsoleState {
            early_launch_time,
            authorizations: comp
                .authorized
                .iter()
                .map(|(_, name)| name.clone())
                .collect(),
            authorizations_required: comp.authorizations_required,
        },
    );
}

/// Обновить окна всех консолей (`UpdateAllEmergencyConsoles`).
pub fn update_all_emergency_consoles(world: &mut World) {
    for console in emergency_consoles(world) {
        update_console_state(world, console);
    }
}

fn console_denied(world: &mut World, user: Entity) {
    let text = loc(world, "emergency-shuttle-console-denied", &[]);
    world.popup_cursor(&text, user, PopupType::Medium);
}

/// Разрешений хватает — досрочный запуск (`CheckForLaunch`).
fn check_for_launch(world: &mut World, console: Entity) -> bool {
    let Some(comp) = world.get::<EmergencyShuttleConsole>(console) else {
        return false;
    };
    if (comp.authorized.len() as i32) < comp.authorizations_required
        || state(world).early_launch_authorized
    {
        return false;
    }
    early_launch(world);
    true
}

/// Разрешить запуск своей ID-картой (`OnEmergencyAuthorize`).
fn authorize(world: &mut World, console: Entity, user: Entity) {
    let Some(card) = crate::access::find_id_card(world, user)
        .filter(|card| crate::access::is_allowed(world, *card, console))
    else {
        console_denied(world, user);
        return;
    };
    let Some(comp) = world.get::<EmergencyShuttleConsole>(console) else {
        return;
    };
    if comp.authorized.iter().any(|(entity, _)| *entity == card) {
        return;
    }
    let name = stations::entity_name(world, card);
    let remaining = {
        let Some(mut comp) = world.get_mut::<EmergencyShuttleConsole>(console) else {
            return;
        };
        comp.authorized.push((card, name));
        comp.authorizations_required - comp.authorized.len() as i32
    };
    let player = stations::entity_name(world, user);
    log(
        world,
        LogImpact::High,
        &format!("Emergency shuttle early launch AUTH by {player}"),
    );
    if remaining > 0 {
        let text = loc(
            world,
            "emergency-shuttle-console-auth-left",
            &[("remaining", LocArg::from(i64::from(remaining)))],
        );
        crate::chat::dispatch_global_announcement(
            world,
            &text,
            None,
            false,
            Some(DANGER_COLOR.into()),
        );
    }
    if !check_for_launch(world, console) {
        world.play_global(&SoundSpecifier::path(ANNOUNCE_START_SOUND), NetTarget::All);
    }
    update_all_emergency_consoles(world);
}

/// Отозвать своё разрешение (`OnEmergencyRepeal`).
fn repeal(world: &mut World, console: Entity, user: Entity) {
    let Some(card) = crate::access::find_id_card(world, user)
        .filter(|card| crate::access::is_allowed(world, *card, console))
    else {
        console_denied(world, user);
        return;
    };
    let remaining = {
        let Some(mut comp) = world.get_mut::<EmergencyShuttleConsole>(console) else {
            return;
        };
        let before = comp.authorized.len();
        comp.authorized.retain(|(entity, _)| *entity != card);
        if comp.authorized.len() == before {
            return;
        }
        comp.authorizations_required - comp.authorized.len() as i32
    };
    let player = stations::entity_name(world, user);
    log(
        world,
        LogImpact::High,
        &format!("Emergency shuttle early launch REPEAL by {player}"),
    );
    let text = loc(
        world,
        "emergency-shuttle-console-auth-revoked",
        &[("remaining", LocArg::from(i64::from(remaining)))],
    );
    crate::chat::dispatch_global_announcement(world, &text, None, true, None);
    check_for_launch(world, console);
    update_all_emergency_consoles(world);
}

/// Отозвать все разрешения (`OnEmergencyRepealAll`): нужен доступ `EmergencyShuttleRepealAll`.
fn repeal_all(world: &mut World, console: Entity, user: Entity) {
    if !crate::access::access_tags(world, user).contains(EMERGENCY_REPEAL_ALL_ACCESS) {
        console_denied(world, user);
        return;
    }
    let required = {
        let Some(mut comp) = world.get_mut::<EmergencyShuttleConsole>(console) else {
            return;
        };
        if comp.authorized.is_empty() {
            return;
        }
        comp.authorized.clear();
        comp.authorizations_required
    };
    let player = stations::entity_name(world, user);
    log(
        world,
        LogImpact::High,
        &format!("Emergency shuttle early launch REPEAL ALL by {player}"),
    );
    let text = loc(
        world,
        "emergency-shuttle-console-auth-revoked",
        &[("remaining", LocArg::from(i64::from(required)))],
    );
    crate::chat::dispatch_global_announcement(world, &text, None, true, None);
    update_all_emergency_consoles(world);
}

// Команды.

fn dockemergencyshuttle(world: &mut World, _: &mut ConsoleShell, _: &[String]) {
    dock_emergency_shuttle(world);
}

fn launchemergencyshuttle(world: &mut World, _: &mut ConsoleShell, _: &[String]) {
    early_launch(world);
}

fn delayroundend(world: &mut World, shell: &mut ConsoleShell, _: &[String]) {
    let key = if delay_emergency_round_end(world) {
        "emergency-shuttle-command-round-yes"
    } else {
        "emergency-shuttle-command-round-no"
    };
    shell.write_line(loc(world, key, &[]));
}

pub fn install(world: &mut World, side: Side) {
    use astrocore_ecs::RegistryWorldExt;
    world.register_data_component::<StationCentcomm>();
    world.register_data_component::<StationEmergencyShuttle>();
    world.register_data_component::<EmergencyShuttleConsole>();
    world.register_cvar(&EMERGENCY_EARLY_LAUNCH_ALLOWED);
    if !side.is_server() {
        return;
    }
    world.register_cvar(&EMERGENCY_SHUTTLE_ENABLED);
    world.register_ui_message::<EmergencyConsoleMessage>();
    world.subscribe::<EmergencyShuttleConsole, astrocore_ecs::ComponentStartup>(
        |world, console, _| update_console_state(world, console),
    );
    world.subscribe::<EmergencyShuttleConsole, BoundUiMessage<EmergencyConsoleMessage>>(
        |world, console, event| match event.message {
            EmergencyConsoleMessage::Authorize => authorize(world, console, event.actor),
            EmergencyConsoleMessage::Repeal => repeal(world, console, event.actor),
            EmergencyConsoleMessage::RepealAll => repeal_all(world, console, event.actor),
        },
    );
    // Окно не открывается, если досрочный запуск запрещён (`OnEmergencyOpenAttempt`).
    world.subscribe::<EmergencyShuttleConsole, crate::activatable_ui::ActivatableUiOpenAttempt>(
        |world, console, event| {
            if world.cvar(&EMERGENCY_EARLY_LAUNCH_ALLOWED) {
                return;
            }
            event.cancelled = true;
            let text = loc(world, "emergency-shuttle-console-no-early-launches", &[]);
            world.popup_entity_for(&text, console, event.user, PopupType::Small);
        },
    );
    for def in [
        &EMERGENCY_SHUTTLE_DOCK_TIME,
        &DOCK_TIME_MULTIPLIER_OTHER_DOCK,
        &DOCK_TIME_MULTIPLIER_NO_DOCK,
        &EMERGENCY_SHUTTLE_AUTHORIZE_TIME,
        &EMERGENCY_SHUTTLE_MIN_TRANSIT_TIME,
        &EMERGENCY_SHUTTLE_MAX_TRANSIT_TIME,
    ] {
        world.register_cvar(def);
    }
    world.init_resource::<Evacuation>();
    world.register_command(ConsoleCommand {
        name: "dockemergencyshuttle",
        description: "Пригоняет шаттл эвакуации и стыкует со станцией, если получится.",
        help: "dockemergencyshuttle",
        execute: dockemergencyshuttle,
        complete: None,
    });
    world.register_command(ConsoleCommand {
        name: "launchemergencyshuttle",
        description: "Досрочно запускает шаттл эвакуации, если можно.",
        help: "launchemergencyshuttle",
        execute: launchemergencyshuttle,
        complete: None,
    });
    world.register_command(ConsoleCommand {
        name: "delayroundend",
        description: "Останавливает таймер конца раунда после отлёта шаттла эвакуации.",
        help: "delayroundend",
        execute: delayroundend,
        complete: None,
    });
    world.subscribe_broadcast::<stations::StationInitialized>(|world, event| {
        on_station_initialized(world, event.station);
    });
    world.subscribe_broadcast::<game_ticker::RoundStarting>(|world, _| {
        cleanup_emergency_console(world);
    });
    world.subscribe_broadcast::<game_ticker::RoundRestartCleanup>(|world, _| {
        state(world).round_end_at = None;
    });
    world.subscribe_broadcast::<round_end::ShuttleCountdownFinished>(|world, event| {
        // Без шаттла эвакуации раунд кончает сам `round_end`.
        if enabled(world) {
            event.handled = true;
            dock_emergency_shuttle(world);
        }
    });
    world.subscribe_broadcast::<CVarChanged>(|world, event| {
        if event.name != EMERGENCY_SHUTTLE_ENABLED.name {
            return;
        }
        if enabled(world) {
            setup_emergency_shuttle(world);
        } else {
            cleanup_emergency_shuttle(world);
        }
    });
    world.subscribe::<StationCentcomm, astrocore_ecs::EntityTerminating>(|world, station, _| {
        clear_centcomm(world, station);
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
