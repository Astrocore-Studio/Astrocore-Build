use super::*;
use crate::testing::{station, world_with};

pub(crate) const PROTOTYPES: &str = r#"
- type: job
  id: Captain
  name: job-name-captain
  startingGear: CaptainGear
  icon: JobIconCaptain
  accessGroups: [ AllAccess ]
- type: job
  id: Engineer
  name: job-name-engineer
- type: job
  id: Chief
  name: job-name-chief
- type: job
  id: Passenger
  name: job-name-passenger
  access: [ Maintenance ]
  extendedAccess: [ Engineering ]
- type: jobWeight
  id: Default
  weights:
    Captain: 20
    Chief: 10
- type: department
  id: Engineering
  roles: [ Chief, Engineer ]
- type: department
  id: Civilian
  ignoreForDepartmentFallback: true
  roles: [ Passenger ]
- type: entity
  id: BaseStation
  abstract: true
  components:
  - type: StationData
- type: entity
  id: TestStation
  parent: BaseStation
  components:
  - type: StationJobs
    availableJobs: {}
  - type: StationSpawning
- type: gameMap
  id: Test
  mapName: 'Test'
  mapPath: /Maps/test.yml
  maxPlayers: 30
  fallback: true
  stations:
    Test:
      stationProto: TestStation
      components:
        - type: StationNameSetup
          mapNameTemplate: '{0} Test {1}'
          nameGenerator:
            !type:NanotrasenNameGenerator
            prefixCreator: '14'
        - type: StationJobs
          availableJobs:
            Captain: [ 1, 1 ]
            Chief: [ 1, 1 ]
            Engineer: [ 1, 2 ]
            Passenger: [ -1, -1 ]
"#;

fn user(n: u8) -> NetUserId {
    NetUserId(uuid::Uuid::from_bytes([n; 16]))
}

fn profile(jobs: &[(&str, JobPriority)]) -> HumanoidCharacterProfile {
    HumanoidCharacterProfile {
        job_priorities: jobs.iter().map(|(j, p)| ((*j).to_owned(), *p)).collect(),
        ..HumanoidCharacterProfile::default()
    }
}

/// Станция тестовой карты на одном гриде.
pub(crate) fn test_station(world: &mut World) -> (Entity, Entity) {
    let grid = station(world);
    world
        .entity_mut(grid)
        .insert(BecomesStation { id: "Test".into() });
    let map = game_map(world.resource::<Prototypes>(), "Test").unwrap();
    let stations = setup_stations(world, &map, &[grid], None);
    assert_eq!(stations.len(), 1);
    (stations[0], grid)
}

#[test]
fn game_map_and_station_come_from_prototypes() {
    let mut world = world_with(PROTOTYPES);
    world.insert_resource(crate::random::GameRandom::seeded(1));
    let map = game_map(world.resource::<Prototypes>(), "Test").unwrap();
    assert_eq!(map.map_path, "/Maps/test.yml");
    assert_eq!(resource_path(&map.map_path), "Maps/test.yml");
    assert_eq!(
        (map.min_players, map.max_players, map.fallback),
        (0, 30, true)
    );
    assert_eq!(map.stations["Test"].station_proto, "TestStation");

    let (station, grid) = test_station(&mut world);
    // Имя по шаблону: «NT14 Test XX-000»; грид назван как станция.
    let name = entity_name(&world, station);
    assert!(name.starts_with("NT14 Test "), "{name}");
    let suffix = name.trim_start_matches("NT14 Test ");
    assert_eq!(suffix.len(), 6);
    assert!(["LV", "NX", "EV", "QT", "PR"].contains(&&suffix[..2]));
    assert_eq!(entity_name(&world, grid), name);
    assert_eq!(world.get::<StationData>(station).unwrap().grids, [grid]);
    assert_eq!(owning_station(&world, grid), Some(station));

    // Слоты: всего по второму числу, −1 — без предела; запасная должность — с −1.
    let jobs = world.get::<StationJobs>(station).unwrap();
    assert_eq!(jobs.job_list["Engineer"], Some(2));
    assert_eq!(jobs.job_list["Passenger"], None);
    assert_eq!(jobs.total_jobs, 4);
    assert_eq!(jobs.mid_round_total_jobs, 4);
    assert_eq!(jobs.overflow_jobs, BTreeSet::from(["Passenger".to_owned()]));
    assert_eq!(stations(&mut world), [station]);
}

#[test]
fn slots_are_taken_and_adjusted() {
    let mut world = world_with(PROTOTYPES);
    let (station, _) = test_station(&mut world);
    assert!(try_assign_job(&mut world, station, "Captain", user(1)));
    assert!(!try_assign_job(&mut world, station, "Captain", user(2)));
    // Без предела — всегда можно.
    for n in 0..5 {
        assert!(try_assign_job(
            &mut world,
            station,
            "Passenger",
            user(10 + n)
        ));
    }
    assert!(!try_adjust_job_slot(
        &mut world, station, "Clown", 1, false, false
    ));
    assert!(try_adjust_job_slot(
        &mut world, station, "Clown", 1, true, false
    ));
    assert!(try_adjust_job_slot(
        &mut world, station, "Engineer", -5, false, true
    ));
    let jobs = world.get::<StationJobs>(station).unwrap();
    assert_eq!(jobs.job_list["Engineer"], Some(0));
    assert_eq!(jobs.player_jobs[&user(1)], ["Captain"]);
    assert_eq!(
        available_jobs(&world, station),
        ["Chief", "Clown", "Passenger"]
    );
    // Лучшая свободная по приоритету; занятая не выпадает, запасная — по запросу.
    let wants = BTreeMap::from([
        ("Captain".to_owned(), JobPriority::High),
        ("Chief".to_owned(), JobPriority::Low),
    ]);
    let pick = pick_best_available_job(&mut world, station, &wants, false, &BTreeSet::new());
    assert_eq!(pick.as_deref(), Some("Chief"));
    let none = pick_best_available_job(
        &mut world,
        station,
        &wants,
        false,
        &BTreeSet::from(["Chief".to_owned()]),
    );
    assert_eq!(none, None);
    // Список для позднего входа.
    let listed = jobs_available(&mut world);
    assert_eq!(listed.stations.len(), 1);
    assert_eq!(
        station_from_bits(&world, listed.stations[0].station),
        Some(station)
    );
    world.set_cvar(&GAME_DISALLOW_LATE_JOINS, true).unwrap();
    assert!(jobs_available(&mut world).stations.is_empty());
}

#[test]
fn round_start_assignment_fills_minimums_then_preferences() {
    let mut world = world_with(PROTOTYPES);
    world.insert_resource(crate::random::GameRandom::seeded(5));
    let (station, _) = test_station(&mut world);
    let profiles = BTreeMap::from([
        // Хочет капитана — получит: высокий приоритет и обязательная должность.
        (user(1), profile(&[("Captain", JobPriority::High)])),
        // Двое хотят капитана пониже; один уйдёт инженером, другой — по запасному пути.
        (
            user(2),
            profile(&[
                ("Captain", JobPriority::Medium),
                ("Engineer", JobPriority::Low),
            ]),
        ),
        (user(3), profile(&[("Captain", JobPriority::Low)])),
        // Ничего не хочет из доступного.
        (user(4), profile(&[("Clown", JobPriority::High)])),
    ]);
    let stations = [station];
    let mut assigned = assign_jobs(&mut world, &profiles, &stations, true);
    assert_eq!(assigned[&user(1)], (Some("Captain".into()), Some(station)));
    assert_eq!(assigned[&user(2)], (Some("Engineer".into()), Some(station)));
    assert!(!assigned.contains_key(&user(3)));
    assert!(!assigned.contains_key(&user(4)));
    // Не назначенные: желающие запасной должности — пассажиры, остальные — в лобби.
    let mut profiles = profiles;
    profiles.get_mut(&user(4)).unwrap().preference_unavailable =
        PreferenceUnavailableMode::StayInLobby;
    let players: Vec<NetUserId> = profiles.keys().copied().collect();
    assign_overflow_jobs(&mut world, &mut assigned, &players, &profiles, &stations);
    assert_eq!(
        assigned[&user(3)],
        (Some("Passenger".into()), Some(station))
    );
    assert_eq!(assigned[&user(4)], (None, None));
}

#[test]
fn minimum_fallback_takes_players_of_the_department() {
    let mut world = world_with(PROTOTYPES);
    world.insert_resource(crate::random::GameRandom::seeded(9));
    let (station, _) = test_station(&mut world);
    // Главного инженера никто не хочет; инженеров двое на один обязательный слот.
    let profiles = BTreeMap::from([
        (user(1), profile(&[("Engineer", JobPriority::High)])),
        (user(2), profile(&[("Engineer", JobPriority::High)])),
        (user(3), profile(&[("Passenger", JobPriority::High)])),
    ]);
    let without = assign_jobs(&mut world, &profiles, &[station], true);
    assert!(
        without
            .values()
            .all(|(job, _)| job.as_deref() != Some("Chief"))
    );
    world.set_cvar(&GAME_MINIMUM_JOB_FALLBACK, 1).unwrap();
    let with = assign_jobs(&mut world, &profiles, &[station], true);
    let chiefs: Vec<_> = with
        .iter()
        .filter(|(_, (job, _))| job.as_deref() == Some("Chief"))
        .map(|(u, _)| *u)
        .collect();
    // Главным становится инженер, а не пассажир: у гражданских отдел не в счёт.
    assert_eq!(chiefs.len(), 1);
    assert!(chiefs[0] == user(1) || chiefs[0] == user(2));
    assert_eq!(with[&user(3)].0.as_deref(), Some("Passenger"));
}

#[test]
fn extended_access_for_small_crews() {
    let mut world = world_with(PROTOTYPES);
    let (station, _) = test_station(&mut world);
    calc_extended_access(&mut world, &BTreeMap::from([(station, 3)]));
    assert!(world.get::<StationJobs>(station).unwrap().extended_access);
    calc_extended_access(&mut world, &BTreeMap::from([(station, 40)]));
    assert!(!world.get::<StationJobs>(station).unwrap().extended_access);
}
