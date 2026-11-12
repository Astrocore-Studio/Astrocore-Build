use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use astrocore_api::{GameModule, HookContext, HookEvent};
use astrocore_core::{NetworkEntityId, ObjectId, PrototypeId};
use astrocore_math::glam::Vec3;
use astrocore_runtime::modules::DoorAction;
use astrocore_world::{
    AuthoredKind, AuthoredObject, FloorId, OpeningKind, Placement, StructureKind, WorldCommand,
};
use prototypes::PrototypeRegistry;

use crate::body::{Body, Environment, VITALS_COMPONENT, Vitals, encode_vitals};
use crate::doafter::{
    Action, BREAK_DISTANCE_M, DOAFTER_COMPONENT, DoAfter, DoAfterView, PRY_TIME_S, encode_doafter,
};
use crate::edibles::{EdibleSpec, openable_closed, solution_template};
use crate::hands::{Hands, HeldItem};
use crate::health::{
    CORPSE_PROTOTYPE, DEFAULT_ATTACK_RATE, DamageSpec, FIST_DAMAGE, GHOST_COMPONENT,
    HEALTH_COMPONENT, Healing, Health, MELEE_RANGE_M, MOBSTATE_COMPONENT, Melee, MobState,
    RESPAWN_DELAY_S, encode_health,
};
use crate::interact::{InteractionHandler, Target, Used};
use crate::inventory::Inventory;
use crate::items::{ItemInfo, item_info};
use crate::reagents::Solution;
use crate::storage::Storage;
use crate::wire::{
    BagView, CMD_ATTACK, CMD_CANCEL, CMD_DROP, CMD_EQUIP, CMD_FEED, CMD_HEAL, CMD_INTERACT,
    CMD_MOVE_HAND, CMD_MOVE_SLOT, CMD_PICKUP, CMD_PICKUP_ANY, CMD_RESIST, CMD_RESPAWN,
    CMD_SELECT_HAND, CMD_SPILL, CMD_STORE, CMD_SWAP_HANDS, CMD_TAKE, CMD_TAKE_TO, CMD_UNEQUIP,
    CMD_UNEQUIP_TO, CMD_USE, CMD_USE_GROUND, HANDS_COMPONENT, INVENTORY_COMPONENT, InventoryView,
    encode_hands, encode_inventory, parse_attack_part, parse_drop, parse_equip, parse_heal,
    parse_move_slot, parse_object_id, parse_store, parse_take, parse_take_to, parse_unequip,
};

/// Дальность взаимодействия, м: `InteractionRange` в SS14 (1,5). Меряется от груди персонажа
/// до центра предмета; путь не должен перекрываться стенами и закрытыми дверями.
pub const INTERACTION_RANGE_M: f32 = 1.5;
/// Высота груди над ступнями, м.
const CHEST_M: f32 = 1.0;
/// Размер выброшенного предмета в мире, м.
const DROPPED_SIZE_M: f32 = 0.3;
/// На сколько выброшенный предмет отступает от препятствия, м.
const DROP_CLEARANCE_M: f32 = 0.1;

/// Пространство имён идентификаторов прототипов билда в движке.
pub const NAMESPACE: &str = "ss14";

/// Всё, что персонаж держит и носит.
pub(crate) struct Actor {
    pub(crate) hands: Hands,
    pub(crate) inventory: Inventory,
    pub(crate) doafter: Option<DoAfter>,
    pub(crate) health: Health,
    /// Состояние, которое клиентам уже разослано (чтобы замечать переходы).
    pub(crate) mob_state: MobState,
    /// Когда персонаж снова может ударить, с (по часам модуля).
    pub(crate) next_attack: f32,
    /// Заморожен ли персонаж (чтобы замечать перемены).
    pub(crate) frozen: bool,
    /// Когда в следующий раз разослать здоровье из-за кровотечения или удушья, с.
    pub(crate) next_health_publish: f32,
    /// Здоровье изменилось сидящими внутри процессами, а разослать ещё не успели.
    pub(crate) health_dirty: bool,
    /// Умерший летает призраком; тело осталось лежать на месте смерти.
    pub(crate) ghost: bool,
    /// Когда призрак может возродиться, с (по часам модуля), и где умер.
    pub(crate) respawn_at: f32,
    pub(crate) death_pos: Option<Vec3>,
    /// Физиология: дыхание, желудок, сытость, баротравма.
    pub(crate) body: Option<Body>,
    /// Последнее разосланное состояние тела (чтобы не слать повторы).
    pub(crate) last_vitals: Option<Vitals>,
    /// Пока не прошло это время (по часам модуля), персонаж сбит с ног лужей.
    pub(crate) slip_until: f32,
    /// Где был на прошлом шаге: клетка и точка — для скорости и входа на клетку.
    pub(crate) last_step: Option<(atmos::Cell, Vec3)>,
}

impl Actor {
    fn new(seed: u32) -> Self {
        Self {
            hands: Hands::humanoid(),
            inventory: Inventory::human(),
            doafter: None,
            health: Health::default(),
            mob_state: MobState::Alive,
            next_attack: 0.0,
            frozen: false,
            next_health_publish: 0.0,
            health_dirty: false,
            ghost: false,
            respawn_at: 0.0,
            death_pos: None,
            body: Some(Body::new(seed)),
            last_vitals: None,
            slip_until: 0.0,
            last_step: None,
        }
    }
}

/// Торговый автомат по прототипу.
#[derive(Debug, Clone)]
pub(crate) struct VendSpec {
    pub(crate) title: String,
    pub(crate) stock: Vec<(String, PrototypeId, u32)>,
    pub(crate) quality: f32,
    pub(crate) eject_delay: f32,
}

/// Ёмкость диспозала в мире: состояние, клетка и точка.
#[derive(Debug, Clone)]
pub(crate) struct DisposalSlot {
    pub(crate) unit: crate::disposal::Unit,
    pub(crate) cell: atmos::Cell,
    pub(crate) at: Vec3,
}

/// Группа сигнализации: сама сигнализация, её датчики, вентили, скрубберы и огнезапоры из списка.
#[derive(Debug, Clone, Default)]
pub(crate) struct AlarmGroup {
    pub(crate) monitors: Vec<(ObjectId, atmos::Cell)>,
    pub(crate) devices: Vec<ObjectId>,
    pub(crate) firelocks: Vec<ObjectId>,
    pub(crate) state: crate::monitor::AlarmState,
    /// Состояние уже применено к огнезапорам (при старте они закрыты, в SS14 — открыты).
    pub(crate) applied: bool,
}

/// Игровой модуль рук, инвентаря и хранилищ; логика следует Content.Shared SS14.
///
/// Предметы на земле — авторские объекты мира (`AuthoredKind::Placement`): они реплицируются,
/// сохраняются и валидируются движком. Предметы в руках, на теле и в сумках живут в состоянии
/// модуля; руки видны всем (`build.hands`), инвентарь и содержимое сумок — только владельцу
/// (`build.inventory`).
pub struct State {
    pub(crate) infos: BTreeMap<PrototypeId, ItemInfo>,
    pub(crate) range_m: f32,
    pub(crate) actors: BTreeMap<NetworkEntityId, Actor>,
    /// Содержимое сумок по идентификатору сумки: переживает смену владельца и выброс сумки.
    pub(crate) storages: BTreeMap<ObjectId, Storage>,
    /// Задержки надевания и снятия одежды, с: (`equipDelay`, `unequipDelay`) из `Clothing`.
    pub(crate) delays: BTreeMap<PrototypeId, (f32, f32)>,
    pub(crate) melee: BTreeMap<PrototypeId, Melee>,
    pub(crate) healing: BTreeMap<PrototypeId, Healing>,
    /// Часы модуля, с: копятся по `Tick`.
    pub(crate) clock: f32,
    /// Сколько применений осталось у начатых стопок лечения (ключ — предмет).
    pub(crate) charges: BTreeMap<ObjectId, u32>,
    /// Куда возрождаются; без точки возрождение недоступно.
    pub(crate) spawn: Option<Vec3>,
    /// Сколько тел оставлено (для идентификаторов).
    pub(crate) corpses: u32,
    /// Атмосфера по этажам.
    pub(crate) atmos: atmos::Grid,
    /// Какой множитель скорости уже отправлен движку.
    pub(crate) speed_sent: BTreeMap<NetworkEntityId, f32>,
    pub(crate) edibles: BTreeMap<PrototypeId, EdibleSpec>,
    pub(crate) solution_templates: BTreeMap<PrototypeId, Solution>,
    /// Прототипы, которые открываются и закрываются (бутылки).
    pub(crate) openable: BTreeMap<PrototypeId, bool>,
    /// Растворы предметов (создаются при первом обращении).
    pub(crate) solutions: BTreeMap<ObjectId, Solution>,
    /// Предметы, которые уже открыты.
    pub(crate) opened: BTreeSet<ObjectId>,
    /// Температура огня у прототипов с `IgnitionSource` (сварка, спички) и зажжённые экземпляры.
    pub(crate) igniters: BTreeMap<PrototypeId, f32>,
    pub(crate) lit: BTreeSet<ObjectId>,
    /// Какие клетки с огнём уже разосланы.
    pub(crate) fires_sent: Vec<crate::fire::FireView>,
    pub(crate) fires_to: BTreeSet<NetworkEntityId>,
    /// Трубы и газовые устройства: разбор прототипов, ключи объектов, подпись мира.
    pub(crate) pipe_parts: BTreeMap<PrototypeId, crate::piping::PipePart>,
    pub(crate) pipe_keys: crate::piping::PipeKeys,
    pub(crate) pipe_sig: Vec<(ObjectId, atmos::Cell, u8)>,
    /// Переносимые баллончики: шаблоны и экземпляры (создаются при первом обращении).
    pub(crate) tank_templates: BTreeMap<PrototypeId, atmos::Tank>,
    pub(crate) tanks: BTreeMap<ObjectId, atmos::Tank>,
    pub(crate) tank_timer: f32,
    pub(crate) pipe_ports: BTreeMap<atmos::Cell, u64>,
    pub(crate) puddles: crate::puddles::Puddles,
    pub(crate) puddles_sent: Vec<crate::puddles::PuddleView>,
    pub(crate) puddles_to: BTreeSet<NetworkEntityId>,
    pub(crate) puddle_timer: f32,
    /// Швабры (`Absorbent`): сколько забирают за раз; ёмкости (`Refillable`); задержка применения.
    pub(crate) absorbents: BTreeMap<PrototypeId, f32>,
    pub(crate) refillables: BTreeSet<PrototypeId>,
    pub(crate) mop_ready: BTreeMap<ObjectId, f32>,
    /// Защита одежды от тепла (`TemperatureProtection.coefficient`).
    pub(crate) heat_protection: BTreeMap<PrototypeId, f32>,
    /// Электричество: разбор прототипов, состояние решателя, подпись мира, заряды аккумуляторов.
    pub(crate) power_parts: BTreeMap<PrototypeId, crate::electric::PowerPart>,
    pub(crate) power: power::PowerState,
    pub(crate) power_sig: Vec<(ObjectId, atmos::Cell, u8)>,
    pub(crate) power_batteries: Vec<(ObjectId, usize)>,
    pub(crate) power_receivers: Vec<(ObjectId, usize, f32)>,
    pub(crate) power_charge: BTreeMap<ObjectId, f32>,
    /// На карте есть проводка: устройства с `ApcPowerReceiver` требуют питания.
    pub(crate) power_required: bool,
    /// Объекты с `ApcPowerReceiver`: при проводке на карте без питания не работают.
    pub(crate) power_gated: std::collections::BTreeSet<ObjectId>,
    /// Светильники мира: объект, точка, вид (по палитре `power_kinds`).
    /// Мониторы воздуха (сигнализации и датчики), группы сигнализаций и их состояние.
    pub(crate) monitor_kinds: BTreeMap<PrototypeId, bool>,
    pub(crate) alarm_groups: Vec<AlarmGroup>,
    pub(crate) alarm_sig: Vec<(ObjectId, atmos::Cell, u32)>,
    pub(crate) monitor_state: BTreeMap<ObjectId, crate::monitor::AlarmState>,
    pub(crate) alarm_timer: f32,
    /// Должности карты, занятость мест, должность каждого игрока, заполнение контейнеров.
    pub(crate) jobs: Vec<crate::jobs::JobSpec>,
    pub(crate) job_taken: BTreeMap<String, u32>,
    pub(crate) player_job: BTreeMap<NetworkEntityId, String>,
    pub(crate) jobs_sent: BTreeMap<NetworkEntityId, Vec<u8>>,
    pub(crate) fills: BTreeMap<PrototypeId, Vec<crate::spawners::FillEntry>>,
    pub(crate) item_serial: u128,
    /// Спавнеры карты, их таблицы и имя прототипа SS14 → идентификатор движка.
    pub(crate) spawner_specs: BTreeMap<PrototypeId, crate::spawners::Spawner>,
    pub(crate) spawner_tables: BTreeMap<String, crate::spawners::Node>,
    pub(crate) proto_by_name: BTreeMap<String, PrototypeId>,
    pub(crate) spawners_done: bool,
    pub(crate) spawn_timer: f32,
    pub(crate) last_dt: f32,
    pub(crate) spawn_serial: u128,
    pub(crate) spawn_rng: crate::disposal::Rng,
    /// Торговые автоматы: описания по прототипам, автоматы в мире, кто какой смотрит.
    pub(crate) vending_specs: BTreeMap<PrototypeId, VendSpec>,
    pub(crate) vending: BTreeMap<ObjectId, crate::vending::Machine>,
    pub(crate) vending_open: BTreeMap<NetworkEntityId, ObjectId>,
    pub(crate) vending_sent: BTreeMap<NetworkEntityId, Vec<u8>>,
    pub(crate) vending_serial: u128,
    pub(crate) vending_rng: crate::disposal::Rng,
    /// Конвейеры: прототипы лент (скорость), выключатели, состояние лент и включённые выключатели.
    pub(crate) conveyor_protos: BTreeMap<PrototypeId, f32>,
    pub(crate) switch_protos: BTreeSet<PrototypeId>,
    pub(crate) conveyor_state: BTreeMap<ObjectId, crate::conveyor::State>,
    pub(crate) switch_on: BTreeSet<ObjectId>,
    pub(crate) conveyor_timer: f32,
    /// Диспозал: прототипы ёмкостей, сеть труб, ёмкости, холдеры в пути.
    pub(crate) disposal_units: BTreeSet<PrototypeId>,
    pub(crate) disposal_net: crate::disposal::Net,
    pub(crate) disposal_entries: BTreeSet<atmos::Cell>,
    pub(crate) disposal_torn: BTreeSet<atmos::Cell>,
    pub(crate) disposal_sig: Vec<(atmos::Cell, String)>,
    pub(crate) disposal_state: BTreeMap<ObjectId, DisposalSlot>,
    pub(crate) disposal_holders: Vec<crate::disposal::Holder>,
    pub(crate) disposal_rng: crate::disposal::Rng,
    pub(crate) disposal_timer: f32,
    /// Игроки, уже получившие место появления.
    pub(crate) placed: BTreeSet<NetworkEntityId>,
    pub(crate) spawn_next: usize,
    /// Огнезапоры: клетки по обе стороны проёма и что они удерживают (давление, температура).
    pub(crate) firelock_cells: BTreeMap<ObjectId, (atmos::Cell, atmos::Cell)>,
    pub(crate) firelock_hold: BTreeMap<ObjectId, (bool, bool)>,
    pub(crate) firelock_timer: f32,
    pub(crate) power_lights: Vec<(ObjectId, Vec3, u8)>,
    pub(crate) power_kinds: Vec<[f32; 5]>,
    pub(crate) lights_sent: crate::electric::LightsView,
    pub(crate) lights_timer: f32,
    pub(crate) lights_to: BTreeSet<NetworkEntityId>,
}

impl State {
    /// `Err`, если два имени прототипов дают один идентификатор движка.
    pub(crate) fn new(registry: Arc<PrototypeRegistry>) -> Result<Self, prototypes::ProtoError> {
        let names = registry.engine_index(NAMESPACE)?;
        let by_name: BTreeMap<String, PrototypeId> = names
            .iter()
            .map(|(id, n)| (n.clone(), id.clone()))
            .collect();
        let mut vending_specs = BTreeMap::new();
        let mut jobs = Vec::new();
        let mut fills = BTreeMap::new();
        let mut spawner_specs = BTreeMap::new();
        let mut spawner_tables = BTreeMap::new();
        let mut infos = BTreeMap::new();
        let mut delays = BTreeMap::new();
        let mut melee = BTreeMap::new();
        let mut healing = BTreeMap::new();
        let mut edibles = BTreeMap::new();
        let mut solution_templates = BTreeMap::new();
        let mut openable = BTreeMap::new();
        let mut igniters = BTreeMap::new();
        let mut pipe_parts = BTreeMap::new();
        let mut absorbents = BTreeMap::new();
        let mut heat_protection = BTreeMap::new();
        let mut power_parts = BTreeMap::new();
        let mut monitor_kinds = BTreeMap::new();
        let mut disposal_units = BTreeSet::new();
        let mut conveyor_protos = BTreeMap::new();
        let mut switch_protos = BTreeSet::new();
        let mut refillables = BTreeSet::new();
        let mut tank_templates = BTreeMap::new();
        for (id, name) in names {
            let resolved = registry.spawnable(&name).ok();
            if let Some(info) = resolved.as_ref().and_then(item_info) {
                infos.insert(id.clone(), info);
            }
            if let Some(r) = resolved.as_ref() {
                if let Some(e) = EdibleSpec::from_resolved(r) {
                    edibles.insert(id.clone(), e);
                }
                if let Some(s) = solution_template(r) {
                    solution_templates.insert(id.clone(), s);
                }
                if let Some(closed) = openable_closed(r) {
                    openable.insert(id.clone(), closed);
                }
                if let Some(t) = crate::fire::ignition_temperature(r) {
                    igniters.insert(id.clone(), t);
                }
                if let Some(p) = crate::mopping::pickup_amount(r) {
                    absorbents.insert(id.clone(), p);
                }
                if let Some(c) = r
                    .component("TemperatureProtection")
                    .and_then(|c| c.get("coefficient"))
                    .and_then(prototypes::Data::as_f64)
                {
                    heat_protection.insert(id.clone(), c as f32);
                }
                if let Some(j) = r
                    .component("Job")
                    .and_then(|c| crate::jobs::JobSpec::parse(&name, c))
                {
                    jobs.push(j);
                }
                if let Some(c) = r.component("StorageFill") {
                    let entries = crate::spawners::FillEntry::parse_list(c);
                    if !entries.is_empty() {
                        fills.insert(id.clone(), entries);
                    }
                }
                if let Some(s) = r
                    .component("Spawner")
                    .and_then(crate::spawners::Spawner::parse)
                {
                    spawner_specs.insert(id.clone(), s);
                }
                if let Some(t) = r
                    .component("EntityTable")
                    .and_then(|c| c.get("table"))
                    .and_then(crate::spawners::Node::parse)
                    && let Some(table_id) = name.strip_prefix("Table")
                {
                    spawner_tables.insert(table_id.to_string(), t);
                }
                if let Some(c) = r.component("VendingMachine") {
                    let stock = c
                        .get("stock")
                        .and_then(prototypes::Data::as_map)
                        .map(|s| {
                            s.iter()
                                .filter_map(|(k, v)| {
                                    // Нет такого прототипа в Build — позиция пропускается (`HasIndex`).
                                    registry.spawnable(k).ok()?;
                                    Some((k.clone(), by_name.get(k)?.clone(), v.as_f64()? as u32))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    vending_specs.insert(
                        id.clone(),
                        VendSpec {
                            title: name.clone(),
                            stock,
                            quality: c
                                .get("initialStockQuality")
                                .and_then(prototypes::Data::as_f64)
                                .unwrap_or(1.0) as f32,
                            eject_delay: c
                                .get("ejectDelay")
                                .and_then(prototypes::Data::as_f64)
                                .unwrap_or(1.2) as f32,
                        },
                    );
                }
                if let Some(c) = r.component("Conveyor") {
                    conveyor_protos.insert(
                        id.clone(),
                        c.get("speed")
                            .and_then(prototypes::Data::as_f64)
                            .unwrap_or(2.0) as f32,
                    );
                }
                if r.component("SignalSwitch").is_some() {
                    switch_protos.insert(id.clone());
                }
                if r.component("DisposalUnit").is_some() {
                    disposal_units.insert(id.clone());
                }
                if let Some(c) = r.component("AtmosMonitor") {
                    monitor_kinds.insert(
                        id.clone(),
                        c.get("kind").and_then(prototypes::Data::as_str) == Some("alarm"),
                    );
                }
                if let Some(p) = crate::electric::PowerPart::from_resolved(r) {
                    power_parts.insert(id.clone(), p);
                }
                if crate::mopping::is_refillable(r) {
                    refillables.insert(id.clone());
                }
                if let Some(p) = crate::piping::PipePart::from_resolved(r) {
                    pipe_parts.insert(id.clone(), p);
                }
                if let Some(t) = crate::piping::tank_template(r) {
                    tank_templates.insert(id.clone(), t);
                }
            }
            let delay = |key: &str| {
                resolved
                    .as_ref()
                    .and_then(|r| r.component("Clothing"))
                    .and_then(|c| c.get(key))
                    .and_then(prototypes::Data::as_f64)
                    .map_or(0.0, |v| v.max(0.0) as f32)
            };
            if let Some(c) = resolved.as_ref().and_then(|r| r.component("MeleeWeapon")) {
                melee.insert(
                    id.clone(),
                    Melee {
                        damage: damage_spec(c.get("damage")),
                        rate: c
                            .get("attackRate")
                            .and_then(prototypes::Data::as_f64)
                            .map_or(DEFAULT_ATTACK_RATE, |v| v.max(0.1) as f32),
                    },
                );
            }
            if let Some(c) = resolved.as_ref().and_then(|r| r.component("Healing")) {
                healing.insert(
                    id.clone(),
                    Healing {
                        damage: damage_spec(c.get("damage")),
                        delay: c
                            .get("delay")
                            .and_then(prototypes::Data::as_f64)
                            .map_or(3.0, |v| v.max(0.0) as f32),
                        bleed_modifier: c
                            .get("bloodlossModifier")
                            .and_then(prototypes::Data::as_f64)
                            .map_or(0.0, |v| v as f32),
                        // Размер стопки — `Stack.count`, как у лечащих предметов SS14.
                        uses: resolved
                            .as_ref()
                            .and_then(|r| r.component("Stack"))
                            .and_then(|s| s.get("count"))
                            .and_then(prototypes::Data::as_int)
                            .map_or(1, |n| n.clamp(1, 1000) as u32),
                    },
                );
            }
            let pair = (delay("equipDelay"), delay("unequipDelay"));
            if pair != (0.0, 0.0) {
                delays.insert(id, pair);
            }
        }
        Ok(Self {
            infos,
            range_m: INTERACTION_RANGE_M,
            actors: BTreeMap::new(),
            storages: BTreeMap::new(),
            delays,
            melee,
            healing,
            clock: 0.0,
            charges: BTreeMap::new(),
            spawn: None,
            corpses: 0,
            atmos: atmos::Grid::new(),
            speed_sent: BTreeMap::new(),
            edibles,
            solution_templates,
            openable,
            solutions: BTreeMap::new(),
            opened: BTreeSet::new(),
            igniters,
            lit: BTreeSet::new(),
            fires_sent: Vec::new(),
            fires_to: BTreeSet::new(),
            pipe_parts,
            pipe_keys: crate::piping::PipeKeys::default(),
            pipe_sig: Vec::new(),
            tank_templates,
            tanks: BTreeMap::new(),
            tank_timer: 0.0,
            pipe_ports: BTreeMap::new(),
            puddles: crate::puddles::Puddles::new(),
            puddles_sent: Vec::new(),
            puddles_to: BTreeSet::new(),
            puddle_timer: 0.0,
            absorbents,
            refillables,
            mop_ready: BTreeMap::new(),
            heat_protection,
            power_parts,
            power: power::PowerState::default(),
            power_sig: Vec::new(),
            power_batteries: Vec::new(),
            power_receivers: Vec::new(),
            power_charge: BTreeMap::new(),
            power_required: false,
            power_gated: std::collections::BTreeSet::new(),
            monitor_kinds,
            alarm_groups: Vec::new(),
            alarm_sig: Vec::new(),
            monitor_state: BTreeMap::new(),
            alarm_timer: 0.0,
            jobs: {
                let mut j = jobs;
                j.sort_by(|a, b| b.weight.cmp(&a.weight).then(a.id.cmp(&b.id)));
                j
            },
            job_taken: BTreeMap::new(),
            player_job: BTreeMap::new(),
            jobs_sent: BTreeMap::new(),
            fills,
            item_serial: 0,
            spawner_specs,
            spawner_tables,
            proto_by_name: by_name.clone(),
            spawners_done: false,
            spawn_timer: 0.0,
            last_dt: 0.0,
            spawn_serial: 0,
            spawn_rng: crate::disposal::Rng(0x1234_5678_9abc_def1),
            vending_specs,
            vending: BTreeMap::new(),
            vending_open: BTreeMap::new(),
            vending_sent: BTreeMap::new(),
            vending_serial: 0,
            vending_rng: crate::disposal::Rng(0x2545_f491_4f6c_dd1d),
            conveyor_protos,
            switch_protos,
            conveyor_state: BTreeMap::new(),
            switch_on: BTreeSet::new(),
            conveyor_timer: 0.0,
            disposal_units,
            disposal_net: crate::disposal::Net::default(),
            disposal_entries: BTreeSet::new(),
            disposal_torn: BTreeSet::new(),
            disposal_sig: Vec::new(),
            disposal_state: BTreeMap::new(),
            disposal_holders: Vec::new(),
            disposal_rng: crate::disposal::Rng(0x9e37_79b9_7f4a_7c15),
            disposal_timer: 0.0,
            placed: BTreeSet::new(),
            spawn_next: 0,
            firelock_cells: BTreeMap::new(),
            firelock_hold: BTreeMap::new(),
            firelock_timer: 0.0,
            power_lights: Vec::new(),
            power_kinds: Vec::new(),
            lights_sent: crate::electric::LightsView::default(),
            lights_timer: 0.0,
            lights_to: BTreeSet::new(),
        })
    }

    #[must_use]
    pub fn with_range(mut self, range_m: f32) -> Self {
        self.range_m = range_m;
        self
    }

    pub fn hands_of(&self, actor: NetworkEntityId) -> Option<&Hands> {
        self.actors.get(&actor).map(|a| &a.hands)
    }

    pub fn inventory_of(&self, actor: NetworkEntityId) -> Option<&Inventory> {
        self.actors.get(&actor).map(|a| &a.inventory)
    }

    pub fn storage_of(&self, bag: ObjectId) -> Option<&Storage> {
        self.storages.get(&bag)
    }

    /// Предмет в активной руке: рука и сам предмет.
    pub(crate) fn used_item(&self, actor: NetworkEntityId) -> Option<Used> {
        let a = self.actors.get(&actor)?;
        let hand = a.hands.active();
        a.hands.get(hand).map(|i| Used {
            hand,
            item: i.clone(),
        })
    }

    pub(crate) fn used_in(&self, actor: NetworkEntityId, hand: usize) -> Option<Used> {
        self.actors.get(&actor)?.hands.get(hand).map(|i| Used {
            hand,
            item: i.clone(),
        })
    }

    pub(crate) fn actor(&mut self, id: NetworkEntityId) -> &mut Actor {
        self.actors
            .entry(id)
            .or_insert_with(|| Actor::new(id.0.wrapping_mul(2_246_822_519) ^ 0x85eb_ca6b))
    }

    pub(crate) fn info(&self, proto: &PrototypeId) -> Option<&ItemInfo> {
        self.infos.get(proto)
    }

    /// Рассылка состояния: руки всем, инвентарь с содержимым сумок — владельцу.
    pub(crate) fn publish(&self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        let Some(a) = self.actors.get(&actor) else {
            return;
        };
        ctx.set_component(actor, HANDS_COMPONENT, encode_hands(&a.hands));
        let mut health_view = a.health.view();
        health_view.ghost = a.ghost;
        health_view.suffocating = a.body.as_ref().is_some_and(|b| b.low_oxygen);
        health_view.respawn_in = if a.ghost {
            (a.respawn_at - self.clock).max(0.0)
        } else {
            0.0
        };
        ctx.set_component(actor, HEALTH_COMPONENT, encode_health(&health_view));
        ctx.set_component(actor, MOBSTATE_COMPONENT, vec![a.health.state().to_byte()]);
        ctx.set_component(actor, GHOST_COMPONENT, vec![u8::from(a.ghost)]);
        ctx.set_component(
            actor,
            crate::fire::BURNING_COMPONENT,
            vec![u8::from(a.body.as_ref().is_some_and(|b| b.on_fire))],
        );
        if let Some(body) = a.body.as_ref() {
            ctx.set_component(actor, VITALS_COMPONENT, encode_vitals(&body.vitals()));
        }
        let worn: crate::wire::Worn = a
            .inventory
            .iter()
            .filter_map(|(d, i)| Some((d.name.clone(), i?.prototype.to_string())))
            .collect();
        ctx.set_component(
            actor,
            crate::wire::WORN_COMPONENT,
            crate::wire::encode_worn(&worn),
        );
        let bags = a
            .hands
            .held()
            .chain(a.inventory.worn())
            .filter(|i| self.info(&i.prototype).is_some_and(|x| x.storage.is_some()))
            .map(|i| BagView {
                id: i.id,
                grid: self
                    .info(&i.prototype)
                    .and_then(|x| x.storage.as_ref())
                    .map(|s| s.cells.iter().copied().collect())
                    .unwrap_or_default(),
                items: self
                    .storages
                    .get(&i.id)
                    .map(|s| s.items().to_vec())
                    .unwrap_or_default(),
            })
            .collect();
        let view = InventoryView {
            inventory: a.inventory.clone(),
            bags,
            charges: self.charges.iter().map(|(id, n)| (*id, *n)).collect(),
            fills: a
                .hands
                .held()
                .chain(a.inventory.worn())
                .filter_map(|i| {
                    let t = self.solution_templates.get(&i.prototype)?;
                    let vol = self
                        .solutions
                        .get(&i.id)
                        .map_or_else(|| t.volume(), Solution::volume);
                    Some((i.id, vol, t.max_volume, self.is_closed(i)))
                })
                .collect(),
        };
        ctx.set_component(actor, INVENTORY_COMPONENT, encode_inventory(&view));
    }

    /// Предмет в зоне досягаемости: близко, без преград между грудью и центром объекта.
    pub(crate) fn reachable(
        &self,
        ctx: &HookContext<'_>,
        feet: Vec3,
        obj: &AuthoredObject,
    ) -> bool {
        let chest = feet + Vec3::Y * CHEST_M;
        let center = (obj.min() + obj.max()) * 0.5;
        (center - chest).length() <= self.range_m && ctx.is_unobstructed(chest, center)
    }

    /// Подбор. `any = false` — клик (только активная рука), `true` — глагол «поднять».
    pub(crate) fn pickup(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
        any: bool,
    ) -> bool {
        let (Some(pos), Some(obj)) = (ctx.position_of(actor), ctx.world.authored_object(id)) else {
            return false;
        };
        let AuthoredKind::Placement { prototype } = &obj.kind else {
            return false;
        };
        if self.info(prototype).is_none() || !self.reachable(ctx, pos, obj) {
            return false;
        }
        let item = HeldItem {
            id,
            prototype: prototype.clone(),
        };
        let hands = &mut self.actor(actor).hands;
        let taken = if any {
            hands.pickup_any(item)
        } else {
            hands.pickup_active(item)
        };
        if taken.is_err() {
            return false;
        }
        ctx.request(WorldCommand::RemoveAuthored { id });
        self.publish(ctx, actor);
        true
    }

    /// Положить предмет в мир в точке `at` (ступни предмета).
    pub(crate) fn place(ctx: &mut HookContext<'_>, item: &HeldItem, at: Vec3) -> bool {
        let Some(floor) = floor_at(ctx, at.y) else {
            return false;
        };
        let h = DROPPED_SIZE_M * 0.5;
        let obj = AuthoredObject::new(
            item.id,
            AuthoredKind::Placement {
                prototype: item.prototype.clone(),
            },
            floor,
            Vec3::new(at.x - h, at.y, at.z - h),
            Vec3::new(at.x + h, at.y + DROPPED_SIZE_M, at.z + h),
        );
        ctx.request(WorldCommand::PlaceAuthored(obj));
        true
    }

    /// Куда ляжет выброшенный предмет (`GetFinalDropCoordinates` в SS14): цель обрезается
    /// дальностью взаимодействия и укорачивается до ближайшей преграды; без цели — под ноги.
    pub(crate) fn drop_point(
        &self,
        ctx: &HookContext<'_>,
        feet: Vec3,
        target: Option<[f32; 3]>,
    ) -> Vec3 {
        let Some([tx, _, tz]) = target else {
            return feet;
        };
        let chest = feet + Vec3::Y * CHEST_M;
        let want = Vec3::new(tx, chest.y, tz) - chest;
        let len = want.length();
        if len < 1e-4 {
            return feet;
        }
        let dir = want / len;
        let reach = len.min(self.range_m);
        let free = ctx
            .unobstructed_distance(chest, chest + dir * reach)
            .min(reach);
        let dist = if free < reach {
            (free - DROP_CLEARANCE_M).max(0.0)
        } else {
            free
        };
        let p = chest + dir * dist;
        Vec3::new(p.x, feet.y, p.z)
    }

    pub(crate) fn drop_hand(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        target: Option<[f32; 3]>,
    ) {
        let (Some(feet), Some(a)) = (ctx.position_of(actor), self.actors.get(&actor)) else {
            return;
        };
        let Some(item) = a.hands.get(hand).cloned() else {
            return;
        };
        let at = self.drop_point(ctx, feet, target);
        if Self::place(ctx, &item, at) {
            let _ = self.actor(actor).hands.drop_hand(hand);
            self.publish(ctx, actor);
        }
    }

    /// Надеть предмет из руки в слот (`CanEquip` + `TryEquip` в SS14).
    pub(crate) fn equip(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        slot: &str,
    ) {
        let Some(a) = self.actors.get(&actor) else {
            return;
        };
        let Some(item) = a.hands.get(hand).cloned() else {
            return;
        };
        let Some(info) = self.info(&item.prototype).cloned() else {
            return;
        };
        if a.inventory.can_equip(slot, &info).is_err() {
            return;
        }
        let a = self.actor(actor);
        let _ = a.hands.drop_hand(hand);
        let _ = a.inventory.equip(slot, item, &info);
        self.publish(ctx, actor);
    }

    /// Снять предмет: в свободную руку (`PickupOrDrop`), иначе на пол; зависимые слоты
    /// выпадают на пол.
    pub(crate) fn unequip(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        slot: &str,
        hand: Option<usize>,
    ) {
        let (Some(feet), Some(a)) = (ctx.position_of(actor), self.actors.get(&actor)) else {
            return;
        };
        let Some(worn) = a.inventory.get(slot) else {
            return;
        };
        let needs_floor = a.hands.order().all(|h| a.hands.get(h).is_some())
            || a.inventory
                .iter()
                .any(|(d, i)| d.depends_on.as_deref() == Some(slot) && i.is_some());
        // Нужен пол, а этажа под ногами нет: ничего не меняем, чтобы предметы не пропали.
        if needs_floor && floor_at(ctx, feet.y).is_none() {
            return;
        }
        let _ = worn;
        let a = self.actor(actor);
        let Ok((item, dropped)) = a.inventory.unequip(slot) else {
            return;
        };
        let picked = match hand {
            Some(h) => a
                .hands
                .pickup_to(h, item.clone())
                .or_else(|_| a.hands.pickup_any(item.clone())),
            None => a.hands.pickup_any(item.clone()),
        };
        let to_floor = match picked {
            Ok(_) => dropped,
            Err(_) => std::iter::once(item).chain(dropped).collect(),
        };
        for i in &to_floor {
            Self::place(ctx, i, feet);
        }
        self.publish(ctx, actor);
    }

    /// Переложить надетое из слота в слот. Всё или ничего: если у снимаемого есть зависимые слоты с
    /// предметами (они бы выпали) или предмет не подходит к новому слоту, ничего не меняется.
    pub(crate) fn move_slot(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        from: &str,
        to: &str,
    ) {
        let Some(a) = self.actors.get(&actor) else {
            return;
        };
        let Some(item) = a.inventory.get(from).cloned() else {
            return;
        };
        let Some(info) = self.info(&item.prototype).cloned() else {
            return;
        };
        let mut next = a.inventory.clone();
        let Ok((item, dropped)) = next.unequip(from) else {
            return;
        };
        if !dropped.is_empty() || next.equip(to, item, &info).is_err() {
            return;
        }
        self.actor(actor).inventory = next;
        self.publish(ctx, actor);
    }

    /// Дотягивается ли `actor` до `target` (себя — всегда): расстояние между грудью и прямая видимость.
    fn in_reach(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        target: NetworkEntityId,
    ) -> bool {
        if actor == target {
            return true;
        }
        let (Some(a), Some(t)) = (ctx.position_of(actor), ctx.position_of(target)) else {
            return false;
        };
        let (a, t) = (a + Vec3::Y * CHEST_M, t + Vec3::Y * CHEST_M);
        (a - t).length() <= MELEE_RANGE_M && ctx.is_unobstructed(a, t)
    }

    /// Ударить: оружие из активной руки или кулак; темп ограничен `attackRate`.
    pub(crate) fn attack(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        target: NetworkEntityId,
        part: Option<&str>,
    ) {
        if actor == target || ctx.position_of(target).is_none() {
            return;
        }
        // Состояние создаётся при первом обращении: стороны могли ещё ничего не делать.
        self.actor(target);
        self.actor(actor);
        let Some(a) = self.actors.get(&actor) else {
            return;
        };
        if self.clock < a.next_attack || !self.in_reach(ctx, actor, target) {
            return;
        }
        if self
            .actors
            .get(&target)
            .is_some_and(|t| t.health.state() == MobState::Dead)
        {
            return;
        }
        let weapon = a
            .hands
            .get(a.hands.active())
            .and_then(|i| self.melee.get(&i.prototype))
            .cloned()
            .unwrap_or_else(|| Melee {
                damage: DamageSpec::from([("Blunt".to_string(), FIST_DAMAGE)]),
                rate: DEFAULT_ATTACK_RATE,
            });
        let clock = self.clock;
        self.actor(actor).next_attack = clock + 1.0 / weapon.rate;
        let t = self.actor(target);
        let dealt = t
            .health
            .apply_to(Some(part.unwrap_or("Torso")), &weapon.damage);
        if let Some(body) = t.body.as_mut() {
            body.on_damage(&dealt);
        }
        self.after_health_change(ctx, target);
    }

    /// Закрыт ли предмет (бутылка, пока её не открыли).
    pub(crate) fn is_closed(&self, item: &HeldItem) -> bool {
        self.openable.get(&item.prototype).copied().unwrap_or(false)
            && !self.opened.contains(&item.id)
    }

    /// Объём раствора предмета: созданный при первом обращении или шаблон прототипа.
    pub(crate) fn solution_volume(&self, item: &HeldItem) -> f32 {
        self.solutions.get(&item.id).map_or_else(
            || {
                self.solution_templates
                    .get(&item.prototype)
                    .map_or(0.0, Solution::volume)
            },
            Solution::volume,
        )
    }

    /// Раствор предмета, создавая его из шаблона при первом обращении.
    pub(crate) fn solution_mut(&mut self, item: &HeldItem) -> Option<&mut Solution> {
        if !self.solutions.contains_key(&item.id) {
            let t = self.solution_templates.get(&item.prototype)?.clone();
            self.solutions.insert(item.id, t);
        }
        self.solutions.get_mut(&item.id)
    }

    /// Открыть предмет (`OpenableSystem.TryOpen`): `false`, если он не открывается или уже открыт.
    pub(crate) fn open(&mut self, item: &HeldItem) -> bool {
        if !self.is_closed(item) {
            return false;
        }
        self.opened.insert(item.id);
        true
    }

    /// Один укус: часть раствора переходит в желудок, пустая еда исчезает, а напиток остаётся.
    /// Съеденное самому себе продолжается само, пока есть что есть и место в желудке.
    fn bite(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        id: ObjectId,
        target: Option<NetworkEntityId>,
    ) {
        let who = target.unwrap_or(actor);
        let Some(item) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .filter(|i| i.id == id)
            .cloned()
        else {
            return;
        };
        let Some(spec) = self.edibles.get(&item.prototype).cloned() else {
            return;
        };
        let room = self
            .actors
            .get(&who)
            .and_then(|a| a.body.as_ref())
            .map_or(0.0, |b| b.stomach.available());
        let Some(sol) = self.solution_mut(&item) else {
            return;
        };
        let amount = spec.transfer.min(sol.volume()).min(room);
        let split = sol.split(amount);
        let empty = sol.is_empty();
        if let Some(body) = self.actor(who).body.as_mut() {
            let rest = body.swallow(&split);
            debug_assert!(rest.is_empty());
        }
        if empty && spec.destroy_on_empty {
            self.solutions.remove(&item.id);
            self.opened.remove(&item.id);
            let _ = self.actor(actor).hands.drop_hand(hand);
        } else if !empty {
            // Продолжаем есть, пока не кончится или не отменят.
            // Сам себя кормят подряд, пока не кончится; чужого — по одному укусу за раз.
            if target.is_none() {
                self.begin(
                    ctx,
                    actor,
                    Action::Eat {
                        hand,
                        item: id,
                        target: None,
                    },
                );
            }
        }
        self.publish(ctx, actor);
    }

    /// Лечение: применяет предмет к цели и расходует его.
    fn heal(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        target: NetworkEntityId,
    ) {
        let Some(h) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .and_then(|i| self.healing.get(&i.prototype))
            .cloned()
        else {
            return;
        };
        // Один пакет стопки расходуется за применение; последний уходит из руки.
        let id = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .map(|i| i.id);
        if let Some(id) = id {
            let left = self.charges.entry(id).or_insert(h.uses);
            *left = left.saturating_sub(1);
            if *left == 0 {
                self.charges.remove(&id);
                let _ = self.actor(actor).hands.drop_hand(hand);
            }
        }
        let t = self.actor(target);
        t.health.apply(&h.damage);
        if let Some(body) = t.body.as_mut() {
            body.modify_bleed(h.bleed_modifier);
        }
        self.after_health_change(ctx, target);
        self.publish(ctx, actor);
    }

    /// После любого изменения здоровья: рассылка и последствия. Тот, кто не может действовать,
    /// замораживается, бросает действие и роняет всё из рук; с двумя негодными ногами не ходят;
    /// негодная рука выпускает предмет и больше ничего не берёт.
    fn after_health_change(&mut self, ctx: &mut HookContext<'_>, who: NetworkEntityId) {
        let Some(a) = self.actors.get(&who) else {
            return;
        };
        let (now, before) = (a.health.state(), a.mob_state);
        let legs = a.health.part_disabled("LeftLeg") && a.health.part_disabled("RightLeg");
        let died = now == MobState::Dead && before != MobState::Dead;
        let resisting = a.body.as_ref().is_some_and(Body::resisting);
        let slipped = a.slip_until > self.clock;
        let freeze = (!now.can_act() || legs || resisting || slipped) && !a.ghost && !died;
        let feet = ctx.position_of(who);
        let mut to_floor: Vec<HeldItem> = Vec::new();
        let clock = self.clock;
        {
            let a = self.actor(who);
            a.mob_state = now;
            if died {
                // Смерть: тело остаётся лежать, игрок становится призраком.
                a.ghost = true;
                a.respawn_at = clock + RESPAWN_DELAY_S;
                a.death_pos = feet;
                ctx.set_ghost(who, true);
            }
            if freeze != a.frozen {
                a.frozen = freeze;
                ctx.freeze(who, freeze);
            }
            if !now.can_act() && before.can_act() {
                to_floor.extend(a.hands.take_all());
            }
            let names: Vec<String> = a.hands.iter().map(|(_, n, _)| n.to_string()).collect();
            for (i, name) in names.iter().enumerate() {
                let part = if name.starts_with('l') {
                    "LeftArm"
                } else {
                    "RightArm"
                };
                let off = a.health.part_disabled(part);
                if off != a.hands.is_disabled(i) {
                    a.hands.set_disabled(i, off);
                    if off && let Ok(item) = a.hands.drop_hand(i) {
                        to_floor.push(item);
                    }
                }
            }
        }
        if !now.can_act() {
            self.cancel(ctx, who);
        }
        if let Some(feet) = feet {
            for i in &to_floor {
                Self::place(ctx, i, feet);
            }
            if died {
                self.corpses += 1;
                let id = ObjectId::from_u128(0xC0F5_0000_0000 + u128::from(self.corpses));
                if let Some(floor) = floor_at(ctx, feet.y) {
                    let obj = AuthoredObject::new(
                        id,
                        AuthoredKind::Placement {
                            prototype: PrototypeId::new(CORPSE_PROTOTYPE)
                                .expect("встроенный идентификатор валиден"),
                        },
                        floor,
                        Vec3::new(feet.x - 0.4, feet.y, feet.z - 0.4),
                        Vec3::new(feet.x + 0.4, feet.y + 0.3, feet.z + 0.4),
                    );
                    ctx.request(WorldCommand::PlaceAuthored(obj));
                }
            }
        }
        self.publish(ctx, who);
    }

    /// Начать действие: сразу выполнить, если задержки нет, иначе запустить таймер. Пока идёт
    /// другое действие, новое не принимается.
    pub(crate) fn begin(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        action: Action,
    ) {
        let Some(a) = self.actors.get(&actor) else {
            return;
        };
        if a.doafter.is_some() || !self.can_start(ctx, actor, &action) {
            return;
        }
        let delay = self.delay_of(actor, &action);
        if delay <= 0.0 {
            self.run(ctx, actor, action);
            return;
        }
        let Some(start) = ctx.position_of(actor) else {
            return;
        };
        self.actor(actor).doafter = Some(DoAfter {
            action,
            delay,
            elapsed: 0.0,
            start,
        });
        self.publish_doafter(ctx, actor);
    }

    /// Условия начала и завершения действия (проверяются в обоих случаях).
    fn can_start(&self, ctx: &HookContext<'_>, actor: NetworkEntityId, action: &Action) -> bool {
        let Some(a) = self.actors.get(&actor) else {
            return false;
        };
        match action {
            Action::Equip { hand, slot } => a.hands.get(*hand).is_some_and(|item| {
                self.info(&item.prototype)
                    .is_some_and(|info| a.inventory.can_equip(slot, info).is_ok())
            }),
            Action::Unequip { slot, .. } => a.inventory.get(slot).is_some(),
            Action::Eat { hand, item, target } => {
                if target.is_some_and(|t| !self.in_reach(ctx, actor, t)) {
                    return false;
                }
                let held = a.hands.get(*hand).filter(|i| i.id == *item);
                let Some(held) = held else { return false };
                let Some(_) = self.edibles.get(&held.prototype) else {
                    return false;
                };
                let closed = self.is_closed(held);
                let has_food = self.solution_volume(held) > 1e-4;
                let room = a
                    .body
                    .as_ref()
                    .is_some_and(|b| b.stomach.available() > 1e-4);
                !closed && has_food && room
            }
            Action::Heal { hand, target } => {
                let healable = ctx.position_of(*target).is_some()
                    && self
                        .actors
                        .get(target)
                        .is_none_or(|t| t.health.state() != MobState::Dead);
                let item_ok = a
                    .hands
                    .get(*hand)
                    .is_some_and(|i| self.healing.contains_key(&i.prototype));
                healable && item_ok && self.in_reach(ctx, actor, *target)
            }
            Action::Pry { element, hand } => {
                ctx.door_open(*element) == Some(false)
                    && a.hands.get(*hand).is_some_and(|i| {
                        self.info(&i.prototype)
                            .is_some_and(|x| x.qualities.contains("Prying"))
                    })
            }
        }
    }

    fn delay_of(&self, actor: NetworkEntityId, action: &Action) -> f32 {
        let Some(a) = self.actors.get(&actor) else {
            return 0.0;
        };
        match action {
            Action::Equip { hand, .. } => a
                .hands
                .get(*hand)
                .and_then(|i| self.delays.get(&i.prototype))
                .map_or(0.0, |d| d.0),
            Action::Unequip { slot, .. } => a
                .inventory
                .get(slot)
                .and_then(|i| self.delays.get(&i.prototype))
                .map_or(0.0, |d| d.1),
            Action::Eat { hand, target, .. } => a
                .hands
                .get(*hand)
                .and_then(|i| self.edibles.get(&i.prototype))
                .map_or(0.0, |e| {
                    if target.is_some() {
                        e.force_delay
                    } else {
                        e.delay
                    }
                }),
            Action::Heal { hand, .. } => a
                .hands
                .get(*hand)
                .and_then(|i| self.healing.get(&i.prototype))
                .map_or(0.0, |h| h.delay),
            Action::Pry { .. } => PRY_TIME_S,
        }
    }

    fn run(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId, action: Action) {
        match action {
            Action::Equip { hand, slot } => self.equip(ctx, actor, hand, &slot),
            Action::Unequip { slot, hand } => self.unequip(ctx, actor, &slot, hand),
            Action::Pry { element, .. } => ctx.request_door(element, DoorAction::Open, true),
            Action::Heal { hand, target } => self.heal(ctx, actor, hand, target),
            Action::Eat { hand, item, target } => self.bite(ctx, actor, hand, item, target),
        }
    }

    /// Отменить идущее действие.
    pub(crate) fn cancel(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        if self
            .actors
            .get_mut(&actor)
            .and_then(|a| a.doafter.take())
            .is_some()
        {
            self.publish_doafter(ctx, actor);
        }
    }

    fn publish_doafter(&self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        let view = self
            .actors
            .get(&actor)
            .and_then(|a| a.doafter.as_ref())
            .map(|d| DoAfterView {
                kind: d.action.kind().into(),
                delay_s: d.delay,
            });
        ctx.set_component(actor, DOAFTER_COMPONENT, encode_doafter(view.as_ref()));
    }

    /// Возродиться: призрак по истечении ожидания получает новое тело в точке возрождения. Одежда
    /// старого тела остаётся лежать у места смерти.
    pub(crate) fn respawn(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        let died_as_ghost = self
            .actors
            .get(&actor)
            .is_some_and(|a| a.ghost && self.clock >= a.respawn_at);
        if died_as_ghost {
            self.release_job(actor);
        }
        let (Some(spawn), Some(a)) = (self.spawn, self.actors.get(&actor)) else {
            return;
        };
        if !a.ghost || self.clock < a.respawn_at {
            return;
        }
        let at = a.death_pos.unwrap_or(spawn);
        let a = self.actor(actor);
        let worn = a.inventory.take_all();
        a.health = Health::default();
        a.body = Some(Body::new(actor.0.wrapping_mul(97) ^ 0x5bd1_e995));
        a.last_vitals = None;
        a.mob_state = MobState::Alive;
        a.ghost = false;
        a.frozen = false;
        a.doafter = None;
        a.next_attack = 0.0;
        a.death_pos = None;
        for h in 0..a.hands.len() {
            a.hands.set_disabled(h, false);
        }
        for i in &worn {
            Self::place(ctx, i, at);
        }
        ctx.set_ghost(actor, false);
        ctx.freeze(actor, false);
        ctx.teleport(actor, spawn);
        self.publish_doafter(ctx, actor);
        self.publish(ctx, actor);
    }

    /// Атмосфера и тело: обновить геометрию, продвинуть газ, дать каждому персонажу подышать,
    /// переварить съеденное и проверить давление. Кровотечение идёт тут же. Здоровье рассылается не
    /// чаще двух раз в секунду, но конец процесса — сразу, чтобы клиент не застрял на старом числе.
    fn tick_health(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        self.tick_atmosphere(ctx, dt);
        let ids: Vec<NetworkEntityId> = ctx.players().collect();
        for id in ids {
            let Some(pos) = ctx.position_of(id) else {
                continue;
            };
            let floor = floor_at(ctx, pos.y).map_or(0, |f| f.0);
            let cell = atmos::floor_cell(floor, pos.x.floor() as i32, pos.z.floor() as i32);
            let clock = self.clock;
            self.actor(id);
            let Some(mut body) = self.actors.get_mut(&id).and_then(|a| a.body.take()) else {
                continue;
            };
            let (mob, barotrauma) = {
                let a = &self.actors[&id];
                (
                    a.health.state(),
                    a.health.damage_of("Blunt") + a.health.damage_of("Heat"),
                )
            };
            let a_ghost = self.actors[&id].ghost;
            let protection: f32 = self.actors[&id]
                .inventory
                .worn()
                .filter_map(|i| self.heat_protection.get(&i.prototype))
                .product();
            body.set_heat_protection(protection);
            let outcome = {
                let mut env = ActorEnv {
                    grid: Some(&mut self.atmos),
                    cell,
                };
                body.tick(dt, &mut env, mob, barotrauma)
            };
            let vitals = body.vitals();
            let speed = if a_ghost { 1.0 } else { vitals.speed };
            if (speed - self.speed_sent.get(&id).copied().unwrap_or(1.0)).abs() > 1e-4 {
                self.speed_sent.insert(id, speed);
                ctx.set_speed(id, speed);
            }
            if outcome.blood_spilled > 0.0 {
                let mut blood = Solution::new(1.0e9);
                blood.add("Blood", outcome.blood_spilled);
                self.puddles.spill(self.atmos.layout(), cell, &blood);
            }
            let a = self.actor(id);
            let dealt = a.health.apply(&outcome.damage);
            body.on_damage(&dealt);
            let vitals = body.vitals();
            let vitals_changed = a
                .last_vitals
                .as_ref()
                .is_none_or(|v| vitals_differ(v, &vitals));
            a.body = Some(body);
            let changed = outcome.changed;
            a.health_dirty |= changed;
            let due = clock >= a.next_health_publish;
            if (a.health_dirty && (due || !changed))
                || a.health.state() != a.mob_state
                || vitals_changed
            {
                a.health_dirty = false;
                a.next_health_publish = clock + 0.5;
                a.last_vitals = Some(vitals);
                self.after_health_change(ctx, id);
            }
        }
    }

    /// Геометрия и ход газа: при любом изменении стен или дверей сетка перестраивается, воздух
    /// уцелевших клеток сохраняется.
    fn tick_atmosphere(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        let layout = crate::world_atmos::layout(ctx.world, &|id| ctx.door_open(id) == Some(true));
        if self.atmos.layout() != &layout {
            self.atmos.set_layout(layout);
        }
        self.tick_igniters(ctx);
        self.tick_puddles(ctx, dt);
        self.sync_pipes(ctx);
        self.sync_power(ctx);
        self.place_joiners(ctx);
        self.tick_disposal(ctx, dt);
        self.tick_conveyors(ctx, dt);
        self.tick_vending(ctx);
        self.publish_jobs(ctx);
        self.resolve_spawners(ctx);
        self.tick_alarms(ctx, dt);
        self.tick_firelocks(ctx, dt);
        self.tick_power(dt);
        self.publish_lights(ctx, dt);
        self.atmos.advance(dt);
        self.tick_devices(ctx, dt);
        self.publish_fires(ctx);
    }

    /// Зажжённый предмет в руке поджигает воздух клетки владельца (`IgnitionSourceSystem.Update`);
    /// выпавший из рук гаснет.
    fn tick_igniters(&mut self, ctx: &mut HookContext<'_>) {
        if self.lit.is_empty() {
            return;
        }
        let mut held = BTreeSet::new();
        let mut burning: Vec<(NetworkEntityId, f32)> = Vec::new();
        for (id, a) in &self.actors {
            for item in a.hands.held() {
                if self.lit.contains(&item.id) {
                    held.insert(item.id);
                    if let Some(&t) = self.igniters.get(&item.prototype) {
                        burning.push((*id, t));
                    }
                }
            }
        }
        self.lit.retain(|i| held.contains(i));
        for (id, temperature) in burning {
            if let Some(cell) = self.cell_of(ctx, id) {
                self.atmos.ignite(cell, temperature, IGNITION_SOURCE_VOLUME);
            }
        }
    }

    /// Трубы-зоны и устройства мира → узлы и устройства атмосферы. Состояние устройств (клапаны,
    /// баллоны) сохраняется, пока объект стоит на месте.
    fn sync_pipes(&mut self, ctx: &mut HookContext<'_>) {
        use crate::piping::{
            DeviceSpec, PIPE_KEY, PIPE_VOLUME_KEY, cell_at, device_of, node_key, parse_dirs,
        };
        let mut layout = crate::piping::PipeLayout::default();
        let mut sig: Vec<(ObjectId, atmos::Cell, u8)> = Vec::new();
        let mut parts: Vec<(ObjectId, atmos::Cell, crate::piping::PipePart)> = Vec::new();
        for o in ctx.world.authored() {
            let centre = (o.min() + o.max()) * 0.5;
            let cell = cell_at(o.floor.0, centre);
            match &o.kind {
                AuthoredKind::Zone => {
                    let Some(dirs) = o.metadata.get(PIPE_KEY).map(|s| parse_dirs(s)) else {
                        continue;
                    };
                    let volume = o
                        .metadata
                        .get(PIPE_VOLUME_KEY)
                        .and_then(|v| v.parse::<f32>().ok())
                        .unwrap_or(atmos::pipes::DEFAULT_PIPE_VOLUME);
                    parts.push((
                        o.id,
                        cell,
                        crate::piping::PipePart {
                            nodes: vec![(dirs, volume)],
                            device: None,
                        },
                    ));
                    sig.push((o.id, cell, dirs));
                }
                AuthoredKind::Placement { prototype } => {
                    if let Some(p) = self.pipe_parts.get(prototype) {
                        let rot = o
                            .metadata
                            .get("rot")
                            .and_then(|v| v.parse::<u8>().ok())
                            .unwrap_or(0);
                        let mut part = p.clone();
                        for n in &mut part.nodes {
                            n.0 = crate::piping::rotate_dirs(n.0, rot);
                        }
                        parts.push((o.id, cell, part));
                        sig.push((o.id, cell, rot));
                    }
                }
                _ => {}
            }
        }
        sig.sort();
        if sig == self.pipe_sig {
            return;
        }
        self.pipe_sig = sig;
        for (id, cell, part) in &parts {
            let key = self.pipe_keys.key_of(*id);
            for (i, (dirs, volume)) in part.nodes.iter().enumerate() {
                layout.nodes.push(atmos::NodeSpec {
                    key: node_key(key, i),
                    cell: *cell,
                    dirs: *dirs,
                    volume: *volume,
                });
                if matches!(part.device, Some(DeviceSpec::Port)) && i == 0 {
                    layout.ports.insert(*cell, node_key(key, 0));
                }
            }
        }
        for (id, cell, part) in &parts {
            let key = self.pipe_keys.key_of(*id);
            if let Some(spec) = &part.device
                && let Some(device) = device_of(spec, key, layout.ports.get(cell).copied())
            {
                layout.devices.push((key, *cell, device, *id));
            }
        }
        self.pipe_ports = layout.ports.clone();
        self.atmos.set_pipe_nodes(&layout.nodes);
        let wanted: BTreeSet<u64> = layout.devices.iter().map(|d| d.0).collect();
        let present: Vec<u64> = self.atmos.pipes().device_keys().collect();
        for k in present {
            if !wanted.contains(&k) {
                self.atmos.pipes_mut().remove_device(k);
            }
        }
        for (key, cell, device, _) in layout.devices {
            if self.atmos.pipes().device(key).is_none() {
                self.atmos.pipes_mut().add_device(key, cell, device);
            }
        }
    }

    /// Лужи: испарение, растекание, поскальзывание, рассылка клиентам.
    fn tick_puddles(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        self.puddles.tick(dt, self.atmos.layout());
        // Поскальзывание: вход на скользкую клетку на достаточной скорости.
        let ids: Vec<NetworkEntityId> = ctx.players().collect();
        let mut slips: Vec<NetworkEntityId> = Vec::new();
        for id in ids {
            let (Some(pos), Some(cell)) = (ctx.position_of(id), self.cell_of(ctx, id)) else {
                continue;
            };
            let clock = self.clock;
            let a = self.actor(id);
            let prev = a.last_step.replace((cell, pos));
            let Some((old_cell, old_pos)) = prev else {
                continue;
            };
            if old_cell == cell || a.ghost || !a.health.state().can_act() {
                continue;
            }
            let speed =
                ((pos.x - old_pos.x).powi(2) + (pos.z - old_pos.z).powi(2)).sqrt() / dt.max(1e-3);
            let Some(slip) = self.puddles.slip(cell) else {
                continue;
            };
            let down = self.actors[&id].slip_until > clock;
            if speed >= slip.required_speed && (!down || slip.super_slippery) {
                slips.push(id);
            }
        }
        for id in slips {
            self.slip(ctx, id);
        }
        // Окончание падения.
        let clock = self.clock;
        let risen: Vec<NetworkEntityId> = self
            .actors
            .iter()
            .filter(|(_, a)| a.slip_until > 0.0 && a.slip_until <= clock)
            .map(|(id, _)| *id)
            .collect();
        for id in risen {
            self.actor(id).slip_until = 0.0;
            self.after_health_change(ctx, id);
        }
        self.puddle_timer += dt;
        if self.puddle_timer >= 0.5 {
            self.puddle_timer = 0.0;
            self.publish_puddles(ctx);
        }
    }

    /// `SlipperySystem.TrySlip`: человек роняет всё из рук и падает (оглушение 0,5 с и сбивание 1,5 с).
    fn slip(&mut self, ctx: &mut HookContext<'_>, id: NetworkEntityId) {
        let clock = self.clock;
        let feet = ctx.position_of(id);
        let dropped: Vec<HeldItem> = {
            let a = self.actor(id);
            let knocked = a.slip_until > clock;
            a.slip_until = clock + crate::puddles::SLIP_STUN_S + crate::puddles::SLIP_KNOCKDOWN_S;
            if knocked {
                Vec::new()
            } else {
                a.hands.take_all()
            }
        };
        if let Some(at) = feet {
            for item in &dropped {
                State::place(ctx, item, at);
            }
        }
        self.after_health_change(ctx, id);
    }

    fn publish_puddles(&mut self, ctx: &mut HookContext<'_>) {
        let mut views: Vec<crate::puddles::PuddleView> = self
            .puddles
            .cells()
            .map(|(c, s)| {
                let floor = (c.1 + atmos::FLOOR_STRIDE / 2).div_euclid(atmos::FLOOR_STRIDE);
                let z = c.1 - floor * atmos::FLOOR_STRIDE;
                let y = ctx
                    .world
                    .floors()
                    .find(|f| f.id.0 == floor)
                    .map_or(0.0, |f| f.base_y);
                crate::puddles::PuddleView {
                    x: c.0 as f32 + 0.5,
                    y,
                    z: z as f32 + 0.5,
                    fill: (s.volume() / crate::puddles::OVERFLOW_VOLUME * 10.0).round() / 10.0,
                    color: self.puddles.color(*c),
                    slippery: self.puddles.slip(*c).is_some(),
                }
            })
            .collect();
        views.truncate(crate::puddles::MAX_PUDDLES);
        let ids: Vec<NetworkEntityId> = ctx.players().collect();
        let fresh = ids.iter().any(|i| !self.puddles_to.contains(i));
        if views == self.puddles_sent && !fresh {
            return;
        }
        let bytes = crate::puddles::encode_puddles(&views);
        self.puddles_sent = views;
        self.puddles_to = ids.iter().copied().collect();
        for id in ids {
            ctx.set_component(id, crate::puddles::PUDDLES_COMPONENT, bytes.clone());
        }
    }

    /// Предмет из руки по точке на полу: швабра по луже или по ведру (`SharedAbsorbentSystem.Mop`).
    pub(crate) fn use_on_ground(
        &mut self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        point: Vec3,
    ) -> bool {
        use crate::mopping::{MOP_DELAY_S, MopResult, mop_container, mop_puddle};
        let Some(item) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return false;
        };
        let Some(&pickup) = self.absorbents.get(&item.prototype) else {
            return false;
        };
        let Some(feet) = ctx.position_of(actor) else {
            return false;
        };
        // Дальность до точки на полу меряется от ступней (цель лежит на полу).
        if (point - feet).length() > self.range_m {
            return false;
        }
        if self
            .mop_ready
            .get(&item.id)
            .is_some_and(|t| *t > self.clock)
        {
            return true;
        }
        let Some(floor) = floor_at(ctx, point.y.max(feet.y)) else {
            return false;
        };
        let cell = crate::piping::cell_at(floor.0, point);
        // Сначала лужа под курсором (`TryPuddleInteract`), иначе ёмкость на полу.
        let Some(mut absorber) = self.solution_mut(&item).cloned() else {
            return false;
        };
        let mut result = MopResult::Nothing;
        let mut handled = false;
        // Ёмкость на полу под курсором важнее лужи: по ней целятся осознанно.
        let target = ctx.world.authored().find_map(|o| {
            let AuthoredKind::Placement { prototype } = &o.kind else {
                return None;
            };
            let c = (o.min() + o.max()) * 0.5;
            let near = (c.x - point.x).hypot(c.z - point.z) < 0.6 && (c.y - point.y).abs() < 1.0;
            (near && self.refillables.contains(prototype)).then(|| HeldItem {
                id: o.id,
                prototype: prototype.clone(),
            })
        });
        if let Some(t) = target
            && let Some(mut container) = self.solution_mut(&t).cloned()
        {
            result = mop_container(&mut absorber, &mut container, pickup);
            self.solutions.insert(t.id, container);
            handled = true;
        } else if let Some(puddle) = self.puddles.solution_mut(cell)
            && puddle.volume() > 0.0
        {
            result = mop_puddle(&mut absorber, puddle, pickup);
            handled = true;
        }
        if !handled {
            return false;
        }
        self.solutions.insert(item.id, absorber);
        if result == MopResult::Done {
            self.mop_ready.insert(item.id, self.clock + MOP_DELAY_S);
        }
        true
    }

    /// Вылить содержимое предмета из руки под ноги (`SpillableComponent`, действие «Вылить»).
    pub(crate) fn spill_item(
        &mut self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
    ) -> bool {
        let Some(item) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return false;
        };
        if self.is_closed(&item) || self.solution_volume(&item) <= 0.0 {
            return false;
        }
        let Some(cell) = self.cell_of(ctx, actor) else {
            return false;
        };
        let Some(sol) = self.solution_mut(&item) else {
            return false;
        };
        let all = sol.volume();
        let spilled = sol.split(all);
        self.puddles.spill(self.atmos.layout(), cell, &spilled)
    }

    /// Электросети мира: кабели-зоны и устройства → сети, источники, потребители и аккумуляторы.
    /// Заряд аккумуляторов переживает перестройку.
    fn sync_power(&mut self, ctx: &mut HookContext<'_>) {
        use crate::electric::CABLE_KEY;
        use power::CableGroup;
        let mut cables: Vec<(CableGroup, atmos::Cell)> = Vec::new();
        let mut parts: Vec<(ObjectId, atmos::Cell, crate::electric::PowerPart)> = Vec::new();
        let mut sig: Vec<(ObjectId, atmos::Cell, u8)> = Vec::new();
        for o in ctx.world.authored() {
            let cell = crate::piping::cell_at(o.floor.0, (o.min() + o.max()) * 0.5);
            match &o.kind {
                AuthoredKind::Zone => {
                    if let Some(g) = o.metadata.get(CABLE_KEY).and_then(|v| CableGroup::parse(v)) {
                        cables.push((g, cell));
                        sig.push((o.id, cell, g as u8 + 1));
                    }
                }
                AuthoredKind::Placement { prototype } => {
                    if let Some(p) = self.power_parts.get(prototype) {
                        parts.push((o.id, cell, p.clone()));
                        sig.push((o.id, cell, 0));
                    }
                }
                _ => {}
            }
        }
        sig.sort();
        if sig == self.power_sig {
            return;
        }
        // Сохранить заряды перед перестройкой.
        for (id, idx) in &self.power_batteries {
            if let Some(b) = self.power.batteries.get(*idx) {
                self.power_charge.insert(*id, b.current_storage);
            }
        }
        self.power_lights.clear();
        self.power_kinds.clear();
        for o in ctx.world.authored() {
            let AuthoredKind::Placement { prototype } = &o.kind else {
                continue;
            };
            let Some(spec) = self
                .power_parts
                .get(prototype)
                .and_then(|p| p.light.clone())
            else {
                continue;
            };
            let kind = [
                f32::from(spec.color[0]),
                f32::from(spec.color[1]),
                f32::from(spec.color[2]),
                spec.energy,
                spec.radius,
            ];
            let k = self
                .power_kinds
                .iter()
                .position(|x| *x == kind)
                .unwrap_or_else(|| {
                    self.power_kinds.push(kind);
                    self.power_kinds.len() - 1
                });
            let c = (o.min() + o.max()) * 0.5;
            let base = ctx
                .world
                .floors()
                .find(|f| f.id.0 == o.floor.0)
                .map_or(0.0, |f| f.base_y);
            self.power_lights
                .push((o.id, Vec3::new(c.x, base + 2.4, c.z), k as u8));
        }
        self.power_sig = sig;
        self.power_required = !cables.is_empty()
            || parts
                .iter()
                .any(|(_, _, p)| p.supplier.is_some() || p.battery.is_some());
        self.power_gated = parts
            .iter()
            .filter(|(_, _, p)| p.receiver_load.is_some())
            .map(|(id, _, _)| *id)
            .collect();
        let built = crate::electric::build(&cables, &parts, &self.power_charge);
        self.power_batteries = built.batteries;
        self.power_receivers = built.receivers;
        self.power = built.state;
    }

    /// Точка клетки сетки: центр клетки на полу своего этажа.
    fn cell_point(ctx: &HookContext<'_>, cell: atmos::Cell) -> Vec3 {
        let stride = atmos::grid::FLOOR_STRIDE;
        let floor = (cell.1 + stride / 2).div_euclid(stride);
        let z = cell.1 - floor * stride;
        let base = ctx
            .world
            .floors()
            .find(|f| f.id.0 == floor)
            .map_or(0.0, |f| f.base_y);
        Vec3::new(cell.0 as f32 + 0.5, base + 0.2, z as f32 + 0.5)
    }

    /// Сеть труб и ёмкости из карты: зоны `disposal_tube` и объекты с `DisposalUnit`.
    fn sync_disposal(&mut self, ctx: &mut HookContext<'_>) {
        use crate::disposal::{ENTRY_KEY, TUBE_KEY, Tube, parse_exits};
        let mut tubes: Vec<(atmos::Cell, String, bool)> = Vec::new();
        let mut units: Vec<(ObjectId, atmos::Cell, Vec3)> = Vec::new();
        for o in ctx.world.authored() {
            let centre = (o.min() + o.max()) * 0.5;
            let cell = crate::piping::cell_at(o.floor.0, centre);
            match &o.kind {
                AuthoredKind::Zone => {
                    if let Some(exits) = o.metadata.get(TUBE_KEY)
                        && !self.disposal_torn.contains(&cell)
                    {
                        tubes.push((cell, exits.clone(), o.metadata.contains_key(ENTRY_KEY)));
                    }
                }
                AuthoredKind::Placement { prototype }
                    if self.disposal_units.contains(prototype) =>
                {
                    units.push((o.id, cell, Vec3::new(centre.x, o.min().y, centre.z)));
                }
                _ => {}
            }
        }
        let mut sig: Vec<(atmos::Cell, String)> =
            tubes.iter().map(|(c, e, _)| (*c, e.clone())).collect();
        sig.sort();
        if sig != self.disposal_sig {
            self.disposal_sig = sig;
            self.disposal_entries = tubes.iter().filter(|t| t.2).map(|t| t.0).collect();
            self.disposal_net = crate::disposal::Net::new(tubes.iter().map(|(c, e, entry)| {
                (
                    *c,
                    Tube {
                        exits: parse_exits(e),
                        entry: *entry,
                    },
                )
            }));
        }
        let ids: BTreeSet<ObjectId> = units.iter().map(|u| u.0).collect();
        self.disposal_state.retain(|id, _| ids.contains(id));
        for (id, cell, at) in units {
            self.disposal_state
                .entry(id)
                .and_modify(|s| {
                    s.cell = cell;
                    s.at = at;
                })
                .or_insert_with(|| DisposalSlot {
                    unit: crate::disposal::Unit::default(),
                    cell,
                    at,
                });
        }
    }

    /// Диспозал каждый кадр: ёмкости набирают давление и сбрасывают, холдеры едут по трубам.
    fn tick_disposal(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        self.disposal_timer += dt;
        if self.disposal_timer >= 1.0
            || self.disposal_sig.is_empty() && self.disposal_state.is_empty()
        {
            if self.disposal_timer >= 1.0 {
                self.disposal_timer = 0.0;
            }
            self.sync_disposal(ctx);
        }
        if self.disposal_state.is_empty() && self.disposal_holders.is_empty() {
            return;
        }
        let now = self.clock;
        let powered = self.powered_map();
        let mut due = Vec::new();
        for (id, slot) in &mut self.disposal_state {
            let on = !self.power_required
                || !self.power_gated.contains(id)
                || powered.get(id).copied().unwrap_or(false);
            if on != slot.unit.powered {
                slot.unit.powered = on;
                slot.unit.recalculate(now, true);
            }
            if slot.unit.update(now) {
                due.push(*id);
            }
        }
        for id in due {
            self.flush_unit(ctx, id);
        }
        let mut holders = std::mem::take(&mut self.disposal_holders);
        let mut net = std::mem::take(&mut self.disposal_net);
        let mut kept = Vec::new();
        for mut h in holders.drain(..) {
            match h.advance(&mut net, dt, &mut self.disposal_rng) {
                crate::disposal::Outcome::Moving => kept.push(h),
                crate::disposal::Outcome::Exited { at, dir } => {
                    if !net.tube(at).is_some() {
                        self.disposal_torn.insert(at);
                    }
                    self.exit_holder(ctx, h, at, dir);
                }
            }
        }
        self.disposal_net = net;
        self.disposal_holders = kept;
    }

    /// `TryFlush`: найти вход в трубы под ёмкостью, отправить содержимое.
    fn flush_unit(&mut self, ctx: &mut HookContext<'_>, id: ObjectId) {
        let now = self.clock;
        let Some(slot) = self.disposal_state.get_mut(&id) else {
            return;
        };
        if !slot.unit.can_flush(now) {
            return;
        }
        if !self.disposal_entries.contains(&slot.cell) {
            slot.unit.set_engage(false, now);
            return;
        }
        let cell = slot.cell;
        let items = slot.unit.eject(now);
        slot.unit.flushed(now);
        if items.is_empty() {
            return;
        }
        let mut holder = crate::disposal::Holder::new(items, cell);
        let mut net = std::mem::take(&mut self.disposal_net);
        if holder.enter(&mut net, cell, &mut self.disposal_rng) {
            self.disposal_holders.push(holder);
            self.disposal_net = net;
        } else {
            self.disposal_net = net;
            let dir = holder.dir;
            self.exit_holder(ctx, holder, cell, dir);
        }
    }

    /// `Exit` холдера: в ёмкость на той же клетке (и сразу из неё), иначе выброс вперёд на
    /// `exitDistanceMultiplier` клеток, но не сквозь стену.
    fn exit_holder(
        &mut self,
        ctx: &mut HookContext<'_>,
        holder: crate::disposal::Holder,
        at: atmos::Cell,
        dir: Option<crate::disposal::Dir>,
    ) {
        let start = Self::cell_point(ctx, at);
        let landing = if let Some(slot) = self.disposal_state.values().find(|s| s.cell == at) {
            Vec3::new(slot.at.x, start.y, slot.at.z)
        } else if let Some(d) = dir {
            let (dx, dz) = d.delta();
            let target = [
                start.x + dx as f32 * crate::disposal::EXIT_DISTANCE,
                start.y,
                start.z + dz as f32 * crate::disposal::EXIT_DISTANCE,
            ];
            self.drop_point(ctx, start, Some(target))
        } else {
            start
        };
        for item in &holder.items {
            Self::place(ctx, item, landing);
        }
    }

    /// Создать предмет по имени прототипа SS14; контейнеры наполняются (`StorageFill`).
    pub(crate) fn create_item(&mut self, name: &str) -> Option<HeldItem> {
        let proto = self.proto_by_name.get(name)?.clone();
        self.item_serial += 1;
        let item = HeldItem {
            id: ObjectId::from_u128(0xA11C_E000_0000 + self.item_serial),
            prototype: proto.clone(),
        };
        if let Some(entries) = self.fills.get(&proto).cloned() {
            let spec = self.info(&proto).and_then(|i| i.storage.clone());
            if let Some(spec) = spec {
                for n in crate::spawners::fill_spawns(&entries, &mut self.spawn_rng) {
                    if let Some(inner) = self.create_item(&n)
                        && let Some(info) = self.info(&inner.prototype).cloned()
                    {
                        let _ = self
                            .storages
                            .entry(item.id)
                            .or_default()
                            .insert(&spec, inner, &info);
                    }
                }
            }
        }
        Some(item)
    }

    /// Выбор должности: места есть, у игрока её ещё нет — создать и надеть снаряжение.
    pub(crate) fn pick_job(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        job: &str,
    ) {
        if self.player_job.contains_key(&actor)
            || !self
                .actors
                .get(&actor)
                .is_some_and(|a| !a.ghost && a.health.state().can_act())
        {
            return;
        }
        let Some(spec) = self.jobs.iter().find(|j| j.id == job).cloned() else {
            return;
        };
        let taken = self.job_taken.get(job).copied().unwrap_or(0);
        if spec.max >= 0 && taken >= spec.max as u32 {
            return;
        }
        *self.job_taken.entry(job.to_string()).or_default() += 1;
        self.player_job.insert(actor, job.to_string());
        // Слоты по порядку шаблона: сначала то, от чего зависят другие (комбинезон, верхняя одежда).
        for slot in crate::inventory::human_template() {
            let Some(name) = spec.equipment.get(&slot.name) else {
                continue;
            };
            if let Some(item) = self.create_item(name) {
                self.actor(actor).inventory.equip_forced(&slot.name, item);
            }
        }
        for name in &spec.inhand {
            if let Some(item) = self.create_item(name)
                && let Err(_) = self.actor(actor).hands.pickup_any(item.clone())
                && let Some(feet) = ctx.position_of(actor)
            {
                Self::place(ctx, &item, feet);
            }
        }
        for (slot, names) in &spec.storage {
            let Some(bag) = self
                .actors
                .get(&actor)
                .and_then(|a| a.inventory.get(slot))
                .cloned()
            else {
                continue;
            };
            let Some(bag_spec) = self.info(&bag.prototype).and_then(|i| i.storage.clone()) else {
                continue;
            };
            for name in names {
                if let Some(item) = self.create_item(name)
                    && let Some(info) = self.info(&item.prototype).cloned()
                {
                    let _ = self
                        .storages
                        .entry(bag.id)
                        .or_default()
                        .insert(&bag_spec, item, &info);
                }
            }
        }
        self.publish(ctx, actor);
    }

    /// Освободить место должности (игрок ушёл или возродился).
    pub(crate) fn release_job(&mut self, actor: NetworkEntityId) {
        if let Some(j) = self.player_job.remove(&actor)
            && let Some(n) = self.job_taken.get_mut(&j)
        {
            *n = n.saturating_sub(1);
        }
    }

    /// Список должностей каждому игроку (по изменению).
    fn publish_jobs(&mut self, ctx: &mut HookContext<'_>) {
        if self.jobs.is_empty() {
            return;
        }
        let list: Vec<crate::jobs::JobEntry> = self
            .jobs
            .iter()
            .map(|j| crate::jobs::JobEntry {
                id: j.id.clone(),
                title: j.title.clone(),
                taken: self.job_taken.get(&j.id).copied().unwrap_or(0),
                max: j.max,
            })
            .collect();
        for p in ctx.players().collect::<Vec<_>>() {
            let view = crate::jobs::JobsView {
                jobs: list.clone(),
                current: self.player_job.get(&p).cloned(),
            };
            let bytes = crate::jobs::encode_jobs(&view);
            if self.jobs_sent.get(&p) != Some(&bytes) {
                ctx.set_component(p, crate::jobs::JOBS_COMPONENT, bytes.clone());
                self.jobs_sent.insert(p, bytes);
            }
        }
    }

    /// Старт раунда: каждый спавнер карты заменяется предметами (`MapInit` спавнеров SS14).
    fn resolve_spawners(&mut self, ctx: &mut HookContext<'_>) {
        // Спавнер срабатывает, как только появился (`MapInit`): проверка раз в полсекунды.
        if self.spawner_specs.is_empty() {
            return;
        }
        self.spawn_timer += self.last_dt;
        if self.spawners_done && self.spawn_timer < 0.5 {
            return;
        }
        self.spawn_timer = 0.0;
        self.spawners_done = true;
        let found: Vec<(ObjectId, PrototypeId, Vec3, f32)> = ctx
            .world
            .authored()
            .filter_map(|o| match &o.kind {
                AuthoredKind::Placement { prototype }
                    if self.spawner_specs.contains_key(prototype) =>
                {
                    let c = (o.min() + o.max()) * 0.5;
                    Some((o.id, prototype.clone(), Vec3::new(c.x, o.min().y, c.z), 0.0))
                }
                _ => None,
            })
            .collect();
        for (id, proto, at, _) in found {
            ctx.request(WorldCommand::RemoveAuthored { id });
            let Some(spec) = self.spawner_specs.get(&proto) else {
                continue;
            };
            let offset = spec.offset();
            for name in spec.roll(&self.spawner_tables, &mut self.spawn_rng) {
                // Прототипа нет в Build (мобы и т. п.) — пропускаем.
                let Some(item) = self.proto_by_name.get(&name) else {
                    continue;
                };
                self.spawn_serial += 1;
                let dx = (self.spawn_rng.next_f32() * 2.0 - 1.0) * offset;
                let dz = (self.spawn_rng.next_f32() * 2.0 - 1.0) * offset;
                let held = HeldItem {
                    id: ObjectId::from_u128(0xE5E5_0000_0000 + self.spawn_serial),
                    prototype: item.clone(),
                };
                Self::place(ctx, &held, at + Vec3::new(dx, 0.0, dz));
            }
        }
    }

    /// Автоматы из карты получают начальный запас при первом появлении (`MapInit`).
    fn sync_vending(&mut self, ctx: &HookContext<'_>) {
        let mut seen = BTreeSet::new();
        for o in ctx.world.authored() {
            let AuthoredKind::Placement { prototype } = &o.kind else {
                continue;
            };
            let Some(spec) = self.vending_specs.get(prototype) else {
                continue;
            };
            seen.insert(o.id);
            if !self.vending.contains_key(&o.id) {
                let machine = crate::vending::Machine::new(
                    spec.stock.iter().cloned(),
                    spec.quality,
                    spec.eject_delay,
                    &mut self.vending_rng,
                );
                self.vending.insert(o.id, machine);
            }
        }
        self.vending.retain(|id, _| seen.contains(id));
    }

    /// Окно автомата открывается пустой рукой, если есть ток (`ActivatableUIRequiresPower`).
    pub(crate) fn open_vending(&mut self, user: NetworkEntityId, id: ObjectId) -> bool {
        if !self.vending.contains_key(&id) {
            return false;
        }
        if self.power_required && !self.powered_map().get(&id).copied().unwrap_or(false) {
            return true;
        }
        self.vending_open.insert(user, id);
        true
    }

    /// Заказ товара из открытого автомата.
    pub(crate) fn vend(&mut self, ctx: &HookContext<'_>, user: NetworkEntityId, item: &str) {
        let Some(id) = self.vending_open.get(&user).copied() else {
            return;
        };
        let now = self.clock;
        let powered = !self.power_required || self.powered_map().get(&id).copied().unwrap_or(false);
        let _ = ctx;
        if let Some(machine) = self.vending.get_mut(&id) {
            let _ = machine.order(now, item, powered);
        }
    }

    /// Автоматы каждый кадр: выдача готовых товаров, окна у игроков.
    fn tick_vending(&mut self, ctx: &mut HookContext<'_>) {
        if self.vending_specs.is_empty() {
            return;
        }
        self.sync_vending(ctx);
        let now = self.clock;
        let ids: Vec<ObjectId> = self.vending.keys().copied().collect();
        for id in ids {
            let Some(done) = self.vending.get_mut(&id).and_then(|m| m.update(now)) else {
                continue;
            };
            let Some(o) = ctx.world.authored_object(id) else {
                continue;
            };
            // Товар ложится у автомата, на полу перед ним.
            let c = (o.min() + o.max()) * 0.5;
            self.vending_serial += 1;
            let item = HeldItem {
                id: ObjectId::from_u128(0xD3AD_0000_0000 + self.vending_serial),
                prototype: done,
            };
            Self::place(ctx, &item, Vec3::new(c.x, o.min().y, c.z));
        }
        // Окна: закрыть, если игрок ушёл от автомата; послать изменения.
        let players: Vec<NetworkEntityId> = ctx.players().collect();
        let mut views: Vec<(NetworkEntityId, Vec<u8>)> = Vec::new();
        for p in players {
            let open = self.vending_open.get(&p).copied();
            let view = open.and_then(|id| {
                let o = ctx.world.authored_object(id)?;
                let feet = ctx.position_of(p)?;
                let c = (o.min() + o.max()) * 0.5;
                if (c - feet).length() > self.range_m + 1.0 {
                    return None;
                }
                let machine = self.vending.get(&id)?;
                let AuthoredKind::Placement { prototype } = &o.kind else {
                    return None;
                };
                Some(crate::vending::VendingView {
                    machine: id,
                    name: self
                        .vending_specs
                        .get(prototype)
                        .map(|s| s.title.clone())
                        .unwrap_or_default(),
                    items: machine
                        .entries
                        .iter()
                        .map(|e| crate::vending::VendingItem {
                            name: e.name.clone(),
                            proto: e.proto.to_string(),
                            amount: e.amount,
                        })
                        .collect(),
                    ejecting: machine.ejecting(),
                    denying: machine.denying(),
                })
            });
            if view.is_none() {
                self.vending_open.remove(&p);
            }
            let bytes = crate::vending::encode_vending(view.as_ref());
            if self.vending_sent.get(&p) != Some(&bytes) {
                views.push((p, bytes));
            }
        }
        for (p, bytes) in views {
            ctx.set_component(p, crate::vending::VENDING_COMPONENT, bytes.clone());
            self.vending_sent.insert(p, bytes);
        }
    }

    /// Нажатие выключателя (`SignalSwitchSystem.OnActivated`): состояние переключается, порт `On` или
    /// `Off` посылает сигнал связанным лентам — вперёд или стоп.
    pub(crate) fn toggle_switch(&mut self, ctx: &HookContext<'_>, id: ObjectId) -> bool {
        let Some(obj) = ctx.world.authored_object(id) else {
            return false;
        };
        let on = !self.switch_on.contains(&id);
        if on {
            self.switch_on.insert(id);
        } else {
            self.switch_on.remove(&id);
        }
        let cells: BTreeSet<atmos::Cell> = obj
            .metadata
            .get("links")
            .map(String::as_str)
            .unwrap_or("")
            .split(';')
            .filter_map(|p| p.split_once(','))
            .filter_map(|(x, z)| {
                Some(atmos::floor_cell(
                    obj.floor.0,
                    x.parse().ok()?,
                    z.parse().ok()?,
                ))
            })
            .collect();
        let state = if on {
            crate::conveyor::State::Forward
        } else {
            crate::conveyor::State::Off
        };
        for o in ctx.world.authored() {
            let AuthoredKind::Placement { prototype } = &o.kind else {
                continue;
            };
            if self.conveyor_protos.contains_key(prototype)
                && cells.contains(&crate::piping::cell_at(
                    o.floor.0,
                    (o.min() + o.max()) * 0.5,
                ))
            {
                self.conveyor_state.insert(o.id, state);
            }
        }
        true
    }

    /// Ленты (10 раз в секунду): предметы над работающей лентой едут к её центру и вдоль неё.
    fn tick_conveyors(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        const STEP: f32 = 0.1;
        self.conveyor_timer += dt;
        if self.conveyor_timer < STEP {
            return;
        }
        let step = self.conveyor_timer;
        self.conveyor_timer = 0.0;
        if self
            .conveyor_state
            .values()
            .all(|s| *s == crate::conveyor::State::Off)
        {
            return;
        }
        let powered = self.powered_map();
        // Работающие ленты по клеткам: направление, скорость, центр.
        let mut belts: BTreeMap<atmos::Cell, ([f32; 2], f32, [f32; 2])> = BTreeMap::new();
        for o in ctx.world.authored() {
            let AuthoredKind::Placement { prototype } = &o.kind else {
                continue;
            };
            let Some(speed) = self.conveyor_protos.get(prototype) else {
                continue;
            };
            let state = self.conveyor_state.get(&o.id).copied().unwrap_or_default();
            let on = !self.power_required
                || !self.power_gated.contains(&o.id)
                || powered.get(&o.id).copied().unwrap_or(false);
            if state == crate::conveyor::State::Off || !on {
                continue;
            }
            let rot = o
                .metadata
                .get("rot")
                .and_then(|v| v.parse::<u8>().ok())
                .unwrap_or(0);
            let c = (o.min() + o.max()) * 0.5;
            belts.insert(
                crate::piping::cell_at(o.floor.0, c),
                (crate::conveyor::direction(rot, state), *speed, [c.x, c.z]),
            );
        }
        if belts.is_empty() {
            return;
        }
        let mut moves = Vec::new();
        for o in ctx.world.authored() {
            let AuthoredKind::Placement { prototype } = &o.kind else {
                continue;
            };
            if !self.infos.contains_key(prototype) {
                continue;
            }
            let c = (o.min() + o.max()) * 0.5;
            let Some((dir, speed, centre)) = belts.get(&crate::piping::cell_at(o.floor.0, c))
            else {
                continue;
            };
            let v = crate::conveyor::convey(*dir, *speed, [centre[0] - c.x, centre[1] - c.z]);
            let delta = Vec3::new(v[0] * step, 0.0, v[1] * step);
            if delta.length() < 1e-4 {
                continue;
            }
            // Стена на пути: предмет не едет.
            let probe = c + Vec3::Y * 0.3;
            let len = delta.length();
            if ctx.unobstructed_distance(probe, probe + delta * (0.5 / len + 1.0)) < len * 1.5 {
                continue;
            }
            moves.push((o.clone(), delta));
        }
        for (mut obj, delta) in moves {
            let mm = |v: f32| (v * 1000.0).round() as i32;
            for k in [0, 2] {
                let d = mm(if k == 0 { delta.x } else { delta.z });
                obj.min_mm[k] += d;
                obj.max_mm[k] += d;
            }
            ctx.request(WorldCommand::RemoveAuthored { id: obj.id });
            ctx.request(WorldCommand::PlaceAuthored(obj));
        }
    }

    /// Положить предмет из руки в ёмкость диспозала (`AfterInteractUsing`).
    pub(crate) fn insert_into_unit(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        unit: ObjectId,
        hand: usize,
    ) -> bool {
        let now = self.clock;
        let Some(item) = self.actor(actor).hands.get(hand).cloned() else {
            return false;
        };
        let Some(slot) = self.disposal_state.get_mut(&unit) else {
            return false;
        };
        if slot.unit.insert(item, now).is_err() {
            return false;
        }
        let _ = self.actor(actor).hands.drop_hand(hand);
        self.publish(ctx, actor);
        true
    }

    /// Новички появляются на точках должностей `Passenger` и `Latejoin` карты по очереди; если на
    /// карте таких нет, остаётся точка появления по умолчанию.
    fn place_joiners(&mut self, ctx: &mut HookContext<'_>) {
        let ids: Vec<NetworkEntityId> = ctx.players().collect();
        self.placed.retain(|p| ids.contains(p));
        let fresh: Vec<NetworkEntityId> = ids
            .into_iter()
            .filter(|p| !self.placed.contains(p))
            .collect();
        if fresh.is_empty() {
            return;
        }
        let mut points: Vec<Vec3> = ctx
            .world
            .authored()
            .filter(|o| {
                matches!(o.kind, AuthoredKind::Zone)
                    && matches!(
                        o.metadata.get("spawn_job").map(String::as_str),
                        Some("Latejoin")
                    )
            })
            .map(|o| {
                let c = (o.min() + o.max()) * 0.5;
                Vec3::new(c.x, o.min().y + 0.05, c.z)
            })
            .collect();
        points.sort_by(|a, b| {
            a.to_array()
                .partial_cmp(&b.to_array())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for p in fresh {
            self.placed.insert(p);
            if points.is_empty() {
                continue;
            }
            let at = points[self.spawn_next % points.len()];
            self.spawn_next += 1;
            ctx.teleport(p, at);
        }
    }

    /// Группы сигнализаций из карты: монитор, его список связей (клетки) → датчики, устройства труб,
    /// огнезапоры (двери с материалом `ss14:firelock`, касающиеся клетки).
    fn sync_alarms(&mut self, ctx: &mut HookContext<'_>) {
        let mut sig: Vec<(ObjectId, atmos::Cell, u32)> = Vec::new();
        let mut firelock_doors = 0u32;
        for e in ctx.world.elements() {
            if matches!(e.kind, StructureKind::Opening(OpeningKind::Door))
                && e.material.as_str().ends_with("firelock")
            {
                firelock_doors += 1;
            }
        }
        for o in ctx.world.authored() {
            if let AuthoredKind::Placement { prototype } = &o.kind
                && self.monitor_kinds.contains_key(prototype)
            {
                sig.push((
                    o.id,
                    crate::piping::cell_at(o.floor.0, (o.min() + o.max()) * 0.5),
                    o.metadata.get("links").map_or(0, |l| l.len() as u32),
                ));
            }
        }
        sig.push((ObjectId::from_u128(0), (0, 0), firelock_doors));
        sig.sort();
        if sig == self.alarm_sig {
            return;
        }
        self.alarm_sig = sig;
        // Что лежит в каждой клетке: датчики, устройства труб.
        let mut at_cell: BTreeMap<atmos::Cell, Vec<(ObjectId, PrototypeId)>> = BTreeMap::new();
        for o in ctx.world.authored() {
            if let AuthoredKind::Placement { prototype } = &o.kind {
                at_cell
                    .entry(crate::piping::cell_at(o.floor.0, (o.min() + o.max()) * 0.5))
                    .or_default()
                    .push((o.id, prototype.clone()));
            }
        }
        // Огнезапоры по клеткам, которых касается их ребро.
        let mut doors_at: BTreeMap<atmos::Cell, Vec<ObjectId>> = BTreeMap::new();
        self.firelock_cells.clear();
        for e in ctx.world.elements() {
            let (StructureKind::Opening(OpeningKind::Door), Placement::Edge(g)) =
                (e.kind, e.placement)
            else {
                continue;
            };
            if !e.material.as_str().ends_with("firelock") {
                continue;
            }
            let f = e.floor.0;
            let (a, b) = match g.dir {
                astrocore_math::EdgeDir::AlongX => ((g.x, g.z - 1), (g.x, g.z)),
                astrocore_math::EdgeDir::AlongZ => ((g.x - 1, g.z), (g.x, g.z)),
            };
            self.firelock_cells.insert(
                e.id,
                (
                    atmos::floor_cell(f, a.0, a.1),
                    atmos::floor_cell(f, b.0, b.1),
                ),
            );
            for c in [a, b] {
                doors_at
                    .entry(atmos::floor_cell(f, c.0, c.1))
                    .or_default()
                    .push(e.id);
            }
        }
        let mut groups = Vec::new();
        for o in ctx.world.authored() {
            let AuthoredKind::Placement { prototype } = &o.kind else {
                continue;
            };
            if self.monitor_kinds.get(prototype) != Some(&true) {
                continue;
            }
            let here = crate::piping::cell_at(o.floor.0, (o.min() + o.max()) * 0.5);
            let mut group = AlarmGroup {
                monitors: vec![(o.id, here)],
                ..AlarmGroup::default()
            };
            let links = o.metadata.get("links").cloned().unwrap_or_default();
            for pair in links.split(';').filter(|s| !s.is_empty()) {
                let Some((x, z)) = pair.split_once(',') else {
                    continue;
                };
                let (Ok(x), Ok(z)) = (x.parse::<i32>(), z.parse::<i32>()) else {
                    continue;
                };
                let cell = atmos::floor_cell(o.floor.0, x, z);
                for (id, proto) in at_cell.get(&cell).into_iter().flatten() {
                    if self.monitor_kinds.get(proto) == Some(&false) {
                        group.monitors.push((*id, cell));
                    } else if self.pipe_parts.contains_key(proto) {
                        group.devices.push(*id);
                    }
                }
                group
                    .firelocks
                    .extend(doors_at.get(&cell).into_iter().flatten().copied());
            }
            group.firelocks.sort();
            group.firelocks.dedup();
            groups.push(group);
        }
        // Огнезапоры вне групп открыты (в SS14 они открыты, пока не сработала сигнализация).
        let grouped: BTreeSet<ObjectId> = groups
            .iter()
            .flat_map(|g| g.firelocks.iter().copied())
            .collect();
        for ids in doors_at.values() {
            for id in ids {
                if !grouped.contains(id) && ctx.door_open(*id) == Some(false) {
                    ctx.request_door(*id, DoorAction::Open, false);
                }
            }
        }
        self.alarm_groups = groups;
    }

    /// `FirelockSystem.Update`: раз в 1,5 с закрытый огнезапор измеряет разность давления и
    /// температуры по обе стороны (`PressureThreshold` 20 кПа, `TemperatureThreshold` 330 К) и
    /// не даёт руке открыть себя, пока она велика; ломик открывает силой.
    fn tick_firelocks(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        const PRESSURE_KPA: f32 = 20.0;
        const TEMPERATURE_K: f32 = 330.0;
        self.firelock_timer += dt;
        if self.firelock_timer < 1.5 {
            return;
        }
        self.firelock_timer = 0.0;
        let mut hold = BTreeMap::new();
        for (id, (a, b)) in &self.firelock_cells {
            if ctx.door_open(*id) != Some(false) {
                continue;
            }
            let (pa, pb) = (self.atmos.air(*a), self.atmos.air(*b));
            let pressure = (pa.pressure() - pb.pressure()).abs() > PRESSURE_KPA;
            let temperature = (pa.temperature() - pb.temperature()).abs() > TEMPERATURE_K;
            if pressure || temperature {
                hold.insert(*id, (pressure, temperature));
            }
        }
        self.firelock_hold = hold;
    }

    /// Получает ли потребитель всю мощность.
    fn powered_map(&self) -> BTreeMap<ObjectId, bool> {
        self.power_receivers
            .iter()
            .map(|(id, idx, want)| {
                (
                    *id,
                    self.power
                        .loads
                        .get(*idx)
                        .is_some_and(|l| l.receiving_power >= want * 0.999),
                )
            })
            .collect()
    }

    /// Раз в секунду: мониторы измеряют воздух, группа берёт самое тяжёлое состояние, при смене
    /// огнезапоры, вентили и скрубберы реагируют.
    fn tick_alarms(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        use crate::monitor::{AlarmState, evaluate};
        self.alarm_timer += dt;
        if self.alarm_timer < 1.0 {
            return;
        }
        self.alarm_timer = 0.0;
        self.sync_alarms(ctx);
        let powered = self.powered_map();
        let mut groups = std::mem::take(&mut self.alarm_groups);
        for g in &mut groups {
            for (id, cell) in &g.monitors {
                // Без питания монитор не обновляется.
                let on = !self.power_required
                    || !self.power_gated.contains(id)
                    || powered.get(id).copied().unwrap_or(false);
                if on {
                    // Настенный монитор стоит в клетке стены без воздуха: берём воздух соседней клетки.
                    let mut air = self.atmos.air(*cell);
                    if air.total_moles() <= 0.0 {
                        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                            let next = self.atmos.air((cell.0 + dx, cell.1 + dz));
                            if next.total_moles() > air.total_moles() {
                                air = next;
                            }
                        }
                    }
                    self.monitor_state.insert(*id, evaluate(&air));
                }
            }
            let state = g
                .monitors
                .iter()
                .map(|(id, _)| {
                    self.monitor_state
                        .get(id)
                        .copied()
                        .unwrap_or(AlarmState::Normal)
                })
                .max()
                .unwrap_or(AlarmState::Normal);
            if state == g.state && g.applied {
                continue;
            }
            g.state = state;
            g.applied = true;
            for fl in &g.firelocks {
                match state {
                    AlarmState::Danger => ctx.request_door(*fl, DoorAction::Close, false),
                    AlarmState::Normal => {
                        if ctx.door_open(*fl) == Some(false)
                            && self.firelock_hold.get(fl).is_none_or(|h| !h.0 && !h.1)
                        {
                            ctx.request_door(*fl, DoorAction::Open, false);
                        }
                    }
                    AlarmState::Warning => {}
                }
            }
            for dev in &g.devices {
                let Some(key) = self.pipe_keys.existing(*dev) else {
                    continue;
                };
                match self.atmos.pipes_mut().device_mut(key) {
                    Some(atmos::Device::VentPump(v)) => {
                        v.enabled = state != AlarmState::Danger;
                    }
                    Some(atmos::Device::Scrubber(s)) => {
                        s.enabled = true;
                        s.wide_net = state == AlarmState::Danger;
                    }
                    _ => {}
                }
            }
        }
        self.alarm_groups = groups;
    }

    /// Светильники клиентам: включён, когда получает всю мощность (или питание не требуется).
    fn publish_lights(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        self.lights_timer += dt;
        if self.lights_timer < 0.5 {
            return;
        }
        self.lights_timer = 0.0;
        let received: BTreeMap<ObjectId, bool> = self
            .power_receivers
            .iter()
            .map(|(id, idx, want)| {
                (
                    *id,
                    self.power
                        .loads
                        .get(*idx)
                        .is_some_and(|l| l.receiving_power >= want * 0.999),
                )
            })
            .collect();
        let mut view = crate::electric::LightsView {
            kinds: self.power_kinds.clone(),
            lights: Vec::new(),
        };
        for (id, p, k) in self.power_lights.iter().take(crate::electric::MAX_LIGHTS) {
            let on = !self.power_required || received.get(id).copied().unwrap_or(false);
            view.lights.push((
                (p.x * 10.0).round() / 10.0,
                (p.y * 10.0).round() / 10.0,
                (p.z * 10.0).round() / 10.0,
                *k,
                on,
            ));
        }
        let ids: Vec<NetworkEntityId> = ctx.players().collect();
        let fresh: Vec<NetworkEntityId> = ids
            .iter()
            .copied()
            .filter(|i| !self.lights_to.contains(i))
            .collect();
        if view == self.lights_sent && fresh.is_empty() {
            return;
        }
        let bytes = crate::electric::encode_lights(&view);
        self.lights_sent = view;
        self.lights_to = ids.iter().copied().collect();
        for id in ids {
            ctx.set_component(id, crate::electric::LIGHTS_COMPONENT, bytes.clone());
        }
    }

    /// Шаг решателя и питание устройств труб (`ApcPowerReceiver.Powered`): устройство с потребителем
    /// работает, только если получает всю мощность; без проводки на карте питание не требуется.
    fn tick_power(&mut self, dt: f32) {
        let gated = self.power_required;
        if gated {
            self.power.tick(dt);
        }
        let received: BTreeMap<ObjectId, bool> = self
            .power_receivers
            .iter()
            .map(|(id, idx, want)| {
                (
                    *id,
                    self.power
                        .loads
                        .get(*idx)
                        .is_some_and(|l| l.receiving_power >= want * 0.999),
                )
            })
            .collect();
        let keys: Vec<(ObjectId, u64)> = self.pipe_keys.keys_iter().collect();
        for (id, key) in keys {
            let on = !gated
                || !self.power_gated.contains(&id)
                || received.get(&id).copied().unwrap_or(false);
            self.atmos.pipes_mut().set_powered(key, on);
        }
    }

    /// Устройства, баллончики и их события (лопнувший баллон).
    fn tick_devices(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        self.tank_timer += dt;
        let step = atmos::grid::ATMOS_TIME * 9.0;
        while self.tank_timer >= step {
            self.tank_timer -= step;
            self.tick_tanks(ctx, step);
        }
        for event in self.atmos.take_device_events() {
            let atmos::DeviceEvent::Ruptured { key, .. } = event;
            // Лопнувший баллон исчезает из мира.
            let victim = self
                .pipe_keys
                .keys_iter()
                .find(|(_, k)| *k == key)
                .map(|(id, _)| id);
            if let Some(id) = victim {
                ctx.request(WorldCommand::RemoveAuthored { id });
            }
        }
    }

    /// Узел разъёма в клетке (для подключения баллона).
    pub(crate) fn port_node_at(&self, cell: atmos::Cell) -> Option<u64> {
        self.pipe_ports.get(&cell).copied()
    }

    /// Переносные баллончики: выпуск газа, реакции, разрыв при перегрузке.
    fn tick_tanks(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        let ids: Vec<ObjectId> = self.tanks.keys().copied().collect();
        for id in ids {
            // Где баллончик: в руках игрока или на земле.
            let holder = self.actors.iter().find_map(|(who, a)| {
                a.hands
                    .iter()
                    .enumerate()
                    .find(|(_, (_, _, it))| it.as_ref().is_some_and(|i| i.id == id))
                    .map(|(hand, _)| (*who, hand))
            });
            let cell = match holder {
                Some((who, _)) => self.cell_of(ctx, who),
                None => ctx
                    .world
                    .authored_object(id)
                    .map(|o| crate::piping::cell_at(o.floor.0, (o.min() + o.max()) * 0.5)),
            };
            let Some(cell) = cell else { continue };
            let Some(mut tank) = self.tanks.remove(&id) else {
                continue;
            };
            let status = self
                .atmos
                .with_air(cell, |air| tank.update(dt, Some(air)))
                .unwrap_or_else(|| tank.update(dt, None));
            if status == atmos::PressureStatus::Ruptured {
                match holder {
                    Some((who, hand)) => {
                        if let Some(a) = self.actors.get_mut(&who) {
                            let _ = a.hands.drop_hand(hand);
                        }
                        self.publish(ctx, who);
                    }
                    None => ctx.request(WorldCommand::RemoveAuthored { id }),
                }
                continue;
            }
            self.tanks.insert(id, tank);
        }
    }

    /// Баллончик предмета: создаётся из шаблона при первом обращении.
    pub(crate) fn tank_of(&mut self, item: &HeldItem) -> Option<&mut atmos::Tank> {
        if !self.tanks.contains_key(&item.id) {
            let t = self.tank_templates.get(&item.prototype)?.clone();
            self.tanks.insert(item.id, t);
        }
        self.tanks.get_mut(&item.id)
    }

    /// Клетка атмосферы под ногами игрока.
    pub(crate) fn cell_of(
        &self,
        ctx: &HookContext<'_>,
        id: NetworkEntityId,
    ) -> Option<atmos::Cell> {
        let pos = ctx.position_of(id)?;
        let floor = floor_at(ctx, pos.y).map_or(0, |f| f.0);
        Some(atmos::floor_cell(
            floor,
            pos.x.floor() as i32,
            pos.z.floor() as i32,
        ))
    }

    /// Разослать клиентам клетки с огнём, если их набор изменился.
    fn publish_fires(&mut self, ctx: &mut HookContext<'_>) {
        let mut views: Vec<crate::fire::FireView> = self
            .atmos
            .fires()
            .into_iter()
            .map(|f| {
                let floor = (f.cell.1 + atmos::FLOOR_STRIDE / 2).div_euclid(atmos::FLOOR_STRIDE);
                let z = f.cell.1 - floor * atmos::FLOOR_STRIDE;
                let y = ctx
                    .world
                    .floors()
                    .find(|x| x.id.0 == floor)
                    .map_or(0.0, |x| x.base_y);
                crate::fire::FireView {
                    x: f.cell.0 as f32 + 0.5,
                    y,
                    z: z as f32 + 0.5,
                    state: f.state,
                }
            })
            .collect();
        views.truncate(crate::fire::MAX_FIRES);
        let ids: Vec<NetworkEntityId> = ctx.players().collect();
        let fresh = ids.iter().any(|i| !self.fires_to.contains(i));
        if views == self.fires_sent && !fresh {
            return;
        }
        let bytes = crate::fire::encode_fires(&views);
        self.fires_sent = views;
        self.fires_to = ids.iter().copied().collect();
        for id in ids {
            ctx.set_component(id, crate::fire::FIRES_COMPONENT, bytes.clone());
        }
    }

    /// Шаг времени: отсчёт, прерывание движением и потерей условий, завершение.
    pub(crate) fn tick(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        self.clock += dt;
        self.last_dt = dt;
        self.tick_health(ctx, dt);
        let waiting: Vec<NetworkEntityId> = self
            .actors
            .iter()
            .filter(|(_, a)| a.doafter.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in waiting {
            let Some(pos) = ctx.position_of(id) else {
                self.cancel(ctx, id);
                continue;
            };
            let Some(d) = self.actors.get_mut(&id).and_then(|a| a.doafter.as_mut()) else {
                continue;
            };
            if d.action.breaks_on_move() && pos.distance(d.start) > BREAK_DISTANCE_M {
                self.cancel(ctx, id);
                continue;
            }
            d.elapsed += dt;
            if d.elapsed < d.delay {
                continue;
            }
            let action = d.action.clone();
            if let Some(a) = self.actors.get_mut(&id) {
                a.doafter = None;
            }
            self.publish_doafter(ctx, id);
            if self.can_start(ctx, id, &action) {
                self.run(ctx, id, action);
            }
        }
    }

    /// Прототип сумки, если актёр может до неё дотянуться: она в руке, надета или лежит рядом.
    pub(crate) fn bag_proto(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        bag: ObjectId,
    ) -> Option<PrototypeId> {
        let a = self.actors.get(&actor)?;
        if let Some(i) = a
            .hands
            .held()
            .chain(a.inventory.worn())
            .find(|i| i.id == bag)
        {
            return Some(i.prototype.clone());
        }
        let feet = ctx.position_of(actor)?;
        let obj = ctx.world.authored_object(bag)?;
        let AuthoredKind::Placement { prototype } = &obj.kind else {
            return None;
        };
        self.reachable(ctx, feet, obj).then(|| prototype.clone())
    }

    /// Положить предмет из руки в сумку.
    pub(crate) fn store(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        bag: ObjectId,
    ) -> bool {
        let Some(item) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return false;
        };
        if item.id == bag {
            return false;
        }
        let (Some(bag_proto), Some(info)) = (
            self.bag_proto(ctx, actor, bag),
            self.info(&item.prototype).cloned(),
        ) else {
            return false;
        };
        let Some(spec) = self.info(&bag_proto).and_then(|i| i.storage.clone()) else {
            return false;
        };
        let storage = self.storages.entry(bag).or_default();
        if storage.insert(&spec, item, &info).is_err() {
            return false;
        }
        let _ = self.actor(actor).hands.drop_hand(hand);
        self.publish(ctx, actor);
        true
    }

    /// Достать предмет из сумки в свободную руку.
    pub(crate) fn take(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        bag: ObjectId,
        id: ObjectId,
        hand: Option<usize>,
    ) -> bool {
        if self.bag_proto(ctx, actor, bag).is_none() {
            return false;
        }
        let Some(storage) = self.storages.get_mut(&bag) else {
            return false;
        };
        let Some(stored) = storage.items().iter().find(|s| s.item.id == id) else {
            return false;
        };
        let item = stored.item.clone();
        // Сначала место в руках: если его нет, предмет остаётся в сумке.
        let hands = &mut self.actor(actor).hands;
        let taken = match hand {
            Some(h) => hands
                .pickup_to(h, item.clone())
                .or_else(|_| hands.pickup_any(item)),
            None => hands.pickup_any(item),
        };
        if taken.is_err() {
            return false;
        }
        if let Some(s) = self.storages.get_mut(&bag) {
            let _ = s.remove(id);
        }
        self.publish(ctx, actor);
        true
    }
}

/// Урон из данных прототипа: карта «вид → число».
fn damage_spec(d: Option<&prototypes::Data>) -> DamageSpec {
    d.and_then(prototypes::Data::as_map)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32)))
                .collect()
        })
        .unwrap_or_default()
}

/// Воздух под ногами персонажа: клетка сетки своего этажа. Вне сетки (в пустоте) дышать нечем.
struct ActorEnv<'a> {
    grid: Option<&'a mut atmos::Grid>,
    cell: (i32, i32),
}

impl Environment for ActorEnv<'_> {
    fn take_breath(&mut self, litres: f32) -> Option<atmos::GasMixture> {
        let cell = self.cell;
        self.grid.as_mut()?.remove_volume(cell, litres)
    }

    fn give_back(&mut self, gas: &atmos::GasMixture) {
        let cell = self.cell;
        if let Some(g) = self.grid.as_mut() {
            g.merge_into(cell, gas);
        }
    }

    fn pressure(&self) -> f32 {
        self.grid.as_ref().map_or(0.0, |g| g.pressure(self.cell))
    }

    fn fire_temperature(&self) -> Option<f32> {
        self.grid
            .as_ref()?
            .hotspot(self.cell)
            .map(|h| h.temperature)
    }

    fn oxygen_moles(&self) -> f32 {
        self.grid
            .as_ref()
            .map_or(0.0, |g| g.air(self.cell).moles(atmos::Gas::Oxygen))
    }

    fn air_thermal(&self) -> Option<(f32, f32)> {
        let a = self.grid.as_ref()?.air(self.cell);
        (!self.grid.as_ref()?.is_space(self.cell))
            .then(|| (a.temperature(), a.heat_capacity() * atmos::HEAT_SCALE))
    }

    fn set_air_temperature(&mut self, t: f32) {
        let cell = self.cell;
        if let Some(g) = self.grid.as_mut() {
            g.with_air(cell, |a| a.set_temperature(t));
        }
    }

    fn ignite_tile(&mut self, temperature: f32) {
        let cell = self.cell;
        if let Some(g) = self.grid.as_mut() {
            g.ignite(cell, temperature, IGNITION_SOURCE_VOLUME);
        }
    }
}

/// Объём, которым источник огня «выдыхает» на клетку (`HotspotExpose(..., 50)`).
pub const IGNITION_SOURCE_VOLUME: f32 = 50.0;

/// Достаточно ли изменилось состояние тела, чтобы слать его заново.
fn vitals_differ(a: &Vitals, b: &Vitals) -> bool {
    a.hunger.round() != b.hunger.round()
        || a.thirst.round() != b.thirst.round()
        || a.hunger_alert != b.hunger_alert
        || a.thirst_alert != b.thirst_alert
        || a.pressure_alert != b.pressure_alert
        || a.pressure_kpa.round() != b.pressure_kpa.round()
        || a.low_oxygen != b.low_oxygen
        || a.on_fire != b.on_fire
        || a.temperature_alert != b.temperature_alert
        || a.temperature.round() != b.temperature.round()
        || a.resisting != b.resisting
}

/// Этаж, в который попадает высота `y` (ступни стоят на нижней границе).
fn floor_at(ctx: &HookContext<'_>, y: f32) -> Option<FloorId> {
    ctx.world
        .floors()
        .find(|f| y >= f.base_y - 0.5 && y < f.top_y())
        .map(|f| f.id)
}

/// Модуль рук, инвентаря, хранилищ и взаимодействий; логика следует Content.Shared SS14.
///
/// Предметы на земле — авторские объекты мира (`AuthoredKind::Placement`): они реплицируются,
/// сохраняются и валидируются движком. Предметы в руках, на теле и в сумках живут в состоянии
/// модуля; руки видны всем (`build.hands`), инвентарь и содержимое сумок — только владельцу
/// (`build.inventory`). Взаимодействия идут через цепочку обработчиков (`InteractionHandler`):
/// форки добавляют свои, не трогая базовые.
pub struct GameplayModule {
    state: State,
    handlers: Vec<Box<dyn InteractionHandler>>,
}

impl GameplayModule {
    /// `Err`, если два имени прототипов дают один идентификатор движка.
    pub fn new(registry: Arc<PrototypeRegistry>) -> Result<Self, prototypes::ProtoError> {
        Ok(Self {
            state: State::new(registry)?,
            handlers: crate::handlers::default_handlers(),
        })
    }

    #[must_use]
    pub fn with_range(mut self, range_m: f32) -> Self {
        self.state.range_m = range_m;
        self
    }

    /// Точка возрождения (ступни); без неё призраки не возрождаются.
    #[must_use]
    pub fn with_spawn(mut self, spawn: Vec3) -> Self {
        self.state.spawn = Some(spawn);
        self
    }

    /// Добавить обработчик в конец цепочки (после базовых).
    #[must_use]
    pub fn with_handler(mut self, handler: Box<dyn InteractionHandler>) -> Self {
        self.handlers.push(handler);
        self
    }

    pub fn hands_of(&self, actor: NetworkEntityId) -> Option<&Hands> {
        self.state.hands_of(actor)
    }

    pub fn inventory_of(&self, actor: NetworkEntityId) -> Option<&Inventory> {
        self.state.inventory_of(actor)
    }

    pub fn storage_of(&self, bag: ObjectId) -> Option<&Storage> {
        self.state.storage_of(bag)
    }

    /// Цель взаимодействия по идентификатору: предмет на земле или дверь.
    fn resolve_target(&self, ctx: &HookContext<'_>, id: ObjectId) -> Option<Target> {
        if let Some(obj) = ctx.world.authored_object(id)
            && let AuthoredKind::Placement { prototype } = &obj.kind
        {
            return Some(Target::Item {
                id,
                prototype: prototype.clone(),
                at: (obj.min() + obj.max()) * 0.5,
            });
        }
        let e = ctx.world.element(id)?;
        let (StructureKind::Opening(OpeningKind::Door), Placement::Edge(edge)) =
            (e.kind, e.placement)
        else {
            return None;
        };
        let floor = ctx.world.floor(e.floor)?;
        let (a, b) = edge.endpoints(ctx.world.config().grid_step_m, floor.base_y);
        // Точка двери на высоте груди: от неё считаются дальность и видимость.
        Some(Target::Door {
            element: id,
            at: Vec3::new((a.x + b.x) * 0.5, floor.base_y + CHEST_M, (a.z + b.z) * 0.5),
        })
    }

    /// Взаимодействие с целью (`UserInteraction` в SS14): дальность и видимость, затем цепочка
    /// обработчиков — пустая рука или предмет в активной руке.
    fn interact(&mut self, ctx: &mut HookContext<'_>, user: NetworkEntityId, id: ObjectId) {
        let (Some(feet), Some(target)) = (ctx.position_of(user), self.resolve_target(ctx, id))
        else {
            return;
        };
        let chest = feet + Vec3::Y * CHEST_M;
        let at = target.at();
        // Дверь стоит в стене: луч упирается в саму её коробку, поэтому для неё проверяем
        // видимость до точки чуть перед проёмом.
        let visible = match target {
            Target::Door { .. } => {
                ctx.unobstructed_distance(chest, at) >= (at - chest).length() - 0.35
            }
            Target::Item { .. } => ctx.is_unobstructed(chest, at),
        };
        if (at - chest).length() > self.state.range_m || !visible {
            return;
        }
        let used = self.state.used_item(user);
        let mut handlers = std::mem::take(&mut self.handlers);
        match used {
            None => {
                for h in &mut handlers {
                    if h.interact_hand(&mut self.state, ctx, user, &target) {
                        break;
                    }
                }
            }
            Some(used) => {
                let handled = handlers
                    .iter_mut()
                    .any(|h| h.interact_using(&mut self.state, ctx, user, &used, &target));
                if !handled {
                    // Обработчики самого предмета: инструмент в руке «после» взаимодействия.
                    let _ = handlers
                        .iter_mut()
                        .any(|h| h.after_interact(&mut self.state, ctx, user, &used, &target));
                }
            }
        }
        self.handlers = handlers;
    }

    fn use_in_hand(&mut self, ctx: &mut HookContext<'_>, user: NetworkEntityId, hand: usize) {
        let Some(used) = self.state.used_in(user, hand) else {
            return;
        };
        let mut handlers = std::mem::take(&mut self.handlers);
        let _ = handlers
            .iter_mut()
            .any(|h| h.use_in_hand(&mut self.state, ctx, user, &used));
        self.handlers = handlers;
    }
}

impl GameModule for GameplayModule {
    fn id(&self) -> &str {
        "build.gameplay"
    }

    fn handle(&mut self, ctx: &mut HookContext<'_>, event: &HookEvent) {
        let st = &mut self.state;
        match event {
            HookEvent::Command { actor, kind, .. } if kind == CMD_RESPAWN => {
                st.respawn(ctx, *actor);
            }
            HookEvent::Command { actor, .. }
                if st
                    .actors
                    .get(actor)
                    .is_some_and(|a| !a.health.state().can_act()) => {}
            HookEvent::Command { actor, kind, data } => match kind.as_str() {
                CMD_USE_GROUND => {
                    if let Some((hand, Some([x, y, z]))) = crate::wire::parse_drop(data) {
                        st.use_on_ground(ctx, *actor, usize::from(hand), Vec3::new(x, y, z));
                    }
                }
                CMD_SPILL => {
                    if let [hand] = data[..] {
                        st.spill_item(ctx, *actor, usize::from(hand));
                    }
                }
                crate::wire::CMD_VEND => {
                    if let Ok(item) = std::str::from_utf8(data) {
                        st.vend(ctx, *actor, item);
                    }
                }
                crate::wire::CMD_JOB => {
                    if let Ok(job) = std::str::from_utf8(data) {
                        st.pick_job(ctx, *actor, job);
                    }
                }
                crate::wire::CMD_VEND_CLOSE => {
                    st.vending_open.remove(actor);
                }
                CMD_RESIST => {
                    let resisted = st
                        .actors
                        .get_mut(actor)
                        .and_then(|a| a.body.as_mut())
                        .is_some_and(Body::resist);
                    if resisted {
                        st.after_health_change(ctx, *actor);
                    }
                }
                CMD_ATTACK => {
                    if let Some((target, part)) = parse_attack_part(data) {
                        st.attack(ctx, *actor, target, part);
                    }
                }
                CMD_FEED => {
                    if let Some((hand, target)) = parse_heal(data)
                        && target != *actor
                        && let Some(it) = st
                            .actors
                            .get(actor)
                            .and_then(|a| a.hands.get(usize::from(hand)))
                            .cloned()
                    {
                        st.begin(
                            ctx,
                            *actor,
                            Action::Eat {
                                hand: usize::from(hand),
                                item: it.id,
                                target: Some(target),
                            },
                        );
                    }
                }
                CMD_HEAL => {
                    if let Some((hand, target)) = parse_heal(data) {
                        st.begin(
                            ctx,
                            *actor,
                            Action::Heal {
                                hand: usize::from(hand),
                                target,
                            },
                        );
                    }
                }
                CMD_PICKUP | CMD_PICKUP_ANY => {
                    if let Some(id) = parse_object_id(data) {
                        st.pickup(ctx, *actor, id, kind == CMD_PICKUP_ANY);
                    }
                }
                CMD_DROP => {
                    if let Some((hand, target)) = parse_drop(data) {
                        st.drop_hand(ctx, *actor, usize::from(hand), target);
                    }
                }
                CMD_SELECT_HAND => {
                    if let [hand] = data[..]
                        && st.actor(*actor).hands.select(usize::from(hand)).is_ok()
                    {
                        st.publish(ctx, *actor);
                    }
                }
                CMD_MOVE_HAND => {
                    if let [from, to] = data[..]
                        && st
                            .actor(*actor)
                            .hands
                            .exchange(usize::from(from), usize::from(to))
                            .is_ok()
                    {
                        st.publish(ctx, *actor);
                    }
                }
                CMD_SWAP_HANDS => {
                    let reverse = data.first() == Some(&1);
                    st.actor(*actor).hands.swap(reverse);
                    st.publish(ctx, *actor);
                }
                CMD_EQUIP => {
                    if let Some((hand, slot)) = parse_equip(data) {
                        st.begin(
                            ctx,
                            *actor,
                            Action::Equip {
                                hand: usize::from(hand),
                                slot: slot.to_string(),
                            },
                        );
                    }
                }
                CMD_UNEQUIP => {
                    if let Some(slot) = parse_unequip(data) {
                        st.begin(
                            ctx,
                            *actor,
                            Action::Unequip {
                                slot: slot.to_string(),
                                hand: None,
                            },
                        );
                    }
                }
                CMD_CANCEL => st.cancel(ctx, *actor),
                CMD_UNEQUIP_TO => {
                    if let Some((hand, slot)) = parse_equip(data) {
                        st.begin(
                            ctx,
                            *actor,
                            Action::Unequip {
                                slot: slot.to_string(),
                                hand: Some(usize::from(hand)),
                            },
                        );
                    }
                }
                CMD_TAKE_TO => {
                    if let Some((hand, bag, item)) = parse_take_to(data) {
                        st.take(ctx, *actor, bag, item, Some(usize::from(hand)));
                    }
                }
                CMD_MOVE_SLOT => {
                    if let Some((from, to)) = parse_move_slot(data) {
                        st.move_slot(ctx, *actor, from, to);
                    }
                }
                CMD_STORE => {
                    if let Some((hand, bag)) = parse_store(data) {
                        st.store(ctx, *actor, usize::from(hand), bag);
                    }
                }
                CMD_TAKE => {
                    if let Some((bag, item)) = parse_take(data) {
                        st.take(ctx, *actor, bag, item, None);
                    }
                }
                CMD_INTERACT => {
                    if let Some(id) = parse_object_id(data) {
                        self.interact(ctx, *actor, id);
                    }
                }
                CMD_USE => {
                    if let [hand] = data[..] {
                        self.use_in_hand(ctx, *actor, usize::from(hand));
                    }
                }
                _ => {}
            },
            HookEvent::Tick { dt, .. } => st.tick(ctx, *dt),
            HookEvent::PlayerLeft { actor, pos } => {
                // Всё, что игрок держал и носил, остаётся лежать там, где он стоял (DropAll);
                // содержимое сумок остаётся в сумках.
                st.release_job(*actor);
                if let Some(mut a) = st.actors.remove(actor) {
                    for item in a.hands.take_all().into_iter().chain(a.inventory.take_all()) {
                        State::place(ctx, &item, *pos);
                    }
                }
            }
            _ => {}
        }
    }
}
