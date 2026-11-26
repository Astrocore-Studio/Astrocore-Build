use astrocore_ecs::GameWorldExt;

use super::*;
use crate::do_after::{DoAfterArgs, DoAfterStart};
use crate::testing::{spawn_at, station, world_with};

pub(crate) const DAMAGE_PROTOTYPES: &str = r#"
- type: damageType
  id: Blunt
  name: damage-type-blunt
- type: damageType
  id: Slash
  name: damage-type-slash
- type: damageType
  id: Heat
  name: damage-type-heat
- type: damageGroup
  id: Brute
  name: damage-group-brute
  damageTypes: [ Blunt, Slash ]
- type: damageGroup
  id: Burn
  name: damage-group-burn
  damageTypes: [ Heat ]
- type: damageContainer
  id: Biological
  supportedGroups: [ Brute, Burn ]
- type: damageContainer
  id: Bruisable
  supportedGroups: [ Brute ]
- type: damageModifierSet
  id: Padded
  coefficients:
    Blunt: 0.5
  flatReductions:
    Blunt: 1
"#;

fn prototypes(extra: &str) -> String {
    format!("{DAMAGE_PROTOTYPES}{extra}")
}

const ENTITIES: &str = r#"
- type: entity
  id: Dummy
  components:
  - type: Damageable
    damageModifierSet: Padded
  - type: Injurable
    damageContainer: Bruisable
  - type: DoAfter
- type: entity
  id: Statue
  components:
  - type: Damageable
"#;

fn blunt(amount: f32) -> DamageSpecifier {
    DamageSpecifier::new([("Blunt", FixedPoint2::from_f32(amount))])
}

#[test]
fn resistances_and_container_shape_the_damage() {
    let mut world = world_with(&prototypes(ENTITIES));
    let grid = station(&mut world);
    let dummy = spawn_at(&mut world, "Dummy", grid, 2, 2);

    // (5 − 1) × 0,5 = 2 тупого; ожог контейнер не принимает.
    let damage = DamageSpecifier::new([
        ("Blunt", FixedPoint2::new(5)),
        ("Heat", FixedPoint2::new(3)),
    ]);
    let dealt = world.change_damage(dummy, &damage, false, true, None);
    assert_eq!(dealt.types["Blunt"], FixedPoint2::new(2));
    let damageable = world.get::<Damageable>(dummy).unwrap();
    assert_eq!(damageable.damage.types.get("Heat"), None);
    assert_eq!(damageable.total_damage, FixedPoint2::new(2));
    assert_eq!(damageable.damage_per_group["Brute"], FixedPoint2::new(2));
    assert!(!damageable.damage_per_group.contains_key("Burn"));

    // Лечение сопротивлениями не ослабляется и не уводит ниже нуля.
    world.change_damage(dummy, &blunt(-10.0), false, true, None);
    assert_eq!(world.total_damage(dummy), FixedPoint2::ZERO);

    // Без компонента модели урона (`Injurable`) урон не записывается — как в SS14.
    let statue = spawn_at(&mut world, "Statue", grid, 3, 2);
    world.change_damage(statue, &blunt(5.0), true, true, None);
    assert_eq!(world.total_damage(statue), FixedPoint2::ZERO);
}

#[test]
fn healing_evenly_and_proportionally() {
    let mut world = world_with(&prototypes(ENTITIES));
    let grid = station(&mut world);
    let dummy = spawn_at(&mut world, "Dummy", grid, 2, 2);
    let hurt = DamageSpecifier::new([
        ("Blunt", FixedPoint2::new(10)),
        ("Slash", FixedPoint2::new(4)),
    ]);
    world.change_damage(dummy, &hurt, true, true, None);

    world.heal_evenly(dummy, FixedPoint2::new(-6), None, None);
    let damage = &world.get::<Damageable>(dummy).unwrap().damage;
    assert_eq!(damage.types["Blunt"], FixedPoint2::new(7));
    assert_eq!(damage.types["Slash"], FixedPoint2::new(1));

    world.heal_distributed(dummy, FixedPoint2::new(-4), Some("Brute"), None);
    let damage = &world.get::<Damageable>(dummy).unwrap().damage;
    // Доли 7/8 и 1/8 округляются вниз до сотых, как в SS14: снимается 3,96, а не 4.
    assert_eq!(damage.total(), FixedPoint2::from_cents(404));
    assert!(damage.types["Blunt"] > damage.types["Slash"]);

    // Лечения больше, чем урона, — всё снимается.
    world.heal_evenly(dummy, FixedPoint2::new(-100), None, None);
    assert_eq!(world.total_damage(dummy), FixedPoint2::ZERO);

    let spread = DamageSpecifier::from_group(
        &["Blunt".into(), "Slash".into(), "Heat".into()],
        FixedPoint2::new(10),
    );
    assert_eq!(spread.total(), FixedPoint2::new(10));
    assert_eq!(spread.types["Blunt"], FixedPoint2::from_cents(333));
    assert_eq!(spread.types["Heat"], FixedPoint2::from_cents(334));
}

#[derive(Debug, Clone)]
struct Bandage;

#[test]
fn damage_interrupts_do_afters_above_threshold() {
    let mut world = world_with(&prototypes(ENTITIES));
    let grid = station(&mut world);
    let dummy = spawn_at(&mut world, "Dummy", grid, 2, 2);
    world.run_tick();
    let mut args = DoAfterArgs::new(dummy, 5.0, None, None, None);
    args.break_on_damage = true;
    args.damage_threshold = 1.0;
    assert!(matches!(
        world.try_start_do_after(args, Bandage),
        DoAfterStart::Started(_)
    ));
    // Ниже порога — продолжается; тупой урон после брони 0,5 ниже порога 1.
    world.change_damage(dummy, &blunt(2.0), false, true, None);
    assert!(world.has_do_after::<Bandage>(dummy));
    world.change_damage(dummy, &blunt(9.0), false, true, None);
    assert!(!world.has_do_after::<Bandage>(dummy));
}
