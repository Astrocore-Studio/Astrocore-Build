use super::*;
use crate::damage::Damageable;
use crate::solution::ReagentId;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: damageType
  id: Caustic
  name: damage-type-caustic
- type: damageType
  id: Poison
  name: damage-type-poison
- type: reagent
  id: Acid
  reactiveEffects:
    Acidic:
      methods: [ Touch ]
      effects:
      - !type:HealthChange
        damage:
          types:
            Caustic: 0.5
- type: reagent
  id: Venom
- type: entity
  id: Mob
  components:
  - type: Damageable
  - type: Injurable
  - type: Reactive
    groups:
      Acidic: [ Touch ]
    reactions:
    - methods: [ Injection ]
      reagents: [ Venom ]
      effects:
      - !type:HealthChange
        damage:
          types:
            Poison: 1
"#;

fn damage(world: &World, entity: Entity, kind: &str) -> f32 {
    world
        .get::<Damageable>(entity)
        .and_then(|d| d.damage.types.get(kind))
        .map_or(0.0, |v| v.float())
}

fn solution(reagent: &str, amount: i32) -> Solution {
    Solution::from_reagents([(ReagentId::new(reagent), FixedPoint2::new(amount))])
}

#[test]
fn groups_and_own_reactions_scale_with_quantity() {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let mob = spawn_at(&mut world, "Mob", grid, 1, 1);
    // Кислота касанием: 10 × 0,5.
    do_entity_reaction(
        &mut world,
        mob,
        &solution("Acid", 10),
        ReactionMethod::Touch,
    );
    assert_eq!(damage(&world, mob, "Caustic"), 5.0);
    // Впрыск кислоты на группу «кислота» не действует: группа отзывается только на касание.
    do_entity_reaction(
        &mut world,
        mob,
        &solution("Acid", 10),
        ReactionMethod::Injection,
    );
    assert_eq!(damage(&world, mob, "Caustic"), 5.0);
    // Своя реакция на впрыск яда.
    do_entity_reaction(
        &mut world,
        mob,
        &solution("Venom", 3),
        ReactionMethod::Injection,
    );
    assert_eq!(damage(&world, mob, "Poison"), 3.0);
    do_entity_reaction(
        &mut world,
        mob,
        &solution("Venom", 3),
        ReactionMethod::Touch,
    );
    assert_eq!(damage(&world, mob, "Poison"), 3.0);
}
