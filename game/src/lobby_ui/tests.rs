use std::collections::BTreeMap;

use astrocore_ecs::GameTick;

use super::*;
use crate::game_ticker::RoundEndPlayerInfo;
use crate::testing::world_with;

#[test]
fn clock_and_status_follow_the_countdown() {
    assert_eq!(clock(125.2), "02:06");
    assert_eq!(clock(-3.0), "00:00");
    let mut world = world_with(crate::stations::tests::PROTOTYPES);
    let mut lobby = ClientLobby {
        in_lobby: true,
        start_at: Some(Duration::from_secs(90)),
        ..ClientLobby::default()
    };
    // Без перевода — ключи сообщений.
    assert_eq!(
        status_line(&world, &lobby),
        "lobby-state-round-start-countdown-text"
    );
    world.resource_mut::<GameTime>().tick = GameTick(30 * 120);
    assert_eq!(status_line(&world, &lobby), "Раунд скоро начнётся");
    lobby.paused = true;
    assert_eq!(status_line(&world, &lobby), "Пауза");
    lobby.start_at = None;
    lobby.paused = false;
    assert_eq!(status_line(&world, &lobby), "Раунд ещё не начался");
    lobby.round_started = true;
    assert_eq!(
        status_line(&world, &lobby),
        "lobby-state-player-status-round-time"
    );
}

#[test]
fn late_join_lists_open_jobs_by_department() {
    let world = world_with(crate::stations::tests::PROTOTYPES);
    let station = StationJobsInfo {
        station: 1,
        name: "NT14".into(),
        jobs: BTreeMap::from([
            ("Engineer".to_owned(), Some(2)),
            ("Chief".to_owned(), Some(0)),
            ("Passenger".to_owned(), None),
        ]),
    };
    let rows = late_join_rows(&world, &station);
    // Главного инженера нет: слотов ноль; у пассажира — без предела.
    // Отделы с равным весом — по идентификатору: гражданские раньше инженерного.
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].1,
        [(
            "Passenger".to_owned(),
            "late-join-gui-job-slot-uncapped".to_owned()
        )]
    );
    assert_eq!(rows[1].1.len(), 1);
    assert_eq!(rows[1].1[0].0, "Engineer");
}

#[test]
fn round_end_summary_lists_players() {
    let world = world_with(crate::stations::tests::PROTOTYPES);
    let end = RoundEndMessage {
        gamemode_title: "Secret".into(),
        round_end_text: "\nСтанция уцелела\n".into(),
        round_duration: 3725.0,
        round_id: 12,
        players: vec![
            RoundEndPlayerInfo {
                ooc_name: "Urist".into(),
                ic_name: Some("Bob".into()),
                role: "Captain".into(),
                antag: false,
                observer: false,
                connected: true,
            },
            RoundEndPlayerInfo {
                ooc_name: "Ghost".into(),
                ic_name: None,
                role: String::new(),
                antag: false,
                observer: true,
                connected: false,
            },
        ],
    };
    let lines = round_end_lines(&world, &end);
    assert_eq!(lines.len(), 6);
    assert_eq!(lines[3], "Станция уцелела");
    assert_eq!(
        lines[5],
        "round-end-summary-window-player-info-if-observer-text"
    );
}
