use super::*;
use crate::testing::world_with;

pub(crate) const PROTOTYPES: &str = r##"
- type: species
  id: Human
  name: species-name-human
  roundStart: true
  prototype: MobHuman
  skinColoration: HumanToned
- type: species
  id: Reptilian
  name: species-name-reptilian
  roundStart: true
  prototype: MobReptilian
  skinColoration: Hues
  defaultSkinTone: "#34a223"
  naming: First
  minAge: 20
  oldAge: 50
  maxAge: 80
- type: species
  id: Skeleton
  name: species-name-skeleton
  roundStart: false
  prototype: MobSkeleton
  skinColoration: TintedHues
  sexes: [ Unsexed ]
- type: localizedDataset
  id: NamesFirstMale
  values:
    prefix: names-first-male-dataset-
    count: 3
- type: localizedDataset
  id: NamesFirstFemale
  values:
    prefix: names-first-female-dataset-
    count: 3
- type: localizedDataset
  id: NamesLast
  values:
    prefix: names-last-dataset-
    count: 3
- type: job
  id: Passenger
- type: job
  id: Captain
- type: job
  id: Borg
  setPreference: false
- type: antag
  id: Traitor
- type: antag
  id: Hidden
  setPreference: false
- type: traitCategory
  id: Quirks
  maxTraitPoints: 2
- type: trait
  id: Blind
  category: Quirks
  cost: 2
- type: trait
  id: Deaf
  category: Quirks
  cost: 1
- type: trait
  id: Accent
- type: roleLoadout
  id: JobPassenger
"##;

#[test]
fn only_one_job_keeps_high_priority() {
    let mut profile = HumanoidCharacterProfile::default();
    assert_eq!(profile.job_priorities["Passenger"], JobPriority::High);
    profile.set_job_priority("Captain", JobPriority::High);
    assert_eq!(profile.job_priorities["Passenger"], JobPriority::Medium);
    assert_eq!(profile.job_priorities["Captain"], JobPriority::High);
    profile.set_job_priority("Passenger", JobPriority::Never);
    assert!(!profile.job_priorities.contains_key("Passenger"));
    profile.set_job_priorities(BTreeMap::from([
        ("A".to_owned(), JobPriority::High),
        ("B".to_owned(), JobPriority::High),
        ("C".to_owned(), JobPriority::Never),
    ]));
    assert_eq!(
        profile.job_priorities,
        BTreeMap::from([
            ("A".to_owned(), JobPriority::High),
            ("B".to_owned(), JobPriority::Medium)
        ])
    );
}

#[test]
fn names_are_cleaned_like_ss14() {
    assert_eq!(restrict_name("Иван Smith-O'Neil!"), " Smith-O'Neil");
    assert_eq!(name_case("john von doe"), "John von Doe");
    assert_eq!(name_case("  single "), "  Single ");
    assert_eq!(gender_for_sex(Sex::Unsexed), Gender::Epicene);
    assert_eq!(Gender::parse("female"), Some(Gender::Female));
    assert_eq!(parse_sex("Unsexed"), Some(Sex::Unsexed));
}

#[test]
fn ensure_valid_repairs_profile() {
    let mut world = world_with(PROTOTYPES);
    let mut profile = HumanoidCharacterProfile {
        name: "  ivan petrov the very long name that keeps going  ".into(),
        flavor_text: "[bold]Смелый[/bold]".into(),
        species: "Skeleton".into(),
        age: 500,
        sex: Sex::Unsexed,
        voice: "Robot".into(),
        appearance: HumanoidCharacterAppearance {
            eye_color: Color::rgba(0.2, 0.4, 0.6, 0.5),
            ..Default::default()
        },
        job_priorities: BTreeMap::from([
            ("Captain".to_owned(), JobPriority::High),
            ("Borg".to_owned(), JobPriority::Low),
            ("Unknown".to_owned(), JobPriority::Low),
        ]),
        antag_preferences: ["Traitor".to_owned(), "Hidden".to_owned()].into(),
        trait_preferences: ["Blind".to_owned(), "Deaf".to_owned(), "Accent".to_owned()].into(),
        loadouts: BTreeMap::from([
            ("JobPassenger".to_owned(), RoleLoadout::default()),
            ("JobNobody".to_owned(), RoleLoadout::default()),
        ]),
        ..HumanoidCharacterProfile::default()
    };
    ensure_valid(&mut world, &mut profile);
    // Вид не из начала раунда — человек; пол — первый доступный виду.
    assert_eq!(profile.species, "Human");
    assert_eq!(profile.sex, Sex::Male);
    assert_eq!(profile.age, 120);
    assert_eq!(profile.voice, "MaleHuman");
    // Обрезка до 32 символов — до снятия пробелов, как в SS14.
    assert_eq!(profile.name, "Ivan petrov the very long Name");
    assert_eq!(profile.flavor_text, "Смелый");
    assert_eq!(profile.appearance.eye_color.to_bytes()[3], 255);
    assert_eq!(
        profile.job_priorities,
        BTreeMap::from([("Captain".to_owned(), JobPriority::High)])
    );
    assert_eq!(profile.antag_preferences, ["Traitor".to_owned()].into());
    // Очки черт в категории не больше двух: «Accent» вне категории, «Blind» занял оба очка.
    assert_eq!(
        profile.trait_preferences,
        ["Accent".to_owned(), "Blind".to_owned()].into()
    );
    assert_eq!(
        profile.loadouts.keys().collect::<Vec<_>>(),
        ["JobPassenger"]
    );
    assert_eq!(profile.loadouts["JobPassenger"].role, "JobPassenger");

    // Пустое имя заменяется случайным по набору вида.
    let mut nameless = HumanoidCharacterProfile {
        name: "!!!".into(),
        ..HumanoidCharacterProfile::default()
    };
    ensure_valid(&mut world, &mut nameless);
    assert!(nameless.name.starts_with("namepreset-firstlast"));
}

#[test]
fn random_profiles_respect_species_rules() {
    let mut world = world_with(PROTOTYPES);
    world.insert_resource(GameRandom::seeded(7));
    for _ in 0..20 {
        let profile = random_profile(&mut world, &BTreeSet::from(["Reptilian".to_owned()]));
        assert_eq!(profile.species, "Human");
        assert!((18..60).contains(&profile.age));
        assert_eq!(profile.gender, gender_for_sex(profile.sex));
        let expected_voice = if profile.sex == Sex::Female {
            "FemaleHuman"
        } else {
            "MaleHuman"
        };
        assert_eq!(profile.voice, expected_voice);
        assert_eq!(profile.job_priorities["Passenger"], JobPriority::High);
    }
    let mut seen = BTreeSet::new();
    for _ in 0..40 {
        let profile = random_profile(&mut world, &BTreeSet::new());
        if profile.species == "Reptilian" {
            assert!((20..50).contains(&profile.age));
            // Имя у ящеров — одно слово.
            assert!(profile.name.starts_with("namepreset-first"));
        }
        seen.insert(profile.species);
    }
    assert_eq!(seen, ["Human".to_owned(), "Reptilian".to_owned()].into());
}

#[test]
fn jobs_for_new_characters_come_from_config() {
    let mut world = world_with(PROTOTYPES);
    world
        .set_cvar_text("game.new_character_jobs", "Captain, Passenger")
        .unwrap();
    let mut profile = HumanoidCharacterProfile::default();
    profile.set_jobs_from_config(world.resource::<Config>());
    assert_eq!(
        profile.job_priorities,
        BTreeMap::from([
            ("Captain".to_owned(), JobPriority::High),
            ("Passenger".to_owned(), JobPriority::Medium)
        ])
    );
}
