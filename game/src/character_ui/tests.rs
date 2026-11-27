use std::collections::BTreeMap;

use super::*;
use crate::Side;
use crate::profiles::PlayerPreferences;
use crate::profiles::tests::PROTOTYPES;

fn world() -> GameWorld {
    let prototypes = format!(
        "{PROTOTYPES}
- type: skinColoration
  id: HumanToned
  strategy: !type:HumanTonedSkinColoration {{}}
"
    );
    let mut world = crate::testing::world_with(&prototypes);
    crate::preferences::install(&mut world, Side::Client);
    let bob = HumanoidCharacterProfile {
        name: "Bob".into(),
        ..HumanoidCharacterProfile::default()
    };
    let ann = HumanoidCharacterProfile {
        name: "Ann".into(),
        ..HumanoidCharacterProfile::default()
    };
    *world.resource_mut::<ClientPreferences>() = ClientPreferences {
        preferences: Some(PlayerPreferences {
            characters: BTreeMap::from([(0, bob), (2, ann)]),
            selected_character_index: 2,
            admin_ooc_color: GameColor::WHITE,
        }),
        max_character_slots: 3,
    };
    world
}

fn state(world: &GameWorld) -> CharacterEditor {
    world.resource::<CharacterEditor>().clone()
}

fn prefs(world: &GameWorld) -> PlayerPreferences {
    world
        .resource::<ClientPreferences>()
        .preferences
        .clone()
        .unwrap()
}

#[test]
fn opens_selected_character_and_saves_validated_edits() {
    let mut world = world();
    toggle(&mut world);
    let opened = state(&world);
    assert!(opened.open);
    assert_eq!(opened.slot, Some(2));
    assert_eq!(opened.profile.unwrap().name, "Ann");

    // Имя и возраст печатаются; возраст — только цифрами.
    type_text(&mut world, Field::Name, "a", false);
    type_text(&mut world, Field::Age, "x", false);
    type_text(&mut world, Field::Age, "9", false);
    let edited = state(&world);
    assert!(edited.dirty);
    let profile = edited.profile.unwrap();
    assert_eq!((profile.name.as_str(), profile.age), ("Anna", 189));

    // Сохранение проверяет профиль: возраст — в пределах вида.
    save(&mut world);
    let saved = state(&world);
    assert!(!saved.dirty);
    assert_eq!(saved.profile.as_ref().unwrap().age, 120);
    assert_eq!(prefs(&world).characters[&2].age, 120);
    assert_eq!(prefs(&world).characters[&2].name, "Anna");
}

#[test]
fn closing_with_changes_asks_first() {
    let mut world = world();
    toggle(&mut world);
    type_text(&mut world, Field::Name, "", true);
    close(&mut world);
    assert!(state(&world).open);
    assert!(state(&world).confirm_close);
    // «Отмена» оставляет окно и правки.
    answer_close(&mut world, None);
    assert!(state(&world).open && state(&world).dirty);
    // «Не сохранять» закрывает и возвращает сохранённое.
    close(&mut world);
    answer_close(&mut world, Some(false));
    let closed = state(&world);
    assert!(!closed.open && !closed.dirty);
    assert_eq!(closed.profile.unwrap().name, "Ann");
    assert_eq!(prefs(&world).characters[&2].name, "Ann");
}

#[test]
fn slots_are_created_selected_and_deleted() {
    let mut world = world();
    toggle(&mut world);
    new_character(&mut world);
    // Новый — в первый свободный слот и сразу открыт.
    assert_eq!(state(&world).slot, Some(1));
    assert_eq!(prefs(&world).characters.len(), 3);
    select(&mut world, 0);
    assert_eq!(prefs(&world).selected_character_index, 0);
    assert_eq!(state(&world).profile.unwrap().name, "Bob");
    // Удаление — со второго нажатия; открытым становится выбранный.
    delete(&mut world, 0);
    assert_eq!(prefs(&world).characters.len(), 3);
    assert_eq!(state(&world).confirm_delete, Some(0));
    delete(&mut world, 0);
    assert!(!prefs(&world).characters.contains_key(&0));
    assert_eq!(state(&world).confirm_delete, None);
    assert!(state(&world).slot.is_some_and(|s| s != 0));
}

#[test]
fn palette_paints_selected_target() {
    let mut world = world();
    toggle(&mut world);
    set_target(&mut world, ColorTarget::Eyes);
    let green = GameColor::parse("#00ff00").unwrap();
    apply_color(&mut world, green);
    let painted = state(&world);
    assert_eq!(painted.profile.unwrap().appearance.eye_color, green);
    assert_eq!(painted.hex, "#00FF00");
    // Цвет текстом — по Enter.
    {
        let mut editor_state = world.resource_mut::<CharacterEditor>();
        editor_state.typing = Some(Field::Hex);
        editor_state.hex = "#1020".into();
    }
    type_text(&mut world, Field::Hex, "30", false);
    finish_typing(&mut world);
    assert_eq!(
        state(&world).profile.unwrap().appearance.eye_color,
        GameColor::parse("#102030").unwrap()
    );
    // Кожа человека — тоном: зелёный подтягивается к человеческому.
    set_target(&mut world, ColorTarget::Skin);
    apply_color(&mut world, green);
    let skin = state(&world).profile.unwrap().appearance.skin_color;
    assert!(crate::markings::SkinStrategy::HumanToned.verify(skin));
    assert_eq!(palette_colors().len(), 75);
}

#[test]
fn randomize_keeps_jobs() {
    let mut world = world();
    toggle(&mut world);
    change(&mut world, |_, profile, _| {
        profile.set_job_priority("Captain", JobPriority::High);
    });
    randomize(&mut world);
    let profile = state(&world).profile.unwrap();
    assert_eq!(profile.job_priorities["Captain"], JobPriority::High);
    assert!(profile.name.starts_with("namepreset-"));
}
