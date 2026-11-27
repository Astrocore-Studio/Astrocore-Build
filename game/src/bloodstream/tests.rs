use super::*;
use crate::alerts::Alerts;
use crate::damage::Damageable;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: damageType
  id: Bloodloss
  name: damage-type-bloodloss
- type: damageType
  id: Slash
  name: damage-type-slash
- type: damageType
  id: Heat
  name: damage-type-heat
- type: damageModifierSet
  id: BloodlossHuman
  coefficients:
    Bloodloss: 0.0
    Heat: -0.5
    Slash: 0.25
- type: alert
  id: Bleed
  icons:
  - sprite: /Textures/Interface/Alerts/bleed.rsi
    state: bleed0
  maxSeverity: 10
- type: entity
  id: Body
  components:
  - type: Damageable
  - type: Injurable
  - type: Alerts
  - type: Bloodstream
    bloodlossDamage:
      types:
        Bloodloss: 0.5
    bloodlossHealDamage:
      types:
        Bloodloss: -1
- type: entity
  id: Gingerbread
  components:
  - type: Damageable
  - type: Injurable
  - type: Bloodstream
    bloodReferenceSolution:
      reagents:
      - ReagentId: Sugar
        Quantity: 200
      - ReagentId: Butter
        Quantity: 400
- type: entity
  id: Gauze
  components:
  - type: Healing
    damage:
      types:
        Slash: -5
    bloodlossModifier: -10
"#;

fn body(world: &mut World, prototype: &str) -> Entity {
    let grid = station(world);
    spawn_at(world, prototype, grid, 3, 3)
}

/// Пройти одно обновление кровотока (3 с).
fn one_update(world: &mut World) {
    let rate = usize::from(world.resource::<GameTime>().tick_rate);
    for _ in 0..=rate * 3 {
        world.run_tick();
    }
}

fn blood(world: &World, entity: Entity) -> Bloodstream {
    world.get::<Bloodstream>(entity).unwrap().clone()
}

fn set(world: &mut World, entity: Entity, change: impl FnOnce(&mut Bloodstream)) {
    change(&mut world.get_mut::<Bloodstream>(entity).unwrap());
}

fn damage(world: &World, entity: Entity, kind: &str) -> f32 {
    world
        .get::<Damageable>(entity)
        .and_then(|d| d.damage.types.get(kind))
        .map_or(0.0, |v| v.float())
}

fn hurt(world: &mut World, entity: Entity, kind: &str, amount: i32) {
    let mut damage = DamageSpecifier::default();
    damage.types.insert(kind.into(), FixedPoint2::new(amount));
    world.change_damage(entity, &damage, true, false, None);
}

fn bleed_severity(world: &World, entity: Entity) -> Option<Option<i16>> {
    world
        .get::<Alerts>(entity)
        .and_then(|a| a.alerts.values().find(|s| s.alert_type == "Bleed").cloned())
        .map(|s| s.severity)
}

#[test]
fn body_starts_with_reference_blood() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    let bloodstream = blood(&world, entity);
    assert_eq!(bloodstream.blood.get("Blood"), Some(&FixedPoint2::new(600)));
    assert_eq!(bloodstream.blood_level(), 1.0);
}

#[test]
fn slashes_open_bleeding_with_an_alert() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    // 20 режущего × 0,25.
    hurt(&mut world, entity, "Slash", 20);
    assert_eq!(blood(&world, entity).bleed_amount, 5.0);
    assert_eq!(bleed_severity(&world, entity), Some(Some(5)));
    // Сверх наибольшего не течёт.
    hurt(&mut world, entity, "Slash", 40);
    assert_eq!(blood(&world, entity).bleed_amount, 10.0);
    assert_eq!(bleed_severity(&world, entity), Some(Some(10)));
}

#[test]
fn bleeding_drains_blood_and_slows_down() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    set(&mut world, entity, |b| b.bleed_amount = 2.0);
    one_update(&mut world);
    // Крови полная норма — восстанавливать нечего; вытекло 2, кровотечение слабеет на 0,33.
    let bloodstream = blood(&world, entity);
    assert_eq!(bloodstream.blood_volume(), FixedPoint2::new(598));
    assert!((bloodstream.bleed_amount - 1.67).abs() < 1e-4);
    one_update(&mut world);
    // +1 восстановления, −1,67.
    let bloodstream = blood(&world, entity);
    assert_eq!(bloodstream.blood_volume(), FixedPoint2::from_f32(597.33));
    assert!((bloodstream.bleed_amount - 1.34).abs() < 1e-4);
}

#[test]
fn low_blood_hurts_and_normal_blood_heals() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    set(&mut world, entity, |b| {
        b.blood.insert("Blood".into(), FixedPoint2::new(300));
    });
    one_update(&mut world);
    // 301 из 600: урон 0,5 / (0,1 + 0,50166).
    let level = 301.0 / 600.0;
    let expected = 0.5 / (0.1 + level);
    assert!((damage(&world, entity, "Bloodloss") - expected).abs() < 0.02);
    assert_eq!(blood(&world, entity).blood_volume(), FixedPoint2::new(301));

    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    hurt(&mut world, entity, "Bloodloss", 3);
    one_update(&mut world);
    assert_eq!(damage(&world, entity, "Bloodloss"), 2.0);
}

#[test]
fn burns_cauterize_wounds() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    hurt(&mut world, entity, "Slash", 12);
    assert_eq!(blood(&world, entity).bleed_amount, 3.0);
    // 10 ожога × −0,5.
    hurt(&mut world, entity, "Heat", 10);
    assert_eq!(blood(&world, entity).bleed_amount, 0.0);
    assert_eq!(bleed_severity(&world, entity), None);
}

#[test]
fn rejuvenate_restores_blood_and_stops_bleeding() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    hurt(&mut world, entity, "Slash", 20);
    set(&mut world, entity, |b| {
        b.blood.insert("Blood".into(), FixedPoint2::new(100));
    });
    world.rejuvenate(entity);
    let bloodstream = blood(&world, entity);
    assert_eq!(bloodstream.bleed_amount, 0.0);
    assert_eq!(bloodstream.blood_volume(), FixedPoint2::new(600));
    assert_eq!(bleed_severity(&world, entity), None);
}

#[test]
fn mixed_blood_bleeds_by_composition() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Gingerbread");
    set(&mut world, entity, |b| {
        b.bleed_out(FixedPoint2::new(6));
    });
    let bloodstream = blood(&world, entity);
    assert_eq!(bloodstream.blood.get("Sugar"), Some(&FixedPoint2::new(198)));
    assert_eq!(
        bloodstream.blood.get("Butter"),
        Some(&FixedPoint2::new(396))
    );
    // Вытекшее больше порога лужи — пропадает: луж пока нет.
    assert!(bloodstream.leaked.is_empty());
    // Пригодной крови — по меньшему из реагентов.
    set(&mut world, entity, |b| {
        b.blood.insert("Sugar".into(), FixedPoint2::new(100));
    });
    assert_eq!(blood(&world, entity).blood_level(), 0.5);
    // Подводится к норме не больше чем на долю `amount` по каждому реагенту.
    set(&mut world, entity, |b| {
        b.regulate(FixedPoint2::new(6), 1.0);
    });
    let bloodstream = blood(&world, entity);
    assert_eq!(bloodstream.blood.get("Sugar"), Some(&FixedPoint2::new(102)));
    assert_eq!(
        bloodstream.blood.get("Butter"),
        Some(&FixedPoint2::new(400))
    );
}

#[test]
fn health_examine_tells_bleeding_and_pallor() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    assert!(health_examine_lines(&world, entity).is_empty());
    set(&mut world, entity, |b| b.bleed_amount = 8.0);
    assert_eq!(
        health_examine_lines(&world, entity),
        vec!["bloodstream-component-massive-bleeding".to_owned()]
    );
    set(&mut world, entity, |b| {
        b.bleed_amount = 1.0;
        b.blood.insert("Blood".into(), FixedPoint2::new(500));
    });
    assert_eq!(
        health_examine_lines(&world, entity),
        vec![
            "bloodstream-component-slight-bleeding".to_owned(),
            "bloodstream-component-looks-pale".to_owned()
        ]
    );
}

#[test]
fn gauze_treats_bleeding_even_without_wounds() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    let gauze = body(&mut world, "Gauze");
    let healing = world.get::<crate::healing::Healing>(gauze).unwrap().clone();
    assert!(!crate::healing::has_damage(&world, &healing, entity));
    set(&mut world, entity, |b| b.bleed_amount = 2.0);
    assert!(crate::healing::has_damage(&world, &healing, entity));
}
