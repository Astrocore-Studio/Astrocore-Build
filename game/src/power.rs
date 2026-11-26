//! Электросеть: источники, потребители, аккумуляторы, АПЦ и питаемые устройства на сетях
//! кабелей. Образец — `PowerNetSystem`, `BaseNetConnectorComponent`, `PowerSupplierComponent`,
//! `PowerConsumerComponent`, `PowerNetworkBatteryComponent`, `BatteryChargerComponent`,
//! `BatteryDischargerComponent`, `ApcPowerProviderComponent`, `ApcPowerReceiverComponent`,
//! `ExtensionCableSystem`, `BatterySystem` SS14 @ 2f4a2aad. Решатель — `BatteryRampPegSolver`
//! из крейта `power`.
//!
//! Сеть — группа узлов `HVPower`, `MVPower` или `Apc` (`node_container`). Каждый такт состояние
//! решателя собирается из компонентов, решатель делает шаг, результаты пишутся обратно: заряд
//! аккумуляторов, положение рамп, получаемая мощность. Устройство с `ApcPowerReceiver`
//! питается от ближайшего АПЦ (`ExtensionCableProvider`) в пределах дальностей обоих.
//! Сущности на паузе (карта для маппинга) в сети не участвуют.

use std::collections::HashMap;

use astrocore_ecs::{
    ComponentInit, EntityTerminating, EventWorldExt, GameTime, GameWorldExt, LifeStage, MapInit,
    MetaData, Paused, RegistryWorldExt, Update,
};
use astrocore_map::{AnchorStateChanged, MapGrid, TileCoord, Transform};
use astrocore_net::NetRegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::schedule::IntoScheduleConfigs;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::node_container::{NodeContainer, NodeGroups, node_info};

/// Напряжение сети (`Voltage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Voltage {
    #[default]
    High,
    Medium,
    Apc,
}

impl Voltage {
    /// `nodeGroupID` сетей этого напряжения.
    pub fn group(self) -> &'static str {
        match self {
            Voltage::High => "HVPower",
            Voltage::Medium => "MVPower",
            Voltage::Apc => "Apc",
        }
    }
}

fn yes() -> bool {
    true
}

fn ramp() -> f32 {
    5000.0
}

fn one() -> f32 {
    1.0
}

fn five() -> f32 {
    5.0
}

fn three() -> i32 {
    3
}

/// Источник (`PowerSupplierComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerSupplier {
    #[serde(default)]
    pub supply_rate: f32,
    #[serde(default = "ramp")]
    pub supply_ramp_tolerance: f32,
    #[serde(default = "ramp")]
    pub supply_ramp_rate: f32,
    #[serde(default)]
    pub supply_ramp_position: f32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub voltage: Voltage,
    #[serde(default)]
    pub node: Option<String>,
    /// Сколько отдаёт сейчас.
    #[serde(skip)]
    pub current_supply: f32,
}

/// Потребитель на сети напрямую (`PowerConsumerComponent`): эмиттеры, сингулярность и т. п.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerConsumer {
    #[serde(default)]
    pub draw_rate: f32,
    #[serde(default = "yes")]
    pub show_in_monitor: bool,
    #[serde(default)]
    pub voltage: Voltage,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(skip)]
    pub received_power: f32,
}

/// Заряд (`BatteryComponent`), в джоулях.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Battery {
    #[serde(default)]
    pub max_charge: f32,
    #[serde(default)]
    pub starting_charge: f32,
    #[serde(default)]
    pub last_charge: f32,
    /// Передавать ли заряд по сети (`NetSyncEnabled`).
    #[serde(default = "yes")]
    pub netsync: bool,
    #[serde(default = "price_per_joule")]
    pub price_per_joule: f32,
    #[serde(default = "yes")]
    pub show_item_status: bool,
}

fn price_per_joule() -> f32 {
    0.0001
}

/// Аккумулятор на сети (`PowerNetworkBatteryComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerNetworkBattery {
    #[serde(default)]
    pub max_charge_rate: f32,
    #[serde(default)]
    pub max_supply: f32,
    #[serde(default = "ramp")]
    pub supply_ramp_tolerance: f32,
    #[serde(default = "ramp")]
    pub supply_ramp_rate: f32,
    #[serde(default)]
    pub supply_ramp_position: f32,
    #[serde(default)]
    pub current_supply: f32,
    #[serde(default)]
    pub current_receiving: f32,
    #[serde(default)]
    pub loading_network_demand: f32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub can_charge: bool,
    #[serde(default = "yes")]
    pub can_discharge: bool,
    #[serde(default = "one")]
    pub efficiency: f32,
}

/// Подключение заряда аккумулятора к сети (`BatteryChargerComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatteryCharger {
    pub voltage: Voltage,
    pub node: Option<String>,
}

/// Подключение разряда аккумулятора к сети (`BatteryDischargerComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatteryDischarger {
    pub voltage: Voltage,
    pub node: Option<String>,
}

/// АПЦ раздаёт питание устройствам (`ApcPowerProviderComponent`, в данных `PowerProvider`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerProvider {
    pub voltage: Voltage,
    pub node: Option<String>,
}

/// АПЦ (`ApcComponent`): главный выключатель разряда в сеть устройств.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Apc {
    #[serde(default)]
    pub voltage: Voltage,
    /// Главный выключатель (`MainBreakerEnabled`).
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// Питаемое от АПЦ устройство (`ApcPowerReceiverComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApcPowerReceiver {
    #[serde(default = "yes")]
    pub needs_power: bool,
    #[serde(default)]
    pub power_disabled: bool,
    /// Потребляемая мощность, Вт; по умолчанию 5 (`NetworkLoad.DesiredPower`).
    #[serde(default = "five")]
    pub power_load: f32,
    #[serde(skip)]
    pub received_power: f32,
}

/// Устройство запитано. Сетевой компонент: клиент рисует по нему вид устройств.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApcPowered(pub bool);

/// Раздача питания на удлинителе (`ExtensionCableProviderComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionCableProvider {
    #[serde(default = "three")]
    pub transfer_range: i32,
}

/// Приём питания с удлинителя (`ExtensionCableReceiverComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionCableReceiver {
    #[serde(default = "three")]
    pub reception_range: i32,
}

/// Ключ окна АПЦ.
pub const APC_UI_KEY: &str = "enum.ApcUiKey.Key";

/// Внешнее питание АПЦ (`ApcExternalPowerState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApcExternalPower {
    None,
    Low,
    Good,
}

/// Состояние окна АПЦ (`ApcBoundInterfaceState`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApcUiState {
    pub main_breaker: bool,
    /// Отдаёт устройствам, Вт.
    pub power: i32,
    pub external: ApcExternalPower,
    /// Заряд от 0 до 1 с шагом 5 %.
    pub charge: f32,
}

/// Переключить главный выключатель (`ApcToggleMainBreakerMessage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApcToggleMainBreaker;

/// Питание устройства изменилось (`PowerChangedEvent`). Поднимается на устройстве.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerChanged {
    pub powered: bool,
    pub receiving: f32,
}

/// Связи устройств с АПЦ. Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct ApcLinks {
    pub provider_of: HashMap<Entity, Entity>,
    /// Связи надо пересчитать.
    pub dirty: bool,
    /// Поколение групп узлов, по которому посчитаны связи.
    generation: u64,
}

/// Узел сущности для подключения к сети напряжения: заданный по имени или первый узел
/// группы этого напряжения (`TryFindNet`).
fn net_of(
    world: &World,
    groups: &NodeGroups,
    entity: Entity,
    voltage: Voltage,
    node: Option<&str>,
) -> Option<usize> {
    let container = world.get::<NodeContainer>(entity)?;
    let name = container
        .nodes
        .iter()
        .filter(|(name, _)| node.is_none_or(|wanted| wanted == name.as_str()))
        .find(|(_, data)| node_info(data).is_some_and(|info| info.group == voltage.group()))
        .map(|(name, _)| name.clone())?;
    groups.group_of(entity, &name)
}

fn active(world: &World, entity: Entity) -> bool {
    world.get::<Paused>(entity).is_none()
        && world
            .get::<MetaData>(entity)
            .is_none_or(|meta| meta.stage < LifeStage::Terminating)
}

/// Клетка привязанной к гриду сущности.
fn cell_of(world: &World, entity: Entity) -> Option<(Entity, TileCoord, astrocore_math::Vec2)> {
    let transform = world.get::<Transform>(entity)?;
    if !transform.anchored() {
        return None;
    }
    let local = transform.local_position();
    Some((
        transform.grid()?,
        TileCoord::containing(local, transform.level()),
        local,
    ))
}

/// Пересчитывает связи устройств с ближайшими АПЦ (`TryFindAvailableProvider`).
pub fn relink(world: &mut World) {
    let receivers: Vec<(Entity, i32)> = world
        .query::<(Entity, &ExtensionCableReceiver)>()
        .iter(world)
        .map(|(entity, receiver)| (entity, receiver.reception_range))
        .collect();
    let mut links = HashMap::new();
    // Связи не зависят от паузы: на карте для маппинга они готовы к её инициализации.
    for (receiver, range) in receivers {
        let Some((grid, cell, position)) = cell_of(world, receiver) else {
            continue;
        };
        let Some(map_grid) = world.get::<MapGrid>(grid) else {
            continue;
        };
        let mut best: Option<(f32, Entity)> = None;
        for dx in -range..=range {
            for dy in -range..=range {
                let here = TileCoord::new(cell.x + dx, cell.y + dy, cell.z);
                for &entity in map_grid.anchored_at(here) {
                    if entity == receiver {
                        continue;
                    }
                    let Some(provider) = world.get::<ExtensionCableProvider>(entity) else {
                        continue;
                    };
                    let Some(transform) = world.get::<Transform>(entity) else {
                        continue;
                    };
                    let distance = (transform.local_position() - position).length();
                    if distance > range.min(provider.transfer_range) as f32 {
                        continue;
                    }
                    if best.is_none_or(|(d, e)| (distance, entity) < (d, e)) {
                        best = Some((distance, entity));
                    }
                }
            }
        }
        if let Some((_, provider)) = best {
            links.insert(receiver, provider);
        }
    }
    let generation = world
        .get_resource::<NodeGroups>()
        .map_or(0, |g| g.generation);
    let mut state = world.get_resource_or_insert_with(ApcLinks::default);
    state.provider_of = links;
    state.dirty = false;
    state.generation = generation;
}

fn mark_links_dirty(world: &mut World) {
    world.get_resource_or_insert_with(ApcLinks::default).dirty = true;
}

/// Такт электросети (`PowerNetSystem.Update`).
pub fn update(world: &mut World) {
    let generation = world
        .get_resource::<NodeGroups>()
        .map_or(0, |g| g.generation);
    let stale = world
        .get_resource::<ApcLinks>()
        .is_none_or(|links| links.dirty || links.generation != generation);
    if stale {
        relink(world);
    }
    let dt = world.resource::<GameTime>().frame_time();
    let Some(groups) = world.remove_resource::<NodeGroups>() else {
        return;
    };
    let mut state = power::PowerState::default();
    let mut nets: HashMap<usize, usize> = HashMap::new();
    let mut net = |state: &mut power::PowerState, group: usize| -> usize {
        *nets.entry(group).or_insert_with(|| state.add_network())
    };

    // Источники.
    let suppliers: Vec<(Entity, PowerSupplier)> = world
        .query::<(Entity, &PowerSupplier)>()
        .iter(world)
        .map(|(e, s)| (e, s.clone()))
        .collect();
    let mut supply_ids = Vec::new();
    for (entity, supplier) in suppliers {
        if !active(world, entity) {
            continue;
        }
        let Some(group) = net_of(
            world,
            &groups,
            entity,
            supplier.voltage,
            supplier.node.as_deref(),
        ) else {
            continue;
        };
        let mut supply = power::Supply::new(supplier.supply_rate);
        supply.enabled = supplier.enabled;
        supply.ramp_rate = supplier.supply_ramp_rate;
        supply.ramp_tolerance = supplier.supply_ramp_tolerance;
        supply.ramp_position = supplier.supply_ramp_position;
        let id = net(&mut state, group);
        supply_ids.push((entity, state.add_supply(id, supply)));
    }

    // Потребители на сети.
    let consumers: Vec<(Entity, PowerConsumer)> = world
        .query::<(Entity, &PowerConsumer)>()
        .iter(world)
        .map(|(e, c)| (e, c.clone()))
        .collect();
    let mut consumer_ids = Vec::new();
    for (entity, consumer) in consumers {
        if !active(world, entity) {
            continue;
        }
        let Some(group) = net_of(
            world,
            &groups,
            entity,
            consumer.voltage,
            consumer.node.as_deref(),
        ) else {
            continue;
        };
        let id = net(&mut state, group);
        consumer_ids.push((
            entity,
            state.add_load(id, power::Load::new(consumer.draw_rate)),
        ));
    }

    // Аккумуляторы: СМЭС, подстанции, АПЦ.
    let batteries: Vec<(Entity, PowerNetworkBattery)> = world
        .query::<(Entity, &PowerNetworkBattery)>()
        .iter(world)
        .map(|(e, b)| (e, b.clone()))
        .collect();
    let mut battery_ids = Vec::new();
    for (entity, battery) in batteries {
        if !active(world, entity) {
            continue;
        }
        let charging = world
            .get::<BatteryCharger>(entity)
            .and_then(|c| net_of(world, &groups, entity, c.voltage, c.node.as_deref()));
        // АПЦ разряжается в сеть устройств своего выхода.
        let discharging = world
            .get::<BatteryDischarger>(entity)
            .and_then(|d| net_of(world, &groups, entity, d.voltage, d.node.as_deref()))
            .or_else(|| {
                world.get::<Apc>(entity)?;
                let provider = world.get::<PowerProvider>(entity)?;
                net_of(
                    world,
                    &groups,
                    entity,
                    provider.voltage,
                    provider.node.as_deref(),
                )
            });
        let (capacity, storage) = world
            .get::<Battery>(entity)
            .map_or((0.0, 0.0), |b| (b.max_charge, b.last_charge));
        let mut net_battery = power::Battery::new(capacity, storage);
        net_battery.enabled = battery.enabled;
        net_battery.can_charge = battery.can_charge;
        net_battery.can_discharge =
            battery.can_discharge && world.get::<Apc>(entity).is_none_or(|apc| apc.enabled);
        net_battery.max_charge_rate = battery.max_charge_rate;
        net_battery.max_supply = battery.max_supply;
        net_battery.supply_ramp_tolerance = battery.supply_ramp_tolerance;
        net_battery.supply_ramp_rate = battery.supply_ramp_rate;
        net_battery.efficiency = battery.efficiency;
        net_battery.supply_ramp_position = battery.supply_ramp_position;
        net_battery.current_supply = battery.current_supply;
        let charging = charging.map(|g| net(&mut state, g));
        let discharging = discharging.map(|g| net(&mut state, g));
        battery_ids.push((
            entity,
            state.add_battery_ports(charging, discharging, net_battery),
        ));
    }

    // Устройства на АПЦ.
    let links = world
        .get_resource::<ApcLinks>()
        .map(|l| l.provider_of.clone())
        .unwrap_or_default();
    let receivers: Vec<(Entity, ApcPowerReceiver)> = world
        .query::<(Entity, &ApcPowerReceiver)>()
        .iter(world)
        .map(|(e, r)| (e, r.clone()))
        .collect();
    let mut receiver_ids = Vec::new();
    for (entity, receiver) in &receivers {
        if !active(world, *entity) {
            continue;
        }
        let group = links.get(entity).and_then(|provider| {
            let data = world.get::<PowerProvider>(*provider)?;
            net_of(
                world,
                &groups,
                *provider,
                data.voltage,
                data.node.as_deref(),
            )
        });
        let Some(group) = group else {
            continue;
        };
        let id = net(&mut state, group);
        let mut load = power::Load::new(receiver.power_load);
        load.enabled = !receiver.power_disabled;
        receiver_ids.push((*entity, state.add_load(id, load)));
    }
    world.insert_resource(groups);

    state.tick(dt);

    // Результаты.
    for (entity, id) in supply_ids {
        let supply = &state.supplies[id];
        if let Some(mut supplier) = world.get_mut::<PowerSupplier>(entity) {
            supplier.supply_ramp_position = supply.ramp_position;
            supplier.current_supply = supply.current_supply;
        }
    }
    for (entity, id) in consumer_ids {
        let received = state.loads[id].receiving_power;
        if let Some(mut consumer) = world.get_mut::<PowerConsumer>(entity) {
            consumer.received_power = received;
        }
    }
    for (entity, id) in battery_ids {
        let result = state.batteries[id].clone();
        if let Some(mut battery) = world.get_mut::<PowerNetworkBattery>(entity) {
            battery.supply_ramp_position = result.supply_ramp_position;
            battery.current_supply = result.current_supply;
            battery.current_receiving = result.current_receiving;
            battery.loading_network_demand = result.loading_network_demand;
        }
        if let Some(mut charge) = world.get_mut::<Battery>(entity) {
            charge.last_charge = result.current_storage.clamp(0.0, charge.max_charge);
        }
    }
    let received: HashMap<Entity, f32> = receiver_ids
        .into_iter()
        .map(|(entity, id)| (entity, state.loads[id].receiving_power))
        .collect();
    for (entity, receiver) in receivers {
        if !active(world, entity) {
            continue;
        }
        let receiving = received.get(&entity).copied().unwrap_or(0.0);
        // `IsPoweredCalculate`.
        let powered = if receiver.power_disabled {
            false
        } else if !receiver.needs_power {
            true
        } else {
            receiver.power_load > 0.0 && close_to_percent(receiving, receiver.power_load)
        };
        if let Some(mut component) = world.get_mut::<ApcPowerReceiver>(entity) {
            component.received_power = receiving;
        }
        let was = world.get::<ApcPowered>(entity).is_some_and(|p| p.0);
        if was != powered {
            world.entity_mut(entity).insert(ApcPowered(powered));
            world.raise(entity, &mut PowerChanged { powered, receiving });
        }
    }
}

/// `MathHelper.CloseToPercent` с допуском 0,00001 доли.
fn close_to_percent(a: f32, b: f32) -> bool {
    let epsilon = (a.abs().max(b.abs()) * 0.00001).max(f32::EPSILON);
    (a - b).abs() <= epsilon
}

/// Состояние окна АПЦ (`UpdateUIState`, `CalcExtPowerState`).
pub fn apc_ui_state(world: &World, apc: Entity) -> Option<ApcUiState> {
    let component = world.get::<Apc>(apc)?;
    let battery = world.get::<PowerNetworkBattery>(apc)?;
    let charge = world.get::<Battery>(apc)?;
    let fraction = if charge.max_charge > 0.0 {
        charge.last_charge / charge.max_charge
    } else {
        0.0
    };
    let delta = battery.current_supply - battery.current_receiving;
    let external = if battery.current_receiving == 0.0 && (fraction - 1.0).abs() > 1e-5 {
        ApcExternalPower::None
    } else if delta < -0.1 {
        // `!CloseToPercent(delta, 0, 0.1) && delta < 0`: АПЦ принимает больше, чем отдаёт.
        ApcExternalPower::Low
    } else {
        ApcExternalPower::Good
    };
    Some(ApcUiState {
        main_breaker: component.enabled,
        power: battery.current_supply.ceil() as i32,
        external,
        charge: (fraction * 20.0).round() / 20.0,
    })
}

/// Переключает главный выключатель (`ApcToggleBreaker`).
pub fn toggle_apc_breaker(world: &mut World, apc: Entity) {
    let Some(mut component) = world.get_mut::<Apc>(apc) else {
        return;
    };
    component.enabled = !component.enabled;
    let enabled = component.enabled;
    if let Some(mut battery) = world.get_mut::<PowerNetworkBattery>(apc) {
        battery.can_discharge = enabled;
    }
    let mut sound = astrocore_audio::SoundSpecifier::path("/Audio/Machines/machine_switch.ogg");
    if let astrocore_audio::SoundSpecifier::Path { params, .. } = &mut sound {
        params.volume -= 2.0;
    }
    astrocore_audio::AudioWorldExt::play_entity(world, &sound, apc, None);
    if let Some(state) = apc_ui_state(world, apc) {
        astrocore_ui::UiWorldExt::set_ui_state(world, apc, APC_UI_KEY, &state);
    }
}

/// Раз в секунду состояние окон АПЦ.
fn update_apc_ui(world: &mut World, mut last: bevy_ecs::system::Local<f64>) {
    let now = world.resource::<GameTime>().cur_time().as_secs_f64();
    if now - *last < 1.0 {
        return;
    }
    *last = now;
    let apcs: Vec<Entity> = world
        .query::<(Entity, &Apc)>()
        .iter(world)
        .map(|(e, _)| e)
        .collect();
    for apc in apcs {
        if let Some(state) = apc_ui_state(world, apc) {
            astrocore_ui::UiWorldExt::set_ui_state(world, apc, APC_UI_KEY, &state);
        }
    }
}

/// Питается ли устройство.
pub fn is_powered(world: &World, entity: Entity) -> bool {
    match world.get::<ApcPowerReceiver>(entity) {
        Some(receiver) if !receiver.needs_power && !receiver.power_disabled => true,
        Some(_) => world.get::<ApcPowered>(entity).is_some_and(|p| p.0),
        None => true,
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<PowerSupplier>();
    world.register_data_component::<PowerConsumer>();
    world.register_data_component::<Battery>();
    world.register_data_component::<PowerNetworkBattery>();
    world.register_data_component::<BatteryCharger>();
    world.register_data_component::<BatteryDischarger>();
    world.register_data_component::<PowerProvider>();
    world.register_data_component::<Apc>();
    world.register_data_component::<ApcPowerReceiver>();
    world.register_data_component::<ExtensionCableProvider>();
    world.register_data_component::<ExtensionCableReceiver>();
    world.register_net_serde::<ApcPowered>();
    astrocore_ui::UiWorldExt::register_ui_message::<ApcToggleMainBreaker>(world);
    if !side.is_server() {
        return;
    }
    world.subscribe::<Apc, astrocore_ui::BoundUiMessage<ApcToggleMainBreaker>>(
        |world, apc, event| {
            // `OnToggleMainBreaker`: только с доступом считывателя АПЦ.
            if crate::access::is_allowed(world, event.actor, apc) {
                toggle_apc_breaker(world, apc);
            } else {
                let text = crate::examine::loc(world, "apc-component-insufficient-access", &[]);
                crate::popups::PopupWorldExt::popup_entity_for(
                    world,
                    &text,
                    apc,
                    event.actor,
                    crate::popups::PopupType::Medium,
                );
            }
        },
    );
    world.add_game_systems(Update, update_apc_ui);
    world.init_resource::<ApcLinks>();
    // Заряд на старте (`SharedBatterySystem.OnMapInit`).
    world.subscribe::<Battery, MapInit>(|world, entity, _| {
        if let Some(mut battery) = world.get_mut::<Battery>(entity) {
            battery.last_charge = battery.starting_charge.min(battery.max_charge);
        }
    });
    world.subscribe::<ExtensionCableReceiver, ComponentInit>(|world, _, _| mark_links_dirty(world));
    world.subscribe::<ExtensionCableProvider, ComponentInit>(|world, _, _| mark_links_dirty(world));
    world.subscribe::<ExtensionCableReceiver, AnchorStateChanged>(|world, _, _| {
        mark_links_dirty(world)
    });
    world.subscribe::<ExtensionCableProvider, AnchorStateChanged>(|world, _, _| {
        mark_links_dirty(world)
    });
    world.subscribe::<ExtensionCableReceiver, EntityTerminating>(|world, _, _| {
        mark_links_dirty(world)
    });
    world.subscribe::<ExtensionCableProvider, EntityTerminating>(|world, _, _| {
        mark_links_dirty(world)
    });
    world.add_game_systems(Update, update.after(crate::node_container::update_system));
}

#[cfg(test)]
mod tests;
