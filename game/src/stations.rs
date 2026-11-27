//! Станции и должности: карта раунда (`gameMap`), станция из гридов карты, имя станции,
//! слоты должностей, распределение игроков по должностям в начале раунда и выбор должности
//! при позднем входе. Образец — `GameMapPrototype`, `StationConfig`, `ServerStationSystem`
//! (`OnPostGameMapLoad`, `InitializeNewStation`, `AddGridToStation`), `StationNameSystem`,
//! `NanotrasenNameGenerator`, `ServerStationJobsSystem` (`TryAssignJob`, `TryAdjustJobSlot`,
//! `PickBestAvailableJobWithPriority`, `GenerateJobsAvailableEvent`) и
//! `ServerStationJobsSystem.Roundstart` (`AssignJobs`, `AssignOverflowJobs`,
//! `CalcExtendedAccess`) SS14 @ 2f4a2aad.
//!
//! Нет пока: запретов на роли и требований по наигранному времени при распределении,
//! ограничений антагонистов на должности, станционных событий, разделения гридов.

use std::collections::{BTreeMap, BTreeSet};

use astrocore_console::{CVarDef, CVarFlags, ConfigWorldExt};
use astrocore_data::DataNode;
use astrocore_ecs::{ComponentInit, EventWorldExt, LifecycleWorldExt, MetaData, RegistryWorldExt};
use astrocore_net::{NetEventWorldExt, NetUserId};
use astrocore_prototypes::{EntityOverrides, PrototypeWorldExt, Prototypes};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::profiles::{HumanoidCharacterProfile, JobPriority, PreferenceUnavailableMode};
use crate::random::GameRandom;

/// Что делать с незанятым минимумом должности (`MinimumJobFallback`): 0 — ничего, 1 —
/// игрок того же отдела, 2 — затем любой подходящий.
pub const GAME_MINIMUM_JOB_FALLBACK: CVarDef<i32> = CVarDef::new(
    "game.minimum_job_fallback",
    0,
    CVarFlags::SERVERONLY,
    "Кого ставить на обязательную должность без желающих: 0 — никого, 1 — из отдела, 2 — любого.",
);

/// Запретить поздний вход (`GameDisallowLateJoins`).
pub const GAME_DISALLOW_LATE_JOINS: CVarDef<bool> = CVarDef::new(
    "game.disallowlatejoins",
    false,
    CVarFlags::ARCHIVE.union(CVarFlags::SERVERONLY),
    "Запретить вход в идущий раунд.",
);

// Карта раунда.

/// Настройка станции карты (`StationConfig`).
#[derive(Debug, Clone, PartialEq)]
pub struct StationConfig {
    pub station_proto: String,
    /// Компоненты, заменяющие компоненты прототипа станции (`type` + поля).
    pub components: Vec<DataNode>,
}

/// Карта раунда (`GameMapPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct GameMap {
    pub id: String,
    pub map_name: String,
    pub map_path: String,
    pub min_players: i32,
    pub max_players: i32,
    pub fallback: bool,
    pub job_weights: Option<String>,
    pub stations: BTreeMap<String, StationConfig>,
}

fn text(node: &DataNode, key: &str) -> Option<String> {
    node.get(key)?.as_str().map(str::to_owned)
}

fn number(node: &DataNode, key: &str) -> Option<i32> {
    node.get(key)?.as_str()?.trim().parse().ok()
}

/// Карта раунда по прототипу `gameMap`.
pub fn game_map(prototypes: &Prototypes, id: &str) -> Option<GameMap> {
    let node = prototypes.resolved_node("gameMap", id)?;
    let stations = node
        .get("stations")
        .and_then(DataNode::as_map)
        .map(|stations| {
            stations
                .iter()
                .filter_map(|(id, config)| {
                    Some((
                        id.to_owned(),
                        StationConfig {
                            station_proto: text(config, "stationProto")?,
                            components: config
                                .get("components")
                                .and_then(DataNode::as_seq)
                                .map(<[DataNode]>::to_vec)
                                .unwrap_or_default(),
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(GameMap {
        id: id.to_owned(),
        map_name: text(node, "mapName").unwrap_or_else(|| id.to_owned()),
        map_path: text(node, "mapPath").unwrap_or_default(),
        min_players: number(node, "minPlayers").unwrap_or(0),
        max_players: number(node, "maxPlayers").unwrap_or(i32::MAX),
        fallback: text(node, "fallback").is_some_and(|t| t == "true"),
        job_weights: text(node, "jobWeights"),
        stations,
    })
}

/// Путь карты для ресурсов: без ведущей `/`.
pub fn resource_path(map_path: &str) -> String {
    map_path.trim_start_matches('/').to_owned()
}

// Компоненты станций.

/// Грид станет частью станции карты с этим идентификатором (`BecomesStationComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BecomesStation {
    pub id: String,
}

/// Станция (`StationDataComponent`): её гриды и веса должностей карты.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StationData {
    #[serde(skip)]
    pub grids: Vec<Entity>,
    #[serde(skip)]
    pub job_weights: Option<String>,
}

/// Грид принадлежит станции (`StationMemberComponent`).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct StationMember {
    pub station: Entity,
}

/// Генератор имени (`NanotrasenNameGenerator`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NameGenerator {
    pub prefix_creator: String,
}

/// Имя станции по шаблону (`StationNameSetupComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StationNameSetup {
    pub map_name_template: String,
    pub name_generator: Option<NameGenerator>,
}

/// Слоты должностей станции (`StationJobsComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StationJobs {
    /// Должность → [в начале раунда, всего]; −1 — без предела (`availableJobs`).
    #[serde(rename = "availableJobs")]
    pub setup_available_jobs: BTreeMap<String, [i32; 2]>,
    pub extended_access_threshold: i32,
    /// Свободные слоты; `None` — без предела.
    #[serde(skip)]
    pub job_list: BTreeMap<String, Option<i32>>,
    #[serde(skip)]
    pub total_jobs: i32,
    #[serde(skip)]
    pub mid_round_total_jobs: i32,
    #[serde(skip)]
    pub overflow_jobs: BTreeSet<String>,
    #[serde(skip)]
    pub player_jobs: BTreeMap<NetUserId, Vec<String>>,
    #[serde(skip)]
    pub extended_access: bool,
}

impl Default for StationJobs {
    fn default() -> Self {
        Self {
            setup_available_jobs: BTreeMap::new(),
            extended_access_threshold: 15,
            job_list: BTreeMap::new(),
            total_jobs: 0,
            mid_round_total_jobs: 0,
            overflow_jobs: BTreeSet::new(),
            player_jobs: BTreeMap::new(),
            extended_access: false,
        }
    }
}

/// Станция появления игроков (`StationSpawningComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StationSpawning {}

/// Станция создана (`StationInitializedEvent`). Широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct StationInitialized {
    pub station: Entity,
}

/// Список должностей изменился: свободные слоты надо разослать игрокам.
#[derive(bevy_ecs::resource::Resource, Debug, Default)]
pub struct JobsAvailableDirty(pub bool);

fn jobs_changed(world: &mut World) {
    world
        .get_resource_or_insert_with(JobsAvailableDirty::default)
        .0 = true;
}

// Создание станции.

/// Имя станции по шаблону: `{0}` — `NT` + префикс, `{1}` — код и номер (`FormatName`).
pub fn generate_station_name(random: &mut GameRandom, setup: &StationNameSetup) -> String {
    let Some(generator) = &setup.name_generator else {
        return setup.map_name_template.clone();
    };
    const SUFFIX_CODES: [&str; 5] = ["LV", "NX", "EV", "QT", "PR"];
    let code = SUFFIX_CODES[random.range(0, SUFFIX_CODES.len() as i32) as usize];
    let number = random.range(0, 999);
    setup
        .map_name_template
        .replace("{0}", &format!("NT{}", generator.prefix_creator))
        .replace("{1}", &format!("{code}-{number:03}"))
}

fn set_name(world: &mut World, entity: Entity, name: &str) {
    if let Some(mut meta) = world.get_mut::<MetaData>(entity) {
        meta.name = Some(name.to_owned());
    }
}

/// Имя сущности.
pub fn entity_name(world: &World, entity: Entity) -> String {
    world
        .get::<MetaData>(entity)
        .and_then(|m| m.name.clone())
        .unwrap_or_default()
}

/// Компоненты настройки станции — изменения прототипа без ключа `type`.
fn component_overrides(components: &[DataNode]) -> Vec<(String, DataNode)> {
    components
        .iter()
        .filter_map(|node| {
            let name = node.get("type")?.as_str()?.to_owned();
            let mut map = node.as_map()?.clone();
            map.remove("type");
            Some((name, DataNode::map(map)))
        })
        .collect()
}

/// Добавить грид станции (`AddGridToStation`): грид получает имя станции.
pub fn add_grid_to_station(world: &mut World, station: Entity, grid: Entity, name: Option<&str>) {
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        set_name(world, grid, name);
    }
    world.entity_mut(grid).insert(StationMember { station });
    if let Some(mut data) = world.get_mut::<StationData>(station) {
        data.grids.push(grid);
    }
}

/// Новая станция (`InitializeNewStation`): сущность из прототипа с компонентами настройки,
/// имя, гриды, слоты должностей.
pub fn initialize_new_station(
    world: &mut World,
    config: &StationConfig,
    grids: &[Entity],
    name: Option<&str>,
    job_weights: Option<String>,
) -> Option<Entity> {
    let overrides = component_overrides(&config.components);
    let refs = EntityOverrides {
        components: overrides.iter().map(|(n, d)| (n.as_str(), d)).collect(),
        missing: Vec::new(),
    };
    let station = match world.spawn_prototype_with(&config.station_proto, &refs) {
        Ok(station) => station,
        Err(error) => {
            eprintln!("станция {}: {error}", config.station_proto);
            return None;
        }
    };
    world.initialize_and_start(station, false);
    if world.get::<StationData>(station).is_none() {
        world.entity_mut(station).insert(StationData::default());
    }
    if let Some(name) = name {
        set_name(world, station, name);
    }
    if let Some(mut data) = world.get_mut::<StationData>(station) {
        data.job_weights = job_weights;
    }
    let name = entity_name(world, station);
    for &grid in grids {
        add_grid_to_station(world, station, grid, Some(&name));
    }
    station_initialized(world, station);
    Some(station)
}

/// Слоты должностей из настройки (`OnStationInitialized`).
fn station_initialized(world: &mut World, station: Entity) {
    if let Some(mut jobs) = world.get_mut::<StationJobs>(station) {
        jobs.job_list = jobs
            .setup_available_jobs
            .iter()
            .map(|(job, [_, total])| (job.clone(), (*total >= 0).then_some(*total)))
            .collect();
        jobs.total_jobs = jobs.job_list.values().map(|v| v.unwrap_or(0)).sum();
    }
    jobs_changed(world);
    world.raise_broadcast(&mut StationInitialized { station });
}

/// Станции из гридов загруженной карты (`OnPostGameMapLoad`): гриды с `BecomesStation`
/// собираются по идентификатору станции карты.
pub fn setup_stations(
    world: &mut World,
    map: &GameMap,
    grids: &[Entity],
    name: Option<&str>,
) -> Vec<Entity> {
    let mut by_id: BTreeMap<String, Vec<Entity>> = BTreeMap::new();
    for &grid in grids {
        if let Some(becomes) = world.get::<BecomesStation>(grid) {
            by_id.entry(becomes.id.clone()).or_default().push(grid);
        }
    }
    if by_id.is_empty() {
        eprintln!("на карте {} нет гридов станций", map.id);
    }
    let mut stations = Vec::new();
    for (id, grids) in by_id {
        let Some(config) = map.stations.get(&id) else {
            eprintln!("у станции {id} карты {} нет настройки", map.id);
            continue;
        };
        if let Some(station) =
            initialize_new_station(world, config, &grids, name, map.job_weights.clone())
        {
            stations.push(station);
        }
    }
    stations
}

/// Все станции.
pub fn stations(world: &mut World) -> Vec<Entity> {
    let mut list: Vec<Entity> = world
        .query_filtered::<Entity, bevy_ecs::query::With<StationData>>()
        .iter(world)
        .collect();
    list.sort();
    list
}

/// Станция, которой принадлежит сущность (`GetOwningStation`): по гриду.
pub fn owning_station(world: &World, entity: Entity) -> Option<Entity> {
    if world.get::<StationData>(entity).is_some() {
        return Some(entity);
    }
    let grid = world
        .get::<astrocore_map::Transform>(entity)
        .and_then(astrocore_map::Transform::grid);
    let grid = grid.or_else(|| world.get::<astrocore_map::MapGrid>(entity).map(|_| entity))?;
    world.get::<StationMember>(grid).map(|m| m.station)
}

// Слоты.

/// Занять слот должности игроком (`TryAssignJob`).
pub fn try_assign_job(world: &mut World, station: Entity, job: &str, user: NetUserId) -> bool {
    if world.get::<StationJobs>(station).is_none() {
        return false;
    }
    if !try_adjust_job_slot(world, station, job, -1, false, false) {
        return false;
    }
    if let Some(mut jobs) = world.get_mut::<StationJobs>(station) {
        jobs.player_jobs
            .entry(user)
            .or_default()
            .push(job.to_owned());
    }
    true
}

/// Изменить число слотов (`TryAdjustJobSlot`).
pub fn try_adjust_job_slot(
    world: &mut World,
    station: Entity,
    job: &str,
    amount: i32,
    create_slot: bool,
    clamp: bool,
) -> bool {
    if amount == 0 {
        return true;
    }
    let changed = {
        let Some(mut jobs) = world.get_mut::<StationJobs>(station) else {
            return false;
        };
        match jobs.job_list.get(job).copied() {
            None if amount < 0 => return false,
            None => {
                if !create_slot {
                    return false;
                }
                jobs.total_jobs += amount;
                jobs.job_list.insert(job.to_owned(), Some(amount));
                true
            }
            Some(None) => return true,
            Some(Some(available)) => {
                if available + amount < 0 && !clamp {
                    return false;
                }
                jobs.job_list
                    .insert(job.to_owned(), Some((available + amount).max(0)));
                jobs.total_jobs = jobs.job_list.values().map(|v| v.unwrap_or(0)).sum();
                true
            }
        }
    };
    if changed {
        jobs_changed(world);
    }
    true
}

/// Должности со свободными слотами (`GetAvailableJobs`).
pub fn available_jobs(world: &World, station: Entity) -> Vec<String> {
    world
        .get::<StationJobs>(station)
        .map(|jobs| {
            jobs.job_list
                .iter()
                .filter(|(_, slots)| **slots != Some(0))
                .map(|(job, _)| job.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Должности для тех, кому не хватило места (`GetOverflowJobs`): с минимумом −1.
pub fn overflow_jobs(world: &World, station: Entity) -> BTreeSet<String> {
    world
        .get::<StationJobs>(station)
        .map(|jobs| {
            jobs.setup_available_jobs
                .iter()
                .filter(|(_, [start, _])| *start < 0)
                .map(|(job, _)| job.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Минимум должностей в начале раунда (`GetRoundStartJobs`); `None` — без предела.
pub fn round_start_jobs(world: &World, station: Entity) -> BTreeMap<String, Option<i32>> {
    world
        .get::<StationJobs>(station)
        .map(|jobs| {
            jobs.setup_available_jobs
                .iter()
                .map(|(job, [start, _])| (job.clone(), (*start >= 0).then_some(*start)))
                .collect()
        })
        .unwrap_or_default()
}

fn pick<T: Clone>(random: &mut GameRandom, items: &[T]) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    Some(items[random.range(0, items.len() as i32) as usize].clone())
}

fn shuffle<T>(random: &mut GameRandom, items: &mut [T]) {
    for i in (1..items.len()).rev() {
        let j = random.range(0, i as i32 + 1) as usize;
        items.swap(i, j);
    }
}

/// Лучшая свободная должность по приоритетам игрока (`PickBestAvailableJobWithPriority`).
pub fn pick_best_available_job(
    world: &mut World,
    station: Entity,
    priorities: &BTreeMap<String, JobPriority>,
    pick_overflows: bool,
    disallowed: &BTreeSet<String>,
) -> Option<String> {
    let mut available: BTreeSet<String> = available_jobs(world, station).into_iter().collect();
    if pick_overflows {
        available.extend(overflow_jobs(world, station));
    }
    for priority in [JobPriority::High, JobPriority::Medium, JobPriority::Low] {
        let filtered: Vec<String> = priorities
            .iter()
            .filter(|(job, p)| {
                **p == priority && !disallowed.contains(*job) && available.contains(*job)
            })
            .map(|(job, _)| job.clone())
            .collect();
        if let Some(job) = pick(&mut crate::random::random(world), &filtered) {
            return Some(job);
        }
    }
    None
}

// Распределение в начале раунда.

/// Итог распределения: должность (`None` — остаётся в лобби) и станция.
pub type Assignment = (Option<String>, Option<Entity>);

/// Вес должности для порядка обязательных (`GetJobWeight`): веса карты, затем `Default`.
fn job_weight(prototypes: &Prototypes, map_weights: Option<&str>, job: &str) -> i32 {
    let from = |id: &str| {
        prototypes
            .resolved_node("jobWeight", id)
            .and_then(|n| n.get("weights"))
            .and_then(|w| w.get(job))
            .and_then(DataNode::as_str)
            .and_then(|t| t.trim().parse().ok())
    };
    map_weights
        .and_then(from)
        .or_else(|| from("Default"))
        .unwrap_or(0)
}

/// Основной отдел должности (`TryGetPrimaryDepartment`) — его должности.
fn primary_department_jobs(prototypes: &Prototypes, job: &str) -> Option<Vec<String>> {
    prototypes.ids("department").find_map(|id| {
        let node = prototypes.resolved_node("department", id)?;
        let primary = text(node, "primary").is_none_or(|t| t == "true");
        let ignored = text(node, "ignoreForDepartmentFallback").is_some_and(|t| t == "true");
        let roles: Vec<String> = node
            .get("roles")?
            .as_seq()?
            .iter()
            .filter_map(|r| r.as_str().map(str::to_owned))
            .collect();
        (primary && !ignored && roles.iter().any(|r| r == job)).then_some(roles)
    })
}

type JobCandidates = BTreeMap<String, BTreeMap<JobPriority, BTreeSet<NetUserId>>>;
type PlayerCandidates = BTreeMap<NetUserId, BTreeMap<JobPriority, Vec<String>>>;

/// Кандидаты по должностям и приоритетам (`GetJobCandidates`).
fn job_candidates(
    prototypes: &Prototypes,
    profiles: &BTreeMap<NetUserId, HumanoidCharacterProfile>,
) -> JobCandidates {
    let mut output = JobCandidates::new();
    for (player, profile) in profiles {
        for (job, priority) in &profile.job_priorities {
            if *priority == JobPriority::Never || prototypes.resolved_node("job", job).is_none() {
                continue;
            }
            output
                .entry(job.clone())
                .or_default()
                .entry(*priority)
                .or_default()
                .insert(*player);
        }
    }
    output
}

/// Обратная таблица: игрок → приоритет → должности (`GetPlayerCandidates`).
fn player_candidates(jobs: &JobCandidates) -> PlayerCandidates {
    let mut output = PlayerCandidates::new();
    for (job, priorities) in jobs {
        for (priority, players) in priorities {
            for player in players {
                output
                    .entry(*player)
                    .or_default()
                    .entry(*priority)
                    .or_default()
                    .push(job.clone());
            }
        }
    }
    output
}

const PRIORITIES: [JobPriority; 3] = [JobPriority::High, JobPriority::Medium, JobPriority::Low];

struct Assigner<'a> {
    random: &'a mut GameRandom,
    station_jobs: BTreeMap<Entity, BTreeMap<String, Option<i32>>>,
    job_candidates: JobCandidates,
    player_candidates: PlayerCandidates,
    profiles: BTreeMap<NetUserId, HumanoidCharacterProfile>,
    assigned: BTreeMap<NetUserId, Assignment>,
}

impl Assigner<'_> {
    /// Кандидат на должность с самым высоким приоритетом (`TryPickCandidate`).
    fn pick_candidate(&mut self, job: &str) -> Option<NetUserId> {
        let candidates = self.job_candidates.get(job)?;
        for priority in PRIORITIES {
            let players: Vec<NetUserId> = candidates
                .get(&priority)
                .map(|p| p.iter().copied().collect())
                .unwrap_or_default();
            if let Some(player) = pick(self.random, &players) {
                return Some(player);
            }
        }
        None
    }

    /// Должность игрока со свободным слотом станции (`TryPickJob`).
    fn pick_job(&mut self, player: NetUserId, station: Entity) -> Option<String> {
        let candidates = self.player_candidates.get(&player)?;
        let slots = &self.station_jobs[&station];
        for priority in PRIORITIES {
            let Some(jobs) = candidates.get(&priority) else {
                continue;
            };
            let available: Vec<String> = jobs
                .iter()
                .filter(|job| matches!(slots.get(*job), Some(None) | Some(Some(1..))))
                .cloned()
                .collect();
            if let Some(job) = pick(self.random, &available) {
                return Some(job);
            }
        }
        None
    }

    /// Кандидат без учёта желаний (`TryPickCandidateIgnoringPreferences`).
    fn pick_ignoring_preferences(&mut self, players: &[NetUserId]) -> Option<NetUserId> {
        pick(self.random, players)
    }

    /// Запасной кандидат на обязательную должность (`TryPickMinimumJobFallbackCandidate`).
    fn pick_fallback(
        &mut self,
        prototypes: &Prototypes,
        job: &str,
        fallback: i32,
    ) -> Option<NetUserId> {
        if fallback <= 0 || prototypes.resolved_node("job", job).is_none() {
            return None;
        }
        let same_department: Vec<NetUserId> = primary_department_jobs(prototypes, job)
            .map(|roles| {
                self.profiles
                    .iter()
                    .filter(|(_, p)| {
                        p.job_priorities
                            .iter()
                            .any(|(j, pr)| *pr != JobPriority::Never && roles.contains(j))
                    })
                    .map(|(u, _)| *u)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(player) = self.pick_ignoring_preferences(&same_department) {
            return Some(player);
        }
        if fallback >= 2 {
            let everyone: Vec<NetUserId> = self.profiles.keys().copied().collect();
            return self.pick_ignoring_preferences(&everyone);
        }
        None
    }

    /// Назначить (`AssignPlayer`).
    fn assign(&mut self, player: NetUserId, job: &str, station: Entity) {
        if let Some(Some(slots)) = self
            .station_jobs
            .get_mut(&station)
            .and_then(|s| s.get_mut(job))
        {
            *slots -= 1;
        }
        for priorities in self.job_candidates.values_mut() {
            for players in priorities.values_mut() {
                players.remove(&player);
            }
        }
        self.player_candidates.remove(&player);
        self.profiles.remove(&player);
        self.assigned
            .insert(player, (Some(job.to_owned()), Some(station)));
    }
}

/// Распределить игроков по должностям станций (`AssignJobs`): сначала обязательные
/// должности по весу, затем каждый игрок в случайном порядке — по своим приоритетам.
pub fn assign_jobs(
    world: &mut World,
    profiles: &BTreeMap<NetUserId, HumanoidCharacterProfile>,
    stations: &[Entity],
    use_round_start_jobs: bool,
) -> BTreeMap<NetUserId, Assignment> {
    if profiles.is_empty() || stations.is_empty() {
        return BTreeMap::new();
    }
    let fallback = world.cvar(&GAME_MINIMUM_JOB_FALLBACK);
    let station_jobs: BTreeMap<Entity, BTreeMap<String, Option<i32>>> = stations
        .iter()
        .map(|s| {
            (
                *s,
                world
                    .get::<StationJobs>(*s)
                    .map(|j| j.job_list.clone())
                    .unwrap_or_default(),
            )
        })
        .collect();
    let minimums: BTreeMap<Entity, BTreeMap<String, Option<i32>>> = stations
        .iter()
        .map(|s| {
            let jobs = if use_round_start_jobs {
                round_start_jobs(world, *s)
            } else {
                BTreeMap::new()
            };
            (*s, jobs)
        })
        .collect();
    let weights: BTreeMap<Entity, Option<String>> = stations
        .iter()
        .map(|s| {
            (
                *s,
                world
                    .get::<StationData>(*s)
                    .and_then(|d| d.job_weights.clone()),
            )
        })
        .collect();
    world.resource_scope(|world, prototypes: bevy_ecs::world::Mut<Prototypes>| {
        let mut random = crate::random::random(world);
        let job_candidates = job_candidates(&prototypes, profiles);
        let mut assigner = Assigner {
            random: &mut random,
            player_candidates: player_candidates(&job_candidates),
            job_candidates,
            station_jobs,
            profiles: profiles.clone(),
            assigned: BTreeMap::new(),
        };
        for &station in stations {
            let mut required: Vec<(String, i32)> = minimums[&station]
                .iter()
                .filter_map(|(job, min)| min.filter(|m| *m > 0).map(|m| (job.clone(), m)))
                .collect();
            let map_weights = weights[&station].as_deref();
            required.sort_by(|(a, _), (b, _)| {
                job_weight(&prototypes, map_weights, b)
                    .cmp(&job_weight(&prototypes, map_weights, a))
                    .then_with(|| a.cmp(b))
            });
            for (job, minimum) in required {
                for _ in 0..minimum {
                    if assigner.profiles.is_empty()
                        || matches!(assigner.station_jobs[&station].get(&job), Some(Some(..=0)))
                    {
                        break;
                    }
                    let Some(player) = assigner
                        .pick_candidate(&job)
                        .or_else(|| assigner.pick_fallback(&prototypes, &job, fallback))
                    else {
                        break;
                    };
                    assigner.assign(player, &job, station);
                }
            }
        }
        for &station in stations {
            let mut players: Vec<NetUserId> = assigner.profiles.keys().copied().collect();
            shuffle(assigner.random, &mut players);
            for player in players {
                if let Some(job) = assigner.pick_job(player, station) {
                    assigner.assign(player, &job, station);
                }
            }
        }
        assigner.assigned
    })
}

/// Тем, кому не досталось должности (`AssignOverflowJobs`): желающим — запасная должность
/// случайной станции, остальным — лобби.
pub fn assign_overflow_jobs(
    world: &mut World,
    assigned: &mut BTreeMap<NetUserId, Assignment>,
    players: &[NetUserId],
    profiles: &BTreeMap<NetUserId, HumanoidCharacterProfile>,
    stations: &[Entity],
) {
    if stations.is_empty() {
        return;
    }
    let mut given: Vec<Entity> = stations.to_vec();
    for player in players {
        if assigned.contains_key(player) {
            continue;
        }
        let wants_overflow = profiles.get(player).is_some_and(|p| {
            p.preference_unavailable == PreferenceUnavailableMode::SpawnAsOverflow
        });
        if !wants_overflow {
            assigned.insert(*player, (None, None));
            continue;
        }
        shuffle(&mut crate::random::random(world), &mut given);
        for &station in &given {
            let mut overflows: Vec<String> = overflow_jobs(world, station).into_iter().collect();
            shuffle(&mut crate::random::random(world), &mut overflows);
            if let Some(job) = overflows.first() {
                assigned.insert(*player, (Some(job.clone()), Some(station)));
                break;
            }
        }
    }
}

/// Расширенный доступ при малом экипаже (`CalcExtendedAccess`).
pub fn calc_extended_access(world: &mut World, counts: &BTreeMap<Entity, i32>) {
    for (station, count) in counts {
        if let Some(mut jobs) = world.get_mut::<StationJobs>(*station) {
            jobs.extended_access = *count <= jobs.extended_access_threshold;
        }
    }
}

// Свободные должности для позднего входа.

/// Должности одной станции для окна позднего входа.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StationJobsInfo {
    /// Сущность станции на сервере (`to_bits`): ею клиент называет станцию.
    pub station: u64,
    pub name: String,
    pub jobs: BTreeMap<String, Option<i32>>,
}

/// Свободные должности станций (`TickerJobsAvailableEvent`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JobsAvailable {
    pub stations: Vec<StationJobsInfo>,
}

/// Свободные должности сейчас (`GenerateJobsAvailableEvent`); при запрете позднего входа —
/// пусто.
pub fn jobs_available(world: &mut World) -> JobsAvailable {
    if world.cvar(&GAME_DISALLOW_LATE_JOINS) {
        return JobsAvailable::default();
    }
    let list = stations(world);
    JobsAvailable {
        stations: list
            .into_iter()
            .filter_map(|station| {
                let jobs = world.get::<StationJobs>(station)?.job_list.clone();
                Some(StationJobsInfo {
                    station: station.to_bits(),
                    name: entity_name(world, station),
                    jobs,
                })
            })
            .collect(),
    }
}

/// Станция по номеру из сообщения клиента.
pub fn station_from_bits(world: &World, bits: u64) -> Option<Entity> {
    let entity = Entity::try_from_bits(bits)?;
    world.get::<StationData>(entity).map(|_| entity)
}

/// Свободные должности на клиенте.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default, PartialEq)]
pub struct ClientJobsAvailable(pub JobsAvailable);

pub fn install(world: &mut World, side: Side) {
    world.register_cvar(&GAME_MINIMUM_JOB_FALLBACK);
    world.register_cvar(&GAME_DISALLOW_LATE_JOINS);
    world.register_data_component::<BecomesStation>();
    world.register_data_component::<StationData>();
    world.register_data_component::<StationNameSetup>();
    world.register_data_component::<StationJobs>();
    world.register_data_component::<StationSpawning>();
    world.register_net_event::<JobsAvailable>();
    if !side.is_server() {
        world.init_resource::<ClientJobsAvailable>();
        world.subscribe_broadcast::<astrocore_net::NetEvent<JobsAvailable>>(|world, event| {
            world.resource_mut::<ClientJobsAvailable>().0 = event.event.clone();
        });
        return;
    }
    world.init_resource::<JobsAvailableDirty>();
    // Обязательные слоты и запасные должности (`StationJobsComponent.OnInit`).
    world.subscribe::<StationJobs, ComponentInit>(|world, station, _| {
        if let Some(mut jobs) = world.get_mut::<StationJobs>(station) {
            jobs.mid_round_total_jobs = jobs
                .setup_available_jobs
                .values()
                .map(|[_, total]| (*total).max(0))
                .sum();
            jobs.overflow_jobs = jobs
                .setup_available_jobs
                .iter()
                .filter(|(_, [start, _])| *start < 0)
                .map(|(job, _)| job.clone())
                .collect();
        }
    });
    // Имя станции по шаблону (`OnStationNameSetupInit`).
    world.subscribe::<StationNameSetup, ComponentInit>(|world, station, _| {
        let Some(setup) = world.get::<StationNameSetup>(station).cloned() else {
            return;
        };
        let name = generate_station_name(&mut crate::random::random(world), &setup);
        set_name(world, station, &name);
    });
}

#[cfg(test)]
pub(crate) mod tests;
