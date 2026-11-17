use super::*;
use crate::wires::{LightState, View};
use astrocore_math::{GridCell, GridEdge};
use astrocore_world::{OpeningKind, Placement, StructureElement, StructureKind};

const DOOR: u128 = 0xDA001;
const ASSEMBLY: u128 = 0xDA002;

fn fixture(insulated: bool) -> (Harness, ConnId, ConnId) {
    let mut h = harness();
    let who = h.join("engineer");
    let other = h.join("observer");
    let door = StructureElement {
        id: oid(DOOR),
        floor: FloorId(0),
        kind: StructureKind::Opening(OpeningKind::Door),
        placement: Placement::Edge(
            GridEdge::between(GridCell::new(2, 1), GridCell::new(3, 1)).unwrap(),
        ),
        material: PrototypeId::new("ss14:airlock").unwrap(),
        thickness: 0.1,
    };
    let out = h
        .server
        .apply_world_command(WorldCommand::PlaceElement(door))
        .unwrap();
    h.deliver(out);
    let mut object = AuthoredObject::new(
        oid(ASSEMBLY),
        AuthoredKind::Zone,
        FloorId(0),
        Vec3::new(2.2, 0.2, 1.2),
        Vec3::new(2.8, 1., 1.8),
    );
    object.metadata = [
        (
            "door_element".into(),
            format!("{:x}", oid(DOOR).as_uuid().as_u128()),
        ),
        ("construction_graph".into(), "Airlock".into()),
        ("construction_node".into(), "airlock".into()),
        ("panel_open".into(), "1".into()),
        ("frame_board".into(), "DoorElectronics".into()),
    ]
    .into();
    spawn(&mut h, object);
    if insulated {
        equip(&mut h, who, 0xDA100, "clothing_hands_gloves_color_yellow");
        command(&mut h, who, CMD_EQUIP, equip_data(0, "gloves"));
    }
    equip(&mut h, who, 0xDA101, "multitool");
    command(&mut h, who, crate::wires::CMD_OPEN, pickup_data(oid(DOOR)));
    h.ticks(2);
    (h, who, other)
}

fn equip(h: &mut Harness, who: ConnId, id: u128, prototype: &str) {
    if hands_of(h, who, who).is_some_and(|hands| hands.get(0).is_some()) {
        command(h, who, CMD_DROP, drop_data(0));
    }
    let feet = h.client(who).position();
    spawn(
        h,
        item_at(id, &format!("ss14:{prototype}"), feet + Vec3::Y * 0.4),
    );
    command(h, who, CMD_PICKUP, pickup_data(oid(id)));
}

fn view(h: &Harness, who: ConnId) -> View {
    let me = h.client(who).net_id().unwrap();
    h.client(who)
        .component(me, crate::wires::COMPONENT)
        .and_then(crate::wires::decode)
        .unwrap()
}

fn action(h: &mut Harness, who: ConnId, id: u8, operation: &str) {
    command(
        h,
        who,
        crate::wires::CMD_ACTION,
        target_data(
            VerbTarget::Object(oid(DOOR)),
            Some(&format!("door_wire_{operation}_{id}")),
        ),
    );
    h.ticks(2);
}

fn light(view: &View, name: &str) -> LightState {
    view.lights
        .iter()
        .find(|light| light.text == name)
        .unwrap()
        .state
}

fn motion(h: &Harness, who: ConnId) -> Vec<crate::wires::Motion> {
    let me = h.client(who).net_id().unwrap();
    h.client(who)
        .component(me, crate::wires::MOTION_COMPONENT)
        .and_then(crate::wires::decode_motions)
        .unwrap()
}

fn door_open(h: &Harness, who: ConnId) -> Option<bool> {
    let client = h.client(who);
    let id = client
        .door_ids()
        .into_iter()
        .find(|id| client.door_element(*id) == Some(oid(DOOR)))?;
    client.door_open(id)
}

fn metadata(h: &mut Harness, who: ConnId, key: &str, value: &str) {
    let mut object = h
        .client(who)
        .world()
        .unwrap()
        .authored_object(oid(ASSEMBLY))
        .unwrap()
        .clone();
    object.metadata.insert(key.into(), value.into());
    let out = h
        .server
        .apply_world_command(WorldCommand::RemoveAuthored { id: object.id })
        .unwrap();
    h.deliver(out);
    spawn(h, object);
}

fn empty_hand_open(h: &mut Harness, who: ConnId) {
    command(h, who, CMD_DROP, drop_data(0));
    command(h, who, CMD_INTERACT, interact_data(oid(DOOR)));
    h.ticks(14);
    assert_eq!(door_open(h, who), Some(true));
}

#[test]
fn returning_power_starts_a_full_auto_close_delay() {
    let (mut h, who, _) = fixture(true);
    empty_hand_open(&mut h, who);
    h.ticks(80);
    metadata(&mut h, who, "wire_power_disabled", "1");
    h.ticks(130);
    assert_eq!(door_open(&h, who), Some(true));
    metadata(&mut h, who, "wire_power_disabled", "0");
    h.ticks(96);
    assert_eq!(door_open(&h, who), Some(true));
    h.ticks(18);
    assert_eq!(door_open(&h, who), Some(false));
}

#[test]
fn lowering_bolts_during_closing_aborts_the_partial_close() {
    let (mut h, who, other) = fixture(true);
    empty_hand_open(&mut h, who);
    command(&mut h, who, CMD_INTERACT, interact_data(oid(DOOR)));
    h.ticks(3);
    metadata(&mut h, who, "bolted", "1");
    h.ticks(12);
    assert_eq!(door_open(&h, other), Some(true));
    assert!(motion(&h, other).is_empty());
    metadata(&mut h, who, "bolted", "0");
    h.ticks(96);
    assert_eq!(door_open(&h, other), Some(true));
    h.ticks(18);
    assert_eq!(door_open(&h, other), Some(false));
}

#[test]
fn safety_prevents_closing_on_a_person_and_disabled_safety_crushes_then_reopens() {
    let (mut h, who, other) = fixture(true);
    empty_hand_open(&mut h, who);
    for _ in 0..20 {
        if h.server.player_position(who).unwrap().z <= 1.6 {
            break;
        }
        h.input(who, 0, 1, 0.);
    }
    for _ in 0..20 {
        if h.server.player_position(who).unwrap().x >= 2.9 {
            break;
        }
        h.input(who, 1, 0, 0.);
    }
    h.input(who, 0, 0, 0.);
    assert!((h.server.player_position(who).unwrap().z - 1.5).abs() < 0.2);
    assert!(
        (h.server.player_position(who).unwrap().x - 3.).abs() < 0.2,
        "{:?}",
        h.server.player_position(who)
    );
    command(&mut h, who, CMD_INTERACT, interact_data(oid(DOOR)));
    h.ticks(10);
    assert_eq!(
        door_open(&h, other),
        Some(true),
        "server={:?}, client={:?}, motion={:?}",
        h.server.player_position(who),
        h.client(who).position(),
        motion(&h, other)
    );
    assert!(motion(&h, other).is_empty());
    metadata(&mut h, who, "safety", "0");
    command(&mut h, who, CMD_INTERACT, interact_data(oid(DOOR)));
    h.ticks(10);
    assert_eq!(
        door_open(&h, other),
        Some(false),
        "position={:?}, motion={:?}, health={:?}",
        h.server.player_position(who),
        motion(&h, other),
        decode_health(
            h.client(who)
                .component(h.client(who).net_id().unwrap(), HEALTH_COMPONENT)
                .unwrap()
        )
    );
    let me = h.client(who).net_id().unwrap();
    let health = decode_health(h.client(who).component(me, HEALTH_COMPONENT).unwrap()).unwrap();
    assert_eq!(health.damage.get("Blunt"), Some(&15.));
    h.ticks(50);
    assert_eq!(door_open(&h, other), Some(true));
}

#[test]
fn a_crowbar_cannot_pry_a_powered_airlock_but_can_open_and_close_an_unpowered_one() {
    let (mut h, who, other) = fixture(true);
    equip(&mut h, who, 0xDA105, "crowbar");
    command(&mut h, who, CMD_INTERACT, interact_data(oid(DOOR)));
    assert!(inventory_and_storage::doafter(&h, who).is_none());
    metadata(&mut h, who, "wire_power_disabled", "1");
    h.ticks(2);
    command(&mut h, who, CMD_INTERACT, interact_data(oid(DOOR)));
    assert_eq!(
        inventory_and_storage::doafter(&h, who).unwrap().delay_s,
        1.5
    );
    h.ticks(27);
    assert_ne!(door_open(&h, other), Some(true));
    h.ticks(18);
    assert_eq!(door_open(&h, other), Some(true));
    command(&mut h, who, CMD_INTERACT, interact_data(oid(DOOR)));
    h.ticks(45);
    assert_eq!(door_open(&h, other), Some(false));
}

#[test]
fn protective_plating_keeps_the_same_door_and_returns_source_materials_when_removed() {
    for (index, material, unfinished, secured, weld_seconds, strip_seconds, refund) in [
        (
            0,
            "sheet_steel10",
            "medSecurityUnfinished",
            "medSecurity",
            3,
            10,
            "ss14:sheet_steel1",
        ),
        (
            1,
            "sheet_plasteel10",
            "highSecurityUnfinished",
            "highSecurity",
            5,
            15,
            "ss14:sheet_plasteel1",
        ),
    ] {
        let (mut h, who, other) = fixture(true);
        let current = |h: &Harness| {
            h.client(other)
                .world()
                .unwrap()
                .authored_object(oid(ASSEMBLY))
                .unwrap()
                .metadata
                .clone()
        };
        equip(&mut h, who, 0xDA200 + index, material);
        command(&mut h, who, CMD_INTERACT, interact_data(oid(DOOR)));
        h.ticks(45);
        assert_eq!(current(&h)["construction_node"], unfinished);
        assert_eq!(current(&h)["frame_board"], "DoorElectronics");
        assert!(
            h.client(other)
                .world()
                .unwrap()
                .element(oid(DOOR))
                .is_some()
        );
        command(&mut h, who, crate::wires::CMD_OPEN, pickup_data(oid(DOOR)));
        let me = h.client(who).net_id().unwrap();
        assert!(
            h.client(who)
                .component(me, crate::wires::COMPONENT)
                .and_then(crate::wires::decode)
                .is_none()
        );
        equip(&mut h, who, 0xDA210 + index, "welder");
        command(&mut h, who, CMD_USE, use_data(0));
        command(&mut h, who, CMD_INTERACT, interact_data(oid(DOOR)));
        h.ticks(weld_seconds * 20 + 5);
        assert_eq!(current(&h)["construction_node"], secured);
        command(&mut h, who, CMD_INTERACT, interact_data(oid(DOOR)));
        h.ticks(strip_seconds * 20 + 5);
        assert_eq!(current(&h)["construction_node"], unfinished);
        equip(&mut h, who, 0xDA220 + index, "crowbar");
        command(&mut h, who, CMD_INTERACT, interact_data(oid(DOOR)));
        h.ticks(85);
        assert_eq!(current(&h)["construction_node"], "airlock");
        assert_eq!(
            current(&h)["door_element"],
            format!("{:x}", oid(DOOR).as_uuid().as_u128())
        );
        let hands = hands_of(&h, who, who).unwrap();
        let item = hands
            .held()
            .find(|item| item.prototype.as_str() == refund)
            .unwrap();
        assert_eq!(
            inv_of(&h, who, who)
                .unwrap()
                .charges
                .iter()
                .find(|(id, _)| *id == item.id)
                .unwrap()
                .1,
            2
        );
    }
}

#[test]
fn an_airlock_animates_before_becoming_passable_and_auto_closes_for_every_client() {
    let (mut h, who, other) = fixture(true);
    command(&mut h, who, CMD_DROP, drop_data(0));
    command(&mut h, who, CMD_INTERACT, interact_data(oid(DOOR)));
    h.ticks(4);
    assert_ne!(door_open(&h, who), Some(true));
    assert!(
        motion(&h, other)
            .iter()
            .any(|m| m.element == oid(DOOR) && m.opening)
    );
    let late = h.join("late engineer");
    h.ticks(1);
    assert!(
        motion(&h, late)
            .iter()
            .any(|m| m.element == oid(DOOR) && m.opening && m.elapsed > 0.)
    );
    h.ticks(5);
    for client in [who, other, late] {
        assert_eq!(door_open(&h, client), Some(true));
    }
    h.ticks(94);
    assert_eq!(door_open(&h, who), Some(true));
    h.ticks(20);
    for client in [who, other, late] {
        assert_eq!(door_open(&h, client), Some(false));
        assert!(!motion(&h, client).iter().any(|m| m.element == oid(DOOR)));
    }
}

#[test]
fn wire_pulses_restore_timed_states_and_replicate_immediately() {
    let (mut h, who, other) = fixture(true);
    let initial = view(&h, who);
    assert_eq!(initial.wires.len(), 8);
    assert_eq!(initial.serial.chars().count(), 9);
    let mut changed = std::collections::BTreeSet::new();
    for wire in initial.wires {
        let before = view(&h, who);
        action(&mut h, who, wire.id, "Pulse");
        let after = view(&h, who);
        for status in &after.lights {
            if light(&before, &status.text) != status.state {
                changed.insert(status.text.clone());
            }
        }
        let world = h.client(other).world().unwrap();
        let obj = world.authored_object(oid(ASSEMBLY)).unwrap();
        if light(&after, "Power") == LightState::BlinkingSlow {
            assert_eq!(obj.metadata["wire_power_disabled"], "1");
            assert!(!after.wires.iter().any(|wire| wire.cut));
        }
        h.ticks(620);
        let restored = view(&h, who);
        for name in ["Safety", "Timing", "Access logging", "Power"] {
            assert_eq!(light(&restored, name), LightState::On, "{name}");
        }
    }
    for name in [
        "Power",
        "Safety",
        "Timing",
        "Access logging",
        "Bolts",
        "Bolt lights",
    ] {
        assert!(changed.contains(name), "{name}: {changed:?}");
    }
    assert_eq!(light(&view(&h, who), "AI control"), LightState::On);
}

#[test]
fn a_power_wire_shocks_an_unprotected_operator_before_it_can_be_cut() {
    let (mut h, who, other) = fixture(true);
    let mut power = None;
    for wire in view(&h, who).wires {
        action(&mut h, who, wire.id, "Pulse");
        if light(&view(&h, who), "Power") == LightState::BlinkingSlow {
            power = Some(wire.id);
            break;
        }
        h.ticks(620);
    }
    let power = power.unwrap();
    h.ticks(620);
    // Take the insulating gloves off, then make the same attempt with wirecutters.
    command(&mut h, who, CMD_DROP, drop_data(0));
    command(&mut h, who, CMD_UNEQUIP, unequip_data("gloves"));
    command(&mut h, who, CMD_DROP, drop_data(0));
    equip(&mut h, who, 0xDA102, "wirecutter");
    action(&mut h, who, power, "Cut");
    let me = h.client(who).net_id().unwrap();
    let health = decode_health(h.client(who).component(me, HEALTH_COMPONENT).unwrap()).unwrap();
    assert_eq!(health.damage.get("Shock"), Some(&7.));
    let object = h
        .client(other)
        .world()
        .unwrap()
        .authored_object(oid(ASSEMBLY))
        .unwrap();
    let mask = object.metadata["cut_wires"].parse::<u32>().unwrap();
    assert_eq!(mask & (1 << power), 0);
}

#[test]
fn losing_power_cancels_the_safety_pulse_restore_without_turning_safety_on() {
    let (mut h, who, _) = fixture(true);
    let mut safety = None;
    for wire in view(&h, who).wires {
        action(&mut h, who, wire.id, "Pulse");
        if light(&view(&h, who), "Safety") == LightState::Off {
            safety = Some(wire.id);
            break;
        }
        h.ticks(620);
    }
    let safety = safety.unwrap();
    metadata(&mut h, who, "wire_power_disabled", "1");
    h.ticks(620);
    assert_eq!(light(&view(&h, who), "Safety"), LightState::Off);
    metadata(&mut h, who, "wire_power_disabled", "0");
    h.ticks(620);
    assert_eq!(light(&view(&h, who), "Safety"), LightState::Off);
    equip(&mut h, who, 0xDA301, "wirecutter");
    action(&mut h, who, safety, "Cut");
    action(&mut h, who, safety, "Mend");
    assert_eq!(light(&view(&h, who), "Safety"), LightState::On);
}

#[test]
fn invalid_wire_actions_report_the_source_error_and_leave_the_wire_unchanged() {
    let (mut h, who, _) = fixture(true);
    let popup = |h: &Harness| {
        let me = h.client(who).net_id().unwrap();
        decode_fx(h.client(who).component(me, FX_COMPONENT).unwrap())
            .unwrap()
            .popups
    };
    action(&mut h, who, 0, "Cut");
    assert!(!view(&h, who).wires[0].cut);
    assert!(
        popup(&h)
            .iter()
            .any(|text| text == "You need to hold a wirecutter in your hand!")
    );
    equip(&mut h, who, 0xDA400, "wirecutter");
    action(&mut h, who, 0, "Cut");
    assert!(view(&h, who).wires[0].cut);
    action(&mut h, who, 0, "Cut");
    assert!(view(&h, who).wires[0].cut);
    assert!(
        popup(&h)
            .iter()
            .any(|text| text == "You can't cut a wire that's been cut!")
    );
    equip(&mut h, who, 0xDA401, "multitool");
    action(&mut h, who, 0, "Pulse");
    assert!(view(&h, who).wires[0].cut);
    assert!(
        popup(&h)
            .iter()
            .any(|text| text == "You can't pulse a wire that's been cut!")
    );
}
