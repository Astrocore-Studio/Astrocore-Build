use astrocore_ecs::GameWorldExt;
use astrocore_map::ContainerWorldExt;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: species
  id: Human
  name: species-name-human
  prototype: MobHuman
  skinColoration: HumanToned
- type: entity
  id: OrganHandLeft
  components:
  - type: Organ
    category: HandLeft
  - type: HandOrgan
    handID: left
    data:
      location: Left
  - type: VisualOrgan
    layer: enum.HumanoidVisualLayers.LHand
    data:
      state: l_hand
- type: entity
  id: OrganHandRight
  components:
  - type: Organ
    category: HandRight
  - type: HandOrgan
    handID: right
    data:
      location: Right
  - type: VisualOrgan
    layer: enum.HumanoidVisualLayers.RHand
    data:
      state: r_hand
- type: entity
  id: OrganTorso
  components:
  - type: Organ
    category: Torso
  - type: VisualOrgan
    layer: enum.HumanoidVisualLayers.Chest
    data:
      state: torso_m
    sexStateOverrides:
      Male: torso_m
      Female: torso_f
- type: entity
  id: MobHuman
  components:
  - type: Body
  - type: VisualBody
  - type: Hands
  - type: HumanoidProfile
    species: Human
    sex: Female
  - type: InitialBody
    organs:
      Torso: OrganTorso
      HandRight: OrganHandRight
      HandLeft: OrganHandLeft
"#;

#[test]
fn skin_tone_matches_ss14() {
    // Тон 20 — оттенок 25°, насыщенность 20%, яркость 100%.
    let color = human_skin_tone(20.0);
    assert_eq!(color.to_bytes(), [255, 225, 204, 255]);
    // Тон 100 — самая тёмная кожа.
    let dark = human_skin_tone(100.0);
    assert!(dark.r < 0.25 && dark.g < dark.r);
}

#[test]
fn organs_give_hands_and_skin_from_the_species() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let human = spawn_at(&mut world, "MobHuman", grid, 2, 2);
    world.run_tick();

    let organs = world.organs(human);
    assert_eq!(organs.len(), 3);
    assert_eq!(
        world.hands_in_order(human),
        vec!["right".to_owned(), "left".to_owned()]
    );
    for organ in &organs {
        assert_eq!(world.get::<Organ>(*organ).unwrap().body, Some(human));
        let visual = world.get::<VisualOrgan>(*organ).unwrap();
        assert_eq!(visual.data.color, Some(human_skin_tone(20.0)));
    }
    let torso = organs
        .iter()
        .find(|o| world.get::<Organ>(**o).unwrap().category.as_deref() == Some("Torso"))
        .copied()
        .unwrap();
    // Состояние по полу профиля.
    assert_eq!(
        world
            .get::<VisualOrgan>(torso)
            .unwrap()
            .data
            .state
            .as_deref(),
        Some("torso_f")
    );

    // Нет кисти — нет руки.
    let hand = organs
        .iter()
        .find(|o| {
            world
                .get::<HandOrgan>(**o)
                .is_some_and(|h| h.hand_id == "left")
        })
        .copied()
        .unwrap();
    world.remove_from_container(hand, None).unwrap();
    assert_eq!(world.hands_in_order(human), vec!["right".to_owned()]);
    assert_eq!(world.get::<Organ>(hand).unwrap().body, None);
}
