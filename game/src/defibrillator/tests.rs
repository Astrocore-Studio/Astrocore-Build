use astrocore_ecs::GameWorldExt;

use super::*;
use crate::damage::Damageable;
use crate::fixed_point::FixedPoint2;
use crate::item_toggle::{is_activated, toggle};
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: damageType
  id: Asphyxiation
  name: damage-type-asphyxiation
- type: damageType
  id: Shock
  name: damage-type-shock
- type: statusEffect
  id: Electrocution
- type: entity
  id: Cell
  components:
  - type: Battery
    maxCharge: 250
    startingCharge: 250
  - type: PowerCell
- type: entity
  id: Defib
  components:
  - type: ItemToggle
  - type: UseDelay
  - type: Defibrillator
    zapHeal:
      types:
        Asphyxiation: -40
  - type: ToggleCellDraw
  - type: PowerCellDraw
    useCharge: 100
  - type: PowerCellSlot
    cellSlotId: cell_slot
  - type: ItemSlots
    slots:
      cell_slot:
        startingItem: Cell
- type: entity
  id: Patient
  components:
  - type: Damageable
  - type: Injurable
  - type: MobState
  - type: MobThresholds
    thresholds:
      0: Alive
      100: Critical
      200: Dead
  - type: StatusEffects
    allowed: [ Electrocution ]
"#;

fn setup() -> (World, Entity, Entity, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let defib = spawn_at(&mut world, "Defib", grid, 1, 1);
    let patient = spawn_at(&mut world, "Patient", grid, 2, 1);
    let medic = spawn_at(&mut world, "Patient", grid, 1, 2);
    (world, defib, patient, medic)
}

fn damage(world: &World, entity: Entity, kind: &str) -> f32 {
    world
        .get::<Damageable>(entity)
        .and_then(|d| d.damage.types.get(kind))
        .map_or(0.0, |v| v.float())
}

fn suffocate(world: &mut World, entity: Entity, amount: i32) {
    let damage = DamageSpecifier::new([("Asphyxiation", FixedPoint2::new(amount))]);
    world.change_damage(entity, &damage, true, false, None);
}

#[test]
fn zap_brings_back_the_dead_below_the_threshold() {
    let (mut world, defib, patient, medic) = setup();
    suffocate(&mut world, patient, 220);
    assert!(world.is_mob_dead(patient));
    // Выключенный не бьёт.
    assert!(!can_zap(&mut world, defib, None));
    assert!(toggle(&mut world, defib, None, false));
    assert!(can_zap(&mut world, defib, None));
    zap(&mut world, defib, patient, medic);
    // −40 удушья: 180 < 200 — крит; ток добавил 5.
    assert_eq!(damage(&world, patient, "Asphyxiation"), 180.0);
    assert_eq!(damage(&world, patient, "Shock"), 5.0);
    assert_eq!(world.mob_state(patient), Some(MobState::Critical));
    // Пауза разряда.
    assert!(!can_zap(&mut world, defib, None));
}

#[test]
fn last_charge_is_spent_without_a_zap_like_in_ss14() {
    let (mut world, defib, patient, medic) = setup();
    toggle(&mut world, defib, None, false);
    suffocate(&mut world, patient, 50);
    zap(&mut world, defib, patient, medic);
    // Живому — только лечение; осталось 150 из 250.
    assert_eq!(damage(&world, patient, "Asphyxiation"), 10.0);
    assert_eq!(world.mob_state(patient), Some(MobState::Alive));
    assert!(is_activated(&world, defib));
    crate::use_delay::cancel_delay(&mut world, defib, "defib-delay");
    world.run_tick();
    suffocate(&mut world, patient, 50);
    zap(&mut world, defib, patient, medic);
    // SS14 тратит заряд до повторной проверки `CanZap`: после траты 50 < 100 — разряда
    // нет, заряд потрачен.
    assert_eq!(damage(&world, patient, "Asphyxiation"), 60.0);
    let cell = crate::power_cell::battery_in_slot(&world, defib).unwrap();
    // Включённый ещё и тратит 1 Дж/с.
    assert!((crate::battery::charge(&world, cell) - 50.0).abs() < 0.1);
    assert!(!can_zap(&mut world, defib, None));
}

#[test]
fn dead_too_far_gone_stays_dead() {
    let (mut world, defib, patient, medic) = setup();
    toggle(&mut world, defib, None, false);
    suffocate(&mut world, patient, 300);
    zap(&mut world, defib, patient, medic);
    assert!(world.is_mob_dead(patient));
    assert_eq!(damage(&world, patient, "Asphyxiation"), 260.0);
}
