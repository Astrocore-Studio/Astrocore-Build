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
    host.register(Box::new(GameplayModule::new(registry()).unwrap()))
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
        item_at(100, "ss14:crowbar", Vec3::new(3.0, 0.5, 3.0)),
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
        item_at(100, "ss14:crowbar", Vec3::new(3.0, 0.5, 3.0)),
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
        item_at(1, "ss14:crowbar", Vec3::new(30.0, 0.5, 3.0)),
    );
    spawn(&mut h, item_at(2, "ss14:boulder", Vec3::new(3.0, 0.5, 3.0)));
    spawn(&mut h, item_at(3, "ss14:crowbar", Vec3::new(3.0, 0.5, 2.0)));
    spawn(&mut h, item_at(4, "ss14:crowbar", Vec3::new(2.0, 0.5, 3.0)));
    spawn(&mut h, item_at(5, "ss14:crowbar", Vec3::new(2.5, 0.5, 3.0)));
    for id in [1u128, 2] {
        command(&mut h, a, CMD_PICKUP, pickup_data(oid(id)));
        assert!(
            on_ground(&h, a, id),
            "{id}: слишком далеко или не предмет — остаётся"
        );
    }
    for id in [3u128, 4, 5] {
        command(&mut h, a, CMD_PICKUP, pickup_data(oid(id)));
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
    spawn(&mut h, item_at(1, "ss14:crowbar", Vec3::new(3.0, 0.5, 3.0)));
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
    spawn(&mut h, item_at(1, "ss14:crowbar", Vec3::new(3.0, 0.5, 3.0)));
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
