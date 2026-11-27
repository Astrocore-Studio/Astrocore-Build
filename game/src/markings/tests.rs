use super::*;
use crate::testing::world_with;

const PROTOTYPES: &str = r##"
- type: skinColoration
  id: HumanToned
  strategy: !type:HumanTonedSkinColoration {}
  realisticColors: true
- type: skinColoration
  id: Hues
  strategy: !type:ClampedHsvColoration
    value: [0.175, 1]
- type: species
  id: Test
  name: species-name-test
  roundStart: true
  prototype: MobTest
  dollPrototype: AppearanceTest
  skinColoration: HumanToned
- type: species
  id: Lizard
  name: species-name-lizard
  roundStart: true
  prototype: MobTest
  dollPrototype: AppearanceTest
  skinColoration: Hues
- type: markingsGroup
  id: Undergarments
  limits:
    enum.HumanoidVisualLayers.UndergarmentTop:
      limit: 1
      required: false
- type: markingsGroup
  id: Test
  parent: Undergarments
  limits:
    enum.HumanoidVisualLayers.Hair:
      limit: 1
      required: false
      weight: 1
    enum.HumanoidVisualLayers.Tail:
      limit: 1
      required: true
      default: [ TailDefault ]
  appearances:
    enum.HumanoidVisualLayers.Snout:
      matchSkin: true
      layerAlpha: 0.5
- type: marking
  id: HairShort
  bodyPart: Hair
  sprites:
  - sprite: Mobs/Customization/human_hair.rsi
    state: short
- type: marking
  id: HairFemale
  bodyPart: Hair
  sexRestriction: Female
  sprites:
  - sprite: Mobs/Customization/human_hair.rsi
    state: long
- type: marking
  id: HairOther
  bodyPart: Hair
  groupWhitelist: [ Other ]
  sprites:
  - sprite: Mobs/Customization/human_hair.rsi
    state: other
- type: marking
  id: TailDefault
  bodyPart: Tail
  forcedColoring: true
  sprites:
  - sprite: Mobs/Customization/tails.rsi
    state: tail
  - sprite: Mobs/Customization/tails.rsi
    state: tip
    coloring: { type: !type:EyeColoring {} }
- type: marking
  id: SnoutPlain
  bodyPart: Snout
  sprites:
  - sprite: Mobs/Customization/snouts.rsi
    state: snout
- type: marking
  id: Shirt
  bodyPart: UndergarmentTop
  coloring:
    default:
      type: !type:SimpleColoring
        color: "#FFFFFF"
  sprites:
  - sprite: Mobs/Customization/undergarment.rsi
    state: shirt
- type: entity
  id: OrganTestHead
  components:
  - type: VisualOrganMarkings
    markingData:
      layers: [ Hair, Snout ]
      group: Test
- type: entity
  id: OrganTestTorso
  components:
  - type: VisualOrganMarkings
    markingData:
      layers: [ Tail, UndergarmentTop ]
      group: Test
- type: entity
  id: AppearanceTest
  components:
  - type: InitialBody
    organs:
      Head: OrganTestHead
      Torso: OrganTestTorso
"##;

fn marking(id: &str, colors: &[&str]) -> Marking {
    Marking {
        marking_id: id.into(),
        marking_colors: colors.iter().map(|c| Color::parse(c).unwrap()).collect(),
    }
}

fn close(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 0.01 && (a.g - b.g).abs() < 0.01 && (a.b - b.b).abs() < 0.01
}

#[test]
fn hsv_and_hsl_round_trip_like_robust() {
    let color = Color::parse("#4682B4").unwrap();
    assert!(close(from_hsv(to_hsv(color), 1.0), color));
    assert!(close(from_hsl(to_hsl(color), 1.0), color));
    let [h, s, v] = to_hsv(Color::rgba(1.0, 0.0, 0.0, 1.0));
    assert_eq!((h, s, v), (0.0, 1.0, 1.0));
    // Дополнительный цвет красного — голубой.
    let [_, plus, minus] = complementary(Color::rgba(1.0, 0.0, 0.0, 1.0), 0.5);
    assert!(close(plus, Color::rgba(0.0, 1.0, 1.0, 1.0)));
    assert!(close(minus, Color::rgba(0.0, 1.0, 1.0, 1.0)));
}

#[test]
fn skin_strategies_clamp_colors() {
    let human = SkinStrategy::HumanToned;
    assert!(human.verify(human_tone(20.0)));
    let green = Color::parse("#00ff00").unwrap();
    assert!(!human.verify(green));
    assert!(human.verify(human.closest(green)));
    let hues = SkinStrategy::ClampedHsv {
        hue: None,
        saturation: None,
        value: Some((0.175, 1.0)),
    };
    let dark = Color::parse("#050505").unwrap();
    assert!(!hues.verify(dark));
    let fixed = hues.ensure_verified(dark);
    assert!((to_hsv(fixed)[2] - 0.175).abs() < 0.001);
    // Цвет на грани проходит с поправкой на округление.
    assert_eq!(hues.ensure_verified(green), green);
    let tinted = SkinStrategy::ClampedHsl {
        hue: None,
        saturation: Some((0.0, 0.1)),
        lightness: Some((0.85, 1.0)),
    };
    assert!(tinted.verify(tinted.closest(green)));
    let nodes = SkinStrategy::HueNodes(vec![
        HueNode {
            hue: 0.0,
            saturation: (0.2, 0.5),
            value: (0.36, 0.55),
        },
        HueNode {
            hue: 0.5,
            saturation: (0.2, 0.8),
            value: (0.36, 0.55),
        },
    ]);
    let bright = from_hsv([0.25, 0.9, 0.9], 1.0);
    let fixed = nodes.closest(bright);
    let [_, s, v] = to_hsv(fixed);
    // Между узлами пределы насыщенности — посередине: 0,2…0,65.
    assert!((s - 0.65).abs() < 0.01 && (v - 0.55).abs() < 0.01);
    assert!(nodes.verify(fixed));
}

#[test]
fn groups_merge_parents_and_filter_markings() {
    let world = world_with(PROTOTYPES);
    let prototypes = world.resource::<Prototypes>();
    let group = markings_group(prototypes, "Test").unwrap();
    assert_eq!(
        group.limits.keys().collect::<Vec<_>>(),
        ["Hair", "Tail", "UndergarmentTop"]
    );
    assert!(group.appearances["Snout"].match_skin);
    let male: Vec<String> = markings_for(prototypes, "Hair", &group, Sex::Male)
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(male, ["HairShort"]);
    let female = markings_for(prototypes, "Hair", &group, Sex::Female);
    assert_eq!(female.len(), 2);
    let organs = species_marking_data(prototypes, "Test");
    assert_eq!(
        organs["Head"].layers,
        ["Hair".to_owned(), "Snout".to_owned()].into()
    );
    assert_eq!(organs["Torso"].group, "Test");
}

#[test]
fn appearance_is_validated_against_species_organs() {
    let world = world_with(PROTOTYPES);
    let prototypes = world.resource::<Prototypes>();
    let mut markings = OrganMarkings::new();
    markings.insert(
        "Head".into(),
        BTreeMap::from([(
            "Hair".to_owned(),
            vec![
                marking("HairFemale", &["#ff0000"]),
                marking("Unknown", &[]),
                marking("HairShort", &[]),
                marking("HairOther", &["#00ff00"]),
            ],
        )]),
    );
    markings.insert("Wings".into(), BTreeMap::new());
    let appearance = HumanoidCharacterAppearance {
        eye_color: Color::parse("#0000ff80").unwrap(),
        skin_color: Color::parse("#00ff00").unwrap(),
        markings,
    };
    let valid = ensure_valid_appearance(prototypes, &appearance, "Test", Sex::Male);
    // Кожа — к человеческому тону, глаза без прозрачности.
    assert!(SkinStrategy::HumanToned.verify(valid.skin_color));
    assert_eq!(valid.eye_color.to_bytes()[3], 255);
    // Органа «Wings» у вида нет; женская и чужая причёски убраны, цвета — по слоям.
    assert!(!valid.markings.contains_key("Wings"));
    assert_eq!(
        valid.markings["Head"]["Hair"],
        [marking("HairShort", &["#FFFFFF"])]
    );
    // Обязательный хвост — по умолчанию, цвета по окраске: кожа и глаза.
    let tail = &valid.markings["Torso"]["Tail"][0];
    assert_eq!(tail.marking_id, "TailDefault");
    assert_eq!(tail.marking_colors[0], valid.skin_color);
    assert_eq!(tail.marking_colors[1], valid.eye_color);

    // Две причёски при пределе одна: счёт с конца, как в SS14, — остаётся последняя.
    let mut two = appearance.clone();
    two.markings.get_mut("Head").unwrap().insert(
        "Hair".into(),
        vec![
            marking("HairFemale", &["#ff0000"]),
            marking("HairShort", &["#00ff00"]),
        ],
    );
    let valid = ensure_valid_appearance(prototypes, &two, "Test", Sex::Female);
    assert_eq!(valid.markings["Head"]["Hair"].len(), 1);
    assert_eq!(valid.markings["Head"]["Hair"][0].marking_id, "HairShort");
}

#[test]
fn random_appearance_fits_species() {
    let world = world_with(PROTOTYPES);
    let prototypes = world.resource::<Prototypes>();
    let mut random = GameRandom::seeded(3);
    for _ in 0..10 {
        let appearance = random_appearance(prototypes, &mut random, "Lizard", Sex::Male);
        // Причёска с весом 1 есть всегда, хвост обязателен.
        assert_eq!(
            appearance.markings["Head"]["Hair"][0].marking_id,
            "HairShort"
        );
        assert_eq!(appearance.markings["Torso"]["Tail"].len(), 1);
        assert!(to_hsv(appearance.skin_color)[2] >= 0.175 - 0.001);
        assert_eq!(
            ensure_valid_appearance(prototypes, &appearance, "Lizard", Sex::Male),
            appearance
        );
    }
}

#[test]
fn forced_colors_are_resolved_on_body() {
    let world = world_with(PROTOTYPES);
    let prototypes = world.resource::<Prototypes>();
    let skin = Color::parse("#aa8866").unwrap();
    let eyes = Color::parse("#112233").unwrap();
    let group = markings_group(prototypes, "Test").unwrap();
    let resolved = resolve_markings(
        prototypes,
        &[
            marking("TailDefault", &["#000000", "#000000"]),
            marking("SnoutPlain", &["#123456"]),
            marking("HairShort", &["#654321"]),
        ],
        Some(skin),
        Some(eyes),
        &group.appearances,
    );
    // Свободные цвета — как были; принудительные — по коже и глазам; морда — под кожу.
    assert_eq!(resolved[0], marking("HairShort", &["#654321"]));
    assert_eq!(resolved[1].marking_colors, [skin, eyes]);
    assert_eq!(resolved[2].marking_colors, [skin.with_alpha(0.5)]);
}
