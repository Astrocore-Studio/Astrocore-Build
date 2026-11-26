use astrocore_ecs::GameWorldExt;

use super::*;
use crate::damage::DamageableWorldExt;
use crate::doors::is_bolted;
use crate::hands::{Hand, HandsWorldExt};
use crate::interaction::InteractionWorldExt;
use crate::testing::{spawn_at, station, world_with};

pub(crate) const PROTOTYPES: &str = r#"
- type: damageType
  id: Shock
  name: damage-type-shock
- type: statusEffect
  id: Electrocution
- type: wireLayout
  id: TestAirlock
  wires:
  - !type:PowerWireAction
  - !type:PowerWireAction
    pulseTimeout: 15
  - !type:LogWireAction
  - !type:DoorBoltWireAction
  - !type:DoorBoltLightWireAction
  - !type:DoorTimingWireAction
  - !type:DoorSafetyWireAction
  - !type:AiInteractWireAction
- type: wireLayout
  parent: TestAirlock
  id: TestAirlockEngineering
- type: entity
  id: WiredAirlock
  components:
  - type: Appearance
  - type: Door
  - type: DoorBolt
  - type: Airlock
  - type: ApcPowerReceiver
    needsPower: false
  - type: AccessReader
  - type: Electrified
    enabled: false
    usesApcPower: true
    onActivateInWorld: true
  - type: WiresPanel
  - type: Wires
    layoutId: TestAirlock
- type: entity
  id: OtherAirlock
  parent: WiredAirlock
  components:
  - type: Wires
    layoutId: TestAirlockEngineering
- type: entity
  id: Engineer
  components:
  - type: Hands
  - type: ComplexInteraction
  - type: DoAfter
  - type: Damageable
  - type: Injurable
  - type: StatusEffects
    allowed: [ Electrocution ]
  - type: Physics
    bodyType: KinematicController
- type: entity
  id: Screwdriver
  components:
  - type: Item
  - type: Tool
    qualities: [ Screwing ]
- type: entity
  id: Wirecutter
  components:
  - type: Item
  - type: Tool
    qualities: [ Cutting ]
- type: entity
  id: Multitool
  components:
  - type: Item
  - type: Tool
    qualities: [ Pulsing ]
"#;

struct Scene {
    world: World,
    door: Entity,
    user: Entity,
}

fn scene() -> Scene {
    let mut world = world_with(PROTOTYPES);
    let grid = station(&mut world);
    let door = spawn_at(&mut world, "WiredAirlock", grid, 2, 1);
    let user = spawn_at(&mut world, "Engineer", grid, 1, 1);
    world.add_hand(user, "right", Hand::default());
    world.run_tick();
    Scene { world, door, user }
}

impl Scene {
    fn hold(&mut self, prototype: &str) -> Entity {
        if let Some(old) = self.world.active_item(self.user) {
            astrocore_ecs::LifecycleWorldExt::delete_entity(&mut self.world, old);
        }
        let grid = self
            .world
            .get::<astrocore_map::Transform>(self.door)
            .unwrap()
            .grid()
            .unwrap();
        let tool = spawn_at(&mut self.world, prototype, grid, 1, 1);
        assert!(self.world.try_pickup(self.user, tool, None, false));
        tool
    }

    fn wire_of(&self, kind: &str) -> usize {
        self.world
            .get::<Wires>(self.door)
            .unwrap()
            .wires
            .iter()
            .find(|w| w.action.as_ref().is_some_and(|a| a.kind() == kind))
            .map(|w| w.id)
            .unwrap()
    }

    fn act(&mut self, tool: Entity, kind: &str, action: WiresAction) {
        let id = self.wire_of(kind);
        try_do_wire_action(&mut self.world, self.door, self.user, tool, id, action);
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * 30.0).ceil() as u32 {
            self.world.run_tick();
        }
    }
}

#[test]
fn screwdriver_opens_the_panel() {
    let mut scene = scene();
    let screwdriver = scene.hold("Screwdriver");
    let (door, user) = (scene.door, scene.user);
    let at = astrocore_map::TransformWorldExt::map_coordinates(&scene.world, door).unwrap();
    assert!(scene.world.interact_using(user, screwdriver, door, at));
    assert!(!is_panel_open(&mut scene.world, door, None));
    scene.run(1.2);
    assert!(is_panel_open(&mut scene.world, door, None));
    assert!(matches!(
        scene.world.appearance(door, PANEL_STATE),
        Some(astrocore_net::AppearanceValue::Bool(true))
    ));
}

#[test]
fn layout_is_the_same_for_every_door_of_a_kind() {
    let mut scene = scene();
    let grid = scene
        .world
        .get::<astrocore_map::Transform>(scene.door)
        .unwrap()
        .grid()
        .unwrap();
    let second = spawn_at(&mut scene.world, "WiredAirlock", grid, 5, 5);
    let wires = |world: &World, door: Entity| -> Vec<(WireColor, usize, Option<String>)> {
        world
            .get::<Wires>(door)
            .unwrap()
            .wires
            .iter()
            .map(|w| {
                (
                    w.color,
                    w.letter,
                    w.action.as_ref().map(|a| a.kind().to_owned()),
                )
            })
            .collect()
    };
    let first = wires(&scene.world, scene.door);
    assert_eq!(first.len(), 8);
    assert_eq!(first, wires(&scene.world, second));
    // Наследник раскладки получает те же провода, но свою перестановку.
    let other = spawn_at(&mut scene.world, "OtherAirlock", grid, 7, 7);
    assert_eq!(wires(&scene.world, other).len(), 8);
    let serial = scene
        .world
        .get::<Wires>(scene.door)
        .unwrap()
        .serial_number
        .clone()
        .unwrap();
    assert_eq!(serial.chars().nth(4), Some('-'));
}

#[test]
fn bolt_wire_drops_bolts_and_pulse_toggles_them() {
    let mut scene = scene();
    let cutter = scene.hold("Wirecutter");
    scene.act(cutter, "DoorBolt", WiresAction::Cut);
    assert!(is_bolted(&scene.world, scene.door));
    scene.act(cutter, "DoorBolt", WiresAction::Mend);
    let multitool = scene.hold("Multitool");
    scene.act(multitool, "DoorBolt", WiresAction::Pulse);
    assert!(!is_bolted(&scene.world, scene.door));
    scene.act(multitool, "DoorBolt", WiresAction::Pulse);
    assert!(is_bolted(&scene.world, scene.door));
}

#[test]
fn wrong_tool_does_nothing() {
    let mut scene = scene();
    let multitool = scene.hold("Multitool");
    scene.act(multitool, "DoorBolt", WiresAction::Cut);
    let id = scene.wire_of("DoorBolt");
    assert!(!scene.world.get::<Wires>(scene.door).unwrap().wires[id].is_cut);
}

#[test]
fn safety_and_timing_wires_change_the_airlock() {
    let mut scene = scene();
    let multitool = scene.hold("Multitool");
    scene.act(multitool, "DoorSafety", WiresAction::Pulse);
    assert!(!scene.world.get::<Airlock>(scene.door).unwrap().safety);
    scene.act(multitool, "DoorTiming", WiresAction::Pulse);
    assert_eq!(
        scene
            .world
            .get::<Airlock>(scene.door)
            .unwrap()
            .auto_close_delay_modifier,
        0.5
    );
    // Через 30 секунд импульс проходит.
    scene.run(30.5);
    assert!(scene.world.get::<Airlock>(scene.door).unwrap().safety);
    assert_eq!(
        scene
            .world
            .get::<Airlock>(scene.door)
            .unwrap()
            .auto_close_delay_modifier,
        1.0
    );

    let cutter = scene.hold("Wirecutter");
    scene.act(cutter, "DoorTiming", WiresAction::Cut);
    assert_eq!(
        scene
            .world
            .get::<Airlock>(scene.door)
            .unwrap()
            .auto_close_delay_modifier,
        0.01
    );
}

#[test]
fn cutting_a_power_wire_shocks_without_insulation() {
    let mut scene = scene();
    let cutter = scene.hold("Wirecutter");
    scene.act(cutter, "Power", WiresAction::Cut);
    let id = scene.wire_of("Power");
    // Ударило током: провод цел, на теле ожог.
    assert!(!scene.world.get::<Wires>(scene.door).unwrap().wires[id].is_cut);
    assert!(scene.world.total_damage(scene.user).float() > 0.0);

    // С изоляцией провода режутся; без обоих проводов шлюз обесточен.
    scene
        .world
        .entity_mut(scene.user)
        .insert(crate::electrocution::Insulated { coefficient: 0.0 });
    scene.act(cutter, "Power", WiresAction::Cut);
    assert!(scene.world.get::<Wires>(scene.door).unwrap().wires[id].is_cut);
    let second = scene
        .world
        .get::<Wires>(scene.door)
        .unwrap()
        .wires
        .iter()
        .find(|w| w.id != id && w.action.as_ref().is_some_and(|a| a.kind() == "Power"))
        .unwrap()
        .id;
    try_do_wire_action(
        &mut scene.world,
        scene.door,
        scene.user,
        cutter,
        second,
        WiresAction::Cut,
    );
    scene.run(0.2);
    assert!(!is_powered(&scene.world, scene.door));
    assert!(!scene.world.get::<Airlock>(scene.door).unwrap().powered);
}

#[test]
fn ui_lists_wires_and_lights() {
    let mut scene = scene();
    let state = ui_state(&mut scene.world, scene.door).expect("состояние окна");
    assert_eq!(state.wires.len(), 8);
    // Питание, журнал, болты, свет болтов, таймер, защита.
    assert_eq!(state.statuses.len(), 6);
}
