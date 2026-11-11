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
    (id: "Backpack", components: {
        "Item": (size: "Large"),
        "Clothing": (slots: ["Back"]),
        "Storage": (grid: ["0,0,5,3"], maxItemSize: "Normal"),
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
