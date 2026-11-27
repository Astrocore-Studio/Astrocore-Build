use super::*;
use crate::profiles::Gender;
use crate::testing::world_with;

const PROTOTYPES: &str = r##"
- type: skinColoration
  id: HumanToned
  strategy: !type:HumanTonedSkinColoration {}
- type: skinColoration
  id: Hues
  strategy: !type:ClampedHsvColoration
    value: [0.175, 1]
- type: species
  id: Human
  name: species-name-human
  roundStart: true
  prototype: MobHuman
  dollPrototype: AppearanceHuman
  skinColoration: HumanToned
- type: species
  id: Lizard
  name: species-name-lizard
  roundStart: true
  prototype: MobLizard
  dollPrototype: AppearanceHuman
  skinColoration: Hues
  sexes: [ Female, Unsexed ]
  voices: [ Reptilian ]
  defaultSoundsBySex: [ Reptilian, Reptilian, Reptilian ]
- type: emoteSounds
  id: MaleHuman
  voiceSelectorName: humanoid-profile-editor-voice-masculine
- type: markingsGroup
  id: Human
  limits:
    enum.HumanoidVisualLayers.Hair:
      limit: 1
      required: false
    enum.HumanoidVisualLayers.Tail:
      limit: 1
      required: true
      default: [ TailPlain ]
    enum.HumanoidVisualLayers.Chest:
      limit: 2
      required: false
  appearances:
    enum.HumanoidVisualLayers.Snout:
      matchSkin: true
- type: marking
  id: HairShort
  bodyPart: Hair
  sprites:
  - sprite: Mobs/Customization/human_hair.rsi
    state: short
- type: marking
  id: HairLong
  bodyPart: Hair
  sexRestriction: Female
  sprites:
  - sprite: Mobs/Customization/human_hair.rsi
    state: long
- type: marking
  id: TailPlain
  bodyPart: Tail
  sprites:
  - sprite: Mobs/Customization/tails.rsi
    state: plain
- type: marking
  id: TailSpiked
  bodyPart: Tail
  forcedColoring: true
  sprites:
  - sprite: Mobs/Customization/tails.rsi
    state: spiked
- type: marking
  id: TattooA
  bodyPart: Chest
  coloring:
    default:
      type: !type:SimpleColoring
        color: "#112233"
  sprites:
  - sprite: Mobs/Customization/tattoos.rsi
    state: a
  - sprite: Mobs/Customization/tattoos.rsi
    state: a2
- type: marking
  id: TattooB
  bodyPart: Chest
  sprites:
  - sprite: Mobs/Customization/tattoos.rsi
    state: b
- type: marking
  id: TattooC
  bodyPart: Chest
  sprites:
  - sprite: Mobs/Customization/tattoos.rsi
    state: c
- type: marking
  id: SnoutPlain
  bodyPart: Snout
  sprites:
  - sprite: Mobs/Customization/snouts.rsi
    state: snout
- type: entity
  id: OrganHead
  components:
  - type: VisualOrganMarkings
    markingData:
      layers: [ Hair, Snout ]
      group: Human
- type: entity
  id: OrganTorso
  components:
  - type: VisualOrganMarkings
    markingData:
      layers: [ Tail, Chest ]
      group: Human
- type: entity
  id: AppearanceHuman
  components:
  - type: InitialBody
    organs:
      Head: OrganHead
      Torso: OrganTorso
- type: job
  id: Passenger
  name: job-name-passenger
- type: job
  id: Visitor
  name: job-name-visitor
- type: job
  id: Captain
  name: job-name-captain
- type: job
  id: HeadOfPersonnel
  name: job-name-hop
- type: job
  id: Borg
  setPreference: false
- type: jobWeight
  id: Default
  weights:
    Passenger: 0
    Visitor: 0
    Captain: 20
    HeadOfPersonnel: 20
- type: department
  id: Civilian
  name: department-Civilian
  color: "#7F7F7F"
  weight: -10
  roles: [ Visitor, Passenger ]
- type: department
  id: Command
  name: department-Command
  weight: 100
  roles: [ HeadOfPersonnel, Captain ]
- type: department
  id: Silicon
  name: department-Silicon
  roles: [ Borg ]
- type: department
  id: CentralCommand
  name: department-CentralCommand
  weight: 120
  editorHidden: true
  roles: [ Captain ]
- type: antag
  id: Traitor
  name: roles-antag-traitor-name
  setPreference: true
- type: antag
  id: Hidden
  name: roles-antag-hidden-name
"##;

fn profile(prototypes: &Prototypes) -> HumanoidCharacterProfile {
    let mut profile = HumanoidCharacterProfile::default();
    revalidate(prototypes, &mut profile);
    profile
}

#[test]
fn species_change_fixes_sex_voice_skin_and_markings() {
    let world = world_with(PROTOTYPES);
    let prototypes = world.resource::<Prototypes>();
    let mut profile = profile(prototypes);
    // У человека обязательный хвост по умолчанию.
    assert_eq!(
        selected(&profile, "Torso", "Tail")[0].marking_id,
        "TailPlain"
    );
    set_skin(prototypes, &mut profile, Some(80.0), Color::WHITE);
    assert_eq!(skin_unary(prototypes, &profile).map(f32::round), Some(80.0));

    set_species(prototypes, &mut profile, "Lizard");
    // Мужского пола у ящера нет — первый доступный; голос — свой у вида.
    assert_eq!(profile.sex, Sex::Female);
    assert_eq!(profile.gender, Gender::Female);
    assert_eq!(profile.voice, "Reptilian");
    // Кожа задаётся цветом: ползунка нет, тёмный цвет подтянут к пределу яркости.
    assert_eq!(skin_unary(prototypes, &profile), None);
    set_skin(
        prototypes,
        &mut profile,
        None,
        Color::parse("#050505").unwrap(),
    );
    let value = crate::markings::to_hsv(profile.appearance.skin_color)[2];
    assert!((value - 0.175).abs() < 0.01);

    set_sex(prototypes, &mut profile, Sex::Unsexed);
    assert_eq!(profile.gender, Gender::Epicene);
    assert_eq!(profile.voice, "Reptilian");
    assert_eq!(
        voice_name(prototypes, "MaleHuman"),
        "humanoid-profile-editor-voice-masculine"
    );
    assert_eq!(
        voice_name(prototypes, "Reptilian"),
        "humanoid-profile-editor-voice-none"
    );
}

#[test]
fn markings_are_selected_within_limits() {
    let world = world_with(PROTOTYPES);
    let prototypes = world.resource::<Prototypes>();
    let mut profile = profile(prototypes);
    let mut previous = PreviousColors::new();
    let layers = layer_choices(prototypes, &profile, "Head");
    let hair = layers.iter().find(|c| c.layer == "Hair").unwrap();
    // Женская причёска мужчине недоступна.
    assert_eq!(
        hair.available
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["HairShort"]
    );
    assert!(!select_marking(
        prototypes,
        &mut profile,
        "Head",
        "Hair",
        "HairLong",
        &previous
    ));
    assert!(select_marking(
        prototypes,
        &mut profile,
        "Head",
        "Hair",
        "HairShort",
        &previous
    ));

    // Предел один — новая метка заменяет прежнюю.
    assert!(select_marking(
        prototypes,
        &mut profile,
        "Torso",
        "Tail",
        "TailSpiked",
        &previous
    ));
    assert_eq!(selected(&profile, "Torso", "Tail").len(), 1);
    assert_eq!(
        selected(&profile, "Torso", "Tail")[0].marking_id,
        "TailSpiked"
    );
    // Обязательный слой пустым не остаётся.
    assert!(!deselect_marking(
        prototypes,
        &mut profile,
        "Torso",
        "Tail",
        "TailSpiked",
        &mut previous
    ));

    // Предел два: третья не встаёт; цвета — по окраске метки, по числу слоёв.
    assert!(select_marking(
        prototypes,
        &mut profile,
        "Torso",
        "Chest",
        "TattooA",
        &previous
    ));
    assert!(select_marking(
        prototypes,
        &mut profile,
        "Torso",
        "Chest",
        "TattooB",
        &previous
    ));
    assert!(!select_marking(
        prototypes,
        &mut profile,
        "Torso",
        "Chest",
        "TattooC",
        &previous
    ));
    let tattoo = &selected(&profile, "Torso", "Chest")[0];
    assert_eq!(tattoo.marking_colors, [Color::parse("#112233").unwrap(); 2]);

    // Снятая метка помнит цвета и возвращается с ними.
    set_marking_color(
        prototypes,
        &mut profile,
        "Torso",
        "Chest",
        "TattooA",
        1,
        Color::WHITE,
    );
    assert!(deselect_marking(
        prototypes,
        &mut profile,
        "Torso",
        "Chest",
        "TattooA",
        &mut previous
    ));
    assert_eq!(selected(&profile, "Torso", "Chest").len(), 1);
    assert!(select_marking(
        prototypes,
        &mut profile,
        "Torso",
        "Chest",
        "TattooA",
        &previous
    ));
    let tattoo = selected(&profile, "Torso", "Chest")
        .iter()
        .find(|m| m.marking_id == "TattooA")
        .unwrap();
    assert_eq!(tattoo.marking_colors[1], Color::WHITE);

    // Цвета не меняются у принудительной окраски и у слоя под цвет кожи.
    assert!(color_customizable(
        prototypes, &profile, "Torso", "Chest", "TattooA"
    ));
    assert!(!color_customizable(
        prototypes,
        &profile,
        "Torso",
        "Tail",
        "TailSpiked"
    ));
    assert!(!color_customizable(
        prototypes,
        &profile,
        "Head",
        "Snout",
        "SnoutPlain"
    ));
}

#[test]
fn departments_and_antags_follow_ss14_order() {
    let world = world_with(PROTOTYPES);
    let prototypes = world.resource::<Prototypes>();
    let list = departments(prototypes);
    // Скрытый отдел не показан; пустой (без выбираемых должностей) остаётся.
    assert_eq!(
        list.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        ["Command", "Silicon", "Civilian"]
    );
    assert_eq!(
        list[0]
            .jobs
            .iter()
            .map(|j| j.id.as_str())
            .collect::<Vec<_>>(),
        ["Captain", "HeadOfPersonnel"]
    );
    assert!(list[1].jobs.is_empty());
    assert_eq!(
        list[2]
            .jobs
            .iter()
            .map(|j| j.id.as_str())
            .collect::<Vec<_>>(),
        ["Passenger", "Visitor"]
    );
    assert_eq!(list[2].color, Color::parse("#7F7F7F"));
    assert_eq!(list[0].jobs[0].name, "job-name-captain");

    // Без `setPreference: true` антагонист не выбирается.
    let antags = antags(prototypes);
    assert_eq!(antags.len(), 1);
    assert_eq!(antags[0].id, "Traitor");
    let mut profile = HumanoidCharacterProfile::default();
    set_antag(&mut profile, "Traitor", true);
    assert!(profile.antag_preferences.contains("Traitor"));
    set_antag(&mut profile, "Traitor", false);
    assert!(profile.antag_preferences.is_empty());
    assert_eq!(job_priority(&profile, "Passenger"), JobPriority::High);
    assert_eq!(job_priority(&profile, "Captain"), JobPriority::Never);
}

#[test]
fn hex_colors_round_trip() {
    assert_eq!(parse_hex("#aa5500"), Color::parse("#aa5500"));
    assert_eq!(parse_hex(" AA5500 "), Color::parse("#aa5500"));
    assert_eq!(parse_hex("#aa55"), None);
    assert_eq!(parse_hex("#gg5500"), None);
    assert_eq!(hex(Color::parse("#aa5500").unwrap()), "#AA5500");
}
