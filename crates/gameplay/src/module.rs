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
use crate::chat::ChatState;
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
use crate::lobby::Lobby;
use crate::reagents::Solution;
use crate::round_end::RoundState;
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
const CHEST_M: f32 = 0.55;
/// Скорость шага относительно бега.
const WALK_FACTOR: f32 = 2.5 / 4.5;
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
    /// Боевой режим включён; идёт шагом, а не бегом.
    pub(crate) combat: bool,
    pub(crate) walking: bool,
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
    /// Тело, оставшееся после смерти (предмет-труп в мире): по нему дефибриллятор находит владельца.
    pub(crate) corpse: Option<ObjectId>,
    /// Криокапсула, в которой стоит персонаж.
    pub(crate) cryo: Option<ObjectId>,
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
            combat: false,
            walking: false,
            next_health_publish: 0.0,
            health_dirty: false,
            ghost: false,
            respawn_at: 0.0,
            death_pos: None,
            body: Some(Body::new(seed)),
            last_vitals: None,
            slip_until: 0.0,
            corpse: None,
            cryo: None,
            last_step: None,
        }
    }
}

/// Удостоверение по прототипу: уровни доступа и должность, по которой они выдаются.
#[derive(Debug, Clone)]
pub(crate) struct IdSpec {
    pub(crate) access: Vec<String>,
    pub(crate) job: Option<String>,
}

/// Поведение режима шприца (`InjectorBehavior`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Behavior {
    Inject,
    Draw,
    Dynamic,
}

/// Режим шприца (`InjectorModePrototype`).
#[derive(Debug, Clone)]
pub(crate) struct InjectorMode {
    pub(crate) behavior: Behavior,
    pub(crate) amounts: Vec<f32>,
    pub(crate) mob_time: f32,
    pub(crate) self_modifier: f32,
    pub(crate) downed_modifier: f32,
    pub(crate) delay_per_volume: f32,
    pub(crate) ignore_delay_for_volume: f32,
}

/// Шприц или гипоспрей по прототипу.
#[derive(Debug, Clone)]
pub(crate) struct InjectorSpec {
    pub(crate) modes: Vec<InjectorMode>,
    pub(crate) active: usize,
    /// Сколько переливать за раз; `None` — всё содержимое (автоинжектор).
    pub(crate) amount: Option<f32>,
}

impl InjectorSpec {
    fn parse(c: &prototypes::Data) -> Option<Self> {
        let modes: Vec<InjectorMode> = c
            .get("modes")?
            .as_list()?
            .iter()
            .filter_map(|d| {
                let f = |k: &str, def: f32| {
                    d.get(k)
                        .and_then(prototypes::Data::as_f64)
                        .map_or(def, |v| v as f32)
                };
                Some(InjectorMode {
                    behavior: match d.get("behavior")?.as_str()? {
                        "Draw" => Behavior::Draw,
                        "Dynamic" => Behavior::Dynamic,
                        _ => Behavior::Inject,
                    },
                    amounts: d
                        .get("amounts")
                        .and_then(prototypes::Data::as_list)
                        .map(|l| {
                            l.iter()
                                .filter_map(|x| x.as_f64().map(|v| v as f32))
                                .collect()
                        })
                        .unwrap_or_default(),
                    mob_time: f("mobTime", 5.0),
                    self_modifier: f("selfModifier", 0.5),
                    downed_modifier: f("downedModifier", 0.5),
                    delay_per_volume: f("delayPerVolume", 0.1),
                    ignore_delay_for_volume: f("ignoreDelayForVolume", 5.0),
                })
            })
            .collect();
        if modes.is_empty() {
            return None;
        }
        let amount = c
            .get("amount")
            .and_then(prototypes::Data::as_f64)
            .map(|v| v as f32);
        Some(Self {
            modes,
            active: c
                .get("active")
                .and_then(prototypes::Data::as_int)
                .unwrap_or(0) as usize,
            amount: amount.filter(|a| *a > 0.0),
        })
    }
}

/// Раздатчик реагентов в мире: банки, мензурка в слоте и выбранное количество.
#[derive(Debug, Clone)]
pub(crate) struct Dispenser {
    pub(crate) jugs: Vec<HeldItem>,
    pub(crate) beaker: Option<HeldItem>,
    pub(crate) amount: f32,
}

/// Слоты консоли выдачи ID: карта с правами и целевая карта.
#[derive(Debug, Clone, Default)]
pub(crate) struct ConsoleSlots {
    pub(crate) privileged: Option<HeldItem>,
    pub(crate) target: Option<HeldItem>,
}

/// Шкаф или ящик по прототипу закрытого вида.
#[derive(Debug, Clone)]
pub(crate) struct StorageSpec {
    pub(crate) capacity: usize,
    /// Имя прототипа открытого вида.
    pub(crate) open_name: String,
    /// Замок: `Some(начальное состояние)`; `None` — замка нет.
    pub(crate) lock: Option<bool>,
    pub(crate) access: Vec<Vec<String>>,
    pub(crate) fill: Option<crate::spawners::Node>,
}

/// Шкаф или ящик в мире: открыт ли, заперт ли и что лежит внутри.
#[derive(Debug, Clone)]
pub(crate) struct Locker {
    pub(crate) closed_proto: PrototypeId,
    pub(crate) open: bool,
    pub(crate) locked: bool,
    pub(crate) contents: Vec<HeldItem>,
}

/// Торговый автомат по прототипу.
#[derive(Debug, Clone)]
pub(crate) struct VendSpec {
    pub(crate) title: String,
    pub(crate) stock: Vec<(String, PrototypeId, u32)>,
    pub(crate) quality: f32,
    pub(crate) eject_delay: f32,
    pub(crate) access: Vec<Vec<String>>,
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
    pub(crate) owner: Option<ObjectId>,
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
    pub(crate) power_suppliers: Vec<(ObjectId, usize)>,
    pub(crate) power_cables: Vec<(power::CableGroup, atmos::Cell, usize)>,
    pub(crate) power_monitor: crate::power_monitor::Monitoring,
    pub(crate) alerts: crate::alerts::Alerts,
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
    pub(crate) alarm_sig: Vec<(ObjectId, atmos::Cell, String)>,
    pub(crate) monitor_state: BTreeMap<ObjectId, crate::monitor::AlarmState>,
    pub(crate) alarm_timer: f32,
    pub(crate) transfer_amount: BTreeMap<ObjectId, f32>,
    pub(crate) chemhand_sent: BTreeMap<NetworkEntityId, Vec<u8>>,
    pub(crate) transfer_max: BTreeMap<PrototypeId, f32>,
    /// Очередь подсказок игрокам и звуков в мире за тик, номера пачек.
    pub(crate) fx_popups: Vec<(NetworkEntityId, String)>,
    pub(crate) fx_sounds: Vec<(String, Vec3, f32)>,
    pub(crate) fx_seq: BTreeMap<NetworkEntityId, u32>,
    /// Анализаторы здоровья: прототипы, кто кого сканирует, таймер обновления.
    pub(crate) analyzers: BTreeSet<PrototypeId>,
    pub(crate) lab: crate::chemlab::LabSpecs,
    pub(crate) cryo_specs: BTreeMap<PrototypeId, crate::cryo::CryoSpec>,
    pub(crate) cryo: BTreeMap<ObjectId, crate::cryo::CryoPod>,
    pub(crate) cryo_open: BTreeMap<NetworkEntityId, ObjectId>,
    pub(crate) cryo_sent: BTreeMap<NetworkEntityId, Vec<u8>>,
    pub(crate) mixer_machines: BTreeMap<ObjectId, crate::chemlab::MixerMachine>,
    pub(crate) masters: BTreeMap<ObjectId, crate::chemlab::Master>,
    pub(crate) lab_open: BTreeMap<NetworkEntityId, ObjectId>,
    pub(crate) lab_sent: BTreeMap<NetworkEntityId, Vec<u8>>,
    pub(crate) heat_timer: f32,
    /// Названия прототипов для окон.
    pub(crate) titles: BTreeMap<PrototypeId, String>,
    /// Ярлыки ХимМастера у бутылок, банок и таблеток.
    pub(crate) labels: BTreeMap<ObjectId, String>,
    pub(crate) defibs: BTreeMap<PrototypeId, crate::defib::DefibSpec>,
    pub(crate) defib_state: BTreeMap<ObjectId, crate::defib::DefibState>,
    pub(crate) scans: BTreeMap<NetworkEntityId, NetworkEntityId>,
    pub(crate) scan_sent: BTreeMap<NetworkEntityId, Vec<u8>>,
    pub(crate) scan_timer: f32,
    /// Раздатчики реагентов: таблицы наполнения, состояние, у кого открыто окно, предметы под мензурку.
    pub(crate) dispenser_tables: BTreeMap<PrototypeId, crate::spawners::Node>,
    pub(crate) dispensers: BTreeMap<ObjectId, Dispenser>,
    pub(crate) dispenser_open: BTreeMap<NetworkEntityId, ObjectId>,
    pub(crate) dispenser_sent: BTreeMap<NetworkEntityId, Vec<u8>>,
    pub(crate) fits_in_dispenser: BTreeSet<PrototypeId>,
    /// Химия: таблица реагентов и реакций, шприцы и гипоспреи, перелив между ёмкостями.
    pub(crate) chem: std::sync::Arc<crate::chem::ReagentTable>,
    pub(crate) injectors: BTreeMap<PrototypeId, InjectorSpec>,
    pub(crate) injector_state: BTreeMap<ObjectId, (usize, f32)>,
    pub(crate) transfers: BTreeMap<PrototypeId, f32>,
    pub(crate) drainables: BTreeSet<PrototypeId>,
    /// Консоли выдачи ID: уровни по прототипу, слоты, у кого открыто окно, имена на картах.
    pub(crate) id_console_levels: BTreeMap<PrototypeId, Vec<String>>,
    pub(crate) consoles: BTreeMap<ObjectId, ConsoleSlots>,
    pub(crate) console_open: BTreeMap<NetworkEntityId, ObjectId>,
    pub(crate) console_sent: BTreeMap<NetworkEntityId, Vec<u8>>,
    pub(crate) console_protos: BTreeMap<ObjectId, PrototypeId>,
    pub(crate) card_names: BTreeMap<ObjectId, (String, String)>,
    /// Шкафы и ящики: описания закрытых прототипов, соответствие «открытый → закрытый», состояние.
    pub(crate) storage_specs: BTreeMap<PrototypeId, StorageSpec>,
    pub(crate) storage_open_of: BTreeMap<PrototypeId, PrototypeId>,
    pub(crate) lockers: BTreeMap<ObjectId, Locker>,
    pub(crate) locker_timer: f32,
    /// Удостоверения: уровни доступа по прототипу, карта в КПК, выданные при выборе должности.
    pub(crate) id_specs: BTreeMap<PrototypeId, IdSpec>,
    pub(crate) pda_cards: BTreeMap<PrototypeId, String>,
    pub(crate) card_access: BTreeMap<ObjectId, BTreeSet<String>>,
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
    /// Лобби: `None` — выключено (игроки сразу выходят на станцию).
    pub(crate) lobby: Option<Lobby>,
    /// Конец раунда: сигнал серверу, предел времени, часы раунда, экипаж, итог.
    pub(crate) round: RoundState,
    /// Чат: журнал строк по получателям.
    pub(crate) chat: ChatState,
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
        let mut transfer_max = BTreeMap::new();
        let mut analyzers = BTreeSet::new();
        let mut defibs = BTreeMap::new();
        let mut lab = crate::chemlab::LabSpecs::default();
        let mut cryo_specs = BTreeMap::new();
        let mut titles = BTreeMap::new();
        let mut dispenser_tables = BTreeMap::new();
        let mut fits_in_dispenser = BTreeSet::new();
        let mut injectors = BTreeMap::new();
        let mut transfers = BTreeMap::new();
        let mut drainables = BTreeSet::new();
        let mut id_console_levels = BTreeMap::new();
        let mut storage_specs = BTreeMap::new();
        let mut storage_open_of: BTreeMap<PrototypeId, PrototypeId> = BTreeMap::new();
        let mut id_specs = BTreeMap::new();
        let mut pda_cards = BTreeMap::new();
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
                if let Some(spec) = r.component("Injector").and_then(InjectorSpec::parse) {
                    injectors.insert(id.clone(), spec);
                }
                if let Some(c) = r.component("SolutionTransfer") {
                    transfer_max.insert(
                        id.clone(),
                        c.get("max")
                            .and_then(prototypes::Data::as_f64)
                            .unwrap_or(1000.0) as f32,
                    );
                    transfers.insert(
                        id.clone(),
                        c.get("amount")
                            .and_then(prototypes::Data::as_f64)
                            .unwrap_or(5.0) as f32,
                    );
                }
                if r.component("Drainable").is_some() {
                    drainables.insert(id.clone());
                }
                if let Some(t) = r
                    .component("ReagentDispenser")
                    .and_then(|c| c.get("table"))
                    .and_then(crate::spawners::Node::parse)
                {
                    dispenser_tables.insert(id.clone(), t);
                }
                lab.load(&id, r);
                if let Some(c) = crate::cryo::CryoSpec::parse(r) {
                    cryo_specs.insert(id.clone(), c);
                }
                if let Some(n) = r.name.as_ref() {
                    titles.insert(id.clone(), n.clone());
                }
                if let Some(c) = r.component("Defibrillator") {
                    defibs.insert(id.clone(), crate::defib::DefibSpec::parse(c));
                }
                if r.component("HealthAnalyzer").is_some() {
                    analyzers.insert(id.clone());
                }
                if r.component("FitsInDispenser").is_some() {
                    fits_in_dispenser.insert(id.clone());
                }
                if let Some(c) = r.component("IdCardConsole") {
                    id_console_levels.insert(
                        id.clone(),
                        c.get("access")
                            .and_then(prototypes::Data::as_list)
                            .map(|l| {
                                l.iter()
                                    .filter_map(|x| x.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    );
                }
                if let Some(c) = r.component("EntityStorage") {
                    storage_specs.insert(
                        id.clone(),
                        StorageSpec {
                            capacity: c
                                .get("capacity")
                                .and_then(prototypes::Data::as_int)
                                .unwrap_or(30) as usize,
                            open_name: c
                                .get("openProto")
                                .and_then(prototypes::Data::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            lock: r.component("Lock").map(|l| {
                                l.get("locked")
                                    .and_then(prototypes::Data::as_bool)
                                    .unwrap_or(true)
                            }),
                            access: r
                                .component("AccessReader")
                                .map(access_lists)
                                .unwrap_or_default(),
                            fill: r
                                .component("ContainerFill")
                                .and_then(|f| f.get("table"))
                                .and_then(crate::spawners::Node::parse),
                        },
                    );
                }
                if let Some(closed) = r
                    .component("EntityStorageOpen")
                    .and_then(|c| c.get("closedProto"))
                    .and_then(prototypes::Data::as_str)
                    && let Some(cid) = by_name.get(closed)
                {
                    storage_open_of.insert(id.clone(), cid.clone());
                }
                if let Some(c) = r.component("IdCard") {
                    id_specs.insert(
                        id.clone(),
                        IdSpec {
                            access: c
                                .get("access")
                                .and_then(prototypes::Data::as_list)
                                .map(|l| {
                                    l.iter()
                                        .filter_map(|x| x.as_str().map(String::from))
                                        .collect()
                                })
                                .unwrap_or_default(),
                            job: c
                                .get("job")
                                .and_then(prototypes::Data::as_str)
                                .map(String::from),
                        },
                    );
                }
                if let Some(card) = r
                    .component("Pda")
                    .and_then(|c| c.get("card"))
                    .and_then(prototypes::Data::as_str)
                {
                    pda_cards.insert(id.clone(), card.to_string());
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
                            access: r
                                .component("AccessReader")
                                .map(access_lists)
                                .unwrap_or_default(),
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
            power_monitor: crate::power_monitor::Monitoring::load(&registry),
            alerts: crate::alerts::Alerts::load(&registry),
            power_suppliers: Vec::new(),
            power_cables: Vec::new(),
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
            transfer_amount: BTreeMap::new(),
            chemhand_sent: BTreeMap::new(),
            transfer_max,
            fx_popups: Vec::new(),
            fx_sounds: Vec::new(),
            fx_seq: BTreeMap::new(),
            analyzers,
            defibs,
            defib_state: BTreeMap::new(),
            lab,
            cryo_specs,
            cryo: BTreeMap::new(),
            cryo_open: BTreeMap::new(),
            cryo_sent: BTreeMap::new(),
            mixer_machines: BTreeMap::new(),
            masters: BTreeMap::new(),
            lab_open: BTreeMap::new(),
            lab_sent: BTreeMap::new(),
            heat_timer: 0.0,
            titles,
            labels: BTreeMap::new(),
            scans: BTreeMap::new(),
            scan_sent: BTreeMap::new(),
            scan_timer: 0.0,
            dispenser_tables,
            dispensers: BTreeMap::new(),
            dispenser_open: BTreeMap::new(),
            dispenser_sent: BTreeMap::new(),
            fits_in_dispenser,
            chem: crate::chem::ReagentTable::builtin(),
            injectors,
            injector_state: BTreeMap::new(),
            transfers,
            drainables,
            id_console_levels,
            consoles: BTreeMap::new(),
            console_open: BTreeMap::new(),
            console_sent: BTreeMap::new(),
            console_protos: BTreeMap::new(),
            card_names: BTreeMap::new(),
            storage_specs,
            storage_open_of,
            lockers: BTreeMap::new(),
            locker_timer: 0.0,
            id_specs,
            pda_cards,
            card_access: BTreeMap::new(),
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
            lobby: None,
            round: RoundState::default(),
            chat: ChatState::default(),
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

    /// Достаёт ли рука до тела: живого — где он стоит, погибшего (призрак) — где лежит его труп.
    fn in_reach_body(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        target: NetworkEntityId,
    ) -> bool {
        let Some(t) = self.actors.get(&target) else {
            return self.in_reach(ctx, actor, target);
        };
        if !t.ghost {
            return self.in_reach(ctx, actor, target);
        }
        let (Some(a), Some(body)) = (ctx.position_of(actor), t.death_pos) else {
            return false;
        };
        let (a, body) = (a + Vec3::Y * CHEST_M, body + Vec3::Y * CHEST_M);
        t.corpse.is_some() && (a - body).length() <= MELEE_RANGE_M && ctx.is_unobstructed(a, body)
    }

    /// Включить или выключить дефибриллятор в руке (`ItemToggle` + `ToggleCellDraw`).
    pub(crate) fn toggle_defib(
        &mut self,
        ctx: &HookContext<'_>,
        user: NetworkEntityId,
        item: &HeldItem,
    ) -> bool {
        let Some(spec) = self.defibs.get(&item.prototype).cloned() else {
            return false;
        };
        let st = self
            .defib_state
            .entry(item.id)
            .or_insert_with(|| spec.fresh());
        let (was_on, can_on) = (st.on, spec.can_turn_on(st));
        if was_on || can_on {
            st.on = !was_on;
        }
        let at = ctx.position_of(user);
        let (text, sound) = match (was_on, can_on) {
            (true, _) => (
                "Дефибриллятор выключен",
                Some("Items/Defib/defib_safety_off.ogg"),
            ),
            (false, true) => (
                "Дефибриллятор включён",
                Some("Items/Defib/defib_safety_on.ogg"),
            ),
            (false, false) => ("Не хватает заряда", None),
        };
        self.popup(user, text);
        if let (Some(at), Some(sound)) = (at, sound) {
            self.sound(sound, at);
        }
        true
    }

    /// Начало приложения: звук зарядки и тревога цели (`TryStartZap`).
    pub(crate) fn defib_begin_feedback(
        &mut self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        target: NetworkEntityId,
    ) {
        if let Some(at) = ctx.position_of(actor) {
            self.sound("Items/Defib/defib_charge.ogg", at);
        }
        self.popup(actor, "Разряд заряжается");
        if target != actor {
            self.popup(target, "Вас сейчас ударят током из дефибриллятора");
        }
    }

    /// `Zap`: заряд тратится, начинается откат, цель лечится и бьётся током; погибший с уроном ниже порога смерти оживает.
    fn zap(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        target: NetworkEntityId,
    ) {
        let Some(item) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return;
        };
        let Some(spec) = self.defibs.get(&item.prototype).cloned() else {
            return;
        };
        let at = ctx.position_of(actor);
        let Some(st) = self.defib_state.get_mut(&item.id) else {
            return;
        };
        if spec.powered {
            st.charge -= spec.use_charge;
        }
        st.cooldown = spec.zap_delay;
        if spec.powered && !spec.can_fire(st) {
            st.on = false;
        }
        if let Some(at) = at {
            self.sound("Items/Defib/defib_zap.ogg", at);
        }
        let clock = self.clock;
        let t = self.actor(target);
        let was_gone = t.ghost;
        t.health.apply_forced(&spec.heal);
        let mut shock = DamageSpec::new();
        shock.insert("Shock".into(), spec.zap_damage);
        t.health.apply_forced(&shock);
        t.slip_until = t.slip_until.max(clock + spec.writhe);
        let revived = t.health.state() != MobState::Dead;
        if was_gone && revived {
            // Оживление: тело встаёт на месте трупа, владелец возвращается в него.
            let (corpse, pos) = (t.corpse.take(), t.death_pos.take());
            t.ghost = false;
            t.respawn_at = 0.0;
            if let Some(id) = corpse {
                ctx.request(WorldCommand::RemoveAuthored { id });
            }
            ctx.set_ghost(target, false);
            if let Some(pos) = pos {
                ctx.teleport(target, pos);
            }
        }
        self.after_health_change(ctx, target);
        let ok = !was_gone || revived;
        if let Some(at) = at {
            self.sound(
                if ok {
                    "Items/Defib/defib_success.ogg"
                } else {
                    "Items/Defib/defib_failed.ogg"
                },
                at,
            );
        }
        self.popup(
            actor,
            if ok {
                "Разряд прошёл"
            } else {
                "Не удалось: слишком тяжёлые повреждения"
            },
        );
        self.publish(ctx, actor);
    }

    /// Расход заряда у включённых и откат между разрядами.
    fn tick_defibs(&mut self, dt: f32) {
        for st in self.defib_state.values_mut() {
            st.tick(dt);
        }
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
        // `Pacified`: миролюбивый не бьёт.
        if a.body.as_ref().is_some_and(|b| b.has_status("Pacified")) {
            self.popup(actor, "Вы не можете ударить");
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
    /// Инъекция: из шприца в кровь `target` уходит `amount` (сколько есть и сколько влезает).
    fn inject_body(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        target: NetworkEntityId,
    ) {
        let Some(item) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return;
        };
        let Some((mode, amount)) = self.injector_of(&item) else {
            return;
        };
        if mode.behavior == Behavior::Draw {
            self.draw_blood(ctx, actor, &item, target, amount);
            return;
        }
        let room = self
            .actors
            .get(&target)
            .and_then(|a| a.body.as_ref())
            .map_or(0.0, |b| b.chemicals.available());
        let Some(sol) = self.solution_mut(&item) else {
            return;
        };
        let take = amount
            .unwrap_or_else(|| sol.volume())
            .min(sol.volume())
            .min(room);
        if take <= 0.0 {
            return;
        }
        let part = sol.split(take);
        if let Some(body) = self.actor(target).body.as_mut() {
            for (r, q) in &part.contents {
                body.chemicals.add(r, *q);
            }
        }
        self.popup(actor, format!("Введено {take:.0} ед."));
        if target != actor {
            self.popup(target, "Вам сделали укол");
        }
        if let Some(at) = ctx.position_of(actor) {
            self.sound("Items/hypospray.ogg", at);
        }
        self.publish(ctx, actor);
    }

    /// `InjectorSystem.DrawFromBlood`: из крови цели в шприц уходит `amount` (по умолчанию 5)
    /// — сколько есть и сколько влезает; кровь убывает.
    fn draw_blood(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        item: &HeldItem,
        target: NetworkEntityId,
        amount: Option<f32>,
    ) {
        let have = self
            .actors
            .get(&target)
            .and_then(|a| a.body.as_ref())
            .map_or(0.0, |b| b.blood_volume());
        let take = amount
            .unwrap_or(5.0)
            .min(have)
            .min(self.solution_room(item));
        if take <= 0.0 {
            self.popup(actor, "Нечего набирать");
            return;
        }
        if let Some(body) = self.actor(target).body.as_mut() {
            body.modify_blood(-take);
        }
        if let Some(sol) = self.solution_mut(item) {
            sol.add("Blood", take);
        }
        self.popup(actor, format!("Набрано {take:.0} ед. крови"));
        if let Some(at) = ctx.position_of(actor) {
            self.sound("Items/hypospray.ogg", at);
        }
        self.after_health_change(ctx, target);
        self.publish(ctx, actor);
    }

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
    pub(crate) fn after_health_change(&mut self, ctx: &mut HookContext<'_>, who: NetworkEntityId) {
        let Some(a) = self.actors.get(&who) else {
            return;
        };
        let (now, before) = (a.health.state(), a.mob_state);
        let legs = a.health.part_disabled("LeftLeg") && a.health.part_disabled("RightLeg");
        let died = now == MobState::Dead && before != MobState::Dead;
        let resisting = a.body.as_ref().is_some_and(Body::resisting);
        let slipped = a.slip_until > self.clock;
        let in_cryo = a.cryo.is_some();
        let status_down = a.body.as_ref().is_some_and(Body::disabled_by_status);
        let freeze = (!now.can_act() || legs || resisting || slipped || in_cryo || status_down)
            && !a.ghost
            && !died;
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
                self.actor(who).corpse = Some(id);
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
        if let Action::Defib { target, .. } = &action {
            let target = *target;
            self.defib_begin_feedback(ctx, actor, target);
        }
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
            Action::Inject { hand, target } => {
                let alive = ctx.position_of(*target).is_some()
                    && self.actors.get(target).is_none_or(|t| t.body.is_some());
                let ok = a.hands.get(*hand).is_some_and(|i| {
                    self.injector_of(i).is_some_and(|(m, _)| match m.behavior {
                        // Набор крови: в шприце есть место, у цели есть кровь.
                        Behavior::Draw => {
                            self.solution_room(i) > 0.0
                                && self
                                    .actors
                                    .get(target)
                                    .and_then(|t| t.body.as_ref())
                                    .is_some_and(|b| b.blood_volume() > 0.0)
                        }
                        _ => self.solution_volume(i) > 0.0,
                    })
                });
                alive && ok && self.in_reach(ctx, actor, *target)
            }
            Action::Mix {
                hand,
                target,
                shake,
            } => self.mix_can(ctx, actor, *hand, *target, *shake),
            Action::CryoPut { pod, target } => {
                self.cryo.get(pod).is_some_and(|c| c.patient.is_none())
                    && self.cryo_pos(ctx, *pod).is_some_and(|p| {
                        ctx.position_of(actor)
                            .is_some_and(|f| (p - f).length() <= crate::cryo::REACH_M)
                            && ctx
                                .position_of(*target)
                                .is_some_and(|t| (p - t).length() <= crate::cryo::REACH_M)
                    })
            }
            Action::Defib { hand, target } => {
                let Some(held) = a.hands.get(*hand) else {
                    return false;
                };
                let (Some(spec), Some(st)) = (
                    self.defibs.get(&held.prototype),
                    self.defib_state.get(&held.id),
                ) else {
                    return false;
                };
                st.on
                    && st.cooldown <= 0.0
                    && spec.can_fire(st)
                    && self.in_reach_body(ctx, actor, *target)
            }
            Action::Scan { hand, target } => {
                ctx.position_of(*target).is_some()
                    && a.hands
                        .get(*hand)
                        .is_some_and(|i| self.analyzers.contains(&i.prototype))
                    && self.in_reach(ctx, actor, *target)
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
            Action::Mix { hand, shake, .. } => self.mix_delay(actor, *hand, *shake),
            Action::CryoPut { .. } => self
                .cryo_specs
                .values()
                .next()
                .map_or(2.0, |s| s.entry_delay),
            Action::Defib { hand, .. } => a
                .hands
                .get(*hand)
                .and_then(|i| self.defibs.get(&i.prototype))
                .map_or(0.0, |d| d.do_after),
            Action::Inject { hand, target } => a
                .hands
                .get(*hand)
                .and_then(|i| {
                    let (mode, amount) = self.injector_of(i)?;
                    let amount = amount
                        .unwrap_or_else(|| self.solution_volume(i))
                        .min(self.solution_volume(i));
                    let mut t = mode.mob_time
                        + mode.delay_per_volume * (amount - mode.ignore_delay_for_volume).max(0.0);
                    if *target == actor {
                        t *= mode.self_modifier;
                    } else if self
                        .actors
                        .get(target)
                        .is_some_and(|x| x.health.state() != MobState::Alive)
                    {
                        t *= mode.downed_modifier;
                    }
                    Some(t)
                })
                .unwrap_or(0.0),
            Action::Scan { .. } => crate::analyzer::SCAN_DELAY_S,
            Action::Pry { .. } => PRY_TIME_S,
        }
    }

    fn run(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId, action: Action) {
        match action {
            Action::Equip { hand, slot } => self.equip(ctx, actor, hand, &slot),
            Action::Unequip { slot, hand } => self.unequip(ctx, actor, &slot, hand),
            Action::Pry { element, .. } => ctx.request_door(element, DoorAction::Open, true),
            Action::Heal { hand, target } => self.heal(ctx, actor, hand, target),
            Action::Inject { hand, target } => self.inject_body(ctx, actor, hand, target),
            Action::Defib { hand, target } => self.zap(ctx, actor, hand, target),
            Action::CryoPut { pod, target } => self.cryo_put(ctx, pod, target),
            Action::Mix {
                hand,
                target,
                shake,
            } => self.mix_run(ctx, actor, hand, target, shake),
            Action::Scan { target, .. } => {
                if let Some(at) = ctx.position_of(actor) {
                    self.sound("Items/Medical/healthscanner.ogg", at);
                }
                self.scans.insert(actor, target);
                self.scan_timer = 1.0;
            }
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
            let (mob, barotrauma, damage_now) = {
                let a = &self.actors[&id];
                (
                    a.health.state(),
                    a.health.damage_of("Blunt") + a.health.damage_of("Heat"),
                    a.health.view().damage,
                )
            };
            body.observe_damage(&damage_now);
            let a_ghost = self.actors[&id].ghost;
            let protection: f32 = self.actors[&id]
                .inventory
                .worn()
                .filter_map(|i| self.heat_protection.get(&i.prototype))
                .product();
            body.set_heat_protection(protection);
            let cryo_key = self.actors[&id].cryo.map(|pod| self.pipe_keys.key_of(pod));
            let outcome = if let Some(key) = cryo_key
                && let Some(air) = self.atmos.pipes_mut().cryo_air_mut(key)
            {
                // Пациент капсулы дышит её воздухом и обменивается с ним теплом.
                let mut env = crate::cryo::CryoEnv { air };
                body.tick(dt, &mut env, mob, barotrauma)
            } else {
                let mut env = ActorEnv {
                    grid: Some(&mut self.atmos),
                    cell,
                };
                body.tick(dt, &mut env, mob, barotrauma)
            };
            let vitals = body.vitals();
            // Шагом идут медленнее: 2,5 против 4,5 у бега (`WalkSpeed` / `SprintSpeed`).
            let walk = if self.actors.get(&id).is_some_and(|a| a.walking) {
                WALK_FACTOR
            } else {
                1.0
            };
            let speed = if a_ghost { 1.0 } else { vitals.speed * walk };
            if (speed - self.speed_sent.get(&id).copied().unwrap_or(1.0)).abs() > 1e-4 {
                self.speed_sent.insert(id, speed);
                ctx.set_speed(id, speed);
            }
            if outcome.blood_spilled > 0.0 {
                let mut blood = Solution::new(1.0e9);
                blood.add("Blood", outcome.blood_spilled);
                self.puddles.spill(self.atmos.layout(), cell, &blood);
            }
            for (text, _large) in &outcome.popups {
                self.popup(id, text.clone());
            }
            if let Some(at) = ctx.position_of(id) {
                for path in &outcome.sounds {
                    self.sound(path, at);
                }
            }
            for sol in &outcome.vomit {
                self.puddles.spill(self.atmos.layout(), cell, sol);
            }
            let a = self.actor(id);
            a.health.set_modifiers(body.damage_modifiers());
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
        self.tick_lobby(ctx);
        self.tick_round(ctx);
        self.tick_chat(ctx);
        self.place_joiners(ctx);
        self.tick_disposal(ctx, dt);
        self.tick_conveyors(ctx, dt);
        self.tick_vending(ctx);
        self.tick_lockers(ctx);
        self.tick_id_consoles(ctx);
        self.tick_dispensers(ctx);
        self.tick_scans(ctx);
        let dt = self.last_dt;
        self.tick_defibs(dt);
        self.tick_lab(ctx, dt);
        self.tick_cryo(ctx);
        self.publish_chemhand(ctx);
        self.publish_jobs(ctx);
        self.resolve_spawners(ctx);
        self.flush_fx(ctx);
        self.tick_alarms(ctx, dt);
        self.tick_firelocks(ctx, dt);
        self.tick_power(dt);
        self.tick_power_monitors(ctx);
        self.publish_atmos_alerts(ctx);
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
                        // Настройки с карты: целевая температура термомашины и фильтруемый газ.
                        match &mut part.device {
                            Some(crate::piping::DeviceSpec::Thermo { target, .. }) => {
                                if let Some(t) = o
                                    .metadata
                                    .get("thermo_target")
                                    .and_then(|v| v.parse::<f32>().ok())
                                {
                                    *target = t;
                                }
                            }
                            Some(crate::piping::DeviceSpec::Filter { gas, .. }) => {
                                if let Some(g) = o
                                    .metadata
                                    .get("filter_gas")
                                    .and_then(|v| crate::piping::gas_named(v))
                                {
                                    *gas = Some(g);
                                }
                            }
                            _ => {}
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
        self.power_suppliers = built.suppliers;
        self.power_cables = built.cables;
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
        let at = Self::cell_point(ctx, cell);
        self.sound("Machines/disposalflush.ogg", at);
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

    /// Уровни доступа удостоверения или КПК (`FindAccessTagsItem`): выданные при выборе должности,
    /// иначе по прототипу карты (`Access.tags` и должность `PresetIdCard`).
    fn item_access(&self, item: &HeldItem) -> BTreeSet<String> {
        if let Some(t) = self.card_access.get(&item.id) {
            return t.clone();
        }
        let card = self
            .pda_cards
            .get(&item.prototype)
            .and_then(|n| self.proto_by_name.get(n))
            .unwrap_or(&item.prototype);
        let Some(spec) = self.id_specs.get(card) else {
            return BTreeSet::new();
        };
        let mut tags: BTreeSet<String> = spec.access.iter().cloned().collect();
        if let Some(job) = spec
            .job
            .as_deref()
            .and_then(|j| self.jobs.iter().find(|x| x.id == j))
        {
            tags.extend(job.access.iter().cloned());
        }
        tags
    }

    /// Всё, что даёт доступ персонажу: удостоверения и КПК в руках и на теле.
    pub(crate) fn user_access(&self, actor: NetworkEntityId) -> BTreeSet<String> {
        let Some(a) = self.actors.get(&actor) else {
            return BTreeSet::new();
        };
        a.hands
            .held()
            .chain(a.inventory.worn())
            .flat_map(|i| self.item_access(i))
            .collect()
    }

    /// Пускает ли считыватель: пустой список — всех, иначе нужен хотя бы один список целиком.
    pub(crate) fn access_ok(lists: &[Vec<String>], tags: &BTreeSet<String>) -> bool {
        lists.is_empty() || lists.iter().any(|l| l.iter().all(|t| tags.contains(t)))
    }

    /// Дверь открывается этим персонажем? Ограничения берутся из зон карты `door_access`.
    pub(crate) fn door_allowed(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        element: ObjectId,
    ) -> bool {
        let Some(e) = ctx.world.element(element) else {
            return true;
        };
        let Placement::Edge(g) = e.placement else {
            return true;
        };
        let key = format!(
            "{},{},{}",
            g.x,
            g.z,
            if matches!(g.dir, astrocore_math::EdgeDir::AlongX) {
                "X"
            } else {
                "Z"
            }
        );
        let Some(lists) = ctx.world.authored().find_map(|o| {
            (o.metadata.get("door_edge") == Some(&key)).then(|| o.metadata.get("door_access"))?
        }) else {
            return true;
        };
        let lists: Vec<Vec<String>> = lists
            .split(';')
            .map(|l| l.split(',').map(String::from).collect())
            .collect();
        Self::access_ok(&lists, &self.user_access(actor))
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
        self.round.crew.insert(actor, job.to_string());
        // Слоты по порядку шаблона: сначала то, от чего зависят другие (комбинезон, верхняя одежда).
        for slot in crate::inventory::human_template() {
            let Some(name) = spec.equipment.get(&slot.name) else {
                continue;
            };
            if let Some(item) = self.create_item(name) {
                self.actor(actor).inventory.equip_forced(&slot.name, item);
            }
        }
        // Удостоверение в слоте `id` получает доступ должности (`SetAccessToJob`).
        if let Some(card) = self.actors.get(&actor).and_then(|a| a.inventory.get("id")) {
            self.card_access
                .insert(card.id, spec.access.iter().cloned().collect());
            self.card_names
                .insert(card.id, ("Новичок".to_string(), spec.title.clone()));
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

    fn tick_lockers(&mut self, ctx: &mut HookContext<'_>) {
        if self.storage_specs.is_empty() {
            return;
        }
        self.locker_timer += self.last_dt;
        if self.locker_timer < 0.5 && !self.lockers.is_empty() {
            return;
        }
        self.locker_timer = 0.0;
        self.sync_lockers(ctx);
    }

    /// Режим и количество шприца (по умолчанию — из прототипа).
    /// Свободное место в растворе предмета.
    pub(crate) fn solution_room(&self, item: &HeldItem) -> f32 {
        self.solutions
            .get(&item.id)
            .or_else(|| self.solution_templates.get(&item.prototype))
            .map_or(0.0, Solution::available)
    }

    pub(crate) fn injector_of(&self, item: &HeldItem) -> Option<(InjectorMode, Option<f32>)> {
        let spec = self.injectors.get(&item.prototype)?;
        let (active, amount) = self
            .injector_state
            .get(&item.id)
            .copied()
            .unwrap_or((spec.active, spec.amount.unwrap_or(-1.0)));
        let mode = spec.modes.get(active)?.clone();
        Some((mode, (amount > 0.0).then_some(amount)))
    }

    /// Перелить `amount` из `from` в `to`: сколько есть и сколько влезает; в приёмнике идут реакции.
    pub(crate) fn pour(&mut self, from: &HeldItem, to: &HeldItem, amount: f32) -> f32 {
        let room = self.solution_mut(to).map_or(0.0, |s| s.available());
        let Some(src) = self.solution_mut(from) else {
            return 0.0;
        };
        let take = amount.min(src.volume()).min(room);
        if take <= 0.0 {
            return 0.0;
        }
        let part = src.split(take);
        let chem = self.chem.clone();
        if let Some(dst) = self.solution_mut(to) {
            dst.merge(&part);
            chem.react(dst);
        }
        take
    }

    /// Предмет из руки на ёмкость в мире: перелив (`SolutionTransferSystem.OnAfterInteract`) или
    /// шприц на контейнере (`InjectorSystem`). Сначала то, что держим, наливается в цель; иначе берётся из цели.
    pub(crate) fn chem_use(&mut self, used: &HeldItem, target: &HeldItem) -> bool {
        if let Some(default) = self.transfers.get(&used.prototype).copied() {
            let amount = self
                .transfer_amount
                .get(&used.id)
                .copied()
                .unwrap_or(default);
            let send = self.drainables.contains(&used.prototype)
                && self.refillables.contains(&target.prototype);
            let recv = self.refillables.contains(&used.prototype)
                && self.drainables.contains(&target.prototype);
            return if send {
                self.pour(used, target, amount) > 0.0 || true
            } else if recv {
                self.pour(target, used, amount) > 0.0 || true
            } else {
                false
            };
        }
        if let Some((mode, amount)) = self.injector_of(used) {
            let amount = amount.unwrap_or_else(|| self.solution_volume(used));
            return match mode.behavior {
                Behavior::Inject if self.refillables.contains(&target.prototype) => {
                    self.pour(used, target, amount);
                    true
                }
                Behavior::Draw | Behavior::Dynamic
                    if self.drainables.contains(&target.prototype) =>
                {
                    self.pour(target, used, amount);
                    true
                }
                _ => false,
            };
        }
        false
    }

    /// Что можно выбрать у предмета в руке: список количеств и текущее.
    fn chemhand_of(&self, item: &HeldItem, hand: usize) -> Option<crate::chemhand::ChemHandView> {
        if let Some(default) = self.transfers.get(&item.prototype) {
            let max = self
                .transfer_max
                .get(&item.prototype)
                .copied()
                .unwrap_or(1000.0);
            let amounts: Vec<f32> = crate::chemhand::TRANSFER_AMOUNTS
                .iter()
                .copied()
                .filter(|a| *a <= max)
                .collect();
            return Some(crate::chemhand::ChemHandView {
                hand: hand as u8,
                mode: String::new(),
                amount: self
                    .transfer_amount
                    .get(&item.id)
                    .copied()
                    .unwrap_or(*default),
                amounts,
            });
        }
        let (mode, amount) = self.injector_of(item)?;
        Some(crate::chemhand::ChemHandView {
            hand: hand as u8,
            mode: match mode.behavior {
                Behavior::Inject => "Inject",
                Behavior::Draw => "Draw",
                Behavior::Dynamic => "Dynamic",
            }
            .to_string(),
            amount: amount.unwrap_or(-1.0),
            amounts: mode.amounts,
        })
    }

    /// Выбор количества: `idx` — номер в списке, `255` — следующее по кругу.
    pub(crate) fn set_chem_amount(&mut self, user: NetworkEntityId, hand: usize, idx: u8) {
        let Some(item) = self
            .actors
            .get(&user)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return;
        };
        let Some(view) = self.chemhand_of(&item, hand) else {
            return;
        };
        if view.amounts.is_empty() {
            return;
        }
        let i = if idx == 255 {
            view.amounts
                .iter()
                .position(|a| (*a - view.amount).abs() < 1e-3)
                .map_or(0, |p| (p + 1) % view.amounts.len())
        } else {
            usize::from(idx).min(view.amounts.len() - 1)
        };
        let amount = view.amounts[i];
        if self.transfers.contains_key(&item.prototype) {
            self.transfer_amount.insert(item.id, amount);
        } else if let Some(spec) = self.injectors.get(&item.prototype) {
            let active = self
                .injector_state
                .get(&item.id)
                .map_or(spec.active, |s| s.0);
            self.injector_state.insert(item.id, (active, amount));
        }
        self.popup(user, format!("Количество: {amount:.0} ед."));
    }

    /// Рассылка владельцу: что настраивается у предмета в активной руке.
    fn publish_chemhand(&mut self, ctx: &mut HookContext<'_>) {
        for p in ctx.players().collect::<Vec<_>>() {
            let view = self.actors.get(&p).and_then(|a| {
                let hand = a.hands.active();
                let item = a.hands.get(hand)?;
                self.chemhand_of(item, hand)
            });
            let bytes = crate::chemhand::encode_chemhand(view.as_ref());
            if self.chemhand_sent.get(&p) != Some(&bytes) {
                ctx.set_component(p, crate::chemhand::CHEMHAND_COMPONENT, bytes.clone());
                self.chemhand_sent.insert(p, bytes);
            }
        }
    }

    /// Переключить режим шприца (по кругу) — «использовать в руке».
    pub(crate) fn toggle_injector(&mut self, item: &HeldItem) -> bool {
        let Some(spec) = self.injectors.get(&item.prototype) else {
            return false;
        };
        if spec.modes.len() < 2 {
            return true;
        }
        let (active, amount) = self
            .injector_state
            .get(&item.id)
            .copied()
            .unwrap_or((spec.active, spec.amount.unwrap_or(-1.0)));
        let next = (active + 1) % spec.modes.len();
        // Количество подгоняется под допустимые значения нового режима.
        let amounts = &spec.modes[next].amounts;
        let amount = if amounts.is_empty() || amount <= 0.0 || amounts.contains(&amount) {
            amount
        } else {
            amounts[0]
        };
        self.injector_state.insert(item.id, (next, amount));
        true
    }

    /// Окна анализатора: раз в секунду данные цели, пока она рядом.
    fn tick_scans(&mut self, ctx: &mut HookContext<'_>) {
        if self.analyzers.is_empty() {
            return;
        }
        self.scan_timer += self.last_dt;
        if self.scan_timer < 1.0 {
            return;
        }
        self.scan_timer = 0.0;
        let pairs: Vec<(NetworkEntityId, NetworkEntityId)> =
            self.scans.iter().map(|(a, b)| (*a, *b)).collect();
        for (user, target) in pairs {
            let (Some(up), Some(tp)) = (ctx.position_of(user), ctx.position_of(target)) else {
                self.scans.remove(&user);
                continue;
            };
            let in_range = (up - tp).length() <= crate::analyzer::SCAN_RANGE_M;
            let Some(t) = self.actors.get(&target) else {
                continue;
            };
            let view = t.body.as_ref().map(|b| {
                let v = b.vitals();
                let damage = t.health.view().damage;
                let groups: Vec<crate::analyzer::DamageGroup> = crate::health::GROUPS
                    .iter()
                    .filter_map(|(g, kinds)| {
                        let items: Vec<(String, f32)> = kinds
                            .iter()
                            .filter_map(|k| {
                                let v = damage.get(*k).copied().unwrap_or(0.0);
                                (v > 0.0).then(|| (k.to_string(), v))
                            })
                            .collect();
                        let total: f32 = items.iter().map(|i| i.1).sum();
                        (total > 0.0).then(|| (g.to_string(), total, items))
                    })
                    .collect();
                crate::analyzer::ScanView {
                    active: in_range,
                    target: format!("Игрок {}", target.0),
                    state: format!("{:?}", t.health.state()),
                    temperature_k: v.temperature,
                    blood: v.blood,
                    bleeding: v.bleed > 0.0,
                    total_damage: damage.values().sum(),
                    groups,
                }
            });
            let bytes = crate::analyzer::encode_scan(view.as_ref());
            if self.scan_sent.get(&user) != Some(&bytes) {
                ctx.set_component(user, crate::analyzer::SCAN_COMPONENT, bytes.clone());
                self.scan_sent.insert(user, bytes);
            }
        }
    }

    /// Центр объекта мира.
    fn center_of(ctx: &HookContext<'_>, id: ObjectId) -> Option<Vec3> {
        let o = ctx.world.authored_object(id)?;
        Some((o.min() + o.max()) * 0.5)
    }

    /// Подсказка игроку.
    pub(crate) fn popup(&mut self, to: NetworkEntityId, text: impl Into<String>) {
        self.fx_popups.push((to, text.into()));
    }

    /// Звук в точке мира (путь внутри `Audio`).
    pub(crate) fn sound(&mut self, path: &str, at: Vec3) {
        self.sound_volume(path, at, 0.0);
    }

    pub(crate) fn sound_volume(&mut self, path: &str, at: Vec3, volume_db: f32) {
        self.fx_sounds.push((path.to_string(), at, volume_db));
    }

    /// В конце тика: каждому игроку — его подсказки и звуки поблизости.
    fn flush_fx(&mut self, ctx: &mut HookContext<'_>) {
        if self.fx_popups.is_empty() && self.fx_sounds.is_empty() {
            return;
        }
        let popups = std::mem::take(&mut self.fx_popups);
        let sounds = std::mem::take(&mut self.fx_sounds);
        for p in ctx.players().collect::<Vec<_>>() {
            let Some(pos) = ctx.position_of(p) else {
                continue;
            };
            let mine: Vec<String> = popups
                .iter()
                .filter(|(t, _)| *t == p)
                .map(|(_, s)| s.clone())
                .collect();
            let near: Vec<crate::fx::FxSound> = sounds
                .iter()
                .filter(|(_, at, _)| (*at - pos).length() <= crate::fx::HEARING_M)
                .map(|(path, at, volume_db)| crate::fx::FxSound {
                    path: path.clone(),
                    at: at.to_array(),
                    volume_db: *volume_db,
                })
                .collect();
            if mine.is_empty() && near.is_empty() {
                continue;
            }
            let seq = self.fx_seq.entry(p).or_insert(0);
            *seq += 1;
            let batch = crate::fx::FxBatch {
                seq: *seq,
                popups: mine,
                sounds: near,
            };
            ctx.set_component(p, crate::fx::FX_COMPONENT, crate::fx::encode_fx(&batch));
        }
    }

    /// Раздатчики карты получают банки при первом появлении (`EntityTableContainerFill`).
    fn sync_dispensers(&mut self, ctx: &HookContext<'_>) {
        let mut seen = BTreeSet::new();
        let mut fresh: Vec<(ObjectId, PrototypeId)> = Vec::new();
        for o in ctx.world.authored() {
            if let AuthoredKind::Placement { prototype } = &o.kind
                && self.dispenser_tables.contains_key(prototype)
            {
                seen.insert(o.id);
                if !self.dispensers.contains_key(&o.id) {
                    fresh.push((o.id, prototype.clone()));
                }
            }
        }
        self.dispensers.retain(|id, _| seen.contains(id));
        for (id, proto) in fresh {
            let names = self
                .dispenser_tables
                .get(&proto)
                .map(|t| t.roll_all(&self.spawner_tables, &mut self.spawn_rng))
                .unwrap_or_default();
            let jugs = names.iter().filter_map(|n| self.create_item(n)).collect();
            self.dispensers.insert(
                id,
                Dispenser {
                    jugs,
                    beaker: None,
                    amount: crate::dispenser::DEFAULT_AMOUNT,
                },
            );
        }
    }

    /// Окно раздатчика открывает пустая рука при наличии тока.
    pub(crate) fn open_dispenser(&mut self, user: NetworkEntityId, id: ObjectId) -> bool {
        if !self.dispensers.contains_key(&id) {
            return false;
        }
        if self.power_required && !self.powered_map().get(&id).copied().unwrap_or(false) {
            return true;
        }
        self.dispenser_open.insert(user, id);
        true
    }

    /// Мензурка из руки в слот (`beakerSlot`: только `FitsInDispenser`).
    pub(crate) fn insert_beaker(
        &mut self,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        id: ObjectId,
        hand: usize,
    ) -> bool {
        let Some(item) = self
            .actors
            .get(&user)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return false;
        };
        if !self.fits_in_dispenser.contains(&item.prototype) {
            return false;
        }
        let Some(d) = self.dispensers.get_mut(&id) else {
            return false;
        };
        if d.beaker.is_some() {
            return false;
        }
        d.beaker = Some(item);
        let _ = self.actor(user).hands.drop_hand(hand);
        self.publish(ctx, user);
        true
    }

    /// Нажатие на банку: перелить выбранное количество в мензурку.
    pub(crate) fn dispense(&mut self, user: NetworkEntityId, jug: usize) {
        let Some(id) = self.dispenser_open.get(&user).copied() else {
            return;
        };
        if self.power_required && !self.powered_map().get(&id).copied().unwrap_or(false) {
            return;
        }
        let Some(d) = self.dispensers.get(&id) else {
            return;
        };
        let (Some(beaker), Some(src)) = (d.beaker.clone(), d.jugs.get(jug).cloned()) else {
            return;
        };
        let amount = d.amount;
        let moved = self.pour(&src, &beaker, amount);
        if moved > 0.0 {
            self.popup(user, format!("Налито {moved:.0} ед."));
        }
    }

    /// Окна раздатчиков у игроков (по изменению).
    fn tick_dispensers(&mut self, ctx: &mut HookContext<'_>) {
        if self.dispenser_tables.is_empty() {
            return;
        }
        self.sync_dispensers(ctx);
        let mut sends: Vec<(NetworkEntityId, Vec<u8>)> = Vec::new();
        for p in ctx.players().collect::<Vec<_>>() {
            let view = self.dispenser_open.get(&p).copied().and_then(|id| {
                let o = ctx.world.authored_object(id)?;
                let feet = ctx.position_of(p)?;
                if (((o.min() + o.max()) * 0.5) - feet).length() > self.range_m + 1.0 {
                    return None;
                }
                let d = self.dispensers.get(&id)?;
                let info = |item: &HeldItem| -> (f32, Vec<(String, f32)>) {
                    let s = self
                        .solutions
                        .get(&item.id)
                        .or_else(|| self.solution_templates.get(&item.prototype));
                    s.map_or((0.0, Vec::new()), |s| {
                        (
                            s.volume(),
                            s.contents.iter().map(|(k, v)| (k.clone(), *v)).collect(),
                        )
                    })
                };
                Some(crate::dispenser::DispenserView {
                    machine: id,
                    beaker: d.beaker.as_ref().map(|b| {
                        let (volume, reagents) = info(b);
                        crate::dispenser::BeakerInfo {
                            name: b.prototype.to_string(),
                            volume,
                            max: self
                                .solutions
                                .get(&b.id)
                                .or_else(|| self.solution_templates.get(&b.prototype))
                                .map_or(0.0, |s| s.max_volume),
                            reagents,
                            temperature: self
                                .solutions
                                .get(&b.id)
                                .or_else(|| self.solution_templates.get(&b.prototype))
                                .map_or(0.0, |s| s.temperature),
                        }
                    }),
                    jugs: d
                        .jugs
                        .iter()
                        .enumerate()
                        .map(|(i, j)| {
                            let (quantity, reagents) = info(j);
                            let main = reagents
                                .iter()
                                .max_by(|a, b| a.1.total_cmp(&b.1))
                                .map(|r| r.0.clone());
                            let def = main.as_ref().and_then(|m| self.chem.reagents.get(m));
                            crate::dispenser::JugInfo {
                                index: i as u8,
                                label: def
                                    .map(|r| r.name.clone())
                                    .or(main)
                                    .unwrap_or_else(|| j.prototype.to_string()),
                                quantity,
                                color: def
                                    .map_or_else(|| "#ffffff".to_string(), |r| r.color.clone()),
                            }
                        })
                        .collect(),
                    amount: d.amount,
                    amounts: crate::dispenser::AMOUNTS.to_vec(),
                })
            });
            if view.is_none() {
                self.dispenser_open.remove(&p);
            }
            let bytes = crate::dispenser::encode_dispenser(view.as_ref());
            if self.dispenser_sent.get(&p) != Some(&bytes) {
                sends.push((p, bytes));
            }
        }
        for (p, bytes) in sends {
            ctx.set_component(p, crate::dispenser::DISPENSER_COMPONENT, bytes.clone());
            self.dispenser_sent.insert(p, bytes);
        }
    }

    /// Это удостоверение или КПК (их слот принимает консоль)?
    fn is_id_item(&self, item: &HeldItem) -> bool {
        self.id_specs.contains_key(&item.prototype) || self.pda_cards.contains_key(&item.prototype)
    }

    fn card_title(&self, item: &HeldItem) -> String {
        self.card_names
            .get(&item.id)
            .map(|n| n.0.clone())
            .unwrap_or_else(|| item.prototype.to_string())
    }

    /// Окно консоли открывается пустой рукой при наличии тока.
    pub(crate) fn open_id_console(&mut self, user: NetworkEntityId, id: ObjectId) -> bool {
        if self.power_required && !self.powered_map().get(&id).copied().unwrap_or(false) {
            return true;
        }
        self.consoles.entry(id).or_default();
        self.console_open.insert(user, id);
        true
    }

    /// Вставить карту из руки в слот (`0` — с правами, `1` — целевая); без номера — в первый свободный.
    pub(crate) fn insert_id(
        &mut self,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        console: ObjectId,
        slot: Option<usize>,
        hand: usize,
    ) -> bool {
        let Some(item) = self
            .actors
            .get(&user)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return false;
        };
        if !self.is_id_item(&item) {
            return false;
        }
        let slots = self.consoles.entry(console).or_default();
        let target_slot = match slot {
            Some(s) => s,
            None if slots.privileged.is_none() => 0,
            None => 1,
        };
        let place = if target_slot == 0 {
            &mut slots.privileged
        } else {
            &mut slots.target
        };
        if place.is_some() {
            return false;
        }
        *place = Some(item);
        let _ = self.actor(user).hands.drop_hand(hand);
        self.publish(ctx, user);
        true
    }

    /// Вынуть карту из слота в руку (или на пол у консоли).
    pub(crate) fn eject_id(
        &mut self,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        console: ObjectId,
        slot: usize,
    ) {
        let Some(slots) = self.consoles.get_mut(&console) else {
            return;
        };
        let Some(item) = (if slot == 0 {
            slots.privileged.take()
        } else {
            slots.target.take()
        }) else {
            return;
        };
        if let Err(_) = self.actor(user).hands.pickup_any(item.clone())
            && let Some(feet) = ctx.position_of(user)
        {
            Self::place(ctx, &item, feet);
        }
        self.publish(ctx, user);
    }

    /// `TryWriteToTargetId`: имя, должность и уровни доступа целевой карты.
    pub(crate) fn write_id(&mut self, user: NetworkEntityId, req: &crate::idconsole::WriteRequest) {
        let Some(console) = self.console_open.get(&user).copied() else {
            return;
        };
        let Some(slots) = self.consoles.get(&console) else {
            return;
        };
        let (Some(privileged), Some(target)) = (slots.privileged.clone(), slots.target.clone())
        else {
            return;
        };
        let levels = self
            .id_console_proto(console)
            .and_then(|p| self.id_console_levels.get(&p).cloned())
            .unwrap_or_default();
        let name = crate::idconsole::clip(&req.name, crate::idconsole::MAX_NAME_LEN);
        let title = crate::idconsole::clip(&req.title, crate::idconsole::MAX_JOB_LEN);
        self.card_names.insert(target.id, (name, title));
        self.popup(user, "Карта записана");
        let old = self.item_access(&target);
        let perms = self.item_access(&privileged);
        if let Some(new) = crate::idconsole::rewrite_access(&old, &req.access, &levels, &perms) {
            self.card_access.insert(target.id, new);
        }
    }

    fn id_console_proto(&self, console: ObjectId) -> Option<PrototypeId> {
        self.console_protos.get(&console).cloned()
    }

    /// Окна консолей у игроков (по изменению).
    fn tick_id_consoles(&mut self, ctx: &mut HookContext<'_>) {
        if self.id_console_levels.is_empty() {
            return;
        }
        // Прототипы консолей на карте: нужны для списка уровней.
        for o in ctx.world.authored() {
            if let AuthoredKind::Placement { prototype } = &o.kind
                && self.id_console_levels.contains_key(prototype)
            {
                self.console_protos.insert(o.id, prototype.clone());
            }
        }
        let mut sends: Vec<(NetworkEntityId, Vec<u8>)> = Vec::new();
        for p in ctx.players().collect::<Vec<_>>() {
            let view = self.console_open.get(&p).copied().and_then(|id| {
                let o = ctx.world.authored_object(id)?;
                let feet = ctx.position_of(p)?;
                if (((o.min() + o.max()) * 0.5) - feet).length() > self.range_m + 1.0 {
                    return None;
                }
                let slots = self.consoles.get(&id)?;
                let levels = self
                    .id_console_levels
                    .get(self.console_protos.get(&id)?)?
                    .clone();
                Some(crate::idconsole::IdConsoleView {
                    console: id,
                    privileged: slots.privileged.as_ref().map(|i| self.card_title(i)),
                    authorized: slots.privileged.is_some(),
                    target: slots.target.as_ref().map(|t| {
                        let (full, job) = self.card_names.get(&t.id).cloned().unwrap_or_default();
                        crate::idconsole::TargetView {
                            name: t.prototype.to_string(),
                            full_name: full,
                            job_title: job,
                            access: self.item_access(t).into_iter().collect(),
                        }
                    }),
                    allowed: slots
                        .privileged
                        .as_ref()
                        .map(|i| self.item_access(i).into_iter().collect())
                        .unwrap_or_default(),
                    levels,
                })
            });
            if view.is_none() {
                self.console_open.remove(&p);
            }
            let bytes = crate::idconsole::encode_view(view.as_ref());
            if self.console_sent.get(&p) != Some(&bytes) {
                sends.push((p, bytes));
            }
        }
        for (p, bytes) in sends {
            ctx.set_component(p, crate::idconsole::IDCONSOLE_COMPONENT, bytes.clone());
            self.console_sent.insert(p, bytes);
        }
    }

    /// Шкафы и ящики карты получают состояние и наполнение при первом появлении (`MapInit`).
    fn sync_lockers(&mut self, ctx: &mut HookContext<'_>) {
        let mut new: Vec<(ObjectId, PrototypeId, bool, Option<bool>, Vec3)> = Vec::new();
        let mut seen = BTreeSet::new();
        for o in ctx.world.authored() {
            let AuthoredKind::Placement { prototype } = &o.kind else {
                continue;
            };
            let (closed, open) = if self.storage_specs.contains_key(prototype) {
                (prototype.clone(), false)
            } else if let Some(c) = self.storage_open_of.get(prototype) {
                (c.clone(), true)
            } else {
                continue;
            };
            seen.insert(o.id);
            if self.lockers.contains_key(&o.id) {
                continue;
            }
            let locked = o.metadata.get("locked").map(|v| v == "1");
            let c = (o.min() + o.max()) * 0.5;
            new.push((o.id, closed, open, locked, Vec3::new(c.x, o.min().y, c.z)));
        }
        self.lockers.retain(|id, _| seen.contains(id));
        for (id, closed, open, locked, at) in new {
            let Some(spec) = self.storage_specs.get(&closed).cloned() else {
                continue;
            };
            let mut contents = Vec::new();
            if let Some(table) = &spec.fill {
                for name in table.roll_all(&self.spawner_tables, &mut self.spawn_rng) {
                    if let Some(item) = self.create_item(&name) {
                        contents.push(item);
                    }
                }
            }
            let state_locked =
                !open && spec.lock.is_some() && locked.or(spec.lock).unwrap_or(false);
            let mut locker = Locker {
                closed_proto: closed,
                open,
                locked: state_locked,
                contents,
            };
            // Открытый на карте шкаф сразу отдаёт содержимое.
            if open {
                let items = std::mem::take(&mut locker.contents);
                self.scatter(ctx, &items, at);
            }
            self.lockers.insert(id, locker);
        }
    }

    /// Разложить предметы вокруг точки (открытый шкаф).
    fn scatter(&mut self, ctx: &mut HookContext<'_>, items: &[HeldItem], at: Vec3) {
        for (i, item) in items.iter().enumerate() {
            let a = i as f32 * 2.399_963; // золотой угол: равномерно по кругу
            let r = 0.15 + 0.05 * (i % 4) as f32;
            Self::place(ctx, item, at + Vec3::new(a.cos() * r, 0.0, a.sin() * r));
        }
    }

    /// Заменить вид шкафа (открытый ↔ закрытый), сохранив идентификатор и метаданные.
    fn swap_locker_proto(ctx: &mut HookContext<'_>, id: ObjectId, to: &PrototypeId) {
        let Some(mut obj) = ctx.world.authored_object(id).cloned() else {
            return;
        };
        obj.kind = AuthoredKind::Placement {
            prototype: to.clone(),
        };
        ctx.request(WorldCommand::RemoveAuthored { id });
        ctx.request(WorldCommand::PlaceAuthored(obj));
    }

    /// `ToggleOpen`: закрытый — открыть (содержимое на пол), открытый — закрыть (предметы рядом внутрь).
    pub(crate) fn toggle_locker(&mut self, ctx: &mut HookContext<'_>, id: ObjectId) -> bool {
        let Some(l) = self.lockers.get(&id).cloned() else {
            return false;
        };
        let Some(spec) = self.storage_specs.get(&l.closed_proto).cloned() else {
            return false;
        };
        let Some(obj) = ctx.world.authored_object(id) else {
            return false;
        };
        let c = (obj.min() + obj.max()) * 0.5;
        let at = Vec3::new(c.x, obj.min().y, c.z);
        let floor = obj.floor;
        if l.open {
            // Закрыть: подбираются предметы в пределах шкафа (`Capacity`).
            let mut taken = Vec::new();
            let room = spec.capacity.saturating_sub(l.contents.len());
            for o in ctx.world.authored() {
                let AuthoredKind::Placement { prototype } = &o.kind else {
                    continue;
                };
                if o.id == id || o.floor != floor || !self.infos.contains_key(prototype) {
                    continue;
                }
                let p = (o.min() + o.max()) * 0.5;
                if (p.x - at.x).abs() <= 0.5 && (p.z - at.z).abs() <= 0.5 && taken.len() < room {
                    taken.push(HeldItem {
                        id: o.id,
                        prototype: prototype.clone(),
                    });
                }
            }
            for t in &taken {
                ctx.request(WorldCommand::RemoveAuthored { id: t.id });
            }
            let locker = self.lockers.get_mut(&id).expect("шкаф известен");
            locker.contents.extend(taken);
            locker.open = false;
            Self::swap_locker_proto(ctx, id, &l.closed_proto);
        } else {
            let Some(open) = self.proto_by_name.get(&spec.open_name).cloned() else {
                return false;
            };
            let items = {
                let locker = self.lockers.get_mut(&id).expect("шкаф известен");
                locker.open = true;
                std::mem::take(&mut locker.contents)
            };
            Self::swap_locker_proto(ctx, id, &open);
            self.scatter(ctx, &items, at);
        }
        self.sound(
            if l.open {
                "Effects/closetclose.ogg"
            } else {
                "Effects/closetopen.ogg"
            },
            at,
        );
        true
    }

    /// `LockSystem.TryUnlock/TryLock`: нужен доступ считывателя шкафа; открытый шкаф не запереть.
    pub(crate) fn toggle_lock(
        &mut self,
        ctx: &HookContext<'_>,
        user: NetworkEntityId,
        id: ObjectId,
        only_unlock: bool,
    ) -> bool {
        let Some(l) = self.lockers.get(&id) else {
            return false;
        };
        let Some(spec) = self.storage_specs.get(&l.closed_proto) else {
            return false;
        };
        if spec.lock.is_none() || l.open || (only_unlock && !l.locked) {
            return false;
        }
        let allowed = Self::access_ok(&spec.access, &self.user_access(user));
        let at = Self::center_of(ctx, id);
        if allowed && let Some(l) = self.lockers.get_mut(&id) {
            l.locked = !l.locked;
            let (sound, text) = if l.locked {
                ("Machines/door_lock_on.ogg", "Заперто")
            } else {
                ("Machines/door_lock_off.ogg", "Отперто")
            };
            if let Some(at) = at {
                self.sound(sound, at);
            }
            self.popup(user, text);
        } else if !allowed {
            self.popup(user, "Доступ запрещён");
            if let Some(at) = at {
                self.sound("Machines/buzz-sigh.ogg", at);
            }
        }
        true
    }

    /// Ближайший шкаф или ящик в пределах досягаемости.
    pub(crate) fn nearest_locker(
        &self,
        ctx: &HookContext<'_>,
        user: NetworkEntityId,
    ) -> Option<ObjectId> {
        let feet = ctx.position_of(user)?;
        self.lockers
            .keys()
            .filter_map(|id| {
                let o = ctx.world.authored_object(*id)?;
                let c = (o.min() + o.max()) * 0.5;
                Some((*id, (c - (feet + Vec3::Y * 0.9)).length()))
            })
            .filter(|(_, d)| *d <= self.range_m)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
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
        // Доступ к автомату (`IsAuthorized`): без нужного уровня — отказ.
        let allowed = ctx
            .world
            .authored_object(id)
            .and_then(|o| match &o.kind {
                AuthoredKind::Placement { prototype } => self.vending_specs.get(prototype),
                _ => None,
            })
            .is_none_or(|s| Self::access_ok(&s.access, &self.user_access(user)));
        let at = Self::center_of(ctx, id);
        let mut say: Option<(&str, &str)> = None;
        if let Some(machine) = self.vending.get_mut(&id) {
            if allowed {
                say = match machine.order(now, item, powered) {
                    Ok(()) => Some(("Machines/machine_vend.ogg", "")),
                    Err(crate::vending::Refusal::Denied) => Some((
                        "Machines/buzz-sigh.ogg",
                        "Нет такого товара или он кончился",
                    )),
                    Err(_) => None,
                };
            } else {
                machine.refuse(now);
                say = Some(("Machines/buzz-sigh.ogg", "Доступ запрещён"));
            }
        }
        if let (Some((path, text)), Some(at)) = (say, at) {
            self.sound(path, at);
            if !text.is_empty() {
                self.popup(user, text);
            }
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

    /// Лобби: новички ждут старта; по таймеру готовые выходят на станцию.
    fn tick_lobby(&mut self, ctx: &mut HookContext<'_>) {
        let dt = self.last_dt;
        let ids: Vec<NetworkEntityId> = ctx.players().collect();
        let hold = self.spawn;
        let Some(l) = self.lobby.as_mut() else {
            return;
        };
        l.known.retain(|p| ids.contains(p));
        l.in_lobby.retain(|p| ids.contains(p));
        l.ready.retain(|p| ids.contains(p));
        l.sent.retain(|p, _| ids.contains(p));
        for p in &ids {
            if l.known.insert(*p) {
                // Новичок: в лобби; `placed` не даёт `place_joiners` вывести его на станцию.
                l.in_lobby.insert(*p);
                self.placed.insert(*p);
            }
        }
        if !l.started {
            l.left -= dt;
            if l.left <= 0.0 {
                l.started = true;
                let go: Vec<NetworkEntityId> = l.ready.iter().copied().collect();
                for p in go {
                    l.in_lobby.remove(&p);
                    self.placed.remove(&p);
                }
            }
        }
        // Ожидающие стоят на месте.
        if let Some(at) = hold {
            for p in &l.in_lobby {
                ctx.teleport(*p, at);
            }
        }
        let total = ids.len() as u32;
        let ready_count = l.ready.len() as u32;
        for p in ids {
            let view = crate::lobby::LobbyView {
                in_lobby: l.in_lobby.contains(&p),
                started: l.started,
                left: l.left.max(0.0).ceil() as u32,
                ready: l.ready.contains(&p),
                ready_count,
                total,
                round: l.round,
                station: self.alerts.station.clone(),
            };
            let bytes = crate::lobby::encode_lobby(&view);
            if l.sent.get(&p) != Some(&bytes) {
                ctx.set_component(p, crate::lobby::LOBBY_COMPONENT, bytes.clone());
                l.sent.insert(p, bytes);
            }
        }
    }

    /// Конец раунда: весь экипаж погиб или вышло время; затем сводка и сигнал перезапуска.
    fn tick_round(&mut self, ctx: &mut HookContext<'_>) {
        use crate::round_end::{CrewEntry, EndReason, RESTART_DELAY_S, RoundEndView};
        let dt = self.last_dt;
        if self.lobby.as_ref().is_some_and(|l| !l.started) {
            return;
        }
        // Пустой сервер раунд не считает: время идёт, пока есть хотя бы один игрок.
        if ctx.players().next().is_none() {
            return;
        }
        self.round.clock += dt;
        if self.round.ended.is_none() {
            let alive = |a: &NetworkEntityId| {
                self.actors.get(a).is_some_and(|x| {
                    !x.ghost && !matches!(x.health.state(), crate::health::MobState::Dead)
                })
            };
            let wiped = !self.round.crew.is_empty()
                && !self
                    .round
                    .crew
                    .keys()
                    .any(|a| alive(a) && self.player_job.contains_key(a));
            let reason = if wiped {
                Some(EndReason::CrewDead)
            } else if self.round.limit_s.is_some_and(|t| self.round.clock >= t) {
                Some(EndReason::TimeLimit)
            } else {
                None
            };
            let Some(reason) = reason else {
                return;
            };
            let crew = self
                .round
                .crew
                .iter()
                .map(|(a, job)| CrewEntry {
                    job: job.clone(),
                    alive: alive(a),
                })
                .collect();
            self.round.ended = Some((reason, self.round.clock, crew));
        }
        let Some((reason, at, crew)) = self.round.ended.clone() else {
            return;
        };
        let left = RESTART_DELAY_S - (self.round.clock - at);
        if left <= 0.0 {
            if let Some(s) = &self.round.signal {
                s.request_restart();
            }
            return;
        }
        let number = self.lobby.as_ref().map_or(0, |l| l.round);
        let view = RoundEndView {
            reason: reason.text().into(),
            duration: at as u32,
            restart_in: left.ceil() as u32,
            round: number,
            crew,
        };
        let bytes = crate::round_end::encode_end(&view);
        for p in ctx.players().collect::<Vec<_>>() {
            if self.round.sent.get(&p) != Some(&bytes) {
                ctx.set_component(p, crate::round_end::ROUNDEND_COMPONENT, bytes.clone());
                self.round.sent.insert(p, bytes.clone());
            }
        }
    }

    /// Название должности или «Игрок N» — вместо имени персонажа.
    fn speaker_name(&self, who: NetworkEntityId) -> String {
        self.player_job
            .get(&who)
            .and_then(|j| self.jobs.iter().find(|s| &s.id == j))
            .map_or_else(|| format!("Игрок {}", who.0), |s| s.title.clone())
    }

    /// Сообщение игрока: кому оно слышно, решает канал и расстояние.
    fn chat_say(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId, data: &[u8]) {
        use crate::chat::{self, Channel};
        let Some((&byte, text)) = data.split_first() else {
            return;
        };
        let (Some(mut channel), Ok(text)) = (Channel::from_byte(byte), std::str::from_utf8(text))
        else {
            return;
        };
        let Some(text) = chat::clean(text) else {
            return;
        };
        if self
            .chat
            .last
            .get(&actor)
            .is_some_and(|t| self.clock - t < chat::MIN_GAP_S)
        {
            return;
        }
        self.chat.last.insert(actor, self.clock);
        let in_lobby = self
            .lobby
            .as_ref()
            .is_some_and(|l| l.in_lobby.contains(&actor));
        let gone = |a: &NetworkEntityId| {
            self.actors.get(a).is_some_and(|x| {
                x.ghost || matches!(x.health.state(), crate::health::MobState::Dead)
            })
        };
        let who = if matches!(channel, Channel::Ooc) || in_lobby {
            format!("Игрок {}", actor.0)
        } else {
            self.speaker_name(actor)
        };
        if in_lobby {
            channel = Channel::Ooc;
        } else if gone(&actor) && !matches!(channel, Channel::Ooc) {
            channel = Channel::Dead;
        }
        let spot = ctx.position_of(actor);
        let players: Vec<NetworkEntityId> = ctx.players().collect();
        let id = self.chat.next_id;
        self.chat.next_id += 1;
        let mut heard: Vec<(NetworkEntityId, String)> = Vec::new();
        for p in players {
            let dist = match (spot, ctx.position_of(p)) {
                (Some(a), Some(b)) => a.distance(b),
                _ => f32::INFINITY,
            };
            let deaf_to_living = gone(&p);
            let text_for = match channel {
                Channel::Ooc => Some(text.clone()),
                Channel::Dead => deaf_to_living.then(|| text.clone()),
                Channel::Say | Channel::Emote => {
                    (p == actor || deaf_to_living || dist <= chat::VOICE_RANGE_M)
                        .then(|| text.clone())
                }
                Channel::Whisper => {
                    if p == actor || deaf_to_living || dist <= chat::WHISPER_CLEAR_M {
                        Some(text.clone())
                    } else if dist <= chat::WHISPER_MUFFLED_M {
                        Some(chat::muffle(id, &text))
                    } else {
                        None
                    }
                }
            };
            if let Some(t) = text_for {
                heard.push((p, t));
            }
        }
        let line = chat::ChatLine {
            id,
            channel,
            who,
            text: String::new(),
        };
        for (p, t) in heard {
            let entry = self.chat.log.entry(p).or_default();
            entry.push_back(chat::ChatLine {
                text: t,
                ..line.clone()
            });
            while entry.len() > chat::KEEP_LINES {
                entry.pop_front();
            }
            self.chat.dirty.insert(p);
        }
    }

    /// Разослать журнал тем, кто получил новые строки.
    fn tick_chat(&mut self, ctx: &mut HookContext<'_>) {
        let ids: Vec<NetworkEntityId> = ctx.players().collect();
        self.chat.log.retain(|p, _| ids.contains(p));
        self.chat.last.retain(|p, _| ids.contains(p));
        for p in std::mem::take(&mut self.chat.dirty) {
            let Some(lines) = self.chat.log.get(&p) else {
                continue;
            };
            let mut view = crate::chat::ChatView {
                lines: lines.iter().cloned().collect(),
            };
            let mut bytes = crate::chat::encode_chat(&view);
            while bytes.len() > crate::chat::CHAT_MAX_BYTES && !view.lines.is_empty() {
                view.lines.remove(0);
                bytes = crate::chat::encode_chat(&view);
            }
            ctx.set_component(p, crate::chat::CHAT_COMPONENT, bytes);
        }
    }

    /// «Готов»/«Не готов» до старта, «Присоединиться» после.
    fn lobby_ready(&mut self, _ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        let Some(l) = self.lobby.as_mut() else {
            return;
        };
        if l.started {
            l.in_lobby.remove(&actor);
            self.placed.remove(&actor);
        } else if !l.ready.remove(&actor) {
            l.ready.insert(actor);
        }
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
        let mut sig: Vec<(ObjectId, atmos::Cell, String)> = Vec::new();
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
                && (self.monitor_kinds.contains_key(prototype) || self.alerts.is_alarm(prototype))
            {
                sig.push((
                    o.id,
                    crate::piping::cell_at(o.floor.0, (o.min() + o.max()) * 0.5),
                    format!(
                        "{}|{}",
                        o.metadata
                            .get("links")
                            .map(String::as_str)
                            .unwrap_or_default(),
                        o.metadata
                            .get("device_ids")
                            .map(String::as_str)
                            .unwrap_or_default()
                    ),
                ));
            }
        }
        sig.push((ObjectId::from_u128(0), (0, 0), firelock_doors.to_string()));
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
            if self.monitor_kinds.get(prototype) != Some(&true) && !self.alerts.is_alarm(prototype)
            {
                continue;
            }
            let here = crate::piping::cell_at(o.floor.0, (o.min() + o.max()) * 0.5);
            let mut group = AlarmGroup {
                owner: Some(o.id),
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
            if let Some(ids) = o.metadata.get("device_ids") {
                // New maps preserve DeviceList UID identity, including firelocks
                // represented by structure elements. Do not infer links by co-location.
                group.monitors.clear();
                group.devices.clear();
                group.firelocks.clear();
                for uid in ids.split(';') {
                    let Some(target) = ctx
                        .world
                        .authored()
                        .find(|target| target.metadata.get("ss14_uid").is_some_and(|id| id == uid))
                    else {
                        continue;
                    };
                    if let Some(id) = target
                        .metadata
                        .get("alarm_firelock")
                        .and_then(|id| u128::from_str_radix(id, 16).ok())
                    {
                        group.firelocks.push(ObjectId::from_u128(id));
                    }
                    if let AuthoredKind::Placement { prototype } = &target.kind {
                        if self.monitor_kinds.get(prototype) == Some(&false) {
                            group.monitors.push((
                                target.id,
                                crate::piping::cell_at(
                                    target.floor.0,
                                    (target.min() + target.max()) * 0.5,
                                ),
                            ));
                        }
                        if self.pipe_parts.contains_key(prototype) {
                            group.devices.push(target.id);
                        }
                    }
                }
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
    pub(crate) fn powered_map(&self) -> BTreeMap<ObjectId, bool> {
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
        self.sample_atmos_alerts(ctx);
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
                .owner
                .and_then(|id| self.alerts.level(id))
                .map(|level| level.alarm_state())
                .unwrap_or_else(|| {
                    g.monitors
                        .iter()
                        .map(|(id, _)| {
                            self.monitor_state
                                .get(id)
                                .copied()
                                .unwrap_or(AlarmState::Normal)
                        })
                        .max()
                        .unwrap_or(AlarmState::Normal)
                });
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
pub(crate) fn damage_spec(d: Option<&prototypes::Data>) -> DamageSpec {
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
/// `AccessReader.access`: списки уровней доступа.
fn access_lists(c: &prototypes::Data) -> Vec<Vec<String>> {
    c.get("access")
        .and_then(prototypes::Data::as_list)
        .map(|ls| {
            ls.iter()
                .filter_map(|l| {
                    Some(
                        l.as_list()?
                            .iter()
                            .filter_map(|t| t.as_str().map(String::from))
                            .collect(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

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

    /// Включить лобби: до старта раунда игроки ждут `secs` секунд (`game.lobbyduration`).
    #[must_use]
    pub fn with_lobby(mut self, secs: f32, round: u32) -> Self {
        self.state.lobby = Some(Lobby::new(secs, round));
        self
    }

    /// Сигнал серверу о конце раунда и предел времени раунда, с.
    #[must_use]
    pub fn with_round_end(mut self, signal: crate::RoundSignal, limit_s: Option<f32>) -> Self {
        self.state.round.signal = Some(signal);
        self.state.round.limit_s = limit_s;
        self
    }

    #[must_use]
    pub fn with_station_name(mut self, name: String) -> Self {
        self.state.power_monitor.station = name.clone();
        self.state.alerts.station = name;
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
        if std::env::var_os("ASTRO_AUTOPLAY_OPEN").is_some() {
            eprintln!(
                "ASTRO_INTERACT: id={id} feet={feet:?} at={at:?} distance={} limit={} visible={visible} ray={} powered={:?}",
                (at - chest).length(),
                self.state.range_m,
                ctx.unobstructed_distance(chest, at),
                self.state.powered_map().get(&id)
            );
        }
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
            HookEvent::Command { actor, kind, data } if kind == crate::wire::CMD_CHAT => {
                st.chat_say(ctx, *actor, data);
            }
            HookEvent::Command { actor, kind, .. }
                if st
                    .lobby
                    .as_ref()
                    .is_some_and(|l| l.in_lobby.contains(actor)) =>
            {
                if kind == crate::wire::CMD_LOBBY_READY {
                    st.lobby_ready(ctx, *actor);
                }
            }
            HookEvent::Command { actor, kind, .. } if kind == CMD_RESPAWN => {
                st.respawn(ctx, *actor);
            }
            HookEvent::Command { actor, .. }
                if st
                    .actors
                    .get(actor)
                    .is_some_and(|a| !a.health.state().can_act()) => {}
            HookEvent::Command { actor, kind, data } => match kind.as_str() {
                crate::power_monitor::CMD_FOCUS => {
                    st.focus_power_monitor(ctx, *actor, data);
                }
                crate::power_monitor::CMD_CLOSE => {
                    st.close_power_monitor(*actor);
                }
                crate::alerts::CMD_FOCUS
                | crate::alerts::CMD_SILENCE
                | crate::alerts::CMD_CLOSE => {
                    st.alerts_command(ctx, *actor, kind, data);
                }
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
                crate::wire::CMD_ID_INSERT => {
                    if let [slot, hand] = data[..]
                        && let Some(console) = st.console_open.get(actor).copied()
                    {
                        st.insert_id(
                            ctx,
                            *actor,
                            console,
                            Some(usize::from(slot)),
                            usize::from(hand),
                        );
                    }
                }
                crate::wire::CMD_ID_EJECT => {
                    if let [slot] = data[..]
                        && let Some(console) = st.console_open.get(actor).copied()
                    {
                        st.eject_id(ctx, *actor, console, usize::from(slot));
                    }
                }
                crate::wire::CMD_ID_CLOSE => {
                    st.console_open.remove(actor);
                }
                crate::wire::CMD_ID_WRITE => {
                    if let Some(req) = crate::idconsole::parse_write(data) {
                        st.write_id(*actor, &req);
                    }
                }
                crate::wire::CMD_DISP_DISPENSE => {
                    if let [i] = data[..] {
                        st.dispense(*actor, usize::from(i));
                    }
                }
                crate::wire::CMD_DISP_AMOUNT => {
                    if let [i] = data[..]
                        && let Some(id) = st.dispenser_open.get(actor).copied()
                        && let Some(a) = crate::dispenser::AMOUNTS.get(usize::from(i))
                        && let Some(d) = st.dispensers.get_mut(&id)
                    {
                        d.amount = *a;
                    }
                }
                crate::wire::CMD_DISP_INSERT => {
                    if let [hand] = data[..]
                        && let Some(id) = st.dispenser_open.get(actor).copied()
                    {
                        st.insert_beaker(ctx, *actor, id, usize::from(hand));
                    }
                }
                crate::wire::CMD_DISP_EJECT => {
                    if let Some(id) = st.dispenser_open.get(actor).copied()
                        && let Some(b) = st.dispensers.get_mut(&id).and_then(|d| d.beaker.take())
                    {
                        if st.actor(*actor).hands.pickup_any(b.clone()).is_err()
                            && let Some(feet) = ctx.position_of(*actor)
                        {
                            State::place(ctx, &b, feet);
                        }
                        st.publish(ctx, *actor);
                    }
                }
                crate::wire::CMD_DISP_CLEAR => {
                    if let Some(id) = st.dispenser_open.get(actor).copied()
                        && let Some(b) = st.dispensers.get(&id).and_then(|d| d.beaker.clone())
                        && let Some(s) = st.solution_mut(&b)
                    {
                        s.contents.clear();
                    }
                }
                crate::wire::CMD_CRYO_PUT => {
                    if let Ok(b) = <[u8; 4]>::try_from(&data[..])
                        && let Some(id) = st.cryo_open.get(actor).copied()
                    {
                        let target = NetworkEntityId(u32::from_le_bytes(b));
                        st.begin(ctx, *actor, Action::CryoPut { pod: id, target });
                    }
                }
                crate::wire::CMD_CRYO_EJECT => {
                    if let Some(id) = st.cryo_open.get(actor).copied() {
                        st.cryo_eject_patient(ctx, id);
                    }
                }
                crate::wire::CMD_CRYO_BEAKER_EJECT => {
                    if let Some(id) = st.cryo_open.get(actor).copied() {
                        st.cryo_eject_beaker(ctx, *actor, id);
                    }
                }
                crate::wire::CMD_CRYO_INSERT => {
                    if let [hand] = data[..]
                        && let Some(id) = st.cryo_open.get(actor).copied()
                    {
                        st.cryo_insert(ctx, *actor, id, usize::from(hand));
                    }
                }
                crate::wire::CMD_CRYO_INJECT => {
                    if let [i] = data[..]
                        && let Some(id) = st.cryo_open.get(actor).copied()
                        && let Some(a) = crate::cryo::INJECT_AMOUNTS.get(usize::from(i))
                    {
                        st.cryo_inject(id, *a);
                    }
                }
                crate::wire::CMD_CRYO_CLEAR => {
                    if let Some(id) = st.cryo_open.get(actor).copied() {
                        st.cryo_clear(id);
                    }
                }
                crate::wire::CMD_CRYO_CLOSE => {
                    st.cryo_open.remove(actor);
                }
                crate::wire::CMD_SHAKE => {
                    let hand = st.actors.get(actor).map(|a| a.hands.active());
                    if let Some(hand) = hand {
                        st.begin(
                            ctx,
                            *actor,
                            Action::Mix {
                                hand,
                                target: None,
                                shake: true,
                            },
                        );
                    }
                }
                crate::wire::CMD_LAB_INSERT => {
                    if let [hand] = data[..]
                        && let Some(id) = st.lab_open.get(actor).copied()
                    {
                        st.lab_insert(ctx, *actor, id, usize::from(hand));
                    }
                }
                crate::wire::CMD_LAB_EJECT => {
                    if let [slot] = data[..]
                        && let Some(id) = st.lab_open.get(actor).copied()
                    {
                        st.lab_eject(ctx, *actor, id, slot);
                    }
                }
                crate::wire::CMD_LAB_START => {
                    if let Some(id) = st.lab_open.get(actor).copied() {
                        st.lab_start(ctx, *actor, id);
                    }
                }
                crate::wire::CMD_LAB_CLOSE => {
                    st.lab_open.remove(actor);
                }
                crate::wire::CMD_LAB_MODE => {
                    if let [m] = data[..]
                        && let Some(id) = st.lab_open.get(actor).copied()
                        && let Some(master) = st.masters.get_mut(&id)
                    {
                        master.discard = m == 1;
                    }
                }
                crate::wire::CMD_LAB_SOURCE => {
                    if let [m] = data[..]
                        && let Some(id) = st.lab_open.get(actor).copied()
                        && let Some(master) = st.masters.get_mut(&id)
                    {
                        master.external = m == 1;
                    }
                }
                crate::wire::CMD_LAB_REAGENT => {
                    if let Some((from_buffer, amount, reagent)) =
                        crate::chemlab::parse_reagent(data)
                        && let Some(id) = st.lab_open.get(actor).copied()
                    {
                        st.master_reagent(id, from_buffer, amount, &reagent);
                    }
                }
                crate::wire::CMD_LAB_PILLS => {
                    if let Some((dose, number, label)) = crate::chemlab::parse_make(data)
                        && let Some(id) = st.lab_open.get(actor).copied()
                    {
                        st.master_pills(*actor, id, dose, number, &label);
                    }
                }
                crate::wire::CMD_LAB_BOTTLE => {
                    if let Some((dose, _, label)) = crate::chemlab::parse_make(data)
                        && let Some(id) = st.lab_open.get(actor).copied()
                    {
                        st.master_bottle(*actor, id, dose, &label);
                    }
                }
                crate::wire::CMD_CHEM_AMOUNT => {
                    if let [hand, idx] = data[..] {
                        st.set_chem_amount(*actor, usize::from(hand), idx);
                    }
                }
                crate::wire::CMD_SCAN_CLOSE => {
                    st.scans.remove(actor);
                    ctx.set_component(*actor, crate::analyzer::SCAN_COMPONENT, Vec::new());
                    st.scan_sent.remove(actor);
                }
                crate::wire::CMD_DISP_CLOSE => {
                    st.dispenser_open.remove(actor);
                }
                crate::wire::CMD_LOCK => {
                    if let Some(id) = st.nearest_locker(ctx, *actor) {
                        st.toggle_lock(ctx, *actor, id, false);
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
                    // Пациент криокапсулы выходит из неё (`TryEjectBody`).
                    if let Some(pod) = st.actors.get(actor).and_then(|a| a.cryo) {
                        st.cryo_eject_patient(ctx, pod);
                    }
                    let resisted = st
                        .actors
                        .get_mut(actor)
                        .and_then(|a| a.body.as_mut())
                        .is_some_and(Body::resist);
                    if resisted {
                        st.after_health_change(ctx, *actor);
                    }
                }
                crate::wire::CMD_COMBAT => {
                    if let [on] = data[..] {
                        st.actor(*actor).combat = on != 0;
                    }
                }
                crate::wire::CMD_WALK => {
                    if let [on] = data[..] {
                        st.actor(*actor).walking = on != 0;
                    }
                }
                CMD_ATTACK => {
                    // Бить можно только в боевом режиме (`CombatModeSystem`).
                    if st.actors.get(actor).is_some_and(|a| a.combat)
                        && let Some((target, part)) = parse_attack_part(data)
                    {
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
                        let holds_injector = st
                            .actors
                            .get(actor)
                            .and_then(|a| a.hands.get(usize::from(hand)))
                            .is_some_and(|i| st.injectors.contains_key(&i.prototype));
                        let holds_analyzer = st
                            .actors
                            .get(actor)
                            .and_then(|a| a.hands.get(usize::from(hand)))
                            .is_some_and(|i| st.analyzers.contains(&i.prototype));
                        let holds_defib = st
                            .actors
                            .get(actor)
                            .and_then(|a| a.hands.get(usize::from(hand)))
                            .is_some_and(|i| st.defibs.contains_key(&i.prototype));
                        let action = if holds_defib {
                            Action::Defib {
                                hand: usize::from(hand),
                                target,
                            }
                        } else if holds_analyzer {
                            Action::Scan {
                                hand: usize::from(hand),
                                target,
                            }
                        } else if holds_injector {
                            Action::Inject {
                                hand: usize::from(hand),
                                target,
                            }
                        } else {
                            Action::Heal {
                                hand: usize::from(hand),
                                target,
                            }
                        };
                        st.begin(ctx, *actor, action);
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
                st.power_monitor.forget(*actor);
                st.alerts.forget(*actor);
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
