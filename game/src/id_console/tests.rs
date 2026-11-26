use super::*;
use crate::access::tests::{PROTOTYPES as ACCESS, human};
use crate::item_slots::insert;
use crate::testing::{spawn_at, station, world_with};

fn prototypes() -> String {
    format!(
        "{ACCESS}{}",
        r#"
- type: entity
  id: ComputerId
  components:
  - type: AccessReader
    access: [ [ Security ] ]
  - type: IdCardConsole
    accessLevels: [ Engineering, Security ]
"#
    )
}

#[test]
fn captain_card_grants_levels_the_console_knows() {
    let mut world = world_with(&prototypes());
    let grid = station(&mut world);
    let _user = human(&mut world, grid);
    let console = spawn_at(&mut world, "ComputerId", grid, 2, 1);
    let captain = spawn_at(&mut world, "CaptainIDCard", grid, 1, 1);
    let target = spawn_at(&mut world, "EngineeringIDCard", grid, 1, 1);
    insert(&mut world, console, TARGET_SLOT, target);

    // Без карты с правами — отказ.
    let write = |access: &[&str]| WriteToTargetId {
        full_name: "Urist McHands".into(),
        job_title: "Engineer".into(),
        access: access.iter().map(|s| s.to_string()).collect(),
    };
    assert!(!write_to_target(
        &mut world,
        console,
        &write(&["Engineering", "Security"])
    ));

    insert(&mut world, console, PRIVILEGED_SLOT, captain);
    let state = console_state(&world, console);
    assert!(state.privileged_authorized && state.target_present);
    assert_eq!(state.target_access, ["Engineering"]);
    assert!(write_to_target(
        &mut world,
        console,
        &write(&["Engineering", "Security"])
    ));
    assert_eq!(
        world.get::<Access>(target).unwrap().tags,
        ["Engineering", "Security"].map(String::from).into()
    );
    assert_eq!(
        world.get::<IdCard>(target).unwrap().full_name.as_deref(),
        Some("Urist McHands")
    );
    // Уровень, которого консоль не знает, не пишется.
    assert!(!write_to_target(&mut world, console, &write(&["Bridge"])));
}

#[test]
fn engineer_card_cannot_grant_security() {
    let mut world = world_with(
        &prototypes().replace("access: [ [ Security ] ]", "access: [ [ Engineering ] ]"),
    );
    let grid = station(&mut world);
    let console = spawn_at(&mut world, "ComputerId", grid, 2, 1);
    let privileged = spawn_at(&mut world, "EngineeringIDCard", grid, 1, 1);
    let target = spawn_at(&mut world, "EngineeringIDCard", grid, 1, 1);
    insert(&mut world, console, PRIVILEGED_SLOT, privileged);
    insert(&mut world, console, TARGET_SLOT, target);
    let message = WriteToTargetId {
        full_name: String::new(),
        job_title: String::new(),
        access: vec!["Engineering".into(), "Security".into()],
    };
    assert!(!write_to_target(&mut world, console, &message));
    assert_eq!(
        world.get::<Access>(target).unwrap().tags,
        ["Engineering"].map(String::from).into()
    );
}
