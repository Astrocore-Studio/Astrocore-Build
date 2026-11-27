use super::*;
use crate::damage::{DamageSpecifier, DamageableWorldExt};
use crate::fixed_point::FixedPoint2;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: entity
  abstract: true
  id: BaseCritAction
  components:
  - type: Action
- type: entity
  parent: BaseCritAction
  id: ActionCritSuccumb
  components:
  - type: InstantAction
    event: !type:CritSuccumbEvent
- type: entity
  parent: BaseCritAction
  id: ActionCritFakeDeath
  components:
  - type: InstantAction
    event: !type:CritFakeDeathEvent
- type: entity
  id: Mob
  components:
  - type: Actions
  - type: Damageable
  - type: Injurable
  - type: MobState
  - type: MobThresholds
    thresholds:
      0: Alive
      100: Critical
      200: Dead
  - type: Deathgasp
  - type: MobStateActions
    actions:
      Critical:
      - ActionCritSuccumb
      - ActionCritFakeDeath
"#;

fn blunt(amount: i32) -> DamageSpecifier {
    DamageSpecifier::new([("Blunt", FixedPoint2::new(amount))])
}

#[test]
fn crit_grants_its_actions_and_recovery_takes_them_away() {
    let prototypes = format!("{}{PROTOTYPES}", crate::damage::tests::DAMAGE_PROTOTYPES);
    let mut world = world_with(&prototypes);
    let grid = station(&mut world);
    let mob = spawn_at(&mut world, "Mob", grid, 2, 2);
    assert!(world.actions_of(mob).is_empty());
    world.change_damage(mob, &blunt(120), true, true, None);
    let granted = world.actions_of(mob);
    assert_eq!(granted.len(), 2);
    // Притвориться мёртвым можно только в крите; без сессии сдаться нельзя.
    assert!(fake_death(&mut world, mob));
    assert!(!succumb(&mut world, mob));
    world.change_damage(mob, &blunt(-60), true, true, None);
    assert!(world.actions_of(mob).is_empty());
    assert!(granted.iter().all(|a| world.get_entity(*a).is_err()));
    assert!(!fake_death(&mut world, mob));
}

#[test]
fn last_words_are_cut_to_thirty_characters() {
    assert_eq!(last_words_text("прощайте"), "прощайте...");
    assert_eq!(
        last_words_text(&"я".repeat(40)),
        format!("{}...", "я".repeat(30))
    );
}
