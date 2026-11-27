use astrocore_ecs::{GameTick, GameWorldExt};
use astrocore_map::{MapSaveOptions, MapSaveWorldExt, Transform};

use astrocore_net::NetUserId;

use super::*;
use crate::profiles::JobPriority;
use crate::stations::BecomesStation;
use crate::testing::{spawn_at, world_with};

const RULES: &str = r#"
- type: entity
  id: BaseGameRule
  abstract: true
  components:
  - type: GameRule
- type: entity
  id: Secret
  parent: BaseGameRule
- type: entity
  id: SlowEvents
  parent: BaseGameRule
  components:
  - type: GameRule
    delay:
      min: 10
      max: 10
- type: entity
  id: Hidden
  parent: BaseGameRule
- type: gamePreset
  id: Secret
  alias: [ secret, sekrit ]
  name: secret-title
  description: secret-description
  rules: [ Secret, SlowEvents, Hidden ]
- type: gamePreset
  id: Sandbox
  alias: [ sandbox ]
  name: sandbox-title
  rules: []
- type: entity
  id: MobObserver
- type: entity
  id: SpawnPointObserver
  components:
  - type: SpawnPoint
    spawn_type: Observer
"#;

/// Временный каталог ресурсов; удаляется вместе с тестом.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "astrocore-ticker-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn prototypes() -> String {
    format!(
        "{}{}{RULES}",
        crate::station_spawning::tests::PROTOTYPES,
        crate::stations::tests::PROTOTYPES
    )
}

/// Сервер с картой «Test» в ресурсах: грид станции, точки появления.
fn server(lobby: bool) -> (World, TempDir) {
    let mut world = world_with(&prototypes());
    world.insert_resource(crate::random::GameRandom::seeded(4));
    let (map_entity, _) = world.create_map(false);
    let grid = world.create_grid(map_entity);
    world.set_tiles(
        grid,
        (0..16).flat_map(|x| {
            (0..16).map(move |y| {
                (
                    astrocore_map::TileCoord::new(x, y, 0),
                    astrocore_map::Tile::new(1),
                )
            })
        }),
    );
    world
        .entity_mut(grid)
        .insert(BecomesStation { id: "Test".into() });
    spawn_at(&mut world, "SpawnPointCaptain", grid, 2, 2);
    spawn_at(&mut world, "SpawnPointLatejoin", grid, 8, 8);
    spawn_at(&mut world, "SpawnPointObserver", grid, 12, 12);
    let map = world.get::<Transform>(grid).unwrap().map().unwrap();
    let saved = world.save_map(map, MapSaveOptions::default()).unwrap();
    let dir = TempDir::new();
    std::fs::create_dir_all(dir.path().join("Maps")).unwrap();
    std::fs::write(
        dir.path().join("Maps/test.yml"),
        astrocore_data::to_yaml_string(&saved.document),
    )
    .unwrap();
    let mut resources = Resources::default();
    resources.mount(dir.path());
    world.insert_resource(resources);
    crate::content::register_cvars(&mut world);
    world.set_cvar_text("game.map", "Test").unwrap();
    world.set_cvar(&GAME_LOBBY_ENABLED, lobby).unwrap();
    world.set_cvar_text("game.defaultpreset", "sekrit").unwrap();
    post_initialize(&mut world);
    (world, dir)
}

/// Игрок входит: сессия, настройки без базы — сразу.
fn connect(world: &mut World, n: u8, jobs: &[(&str, JobPriority)]) -> (Entity, NetUserId) {
    let user = NetUserId(uuid::Uuid::from_bytes([n; 16]));
    let session = world
        .spawn(Session {
            conn: astrocore_net::ConnId(u64::from(n)),
            user_id: user,
            name: format!("Player{n}"),
            attached: None,
            last_input: 0,
            sync: Default::default(),
        })
        .id();
    // Профиль с нужными должностями до загрузки настроек.
    world.raise_broadcast(&mut PlayerConnected { session });
    let mut profile = world
        .resource::<PreferencesCache>()
        .selected_profile(user)
        .cloned()
        .unwrap();
    profile.species = "Human".into();
    profile.job_priorities = jobs.iter().map(|(j, p)| ((*j).to_owned(), *p)).collect();
    crate::preferences::set_profile(world, user, 0, &profile);
    (session, user)
}

fn skip_to(world: &mut World, time: Duration) {
    let rate = world.resource::<GameTime>().tick_rate;
    let tick = (time.as_secs_f64() * f64::from(rate)).ceil() as u32 + 1;
    world.resource_mut::<GameTime>().tick = GameTick(tick);
    world.run_tick();
}

fn prototype(world: &World, entity: Entity) -> Option<String> {
    world
        .get::<MetaData>(entity)?
        .prototype
        .as_deref()
        .map(str::to_owned)
}

fn attached(world: &World, session: Entity) -> Option<Entity> {
    world.get::<Session>(session)?.attached
}

#[test]
fn lobby_counts_down_and_starts_round_with_jobs() {
    let (mut world, _dir) = server(true);
    {
        let ticker = world.resource::<GameTicker>();
        // Никого нет — отсчёт ждёт; карта не загружена.
        assert_eq!(ticker.run_level, GameRunLevel::PreRoundLobby);
        assert!(ticker.waiting_for_players);
        assert_eq!(ticker.preset.as_deref(), Some("Secret"));
        assert!(ticker.default_map.is_none());
    }
    let (captain, captain_user) = connect(&mut world, 1, &[("Captain", JobPriority::High)]);
    let (late, late_user) = connect(&mut world, 2, &[("Engineer", JobPriority::High)]);
    let start = world.resource::<GameTicker>().round_start_time.unwrap();
    assert!(!world.resource::<GameTicker>().waiting_for_players);
    // Готов только первый: в лобби нет тел.
    toggle_ready(&mut world, captain, true);
    assert_eq!(ready_player_count(&mut world), 1);
    assert_eq!(attached(&world, captain), None);
    assert!(info_text(&mut world).starts_with("game-ticker-get-info-preround-text"));

    // За 15 секунд до старта карта загружается заранее, без инициализации.
    skip_to(&mut world, start - Duration::from_secs(10));
    let map = world.resource::<GameTicker>().default_map.unwrap();
    let map_entity = world.map_entity(map).unwrap();
    assert!(!world.get::<Map>(map_entity).unwrap().initialized);
    assert_eq!(run_level(&world), GameRunLevel::PreRoundLobby);

    skip_to(&mut world, start);
    assert_eq!(run_level(&world), GameRunLevel::InRound);
    assert!(world.get::<Map>(map_entity).unwrap().initialized);
    // Готовый — капитан на своей точке; правила пресета запущены, кроме отложенного.
    let body = attached(&world, captain).unwrap();
    assert_eq!(prototype(&world, body).as_deref(), Some("MobTestHuman"));
    assert!(user_has_joined_game(&world, captain_user));
    let station = spawnable_stations(&mut world)[0];
    assert_eq!(
        world.get::<StationJobs>(station).unwrap().player_jobs[&captain_user],
        ["Captain"]
    );
    let rules = added_game_rules(&mut world);
    assert_eq!(rules.len(), 3);
    assert_eq!(active_game_rules(&mut world).len(), 2);
    assert!(
        rules
            .iter()
            .any(|r| world.get::<DelayedStartRule>(*r).is_some())
    );
    // Отложенное правило запускается в свой срок.
    let now = now(&world);
    skip_to(&mut world, now + Duration::from_secs(11));
    assert_eq!(active_game_rules(&mut world).len(), 3);

    // Неготовый остался в лобби; поздний вход — на точку позднего входа.
    assert!(!user_has_joined_game(&world, late_user));
    let mut request = NetEvent {
        session: Some(late),
        event: LobbyRequest::JoinGame {
            station: None,
            job: None,
        },
    };
    receive_request(&mut world, &mut request);
    let late_body = attached(&world, late).unwrap();
    assert_eq!(
        world.get::<StationJobs>(station).unwrap().player_jobs[&late_user],
        ["Engineer"]
    );
    let late_at = world.coordinates(late_body).unwrap().position;
    assert!((late_at.x - 8.5).abs() < 0.01 && (late_at.y - 8.5).abs() < 0.01);
}

#[test]
fn round_ends_and_restarts_with_new_lobby() {
    let (mut world, _dir) = server(true);
    let (session, user) = connect(&mut world, 1, &[("Captain", JobPriority::High)]);
    toggle_ready(&mut world, session, true);
    start_round(&mut world, false);
    let round = world.resource::<GameTicker>().round_id;
    let body = attached(&world, session).unwrap();
    end_round(&mut world, "Конец");
    assert_eq!(run_level(&world), GameRunLevel::PostRound);
    let restart = world.resource::<GameTicker>().restart_at.unwrap();
    skip_to(&mut world, restart);
    // Новый раунд: лобби, всё удалено, сессия осталась и не готова, номер новый.
    let ticker = world.resource::<GameTicker>();
    assert_eq!(ticker.run_level, GameRunLevel::PreRoundLobby);
    assert_eq!(ticker.round_id, round + 1);
    assert!(ticker.default_map.is_none());
    assert!(ticker.bodies.is_empty());
    assert_eq!(
        ticker.statuses.get(&user),
        Some(&PlayerGameStatus::NotReadyToPlay)
    );
    assert!(ticker.round_start_time.is_some());
    assert!(world.get_entity(body).is_err());
    assert_eq!(world.query::<&MetaData>().iter(&world).count(), 0);
    assert!(world.get::<Session>(session).is_some());
    assert_eq!(attached(&world, session), None);
}

#[test]
fn without_lobby_players_join_at_once_and_keep_bodies() {
    let (mut world, _dir) = server(false);
    // Без лобби раунд начинается сразу, пресет — песочница.
    assert_eq!(run_level(&world), GameRunLevel::InRound);
    assert_eq!(
        world.resource::<GameTicker>().current_preset.as_deref(),
        Some("Sandbox")
    );
    let (session, user) = connect(&mut world, 1, &[("Passenger", JobPriority::High)]);
    let body = attached(&world, session).unwrap();
    assert!(user_has_joined_game(&world, user));
    // Вышел и вернулся — то же тело.
    world.raise_broadcast(&mut PlayerDisconnected { session });
    world.despawn(session);
    assert!(world.get_entity(body).is_ok());
    let again = world
        .spawn(Session {
            conn: astrocore_net::ConnId(9),
            user_id: user,
            name: "Player1".into(),
            attached: None,
            last_input: 0,
            sync: Default::default(),
        })
        .id();
    world.raise_broadcast(&mut PlayerConnected { session: again });
    assert_eq!(attached(&world, again), Some(body));
}

#[test]
fn late_join_ban_makes_observers() {
    let (mut world, _dir) = server(false);
    world
        .set_cvar(&crate::stations::GAME_DISALLOW_LATE_JOINS, true)
        .unwrap();
    assert!(world.resource::<GameTicker>().disallow_late_join);
    let (session, user) = connect(&mut world, 3, &[("Passenger", JobPriority::High)]);
    let ghost = attached(&world, session).unwrap();
    assert_eq!(
        prototype(&world, ghost).as_deref(),
        Some(OBSERVER_PROTOTYPE)
    );
    assert!(world.resource::<GameTicker>().observers.contains(&user));
    let at = world.coordinates(ghost).unwrap().position;
    assert!((at.x - 12.5).abs() < 0.01);
}

#[test]
fn presets_are_found_by_alias_and_rules_can_be_ignored() {
    let (mut world, _dir) = server(true);
    let prototypes = world.resource::<Prototypes>();
    assert_eq!(find_game_preset(prototypes, "SEKRIT").unwrap().id, "Secret");
    assert!(find_game_preset(prototypes, "nope").is_none());
    assert!(!set_game_preset(&mut world, "nope", false, None));
    world
        .set_cvar_text("game.ignoredpresets", "Hidden")
        .unwrap();
    let (session, _) = connect(&mut world, 1, &[]);
    toggle_ready(&mut world, session, true);
    start_round(&mut world, false);
    let names: Vec<String> = added_game_rules(&mut world)
        .into_iter()
        .filter_map(|r| prototype(&world, r))
        .collect();
    assert!(!names.contains(&"Hidden".to_owned()));
    assert_eq!(names.len(), 2);
    // Без желаемых должностей — запасная должность станции (пассажир), как в SS14.
    let station = spawnable_stations(&mut world)[0];
    let jobs = &world.get::<StationJobs>(station).unwrap().player_jobs;
    assert_eq!(jobs.values().next().unwrap(), &["Passenger".to_owned()]);
    // Итоги — с номером раунда и режимом.
    end_round(&mut world, "Итог");
    assert_eq!(run_level(&world), GameRunLevel::PostRound);
}

#[test]
fn delaying_and_pausing_the_countdown() {
    let (mut world, _dir) = server(true);
    connect(&mut world, 1, &[]);
    let start = world.resource::<GameTicker>().round_start_time.unwrap();
    assert!(delay_start(&mut world, Duration::from_secs(30)));
    assert_eq!(
        world.resource::<GameTicker>().round_start_time,
        Some(start + Duration::from_secs(30))
    );
    assert!(pause_start(&mut world, true));
    assert!(!pause_start(&mut world, true));
    // На паузе старт не наступает.
    skip_to(&mut world, start + Duration::from_secs(60));
    assert_eq!(run_level(&world), GameRunLevel::PreRoundLobby);
    assert!(pause_start(&mut world, false));
    let resumed = world.resource::<GameTicker>().round_start_time.unwrap();
    assert!(resumed > start + Duration::from_secs(60));
}

#[test]
fn client_lobby_follows_ticker_messages() {
    let mut world = World::new();
    world.insert_resource(GameTime::default());
    apply_ticker_message(&mut world, &TickerMessage::JoinLobby);
    apply_ticker_message(
        &mut world,
        &TickerMessage::LobbyStatus {
            is_round_started: false,
            you_are_ready: true,
            seconds_until_start: Some(90.0),
            paused: false,
            round_duration: 0.0,
        },
    );
    let lobby = world.resource::<ClientLobby>().clone();
    assert!(lobby.in_lobby && lobby.ready && !lobby.round_started);
    // Отсчёт идёт по часам клиента.
    world.resource_mut::<GameTime>().tick = GameTick(30 * 60);
    let now = world.resource::<GameTime>().cur_time();
    let left = lobby.seconds_left(now).unwrap();
    assert!((left - 30.0).abs() < 0.1, "{left}");
    apply_ticker_message(
        &mut world,
        &TickerMessage::RoundEnd(Box::new(RoundEndMessage {
            gamemode_title: "Secret".into(),
            round_end_text: String::new(),
            round_duration: 600.0,
            round_id: 7,
            players: Vec::new(),
        })),
    );
    apply_ticker_message(&mut world, &TickerMessage::JoinGame);
    let lobby = world.resource::<ClientLobby>();
    assert!(!lobby.in_lobby);
    assert!(lobby.show_round_end);
    assert_eq!(lobby.round_end.as_ref().unwrap().round_id, 7);
}
