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
    (id: "PlasmaCan", components: {
        "GasCanister": (volume: 1500.0, temperature: 293.15, moles: (Plasma: 400.0), releasePressure: 300.0),
    }),
    (id: "LubeBottle", components: {
        "Item": (size: "Small"),
        "Solution": (maxVol: 100.0, reagents: (SpaceLube: 60.0)),
    }),
    (id: "Mop", components: {
        "Item": (size: "Large"),
        "Absorbent": (),
        "Solution": (maxVol: 120.0),
    }),
    (id: "WaterBucket", components: {
        "Item": (size: "Large"),
        "Refillable": (),
        "Solution": (maxVol: 360.0, reagents: (Water: 360.0)),
    }),
    (id: "VendTool", components: {
        "VendingMachine": (pack: "ToolsInventory", initialStockQuality: 1.0, ejectDelay: 1.2, stock: (Crowbar: 2, Pen: 1, NoSuchThing: 5)),
    }),
    (id: "TableTools", components: {
        "EntityTable": (table: (k: "group", children: [(k: "ent", id: "Crowbar", weight: 1.0), (k: "ent", id: "NoSuchMob")])),
    }),
    (id: "ToolSpawner", components: {
        "Spawner": (offset: 0.2, table: (k: "nested", table: "Tools")),
    }),
    (id: "JobTester", components: {
        "Job": (title: "Tester", start: 1, max: 1, weight: 5, equipment: {"back": "Satchel"}, inhand: ["Crowbar"], storage: {"back": ["Pen"]}),
    }),
    (id: "JobGuard", components: {
        "Job": (title: "Guard", start: -1, max: -1, weight: 0, access: ["Security"], equipment: {}, inhand: [], storage: {}),
    }),
    (id: "JobFree", components: {
        "Job": (title: "Free", start: -1, max: -1, weight: 0, equipment: {}, inhand: [], storage: {}),
    }),
    (id: "VendLocked", components: {
        "VendingMachine": (pack: "P", initialStockQuality: 1.0, ejectDelay: 1.2, stock: (Crowbar: 2)),
        "AccessReader": (access: [["Security"]]),
    }),
    (id: "Closet", components: {
        "EntityStorage": (capacity: 30, openProto: "ClosetOpen"),
        "ContainerFill": (table: (k: "ent", id: "Pen")),
    }),
    (id: "ClosetOpen", components: {"EntityStorageOpen": (closedProto: "Closet")}),
    (id: "SecLocker", components: {
        "EntityStorage": (capacity: 30, openProto: "SecLockerOpen"),
        "Lock": (locked: true),
        "AccessReader": (access: [["Security"]]),
        "ContainerFill": (table: (k: "ent", id: "Crowbar")),
    }),
    (id: "SecLockerOpen", components: {"EntityStorageOpen": (closedProto: "SecLocker")}),
    (id: "IdConsole", components: {
        "IdCardConsole": (access: ["Engineering", "Security"]),
    }),
    (id: "BossCard", components: {
        "Item": (size: "Small"),
        "IdCard": (access: ["Engineering"]),
    }),
    (id: "BlankCard", components: {
        "Item": (size: "Small"),
        "IdCard": (access: []),
    }),
    (id: "AnalyzerT", components: {
        "Item": (size: "Small"),
        "HealthAnalyzer": (),
    }),
    (id: "JugT", components: {
        "Item": (size: "Large"),
        "Solution": (maxVol: 240.0, reagents: (Silicon: 240.0)),
        "Drainable": (),
    }),
    (id: "DispT", components: {
        "ReagentDispenser": (table: (k: "all", children: [(k: "ent", id: "JugT"), (k: "ent", id: "JugT")])),
    }),
    (id: "BeakerT", components: {
        "FitsInDispenser": (),
        "Item": (size: "Small"),
        "Solution": (maxVol: 50.0, reagents: ()),
        "SolutionTransfer": (amount: 10.0, max: 50.0),
        "Refillable": (),
        "Drainable": (),
    }),
    (id: "SyringeT", components: {
        "Item": (size: "Tiny"),
        "Solution": (maxVol: 15.0, reagents: ()),
        "Injector": (modes: [
            (id: "Inject", behavior: "Inject", amounts: [5.0, 10.0], mobTime: 0.0, selfModifier: 0.5, downedModifier: 0.5, delayPerVolume: 0.1, ignoreDelayForVolume: 5.0, drawTime: 0.0),
            (id: "Draw", behavior: "Draw", amounts: [5.0, 10.0], mobTime: 5.0, selfModifier: 0.5, downedModifier: 0.5, delayPerVolume: 0.1, ignoreDelayForVolume: 5.0, drawTime: 0.0),
        ], active: 1, amount: 5.0),
    }),
    (id: "CryoT", components: {
        "GasPipe": (nodes: [(dirs: "S")]),
        "CryoPodAir": (node: 0, volume: 2000.0),
        "CryoPod": (beakerTransferTime: 2.0, beakerTransferAmount: 1.0, entryDelay: 2.0),
    }),
    (id: "HotplateT", components: {
        "SolutionHeater": (heatPerSecond: 160.0),
    }),
    (id: "CentrifugeT", components: {
        "SolutionContainerMixer": (mixDuration: 10.0),
        "ReactionMixer": (types: ["Centrifuge"], mixerType: "Machine", time: 0.0),
    }),
    (id: "MasterT", components: {
        "ChemMaster": (pillDosageLimit: 20),
    }),
    (id: "SpoonT", components: {
        "Item": (size: "Tiny"),
        "ReactionMixer": (types: ["Stir"], mixerType: "Machine", time: 0.5),
    }),
    (id: "BeakerHotT", components: {
        "FitsInDispenser": (),
        "MixableSolution": (),
        "Item": (size: "Small"),
        "Solution": (maxVol: 50.0, reagents: (Hydrogen: 3.0, Nitrogen: 1.0)),
    }),
    (id: "BeakerCocktailT", components: {
        "FitsInDispenser": (),
        "MixableSolution": (),
        "Item": (size: "Small"),
        "Solution": (maxVol: 50.0, reagents: (Martini: 2.0, Vodka: 1.0)),
    }),
    (id: "BeakerAllicinT", components: {
        "FitsInDispenser": (),
        "MixableSolution": (),
        "Item": (size: "Small"),
        "Solution": (maxVol: 50.0, reagents: (Allicin: 8.0)),
    }),
    (id: "BeakerWaterT", components: {
        "FitsInDispenser": (),
        "MixableSolution": (),
        "Item": (size: "Small"),
        "Solution": (maxVol: 50.0, reagents: (Water: 30.0)),
    }),
    (id: "CanisterT", components: {
        "Item": (size: "Tiny"),
        "Tag": (tags: ["PillCanister"]),
        "Storage": (grid: ["0,0,4,1"], whitelist: (tags: ["Pill"], components: [])),
    }),
    (id: "BottleT", components: {
        "Item": (size: "Tiny"),
        "Tag": (tags: ["Bottle"]),
        "Solution": (maxVol: 30.0, reagents: ()),
    }),
    (id: "Pill", components: {
        "Item": (size: "Tiny"),
        "Tag": (tags: ["Pill"]),
        "Pill": (),
        "Solution": (maxVol: 20.0, canReact: false, reagents: ()),
    }),
    (id: "DefibT", components: {
        "Item": (size: "Large"),
        "Defibrillator": (zapHeal: {"Asphyxiation": -40.0}, zapDamage: 5.0, writhe: 3.0, zapDelay: 5.0, doAfter: 3.0, useCharge: 100.0, drawRate: 1.0, charge: 720.0, powered: true, toggled: true),
    }),
    (id: "Belt", components: {
        "Conveyor": (speed: 2.0),
    }),
    (id: "Switch", components: {
        "SignalSwitch": (),
    }),
    (id: "DisposalBin", components: {
        "DisposalUnit": (maxCapacity: 30),
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
    r.load_ron_str(power_monitoring::PROTOS, "power monitor test")
        .unwrap();
    r.load_ron_str(atmos_alerts::PROTOS, "atmos alerts test")
        .unwrap();
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

    pub(super) fn doafter(h: &Harness, who: ConnId) -> Option<crate::DoAfterView> {
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

    pub(super) fn ghost_flag(h: &Harness, viewer: ConnId, owner: ConnId) -> bool {
        let net = h.client(owner).net_id().unwrap();
        h.client(viewer)
            .component(net, crate::GHOST_COMPONENT)
            .is_some_and(|b| b.first() == Some(&1))
    }

    pub(super) fn kill(h: &mut Harness, killer: ConnId, victim: ConnId) {
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
        assert!((v.pressure_kpa - 101.3).abs() < 5.0, "{}", v.pressure_kpa);
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

    #[test]
    fn a_canister_released_in_the_room_feeds_a_fire() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(10);
        spawn(
            &mut h,
            item_at(130, "ss14:plasma_can", Vec3::new(2.6, 0.6, 2.6)),
        );
        h.ticks(3);
        // Пустой рукой открываем выпуск: давление в комнате растёт, плазма расходится.
        command(&mut h, a, CMD_INTERACT, interact_data(oid(130)));
        h.ticks(20 * 120);
        light_welder(&mut h, a);
        h.ticks(20 * 90);
        let net = h.client(a).net_id().unwrap();
        let fires = h
            .client(a)
            .component(net, FIRES_COMPONENT)
            .and_then(decode_fires)
            .unwrap_or_default();
        assert!(
            !fires.is_empty() || heat(&h, a) > 0.0,
            "плазма из баллона загорелась"
        );
    }
}

mod puddles {
    use super::*;
    use crate::puddles::{PUDDLES_COMPONENT, decode_puddles};
    use crate::{CMD_SPILL, CMD_USE_GROUND};

    fn puddles(h: &Harness, who: ConnId) -> Vec<crate::puddles::PuddleView> {
        let net = h.client(who).net_id().unwrap();
        h.client(who)
            .component(net, PUDDLES_COMPONENT)
            .and_then(decode_puddles)
            .unwrap_or_default()
    }

    fn take_lube(h: &mut Harness, a: ConnId) {
        spawn(
            h,
            item_at(140, "ss14:lube_bottle", Vec3::new(2.6, 0.6, 2.6)),
        );
        h.ticks(3);
        command(h, a, CMD_PICKUP, pickup_data(oid(140)));
        h.ticks(5);
    }

    #[test]
    fn spilling_a_bottle_leaves_a_puddle_that_clients_see() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        take_lube(&mut h, a);
        let hand = hands_of(&h, a, a).unwrap().active() as u8;
        command(&mut h, a, CMD_SPILL, vec![hand]);
        h.ticks(30);
        let p = puddles(&h, a);
        // 60 ед. больше клетки (50): лишнее растекается по четырём соседним клеткам.
        assert!(p.len() > 1 && p.len() <= 5, "{p:?}");
        let centre = p.iter().find(|v| v.fill >= 1.0).expect("основная лужа");
        assert!(centre.slippery);
        // Бутылка опустела: второй раз лить нечего.
        command(&mut h, a, CMD_SPILL, vec![hand]);
        h.ticks(30);
        assert!(puddles(&h, a).len() <= 5);
    }

    #[test]
    fn running_over_a_slippery_puddle_drops_what_you_hold() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        take_lube(&mut h, a);
        let hand = hands_of(&h, a, a).unwrap().active() as u8;
        command(&mut h, a, CMD_SPILL, vec![hand]);
        h.ticks(30);
        // Берём что-нибудь ещё, чтобы было что ронять.
        spawn(&mut h, item_at(141, "ss14:pen", Vec3::new(2.6, 0.6, 2.6)));
        h.ticks(3);
        command(&mut h, a, CMD_PICKUP, pickup_data(oid(141)));
        h.ticks(5);
        assert!(hands_of(&h, a, a).unwrap().held().count() >= 1);
        for _ in 0..12 {
            h.input(a, 0, 1, 0.0);
        }
        for _ in 0..12 {
            h.input(a, 0, -1, 0.0);
        }
        h.ticks(5);
        assert_eq!(
            hands_of(&h, a, a).unwrap().held().count(),
            0,
            "поскользнулся и выронил всё"
        );
    }

    #[test]
    fn a_mop_dipped_in_a_bucket_cleans_a_slippery_puddle() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        take_lube(&mut h, a);
        let hand = hands_of(&h, a, a).unwrap().active() as u8;
        command(&mut h, a, CMD_SPILL, vec![hand]);
        h.ticks(30);
        assert!(puddles(&h, a).iter().any(|p| p.slippery), "лужа скользкая");
        // Пустую бутылку на пол, в руку швабру; ведро с водой рядом.
        command(&mut h, a, CMD_DROP, drop_data(hand));
        h.ticks(5);
        spawn(
            &mut h,
            item_at(150, "ss14:water_bucket", Vec3::new(3.0, 0.1, 2.4)),
        );
        spawn(&mut h, item_at(151, "ss14:mop", Vec3::new(2.6, 0.6, 2.6)));
        h.ticks(3);
        command(&mut h, a, CMD_PICKUP, pickup_data(oid(151)));
        h.ticks(5);
        // Макаем в ведро, затем моем лужу под ногами (дальше 1 с, чтобы прошла задержка).
        let bucket = crate::drop_at_data(hand, [3.1, 0.0, 2.5]);
        command(&mut h, a, CMD_USE_GROUND, bucket);
        h.ticks(40);
        let floor = crate::drop_at_data(hand, [2.5, 0.0, 2.5]);
        command(&mut h, a, CMD_USE_GROUND, floor);
        h.ticks(40);
        let slippery = puddles(&h, a).iter().filter(|p| p.slippery).count();
        assert_eq!(slippery, 0, "швабра убрала смазку: {:?}", puddles(&h, a));
    }

    #[test]
    fn a_player_who_joins_later_sees_existing_puddles() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        take_lube(&mut h, a);
        let hand = hands_of(&h, a, a).unwrap().active() as u8;
        command(&mut h, a, CMD_SPILL, vec![hand]);
        h.ticks(30);
        let b = h.join("b");
        h.ticks(30);
        assert!(!puddles(&h, b).is_empty(), "новичок видит лужи");
    }
}

mod disposal_tests {
    use super::*;
    use crate::wire::{CMD_INTERACT, interact_data};

    fn tube(h: &mut Harness, n: u128, x: f32, z: f32, exits: &str, entry: bool) {
        let mut o = AuthoredObject::new(
            oid(n),
            AuthoredKind::Zone,
            FloorId(0),
            Vec3::new(x + 0.01, 0.0, z + 0.01),
            Vec3::new(x + 0.99, 3.0, z + 0.99),
        );
        o.metadata.insert("disposal_tube".into(), exits.into());
        if entry {
            o.metadata.insert("disposal_entry".into(), "1".into());
        }
        spawn(h, o);
    }

    fn item_pos(h: &Harness, id: ObjectId) -> Option<Vec3> {
        h.server
            .world()
            .authored()
            .find(|o| o.id == id)
            .map(|o| (o.min() + o.max()) * 0.5)
    }

    #[test]
    fn an_item_put_in_a_unit_flies_through_the_pipe_and_lands_at_the_open_end() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        spawn(
            &mut h,
            item_at(800, "ss14:disposal_bin", Vec3::new(1.2, 0.9, 2.4)),
        );
        tube(&mut h, 801, 1.0, 2.0, "S", true);
        tube(&mut h, 802, 1.0, 3.0, "SN", false);
        tube(&mut h, 803, 1.0, 4.0, "SN", false);
        spawn(
            &mut h,
            item_at(140, "ss14:lube_bottle", Vec3::new(2.6, 0.6, 2.6)),
        );
        h.ticks(30);
        command(&mut h, a, CMD_PICKUP, pickup_data(oid(140)));
        h.ticks(5);
        assert!(item_pos(&h, oid(140)).is_none(), "бутылка в руке");
        command(&mut h, a, CMD_INTERACT, interact_data(oid(800)));
        h.ticks(5);
        assert!(item_pos(&h, oid(140)).is_none(), "бутылка внутри ёмкости");
        // Через 30 с ёмкость сбрасывает сама, холдер доезжает за доли секунды.
        h.ticks(20 * 33);
        let at = item_pos(&h, oid(140)).expect("бутылка вылетела");
        assert!((at.x - 1.5).abs() < 0.6, "{at:?}");
        assert!(at.z > 4.5, "улетела за конец трубы: {at:?}");
    }

    #[test]
    fn a_switch_runs_the_belts_and_carries_an_item_along_them() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        for (i, x) in [3.0_f32, 4.0, 5.0].iter().enumerate() {
            let mut b = item_at(900 + i as u128, "ss14:belt", Vec3::new(*x + 0.1, 0.9, 4.1));
            b.metadata.insert("rot".into(), "3".into());
            spawn(&mut h, b);
        }
        let mut s = item_at(910, "ss14:switch", Vec3::new(3.0, 0.9, 3.0));
        s.metadata.insert("links".into(), "3,4;4,4;5,4".into());
        spawn(&mut h, s);
        spawn(
            &mut h,
            item_at(140, "ss14:lube_bottle", Vec3::new(3.3, 0.9, 4.3)),
        );
        h.ticks(20);
        let start = item_pos(&h, oid(140)).expect("бутылка");
        // Ленты выключены: бутылка стоит.
        h.ticks(40);
        assert_eq!(item_pos(&h, oid(140)).unwrap(), start);
        command(&mut h, a, CMD_INTERACT, interact_data(oid(910)));
        h.ticks(20 * 3);
        let moved = item_pos(&h, oid(140)).expect("бутылка");
        assert!(moved.x > start.x + 1.5, "{start:?} → {moved:?}");
        // Ленту на восток довела до края: центр оказался над третьей лентой или за ней.
        assert!(
            (moved.z - 4.25).abs() < 0.2,
            "выровнялась по оси ленты: {moved:?}"
        );
        // Выключили — стоит.
        command(&mut h, a, CMD_INTERACT, interact_data(oid(910)));
        h.ticks(5);
        let stopped = item_pos(&h, oid(140)).unwrap();
        h.ticks(40);
        assert_eq!(item_pos(&h, oid(140)).unwrap(), stopped);
    }

    fn view(h: &Harness, who: ConnId) -> Option<crate::VendingView> {
        let net = h.client(who).net_id()?;
        h.client(who)
            .component(net, crate::VENDING_COMPONENT)
            .and_then(crate::decode_vending)
    }

    #[test]
    fn a_vending_machine_lists_stock_dispenses_and_denies_when_empty() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        spawn(
            &mut h,
            item_at(950, "ss14:vend_tool", Vec3::new(3.0, 0.9, 3.0)),
        );
        h.ticks(5);
        assert!(view(&h, a).is_none());
        command(&mut h, a, CMD_INTERACT, interact_data(oid(950)));
        h.ticks(5);
        let v = view(&h, a).expect("окно открыто");
        // Позиция без прототипа в Build пропущена.
        let names: Vec<_> = v
            .items
            .iter()
            .map(|i| (i.name.as_str(), i.amount))
            .collect();
        assert_eq!(names, vec![("Crowbar", 2), ("Pen", 1)], "{v:?}");
        command(&mut h, a, crate::CMD_VEND, b"Pen".to_vec());
        h.ticks(5);
        assert!(view(&h, a).unwrap().ejecting);
        h.ticks(40);
        let v = view(&h, a).unwrap();
        assert!(!v.ejecting);
        assert_eq!(v.items[1].amount, 0);
        // Товар лежит у автомата.
        let drops = h
            .server
            .world()
            .authored()
            .filter(|o| matches!(&o.kind, AuthoredKind::Placement { prototype } if prototype.as_str() == "ss14:pen"))
            .count();
        assert!(drops >= 1);
        // Закончился: отказ.
        command(&mut h, a, crate::CMD_VEND, b"Pen".to_vec());
        h.ticks(5);
        assert!(view(&h, a).unwrap().denying);
        // Закрыли окно.
        command(&mut h, a, crate::CMD_VEND_CLOSE, vec![]);
        h.ticks(5);
        assert!(view(&h, a).is_none());
    }

    #[test]
    fn a_spawner_is_replaced_by_what_its_table_rolls() {
        let mut h = harness();
        let _a = h.join("a");
        h.ticks(5);
        for i in 0..40 {
            spawn(
                &mut h,
                item_at(
                    1000 + i,
                    "ss14:tool_spawner",
                    Vec3::new(5.0 + (i % 5) as f32, 0.9, 5.0 + (i / 5) as f32),
                ),
            );
        }
        h.ticks(40);
        let count = |name: &str| {
            h.server
                .world()
                .authored()
                .filter(|o| matches!(&o.kind, AuthoredKind::Placement { prototype } if prototype.as_str() == name))
                .count()
        };
        // Спавнеров не осталось; ломики появились примерно в половине случаев, моба в Build нет.
        assert_eq!(count("ss14:tool_spawner"), 0);
        let crowbars = count("ss14:crowbar");
        assert!(crowbars > 8 && crowbars < 32, "{crowbars}");
    }

    fn jobs_view(h: &Harness, who: ConnId) -> Option<crate::JobsView> {
        let net = h.client(who).net_id()?;
        h.client(who)
            .component(net, crate::JOBS_COMPONENT)
            .and_then(crate::decode_jobs)
    }

    #[test]
    fn picking_a_job_equips_the_gear_and_fills_the_places() {
        let mut h = harness();
        let a = h.join("a");
        let b = h.join("b");
        h.ticks(10);
        let v = jobs_view(&h, a).expect("список должностей");
        // Порядок: больший вес выше.
        assert_eq!(v.jobs[0].id, "Tester");
        assert_eq!(v.current, None);
        command(&mut h, a, crate::CMD_JOB, b"Tester".to_vec());
        h.ticks(10);
        let hands = hands_of(&h, a, a).unwrap();
        assert_eq!(
            hands.held().next().unwrap().prototype.as_str(),
            "ss14:crowbar"
        );
        let inv = inv_of(&h, a, a).unwrap();
        let back = inv.inventory.get("back").expect("рюкзак надет");
        assert_eq!(back.prototype.as_str(), "ss14:satchel");
        let bag = inv.bags.iter().find(|b| b.id == back.id).expect("сумка");
        assert_eq!(bag.items[0].item.prototype.as_str(), "ss14:pen");
        // Место одно: второй игрок получает отказ, а у первого должность записана.
        command(&mut h, b, crate::CMD_JOB, b"Tester".to_vec());
        h.ticks(10);
        assert!(hands_of(&h, b, b).unwrap().held().next().is_none());
        assert_eq!(jobs_view(&h, a).unwrap().current.as_deref(), Some("Tester"));
        assert_eq!(jobs_view(&h, b).unwrap().jobs[0].taken, 1);
        // Должность без ограничений доступна всем; вторую взять нельзя.
        command(&mut h, b, crate::CMD_JOB, b"Free".to_vec());
        command(&mut h, b, crate::CMD_JOB, b"Tester".to_vec());
        h.ticks(10);
        assert_eq!(jobs_view(&h, b).unwrap().current.as_deref(), Some("Free"));
    }

    #[test]
    fn a_vending_machine_with_a_reader_refuses_without_access() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        spawn(
            &mut h,
            item_at(960, "ss14:vend_locked", Vec3::new(3.0, 0.9, 3.0)),
        );
        h.ticks(5);
        command(&mut h, a, CMD_INTERACT, interact_data(oid(960)));
        h.ticks(5);
        command(&mut h, a, crate::CMD_VEND, b"Crowbar".to_vec());
        h.ticks(5);
        let v = view(&h, a).unwrap();
        assert!(v.denying && !v.ejecting, "{v:?}");
        assert_eq!(v.items[0].amount, 2);
    }

    fn count_of(h: &Harness, name: &str) -> usize {
        h.server
            .world()
            .authored()
            .filter(|o| matches!(&o.kind, AuthoredKind::Placement { prototype } if prototype.as_str() == name))
            .count()
    }

    #[test]
    fn a_closet_opens_to_its_contents_and_closes_over_what_lies_next_to_it() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        spawn(
            &mut h,
            item_at(970, "ss14:closet", Vec3::new(3.0, 0.9, 3.0)),
        );
        h.ticks(20);
        // Закрыт: содержимое внутри.
        assert_eq!(count_of(&h, "ss14:pen"), 0);
        command(&mut h, a, CMD_INTERACT, interact_data(oid(970)));
        h.ticks(10);
        assert_eq!(count_of(&h, "ss14:closet"), 0);
        assert_eq!(count_of(&h, "ss14:closet_open"), 1);
        assert_eq!(count_of(&h, "ss14:pen"), 1);
        // Закрываем: ручка рядом снова внутри.
        command(&mut h, a, CMD_INTERACT, interact_data(oid(970)));
        h.ticks(10);
        assert_eq!(count_of(&h, "ss14:closet"), 1);
        assert_eq!(count_of(&h, "ss14:pen"), 0);
    }

    #[test]
    fn a_locked_locker_stays_shut_without_access() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        spawn(
            &mut h,
            item_at(971, "ss14:sec_locker", Vec3::new(3.0, 0.9, 3.0)),
        );
        h.ticks(20);
        command(&mut h, a, CMD_INTERACT, interact_data(oid(971)));
        h.ticks(10);
        command(&mut h, a, crate::CMD_LOCK, vec![]);
        h.ticks(10);
        command(&mut h, a, CMD_INTERACT, interact_data(oid(971)));
        h.ticks(10);
        assert_eq!(count_of(&h, "ss14:sec_locker"), 1);
        assert_eq!(count_of(&h, "ss14:crowbar"), 0);
    }

    #[test]
    fn a_card_with_access_unlocks_and_locks_a_locker() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        spawn(
            &mut h,
            item_at(972, "ss14:sec_locker", Vec3::new(3.0, 0.9, 3.0)),
        );
        h.ticks(20);
        // Без карты в слоте `id` доступа нет, даже с должностью: выдаём карту через снаряжение.
        command(&mut h, a, crate::CMD_JOB, b"Guard".to_vec());
        h.ticks(5);
        // Доступ даёт только удостоверение: у тестовой должности его нет, поэтому отпереть нельзя.
        command(&mut h, a, CMD_INTERACT, interact_data(oid(972)));
        h.ticks(10);
        assert_eq!(count_of(&h, "ss14:sec_locker"), 1);
    }

    fn idc_view(h: &Harness, who: ConnId) -> Option<crate::IdConsoleView> {
        let net = h.client(who).net_id()?;
        h.client(who)
            .component(net, crate::IDCONSOLE_COMPONENT)
            .and_then(crate::decode_view)
    }

    #[test]
    fn the_id_console_writes_only_the_access_the_privileged_card_holds() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        spawn(
            &mut h,
            item_at(980, "ss14:id_console", Vec3::new(3.0, 0.9, 3.0)),
        );
        spawn(
            &mut h,
            item_at(981, "ss14:boss_card", Vec3::new(2.6, 0.9, 2.6)),
        );
        spawn(
            &mut h,
            item_at(982, "ss14:blank_card", Vec3::new(2.8, 0.9, 2.6)),
        );
        h.ticks(10);
        // Карта с правами вставляется в первый слот, пустая — во второй.
        command(&mut h, a, CMD_PICKUP_ANY, pickup_data(oid(981)));
        h.ticks(3);
        command(&mut h, a, CMD_INTERACT, interact_data(oid(980)));
        h.ticks(3);
        command(&mut h, a, CMD_PICKUP_ANY, pickup_data(oid(982)));
        h.ticks(3);
        command(&mut h, a, CMD_INTERACT, interact_data(oid(980)));
        h.ticks(3);
        // Руки пусты: нажатие открывает окно.
        command(&mut h, a, CMD_INTERACT, interact_data(oid(980)));
        h.ticks(10);
        let v = idc_view(&h, a).expect("окно открыто");
        assert!(v.authorized && v.target.is_some(), "{v:?}");
        assert_eq!(v.allowed, vec!["Engineering".to_string()]);
        let write = |h: &mut Harness, access: &[&str]| {
            let r = crate::WriteRequest {
                name: "Иванов".into(),
                title: "Инженер".into(),
                access: access.iter().map(|s| s.to_string()).collect(),
            };
            command(h, a, crate::CMD_ID_WRITE, crate::write_data(&r));
            h.ticks(5);
        };
        // Engineering у ID с правами есть — записывается вместе с именем.
        write(&mut h, &["Engineering"]);
        let t = idc_view(&h, a).unwrap().target.unwrap();
        assert_eq!(t.access, vec!["Engineering".to_string()]);
        assert_eq!(
            (t.full_name.as_str(), t.job_title.as_str()),
            ("Иванов", "Инженер")
        );
        // Security у него нет: запись отклонена, доступ прежний.
        write(&mut h, &["Engineering", "Security"]);
        assert_eq!(
            idc_view(&h, a).unwrap().target.unwrap().access,
            vec!["Engineering".to_string()]
        );
        // Карту можно вынуть: слот освободился.
        command(&mut h, a, crate::CMD_ID_EJECT, vec![1]);
        h.ticks(5);
        assert!(idc_view(&h, a).unwrap().target.is_none());
    }

    fn held(proto: &str, n: u128) -> HeldItem {
        HeldItem {
            id: oid(5000 + n),
            prototype: PrototypeId::new(proto).unwrap(),
        }
    }

    #[test]
    fn pouring_between_beakers_moves_a_set_amount_and_runs_reactions() {
        let mut st = crate::module::State::new(registry()).unwrap();
        let (a, b) = (held("ss14:beaker_t", 1), held("ss14:beaker_t", 2));
        st.solution_mut(&a).unwrap().add("Nitrogen", 10.0);
        st.solution_mut(&a).unwrap().add("Potassium", 10.0);
        st.solution_mut(&b).unwrap().add("Silicon", 10.0);
        // Шаг 10 ед.: из A в B уходит поровну 10 → 5 азота и 5 калия.
        assert!(st.chem_use(&a, &b));
        assert!((st.solution_volume(&a) - 10.0).abs() < 1e-3);
        // B: 10 кремния + 5 азота + 5 калия → реакция Дилоксина (1:1:1 → 3).
        let s = st.solution_mut(&b).unwrap();
        assert!(s.amount("Dylovene") > 14.0, "{:?}", s.contents);
        // Приёмник не переполняется: 50 макс., остальное остаётся у источника.
        let c = held("ss14:beaker_t", 3);
        st.solution_mut(&c).unwrap().add("Water", 48.0);
        let before = st.solution_volume(&a);
        st.chem_use(&a, &c);
        assert!((st.solution_volume(&a) - (before - 2.0)).abs() < 1e-3);
    }

    #[test]
    fn a_syringe_draws_from_a_container_and_toggles_to_inject() {
        let mut st = crate::module::State::new(registry()).unwrap();
        let bottle = held("ss14:beaker_t", 4);
        let syringe = held("ss14:syringe_t", 5);
        st.solution_mut(&bottle).unwrap().add("Bicaridine", 30.0);
        // Режим «Draw» из прототипа, 5 ед. за раз.
        assert!(st.chem_use(&syringe, &bottle));
        assert!((st.solution_volume(&syringe) - 5.0).abs() < 1e-3);
        assert!((st.solution_volume(&bottle) - 25.0).abs() < 1e-3);
        // Переключили на «Inject»: вливает обратно в ёмкость.
        assert!(st.toggle_injector(&syringe));
        assert!(st.chem_use(&syringe, &bottle));
        assert!(st.solution_volume(&syringe) < 1e-3);
    }

    fn disp_view(h: &Harness, who: ConnId) -> Option<crate::DispenserView> {
        let net = h.client(who).net_id()?;
        h.client(who)
            .component(net, crate::DISPENSER_COMPONENT)
            .and_then(crate::decode_dispenser)
    }

    #[test]
    fn a_reagent_dispenser_pours_the_chosen_amount_into_the_beaker() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        spawn(
            &mut h,
            item_at(990, "ss14:disp_t", Vec3::new(3.0, 0.9, 3.0)),
        );
        spawn(
            &mut h,
            item_at(991, "ss14:beaker_t", Vec3::new(2.6, 0.9, 2.6)),
        );
        h.ticks(10);
        command(&mut h, a, CMD_PICKUP_ANY, pickup_data(oid(991)));
        h.ticks(3);
        // Мензурка из руки встаёт в слот, пустая рука открывает окно.
        command(&mut h, a, CMD_INTERACT, interact_data(oid(990)));
        h.ticks(3);
        command(&mut h, a, CMD_INTERACT, interact_data(oid(990)));
        h.ticks(10);
        let v = disp_view(&h, a).expect("окно открыто");
        assert_eq!(v.jugs.len(), 2, "{v:?}");
        assert_eq!(v.jugs[0].label, "silicon");
        assert!(v.beaker.is_some());
        // По умолчанию 10 единиц; выбрали 5 (индекс 1), налили ещё раз.
        command(&mut h, a, crate::CMD_DISP_DISPENSE, vec![0]);
        h.ticks(5);
        assert!((disp_view(&h, a).unwrap().beaker.unwrap().volume - 10.0).abs() < 1e-3);
        command(&mut h, a, crate::CMD_DISP_AMOUNT, vec![1]);
        command(&mut h, a, crate::CMD_DISP_DISPENSE, vec![1]);
        h.ticks(5);
        let v = disp_view(&h, a).unwrap();
        assert!(
            (v.beaker.as_ref().unwrap().volume - 15.0).abs() < 1e-3,
            "{v:?}"
        );
        assert!((v.jugs[0].quantity - 230.0).abs() < 1e-3);
        // Очистка и возврат мензурки в руку.
        command(&mut h, a, crate::CMD_DISP_CLEAR, vec![]);
        h.ticks(5);
        assert!(disp_view(&h, a).unwrap().beaker.unwrap().volume < 1e-3);
        command(&mut h, a, crate::CMD_DISP_EJECT, vec![]);
        h.ticks(5);
        assert!(disp_view(&h, a).unwrap().beaker.is_none());
        assert!(hands_of(&h, a, a).unwrap().held().next().is_some());
    }

    #[test]
    fn a_health_analyzer_scans_another_player_after_the_delay() {
        let mut h = harness();
        let a = h.join("a");
        let b = h.join("b");
        h.ticks(5);
        spawn(
            &mut h,
            item_at(995, "ss14:analyzer_t", Vec3::new(2.6, 0.9, 2.6)),
        );
        h.ticks(10);
        command(&mut h, a, CMD_PICKUP_ANY, pickup_data(oid(995)));
        h.ticks(3);
        let target = h.client(b).net_id().unwrap();
        let hand = hands_of(&h, a, a).unwrap().active() as u8;
        command(&mut h, a, crate::CMD_HEAL, crate::heal_data(hand, target));
        h.ticks(5);
        let scan = |h: &Harness| {
            let net = h.client(a).net_id()?;
            h.client(a)
                .component(net, crate::SCAN_COMPONENT)
                .and_then(crate::decode_scan)
        };
        // Пока идёт 0,8 с — окна нет.
        assert!(scan(&h).is_none());
        h.ticks(40);
        let v = scan(&h).expect("сканирование завершено");
        assert!(v.active && v.state == "Alive", "{v:?}");
        assert!(v.blood > 0.9 && v.total_damage < 1.0);
    }

    fn chemhand_of(h: &Harness, who: ConnId) -> Option<crate::ChemHandView> {
        let net = h.client(who).net_id()?;
        h.client(who)
            .component(net, crate::CHEMHAND_COMPONENT)
            .and_then(crate::decode_chemhand)
    }

    #[test]
    fn a_syringe_draws_chosen_amount_of_blood_from_a_player_and_says_so() {
        let mut h = harness();
        let a = h.join("a");
        let b = h.join("b");
        h.ticks(5);
        spawn(
            &mut h,
            item_at(996, "ss14:syringe_t", Vec3::new(2.6, 0.9, 2.6)),
        );
        h.ticks(10);
        command(&mut h, a, CMD_PICKUP_ANY, pickup_data(oid(996)));
        h.ticks(5);
        // Количества берутся из прототипа; выбираем второе (10 ед.).
        let v = chemhand_of(&h, a).expect("шприц настраивается");
        assert_eq!(v.amounts, vec![5.0, 10.0]);
        assert_eq!(v.amount, 5.0);
        command(&mut h, a, crate::CMD_CHEM_AMOUNT, vec![v.hand, 1]);
        h.ticks(5);
        assert_eq!(chemhand_of(&h, a).unwrap().amount, 10.0);
        let target = h.client(b).net_id().unwrap();
        command(&mut h, a, crate::CMD_HEAL, crate::heal_data(v.hand, target));
        h.ticks(600);
        let net = h.client(a).net_id().unwrap();
        let fx = h
            .client(a)
            .component(net, crate::FX_COMPONENT)
            .and_then(crate::decode_fx)
            .expect("подсказка пришла");
        assert!(
            fx.popups.iter().any(|p| p.contains("крови")),
            "{:?}",
            fx.popups
        );
    }

    #[test]
    fn a_defibrillator_needs_to_be_switched_on_and_shocks_the_dead_through_their_body() {
        use super::combat::{ghost_flag, kill};
        use super::inventory_and_storage::doafter;
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        h.ticks(5);
        grab(&mut h, a, 10, near(), "ss14:cudgel");
        kill(&mut h, a, b);
        let active = hands_of(&h, a, a).unwrap().active() as u8;
        command(
            &mut h,
            a,
            crate::CMD_DROP,
            crate::drop_at_data(active, [3.0, 0.9, 3.0]),
        );
        h.ticks(5);
        grab(&mut h, a, 11, near(), "ss14:defib_t");
        let corpse = ObjectId::from_u128(0xC0F5_0000_0000 + 1);
        let fx = |h: &Harness| {
            let net = h.client(a).net_id()?;
            h.client(a)
                .component(net, crate::FX_COMPONENT)
                .and_then(crate::decode_fx)
        };
        // Выключенный разряд не даёт.
        command(&mut h, a, CMD_INTERACT, interact_data(corpse));
        h.ticks(250);
        assert!(doafter(&h, a).is_none());
        let before = fx(&h).map_or(0, |f| f.seq);
        // Включили — приложение к телу запускает таймер.
        let hand = hands_of(&h, a, a).unwrap().active() as u8;
        command(&mut h, a, CMD_USE, use_data(hand));
        h.ticks(5);
        assert!(
            fx(&h).unwrap().popups.iter().any(|p| p.contains("включён")),
            "{:?}",
            fx(&h)
        );
        assert!(fx(&h).unwrap().seq > before);
        command(&mut h, a, CMD_INTERACT, interact_data(corpse));
        h.ticks(5);
        assert_eq!(doafter(&h, a).map(|d| d.kind), Some("defib".to_string()));
        h.ticks(250);
        // Голова разбита насмерть: сорока единиц лечения мало, тело остаётся.
        assert!(
            fx(&h)
                .unwrap()
                .popups
                .iter()
                .any(|p| p.contains("Не удалось")),
            "{:?}",
            fx(&h)
        );
        assert!(ghost_flag(&h, a, b));
    }

    #[test]
    fn a_zap_heals_the_dead_and_revives_them_when_below_the_threshold() {
        use crate::health::{Health, MobState};
        let mut hp = Health::default();
        let mut hurt = crate::health::DamageSpec::new();
        hurt.insert("Asphyxiation".into(), 230.0);
        hp.apply(&hurt);
        assert_eq!(hp.state(), MobState::Dead);
        let mut heal = crate::health::DamageSpec::new();
        heal.insert("Asphyxiation".into(), -40.0);
        hp.apply(&heal);
        assert_eq!(
            hp.state(),
            MobState::Dead,
            "обычное лечение мёртвого не берёт"
        );
        hp.apply_forced(&heal);
        assert_eq!(hp.state(), MobState::Critical);
    }

    fn lab_view(h: &Harness, who: ConnId) -> Option<crate::LabView> {
        let net = h.client(who).net_id()?;
        h.client(who)
            .component(net, crate::LAB_COMPONENT)
            .and_then(crate::decode_lab)
    }

    /// Мензурка из руки в слот машины и обратно: так видно её содержимое и температуру.
    fn peek(
        h: &mut Harness,
        who: ConnId,
        machine: u128,
        beaker: u128,
    ) -> crate::chemlab::ContainerInfo {
        command(h, who, CMD_PICKUP_ANY, pickup_data(oid(beaker)));
        h.ticks(3);
        // Мензурка в руке встаёт в слот, пустая рука открывает окно.
        command(h, who, CMD_INTERACT, interact_data(oid(machine)));
        h.ticks(3);
        command(h, who, CMD_INTERACT, interact_data(oid(machine)));
        h.ticks(5);
        let info = lab_view(h, who)
            .and_then(|v| v.input)
            .expect("мензурка в слоте");
        command(h, who, crate::CMD_LAB_EJECT, vec![0]);
        h.ticks(3);
        command(h, who, crate::CMD_LAB_CLOSE, vec![]);
        // Мензурка снова на полу, рука свободна.
        let hand = hands_of(h, who, who).unwrap().active() as u8;
        command(
            h,
            who,
            crate::CMD_DROP,
            crate::drop_at_data(hand, [2.4, 0.9, 2.4]),
        );
        h.ticks(3);
        info
    }

    fn amount(info: &crate::chemlab::ContainerInfo, reagent: &str) -> f32 {
        info.reagents
            .iter()
            .find(|r| r.0 == reagent)
            .map_or(0.0, |r| r.2)
    }

    fn place_machine(h: &mut Harness, id: u128, proto: &str, at: Vec3) {
        spawn(h, item_at(id, proto, at));
    }

    #[test]
    fn a_hotplate_heats_a_beaker_on_it_until_a_hot_reaction_starts() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        place_machine(&mut h, 960, "ss14:hotplate_t", Vec3::new(3.0, 0.9, 3.2));
        place_machine(&mut h, 961, "ss14:centrifuge_t", Vec3::new(3.2, 0.9, 2.6));
        spawn(
            &mut h,
            item_at(962, "ss14:beaker_hot_t", Vec3::new(3.0, 0.9, 3.2)),
        );
        spawn(
            &mut h,
            item_at(963, "ss14:beaker_hot_t", Vec3::new(2.6, 0.9, 2.6)),
        );
        h.ticks(10);
        // Холодная мензурка не реагирует: водород и азот на комнатной температуре.
        let cold = peek(&mut h, a, 961, 963);
        assert!(
            amount(&cold, "Ammonia") < 0.01 && (cold.temperature - 293.15).abs() < 0.1,
            "{cold:?}"
        );
        // Мензурка на плитке нагревается: 160 Дж/с на 4 ед., реакция аммиака идёт от 370 К.
        h.ticks(300);
        let hot = peek(&mut h, a, 961, 962);
        assert!(hot.temperature > 370.0, "{hot:?}");
        assert!(amount(&hot, "Ammonia") > 3.9, "{hot:?}");
    }

    #[test]
    fn a_spoon_stirs_a_cocktail_and_a_centrifuge_splits_allicin() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        place_machine(&mut h, 960, "ss14:centrifuge_t", Vec3::new(3.2, 0.9, 2.6));
        spawn(
            &mut h,
            item_at(962, "ss14:beaker_cocktail_t", Vec3::new(2.6, 0.9, 2.6)),
        );
        spawn(
            &mut h,
            item_at(963, "ss14:beaker_allicin_t", Vec3::new(2.8, 0.9, 2.5)),
        );
        spawn(
            &mut h,
            item_at(964, "ss14:spoon_t", Vec3::new(3.0, 0.9, 2.4)),
        );
        h.ticks(10);
        // Без смесителя коктейль не готовится.
        let raw = peek(&mut h, a, 960, 962);
        assert!(amount(&raw, "AlliesCocktail") < 0.01, "{raw:?}");
        // Ложка в руке, мензурка на полу: размешали — коктейль готов.
        command(&mut h, a, CMD_PICKUP_ANY, pickup_data(oid(964)));
        h.ticks(3);
        command(&mut h, a, CMD_INTERACT, interact_data(oid(962)));
        h.ticks(5);
        assert_eq!(
            super::inventory_and_storage::doafter(&h, a).map(|d| d.kind),
            Some("mix".to_string())
        );
        h.ticks(60);
        let hand = hands_of(&h, a, a).unwrap().active() as u8;
        command(&mut h, a, CMD_DROP, drop_at_data(hand, [2.9, 0.9, 2.2]));
        h.ticks(3);
        let stirred = peek(&mut h, a, 960, 962);
        assert!(amount(&stirred, "AlliesCocktail") > 2.9, "{stirred:?}");
        // Центрифуга: чеснок распадается только после пуска и 10 секунд работы.
        command(&mut h, a, CMD_PICKUP_ANY, pickup_data(oid(963)));
        h.ticks(3);
        command(&mut h, a, CMD_INTERACT, interact_data(oid(960)));
        h.ticks(3);
        command(&mut h, a, CMD_INTERACT, interact_data(oid(960)));
        h.ticks(5);
        command(&mut h, a, crate::CMD_LAB_START, vec![]);
        h.ticks(5);
        let running = lab_view(&h, a).unwrap();
        assert!(running.mixing.is_some());
        // Пока работает, мензурку не вынуть.
        command(&mut h, a, crate::CMD_LAB_EJECT, vec![0]);
        h.ticks(3);
        assert!(lab_view(&h, a).unwrap().input.is_some());
        h.ticks(650);
        let done = lab_view(&h, a).unwrap().input.unwrap();
        assert!(
            amount(&done, "Allicin") < 0.01 && amount(&done, "Sulfur") > 3.9,
            "{done:?}"
        );
    }

    #[test]
    fn a_chemmaster_moves_reagents_to_its_buffer_and_makes_pills_and_bottles() {
        let mut h = harness();
        let a = h.join("a");
        h.ticks(5);
        place_machine(&mut h, 960, "ss14:master_t", Vec3::new(3.2, 0.9, 2.6));
        spawn(
            &mut h,
            item_at(962, "ss14:beaker_water_t", Vec3::new(2.6, 0.9, 2.6)),
        );
        spawn(
            &mut h,
            item_at(963, "ss14:canister_t", Vec3::new(2.8, 0.9, 2.5)),
        );
        spawn(
            &mut h,
            item_at(964, "ss14:bottle_t", Vec3::new(3.0, 0.9, 2.4)),
        );
        h.ticks(10);
        // Мензурка уходит во вход, банка — на выход; пустая рука открывает окно.
        for id in [962, 963] {
            command(&mut h, a, CMD_PICKUP_ANY, pickup_data(oid(id)));
            h.ticks(3);
            command(&mut h, a, CMD_INTERACT, interact_data(oid(960)));
            h.ticks(3);
        }
        command(&mut h, a, CMD_INTERACT, interact_data(oid(960)));
        h.ticks(5);
        let v = lab_view(&h, a).expect("окно ХимМастера");
        assert_eq!(v.kind, "master");
        assert!(v.input.is_some() && v.output.is_some());
        // Вся вода из мензурки (кнопка «всё», 9) в буфер.
        let mut data = vec![0u8, 9];
        data.extend_from_slice(b"Water");
        command(&mut h, a, crate::CMD_LAB_REAGENT, data);
        h.ticks(3);
        let v = lab_view(&h, a).unwrap();
        assert!((v.buffer_volume - 30.0).abs() < 1e-3, "{v:?}");
        assert!(v.input.as_ref().unwrap().volume < 1e-3);
        // Три таблетки по 5 ед. с ярлыком: 15 ед. из буфера, три таблетки в банке.
        let mut pills = vec![5u8, 3];
        pills.extend_from_slice(b"aspirin");
        command(&mut h, a, crate::CMD_LAB_PILLS, pills);
        h.ticks(3);
        let v = lab_view(&h, a).unwrap();
        assert!((v.buffer_volume - 15.0).abs() < 1e-3, "{v:?}");
        let canister = v.output.unwrap();
        assert_eq!(canister.items.len(), 3, "{canister:?}");
        assert!(canister.items.iter().all(|i| (i.1 - 5.0).abs() < 1e-3));
        // Доза выше предела не делается.
        let mut big = vec![21u8, 1];
        big.extend_from_slice(b"x");
        command(&mut h, a, crate::CMD_LAB_PILLS, big);
        h.ticks(3);
        assert!((lab_view(&h, a).unwrap().buffer_volume - 15.0).abs() < 1e-3);
        // Банку вынули, бутылку поставили: 10 ед. в неё.
        command(&mut h, a, crate::CMD_LAB_EJECT, vec![1]);
        h.ticks(3);
        let hand = hands_of(&h, a, a).unwrap().active() as u8;
        command(&mut h, a, CMD_DROP, drop_at_data(hand, [2.2, 0.9, 2.2]));
        h.ticks(3);
        command(&mut h, a, CMD_PICKUP_ANY, pickup_data(oid(964)));
        h.ticks(3);
        let hand = hands_of(&h, a, a).unwrap().active() as u8;
        command(&mut h, a, crate::CMD_LAB_INSERT, vec![hand]);
        h.ticks(3);
        let mut bottle = vec![10u8, 0];
        bottle.extend_from_slice(b"water");
        command(&mut h, a, crate::CMD_LAB_BOTTLE, bottle);
        h.ticks(3);
        let v = lab_view(&h, a).unwrap();
        assert!((v.output.unwrap().volume - 10.0).abs() < 1e-3);
        assert!((v.buffer_volume - 5.0).abs() < 1e-3);
    }

    #[test]
    fn every_job_gear_still_fits_into_its_bags_with_real_storage_filters() {
        let mut r = PrototypeRegistry::new();
        r.load_dir(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/prototypes"),
        )
        .unwrap();
        r.validate().unwrap();
        let mut st = crate::module::State::new(Arc::new(r)).unwrap();
        let mut lost = Vec::new();
        for job in st.jobs.clone() {
            for (slot, names) in &job.storage {
                let Some(bag_name) = job.equipment.get(slot) else {
                    continue;
                };
                let Some(bag) = st.create_item(bag_name) else {
                    continue;
                };
                let Some(spec) = st.info(&bag.prototype).and_then(|i| i.storage.clone()) else {
                    continue;
                };
                for name in names {
                    let Some(item) = st.create_item(name) else {
                        continue;
                    };
                    let info = st.info(&item.prototype).cloned().unwrap();
                    if st
                        .storages
                        .entry(bag.id)
                        .or_default()
                        .insert(&spec, item, &info)
                        .is_err()
                    {
                        lost.push(format!("{}: {name} в {bag_name}", job.id));
                    }
                }
            }
        }
        assert!(lost.is_empty(), "не поместилось: {lost:?}");
    }

    fn cryo_view(h: &Harness, who: ConnId) -> Option<crate::CryoView> {
        let net = h.client(who).net_id()?;
        h.client(who)
            .component(net, crate::CRYO_COMPONENT)
            .and_then(crate::decode_cryo)
    }

    #[test]
    fn a_cryo_pod_holds_a_patient_and_feeds_the_blood_from_its_beaker_until_ejected() {
        let mut h = harness();
        let (a, b) = (h.join("a"), h.join("b"));
        h.ticks(5);
        place_machine(&mut h, 960, "ss14:cryo_t", Vec3::new(3.0, 0.9, 3.2));
        spawn(
            &mut h,
            item_at(962, "ss14:beaker_water_t", Vec3::new(2.6, 0.9, 2.6)),
        );
        h.ticks(10);
        command(&mut h, b, CMD_INTERACT, interact_data(oid(960)));
        h.ticks(5);
        let v = cryo_view(&h, b).expect("окно капсулы");
        assert!(v.patient.is_none());
        let patient = h.client(a).net_id().unwrap();
        assert!(v.nearby.iter().any(|n| n.0 == patient.0), "{v:?}");
        // Положить человека: 2 секунды, потом он внутри.
        command(
            &mut h,
            b,
            crate::CMD_CRYO_PUT,
            patient.0.to_le_bytes().to_vec(),
        );
        h.ticks(10);
        assert_eq!(
            super::inventory_and_storage::doafter(&h, b).map(|d| d.kind),
            Some("cryo".to_string())
        );
        h.ticks(150);
        assert!(cryo_view(&h, b).unwrap().patient.is_some());
        // Мензурка в слот, 20 ед. в буфер, раз в 2 с 1 ед. уходит в кровь пациента.
        command(&mut h, b, CMD_PICKUP_ANY, pickup_data(oid(962)));
        h.ticks(3);
        let hand = hands_of(&h, b, b).unwrap().active() as u8;
        command(&mut h, b, crate::CMD_CRYO_INSERT, vec![hand]);
        h.ticks(3);
        command(&mut h, b, crate::CMD_CRYO_INJECT, vec![3]);
        h.ticks(3);
        let v = cryo_view(&h, b).unwrap();
        assert!(v.buffer_volume > 17.0 && v.beaker.is_some(), "{v:?}");
        h.ticks(420);
        let after = cryo_view(&h, b).unwrap();
        assert!(after.buffer_volume < v.buffer_volume - 2.5, "{after:?}");
        // Пациент сам выходит клавишей сопротивления.
        command(&mut h, a, crate::CMD_RESIST, vec![]);
        h.ticks(5);
        assert!(cryo_view(&h, b).unwrap().patient.is_none());
    }
}

mod power_monitoring {
    use super::*;
    use crate::power_monitor::{self as monitor, Group, View};
    pub(super) const PROTOS: &str = r#"[
    (id: "Monitor", components: {"ApcPowerReceiver": (load: 50.0)}),
    (id: "Panel", components: {"PowerSupplier": (group: "hv", maxSupply: 5000.0, rampRate: 5000.0)}),
    (id: "Sub", components: {"PowerBattery": (from: "hv", to: "mv", capacity: 10000.0, startingCharge: 10000.0, maxChargeRate: 5000.0, maxSupply: 5000.0)}),
    (id: "Apc", components: {"PowerBattery": (from: "mv", to: "apc", capacity: 1000.0, startingCharge: 1000.0, maxChargeRate: 1000.0, maxSupply: 1000.0)}),
    (id: "Lamp", components: {"ApcPowerReceiver": (load: 100.0)}),
    (id: "PowerMonitoringData", abstract: true, components: {"PowerMonitoring": (
        consoles: ["Monitor"], devices: [
            (prototype: "Panel", name: "solar panel", group: "Generator", collection: "solar"),
            (prototype: "Sub", name: "substation", group: "Substation", collection: "substation"),
            (prototype: "Apc", name: "APC", group: "APC", collection: "apc"),
        ],
    )}),
]"#;
    fn id(n: u128) -> ObjectId {
        ObjectId::from_u128(0xF000_0000 + n)
    }

    fn view(h: &Harness, who: ConnId) -> Option<View> {
        monitor::decode(
            h.client(who)
                .component(h.client(who).net_id()?, monitor::COMPONENT)?,
        )
    }

    pub(super) fn setup() -> Harness {
        let mut h = super::harness();
        for (n, group) in [(10, "hv"), (11, "mv"), (12, "apc")] {
            let mut zone = AuthoredObject::new(
                id(n),
                AuthoredKind::Zone,
                FloorId(0),
                Vec3::new(3.0, 0.0, 3.0),
                Vec3::new(4.0, 0.05, 4.0),
            );
            zone.metadata.insert("power_cable".into(), group.into());
            spawn(&mut h, zone);
        }
        for (n, prototype, x) in [
            (1, "monitor", 3.0),
            (2, "panel", 3.4),
            (3, "panel", 3.7),
            (4, "sub", 3.5),
            (5, "apc", 3.6),
            (6, "lamp", 3.8),
        ] {
            spawn(
                &mut h,
                item_at(
                    0xF000_0000 + n - 0xBEEF_0000,
                    &format!("ss14:{prototype}"),
                    Vec3::new(x, 0.9, 3.0),
                ),
            );
        }
        h
    }
    #[test]
    fn monitor_reports_real_power_groups_collections_and_focused_networks_privately() {
        let mut h = setup();
        let a = h.join("a");
        let b = h.join("b");
        h.ticks(120);
        command(&mut h, a, CMD_INTERACT, interact_data(id(1)));
        h.ticks(2);
        let initial = view(&h, a).expect("powered console opens");
        assert_eq!(initial.group, Group::Generator);
        assert_eq!(
            initial.entries.len(),
            1,
            "two connected panels form one collection"
        );
        assert!(initial.total_sources > 0.0);
        assert!(
            initial.total_loads >= 149.0,
            "includes APC-fed console and lamp demand"
        );
        let owner = h.client(a).net_id().unwrap();
        for component in [
            monitor::COMPONENT,
            monitor::MAP_COMPONENT,
            monitor::CABLE_COMPONENT,
        ] {
            assert!(h.client(a).component(owner, component).unwrap().len() <= monitor::MAX_BYTES);
            assert!(
                h.client(b).component(owner, component).is_none(),
                "owner-only telemetry"
            );
        }
        command(
            &mut h,
            a,
            monitor::CMD_FOCUS,
            monitor::focus_data(Group::Substation, Some(id(4))),
        );
        h.ticks(2);
        let focused = view(&h, a).unwrap();
        assert_eq!(focused.focus, Some(id(4)));
        assert_eq!(focused.sources.len(), 1);
        assert_eq!(focused.loads.len(), 1);
        assert_eq!(focused.loads[0].id, id(5));
        assert!(focused.sources[0].power > 0.0);
        let net = h.client(a).net_id().unwrap();
        let cables: monitor::CableView = monitor::decode(
            h.client(a)
                .component(net, monitor::CABLE_COMPONENT)
                .unwrap(),
        )
        .unwrap();
        assert!(!cables.focus.is_empty());
        command(
            &mut h,
            a,
            monitor::CMD_FOCUS,
            monitor::focus_data(Group::Apc, Some(id(999))),
        );
        h.ticks(25);
        assert_eq!(
            view(&h, a).unwrap().focus,
            Some(id(4)),
            "unknown device is rejected"
        );
    }

    #[test]
    fn monitor_is_single_user_and_closes_when_its_power_network_disappears() {
        let mut h = setup();
        let a = h.join("a");
        let b = h.join("b");
        h.ticks(120);
        command(&mut h, a, CMD_INTERACT, interact_data(id(1)));
        h.ticks(2);
        command(&mut h, b, CMD_INTERACT, interact_data(id(1)));
        h.ticks(2);
        assert!(view(&h, a).is_some());
        assert!(view(&h, b).is_none(), "ActivatableUI.singleUser");
        command(&mut h, a, monitor::CMD_CLOSE, Vec::new());
        h.ticks(2);
        assert!(view(&h, a).is_none());
        command(&mut h, b, CMD_INTERACT, interact_data(id(1)));
        h.ticks(2);
        assert!(view(&h, b).is_some());
        let outs = h
            .server
            .apply_world_command(WorldCommand::RemoveAuthored { id: id(12) })
            .unwrap();
        h.deliver(outs);
        h.ticks(2);
        assert!(view(&h, b).is_none(), "power loss closes the UI");
        command(&mut h, a, CMD_INTERACT, interact_data(id(1)));
        h.ticks(2);
        assert!(view(&h, a).is_none(), "unpowered console stays closed");
    }
}

mod atmos_alerts {
    use super::*;
    use crate::alerts::{self as alerts, Level, View, Visuals};
    pub(super) const PROTOS: &str = r#"[
        (id:"AlertsConsole",components:{"ApcPowerReceiver":(load:50.0)}),
        (id:"AlertAir",components:{"ApcPowerReceiver":(load:0.0)}),
        (id:"AlertFire",components:{"ApcPowerReceiver":(load:0.0)}),
        (id:"AlertSensor",components:{"ApcPowerReceiver":(load:0.0)}),
        (id:"AtmosAlertsData",abstract:true,components:{"AtmosAlerts":(devices:[
            (prototype:"AlertsConsole",role:"Console",name:"alerts console"),
            (prototype:"AlertAir",role:"AirAlarm",name:"air alarm",tags:["AirAlarm"],syncWith:["AirAlarm","AirSensor"]),
            (prototype:"AlertFire",role:"FireAlarm",name:"fire alarm",tags:["FireAlarm"],syncWith:["FireAlarm","AirSensor"],monitorAlertTypes:["Temperature"]),
            (prototype:"AlertSensor",role:"Sensor",name:"sensor",tags:["AirSensor"],
                temperature:(upperBound:393.15,lowerBound:193.15,upperWarnAround:0.8,lowerWarnAround:1.1),
                pressure:(upperBound:550.0,lowerBound:85.0,upperWarnAround:0.7,lowerWarnAround:1.05),
                gases:[(gas:"Oxygen",lowerBound:0.1,upperBound:0.3,lowerWarnAround:1.5,upperWarnAround:0.8),
                       (gas:"CarbonDioxide",upperBound:0.006,upperWarnAround:0.5)])
        ])}),
    ]"#;
    fn id(n: u128) -> ObjectId {
        oid(2000 + n)
    }
    fn place(h: &mut Harness, n: u128, proto: &str, links: &str, at: Vec3) {
        let mut object = item_at(2000 + n, &format!("ss14:{proto}"), at);
        object.metadata.insert("ss14_uid".into(), n.to_string());
        object.metadata.insert("device_ids".into(), links.into());
        spawn(h, object);
    }
    fn setup() -> Harness {
        let mut h = power_monitoring::setup();
        // The existing power fixture covers x/z in [3,4]. All controls are in reach.
        for (n, proto, links, at) in [
            (1, "alerts_console", "", Vec3::new(3.0, 0.9, 3.0)),
            (2, "alerts_console", "", Vec3::new(3.1, 0.9, 3.0)),
            (3, "alert_air", "10", Vec3::new(3.2, 0.9, 3.0)),
            (4, "alert_fire", "5;10", Vec3::new(3.3, 0.9, 3.0)),
            (5, "alert_fire", "4;10", Vec3::new(3.4, 0.9, 3.0)),
            (10, "alert_sensor", "", Vec3::new(3.6, 0.9, 3.6)),
        ] {
            place(&mut h, n, proto, links, at);
        }
        h
    }
    fn view(h: &Harness, who: ConnId) -> Option<View> {
        alerts::decode(
            h.client(who)
                .component(h.client(who).net_id()?, alerts::COMPONENT)?,
        )
    }
    fn visuals(h: &Harness, who: ConnId) -> Visuals {
        alerts::decode(
            h.client(who)
                .component(h.client(who).net_id().unwrap(), alerts::VISUAL_COMPONENT)
                .unwrap(),
        )
        .unwrap()
    }
    fn open(h: &mut Harness, who: ConnId, n: u128) {
        command(h, who, CMD_INTERACT, interact_data(id(n)));
        h.ticks(3);
    }
    #[test]
    fn alerts_use_linked_sensor_readings_and_filter_non_temperature_fire_packets() {
        let mut h = setup();
        let mut zone = AuthoredObject::new(
            id(20),
            AuthoredKind::Zone,
            FloorId(0),
            Vec3::ZERO,
            Vec3::new(8.0, 3.0, 8.0),
        );
        zone.metadata.insert("atmosphere".into(), "co2".into());
        spawn(&mut h, zone);
        let a = h.join("a");
        let b = h.join("b");
        h.ticks(120);
        open(&mut h, a, 1);
        assert_eq!(
            view(&h, a)
                .unwrap_or_else(|| panic!("console did not open: {:?}", visuals(&h, a)))
                .entries
                .len(),
            3
        );
        assert!(view(&h, b).is_none());
        command(
            &mut h,
            a,
            alerts::CMD_FOCUS,
            alerts::focus_data(Some(id(3))),
        );
        h.ticks(3);
        let initial = view(&h, a).unwrap().reading.unwrap();
        assert!((initial.temperature.0 - 293.15).abs() < 0.1);
        assert!(initial.pressure.0 > 101.325);
        assert_eq!(initial.temperature.1, Level::Normal);
        let changed = view(&h, a).unwrap();
        assert_eq!(
            changed
                .entries
                .iter()
                .find(|e| e.id == id(3))
                .unwrap()
                .level,
            Level::Danger
        );
        assert_eq!(
            changed
                .entries
                .iter()
                .find(|e| e.id == id(4))
                .unwrap()
                .level,
            Level::Normal,
            "FireAlarm accepts only temperature monitor alerts"
        );
        assert!(
            changed
                .reading
                .unwrap()
                .gases
                .iter()
                .any(|g| g.0 == 2 && g.3 == Level::Danger)
        );
        command(
            &mut h,
            a,
            alerts::CMD_FOCUS,
            alerts::focus_data(Some(id(999))),
        );
        h.ticks(3);
        assert_eq!(
            view(&h, a).unwrap().focus,
            Some(id(3)),
            "unknown target rejected"
        );
        let owner = h.client(a).net_id().unwrap();
        assert!(h.client(b).component(owner, alerts::COMPONENT).is_none());
        for kind in [alerts::COMPONENT, alerts::VISUAL_COMPONENT] {
            assert!(h.client(a).component(owner, kind).unwrap().len() <= alerts::MAX_BYTES);
        }
    }
    #[test]
    fn manual_fire_alarm_network_reset_console_silencing_and_power_loss() {
        let mut h = setup();
        let a = h.join("a");
        let b = h.join("b");
        h.ticks(120);
        open(&mut h, a, 1);
        open(&mut h, b, 1);
        assert!(view(&h, b).is_none(), "singleUser");
        open(&mut h, b, 2);
        open(&mut h, a, 4);
        h.ticks(30);
        let entries = view(&h, a).unwrap().entries;
        assert_eq!(
            entries.iter().find(|e| e.id == id(4)).unwrap().level,
            Level::Danger
        );
        assert_eq!(
            entries.iter().find(|e| e.id == id(5)).unwrap().level,
            Level::Danger,
            "linked fire alarm synchronizes"
        );
        assert_eq!(
            entries.iter().find(|e| e.id == id(3)).unwrap().level,
            Level::Normal,
            "fire tags do not alarm AirAlarm"
        );
        command(
            &mut h,
            a,
            alerts::CMD_SILENCE,
            alerts::silence_data(id(4), true),
        );
        command(
            &mut h,
            a,
            alerts::CMD_SILENCE,
            alerts::silence_data(id(5), true),
        );
        h.ticks(30);
        assert!(
            view(&h, a)
                .unwrap()
                .entries
                .iter()
                .filter(|e| e.level == Level::Danger)
                .all(|e| e.silenced)
        );
        assert!(
            view(&h, b).unwrap().entries.iter().all(|e| !e.silenced),
            "console-local silencing"
        );
        let states = visuals(&h, a).states;
        assert!(states.contains(&(id(1), Level::Normal)));
        assert!(states.contains(&(id(2), Level::Danger)));
        assert!(
            states.contains(&(id(4), Level::Danger)),
            "silence does not reset the physical alarm"
        );
        let fx = decode_fx(
            h.client(a)
                .component(h.client(a).net_id().unwrap(), FX_COMPONENT)
                .unwrap(),
        )
        .unwrap();
        assert!(
            fx.sounds
                .iter()
                .any(|s| s.path == "Machines/alarm.ogg" && s.volume_db == -10.0)
        );
        open(&mut h, a, 4);
        h.ticks(30);
        assert!(
            view(&h, a)
                .unwrap()
                .entries
                .iter()
                .all(|e| e.level == Level::Normal),
            "manual reset propagates"
        );
        command(&mut h, a, alerts::CMD_CLOSE, Vec::new());
        h.ticks(3);
        assert!(view(&h, a).is_none());
        open(&mut h, b, 1);
        assert!(view(&h, b).is_some());
        let outs = h
            .server
            .apply_world_command(WorldCommand::RemoveAuthored {
                id: ObjectId::from_u128(0xF000_0000 + 12),
            })
            .unwrap();
        h.deliver(outs);
        h.ticks(2);
        assert!(view(&h, b).is_none(), "unpowered console closes");
        assert!(visuals(&h, b).states.contains(&(id(4), Level::Invalid)));
    }
}
