use astrocore_console::{ConfigWorldExt, ConsoleShell, ConsoleWorldExt};
use astrocore_net::ConnId;

use super::*;
use crate::testing::world_with;

const PROTOTYPES: &str = r#"
- type: gamePreset
  id: Extended
  name: extended-title
  showInVote: true
- type: gamePreset
  id: Secret
  name: secret-title
  showInVote: false
- type: gamePreset
  id: Survival
  name: survival-title
  showInVote: true
- type: gameMap
  id: Small
  mapName: Small Station
  mapPath: /Maps/small.yml
- type: gameMap
  id: Big
  mapName: Big Station
  mapPath: /Maps/big.yml
- type: gameMapPool
  id: DefaultMapPool
  maps: [ Small, Big ]
"#;

fn world() -> World {
    let mut world = world_with(PROTOTYPES);
    world.set_cvar_text("game.map", "").unwrap();
    world
}

fn session(world: &mut World, n: u8) -> Entity {
    world
        .spawn(Session {
            conn: ConnId(u64::from(n)),
            user_id: NetUserId(uuid::Uuid::from_bytes([n; 16])),
            name: format!("player{n}"),
            attached: None,
            last_input: 0,
            sync: Default::default(),
        })
        .id()
}

fn run(world: &mut World, player: Option<Entity>, line: &str) -> (Vec<String>, Vec<String>) {
    let mut shell = match player {
        Some(p) => ConsoleShell::player(p),
        None => ConsoleShell::server(),
    };
    world.execute_command(&mut shell, line).unwrap();
    (shell.output, shell.errors)
}

/// Довести голосование до конца сейчас.
fn finish(world: &mut World, id: i32) {
    end_vote(world, id);
    update(world);
}

fn only_vote(world: &World) -> i32 {
    let active = active_votes(world);
    assert_eq!(active.len(), 1, "{active:?}");
    active[0].0
}

#[test]
fn custom_votes_count_changes_and_report_winner_or_tie() {
    let mut world = world();
    let a = session(&mut world, 1);
    let b = session(&mut world, 2);
    let (_, errors) = run(&mut world, None, "customvote \"Pizza?\" Yes No");
    assert!(errors.is_empty(), "{errors:?}");
    let id = only_vote(&world);
    assert_eq!(
        run(&mut world, None, "listvotes").0,
        [format!("[{id}] vote-options-server-initiator-text: Pizza?")]
    );
    run(&mut world, Some(a), &format!("vote {id} 0"));
    run(&mut world, Some(b), &format!("vote {id} 0"));
    // Передумал — голос переходит.
    run(&mut world, Some(b), &format!("vote {id} 1"));
    assert_eq!(
        votes_per_option(&world, id),
        BTreeMap::from([("0".into(), 1), ("1".into(), 1)])
    );
    assert_eq!(
        run(&mut world, Some(a), &format!("vote {id} 7")).1,
        ["cmd-vote-on-execute-error-invalid-option"]
    );
    assert_eq!(
        run(&mut world, None, &format!("vote {id} 0")).1,
        ["cmd-vote-on-execute-error-must-be-player"]
    );
    // Ничья.
    let outcome = {
        let mut state = votes(&mut world);
        let vote = state.votes.get_mut(&id).unwrap();
        vote.entries.iter().map(|e| e.votes).collect::<Vec<_>>()
    };
    assert_eq!(outcome, [1, 1]);
    finish(&mut world, id);
    assert!(active_votes(&world).is_empty());
}

#[test]
fn players_wait_for_other_votes_and_timeouts_admins_do_not() {
    let mut world = world();
    let a = session(&mut world, 1);
    let b = session(&mut world, 2);
    let admin = session(&mut world, 3);
    crate::admin_manager::promote_host(&mut world, admin);
    assert!(can_call_vote(&mut world, a, Some(StandardVoteType::Preset)).0);
    // Голосование за карту выключено по умолчанию.
    assert!(!can_call_vote(&mut world, a, Some(StandardVoteType::Map)).0);
    run(&mut world, Some(a), "createvote preset");
    let id = only_vote(&world);
    // Пока идёт голосование, игрок не вызовет другое; администратор — вызовет.
    assert!(!can_call_vote(&mut world, b, None).0);
    assert_eq!(
        run(&mut world, Some(b), "createvote preset").1,
        ["cmd-createvote-cannot-call-vote-now"]
    );
    assert!(can_call_vote(&mut world, admin, Some(StandardVoteType::Preset)).0);
    finish(&mut world, id);
    // После — перерыв вида и личный перерыв вызвавшего.
    let (ok, _, wait) = can_call_vote(&mut world, b, Some(StandardVoteType::Preset));
    assert!(!ok && wait > Duration::ZERO);
    assert!(!can_call_vote(&mut world, a, None).0);
    assert!(can_call_vote(&mut world, b, None).0);
    // Выключенные голосования игрокам недоступны.
    world.set_cvar(&VOTE_ENABLED, false).unwrap();
    assert!(!can_call_vote(&mut world, b, None).0);
    assert!(can_call_vote(&mut world, admin, None).0);
    assert_eq!(
        run(&mut world, None, "createvote kick").1,
        ["cmd-createvote-invalid-vote-type"]
    );
}

#[test]
fn preset_vote_offers_votable_presets_and_sets_the_winner() {
    let mut world = world();
    let a = session(&mut world, 1);
    let _b = session(&mut world, 2);
    assert_eq!(
        game_presets(&mut world),
        [
            ("Extended".to_owned(), "extended-title".to_owned()),
            ("Survival".to_owned(), "survival-title".to_owned())
        ]
    );
    create_standard_vote(&mut world, Some(a), StandardVoteType::Preset);
    let id = only_vote(&world);
    cast_vote(&mut world, id, a, Some(1));
    finish(&mut world, id);
    assert_eq!(
        world
            .resource::<game_ticker::GameTicker>()
            .preset
            .as_deref(),
        Some("Survival")
    );
}

#[test]
fn map_vote_picks_the_next_map_while_the_lobby_allows() {
    let mut world = world();
    let a = session(&mut world, 1);
    let _b = session(&mut world, 2);
    create_standard_vote(&mut world, Some(a), StandardVoteType::Map);
    let id = only_vote(&world);
    cast_vote(&mut world, id, a, Some(1));
    finish(&mut world, id);
    assert_eq!(crate::game_maps::selected_map(&world).unwrap().id, "Big");
}

#[test]
fn restart_vote_needs_the_round_and_a_large_yes_share() {
    let mut world = world();
    let a = session(&mut world, 1);
    let b = session(&mut world, 2);
    let c = session(&mut world, 3);
    // В лобби не вызывается.
    create_standard_vote(&mut world, Some(a), StandardVoteType::Restart);
    assert!(active_votes(&world).is_empty());
    world.resource_mut::<game_ticker::GameTicker>().run_level = GameRunLevel::InRound;
    create_standard_vote(&mut world, Some(a), StandardVoteType::Restart);
    let id = only_vote(&world);
    // Вызвавший — «за», остальные — «воздержались».
    assert_eq!(
        votes_per_option(&world, id),
        BTreeMap::from([("yes".into(), 1), ("no".into(), 0), ("abstain".into(), 2)])
    );
    cast_vote(&mut world, id, b, Some(0));
    cast_vote(&mut world, id, c, Some(1));
    // 2 из 3 — меньше 85 %: раунд идёт дальше.
    finish(&mut world, id);
    assert_eq!(game_ticker::run_level(&world), GameRunLevel::InRound);

    votes(&mut world).standard_timeout.clear();
    create_standard_vote(&mut world, Some(a), StandardVoteType::Restart);
    let id = only_vote(&world);
    cast_vote(&mut world, id, b, Some(0));
    finish(&mut world, id);
    assert_eq!(game_ticker::run_level(&world), GameRunLevel::PostRound);
}

#[test]
fn clients_see_counts_only_when_shown_or_as_admins_and_leavers_lose_votes() {
    let mut world = world();
    let a = session(&mut world, 1);
    let admin = session(&mut world, 2);
    crate::admin_manager::promote_host(&mut world, admin);
    let id = create_vote(
        &mut world,
        VoteOptions {
            title: "secret".into(),
            options: vec![("A".into(), "a".into()), ("B".into(), "b".into())],
            display_votes: false,
            ..VoteOptions::default()
        },
    );
    cast_vote(&mut world, id, a, Some(1));
    let state = world.resource::<Votes>();
    let vote = &state.votes[&id];
    let for_player = vote_data(&world, id, vote, a);
    assert!(!for_player.display_votes);
    assert_eq!(for_player.options, [(0, "A".into()), (0, "B".into())]);
    assert_eq!(for_player.your_vote, Some(Some(1)));
    let for_admin = vote_data(&world, id, vote, admin);
    assert_eq!(for_admin.options, [(0, "A".into()), (1, "B".into())]);
    world.raise_broadcast(&mut PlayerDisconnected { session: a });
    assert_eq!(votes_per_option(&world, id)["b"], 0);
    assert!(cancel_vote(&mut world, id));
    update(&mut world);
    assert!(active_votes(&world).is_empty());
}

#[test]
fn the_client_follows_votes_and_its_own_choice() {
    let mut world = World::new();
    let data = |active: bool, your_vote: Option<Option<u8>>| VoteData {
        vote_id: 4,
        active,
        title: "Next map".into(),
        initiator: "player1".into(),
        elapsed: 10.0,
        duration: 90.0,
        options: vec![(2, "Small".into()), (1, "Big".into())],
        your_vote,
        display_votes: true,
    };
    assert!(client_receive_vote(&mut world, &data(true, Some(Some(1)))));
    let votes = world.resource::<ClientVotes>();
    let vote = &votes.votes[&4];
    assert_eq!(vote.our_vote, Some(1));
    assert_eq!(vote.entries, [("Small".into(), 2), ("Big".into(), 1)]);
    let left = vote
        .end
        .saturating_duration_since(Instant::now())
        .as_secs_f32();
    assert!((79.0..=80.0).contains(&left), "{left}");
    // Свой голос не трогается, если сервер его не менял.
    assert!(!client_receive_vote(&mut world, &data(true, None)));
    assert_eq!(world.resource::<ClientVotes>().votes[&4].our_vote, Some(1));
    // Законченное убирается.
    client_receive_vote(&mut world, &data(false, None));
    assert!(world.resource::<ClientVotes>().votes.is_empty());

    client_receive_can_call(
        &mut world,
        &VoteCanCall {
            can_call: true,
            when_can_call: 0.0,
            unavailable: vec![
                (StandardVoteType::Map, 0.0),
                (StandardVoteType::Preset, 120.0),
            ],
        },
    );
    let votes = world.resource::<ClientVotes>();
    assert!(votes.can_call);
    assert_eq!(votes.can_call_standard(StandardVoteType::Restart), Ok(()));
    assert_eq!(votes.can_call_standard(StandardVoteType::Map), Err(None));
    assert!(matches!(
        votes.can_call_standard(StandardVoteType::Preset),
        Err(Some(wait)) if wait > Duration::from_secs(100)
    ));
}
