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

mod doors {
    use astrocore_world::{OpeningKind, StructureKind};
    use gameplay::{CMD_INTERACT, CMD_PICKUP_ANY, interact_data};

    use super::*;

    fn door_element(h: &Harness) -> ObjectId {
        h.server
            .world()
            .elements()
            .find(|e| matches!(e.kind, StructureKind::Opening(OpeningKind::Door)))
            .expect("на полигоне есть дверь")
            .id
    }

    /// Подвести игрока к двери: из точки появления (2.5, 4.5) к (5.1, 2.5).
    fn walk_to_door(h: &mut Harness, a: astrocore_network::server::ConnId) {
        for _ in 0..10 {
            h.input(a, 0, 1, 0.0);
        }
        for _ in 0..13 {
            h.input(a, 0, 1, -std::f32::consts::FRAC_PI_2);
        }
    }

    fn interact(h: &mut Harness, a: astrocore_network::server::ConnId, id: ObjectId) {
        let p = h
            .client(a)
            .command_packet(CMD_INTERACT, interact_data(id))
            .unwrap();
        h.send(a, p);
    }

    fn setup() -> (
        Harness,
        astrocore_network::server::ConnId,
        astrocore_network::server::ConnId,
    ) {
        let round = Round::start(1, &polygon::content_dir(), &RoundConfig::default()).unwrap();
        let mut h = Harness::with_server(round.server);
        let (a, b) = (h.join("a"), h.join("b"));
        walk_to_door(&mut h, a);
        (h, a, b)
    }

    #[test]
    fn a_hand_opens_and_closes_the_door_for_everyone() {
        let (mut h, a, b) = setup();
        let door = door_element(&h);
        let net = h.client(b).door_ids()[0];
        assert_eq!(h.client(b).door_open(net), Some(false));
        interact(&mut h, a, door);
        assert!(h.server.doors().is_open(door));
        assert_eq!(
            h.client(b).door_open(net),
            Some(true),
            "второй клиент видит открытую дверь"
        );
        interact(&mut h, a, door);
        assert!(!h.server.doors().is_open(door));
        assert_eq!(h.client(b).door_open(net), Some(false));
    }

    #[test]
    fn a_locked_door_stays_shut_for_hands_but_a_crowbar_forces_it() {
        let (mut h, a, _b) = setup();
        let door = door_element(&h);
        h.server.door_state_mut(door).unwrap().locked = true;
        interact(&mut h, a, door);
        assert!(
            !h.server.doors().is_open(door),
            "рука запертую не открывает"
        );
        // Берём ломик, лежащий рядом, и ломаем замок.
        let crowbar = astrocore_world::AuthoredObject::new(
            ObjectId::from_u128(0xC40),
            astrocore_world::AuthoredKind::Placement {
                prototype: astrocore_core::PrototypeId::new("ss14:crowbar").unwrap(),
            },
            astrocore_world::FloorId(0),
            astrocore_math::glam::Vec3::new(5.0, polygon::FLOOR_TOP_M, 2.0),
            astrocore_math::glam::Vec3::new(5.3, polygon::FLOOR_TOP_M + 0.3, 2.3),
        );
        let outs = h
            .server
            .apply_world_command(astrocore_world::WorldCommand::PlaceAuthored(crowbar))
            .unwrap();
        h.deliver(outs);
        let p = h
            .client(a)
            .command_packet(
                CMD_PICKUP_ANY,
                gameplay::pickup_data(ObjectId::from_u128(0xC40)),
            )
            .unwrap();
        h.send(a, p);
        interact(&mut h, a, door);
        assert!(
            h.server.doors().is_open(door),
            "монтировка вскрыла запертую дверь"
        );
        // Открытую дверь монтировка не трогает; рука с предметом её не закрывает.
        interact(&mut h, a, door);
        assert!(h.server.doors().is_open(door));
    }

    #[test]
    fn a_door_out_of_reach_does_not_respond() {
        let round = Round::start(1, &polygon::content_dir(), &RoundConfig::default()).unwrap();
        let mut h = Harness::with_server(round.server);
        let a = h.join("a"); // стоит в точке появления, дверь далеко
        let door = door_element(&h);
        interact(&mut h, a, door);
        assert!(!h.server.doors().is_open(door));
    }
}
