use astrocore_ecs::GameWorldExt;

use super::*;
use crate::actions::{Action, ActionsWorldExt};
use crate::atmos::ensure_grid_atmosphere;
use crate::damage::Damageable;
use crate::gas_tank::{GasTank, tank_air};
use crate::inventory::EquipOptions;
use crate::testing::{spawn_at, station, world_with};

pub(crate) const PROTOTYPES: &str = r#"
- type: inventoryTemplate
  id: human
  slots:
  - name: mask
    slotFlags: MASK
    uiWindowPos: 1,1
    strippingWindowPos: 1,1
    displayName: Mask
  - name: back
    slotFlags: BACK
    uiWindowPos: 3,1
    strippingWindowPos: 0,3
    displayName: Back
- type: itemSize
  id: Normal
  weight: 4
- type: damageType
  id: Asphyxiation
  name: damage-type-asphyxiation
- type: gas
  id: Oxygen
  name: gas-oxygen
  reagent: Oxygen
- type: reagent
  id: Oxygen
  metabolisms:
    Respiration:
      effects:
      - !type:Oxygenate
        conditions:
        - !type:MetabolizerTypeCondition
          type: [ Human ]
- type: entity
  id: ActionToggleInternals
  components:
  - type: Action
  - type: InstantAction
    event: !type:ToggleActionEvent
- type: entity
  id: TestLungs
  components:
  - type: Organ
    category: Lungs
  - type: Lung
  - type: Metabolizer
    stages: [ Respiration ]
    metabolizerTypes: [ Human ]
- type: entity
  id: Spaceman
  components:
  - type: ComplexInteraction
  - type: DoAfter
  - type: Inventory
  - type: Actions
  - type: Hands
    startingHands:
      hand_right:
        location: Right
  - type: Body
  - type: InitialBody
    organs:
      Lungs: TestLungs
  - type: Damageable
  - type: Injurable
  - type: Internals
  - type: Respirator
    damage:
      types:
        Asphyxiation: 1.0
    damageRecovery:
      types:
        Asphyxiation: -1.0
- type: entity
  id: BreathMask
  components:
  - type: Item
    size: Normal
  - type: Clothing
    slots: [mask]
  - type: BreathMask
- type: entity
  id: OxygenTank
  components:
  - type: Item
    size: Normal
  - type: Clothing
    slots: [back]
  - type: AtmosDevice
    requireAnchored: false
    joinSystem: true
  - type: GasTank
    releasePressure: 21.3
    air:
      volume: 5
      moles:
        Oxygen: 2.051379050
      temperature: 293.15
"#;

pub(crate) fn ticks(world: &mut World, count: usize) {
    for _ in 0..count {
        world.run_tick();
    }
}

fn equip(world: &mut World, wearer: Entity, item: Entity, slot: &str) {
    let options = EquipOptions {
        silent: true,
        force: true,
        delay: false,
    };
    world.try_equip(wearer, wearer, item, slot, options);
}

/// Космонавт в вакууме с маской и баллоном на спине.
fn spaceman(mask: bool) -> (World, Entity, Entity) {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    ensure_grid_atmosphere(&mut world, grid);
    ticks(&mut world, 10);
    let man = spawn_at(&mut world, "Spaceman", grid, 8, 8);
    let tank = spawn_at(&mut world, "OxygenTank", grid, 8, 8);
    equip(&mut world, man, tank, "back");
    if mask {
        let breath = spawn_at(&mut world, "BreathMask", grid, 8, 8);
        equip(&mut world, man, breath, "mask");
    }
    (world, man, tank)
}

fn asphyxiation(world: &World, entity: Entity) -> f32 {
    world
        .get::<Damageable>(entity)
        .and_then(|d| d.damage.types.get("Asphyxiation"))
        .map_or(0.0, |v| v.float())
}

#[test]
fn tank_keeps_spaceman_breathing() {
    let (mut world, man, tank) = spaceman(true);
    assert_eq!(world.get::<Internals>(man).unwrap().breath_tools.len(), 1);
    assert!(toggle_internals(
        &mut world,
        man,
        man,
        false,
        ToggleMode::Toggle
    ));
    assert!(are_internals_working(&world, man));
    assert_eq!(world.get::<GasTank>(tank).unwrap().user, Some(man));
    let before = tank_air(&world, tank).unwrap().total_moles();
    for _ in 0..12 {
        ticks(&mut world, 30);
    }
    assert_eq!(asphyxiation(&world, man), 0.0);
    // Три вдоха по 0,5 л при 21,3 кПа: ≈ 0,00437 моля каждый.
    let spent = before - tank_air(&world, tank).unwrap().total_moles();
    assert!((0.012..0.014).contains(&spent), "{spent}");
}

#[test]
fn without_mask_tank_does_not_connect() {
    let (mut world, man, tank) = spaceman(false);
    assert!(!toggle_internals(
        &mut world,
        man,
        man,
        false,
        ToggleMode::Toggle
    ));
    assert!(world.get::<GasTank>(tank).unwrap().user.is_none());
    ticks(&mut world, 30 * 12);
    assert!(asphyxiation(&world, man) > 0.0);
}

#[test]
fn removing_mask_disconnects_tank() {
    let (mut world, man, tank) = spaceman(true);
    toggle_internals(&mut world, man, man, false, ToggleMode::On);
    // В пределах секунды после подключения баллон не отключается (задержка `gasTank`).
    ticks(&mut world, 35);
    let options = EquipOptions {
        silent: true,
        force: true,
        delay: false,
    };
    world.try_unequip(man, man, "mask", options);
    assert!(!are_internals_working(&world, man));
    assert!(world.get::<GasTank>(tank).unwrap().user.is_none());
}

#[test]
fn tank_grants_toggle_action() {
    let (mut world, man, tank) = spaceman(true);
    let action = world
        .get::<GasTank>(tank)
        .unwrap()
        .toggle_action_entity
        .unwrap();
    assert!(world.actions_of(man).contains(&action));
    world.perform_action(man, action, None, None);
    assert!(world.get::<GasTank>(tank).unwrap().is_connected());
    assert!(world.get::<Action>(action).unwrap().toggled);
    // Задержка 1 с: второе нажатие сразу не отключает.
    world.perform_action(man, action, None, None);
    assert!(world.get::<GasTank>(tank).unwrap().is_connected());
    ticks(&mut world, 35);
    world.perform_action(man, action, None, None);
    assert!(!world.get::<GasTank>(tank).unwrap().is_connected());
    // Снял баллон — действие ушло с панели.
    let options = EquipOptions {
        silent: true,
        force: true,
        delay: false,
    };
    world.try_unequip(man, man, "back", options);
    assert!(!world.actions_of(man).contains(&action));
}

#[test]
fn helping_someone_else_takes_time() {
    let (mut world, man, tank) = spaceman(true);
    let grid = world
        .get::<astrocore_map::Transform>(man)
        .unwrap()
        .grid()
        .unwrap();
    let helper = spawn_at(&mut world, "Spaceman", grid, 8, 8);
    assert!(toggle_internals(
        &mut world,
        man,
        helper,
        false,
        ToggleMode::On
    ));
    assert!(world.get::<GasTank>(tank).unwrap().user.is_none());
    ticks(&mut world, 30 * 3 + 5);
    assert_eq!(world.get::<GasTank>(tank).unwrap().user, Some(man));
}
