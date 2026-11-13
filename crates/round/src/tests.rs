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
        polygon::map().items.len() + polygon::map().zones.len()
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

    /// Закрыть дверь на доступ `Engineering`: зона карты с ребром двери.
    fn restrict(h: &mut Harness, door: ObjectId) {
        use astrocore_world::{AuthoredKind, AuthoredObject, FloorId, Placement, WorldCommand};
        let e = h.server.world().element(door).unwrap().clone();
        let Placement::Edge(g) = e.placement else {
            panic!("дверь на ребре");
        };
        let dir = if matches!(g.dir, astrocore_math::EdgeDir::AlongX) {
            "X"
        } else {
            "Z"
        };
        let mut z = AuthoredObject::new(
            ObjectId::from_u128(0xACCE_5500),
            AuthoredKind::Zone,
            FloorId(0),
            astrocore_math::glam::Vec3::new(0.0, 0.0, 0.0),
            astrocore_math::glam::Vec3::new(0.1, 0.1, 0.1),
        );
        z.metadata
            .insert("door_edge".into(), format!("{},{},{dir}", g.x, g.z));
        z.metadata
            .insert("door_access".into(), "Engineering".into());
        let outs = h
            .server
            .apply_world_command(WorldCommand::PlaceAuthored(z))
            .unwrap();
        h.deliver(outs);
    }

    #[test]
    fn a_door_with_a_reader_opens_only_for_a_card_with_the_access() {
        let (mut h, a, _b) = setup();
        let door = door_element(&h);
        restrict(&mut h, door);
        h.ticks(5);
        // Без должности — без доступа.
        interact(&mut h, a, door);
        assert!(!h.server.doors().is_open(door));
        // Инженер получает КПК с картой `Engineering`.
        let p = h
            .client(a)
            .command_packet(gameplay::CMD_JOB, b"StationEngineer".to_vec())
            .unwrap();
        h.send(a, p);
        h.ticks(10);
        interact(&mut h, a, door);
        assert!(h.server.doors().is_open(door));
    }

    #[test]
    fn a_security_officer_unlocks_the_security_locker_and_a_civilian_cannot() {
        use astrocore_world::{AuthoredKind, AuthoredObject, FloorId, WorldCommand};
        let (mut h, a, _b) = setup();
        let locker = |h: &Harness, name: &str| {
            h.server
                .world()
                .authored()
                .filter(|o| matches!(&o.kind, AuthoredKind::Placement { prototype } if prototype.as_str() == name))
                .count()
        };
        let at = astrocore_math::glam::Vec3::new(4.6, 0.2, 2.3);
        let obj = AuthoredObject::new(
            ObjectId::from_u128(0x10CC_0001),
            AuthoredKind::Placement {
                prototype: astrocore_core::PrototypeId::new("ss14:locker_security_filled").unwrap(),
            },
            FloorId(0),
            at,
            at + astrocore_math::glam::Vec3::splat(0.4),
        );
        let outs = h
            .server
            .apply_world_command(WorldCommand::PlaceAuthored(obj))
            .unwrap();
        h.deliver(outs);
        h.ticks(20);
        assert_eq!(locker(&h, "ss14:locker_security_filled"), 1);
        let id = ObjectId::from_u128(0x10CC_0001);
        // Без должности и карты не отпереть.
        interact(&mut h, a, id);
        interact(&mut h, a, id);
        h.ticks(5);
        assert_eq!(locker(&h, "ss14:locker_security_filled"), 1);
        // Офицер с КПК и картой охраны: первое нажатие отпирает, второе открывает.
        let p = h
            .client(a)
            .command_packet(gameplay::CMD_JOB, b"SecurityOfficer".to_vec())
            .unwrap();
        h.send(a, p);
        h.ticks(10);
        interact(&mut h, a, id);
        h.ticks(5);
        interact(&mut h, a, id);
        h.ticks(10);
        assert_eq!(locker(&h, "ss14:locker_security_filled"), 0);
        assert_eq!(locker(&h, "ss14:locker_security_filled_open"), 1);
    }

    #[test]
    fn an_officer_draws_the_pistol_and_keeps_it_after_shooting() {
        let (mut h, a, _b) = setup();
        let send = |h: &mut Harness, kind: &str, data: Vec<u8>| {
            let p = h.client(a).command_packet(kind, data).unwrap();
            h.send(a, p);
        };
        send(&mut h, gameplay::CMD_JOB, b"SecurityOfficer".to_vec());
        h.ticks(10);
        send(&mut h, gameplay::CMD_UNEQUIP, gameplay::unequip_data("pocket1"));
        h.ticks(60);
        let net = h.client(a).net_id().unwrap();
        let held = |h: &Harness| {
            h.client(a)
                .component(net, gameplay::HANDS_COMPONENT)
                .and_then(gameplay::decode_hands)
                .map(|hs| hs.held().map(|i| i.prototype.to_string()).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        assert_eq!(held(&h), vec!["ss14:weapon_pistol_mk58".to_string()]);
        send(&mut h, gameplay::CMD_COMBAT, vec![1]);
        h.ticks(5);
        for _ in 0..2 {
            send(&mut h, gameplay::CMD_SHOOT, gameplay::throw_data([8.0, 1.0, 2.0]));
            h.ticks(15);
        }
        assert_eq!(held(&h), vec!["ss14:weapon_pistol_mk58".to_string()], "оружие осталось в руке");
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

    /// Положить ломик у двери и взять его в руку.
    fn arm_with_crowbar(h: &mut Harness, a: astrocore_network::server::ConnId) {
        let crowbar = astrocore_world::AuthoredObject::new(
            ObjectId::from_u128(0xC41),
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
                gameplay::pickup_data(ObjectId::from_u128(0xC41)),
            )
            .unwrap();
        h.send(a, p);
    }

    #[test]
    fn walking_away_or_cancelling_stops_the_prying() {
        let (mut h, a, _b) = setup();
        let door = door_element(&h);
        h.server.door_state_mut(door).unwrap().locked = true;
        arm_with_crowbar(&mut h, a);
        interact(&mut h, a, door);
        h.ticks(5);
        for _ in 0..6 {
            h.input(a, 0, 1, 0.0);
        }
        h.ticks(40);
        assert!(!h.server.doors().is_open(door), "ушёл — вскрытие прервано");
        // Отмена командой.
        let (mut h, a, _b) = setup();
        let door = door_element(&h);
        h.server.door_state_mut(door).unwrap().locked = true;
        arm_with_crowbar(&mut h, a);
        interact(&mut h, a, door);
        h.ticks(5);
        let p = h
            .client(a)
            .command_packet(gameplay::CMD_CANCEL, vec![])
            .unwrap();
        h.send(a, p);
        h.ticks(40);
        assert!(
            !h.server.doors().is_open(door),
            "отмена остановила вскрытие"
        );
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
            !h.server.doors().is_open(door),
            "вскрытие занимает время: сразу дверь не открыта"
        );
        let me = h.client(a).net_id().unwrap();
        let view = |h: &Harness| {
            h.client(a)
                .component(me, gameplay::DOAFTER_COMPONENT)
                .and_then(gameplay::decode_doafter)
        };
        assert_eq!(view(&h).map(|v| v.kind), Some("pry".to_string()));
        h.ticks(35);
        assert!(
            h.server.doors().is_open(door),
            "монтировка вскрыла запертую дверь"
        );
        assert_eq!(view(&h), None, "действие закончилось");
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

mod survival {
    use super::*;
    use gameplay::{PressureAlert, VITALS_COMPONENT, decode_vitals};

    #[test]
    fn the_sealed_polygon_keeps_its_air_and_the_player_breathes_normally() {
        let round = Round::start(1, &content(), &RoundConfig::default()).unwrap();
        let mut h = Harness::with_server(round.server);
        let a = h.join("a");
        h.ticks(20 * 90);
        let net = h.client(a).net_id().unwrap();
        let v = decode_vitals(h.client(a).component(net, VITALS_COMPONENT).unwrap()).unwrap();
        assert_eq!(v.pressure_alert, PressureAlert::None);
        assert!((v.pressure_kpa - 101.3).abs() < 2.0, "{}", v.pressure_kpa);
        assert!(!v.low_oxygen);
    }
}

/// Диагностика электросетей импортированной карты: `ASTRO_MAP_FILE=/tmp/saltern.ron cargo test -p round
/// saltern_power -- --ignored --nocapture`.
#[test]
#[ignore]
fn saltern_power_report() {
    use gameplay::electric::{PowerPart, build};
    let Some(path) = std::env::var_os("ASTRO_MAP_FILE") else {
        return;
    };
    let map = maps::MapFile::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    let registry = polygon::load_registry(&content()).unwrap();
    let mut cables = Vec::new();
    for z in &map.zones {
        if let Some(g) = z
            .metadata
            .get("power_cable")
            .and_then(|v| power::CableGroup::parse(v))
        {
            cables.push((
                g,
                (
                    ((z.min[0] + z.max[0]) / 2.0).floor() as i32,
                    ((z.min[2] + z.max[2]) / 2.0).floor() as i32,
                ),
            ));
        }
    }
    let mut parts = Vec::new();
    for (i, it) in map.items.iter().enumerate() {
        if let Ok(r) = registry.resolve(&it.prototype)
            && let Some(p) = PowerPart::from_resolved(&r)
        {
            parts.push((
                ObjectId::from_u128(i as u128 + 1),
                (it.x.floor() as i32, it.z.floor() as i32),
                p,
            ));
        }
    }
    let total = |f: fn(&PowerPart) -> bool| parts.iter().filter(|p| f(&p.2)).count();
    let mut b = build(&cables, &parts, &Default::default());
    println!(
        "кабелей {}; источников {}/{}; аккумуляторов {}/{}; потребителей {}/{}",
        cables.len(),
        b.state.supplies.len(),
        total(|p| p.supplier.is_some()),
        b.batteries.len(),
        total(|p| p.battery.is_some()),
        b.receivers.len(),
        total(|p| p.receiver_load.is_some()),
    );
    println!("сетей питания {}", b.state.networks.len());
    for _ in 0..(180 * 20) {
        b.state.tick(0.05);
    }
    let powered = b
        .receivers
        .iter()
        .filter(|r| b.state.loads[r.1].receiving_power >= r.2 * 0.999)
        .count();
    println!("запитано потребителей {powered} из {}", b.receivers.len());
    let supply: f32 = b.state.supplies.iter().map(|s| s.current_supply).sum();
    println!("выдача источников {supply:.0} Вт");
}

/// Огнезапоры Saltern: сколько закрыто сразу и через минуту. `ASTRO_MAP_FILE=/tmp/saltern.ron cargo test
/// -p round saltern_firelocks -- --ignored --nocapture`.
#[test]
#[ignore]
fn saltern_firelocks() {
    let Some(path) = std::env::var_os("ASTRO_MAP_FILE") else {
        return;
    };
    polygon::use_map_file(std::path::Path::new(&path));
    let round = Round::start(1, &content(), &RoundConfig::default()).unwrap();
    let ids: Vec<_> = round
        .server
        .world()
        .elements()
        .filter(|e| e.material.as_str().ends_with("firelock"))
        .map(|e| e.id)
        .collect();
    let mut h = Harness::with_server(round.server);
    let _a = h.join("a");
    for secs in [1, 5, 30, 90] {
        h.ticks(20 * secs);
        let closed = ids
            .iter()
            .filter(|i| !h.server.doors().is_open(**i))
            .count();
        println!(
            "через {secs} с: огнезапоров {}, закрыто {closed}",
            ids.len()
        );
    }
}

/// Пролом в стене: сигнализации рядом должны закрыть огнезапоры. Запуск как у `saltern_firelocks`.
#[test]
#[ignore]
fn saltern_breach_closes_firelocks() {
    use astrocore_world::{AuthoredKind, Placement, StructureKind, WorldCommand};
    let Some(path) = std::env::var_os("ASTRO_MAP_FILE") else {
        return;
    };
    polygon::use_map_file(std::path::Path::new(&path));
    let mut firelocks = Vec::new();
    let round = Round::start_with(1, &content(), &RoundConfig::default(), |w| {
        firelocks = w
            .elements()
            .filter(|e| e.material.as_str().ends_with("firelock"))
            .map(|e| e.id)
            .collect();
        // Сигнализация, чьи связи касаются огнезапора.
        let fire_cells: std::collections::BTreeSet<(i32, i32)> = w
            .elements()
            .filter(|e| e.material.as_str().ends_with("firelock"))
            .filter_map(|e| match e.placement {
                Placement::Edge(g) => Some(g),
                _ => None,
            })
            .flat_map(|g| match g.dir {
                astrocore_math::EdgeDir::AlongX => [(g.x, g.z - 1), (g.x, g.z)],
                astrocore_math::EdgeDir::AlongZ => [(g.x - 1, g.z), (g.x, g.z)],
            })
            .collect();
        let alarm = w
            .authored()
            .filter(|o| matches!(&o.kind, AuthoredKind::Placement { prototype } if prototype.as_str().ends_with("air_alarm")))
            .find(|o| {
                o.metadata.get("links").is_some_and(|l| {
                    l.split(';').filter_map(|p| p.split_once(',')).any(|(x, z)| {
                        matches!((x.parse(), z.parse()), (Ok(x), Ok(z)) if fire_cells.contains(&(x, z)))
                    })
                })
            })
            .expect("сигнализация с огнезапором")
            .clone();
        let cells: Vec<(i32, i32)> = alarm.metadata["links"]
            .split(';')
            .filter_map(|p| p.split_once(','))
            .filter_map(|(x, z)| Some((x.parse().ok()?, z.parse().ok()?)))
            .collect();
        let walls: Vec<_> = w
            .elements()
            .filter(|e| matches!(e.kind, StructureKind::Wall))
            .filter_map(|e| match e.placement {
                Placement::Edge(g) => Some((e.id, g)),
                _ => None,
            })
            .filter(|(_, g)| cells.iter().any(|c| (g.x - c.0).abs() <= 1 && (g.z - c.1).abs() <= 1))
            .map(|(id, _)| id)
            .collect();
        println!("пролом: убрано стен {}", walls.len());
        for id in walls {
            let _ = w.apply(WorldCommand::RemoveElement { id });
        }
    })
    .unwrap();
    let mut h = Harness::with_server(round.server);
    let _a = h.join("a");
    for secs in [1, 10, 60] {
        h.ticks(20 * secs);
        let closed = firelocks
            .iter()
            .filter(|i| !h.server.doors().is_open(**i))
            .count();
        println!(
            "через {secs} с: огнезапоров {}, закрыто {closed}",
            firelocks.len()
        );
    }
}

/// Появление новичков на точках карты. Запуск как у `saltern_firelocks`.
#[test]
#[ignore]
fn saltern_joiners_spawn_on_map_points() {
    let Some(path) = std::env::var_os("ASTRO_MAP_FILE") else {
        return;
    };
    polygon::use_map_file(std::path::Path::new(&path));
    let round = Round::start(1, &content(), &RoundConfig::default()).unwrap();
    let mut h = Harness::with_server(round.server);
    let ids = [h.join("a"), h.join("b"), h.join("c")];
    h.ticks(40);
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        let p = h.client(id).position();
        println!("игрок на {p:?}");
        seen.insert(format!("{:.1},{:.1}", p.x, p.z));
    }
    assert_eq!(seen.len(), 3, "разные точки появления");
}
