use super::*;
use crate::alerts::Alerts;
use crate::damage::Damageable;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: reagent
  id: Blood
- type: reagent
  id: Sugar
- type: reagent
  id: Butter
- type: reagent
  id: Toxin
- type: reagent
  id: Charcoal
- type: reaction
  id: ToxinFoam
  reactants:
    Toxin:
      amount: 1
    Charcoal:
      amount: 1
  effects:
  - !type:AreaReactionEffect
    duration: 10
  products:
    Sugar: 1
- type: entity
  id: Solution
  components:
  - type: Solution
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

fn blood_of(world: &World, entity: Entity) -> Solution {
    let soln = blood_solution(world, entity).unwrap();
    containers::solution(world, soln).unwrap().clone()
}

fn quantity(world: &World, entity: Entity, reagent: &str) -> FixedPoint2 {
    blood_of(world, entity).prototype_quantity(reagent)
}

/// Задать количество реагента крови напрямую.
fn set_blood(world: &mut World, entity: Entity, reagent: &str, amount: FixedPoint2) {
    let soln = blood_solution(world, entity).unwrap();
    let mut component = world.get_mut::<SolutionComponent>(soln).unwrap();
    let have = component.solution.prototype_quantity(reagent);
    let id = component
        .solution
        .contents
        .iter()
        .find(|(id, _)| id.prototype == reagent)
        .map_or_else(|| ReagentId::new(reagent), |(id, _)| id.clone());
    component.solution.remove_reagent(&id, have, true);
    component.solution.add_reagent(id, amount);
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
    assert_eq!(quantity(&world, entity, "Blood"), FixedPoint2::new(600));
    assert_eq!(blood_level(&world, entity), 1.0);
    // Вместимость — 1,2 нормы; у крови — неизвестная ДНК.
    let solution = blood_of(&world, entity);
    assert_eq!(solution.max_volume, FixedPoint2::new(720));
    assert_eq!(
        solution.contents[0].0.data,
        [ReagentData::Dna {
            dna: "forensics-dna-unknown".into()
        }]
    );
    let temporary = named_solution(&world, entity, BLOOD_TEMPORARY_SOLUTION).unwrap();
    assert_eq!(
        containers::solution(&world, temporary).unwrap().max_volume,
        FixedPoint2::new(4)
    );
    assert!(named_solution(&world, entity, METABOLITES_SOLUTION).is_some());
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
    assert_eq!(blood_volume(&world, entity), FixedPoint2::new(598));
    assert!((blood(&world, entity).bleed_amount - 1.67).abs() < 1e-4);
    one_update(&mut world);
    // +1 восстановления, −1,67.
    assert_eq!(blood_volume(&world, entity), FixedPoint2::from_f32(597.33));
    assert!((blood(&world, entity).bleed_amount - 1.34).abs() < 1e-4);
}

#[test]
fn low_blood_hurts_and_normal_blood_heals() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    set_blood(&mut world, entity, "Blood", FixedPoint2::new(300));
    one_update(&mut world);
    // 301 из 600: урон 0,5 / (0,1 + 0,50166).
    let level = 301.0 / 600.0;
    let expected = 0.5 / (0.1 + level);
    assert!((damage(&world, entity, "Bloodloss") - expected).abs() < 0.02);
    assert_eq!(blood_volume(&world, entity), FixedPoint2::new(301));

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
    set_blood(&mut world, entity, "Blood", FixedPoint2::new(100));
    world.rejuvenate(entity);
    assert_eq!(blood(&world, entity).bleed_amount, 0.0);
    assert_eq!(blood_volume(&world, entity), FixedPoint2::new(600));
    assert_eq!(bleed_severity(&world, entity), None);
}

#[test]
fn mixed_blood_bleeds_by_composition() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Gingerbread");
    bleed_out(&mut world, entity, FixedPoint2::new(6));
    assert_eq!(quantity(&world, entity, "Sugar"), FixedPoint2::new(198));
    assert_eq!(quantity(&world, entity, "Butter"), FixedPoint2::new(396));
    // Вытекшее больше порога лужи — разливается: луж пока нет, пропадает.
    let temporary = named_solution(&world, entity, BLOOD_TEMPORARY_SOLUTION).unwrap();
    assert!(containers::solution(&world, temporary).unwrap().is_empty());
    // Пригодной крови — по меньшему из реагентов.
    set_blood(&mut world, entity, "Sugar", FixedPoint2::new(100));
    assert_eq!(blood_level(&world, entity), 0.5);
    // Подводится к норме не больше чем на долю `amount` по каждому реагенту.
    regulate_blood_level(&mut world, entity, FixedPoint2::new(6));
    assert_eq!(quantity(&world, entity, "Sugar"), FixedPoint2::new(102));
    assert_eq!(quantity(&world, entity, "Butter"), FixedPoint2::new(400));
}

#[test]
fn small_bleeds_collect_in_the_temporary_solution() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    bleed_out(&mut world, entity, FixedPoint2::from_cents(50));
    let temporary = named_solution(&world, entity, BLOOD_TEMPORARY_SOLUTION).unwrap();
    let leaked = containers::solution(&world, temporary).unwrap();
    assert_eq!(
        leaked.prototype_quantity("Blood"),
        FixedPoint2::from_cents(50)
    );
}

#[test]
fn chemicals_in_blood_are_flushed_but_blood_stays() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    let mut injected = Solution::default();
    injected.add_reagent(ReagentId::new("Toxin"), FixedPoint2::new(10));
    injected.add_reagent(ReagentId::new("Charcoal"), FixedPoint2::new(10));
    assert!(try_add_to_bloodstream(&mut world, entity, &injected));
    let flushed = flush_chemicals(&mut world, entity, FixedPoint2::new(3), Some("Charcoal"));
    assert_eq!(
        flushed.unwrap().prototype_quantity("Toxin"),
        FixedPoint2::new(3)
    );
    assert_eq!(quantity(&world, entity, "Toxin"), FixedPoint2::new(7));
    assert_eq!(quantity(&world, entity, "Charcoal"), FixedPoint2::new(10));
    assert_eq!(quantity(&world, entity, "Blood"), FixedPoint2::new(600));
    // Через эффект: очистка с масштабом.
    let node = astrocore_data::parse_yaml(
        "- !type:CleanBloodstream\n  cleanseRate: 2\n- !type:ModifyBleed\n  amount: 1.5\n- !type:ModifyBloodLevel\n  amount: -5",
    )
    .unwrap();
    let effects = crate::entity_effects::parse_effects(&mut world, Some(&node));
    assert_eq!(effects.len(), 3);
    crate::entity_effects::apply_effects(&mut world, entity, &effects, 1.0, None);
    assert_eq!(quantity(&world, entity, "Toxin"), FixedPoint2::new(5));
    assert_eq!(quantity(&world, entity, "Charcoal"), FixedPoint2::new(8));
    assert_eq!(blood(&world, entity).bleed_amount, 1.5);
    assert_eq!(quantity(&world, entity, "Blood"), FixedPoint2::new(595));
}

#[test]
fn blood_reagents_are_kept_from_metabolism() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Gingerbread");
    let mut exclusion = MetabolismExclusion::default();
    world.raise(entity, &mut exclusion);
    let names: Vec<&str> = exclusion
        .reagents
        .iter()
        .map(|id| id.prototype.as_str())
        .collect();
    assert_eq!(names, ["Sugar", "Butter"]);
    // С ДНК, как кровь в растворе.
    let blood = blood_of(&world, entity);
    assert!(
        blood
            .contents
            .iter()
            .any(|(id, _)| *id == exclusion.reagents[0])
    );
}

#[test]
fn blood_can_change_composition() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    bleed_out(&mut world, entity, FixedPoint2::new(100));
    let sugar = Solution::from_reagents([(ReagentId::new("Sugar"), FixedPoint2::new(1))]);
    change_blood_reagents(&mut world, entity, &sugar);
    assert_eq!(quantity(&world, entity, "Blood"), FixedPoint2::ZERO);
    // Как в SS14: доля 500/600 считается в сотых (0,83), поэтому не 500, а 497,99.
    assert_eq!(
        quantity(&world, entity, "Sugar"),
        FixedPoint2::from_cents(49799)
    );
    assert_eq!(
        blood(&world, entity).blood_reference_solution.volume,
        FixedPoint2::new(600)
    );
    assert!((blood_level(&world, entity) - 0.82).abs() < 1e-3);
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
    set(&mut world, entity, |b| b.bleed_amount = 1.0);
    set_blood(&mut world, entity, "Blood", FixedPoint2::new(500));
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

#[test]
fn foam_and_spawning_reactions_do_not_run_in_blood() {
    let mut world = world_with(PROTOTYPES);
    let entity = body(&mut world, "Body");
    let mut injected = Solution::default();
    injected.add_reagent(ReagentId::new("Toxin"), FixedPoint2::new(2));
    injected.add_reagent(ReagentId::new("Charcoal"), FixedPoint2::new(2));
    assert!(try_add_to_bloodstream(&mut world, entity, &injected));
    assert_eq!(quantity(&world, entity, "Toxin"), FixedPoint2::new(2));
    assert_eq!(quantity(&world, entity, "Sugar"), FixedPoint2::ZERO);
    // В другом растворе того же тела реакция идёт.
    let metabolites = named_solution(&world, entity, METABOLITES_SOLUTION).unwrap();
    containers::add_solution(&mut world, metabolites, &injected);
    let other = containers::solution(&world, metabolites).unwrap();
    assert_eq!(other.prototype_quantity("Sugar"), FixedPoint2::new(2));
}
