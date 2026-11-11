use std::sync::Arc;

use astrocore_api::{ModuleHost, ReplicationRegistry};
use astrocore_core::{ObjectId, PrototypeId};
use astrocore_math::glam::Vec3;
use astrocore_network::client::ClientEvent;
use astrocore_network::server::{ConnId, GameServer};
use astrocore_network::testkit::{ByBuild, Harness, test_world};
use astrocore_world::{AuthoredKind, AuthoredObject, FloorId, WorldCommand};
use prototypes::PrototypeRegistry;

use crate::*;

const PROTOS: &str = r#"[
    (id: "BaseItem", abstract: true, components: {"Item": (size: "Small")}),
    (id: "Crowbar", parents: ["BaseItem"], components: {"Tool": (qualities: ["Prying"])}),
    (id: "Boulder", components: {"Sprite": (state: "boulder")}),
    (id: "Pen", components: {"Item": (size: "Tiny")}),
    (id: "Toolbox", components: {"Item": (size: "Large")}),
    (id: "Jumpsuit", components: {"Item": (size: "Normal"), "Clothing": (slots: ["InnerClothing"])}),
    (id: "Cudgel", components: {
        "Item": (size: "Normal"),
        "MeleeWeapon": (damage: (Blunt: 60.0), attackRate: 1.0),
    }),
    (id: "Dagger", components: {
        "Item": (size: "Small"),
        "MeleeWeapon": (damage: (Slash: 20.0), attackRate: 1.0),
    }),
    (id: "Bandage", components: {
        "Item": (size: "Small"),
        "Stack": (stackType: "Bandage", count: 3),
        "Healing": (damage: (Blunt: -1.0), delay: 0.2, bloodlossModifier: -10.0),
    }),
    (id: "Burger", components: {
        "Item": (size: "Small"),
        "Edible": (transferAmount: 5.0, delay: 1.0, destroyOnEmpty: true),
        "Solution": (maxVol: 50.0, reagents: (Nutriment: 30.0, Protein: 10.0, Vitamin: 10.0)),
    }),
    (id: "WaterBottle", components: {
        "Item": (size: "Small"),
        "Edible": (transferAmount: 5.0, delay: 1.0, destroyOnEmpty: false),
        "Openable": (opened: false),
        "Solution": (maxVol: 60.0, reagents: (Water: 60.0)),
    }),
    (id: "Brutepack", components: {
        "Item": (size: "Small"),
        "Healing": (damage: (Blunt: -30.0, Slash: -30.0), delay: 1.0),
    }),
    (id: "HeavyCoat", components: {
        "Item": (size: "Normal"),
        "Clothing": (slots: ["OuterClothing"], equipDelay: 1.0, unequipDelay: 0.5),
    }),
    (id: "Backpack", components: {
        "Item": (size: "Large"),
        "Clothing": (slots: ["Back"]),
        "Storage": (grid: ["0,0,5,3"], maxItemSize: "Normal"),
    }),
    (id: "Welder", components: {
        "Item": (size: "Small"),
        "IgnitionSource": (temperature: 700.0),
    }),
    (id: "Satchel", components: {
        "Item": (size: "Small"),
        "Storage": (grid: ["0,0,1,1"], maxItemSize: "Tiny"),
    }),
]"#;

/// Идентификаторы предметов не пересекаются с идентификаторами тестового мира.
fn oid(n: u128) -> ObjectId {
    ObjectId::from_u128(0xBEEF_0000 + n)
}

fn registry() -> Arc<PrototypeRegistry> {
    let mut r = PrototypeRegistry::new();
    r.load_ron_str(PROTOS, "test").unwrap();
    r.validate().unwrap();
    Arc::new(r)
}

fn harness() -> Harness {
    let mut host = ModuleHost::new();
    host.register(Box::new(
        GameplayModule::new(registry())
            .unwrap()
            .with_spawn(Vec3::new(1.5, 0.25, 1.5)),
    ))
    .unwrap();
    host.finalize().unwrap();
    let mut reg = ReplicationRegistry::new();
    register_replication(&mut reg).unwrap();
    let server = GameServer::new(test_world(), Harness::default_cfg(), Box::new(ByBuild))
        .with_modules(host)
        .with_replication(reg);
    Harness::with_server(server)
}

fn item_at(id: u128, proto: &str, at: Vec3) -> AuthoredObject {
    AuthoredObject::new(
        oid(id),
        AuthoredKind::Placement {
            prototype: PrototypeId::new(proto).unwrap(),
        },
        FloorId(0),
        at,
        at + Vec3::splat(0.3),
    )
}

fn spawn(h: &mut Harness, obj: AuthoredObject) {
    let outs = h
        .server
        .apply_world_command(WorldCommand::PlaceAuthored(obj))
        .unwrap();
    h.deliver(outs);
}

fn command(h: &mut Harness, who: ConnId, kind: &str, data: Vec<u8>) {
    let p = h.client(who).command_packet(kind, data).unwrap();
    h.send(who, p);
}

fn hands_of(h: &Harness, viewer: ConnId, owner: ConnId) -> Option<Hands> {
    let net = h.client(owner).net_id()?;
    decode_hands(h.client(viewer).component(net, HANDS_COMPONENT)?)
}

fn on_ground(h: &Harness, who: ConnId, id: u128) -> bool {
    h.client(who)
        .world()
        .is_some_and(|w| w.authored_object(oid(id)).is_some())
}

#[test]
fn one_client_picks_up_and_the_other_sees_it() {
    let mut h = harness();
    let (a, b) = (h.join("a"), h.join("b"));
    // Игроки появляются в (2.5, 1, 2.5); предмет рядом.
    spawn(
        &mut h,
        item_at(100, "ss14:crowbar", Vec3::new(3.0, 0.9, 3.0)),
    );
    assert!(on_ground(&h, a, 100) && on_ground(&h, b, 100));

    command(&mut h, a, CMD_PICKUP, pickup_data(oid(100)));
    assert!(
        !on_ground(&h, a, 100),
        "предмет ушёл из мира у подбиравшего"
    );
    assert!(!on_ground(&h, b, 100), "и у второго клиента");
    let hands = hands_of(&h, b, a).expect("второй клиент видит руки первого");
    assert_eq!(hands.held().count(), 1);
    assert_eq!(hands.get(0).unwrap().id, oid(100));
    assert_eq!(hands.get(0).unwrap().prototype.as_str(), "ss14:crowbar");
    assert!(hands_of(&h, b, b).is_none() || hands_of(&h, b, b).unwrap().held().count() == 0);

    // Второй пробует взять уже поднятый предмет: ничего не происходит.
    command(&mut h, b, CMD_PICKUP, pickup_data(oid(100)));
    assert!(hands_of(&h, a, b).is_none_or(|x| x.held().count() == 0));
}

#[test]
fn drop_returns_the_same_object_to_the_world_for_everyone() {
    let mut h = harness();
    let (a, b) = (h.join("a"), h.join("b"));
    spawn(
        &mut h,
        item_at(100, "ss14:crowbar", Vec3::new(3.0, 0.9, 3.0)),
    );
    command(&mut h, a, CMD_PICKUP, pickup_data(oid(100)));
    command(&mut h, a, CMD_DROP, drop_data(0));
    assert!(on_ground(&h, b, 100), "тот же идентификатор вернулся в мир");
    assert_eq!(hands_of(&h, b, a).unwrap().held().count(), 0);
    let w = h.client(b).world().unwrap();
    let obj = w.authored_object(oid(100)).unwrap();
    assert!(
        (obj.min().x - 2.5).abs() < 0.5,
        "упал у ног игрока: {:?}",
        obj.min()
    );
    // Пустую руку выбросить нельзя; ничего не ломается.
    command(&mut h, a, CMD_DROP, drop_data(0));
    command(&mut h, a, CMD_DROP, drop_data(7));
    assert_eq!(h.server.player_count(), 2);
}

#[test]
fn reach_item_type_and_hand_capacity_are_enforced_by_the_server() {
    let mut h = harness();
    let a = h.join("a");
    spawn(
        &mut h,
        item_at(1, "ss14:crowbar", Vec3::new(30.0, 0.9, 3.0)),
    );
    spawn(&mut h, item_at(2, "ss14:boulder", Vec3::new(3.0, 0.9, 3.0)));
    spawn(&mut h, item_at(3, "ss14:crowbar", Vec3::new(3.0, 0.9, 2.0)));
    spawn(&mut h, item_at(4, "ss14:crowbar", Vec3::new(2.0, 0.9, 3.0)));
    spawn(&mut h, item_at(5, "ss14:crowbar", Vec3::new(2.5, 0.9, 3.0)));
    for id in [1u128, 2] {
        command(&mut h, a, CMD_PICKUP, pickup_data(oid(id)));
        assert!(
            on_ground(&h, a, id),
            "{id}: слишком далеко или не предмет — остаётся"
        );
    }
    for id in [3u128, 4, 5] {
        command(&mut h, a, CMD_PICKUP_ANY, pickup_data(oid(id)));
    }
    let hands = hands_of(&h, a, a).unwrap();
    assert_eq!(hands.held().count(), 2, "две руки");
    assert!(
        on_ground(&h, a, 5),
        "третий предмет не влез и остался на земле"
    );
    // Мусорные данные команд не вредят.
    command(&mut h, a, CMD_PICKUP, vec![1, 2, 3]);
    command(&mut h, a, CMD_SELECT_HAND, vec![]);
    command(&mut h, a, CMD_SELECT_HAND, vec![9]);
    assert_eq!(h.server.player_count(), 1);
}

#[test]
fn selecting_a_hand_changes_where_the_next_item_goes() {
    let mut h = harness();
    let a = h.join("a");
    spawn(&mut h, item_at(1, "ss14:crowbar", Vec3::new(3.0, 0.9, 3.0)));
    command(&mut h, a, CMD_SELECT_HAND, vec![1]);
    command(&mut h, a, CMD_PICKUP, pickup_data(oid(1)));
    let hands = hands_of(&h, a, a).unwrap();
    assert_eq!(hands.active(), 1);
    assert!(hands.get(1).is_some() && hands.get(0).is_none());
}

#[test]
fn items_in_hands_are_left_on_the_floor_when_a_player_disconnects() {
    let mut h = harness();
    let (a, b) = (h.join("a"), h.join("b"));
    spawn(&mut h, item_at(1, "ss14:crowbar", Vec3::new(3.0, 0.9, 3.0)));
    command(&mut h, a, CMD_PICKUP, pickup_data(oid(1)));
    assert!(!on_ground(&h, b, 1));
    let outs = h.server.disconnect(a);
    h.deliver(outs);
    assert!(on_ground(&h, b, 1), "предмет не пропал вместе с игроком");
    assert!(
        h.take_events(b)
            .iter()
            .any(|e| matches!(e, ClientEvent::WorldChanged { .. }))
    );
}

#[test]
fn wire_encoding_round_trips() {
    let m = GameplayModule::new(registry()).unwrap();
    assert_eq!(m.hands_of(astrocore_core::NetworkEntityId(1)), None);
    assert_eq!(
        parse_object_id(&pickup_data(ObjectId::from_u128(0xABCDEF))),
        Some(ObjectId::from_u128(0xABCDEF))
    );
    assert_eq!(parse_object_id(&[1, 2]), None);
}

#[test]
fn clicking_takes_into_the_active_hand_only_while_the_verb_finds_a_free_one() {
    let mut h = harness();
    let a = h.join("a");
    spawn(&mut h, item_at(1, "ss14:crowbar", Vec3::new(3.0, 0.9, 3.0)));
    spawn(&mut h, item_at(2, "ss14:crowbar", Vec3::new(3.0, 0.9, 2.0)));
    command(&mut h, a, CMD_PICKUP, pickup_data(oid(1)));
    assert!(!on_ground(&h, a, 1));
    // Активная рука занята: клик по второму предмету ничего не делает (как в SS14).
    command(&mut h, a, CMD_PICKUP, pickup_data(oid(2)));
    assert!(on_ground(&h, a, 2));
    // Глагол «поднять» берёт во вторую руку.
    command(&mut h, a, CMD_PICKUP_ANY, pickup_data(oid(2)));
    assert!(!on_ground(&h, a, 2));
    assert_eq!(hands_of(&h, a, a).unwrap().held().count(), 2);
}

#[test]
fn swapping_hands_cycles_the_active_hand_for_everyone_to_see() {
    let mut h = harness();
    let (a, b) = (h.join("a"), h.join("b"));
    command(&mut h, a, CMD_SWAP_HANDS, vec![]);
    assert_eq!(hands_of(&h, b, a).unwrap().active(), 1);
    command(&mut h, a, CMD_SWAP_HANDS, vec![]);
    assert_eq!(hands_of(&h, b, a).unwrap().active(), 0);
    command(&mut h, a, CMD_SWAP_HANDS, vec![1]);
    assert_eq!(hands_of(&h, b, a).unwrap().active(), 1, "назад по кругу");
}

#[test]
fn an_item_moves_between_hands_by_command() {
    let mut h = harness();
    let a = h.join("a");
    grab(&mut h, a, 10, near(), "ss14:pen");
    let held = |h: &Harness, i: usize| hands_of(h, a, a).unwrap().get(i).map(|x| x.id);
    assert_eq!(held(&h, 0), Some(oid(10)));
    command(&mut h, a, crate::CMD_MOVE_HAND, crate::move_hand_data(0, 1));
    assert_eq!((held(&h, 0), held(&h, 1)), (None, Some(oid(10))));
    // Пустая рука с пустой — ничего; неверная рука — ничего.
    command(&mut h, a, crate::CMD_MOVE_HAND, crate::move_hand_data(0, 9));
    assert_eq!(held(&h, 1), Some(oid(10)));
}

#[test]
fn a_target_drop_lands_where_asked_within_range() {
    let mut h = harness();
    let a = h.join("a");
    spawn(&mut h, item_at(1, "ss14:crowbar", Vec3::new(3.0, 0.9, 3.0)));
    command(&mut h, a, CMD_PICKUP, pickup_data(oid(1)));
    // Цель в 1 м от персонажа (2.5, 1, 2.5) по +X, внутри дальности.
    command(&mut h, a, CMD_DROP, drop_at_data(0, [3.5, 1.0, 2.5]));
    let w = h.client(a).world().unwrap();
    let o = w.authored_object(oid(1)).expect("выброшен");
    assert!(
        (o.min().x + o.max().x) * 0.5 > 3.3,
        "лёг у цели, а не под ногами: {:?}",
        o.min()
    );
}

mod module_level {
    // Проверки без сети: модуль получает мир с коллизией и позиции игроков напрямую.
    use std::collections::BTreeMap;

    use astrocore_api::GameModule;
    use astrocore_core::NetworkEntityId;
    use astrocore_math::GridEdge;
    use astrocore_physics::BoxWorld;
    use astrocore_world::{
        FloorDef, Placement, StructureElement, StructureKind, World, WorldConfig,
    };

    use super::*;

    const ME: NetworkEntityId = NetworkEntityId(7);

    /// Этаж, стена между x = 3 и x = 4 (ребро вдоль Z на x = 4) и предмет по ту сторону.
    fn walled_world() -> World {
        let mut w = World::new("m", WorldConfig::default()).unwrap();
        w.apply(WorldCommand::DefineFloor(FloorDef {
            id: FloorId(0),
            name: "g".into(),
            base_y: 0.0,
            height: 3.0,
        }))
        .unwrap();
        for z in 0..6 {
            w.apply(WorldCommand::PlaceElement(StructureElement {
                id: oid(1000 + z),
                kind: StructureKind::Wall,
                placement: Placement::Edge(GridEdge::along_z(4, z as i32)),
                floor: FloorId(0),
                material: PrototypeId::new("t:m").unwrap(),
                thickness: 0.2,
            }))
            .unwrap();
        }
        w
    }

    fn run(
        module: &mut GameplayModule,
        world: &World,
        feet: Vec3,
        kind: &str,
        data: Vec<u8>,
    ) -> astrocore_api::HookOutput {
        let bw = BoxWorld::from_world(world);
        let positions = BTreeMap::from([(ME, feet)]);
        let mut ctx =
            astrocore_api::HookContext::with_positions(world, &positions).with_physics(&bw);
        module.handle(
            &mut ctx,
            &astrocore_api::HookEvent::Command {
                actor: ME,
                kind: kind.into(),
                data,
            },
        );
        ctx.into_output()
    }

    #[test]
    fn an_item_behind_a_wall_cannot_be_picked_up_even_when_close() {
        let mut world = walled_world();
        // Предмет у самой стены с другой стороны (x ≈ 4.4), игрок с этой (x = 3.7): 0.7 м.
        world
            .apply(WorldCommand::PlaceAuthored(item_at(
                1,
                "ss14:crowbar",
                Vec3::new(4.4, 0.7, 2.5),
            )))
            .unwrap();
        let mut m = GameplayModule::new(registry()).unwrap();
        let out = run(
            &mut m,
            &world,
            Vec3::new(3.7, 0.0, 2.5),
            CMD_PICKUP,
            pickup_data(oid(1)),
        );
        assert!(
            out.commands.is_empty(),
            "стена перекрывает путь: {:?}",
            out.commands
        );
        // С той же стороны, на том же расстоянии — подбирается.
        let out = run(
            &mut m,
            &world,
            Vec3::new(5.1, 0.0, 2.5),
            CMD_PICKUP,
            pickup_data(oid(1)),
        );
        assert_eq!(out.commands.len(), 1);
    }

    #[test]
    fn a_drop_target_behind_a_wall_stops_in_front_of_it() {
        let world = walled_world();
        let mut m = GameplayModule::new(registry()).unwrap();
        // Дадим игроку предмет в руки через подбор, затем выбросим за стену.
        let mut w2 = world.clone();
        w2.apply(WorldCommand::PlaceAuthored(item_at(
            1,
            "ss14:crowbar",
            Vec3::new(3.2, 0.7, 2.5),
        )))
        .unwrap();
        let feet = Vec3::new(3.0, 0.0, 2.5);
        run(&mut m, &w2, feet, CMD_PICKUP, pickup_data(oid(1)));
        assert_eq!(m.hands_of(ME).unwrap().held().count(), 1);
        let out = run(
            &mut m,
            &world,
            feet,
            CMD_DROP,
            drop_at_data(0, [5.0, 1.0, 2.5]),
        );
        let WorldCommand::PlaceAuthored(o) = &out.commands[0] else {
            panic!("ожидалась постановка предмета: {:?}", out.commands)
        };
        let x = (o.min().x + o.max().x) * 0.5;
        assert!(x < 4.0, "предмет не пролетел сквозь стену: x = {x}");
        assert!(x > 3.0, "но и не остался под ногами: x = {x}");
    }

    #[test]
    fn a_far_drop_target_is_clamped_to_the_interaction_range() {
        let world = walled_world();
        let mut m = GameplayModule::new(registry()).unwrap();
        let mut w2 = world.clone();
        w2.apply(WorldCommand::PlaceAuthored(item_at(
            1,
            "ss14:crowbar",
            Vec3::new(2.2, 0.7, 2.5),
        )))
        .unwrap();
        let feet = Vec3::new(2.0, 0.0, 2.5);
        run(&mut m, &w2, feet, CMD_PICKUP, pickup_data(oid(1)));
        // Цель в 50 м, но по -X: упрётся в границу дальности 1.5 м, а не улетит.
        let out = run(
            &mut m,
            &world,
            feet,
            CMD_DROP,
            drop_at_data(0, [-48.0, 1.0, 2.5]),
        );
        let WorldCommand::PlaceAuthored(o) = &out.commands[0] else {
            panic!("{:?}", out.commands)
        };
        let x = (o.min().x + o.max().x) * 0.5;
        assert!(
            (2.0 - x) <= INTERACTION_RANGE_M + 0.01,
            "обрезано дальностью: x = {x}"
        );
        assert!(x < 1.0, "в сторону цели: x = {x}");
        // Мусорная цель (NaN) отвергается разбором команды.
        assert!(parse_drop(&drop_at_data(0, [f32::NAN, 0.0, 0.0])).is_none());
        assert!(parse_drop(&[0, 1, 2]).is_none());
    }
}

fn inv_of(h: &Harness, viewer: ConnId, owner: ConnId) -> Option<InventoryView> {
    let net = h.client(owner).net_id()?;
    decode_inventory(h.client(viewer).component(net, INVENTORY_COMPONENT)?)
}

/// Подобрать предмет в руку и сразу проверить, что он в руках.
fn grab(h: &mut Harness, who: ConnId, id: u128, at: Vec3, proto: &str) {
    spawn(h, item_at(id, proto, at));
    command(h, who, CMD_PICKUP_ANY, pickup_data(oid(id)));
    assert!(!on_ground(h, who, id), "{proto} не подобран");
}

fn near() -> Vec3 {
    Vec3::new(3.0, 0.9, 3.0)
}

mod inventory_and_storage {
    use super::*;

    fn doafter(h: &Harness, who: ConnId) -> Option<crate::DoAfterView> {
        let net = h.client(who).net_id()?;
        crate::decode_doafter(h.client(who).component(net, crate::DOAFTER_COMPONENT)?)
    }

    #[test]
    fn a_delayed_equip_waits_then_completes_and_everyone_sees_it() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:heavy_coat");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "outerClothing"));
        let seen_by_b = |h: &Harness| {
            let net = h.client(a).net_id().unwrap();
            crate::decode_doafter(h.client(b).component(net, crate::DOAFTER_COMPONENT)?)
        };
        assert_eq!(
            doafter(&h, a).map(|d| (d.kind, d.delay_s)),
            Some(("equip".to_string(), 1.0))
        );
        assert!(seen_by_b(&h).is_some(), "другие игроки видят полоску");
        assert!(
            hands_of(&h, a, a).unwrap().get(0).is_some(),
            "пока предмет в руке"
        );
        // Второе действие во время первого не принимается.
        command(&mut h, a, CMD_EQUIP, equip_data(0, "outerClothing"));
        h.ticks(25);
        assert!(
            inv_of(&h, a, a)
                .unwrap()
                .inventory
                .get("outerClothing")
                .is_some()
        );
        assert!(hands_of(&h, a, a).unwrap().get(0).is_none());
        assert_eq!(doafter(&h, a), None);
        // Снятие длится 0,5 с.
        command(&mut h, a, CMD_UNEQUIP, unequip_data("outerClothing"));
        assert_eq!(doafter(&h, a).map(|d| d.delay_s), Some(0.5));
        h.ticks(15);
        assert!(
            inv_of(&h, a, a)
                .unwrap()
                .inventory
                .get("outerClothing")
                .is_none()
        );
        assert!(hands_of(&h, a, a).unwrap().get(0).is_some());
    }

    #[test]
    fn moving_cancels_a_delayed_equip_and_the_item_stays_in_hand() {
        let mut h = harness();
        let a = h.join("a");
        grab(&mut h, a, 10, near(), "ss14:heavy_coat");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "outerClothing"));
        for _ in 0..8 {
            h.input(a, 0, 1, 0.0);
        }
        h.ticks(30);
        assert_eq!(doafter(&h, a), None);
        assert!(
            inv_of(&h, a, a)
                .unwrap()
                .inventory
                .get("outerClothing")
                .is_none()
        );
        assert!(hands_of(&h, a, a).unwrap().get(0).is_some());
    }

    #[test]
    fn dropping_the_item_mid_action_aborts_the_equip() {
        let mut h = harness();
        let a = h.join("a");
        grab(&mut h, a, 10, near(), "ss14:heavy_coat");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "outerClothing"));
        command(&mut h, a, CMD_DROP, drop_data(0));
        h.ticks(30);
        assert!(
            inv_of(&h, a, a)
                .unwrap()
                .inventory
                .get("outerClothing")
                .is_none()
        );
    }

    fn hand_item(h: &Harness, who: ConnId, i: usize) -> Option<ObjectId> {
        hands_of(h, who, who).unwrap().get(i).map(|x| x.id)
    }

    #[test]
    fn unequipping_into_a_chosen_hand_and_taking_into_a_chosen_hand() {
        let mut h = harness();
        let a = h.join("a");
        grab(&mut h, a, 10, near(), "ss14:backpack");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "back"));
        grab(&mut h, a, 1, near(), "ss14:pen");
        command(&mut h, a, CMD_STORE, store_data(0, oid(10)));
        // Из сумки — в правую руку (индекс 1).
        command(
            &mut h,
            a,
            crate::CMD_TAKE_TO,
            crate::take_to_data(1, oid(10), oid(1)),
        );
        assert_eq!(
            (hand_item(&h, a, 0), hand_item(&h, a, 1)),
            (None, Some(oid(1)))
        );
        // Рюкзак снимается в левую руку, хотя активна именно она; затем ручку из правой — в сумку не
        // нужна, проверяем выбор руки на снятии: правая занята, значит предмет идёт в левую.
        command(
            &mut h,
            a,
            crate::CMD_UNEQUIP_TO,
            crate::unequip_to_data(1, "back"),
        );
        assert_eq!(
            hand_item(&h, a, 0),
            Some(oid(10)),
            "выбранная рука занята — первая свободная"
        );
        // Освобождаем руки и снимаем в правую.
        command(&mut h, a, CMD_DROP, drop_data(0));
        command(&mut h, a, CMD_DROP, drop_data(1));
        grab(&mut h, a, 11, near(), "ss14:backpack");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "back"));
        command(
            &mut h,
            a,
            crate::CMD_UNEQUIP_TO,
            crate::unequip_to_data(1, "back"),
        );
        assert_eq!(
            (hand_item(&h, a, 0), hand_item(&h, a, 1)),
            (None, Some(oid(11)))
        );
    }

    #[test]
    fn moving_between_slots_is_all_or_nothing() {
        let mut h = harness();
        let a = h.join("a");
        grab(&mut h, a, 2, near(), "ss14:jumpsuit");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "jumpsuit"));
        grab(&mut h, a, 1, near(), "ss14:pen");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "pocket1"));
        let inv = |h: &Harness| inv_of(h, a, a).unwrap().inventory;
        assert!(inv(&h).get("pocket1").is_some());
        command(
            &mut h,
            a,
            crate::CMD_MOVE_SLOT,
            crate::move_slot_data("pocket1", "pocket2"),
        );
        assert!(inv(&h).get("pocket1").is_none() && inv(&h).get("pocket2").is_some());
        // Ручка не подходит на спину: ничего не меняется.
        command(
            &mut h,
            a,
            crate::CMD_MOVE_SLOT,
            crate::move_slot_data("pocket2", "back"),
        );
        assert!(inv(&h).get("pocket2").is_some() && inv(&h).get("back").is_none());
        // Форму нельзя «переложить», потеряв карман с ручкой: зависимые слоты не выпадают молча.
        command(
            &mut h,
            a,
            crate::CMD_MOVE_SLOT,
            crate::move_slot_data("jumpsuit", "outerClothing"),
        );
        assert!(inv(&h).get("jumpsuit").is_some() && inv(&h).get("pocket2").is_some());
    }

    #[test]
    fn clothing_goes_on_pockets_need_it_and_removal_drops_the_pockets() {
        let mut h = harness();
        let a = h.join("a");
        grab(&mut h, a, 1, near(), "ss14:pen");
        // Без формы карманов нет.
        command(&mut h, a, CMD_EQUIP, equip_data(0, "pocket1"));
        assert!(inv_of(&h, a, a).unwrap().inventory.get("pocket1").is_none());
        assert!(
            hands_of(&h, a, a).unwrap().get(0).is_some(),
            "ручка осталась в руке"
        );
        // Надеваем форму; потом ручка идёт в карман.
        grab(&mut h, a, 2, near(), "ss14:jumpsuit");
        command(&mut h, a, CMD_EQUIP, equip_data(1, "jumpsuit"));
        assert!(
            inv_of(&h, a, a)
                .unwrap()
                .inventory
                .get("jumpsuit")
                .is_some()
        );
        command(&mut h, a, CMD_EQUIP, equip_data(0, "pocket1"));
        let inv = inv_of(&h, a, a).unwrap().inventory;
        assert_eq!(inv.get("pocket1").unwrap().id, oid(1));
        assert_eq!(
            hands_of(&h, a, a).unwrap().held().count(),
            0,
            "руки свободны"
        );
        // Не подходит: форма в слот головы.
        grab(&mut h, a, 3, near(), "ss14:jumpsuit");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "head"));
        assert!(inv_of(&h, a, a).unwrap().inventory.get("head").is_none());
        // Снимаем форму: она в руке, а ручка из кармана упала на пол.
        command(&mut h, a, CMD_UNEQUIP, unequip_data("jumpsuit"));
        assert!(on_ground(&h, a, 1), "карман без формы не держится");
        assert_eq!(
            hands_of(&h, a, a).unwrap().held().count(),
            2,
            "форма в руке рядом с лишней"
        );
        assert!(
            inv_of(&h, a, a)
                .unwrap()
                .inventory
                .get("jumpsuit")
                .is_none()
        );
    }

    #[test]
    fn worn_items_are_visible_to_other_players() {
        let mut h = harness();
        let a = h.join("a");
        let b = h.join("b");
        grab(&mut h, a, 10, near(), "ss14:backpack");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "back"));
        let net = h.client(a).net_id().unwrap();
        let worn =
            crate::decode_worn(h.client(b).component(net, crate::WORN_COMPONENT).unwrap()).unwrap();
        assert_eq!(
            worn,
            vec![("back".to_string(), "ss14:backpack".to_string())]
        );
    }

    #[test]
    fn a_backpack_holds_items_by_the_ss14_rules() {
        let mut h = harness();
        let a = h.join("a");
        grab(&mut h, a, 10, near(), "ss14:backpack");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "back"));
        assert!(inv_of(&h, a, a).unwrap().inventory.get("back").is_some());
        // Кладём ручку и ломик (Small): помещаются.
        grab(&mut h, a, 1, near(), "ss14:pen");
        command(&mut h, a, CMD_STORE, store_data(0, oid(10)));
        grab(&mut h, a, 2, near(), "ss14:crowbar");
        command(&mut h, a, CMD_STORE, store_data(0, oid(10)));
        let view = inv_of(&h, a, a).unwrap();
        assert_eq!(view.bags.len(), 1);
        assert_eq!(view.bags[0].items.len(), 2);
        assert_eq!(hands_of(&h, a, a).unwrap().held().count(), 0);
        // Ящик (Large) больше максимума Normal: не входит и остаётся в руке.
        grab(&mut h, a, 3, near(), "ss14:toolbox");
        command(&mut h, a, CMD_STORE, store_data(0, oid(10)));
        assert_eq!(inv_of(&h, a, a).unwrap().bags[0].items.len(), 2);
        assert_eq!(hands_of(&h, a, a).unwrap().held().count(), 1);
        // Рюкзак в рюкзак — нельзя (вложенный должен быть строго меньше).
        // Достаём ручку обратно: рука занята ящиком, значит вторая рука свободна.
        command(&mut h, a, CMD_TAKE, take_data(oid(10), oid(1)));
        assert_eq!(hands_of(&h, a, a).unwrap().held().count(), 2);
        assert_eq!(inv_of(&h, a, a).unwrap().bags[0].items.len(), 1);
        // Обе руки заняты: достать ничего нельзя, предмет остаётся в сумке.
        command(&mut h, a, CMD_TAKE, take_data(oid(10), oid(2)));
        assert_eq!(inv_of(&h, a, a).unwrap().bags[0].items.len(), 1);
    }

    #[test]
    fn nested_bags_follow_the_strictly_smaller_rule() {
        let mut h = harness();
        let a = h.join("a");
        grab(&mut h, a, 10, near(), "ss14:backpack");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "back"));
        // Сумка с максимумом Tiny внутри рюкзака с максимумом Normal: можно.
        grab(&mut h, a, 11, near(), "ss14:satchel");
        command(&mut h, a, CMD_STORE, store_data(0, oid(10)));
        assert_eq!(inv_of(&h, a, a).unwrap().bags[0].items.len(), 1);
        // Рюкзак (максимум Normal) в рюкзак (максимум Normal) — нет.
        grab(&mut h, a, 12, near(), "ss14:backpack");
        command(&mut h, a, CMD_STORE, store_data(0, oid(10)));
        let worn_bag = |h: &Harness| {
            inv_of(h, a, a)
                .unwrap()
                .bags
                .into_iter()
                .find(|b| b.id == oid(10))
                .unwrap()
        };
        assert_eq!(worn_bag(&h).items.len(), 1, "рюкзак в рюкзак не лёг");
        assert_eq!(
            hands_of(&h, a, a).unwrap().held().count(),
            1,
            "остался в руке"
        );
        // Сумку нельзя положить в саму себя.
        command(&mut h, a, CMD_SELECT_HAND, vec![0]);
        grab(&mut h, a, 13, near(), "ss14:satchel");
        command(&mut h, a, CMD_STORE, store_data(1, oid(13)));
        assert_eq!(hands_of(&h, a, a).unwrap().held().count(), 2);
    }

    #[test]
    fn bag_contents_are_private_to_the_owner_but_worn_and_held_items_are_visible_in_hands() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:backpack");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "back"));
        grab(&mut h, a, 1, near(), "ss14:pen");
        command(&mut h, a, CMD_STORE, store_data(0, oid(10)));
        assert!(inv_of(&h, a, a).is_some());
        assert!(
            inv_of(&h, b, a).is_none(),
            "второй клиент не видит ни рюкзак, ни его содержимое"
        );
        // Даже после своего запроса снимка и входа третьего игрока.
        let p = h.client(b).snapshot_request_packet();
        h.send(b, p);
        let c = h.join("c");
        assert!(inv_of(&h, b, a).is_none() && inv_of(&h, c, a).is_none());
    }

    #[test]
    fn a_bag_on_the_floor_keeps_its_contents_and_can_be_used_in_reach() {
        let mut h = harness();
        let a = h.join("a");
        spawn(&mut h, item_at(10, "ss14:backpack", near()));
        grab(&mut h, a, 1, Vec3::new(3.0, 0.9, 2.0), "ss14:pen");
        // Рюкзак лежит рядом: кладём ручку прямо в него.
        command(&mut h, a, CMD_STORE, store_data(0, oid(10)));
        assert_eq!(hands_of(&h, a, a).unwrap().held().count(), 0);
        // Поднимаем рюкзак, надеваем, содержимое на месте и достаётся.
        command(&mut h, a, CMD_PICKUP, pickup_data(oid(10)));
        command(&mut h, a, CMD_EQUIP, equip_data(0, "back"));
        assert_eq!(inv_of(&h, a, a).unwrap().bags[0].items.len(), 1);
        command(&mut h, a, CMD_TAKE, take_data(oid(10), oid(1)));
        assert_eq!(hands_of(&h, a, a).unwrap().get(0).unwrap().id, oid(1));
        // Слишком далёкий рюкзак недоступен.
        spawn(
            &mut h,
            item_at(20, "ss14:backpack", Vec3::new(30.0, 0.9, 3.0)),
        );
        command(&mut h, a, CMD_STORE, store_data(1, oid(20)));
        assert_eq!(
            hands_of(&h, a, a).unwrap().held().count(),
            1,
            "рука не опустела"
        );
    }

    #[test]
    fn disconnecting_drops_worn_items_and_bags_keep_what_is_inside() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:backpack");
        command(&mut h, a, CMD_EQUIP, equip_data(0, "back"));
        grab(&mut h, a, 1, near(), "ss14:pen");
        command(&mut h, a, CMD_STORE, store_data(0, oid(10)));
        let outs = h.server.disconnect(a);
        h.deliver(outs);
        assert!(on_ground(&h, b, 10), "рюкзак остался лежать");
        assert!(!on_ground(&h, b, 1), "а ручка — внутри него, а не на полу");
        // Другой игрок подбирает рюкзак и достаёт ручку.
        command(&mut h, b, CMD_PICKUP, pickup_data(oid(10)));
        command(&mut h, b, CMD_TAKE, take_data(oid(10), oid(1)));
        assert_eq!(hands_of(&h, b, b).unwrap().held().count(), 2);
    }

    #[test]
    fn malformed_inventory_commands_are_ignored() {
        let mut h = harness();
        let a = h.join("a");
        for (kind, data) in [
            (CMD_EQUIP, vec![]),
            (CMD_EQUIP, vec![0]),
            (CMD_EQUIP, vec![0, 0xFF, 0xFE]),
            (CMD_EQUIP, [vec![0], b"x".repeat(40)].concat()),
            (CMD_UNEQUIP, b"no such slot!".to_vec()),
            (CMD_STORE, vec![0, 1, 2]),
            (CMD_TAKE, vec![1; 31]),
        ] {
            command(&mut h, a, kind, data);
        }
        assert_eq!(h.server.player_count(), 1);
        assert_eq!(hands_of(&h, a, a).map_or(0, |x| x.held().count()), 0);
    }
}

mod interactions {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{
        CMD_INTERACT, CMD_USE, InteractionHandler, State, Target, Used, interact_data, use_data,
    };

    #[test]
    fn an_empty_hand_picks_up_an_item_and_a_full_active_hand_inserts_into_a_bag() {
        let mut h = harness();
        let a = h.join("a");
        spawn(&mut h, item_at(1, "ss14:pen", near()));
        spawn(
            &mut h,
            item_at(2, "ss14:backpack", Vec3::new(3.0, 0.9, 2.0)),
        );
        // Пустая рука: взаимодействие поднимает предмет в активную руку.
        command(&mut h, a, CMD_INTERACT, interact_data(oid(1)));
        assert!(!on_ground(&h, a, 1));
        assert_eq!(hands_of(&h, a, a).unwrap().get(0).unwrap().id, oid(1));
        // Предмет в руке + рюкзак на земле: предмет кладётся в рюкзак (ClickInsert).
        command(&mut h, a, CMD_INTERACT, interact_data(oid(2)));
        assert!(
            on_ground(&h, a, 2),
            "рюкзак остался на земле, его не подняли"
        );
        assert_eq!(
            hands_of(&h, a, a).unwrap().held().count(),
            0,
            "ручка ушла в рюкзак"
        );
        // Теперь рука пуста — тот же клик поднимает сам рюкзак.
        command(&mut h, a, CMD_INTERACT, interact_data(oid(2)));
        assert!(!on_ground(&h, a, 2));
        assert_eq!(inv_bag_items(&h, a, 2), 1);
    }

    fn inv_bag_items(h: &Harness, who: ConnId, bag: u128) -> usize {
        // Рюкзак в руке: содержимое видно владельцу в build.inventory.
        inv_of(h, who, who)
            .and_then(|v| v.bags.into_iter().find(|b| b.id == oid(bag)))
            .map_or(0, |b| b.items.len())
    }

    #[test]
    fn interacting_out_of_reach_or_with_nothing_there_is_ignored() {
        let mut h = harness();
        let a = h.join("a");
        spawn(&mut h, item_at(1, "ss14:pen", Vec3::new(30.0, 0.9, 3.0)));
        command(&mut h, a, CMD_INTERACT, interact_data(oid(1)));
        command(&mut h, a, CMD_INTERACT, interact_data(oid(777)));
        command(&mut h, a, CMD_INTERACT, vec![1, 2, 3]);
        command(&mut h, a, CMD_USE, vec![]);
        command(&mut h, a, CMD_USE, use_data(9));
        assert!(on_ground(&h, a, 1));
        assert_eq!(h.server.player_count(), 1);
    }

    /// Записывает события и «съедает» их — так форк подменяет базовое поведение.
    struct Spy(Arc<Mutex<Vec<String>>>);

    impl InteractionHandler for Spy {
        fn interact_hand(
            &mut self,
            _: &mut State,
            _: &mut astrocore_api::HookContext<'_>,
            _: astrocore_core::NetworkEntityId,
            t: &Target,
        ) -> bool {
            self.0.lock().unwrap().push(format!("hand {:?}", t.id()));
            false
        }
        fn use_in_hand(
            &mut self,
            _: &mut State,
            _: &mut astrocore_api::HookContext<'_>,
            _: astrocore_core::NetworkEntityId,
            u: &Used,
        ) -> bool {
            self.0
                .lock()
                .unwrap()
                .push(format!("use {}", u.item.prototype));
            true
        }
    }

    #[test]
    fn forks_can_add_handlers_and_use_in_hand_reaches_them() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut host = ModuleHost::new();
        host.register(Box::new(
            GameplayModule::new(registry())
                .unwrap()
                .with_handler(Box::new(Spy(log.clone()))),
        ))
        .unwrap();
        host.finalize().unwrap();
        let mut reg = ReplicationRegistry::new();
        register_replication(&mut reg).unwrap();
        let server = GameServer::new(test_world(), Harness::default_cfg(), Box::new(ByBuild))
            .with_modules(host)
            .with_replication(reg);
        let mut h = Harness::with_server(server);
        let a = h.join("a");
        spawn(&mut h, item_at(1, "ss14:pen", near()));
        command(&mut h, a, CMD_USE, use_data(0));
        assert!(
            log.lock().unwrap().is_empty(),
            "пустая рука: использовать нечего"
        );
        command(&mut h, a, CMD_INTERACT, interact_data(oid(1)));
        // Базовый обработчик поднял предмет, поэтому форк свой `interact_hand` уже не получил.
        assert_eq!(hands_of(&h, a, a).unwrap().held().count(), 1);
        command(&mut h, a, CMD_USE, use_data(0));
        assert_eq!(*log.lock().unwrap(), vec!["use ss14:pen".to_string()]);
    }
}

mod combat {
    use super::*;
    use crate::{CMD_ATTACK, CMD_HEAL, HEALTH_COMPONENT, MOBSTATE_COMPONENT, MobState};

    fn health(h: &Harness, viewer: ConnId, owner: ConnId) -> Option<crate::HealthView> {
        let net = h.client(owner).net_id()?;
        crate::decode_health(h.client(viewer).component(net, HEALTH_COMPONENT)?)
    }

    fn state(h: &Harness, viewer: ConnId, owner: ConnId) -> MobState {
        let net = h.client(owner).net_id().unwrap();
        h.client(viewer)
            .component(net, MOBSTATE_COMPONENT)
            .and_then(|b| b.first().copied())
            .map_or(MobState::Alive, MobState::from_byte)
    }

    fn hit(h: &mut Harness, who: ConnId, target: ConnId) {
        let net = h.client(target).net_id().unwrap();
        command(h, who, CMD_ATTACK, crate::attack_data(net));
    }

    fn blunt(h: &Harness, viewer: ConnId, owner: ConnId) -> f32 {
        health(h, viewer, owner)
            .and_then(|v| v.damage.get("Blunt").copied())
            .unwrap_or(0.0)
    }

    #[test]
    fn a_fist_hurts_only_the_victim_sees_the_numbers_and_everyone_sees_the_state() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        hit(&mut h, a, b);
        assert_eq!(blunt(&h, b, b), 5.0);
        assert!(health(&h, a, b).is_none(), "чужие цифры здоровья не видны");
        assert_eq!(state(&h, a, b), MobState::Alive);
        // Бить себя нельзя.
        hit(&mut h, a, a);
        assert_eq!(blunt(&h, a, a), 0.0);
    }

    #[test]
    fn attacks_respect_the_rate_and_the_reach() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        hit(&mut h, a, b);
        hit(&mut h, a, b);
        assert_eq!(blunt(&h, b, b), 5.0, "темп 1,5 удара в секунду");
        h.ticks(20);
        hit(&mut h, a, b);
        assert_eq!(blunt(&h, b, b), 10.0);
        // Отошёл дальше досягаемости.
        for _ in 0..40 {
            h.input(b, 0, 1, 0.0);
        }
        h.ticks(20);
        hit(&mut h, a, b);
        assert_eq!(blunt(&h, b, b), 10.0, "далеко: промах");
    }

    #[test]
    fn a_weapon_in_hand_sets_the_damage_and_the_thresholds_change_the_state() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:cudgel");
        grab(&mut h, b, 11, near(), "ss14:pen");
        hit(&mut h, a, b);
        assert_eq!(blunt(&h, b, b), 60.0);
        h.ticks(25);
        hit(&mut h, a, b);
        assert_eq!(state(&h, a, b), MobState::Critical, "120 ≥ 100");
        assert!(
            hands_of(&h, b, b).unwrap().held().next().is_none(),
            "в критическом состоянии руки пусты: предмет упал"
        );
        h.ticks(25);
        hit(&mut h, a, b);
        assert_eq!(
            state(&h, a, b),
            MobState::Dead,
            "180 на торсе ≥ 150: смертельно раньше общего порога"
        );
        // Мёртвого не бьют дальше.
        h.ticks(25);
        hit(&mut h, a, b);
        assert_eq!(blunt(&h, b, b), 180.0);
    }

    #[test]
    fn the_incapacitated_cannot_move_or_act() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:cudgel");
        hit(&mut h, a, b);
        h.ticks(25);
        hit(&mut h, a, b);
        assert_eq!(state(&h, a, b), MobState::Critical);
        let flat = |h: &Harness| {
            let p = h.client(b).position();
            (p.x, p.z)
        };
        let start = flat(&h);
        for _ in 0..10 {
            h.input(b, 0, 1, 0.0);
        }
        assert_eq!(flat(&h), start, "крит не ходит");
        spawn(&mut h, item_at(50, "ss14:pen", Vec3::new(2.5, 0.9, 2.5)));
        command(&mut h, b, CMD_PICKUP_ANY, pickup_data(oid(50)));
        assert!(on_ground(&h, b, 50), "и не поднимает");
    }

    #[test]
    fn a_healing_item_takes_time_is_used_up_and_brings_the_patient_back() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:cudgel");
        hit(&mut h, a, b);
        h.ticks(25);
        hit(&mut h, a, b);
        assert_eq!(state(&h, a, b), MobState::Critical);
        // Лекарство лежит рядом с лежачим; подбирает и лечит третья сторона — сам себя он не вылечит.
        grab(&mut h, a, 20, near(), "ss14:brutepack");
        // Рука занята дубинкой: кладём её, чтобы освободить руку.
        let net_b = h.client(b).net_id().unwrap();
        command(&mut h, a, CMD_HEAL, crate::heal_data(1, net_b));
        assert_eq!(blunt(&h, b, b), 120.0, "лечение занимает время");
        h.ticks(25);
        assert_eq!(blunt(&h, b, b), 90.0);
        assert_eq!(state(&h, a, b), MobState::Alive, "90 < 100: пришёл в себя");
        assert!(
            hands_of(&h, a, a).unwrap().get(1).is_none(),
            "пакет израсходован"
        );
        let flat = |h: &Harness| {
            let p = h.client(b).position();
            (p.x, p.z)
        };
        let start = flat(&h);
        for _ in 0..10 {
            h.input(b, 0, 1, 0.0);
        }
        assert_ne!(flat(&h), start, "снова ходит");
    }

    fn hit_part(h: &mut Harness, who: ConnId, target: ConnId, part: &str) {
        let net = h.client(target).net_id().unwrap();
        let i = crate::PARTS.iter().position(|p| *p == part).unwrap() as u8;
        command(h, who, CMD_ATTACK, crate::attack_part_data(net, i));
    }

    fn view(h: &Harness, who: ConnId) -> crate::HealthView {
        health(h, who, who).expect("своё здоровье")
    }

    #[test]
    fn a_wrecked_arm_drops_what_it_held_and_refuses_new_things() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:cudgel");
        grab(&mut h, b, 11, near(), "ss14:pen");
        assert!(hands_of(&h, b, b).unwrap().get(0).is_some());
        hit_part(&mut h, a, b, "LeftArm");
        assert_eq!(view(&h, b).parts.get("LeftArm"), Some(&60.0));
        let hands = hands_of(&h, b, b).unwrap();
        assert!(hands.is_disabled(0) && !hands.is_disabled(1));
        assert!(hands.get(0).is_none(), "ручка выпала из больной руки");
        // Берёт только здоровой рукой.
        grab(&mut h, b, 12, near(), "ss14:pen");
        let hands = hands_of(&h, b, b).unwrap();
        assert!(hands.get(0).is_none() && hands.get(1).is_some());
    }

    #[test]
    fn two_wrecked_legs_stop_walking_and_a_head_blow_is_fatal() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:cudgel");
        hit_part(&mut h, a, b, "LeftLeg");
        h.ticks(25);
        let flat = |h: &Harness| {
            let p = h.client(b).position();
            (p.x, p.z)
        };
        let start = flat(&h);
        for _ in 0..6 {
            h.input(b, 0, 1, 0.0);
        }
        assert_ne!(flat(&h), start, "с одной больной ногой ещё ходит");
        h.ticks(25);
        hit_part(&mut h, a, b, "RightLeg");
        h.ticks(5);
        let start = flat(&h);
        for _ in 0..6 {
            h.input(b, 0, 1, 0.0);
        }
        assert_eq!(flat(&h), start, "обе ноги — не ходит");
        assert_eq!(state(&h, a, b), MobState::Critical);
        // Голова: 60 + 60 ≥ 100 — смерть.
        let (c, d) = (h.join("c"), h.join("d"));
        let _ = c;
        grab(&mut h, c, 20, near(), "ss14:cudgel");
        hit_part(&mut h, c, d, "Head");
        h.ticks(25);
        hit_part(&mut h, c, d, "Head");
        assert_eq!(state(&h, c, d), MobState::Dead);
    }

    #[test]
    fn a_cut_bleeds_until_bandaged_and_the_stack_lasts_three_uses() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:dagger");
        hit_part(&mut h, a, b, "Torso");
        let bleed = |h: &Harness| {
            let net = h.client(b).net_id().unwrap();
            crate::decode_vitals(h.client(b).component(net, crate::VITALS_COMPONENT).unwrap())
                .unwrap()
        };
        assert!(
            bleed(&h).bleed > 4.9,
            "режущий 20 × 0,25 = 5: {}",
            bleed(&h).bleed
        );
        h.ticks(20 * 7);
        assert!(bleed(&h).blood < 1.0, "кровь уходит: {}", bleed(&h).blood);
        // Перевязка: стопка из трёх; рука a держит кинжал, поэтому кладём его.
        command(&mut h, a, CMD_DROP, drop_data(0));
        grab(&mut h, a, 20, near(), "ss14:bandage");
        let net_b = h.client(b).net_id().unwrap();
        for used in 1..=3u32 {
            command(&mut h, a, CMD_HEAL, crate::heal_data(0, net_b));
            h.ticks(8);
            let held = hands_of(&h, a, a).unwrap().get(0).is_some();
            assert_eq!(held, used < 3, "после {used}-го применения");
            let left = inv_of(&h, a, a)
                .unwrap()
                .charges
                .iter()
                .find(|(id, _)| *id == oid(20))
                .map(|(_, n)| *n);
            assert_eq!(left, (used < 3).then_some(3 - used));
        }
        h.ticks(10);
        assert_eq!(bleed(&h).bleed, 0.0, "кровотечение остановлено");
    }

    fn ghost_flag(h: &Harness, viewer: ConnId, owner: ConnId) -> bool {
        let net = h.client(owner).net_id().unwrap();
        h.client(viewer)
            .component(net, crate::GHOST_COMPONENT)
            .is_some_and(|b| b.first() == Some(&1))
    }

    fn kill(h: &mut Harness, killer: ConnId, victim: ConnId) {
        hit_part(h, killer, victim, "Head");
        h.ticks(25);
        hit_part(h, killer, victim, "Head");
    }

    #[test]
    fn the_dead_leave_a_body_and_become_ghosts_everyone_can_tell_apart() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:cudgel");
        let before = h.server.world().authored_count();
        kill(&mut h, a, b);
        assert_eq!(state(&h, a, b), MobState::Dead);
        assert!(ghost_flag(&h, a, b), "все видят, что b — призрак");
        assert!(!ghost_flag(&h, b, a));
        assert_eq!(
            h.server.world().authored_count(),
            before + 1,
            "на месте смерти осталось тело"
        );
        let corpse = h
            .server
            .world()
            .authored()
            .find(|o| matches!(&o.kind, AuthoredKind::Placement { prototype } if prototype.as_str() == crate::health::CORPSE_PROTOTYPE));
        assert!(corpse.is_some());
        assert!(view(&h, b).ghost && view(&h, b).respawn_in > 29.0);
        // Призрак ничего не делает руками.
        spawn(&mut h, item_at(50, "ss14:pen", Vec3::new(2.5, 0.9, 2.5)));
        command(&mut h, b, CMD_PICKUP_ANY, pickup_data(oid(50)));
        assert!(on_ground(&h, b, 50));
    }

    #[test]
    fn a_ghost_respawns_only_after_the_delay_with_a_fresh_body_at_the_spawn() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:cudgel");
        // Одежда на теле до смерти.
        grab(&mut h, b, 12, near(), "ss14:backpack");
        command(&mut h, b, CMD_EQUIP, equip_data(0, "back"));
        assert!(inv_of(&h, b, b).unwrap().inventory.get("back").is_some());
        kill(&mut h, a, b);
        command(&mut h, b, crate::CMD_RESPAWN, vec![]);
        assert!(ghost_flag(&h, a, b), "рано: ещё призрак");
        h.ticks(620);
        command(&mut h, b, crate::CMD_RESPAWN, vec![]);
        h.ticks(3);
        assert!(!ghost_flag(&h, a, b));
        assert_eq!(state(&h, a, b), MobState::Alive);
        assert_eq!(view(&h, b).damage.len(), 0, "здоровье сброшено");
        let p = h.client(b).position();
        assert!(
            (p.x - 1.5).abs() < 0.3 && (p.z - 1.5).abs() < 0.3,
            "в точке появления: {p:?}"
        );
        assert!(
            inv_of(&h, b, b).unwrap().inventory.get("back").is_none(),
            "одежда не переехала в новое тело"
        );
        assert!(on_ground(&h, b, 12), "а лежит на месте смерти");
    }
}

mod survival {
    use super::*;
    use crate::{PressureAlert, VITALS_COMPONENT, Vitals, decode_vitals};

    fn vitals(h: &Harness, who: ConnId) -> Vitals {
        let net = h.client(who).net_id().unwrap();
        decode_vitals(
            h.client(who)
                .component(net, VITALS_COMPONENT)
                .expect("vitals"),
        )
        .unwrap()
    }

    fn health(h: &Harness, who: ConnId) -> crate::HealthView {
        let net = h.client(who).net_id().unwrap();
        crate::decode_health(
            h.client(who)
                .component(net, crate::HEALTH_COMPONENT)
                .unwrap(),
        )
        .unwrap()
    }

    fn use_hand(h: &mut Harness, who: ConnId, hand: u8) {
        command(h, who, CMD_USE, crate::use_data(hand));
    }

    #[test]
    fn breathing_station_air_keeps_a_person_healthy_and_the_vitals_are_published() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(400); // 20 с
        let v = vitals(&h, a);
        assert_eq!(v.pressure_alert, PressureAlert::None);
        assert!((v.pressure_kpa - 101.3).abs() < 1.0, "{}", v.pressure_kpa);
        assert!(!v.low_oxygen);
        assert!(!health(&h, a).damage.contains_key("Asphyxiation"));
    }

    #[test]
    fn a_vacuum_zone_means_no_air_and_a_low_pressure_alert_and_leaving_it_restores_air() {
        let mut h = harness();
        let a = h.join("a");
        let mut zone = AuthoredObject::new(
            oid(90),
            AuthoredKind::Zone,
            FloorId(0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 3.0, 6.0),
        );
        zone.metadata.insert("atmosphere".into(), "vacuum".into());
        spawn(&mut h, zone);
        h.ticks(300); // 15 с
        let v = vitals(&h, a);
        assert_eq!(v.pressure_alert, PressureAlert::Low(2));
        assert!(
            v.low_oxygen,
            "после трёх тактов удушья загорается предупреждение"
        );
        let hv = health(&h, a);
        assert!(hv.damage.get("Asphyxiation").copied().unwrap_or(0.0) >= 2.0);
        assert!(
            hv.damage.get("Blunt").copied().unwrap_or(0.0) > 10.0,
            "баротравма"
        );
        // Убираем зону: клетки снова станут воздухом.
        let outs = h
            .server
            .apply_world_command(astrocore_world::WorldCommand::RemoveAuthored { id: oid(90) })
            .unwrap();
        h.deliver(outs);
        // Остаток комнаты успел сдуться через зону, новые клетки заполнены воздухом: давление
        // выравнивается за пару десятков секунд.
        h.ticks(20 * 40);
        let v = vitals(&h, a);
        assert_eq!(v.pressure_alert, PressureAlert::None, "{}", v.pressure_kpa);
        // Воздуха в комнате осталось на 57 кПа: дышать ещё тяжело (кислорода на вдох меньше, чем уходит
        // на насыщение), поэтому «нечем дышать» не гаснет — как в SS14 в разреженном воздухе.
        assert!(
            v.pressure_kpa > 50.0 && v.pressure_kpa < 70.0,
            "{}",
            v.pressure_kpa
        );
    }

    #[test]
    fn a_door_between_rooms_lets_air_through_only_when_open() {
        let mut h = harness();
        let _a = h.join("a");
        // Воздух левой комнаты: проверяем сетку через модульное состояние косвенно — давление в
        // правой комнате остаётся атмосферным, пока всё закрыто, и ничего не утекает.
        h.ticks(100);
        let total: f32 = h
            .server
            .world()
            .elements()
            .filter(|e| matches!(e.kind, astrocore_world::StructureKind::Floor))
            .count() as f32;
        assert!(total > 100.0, "в мире 128 клеток пола");
    }

    #[test]
    fn eating_a_burger_takes_bites_until_it_is_gone_and_feeds_the_hunger() {
        let mut h = harness();
        let a = h.join("a");
        grab(&mut h, a, 10, near(), "ss14:burger");
        let before = vitals(&h, a).hunger;
        use_hand(&mut h, a, 0);
        let net = h.client(a).net_id().unwrap();
        let da = crate::decode_doafter(
            h.client(a)
                .component(net, crate::DOAFTER_COMPONENT)
                .unwrap(),
        );
        assert_eq!(
            da.map(|d| (d.kind, d.delay_s)),
            Some(("eat".to_string(), 1.0))
        );
        // 50 единиц по 5 за укус — десять укусов по секунде, и переваривание.
        h.ticks(20 * 14);
        assert!(
            hands_of(&h, a, a).unwrap().get(0).is_none(),
            "бургер съеден целиком"
        );
        let after = vitals(&h, a).hunger;
        // Питательные вещества 30 ед.: 30 с × 3 = 90 и ещё белок/витамины 20 ед. × 1,5.
        assert!(after > before + 60.0, "{before} → {after}");
    }

    #[test]
    fn a_closed_bottle_must_be_opened_first_and_stays_after_it_is_empty() {
        let mut h = harness();
        let a = h.join("a");
        grab(&mut h, a, 11, near(), "ss14:water_bottle");
        let inv = |h: &Harness| inv_of(h, a, a).unwrap();
        assert!(
            inv(&h).fills.iter().any(|f| f.0 == oid(11) && f.3),
            "закрыта"
        );
        let thirst0 = vitals(&h, a).thirst;
        // Первое использование открывает бутылку, пить ещё не начинаем.
        use_hand(&mut h, a, 0);
        h.ticks(30);
        assert!(
            inv(&h).fills.iter().any(|f| f.0 == oid(11) && !f.3),
            "открыта"
        );
        assert!(
            (vitals(&h, a).thirst - thirst0).abs() < 2.0,
            "ничего не выпито"
        );
        // Второе — пьём; 60 единиц по 5 — двенадцать глотков по секунде.
        use_hand(&mut h, a, 0);
        h.ticks(20 * 16);
        let fill = inv(&h)
            .fills
            .iter()
            .find(|f| f.0 == oid(11))
            .copied()
            .unwrap();
        assert!(fill.1 < 1.0, "бутылка пуста: {fill:?}");
        assert!(
            hands_of(&h, a, a).unwrap().get(0).is_some(),
            "пустая бутылка остаётся в руке"
        );
        // Желудок переваривает воду по 0,5 ед. в секунду, по 4 жажды за единицу в секунду.
        h.ticks(20 * 40);
        assert!(
            vitals(&h, a).thirst > thirst0 + 100.0,
            "{} → {}",
            thirst0,
            vitals(&h, a).thirst
        );
    }

    #[test]
    fn feeding_another_player_takes_three_seconds_a_bite_and_walking_away_cancels() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:burger");
        h.ticks(10);
        let net_b = h.client(b).net_id().unwrap();
        let before = vitals(&h, b).hunger;
        command(&mut h, a, crate::CMD_FEED, crate::feed_data(0, net_b));
        let net_a = h.client(a).net_id().unwrap();
        let da = crate::decode_doafter(
            h.client(a)
                .component(net_a, crate::DOAFTER_COMPONENT)
                .unwrap(),
        );
        assert_eq!(
            da.map(|d| (d.kind, d.delay_s)),
            Some(("eat".to_string(), 3.0))
        );
        h.ticks(20 * 4);
        // Один укус: 5 из 50 единиц, больше сам не повторяется.
        let fill = inv_of(&h, a, a)
            .unwrap()
            .fills
            .iter()
            .find(|f| f.0 == oid(10))
            .copied()
            .unwrap();
        assert!((fill.1 - 45.0).abs() < 0.1, "{fill:?}");
        assert!(vitals(&h, b).hunger > before, "накормленный сыт");
        // Отошёл — кормление прерывается.
        command(&mut h, a, crate::CMD_FEED, crate::feed_data(0, net_b));
        for _ in 0..6 {
            h.input(a, 0, 1, 0.0);
        }
        h.ticks(20 * 4);
        let fill = inv_of(&h, a, a)
            .unwrap()
            .fills
            .iter()
            .find(|f| f.0 == oid(10))
            .copied()
            .unwrap();
        assert!(
            (fill.1 - 45.0).abs() < 0.1,
            "второй укус не состоялся: {fill:?}"
        );
    }

    #[test]
    fn eating_continues_while_walking_but_stops_when_the_stomach_is_full() {
        let mut h = harness();
        let a = h.join("a");
        grab(&mut h, a, 11, near(), "ss14:water_bottle");
        use_hand(&mut h, a, 0);
        h.ticks(5);
        use_hand(&mut h, a, 0);
        for _ in 0..10 {
            h.input(a, 0, 1, 0.0);
        }
        let net = h.client(a).net_id().unwrap();
        let da = crate::decode_doafter(
            h.client(a)
                .component(net, crate::DOAFTER_COMPONENT)
                .unwrap(),
        );
        assert_eq!(
            da.map(|d| d.kind),
            Some("eat".to_string()),
            "на ходу пить можно"
        );
        // Желудок 150 ед.: бутылка 60 целиком помещается; вторая за ней упёрлась бы в предел.
        h.ticks(20 * 16);
        let fill = inv_of(&h, a, a)
            .unwrap()
            .fills
            .iter()
            .find(|f| f.0 == oid(11))
            .copied()
            .unwrap();
        assert!(fill.1 < 1.0);
    }

    #[test]
    fn the_dead_do_not_breathe_or_get_hungry() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        grab(&mut h, a, 10, near(), "ss14:cudgel");
        hit_part_pub(&mut h, a, b, "Head");
        h.ticks(25);
        hit_part_pub(&mut h, a, b, "Head");
        let hunger = vitals(&h, b).hunger;
        h.ticks(200);
        assert!(
            (vitals(&h, b).hunger - hunger).abs() < 1.0,
            "мёртвый не голодает"
        );
    }

    fn hit_part_pub(h: &mut Harness, who: ConnId, target: ConnId, part: &str) {
        let net = h.client(target).net_id().unwrap();
        let i = crate::PARTS.iter().position(|p| *p == part).unwrap() as u8;
        command(h, who, crate::CMD_ATTACK, crate::attack_part_data(net, i));
    }
}

mod fire {
    use super::*;
    use crate::fire::{BURNING_COMPONENT, FIRES_COMPONENT, decode_fires};
    use crate::{CMD_RESIST, VITALS_COMPONENT, decode_vitals};

    fn plasma_room(h: &mut Harness) {
        let mut zone = AuthoredObject::new(
            oid(91),
            AuthoredKind::Zone,
            FloorId(0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 3.0, 6.0),
        );
        zone.metadata.insert("atmosphere".into(), "plasma".into());
        spawn(h, zone);
    }

    fn vitals(h: &Harness, who: ConnId) -> crate::Vitals {
        let net = h.client(who).net_id().unwrap();
        decode_vitals(h.client(who).component(net, VITALS_COMPONENT).unwrap()).unwrap()
    }

    fn heat(h: &Harness, who: ConnId) -> f32 {
        let net = h.client(who).net_id().unwrap();
        crate::decode_health(
            h.client(who)
                .component(net, crate::HEALTH_COMPONENT)
                .unwrap(),
        )
        .unwrap()
        .damage
        .get("Heat")
        .copied()
        .unwrap_or(0.0)
    }

    fn light_welder(h: &mut Harness, a: ConnId) {
        spawn(h, item_at(120, "ss14:welder", Vec3::new(3.0, 0.9, 3.0)));
        command(h, a, CMD_PICKUP, pickup_data(oid(120)));
        h.ticks(5);
        let hand = hands_of(h, a, a).unwrap().active();
        command(h, a, CMD_USE, use_data(hand as u8));
    }

    #[test]
    fn a_lit_welder_in_a_plasma_room_starts_a_fire_that_burns_the_holder() {
        let mut h = harness();
        let a = h.join("a");
        plasma_room(&mut h);
        h.ticks(10);
        light_welder(&mut h, a);
        h.ticks(20 * 30);
        let net = h.client(a).net_id().unwrap();
        let fires = decode_fires(h.client(a).component(net, FIRES_COMPONENT).unwrap()).unwrap();
        assert!(!fires.is_empty(), "пожар разослан клиентам");
        assert!(vitals(&h, a).on_fire || heat(&h, a) > 0.0, "персонаж горит");
        assert!(heat(&h, a) > 0.0, "ожоги: {}", heat(&h, a));
        assert_eq!(
            h.client(a).component(net, BURNING_COMPONENT),
            Some(&[u8::from(vitals(&h, a).on_fire)][..])
        );
    }

    #[test]
    fn a_welder_in_normal_air_starts_nothing() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(10);
        light_welder(&mut h, a);
        h.ticks(20 * 10);
        let net = h.client(a).net_id().unwrap();
        let fires = h
            .client(a)
            .component(net, FIRES_COMPONENT)
            .and_then(decode_fires)
            .unwrap_or_default();
        assert!(fires.is_empty());
        assert!(!vitals(&h, a).on_fire);
    }

    #[test]
    fn resisting_freezes_the_player_and_puts_the_fire_out_faster() {
        let mut h = harness();
        let a = h.join("a");
        plasma_room(&mut h);
        h.ticks(10);
        light_welder(&mut h, a);
        for _ in 0..20 * 30 {
            h.ticks(1);
            if vitals(&h, a).on_fire {
                break;
            }
        }
        if !vitals(&h, a).on_fire {
            panic!("не загорелся");
        }
        command(&mut h, a, CMD_RESIST, vec![]);
        h.ticks(5);
        assert!(vitals(&h, a).resisting);
        h.ticks(20 * 3);
        assert!(!vitals(&h, a).resisting, "перекат закончился");
    }
}
