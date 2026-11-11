use astrocore_core::ObjectId;
use astrocore_network::testkit::Harness;
use gameplay::{CMD_PICKUP, HANDS_COMPONENT, decode_hands, pickup_data};

use super::*;

fn content() -> std::path::PathBuf {
    polygon::content_dir()
}

fn item0() -> ObjectId {
    ObjectId::from_u128(0x1000_0000)
}

#[test]
fn a_round_starts_with_the_polygon_and_its_items() {
    let r = Round::start(1, &content(), &RoundConfig::default()).unwrap();
    assert_eq!(r.number, 1);
    assert_eq!(r.server.world().id(), "polygon");
    assert_eq!(
        r.server.world().authored_count(),
        polygon::ITEM_SPAWNS.len()
    );
}

#[test]
fn everything_that_happened_in_a_round_stays_in_that_round() {
    let cfg = RoundConfig::default();
    let mut round = Round::start(1, &content(), &cfg).unwrap();
    let fresh_hash = round.server.world().content_hash();

    // Игрок берёт ломик (он лежит у южной стены комнаты A, недалеко от точки появления).
    let mut h = Harness::with_server(round.server);
    let a = h.join("a");
    let me = h.client(a).net_id().unwrap();
    // Подойти к предмету: появление (2.5, 4.5), ломик в (1.5, 1.5): на север и чуть влево.
    for _ in 0..11 {
        h.input(a, 0, 1, 0.0);
    }
    for _ in 0..5 {
        h.input(a, -1, 0, 0.0);
    }
    let p = h
        .client(a)
        .command_packet(CMD_PICKUP, pickup_data(item0()))
        .unwrap();
    h.send(a, p);
    let hands = decode_hands(h.client(a).component(me, HANDS_COMPONENT).unwrap()).unwrap();
    assert_eq!(hands.held().count(), 1, "ломик взят");
    assert!(h.server.world().authored_object(item0()).is_none());
    assert_ne!(h.server.world().content_hash(), fresh_hash);

    // Новый раунд: мир свежий, ломик на месте, у игроков ничего нет.
    round = Round::start(2, &content(), &cfg).unwrap();
    assert_eq!(round.number, 2);
    assert_eq!(
        round.server.world().content_hash(),
        fresh_hash,
        "та же карта с нуля"
    );
    assert!(round.server.world().authored_object(item0()).is_some());
    let mut h2 = Harness::with_server(round.server);
    let b = h2.join("b");
    let net = h2.client(b).net_id().unwrap();
    assert!(
        h2.client(b).component(net, HANDS_COMPONENT).is_none(),
        "состояние не перенеслось"
    );
}

#[test]
fn restart_replaces_the_server_and_bumps_the_round_number() {
    let cfg = RoundConfig::default();
    let mut r = Round::start(5, &content(), &cfg).unwrap();
    r.restart(&content(), &cfg).unwrap();
    assert_eq!(r.number, 6);
    assert_eq!(r.server.player_count(), 0);
}
