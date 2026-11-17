use super::*;
const TARGET: u128 = 0xDEA001;
const TOOL: u128 = 0xDEA002;

fn fixture(proto: &str, damage: &[(&str, f32)], lit: bool) -> (Harness, ConnId, ConnId) {
    let mut h = harness();
    let who = h.join("engineer");
    let other = h.join("observer");
    let at = h.client(who).position();
    let mut object = item_at(TARGET, proto, at + Vec3::new(0.6, 0.1, 0.));
    let damage: std::collections::BTreeMap<_, _> = damage.iter().copied().collect();
    object
        .metadata
        .insert("damage".into(), serde_json::to_string(&damage).unwrap());
    if proto == "ss14:vending_machine_cola" && damage.values().sum::<f32>() >= 75. {
        object.metadata.insert("broken".into(), "1".into());
    }
    spawn(&mut h, object);
    spawn(
        &mut h,
        item_at(TOOL, "ss14:welder", at + Vec3::new(0.2, 0.2, 0.)),
    );
    command(&mut h, who, CMD_PICKUP, pickup_data(oid(TOOL)));
    if lit {
        command(&mut h, who, CMD_USE, use_data(0));
    }
    h.ticks(2);
    (h, who, other)
}

fn damage(h: &Harness, who: ConnId) -> std::collections::BTreeMap<String, f32> {
    let object = h
        .client(who)
        .world()
        .unwrap()
        .authored_object(oid(TARGET))
        .unwrap();
    serde_json::from_str(&object.metadata["damage"]).unwrap()
}

fn fuel(h: &Harness, who: ConnId) -> f32 {
    inv_of(h, who, who)
        .unwrap()
        .fills
        .iter()
        .find(|entry| entry.0 == oid(TOOL))
        .unwrap()
        .1
}

fn start(h: &mut Harness, who: ConnId) {
    command(h, who, CMD_INTERACT, interact_data(oid(TARGET)));
}

fn popups(h: &Harness, who: ConnId) -> Vec<String> {
    decode_fx(
        h.client(who)
            .component(h.client(who).net_id().unwrap(), FX_COMPONENT)
            .unwrap(),
    )
    .unwrap()
    .popups
}

#[test]
fn evenly_distributed_repair_preserves_hundredths_and_damage_value_precedence() {
    use crate::module::repairable::repair_damage;
    let mut damage = [
        ("Blunt".into(), 1.),
        ("Heat".into(), 1.),
        ("Shock".into(), 1.),
    ]
    .into();
    repair_damage(
        &mut damage,
        &serde_json::json!({"damageValue":-1,"damage":{"types":{"Blunt":-100}}}),
    );
    assert_eq!(
        damage,
        [
            ("Blunt".into(), 0.68),
            ("Heat".into(), 0.66),
            ("Shock".into(), 0.66)
        ]
        .into()
    );
    for _ in 0..200 {
        repair_damage(&mut damage, &serde_json::json!({"damageValue":-0.01}));
    }
    assert!(damage.is_empty());
    let mut damage = [
        ("Blunt".into(), 0.01),
        ("Heat".into(), 1.),
        ("Shock".into(), 1.),
    ]
    .into();
    repair_damage(&mut damage, &serde_json::json!({"damageValue":-1}));
    assert_eq!(
        damage,
        [("Heat".into(), 0.51), ("Shock".into(), 0.5)].into()
    );
    let original = damage.clone();
    repair_damage(&mut damage, &serde_json::json!({"damageValue":1}));
    assert_eq!(damage, original);
    repair_damage(
        &mut damage,
        &serde_json::json!({"damage":{"types":{"Heat":-0.01}}}),
    );
    assert_eq!(damage["Heat"], 0.5);
    repair_damage(&mut damage, &serde_json::json!({}));
    assert!(damage.is_empty());
}

#[test]
fn a_welder_running_low_during_repair_cancels_without_the_completion_charge() {
    let (mut h, who, other) = fixture("ss14:vending_machine_cola", &[("Blunt", 100.)], true);
    let (mut control, owner, _) = fixture("ss14:vending_machine_cola", &[], true);
    h.ticks(1840);
    control.ticks(1840);
    assert!((5.0..13.0).contains(&fuel(&h, who)));
    start(&mut h, who);
    assert!(inventory_and_storage::doafter(&h, who).is_some());
    h.ticks(70);
    control.ticks(70);
    assert!(inventory_and_storage::doafter(&h, who).is_none());
    assert_eq!(damage(&h, other)["Blunt"], 100.);
    assert!((fuel(&control, owner) - fuel(&h, who)).abs() < 0.01);
    start(&mut h, who);
    assert!(inventory_and_storage::doafter(&h, who).is_none());
    assert!(
        popups(&h, who)
            .iter()
            .any(|text| text.contains("does not have enough fuel for that!"))
    );
}

#[test]
fn an_eight_second_vendor_repair_consumes_fuel_on_completion_and_replicates_to_late_joiners() {
    let (mut h, who, other) = fixture("ss14:vending_machine_cola", &[("Blunt", 100.)], true);
    let (mut control, owner, _) = fixture("ss14:vending_machine_cola", &[], true);
    start(&mut h, who);
    assert_eq!(inventory_and_storage::doafter(&h, who).unwrap().delay_s, 8.);
    assert_eq!(fuel(&h, who), fuel(&control, owner));
    h.ticks(155);
    control.ticks(155);
    assert_eq!(damage(&h, other)["Blunt"], 100.);
    assert_eq!(fuel(&h, who), fuel(&control, owner));
    h.ticks(12);
    control.ticks(12);
    assert!(damage(&h, other).is_empty());
    assert_eq!(
        h.client(other)
            .world()
            .unwrap()
            .authored_object(oid(TARGET))
            .unwrap()
            .metadata["broken"],
        "0"
    );
    assert!((fuel(&control, owner) - fuel(&h, who) - 5.).abs() < 0.01);
    let late = h.join("late engineer");
    h.ticks(2);
    assert!(damage(&h, late).is_empty());
    command(&mut h, who, CMD_DROP, drop_data(0));
    start(&mut h, who);
    h.ticks(2);
    assert!(
        decode_vending(
            h.client(who)
                .component(h.client(who).net_id().unwrap(), VENDING_COMPONENT)
                .unwrap()
        )
        .is_some_and(|view| !view.broken)
    );
    command(&mut h, who, CMD_VEND, b"Crowbar".to_vec());
    h.ticks(27);
    assert!(h.client(other).world().unwrap().authored().any(|object| matches!(&object.kind, AuthoredKind::Placement {prototype} if prototype.as_str()=="ss14:crowbar")));
}

#[test]
fn typed_shield_repairs_repeat_and_do_not_spend_the_next_pulses_fuel_early() {
    let (mut h, who, other) = fixture("ss14:riot_shield", &[("Blunt", 45.), ("Heat", 5.)], true);
    let (mut control, owner, _) = fixture("ss14:riot_shield", &[], true);
    start(&mut h, who);
    h.ticks(103);
    control.ticks(103);
    assert_eq!(damage(&h, other), [("Blunt".into(), 25.)].into());
    assert!(inventory_and_storage::doafter(&h, who).is_some());
    assert!((fuel(&control, owner) - fuel(&h, who) - 10.).abs() < 0.01);
    h.ticks(103);
    control.ticks(103);
    assert_eq!(damage(&h, other), [("Blunt".into(), 5.)].into());
    h.ticks(103);
    control.ticks(103);
    assert!(damage(&h, other).is_empty());
    assert!(inventory_and_storage::doafter(&h, who).is_none());
    assert!((fuel(&control, owner) - fuel(&h, who) - 30.).abs() < 0.01);
}

#[test]
fn a_hand_change_or_movement_interrupts_repairs_without_the_completion_charge() {
    for moving in [false, true] {
        let (mut h, who, other) = fixture("ss14:vending_machine_cola", &[("Blunt", 40.)], true);
        let (mut control, owner, _) = fixture("ss14:vending_machine_cola", &[], true);
        start(&mut h, who);
        if moving {
            for _ in 0..5 {
                h.input(who, 0, 1, 0.);
            }
        } else {
            command(&mut h, who, CMD_SELECT_HAND, vec![1]);
            h.ticks(5);
        }
        h.ticks(170);
        control.ticks(175);
        assert_eq!(damage(&h, other)["Blunt"], 40.);
        assert!(inventory_and_storage::doafter(&h, who).is_none());
        assert!((fuel(&control, owner) - fuel(&h, who)).abs() < 0.01);
    }
}

#[test]
fn repair_damage_survives_pickup_drop_and_world_serialization() {
    let (mut h, who, other) = fixture("ss14:riot_shield", &[("Heat", 35.)], false);
    command(&mut h, who, CMD_DROP, drop_data(0));
    command(&mut h, who, CMD_PICKUP, pickup_data(oid(TARGET)));
    command(&mut h, who, CMD_DROP, drop_data(0));
    h.ticks(2);
    assert_eq!(damage(&h, other)["Heat"], 35.);
    let world = h.client(other).world().unwrap();
    let encoded = serde_json::to_vec(world.authored_object(oid(TARGET)).unwrap()).unwrap();
    let saved: AuthoredObject = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        serde_json::from_str::<std::collections::BTreeMap<String, f32>>(&saved.metadata["damage"])
            .unwrap()["Heat"],
        35.
    );
}

#[test]
fn an_unlit_welder_and_a_healthy_target_do_not_start_a_repair() {
    let (mut h, who, other) = fixture("ss14:vending_machine_cola", &[("Blunt", 40.)], false);
    start(&mut h, who);
    h.ticks(1);
    assert!(
        popups(&h, who)
            .iter()
            .any(|text| text.contains("is turned off!"))
    );
    h.ticks(170);
    assert!(inventory_and_storage::doafter(&h, who).is_none());
    assert_eq!(damage(&h, other)["Blunt"], 40.);
    assert_eq!(fuel(&h, who), 100.);
    let (mut h, who, _) = fixture("ss14:riot_shield", &[], true);
    start(&mut h, who);
    assert!(inventory_and_storage::doafter(&h, who).is_none());
}

#[test]
fn two_engineers_repair_the_same_shield_without_losing_either_change() {
    let (mut h, who, other) = fixture("ss14:riot_shield", &[("Blunt", 55.)], true);
    let at = h.client(other).position();
    spawn(
        &mut h,
        item_at(TOOL + 1, "ss14:welder", at + Vec3::new(0.2, 0.2, 0.)),
    );
    command(&mut h, other, CMD_PICKUP, pickup_data(oid(TOOL + 1)));
    command(&mut h, other, CMD_USE, use_data(0));
    start(&mut h, who);
    start(&mut h, other);
    h.ticks(103);
    assert_eq!(damage(&h, who), [("Blunt".into(), 15.)].into());
}

#[test]
fn object_damage_requires_combat_mode_and_uses_the_original_modifiers_and_break_threshold() {
    let (mut h, who, other) = fixture("ss14:vending_machine_cola", &[], false);
    command(&mut h, who, CMD_DROP, drop_data(0));
    command(&mut h, who, CMD_ATTACK_OBJECT, pickup_data(oid(TARGET)));
    assert!(damage(&h, who).is_empty());
    command(&mut h, who, CMD_COMBAT, vec![1]);
    command(&mut h, who, CMD_ATTACK_OBJECT, pickup_data(oid(TARGET)));
    // GlassReinforcedMod: a fist's 5 Blunt * 1.3 - 5 = 1.5.
    assert_eq!(damage(&h, other)["Blunt"], 1.5);
    for _ in 0..49 {
        h.ticks(15);
        command(&mut h, who, CMD_ATTACK_OBJECT, pickup_data(oid(TARGET)));
    }
    assert_eq!(damage(&h, other)["Blunt"], 75.);
    assert_eq!(
        h.client(other)
            .world()
            .unwrap()
            .authored_object(oid(TARGET))
            .unwrap()
            .metadata["broken"],
        "1"
    );
    let dropped = h.client(other).world().unwrap().authored().filter(|object| matches!(&object.kind, AuthoredKind::Placement{prototype} if prototype.as_str()=="ss14:crowbar")).count();
    assert_eq!(
        dropped, 1,
        "the inherited breakage threshold ejects ceil(1 * 0.25) items"
    );
}

#[test]
fn a_second_completed_repair_still_consumes_fuel_when_the_first_one_already_fixed_the_target() {
    let (mut h, who, other) = fixture("ss14:riot_shield", &[("Blunt", 15.)], true);
    let (mut control, owner, _) = fixture("ss14:riot_shield", &[], true);
    let at = h.client(other).position();
    spawn(
        &mut h,
        item_at(TOOL + 1, "ss14:welder", at + Vec3::new(0.2, 0.2, 0.)),
    );
    command(&mut h, other, CMD_PICKUP, pickup_data(oid(TOOL + 1)));
    command(&mut h, other, CMD_USE, use_data(0));
    start(&mut h, who);
    start(&mut h, other);
    h.ticks(103);
    control.ticks(103);
    assert!(damage(&h, who).is_empty());
    let other_fuel = inv_of(&h, other, other)
        .unwrap()
        .fills
        .into_iter()
        .find(|entry| entry.0 == oid(TOOL + 1))
        .unwrap()
        .1;
    assert!((fuel(&control, owner) - fuel(&h, who) - 10.).abs() < 0.01);
    assert!((fuel(&control, owner) - other_fuel - 10.).abs() < 0.01);
    assert!(inventory_and_storage::doafter(&h, who).is_none());
    assert!(inventory_and_storage::doafter(&h, other).is_none());
}

#[test]
fn destroying_a_shield_yields_two_original_material_stacks_with_two_sheets_each() {
    let (mut h, who, other) = fixture("ss14:riot_shield", &[("Blunt", 120.)], false);
    command(&mut h, who, CMD_DROP, drop_data(0));
    let at = h.client(who).position();
    spawn(
        &mut h,
        item_at(TOOL + 5, "ss14:cudgel", at + Vec3::new(0.2, 0.2, 0.)),
    );
    command(&mut h, who, CMD_PICKUP, pickup_data(oid(TOOL + 5)));
    command(&mut h, who, CMD_COMBAT, vec![1]);
    command(&mut h, who, CMD_ATTACK_OBJECT, pickup_data(oid(TARGET)));
    h.ticks(2);
    let world = h.client(other).world().unwrap();
    assert!(world.authored_object(oid(TARGET)).is_none());
    let drops: Vec<_> = world.authored().filter(|object|matches!(&object.kind,AuthoredKind::Placement{prototype} if ["ss14:sheet_steel","ss14:sheet_glass"].contains(&prototype.as_str()))).collect();
    assert_eq!(drops.len(), 2);
    for dropped in drops {
        assert_eq!(dropped.metadata["stack_count"], "2");
        let middle = (dropped.min() + dropped.max()) * 0.5;
        assert!((middle.x - at.x - 0.75).abs() <= 0.5);
        assert!((middle.z - at.z - 0.15).abs() <= 0.5);
    }
}

#[test]
fn destroying_a_vendor_replicates_its_original_metal_glass_break_sound_to_an_observer() {
    let (mut h, who, other) = fixture("ss14:vending_machine_cola", &[("Blunt", 174.)], false);
    command(&mut h, who, CMD_DROP, drop_data(0));
    command(&mut h, who, CMD_COMBAT, vec![1]);
    command(&mut h, who, CMD_ATTACK_OBJECT, pickup_data(oid(TARGET)));
    h.ticks(1);
    assert!(
        h.client(other)
            .world()
            .unwrap()
            .authored_object(oid(TARGET))
            .is_none()
    );
    let fx = decode_fx(
        h.client(other)
            .component(h.client(other).net_id().unwrap(), FX_COMPONENT)
            .unwrap(),
    )
    .unwrap();
    let sounds: Vec<_> = fx
        .sounds
        .iter()
        .filter(|sound| {
            matches!(
                sound.path.as_str(),
                "Effects/metal_glass_break1.ogg" | "Effects/metal_glass_break2.ogg"
            )
        })
        .collect();
    assert_eq!(sounds.len(), 1);
}
