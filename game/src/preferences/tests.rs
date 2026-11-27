use astrocore_ecs::GameWorldExt;

use super::*;
use crate::database::Database;
use crate::profiles::Marking;
use crate::profiles::tests::PROTOTYPES;

fn full_profile() -> HumanoidCharacterProfile {
    let mut markings = OrganMarkings::new();
    markings.entry("Head".into()).or_default().insert(
        "Hair".into(),
        vec![Marking {
            marking_id: "HumanHairAfro".into(),
            marking_colors: vec![Color::parse("#aa5500").unwrap()],
        }],
    );
    HumanoidCharacterProfile {
        name: "Urist McHands".into(),
        flavor_text: "Тихий".into(),
        species: "Reptilian".into(),
        age: 33,
        sex: crate::body::Sex::Female,
        voice: "FemaleHuman".into(),
        gender: Gender::Neuter,
        appearance: HumanoidCharacterAppearance {
            eye_color: Color::parse("#00ff00").unwrap(),
            skin_color: Color::parse("#34a223").unwrap(),
            markings,
        },
        spawn_priority: SpawnPriorityPreference::Cryosleep,
        job_priorities: BTreeMap::from([
            ("Captain".to_owned(), JobPriority::High),
            ("Passenger".to_owned(), JobPriority::Low),
        ]),
        preference_unavailable: PreferenceUnavailableMode::StayInLobby,
        antag_preferences: ["Traitor".to_owned()].into(),
        trait_preferences: ["Accent".to_owned()].into(),
        loadouts: BTreeMap::from([(
            "JobPassenger".to_owned(),
            RoleLoadout {
                role: "JobPassenger".into(),
                entity_name: Some("Бот".into()),
                selected: BTreeMap::from([(
                    "Glasses".to_owned(),
                    vec!["Sunglasses".to_owned(), "Monocle".to_owned()],
                )]),
            },
        )]),
    }
}

#[test]
fn preferences_round_trip_through_database() {
    let database = Database::in_memory().unwrap();
    let db = database.db().clone();
    database.block_on(async {
        let user = NetUserId::random();
        assert_eq!(db.get_player_preferences(user).await.unwrap(), None);
        let first = HumanoidCharacterProfile::default();
        db.init_prefs(user, &first).await.unwrap();
        let profile = full_profile();
        db.save_character_slot(user, Some(&profile), 3)
            .await
            .unwrap();
        db.save_selected_character_index(user, 3).await.unwrap();
        let prefs = db.get_player_preferences(user).await.unwrap().unwrap();
        assert_eq!(prefs.characters.len(), 2);
        assert_eq!(prefs.characters[&3], profile);
        assert_eq!(prefs.characters[&0], first);
        assert_eq!(prefs.selected_character(), Some(&profile));
        // Перезапись слота и удаление с переходом на другой.
        let renamed = HumanoidCharacterProfile {
            name: "Bob".into(),
            ..profile.clone()
        };
        db.save_character_slot(user, Some(&renamed), 3)
            .await
            .unwrap();
        db.delete_slot_and_set_selected_index(user, 0, 3)
            .await
            .unwrap();
        let prefs = db.get_player_preferences(user).await.unwrap().unwrap();
        assert_eq!(prefs.characters.keys().collect::<Vec<_>>(), [&3]);
        assert_eq!(prefs.characters[&3].name, "Bob");
        assert_eq!(prefs.selected_character_index, 3);
    });
}

fn connect(world: &mut World, name: &str) -> (Entity, NetUserId) {
    let user = NetUserId::random();
    let session = world
        .spawn(Session {
            conn: astrocore_net::ConnId(user.0.as_u128() as u64),
            user_id: user,
            name: name.into(),
            attached: None,
            last_input: 0,
            sync: Default::default(),
        })
        .id();
    world.raise_broadcast(&mut PlayerConnected { session });
    (session, user)
}

fn wait_loaded(world: &mut World, user: NetUserId) {
    for _ in 0..400 {
        if world.resource::<PreferencesCache>().get(user).is_some() {
            return;
        }
        world.run_tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("настройки не загрузились");
}

fn wait_db(world: &mut World) {
    for _ in 0..20 {
        world.run_tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn manager_loads_saves_selects_and_deletes() {
    let mut world = crate::testing::world_with(PROTOTYPES);
    world.insert_resource(Database::in_memory().unwrap());
    let (session, user) = connect(&mut world, "Urist");
    wait_loaded(&mut world, user);
    // Новый игрок получает одного случайного персонажа с должностью из настроек.
    let prefs = world
        .resource::<PreferencesCache>()
        .get(user)
        .unwrap()
        .clone();
    assert_eq!(prefs.characters.len(), 1);
    assert_eq!(
        prefs.characters[&0].job_priorities,
        BTreeMap::from([("Passenger".to_owned(), JobPriority::High)])
    );

    // Сохранение проверяет профиль и выбирает слот.
    let mut edited = full_profile();
    edited.age = 999;
    set_profile(&mut world, user, 2, &edited);
    let cached = world
        .resource::<PreferencesCache>()
        .get(user)
        .unwrap()
        .clone();
    assert_eq!(cached.selected_character_index, 2);
    assert_eq!(cached.characters[&2].age, 80);
    // Слоты за пределом не сохраняются.
    set_profile(&mut world, user, 30, &edited);
    assert!(
        !world
            .resource::<PreferencesCache>()
            .get(user)
            .unwrap()
            .characters
            .contains_key(&30)
    );
    select_character(&mut world, user, 0);
    delete_character(&mut world, user, 0);
    let cached = world
        .resource::<PreferencesCache>()
        .get(user)
        .unwrap()
        .clone();
    assert_eq!(cached.characters.keys().collect::<Vec<_>>(), [&2]);
    assert_eq!(cached.selected_character_index, 2);
    // Последнего персонажа не удалить.
    delete_character(&mut world, user, 2);
    assert_eq!(
        world
            .resource::<PreferencesCache>()
            .get(user)
            .unwrap()
            .characters
            .len(),
        1
    );
    wait_db(&mut world);

    // Вышел — забыт; вошёл снова — те же настройки из базы.
    world.raise_broadcast(&mut PlayerDisconnected { session });
    world.despawn(session);
    assert!(world.resource::<PreferencesCache>().get(user).is_none());
    let session = world
        .spawn(Session {
            conn: astrocore_net::ConnId(1),
            user_id: user,
            name: "Urist".into(),
            attached: None,
            last_input: 0,
            sync: Default::default(),
        })
        .id();
    world.raise_broadcast(&mut PlayerConnected { session });
    wait_loaded(&mut world, user);
    assert_eq!(
        world.resource::<PreferencesCache>().get(user).unwrap(),
        &cached
    );
}

#[test]
fn without_database_guests_get_random_character_in_memory() {
    let mut world = crate::testing::world_with(PROTOTYPES);
    #[derive(bevy_ecs::resource::Resource, Default)]
    struct Loaded(Vec<NetUserId>);
    world.init_resource::<Loaded>();
    world.subscribe_broadcast::<PreferencesLoaded>(|world, event| {
        let user = event.user_id;
        world.resource_mut::<Loaded>().0.push(user);
    });
    let (_, user) = connect(&mut world, "Guest");
    // Без базы — сразу.
    let prefs = world
        .resource::<PreferencesCache>()
        .get(user)
        .cloned()
        .unwrap();
    assert_eq!(prefs.characters.len(), 1);
    assert_eq!(world.resource::<Loaded>().0, [user]);
    set_profile(&mut world, user, 1, &full_profile());
    assert_eq!(
        world
            .resource::<PreferencesCache>()
            .selected_profile(user)
            .map(|p| p.name.clone()),
        Some("Urist McHands".into())
    );
}
