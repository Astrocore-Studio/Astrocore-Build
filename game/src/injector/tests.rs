use astrocore_ecs::GameWorldExt;
use astrocore_map::TransformWorldExt;

use super::*;
use crate::hands::HandsWorldExt;
use crate::solution::ReagentId;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: reagent
  id: Blood
- type: reagent
  id: Water
- type: reagent
  id: Venom
- type: damageType
  id: Poison
  name: damage-type-poison
- type: injectorMode
  abstract: true
  id: BaseInjectMode
  name: injector-component-inject-mode-name
  behavior: Inject
- type: injectorMode
  abstract: true
  id: BaseDrawMode
  name: injector-component-draw-mode-name
  behavior: Draw
  popupUserAttempt: injector-component-needle-drawing-user
  popupTargetAttempt: injector-component-needle-drawing-target
- type: injectorMode
  abstract: true
  id: BaseDynamicMode
  name: injector-component-dynamic-mode-name
  behavior: Dynamic
- type: injectorMode
  abstract: true
  id: BaseSyringeMode
  transferAmounts: [ 5, 10, 15 ]
- type: injectorMode
  parent: [ BaseSyringeMode, BaseInjectMode ]
  id: SyringeInjectMode
- type: injectorMode
  parent: [ BaseSyringeMode, BaseDrawMode ]
  id: SyringeDrawMode
- type: injectorMode
  abstract: true
  id: BaseHyposprayMode
  injectPopupTarget: injector-component-feel-prick-message
  injectOnUse: true
  delayPerVolume: 0
  mobTime: 0
  transferAmounts: [ 5 ]
- type: injectorMode
  parent: [ BaseHyposprayMode, BaseDynamicMode ]
  id: HyposprayDynamicMode
- type: entity
  id: Solution
  components:
  - type: Solution
- type: entity
  parent: Solution
  id: SolutionInjector
  components:
  - type: Solution
    id: injector
    solution:
      maxVol: 15
- type: entity
  parent: Solution
  id: SolutionBeaker
  components:
  - type: Solution
    id: beaker
    solution:
      maxVol: 50
- type: entity
  id: Syringe
  components:
  - type: Item
  - type: SolutionManager
    solutions: [ SolutionInjector ]
  - type: Injector
    activeModeProtoId: SyringeDrawMode
    allowedModes: [ SyringeDrawMode, SyringeInjectMode ]
- type: entity
  id: Hypospray
  components:
  - type: Item
  - type: SolutionManager
    solutions: [ SolutionInjector ]
  - type: Injector
    activeModeProtoId: HyposprayDynamicMode
    allowedModes: [ HyposprayDynamicMode ]
- type: entity
  id: Beaker
  components:
  - type: SolutionManager
    solutions: [ SolutionBeaker ]
  - type: DrawableSolution
    solution: beaker
  - type: RefillableSolution
    solution: beaker
- type: entity
  id: Human
  components:
  - type: ComplexInteraction
  - type: DoAfter
  - type: Hands
    startingHands:
      hand_right:
        location: Right
  - type: Damageable
  - type: Injurable
  - type: InjectableSolution
    solution: bloodstream
  - type: Bloodstream
  - type: Reactive
    reactions:
    - methods: [ Injection ]
      reagents: [ Venom ]
      effects:
      - !type:HealthChange
        damage:
          types:
            Poison: 1
"#;

struct Scene {
    world: World,
    user: Entity,
    patient: Entity,
    beaker: Entity,
}

fn n(value: i32) -> FixedPoint2 {
    FixedPoint2::new(value)
}

fn scene(injector_proto: &str) -> (Scene, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let user = spawn_at(&mut world, "Human", grid, 2, 2);
    let patient = spawn_at(&mut world, "Human", grid, 3, 2);
    let beaker = spawn_at(&mut world, "Beaker", grid, 2, 3);
    let injector = spawn_at(&mut world, injector_proto, grid, 2, 2);
    assert!(world.try_pickup(user, injector, None, true));
    world.run_tick();
    (
        Scene {
            world,
            user,
            patient,
            beaker,
        },
        injector,
    )
}

fn fill(world: &mut World, owner: Entity, name: &str, reagent: &str, amount: i32) {
    let soln = containers::try_get_solution(world, owner, name).unwrap();
    containers::try_add_reagent(world, soln, ReagentId::new(reagent), n(amount), None);
}

fn amount(world: &mut World, owner: Entity, name: &str, reagent: &str) -> FixedPoint2 {
    let soln = containers::try_get_solution(world, owner, name).unwrap();
    containers::solution(world, soln)
        .unwrap()
        .prototype_quantity(reagent)
}

fn click(world: &mut World, user: Entity, injector: Entity, target: Entity) -> bool {
    let mut event = AfterInteract {
        user,
        used: injector,
        target: Some(target),
        click: world.map_coordinates(target).unwrap(),
        can_reach: true,
        handled: false,
    };
    on_after_interact(world, injector, &mut event);
    event.handled
}

fn run_seconds(world: &mut World, seconds: f32) {
    let rate = f32::from(world.resource::<astrocore_ecs::GameTime>().tick_rate);
    for _ in 0..(seconds * rate).ceil() as u32 {
        world.run_tick();
    }
}

fn mode_of(world: &World, injector: Entity) -> String {
    world
        .get::<Injector>(injector)
        .unwrap()
        .active_mode_proto_id
        .clone()
}

#[test]
fn modes_load_with_inheritance() {
    let mut world = world_with(PROTOTYPES);
    let draw = mode(&mut world, "SyringeDrawMode").unwrap();
    assert_eq!(draw.behavior, InjectorBehavior::Draw);
    assert_eq!(draw.transfer_amounts, [n(5), n(10), n(15)]);
    assert_eq!(
        draw.popup_user_attempt,
        "injector-component-needle-drawing-user"
    );
    assert_eq!(draw.mob_time, 5.0);
    let hypo = mode(&mut world, "HyposprayDynamicMode").unwrap();
    assert!(hypo.inject_on_use);
    assert_eq!(hypo.mob_time, 0.0);
    assert_eq!(hypo.behavior, InjectorBehavior::Dynamic);
}

#[test]
fn syringe_draws_from_beaker_then_injects_into_a_body() {
    let (mut s, syringe) = scene("Syringe");
    fill(&mut s.world, s.beaker, "beaker", "Venom", 30);
    // Забор из ёмкости — сразу (у режима нет времени для ёмкостей).
    for _ in 0..3 {
        assert!(click(&mut s.world, s.user, syringe, s.beaker));
    }
    assert_eq!(amount(&mut s.world, syringe, "injector", "Venom"), n(15));
    assert_eq!(amount(&mut s.world, s.beaker, "beaker", "Venom"), n(15));
    // Полный шприц сам перешёл на впрыск.
    assert_eq!(mode_of(&s.world, syringe), "SyringeInjectMode");
    // В тело — с задержкой: 5 с + 0,1 с за единицу сверх 5 при 5 за раз — 5 с.
    assert!(click(&mut s.world, s.user, syringe, s.patient));
    run_seconds(&mut s.world, 4.5);
    assert_eq!(
        amount(&mut s.world, s.patient, "bloodstream", "Venom"),
        FixedPoint2::ZERO
    );
    run_seconds(&mut s.world, 1.0);
    assert_eq!(
        amount(&mut s.world, s.patient, "bloodstream", "Venom"),
        n(5)
    );
    // Реакция тела на впрыск.
    let poison = s
        .world
        .get::<crate::damage::Damageable>(s.patient)
        .unwrap()
        .damage
        .types
        .get("Poison")
        .copied();
    assert_eq!(poison, Some(n(5)));
}

#[test]
fn syringe_draws_blood_and_toggles_in_hand() {
    let (mut s, syringe) = scene("Syringe");
    let user = s.user;
    let patient = s.patient;
    // Себе — вдвое быстрее: 2,5 с.
    assert_eq!(
        mobs_do_after_time(&mut s.world, syringe, user, user).map(|t| t.0),
        Some(2.5)
    );
    assert!(click(&mut s.world, user, syringe, patient));
    run_seconds(&mut s.world, 5.1);
    assert_eq!(amount(&mut s.world, syringe, "injector", "Blood"), n(5));
    assert_eq!(crate::bloodstream::blood_volume(&s.world, patient), n(595));
    // В руке: на впрыск и обратно.
    let mut event = UseInHand {
        user,
        used: syringe,
        handled: false,
        apply_delay: true,
    };
    on_use_in_hand(&mut s.world, syringe, &mut event);
    assert!(event.handled && !event.apply_delay);
    assert_eq!(mode_of(&s.world, syringe), "SyringeInjectMode");
    toggle_mode(&mut s.world, syringe, user);
    assert_eq!(mode_of(&s.world, syringe), "SyringeDrawMode");
}

#[test]
fn empty_syringe_cannot_switch_to_inject() {
    let (mut s, syringe) = scene("Syringe");
    toggle_mode(&mut s.world, syringe, s.user);
    assert_eq!(mode_of(&s.world, syringe), "SyringeDrawMode");
    // Из пустой ёмкости набрать нечего.
    assert!(!click(&mut s.world, s.user, syringe, s.beaker));
}

#[test]
fn hypospray_draws_from_containers_and_injects_instantly() {
    let (mut s, hypo) = scene("Hypospray");
    fill(&mut s.world, s.beaker, "beaker", "Water", 20);
    assert!(click(&mut s.world, s.user, hypo, s.beaker));
    assert_eq!(amount(&mut s.world, hypo, "injector", "Water"), n(5));
    // Динамический режим не переключается сам.
    assert_eq!(mode_of(&s.world, hypo), "HyposprayDynamicMode");
    // В тело — мгновенно.
    assert!(click(&mut s.world, s.user, hypo, s.patient));
    assert_eq!(
        amount(&mut s.world, s.patient, "bloodstream", "Water"),
        n(5)
    );
    assert_eq!(
        amount(&mut s.world, hypo, "injector", "Water"),
        FixedPoint2::ZERO
    );
    // Себе — использованием в руке.
    assert!(click(&mut s.world, s.user, hypo, s.beaker));
    let user = s.user;
    let mut event = UseInHand {
        user,
        used: hypo,
        handled: false,
        apply_delay: true,
    };
    on_use_in_hand(&mut s.world, hypo, &mut event);
    assert_eq!(amount(&mut s.world, user, "bloodstream", "Water"), n(5));
}

#[test]
fn amount_verbs_and_ignore_mobs() {
    let (mut s, syringe) = scene("Syringe");
    let mut event = GetVerbs {
        kind: VerbKind::Alternative,
        user: s.user,
        target: syringe,
        using: Some(syringe),
        can_interact: true,
        can_complex_interact: true,
        can_access: true,
        verbs: Vec::new(),
    };
    add_verbs(&mut s.world, syringe, &mut event);
    // Переключатель и три количества; смена режима — только у впрыскивающих в руке.
    assert_eq!(event.verbs.len(), 4);
    (event.verbs[0].act.clone().unwrap())(&mut s.world);
    assert_eq!(
        s.world
            .get::<Injector>(syringe)
            .unwrap()
            .current_transfer_amount,
        Some(n(15))
    );
    s.world.get_mut::<Injector>(syringe).unwrap().ignore_mobs = true;
    assert!(!click(&mut s.world, s.user, syringe, s.patient));
}
