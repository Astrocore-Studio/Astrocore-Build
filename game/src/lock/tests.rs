use super::*;
use crate::access::tests::{PROTOTYPES as ACCESS, human};
use crate::entity_storage::EntityStorageWorldExt;
use crate::hands::HandsWorldExt;
use crate::testing::{spawn_at, station, world_with};

fn prototypes() -> String {
    format!(
        "{ACCESS}{}",
        r#"
- type: entity
  id: LockerEngineering
  components:
  - type: EntityStorage
  - type: Appearance
  - type: AccessReader
    access: [ [ Engineering ] ]
  - type: Lock
"#
    )
}

#[test]
fn locked_locker_needs_access_to_open() {
    let mut world = world_with(&prototypes());
    let grid = station(&mut world);
    let user = human(&mut world, grid);
    let locker = spawn_at(&mut world, "LockerEngineering", grid, 2, 1);
    assert!(is_locked(&world, locker));
    assert!(!world.can_open(user, locker, true));
    assert!(!try_unlock(&mut world, locker, user));
    assert!(is_locked(&world, locker));

    let card = spawn_at(&mut world, "EngineeringIDCard", grid, 1, 1);
    assert!(world.try_pickup(user, card, None, false));
    assert!(try_unlock(&mut world, locker, user));
    assert!(!is_locked(&world, locker));
    assert!(world.can_open(user, locker, true));
    assert!(try_lock(&mut world, locker, user));
    assert!(is_locked(&world, locker));
}
