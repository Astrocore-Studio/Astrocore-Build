//! Сеть устройств: устройство входит в сеть по номеру, получает адрес и слушает частоту;
//! пакет с типизированными данными доходит до адресата или всех слушателей частоты, если
//! вид сети пропускает (проводная — тот же грид, беспроводная — та же карта и дальность,
//! APC — та же сеть APC); список устройств ограничивает, кому слать и от кого принимать.
//! Образец — `DeviceNetworkComponent`, `DeviceNetworkSystem` (с `.API` и `.Net`),
//! `DeviceNet`, `DeviceNetworkPacketEvent`, `BeforePacketSentEvent`,
//! `BeforeBroadcastAttemptEvent`, `WiredNetworkSystem`, `WirelessNetworkSystem`,
//! `ApcNetworkSystem`, `DeviceListComponent`, `DeviceListSystem`,
//! `LinkedDeviceNetworkComponent` и `DeviceFrequencyPrototype` SS14 @ 2f4a2aad (ADR-0010).
//!
//! Нет пока: настройщика сети (`NetworkConfigurator`), маршрутизаторов и глушилок, сетей
//! станции (`StationLimitedNetwork`), сохранения изменённых в игре списков в карту.

use std::collections::{BTreeSet, HashMap};

use astrocore_data::DataNode;
use astrocore_ecs::{EventWorldExt, MapInit, RegistryWorldExt};
use astrocore_map::{MapLoaded, MapPreserved, Transform, TransformWorldExt};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Side;

/// Сеть по умолчанию (`DeviceNetIdDefaults`).
pub const PRIVATE: i32 = 0;
pub const WIRED: i32 = 1;
pub const WIRELESS: i32 = 2;
pub const APC: i32 = 3;
pub const ATMOS_DEVICES: i32 = 4;

/// Номер сети: в данных — имя из `DeviceNetIdDefaults` или число.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceNetId(pub i32);

impl DeviceNetId {
    fn named(name: &str) -> Option<i32> {
        Some(match name {
            "Private" => PRIVATE,
            "Wired" => WIRED,
            "Wireless" => WIRELESS,
            "Apc" => APC,
            "AtmosDevices" => ATMOS_DEVICES,
            "Reserved" => 100,
            other => other.parse().ok()?,
        })
    }

    fn name(self) -> String {
        match self.0 {
            PRIVATE => "Private".into(),
            WIRED => "Wired".into(),
            WIRELESS => "Wireless".into(),
            APC => "Apc".into(),
            ATMOS_DEVICES => "AtmosDevices".into(),
            100 => "Reserved".into(),
            other => other.to_string(),
        }
    }
}

impl Serialize for DeviceNetId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.name())
    }
}

impl<'de> Deserialize<'de> for DeviceNetId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        DeviceNetId::named(&text)
            .map(DeviceNetId)
            .ok_or_else(|| serde::de::Error::custom(format!("сеть «{text}»")))
    }
}

fn yes() -> bool {
    true
}

/// Устройство сети (`DeviceNetworkComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceNetwork {
    #[serde(default)]
    pub device_net_id: DeviceNetId,
    #[serde(default)]
    pub receive_frequency: Option<u32>,
    /// Адрес; 0 — нет (`DeviceAddress.Invalid`).
    #[serde(default)]
    pub address: i32,
    #[serde(default)]
    pub receive_all: bool,
    #[serde(default)]
    pub transmit_frequency: Option<u32>,
    #[serde(default)]
    pub receive_frequency_id: Option<String>,
    #[serde(default)]
    pub transmit_frequency_id: Option<String>,
    #[serde(default)]
    pub send_broadcast_attempt_event: bool,
    #[serde(default)]
    pub examinable_address: bool,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default = "yes")]
    pub auto_connect: bool,
    #[serde(default = "yes")]
    pub savable_address: bool,
    #[serde(default)]
    pub custom_address: bool,
}

/// Проводная сеть: только на своём гриде (`WiredNetworkComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WiredNetwork {}

/// Беспроводная сеть (`WirelessNetworkComponent`): дальность отправителя.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WirelessNetwork {
    #[serde(default)]
    pub range: Option<f32>,
}

/// Сеть по проводам APC (`ApcNetworkComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ApcNetwork {}

fn device_limit() -> usize {
    32
}

/// Список устройств (`DeviceListComponent`). Устройства из карты — номера файла в
/// сохранённых полях (ADR-0010).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceList {
    #[serde(skip)]
    pub devices: BTreeSet<Entity>,
    #[serde(default = "device_limit")]
    pub device_limit: usize,
    #[serde(default = "yes")]
    pub is_allow_list: bool,
    #[serde(default)]
    pub handle_incoming_packets: bool,
}

impl Default for DeviceList {
    fn default() -> Self {
        Self {
            devices: BTreeSet::new(),
            device_limit: device_limit(),
            is_allow_list: true,
            handle_incoming_packets: false,
        }
    }
}

/// Устройство, которое может стоять в списках (`LinkedDeviceNetworkComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LinkedDeviceNetwork {
    #[serde(skip)]
    pub device_lists: BTreeSet<Entity>,
    /// Настройщики, запомнившие устройство (`Configurators`).
    #[serde(skip)]
    pub configurators: BTreeSet<Entity>,
}

/// Устройство в сети (`Device`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Device {
    pub owner: Entity,
    pub receive_frequency: Option<u32>,
    pub address: i32,
    pub receive_all: bool,
}

impl Device {
    fn of(owner: Entity, component: &DeviceNetwork) -> Self {
        Self {
            owner,
            receive_frequency: component.receive_frequency,
            address: component.address,
            receive_all: component.receive_all,
        }
    }
}

/// Одна сеть (`DeviceNet`).
#[derive(Debug, Default)]
pub struct DeviceNet {
    pub devices: HashMap<i32, Device>,
    pub listening: HashMap<u32, BTreeSet<Device>>,
    pub receive_all: HashMap<u32, BTreeSet<Device>>,
}

/// Сети по номерам. Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct DeviceNetworks(pub HashMap<i32, DeviceNet>);

/// Пакет (`DeviceNetworkPacketEvent<T>`). Поднимается на получателе.
#[derive(Debug, Clone)]
pub struct DeviceNetworkPacket<T> {
    pub net_id: i32,
    /// Адресат; `None` — всем на частоте.
    pub address: Option<i32>,
    pub frequency: u32,
    pub sender_address: i32,
    pub sender: Entity,
    pub data: T,
}

/// Перед доставкой (`BeforePacketSentEvent`). На получателе.
#[derive(Debug, Clone)]
pub struct BeforePacketSent {
    pub net_id: i32,
    pub address: Option<i32>,
    pub frequency: u32,
    pub sender_address: i32,
    pub sender: Entity,
    pub sender_grid: Option<Entity>,
    pub sender_map: Option<Entity>,
    pub sender_position: astrocore_math::Vec2,
    pub cancelled: bool,
}

/// Перед рассылкой (`BeforeBroadcastAttemptEvent`). На отправителе.
#[derive(Debug, Clone)]
pub struct BeforeBroadcastAttempt {
    pub recipients: BTreeSet<Device>,
    pub modified_recipients: Option<BTreeSet<Device>>,
    pub cancelled: bool,
}

/// Список устройств изменился (`DeviceListUpdateEvent`). На списке.
#[derive(Debug, Clone)]
pub struct DeviceListUpdate {
    pub old_devices: BTreeSet<Entity>,
    pub devices: BTreeSet<Entity>,
}

/// Итог обновления списка (`DeviceListUpdateResult`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceListUpdateResult {
    NoComponent,
    TooManyDevices,
    UpdateOk,
}

/// Частота прототипа `deviceFrequency`.
pub fn frequency_of(world: &World, id: &str) -> Option<u32> {
    world
        .get_resource::<Prototypes>()?
        .resolved_node("deviceFrequency", id)?
        .get("frequency")?
        .as_str()?
        .parse()
        .ok()
}

/// Адрес текстом (`DeviceAddress.ToString`): `XXXX-XXXX`.
pub fn address_string(address: i32) -> String {
    let bits = address as u32;
    format!("{:04X}-{:04X}", bits >> 16, bits & 0xFFFF)
}

/// Адрес с префиксом (`GetAddressFromId`).
pub fn address_of(world: &World, entity: Entity) -> String {
    let Some(device) = world.get::<DeviceNetwork>(entity) else {
        return String::new();
    };
    let prefix = device
        .prefix
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map(|p| crate::examine::loc(world, p, &[]))
        .unwrap_or_default();
    format!("{prefix}{}", address_string(device.address))
}

fn generate_address(world: &mut World, net: i32) -> i32 {
    loop {
        let address = crate::random::random(world).range(1, i32::MAX);
        let taken = world
            .get_resource::<DeviceNetworks>()
            .and_then(|n| n.0.get(&net))
            .is_some_and(|n| n.devices.contains_key(&address));
        if !taken {
            return address;
        }
    }
}

/// Добавить в сеть (`AddToNetwork`).
fn add_to_network(world: &mut World, entity: Entity) -> bool {
    let Some(component) = world.get::<DeviceNetwork>(entity).cloned() else {
        return false;
    };
    let net = component.device_net_id.0;
    let taken = |world: &World, address: i32| {
        world
            .get_resource::<DeviceNetworks>()
            .and_then(|n| n.0.get(&net))
            .is_some_and(|n| n.devices.contains_key(&address))
    };
    let mut address = component.address;
    if component.custom_address {
        if taken(world, address) {
            return false;
        }
    } else if address == 0 || taken(world, address) {
        address = generate_address(world, net);
        if let Some(mut c) = world.get_mut::<DeviceNetwork>(entity) {
            c.address = address;
        }
    }
    let Some(component) = world.get::<DeviceNetwork>(entity).cloned() else {
        return false;
    };
    let device = Device::of(entity, &component);
    let mut networks = world.get_resource_or_insert_with(DeviceNetworks::default);
    let network = networks.0.entry(net).or_default();
    network.devices.insert(address, device);
    let Some(frequency) = component.receive_frequency else {
        return true;
    };
    network
        .listening
        .entry(frequency)
        .or_default()
        .insert(device);
    if component.receive_all {
        network
            .receive_all
            .entry(frequency)
            .or_default()
            .insert(device);
    }
    true
}

/// Убрать из сети (`RemoveFromNetwork`).
fn remove_from_network(world: &mut World, entity: Entity) -> bool {
    let Some(component) = world.get::<DeviceNetwork>(entity).cloned() else {
        return false;
    };
    let device = Device::of(entity, &component);
    let Some(mut networks) = world.get_resource_mut::<DeviceNetworks>() else {
        return false;
    };
    let Some(network) = networks.0.get_mut(&component.device_net_id.0) else {
        return false;
    };
    if network.devices.remove(&component.address).is_none() {
        return false;
    }
    let Some(frequency) = component.receive_frequency else {
        return true;
    };
    if let Some(listening) = network.listening.get_mut(&frequency) {
        listening.remove(&device);
        if listening.is_empty() {
            network.listening.remove(&frequency);
        }
    }
    if component.receive_all
        && let Some(all) = network.receive_all.get_mut(&frequency)
    {
        all.remove(&device);
        if all.is_empty() {
            network.receive_all.remove(&frequency);
        }
    }
    true
}

/// Подключить (`ConnectDevice`).
pub fn connect_device(world: &mut World, entity: Entity) -> bool {
    add_to_network(world, entity)
}

/// Отключить (`DisconnectDevice`).
pub fn disconnect_device(world: &mut World, entity: Entity, prevent_auto_connect: bool) -> bool {
    if prevent_auto_connect && let Some(mut c) = world.get_mut::<DeviceNetwork>(entity) {
        c.auto_connect = false;
    }
    remove_from_network(world, entity)
}

/// В сети ли (`IsDeviceConnected`).
pub fn is_device_connected(world: &World, entity: Entity) -> bool {
    let Some(component) = world.get::<DeviceNetwork>(entity) else {
        return false;
    };
    world
        .get_resource::<DeviceNetworks>()
        .and_then(|n| n.0.get(&component.device_net_id.0))
        .and_then(|n| n.devices.get(&component.address))
        .is_some_and(|d| d.owner == entity)
}

/// Сменить частоту приёма (`SetReceiveFrequency`).
pub fn set_receive_frequency(world: &mut World, entity: Entity, frequency: Option<u32>) {
    let Some(component) = world.get::<DeviceNetwork>(entity) else {
        return;
    };
    if component.receive_frequency == frequency {
        return;
    }
    remove_from_network(world, entity);
    if let Some(mut c) = world.get_mut::<DeviceNetwork>(entity) {
        c.receive_frequency = frequency;
    }
    add_to_network(world, entity);
}

/// Сменить частоту передачи (`SetTransmitFrequency`).
pub fn set_transmit_frequency(world: &mut World, entity: Entity, frequency: Option<u32>) {
    if let Some(mut c) = world.get_mut::<DeviceNetwork>(entity) {
        c.transmit_frequency = frequency;
    }
}

/// Задать адрес (`SetAddress`).
pub fn set_address(world: &mut World, entity: Entity, address: i32) {
    remove_from_network(world, entity);
    if let Some(mut c) = world.get_mut::<DeviceNetwork>(entity) {
        c.custom_address = true;
        c.address = address;
    }
    add_to_network(world, entity);
}

/// Отправить пакет (`SendPacket`): адресату или всем на частоте. `false` — у отправителя
/// нет адреса или частоты.
pub fn send_packet<T: Clone + Send + Sync + 'static>(
    world: &mut World,
    sender: Entity,
    address: Option<i32>,
    data: T,
    frequency: Option<u32>,
    network: Option<i32>,
) -> bool {
    let Some(component) = world.get::<DeviceNetwork>(sender).cloned() else {
        return false;
    };
    if component.address == 0 {
        return false;
    }
    let Some(frequency) = frequency.or(component.transmit_frequency) else {
        return false;
    };
    let net_id = network.unwrap_or(component.device_net_id.0);
    let packet = DeviceNetworkPacket {
        net_id,
        address,
        frequency,
        sender_address: component.address,
        sender,
        data,
    };
    let recipients: Vec<Device> = {
        let Some(network) = world
            .get_resource::<DeviceNetworks>()
            .and_then(|n| n.0.get(&net_id))
        else {
            return true;
        };
        match address {
            None => {
                let Some(listening) = network.listening.get(&frequency).cloned() else {
                    return true;
                };
                let Some(list) = check_recipients(world, &component, sender, listening) else {
                    return true;
                };
                list.into_iter().collect()
            }
            Some(target) => {
                let mut list: Vec<Device> = network
                    .receive_all
                    .get(&frequency)
                    .map(|d| d.iter().copied().collect())
                    .unwrap_or_default();
                let Some(device) = network.devices.get(&target).copied() else {
                    return true;
                };
                if !device.receive_all && device.receive_frequency == Some(frequency) {
                    list.push(device);
                }
                list
            }
        }
    };
    send_to_connections(world, &recipients, packet);
    true
}

/// Список получателей рассылки (`CheckRecipientsList`): `None` — рассылка отменена.
fn check_recipients(
    world: &World,
    component: &DeviceNetwork,
    sender: Entity,
    recipients: BTreeSet<Device>,
) -> Option<BTreeSet<Device>> {
    if !component.send_broadcast_attempt_event {
        return Some(recipients);
    }
    // Подписчик один — список устройств (`DeviceListSystem.OnBeforeBroadcast`); событие
    // поднимается без изменяемого мира, поэтому разбирается здесь.
    let mut event = BeforeBroadcastAttempt {
        recipients,
        modified_recipients: None,
        cancelled: false,
    };
    before_broadcast(world, sender, &mut event);
    if event.cancelled {
        return None;
    }
    event.modified_recipients.or(Some(event.recipients))
}

/// Фильтр рассылки списка устройств (`OnBeforeBroadcast`).
fn before_broadcast(world: &World, sender: Entity, event: &mut BeforeBroadcastAttempt) {
    let Some(list) = world.get::<DeviceList>(sender) else {
        return;
    };
    if list.devices.is_empty() {
        if list.is_allow_list {
            event.cancelled = true;
        }
        return;
    }
    event.modified_recipients = Some(
        event
            .recipients
            .iter()
            .filter(|d| list.devices.contains(&d.owner) == list.is_allow_list)
            .copied()
            .collect(),
    );
}

/// Доставка (`SendToConnections`).
fn send_to_connections<T: Clone + Send + Sync + 'static>(
    world: &mut World,
    recipients: &[Device],
    packet: DeviceNetworkPacket<T>,
) {
    let sender = packet.sender;
    let transform = world.get::<Transform>(sender);
    let sender_grid = transform.and_then(Transform::grid);
    let sender_map = transform.and_then(Transform::map);
    let sender_position = world
        .world_transform(sender)
        .map_or(astrocore_math::Vec2::ZERO, |t| t.position);
    for device in recipients {
        if device.owner == sender || world.get_entity(device.owner).is_err() {
            continue;
        }
        let mut before = BeforePacketSent {
            net_id: packet.net_id,
            address: packet.address,
            frequency: packet.frequency,
            sender_address: packet.sender_address,
            sender,
            sender_grid,
            sender_map,
            sender_position,
            cancelled: false,
        };
        world.raise(device.owner, &mut before);
        if before.cancelled {
            continue;
        }
        let mut delivered = packet.clone();
        world.raise(device.owner, &mut delivered);
    }
}

/// Группа узлов сети APC устройства: сеть поставщика питания (`ConnectedNode.NodeGroup`).
fn apc_group(world: &World, entity: Entity) -> Option<usize> {
    let provider = world
        .get_resource::<crate::power::ApcLinks>()?
        .provider_of
        .get(&entity)
        .copied()?;
    let groups = world.get_resource::<crate::node_container::NodeGroups>()?;
    groups
        .group_of(provider, "power")
        .or_else(|| groups.group_of(provider, "output"))
}

// Список устройств.

/// Устройства списка (`GetAllDevices`).
pub fn devices_of(world: &World, list: Entity) -> Vec<Entity> {
    world
        .get::<DeviceList>(list)
        .map(|l| l.devices.iter().copied().collect())
        .unwrap_or_default()
}

/// Есть ли адрес в списке (`ExistsInDeviceList`).
pub fn exists_in_device_list(world: &World, list: Entity, address: i32) -> bool {
    devices_of(world, list).into_iter().any(|d| {
        world
            .get::<DeviceNetwork>(d)
            .is_some_and(|n| n.address == address)
    })
}

/// Задать или дополнить список (`UpdateDeviceList`).
pub fn update_device_list(
    world: &mut World,
    list: Entity,
    devices: &[Entity],
    merge: bool,
) -> DeviceListUpdateResult {
    let Some(component) = world.get::<DeviceList>(list).cloned() else {
        return DeviceListUpdateResult::NoComponent;
    };
    let mut wanted: BTreeSet<Entity> = devices.iter().copied().collect();
    if merge {
        wanted.extend(component.devices.iter().copied());
    }
    if wanted.len() > component.device_limit {
        return DeviceListUpdateResult::TooManyDevices;
    }
    let old = component.devices.clone();
    let mut current = component.devices.clone();
    for device in &old {
        if wanted.contains(device) {
            continue;
        }
        current.remove(device);
        if let Some(mut linked) = world.get_mut::<LinkedDeviceNetwork>(*device) {
            linked.device_lists.remove(&list);
        }
    }
    for device in &wanted {
        let Some(mut linked) = world.get_mut::<LinkedDeviceNetwork>(*device) else {
            continue;
        };
        if current.insert(*device) {
            linked.device_lists.insert(list);
        }
    }
    if let Some(mut component) = world.get_mut::<DeviceList>(list) {
        component.devices = current.clone();
    }
    world.raise(
        list,
        &mut DeviceListUpdate {
            old_devices: old,
            devices: current,
        },
    );
    DeviceListUpdateResult::UpdateOk
}

/// Номера сущностей файла из сохранённого поля компонента (`devices: [...]`).
pub fn preserved_uids(world: &World, entity: Entity, component: &str, field: &str) -> Vec<u32> {
    let Some(preserved) = world.get::<MapPreserved>(entity) else {
        return Vec::new();
    };
    preserved
        .fields
        .iter()
        .filter(|(name, _)| name == component)
        .filter_map(|(_, fields)| fields.get(field))
        .flat_map(|node| node.as_seq().map(<[DataNode]>::to_vec).unwrap_or_default())
        .filter_map(|n| n.as_str()?.parse().ok())
        .collect()
}

/// Списки устройств, изменённые в игре, — в сохранённую карту номерами её сущностей
/// (ADR-0014). Список, совпадающий с загруженным, остаётся как в файле (порядок номеров).
pub fn write_references(world: &mut World, save: &mut astrocore_map::MapSave) {
    let mut lists: Vec<(Entity, Vec<Entity>)> = world
        .query::<(Entity, &DeviceList)>()
        .iter(world)
        .filter(|(entity, _)| save.uids.contains_key(entity))
        .map(|(entity, list)| (entity, list.devices.iter().copied().collect()))
        .collect();
    lists.sort();
    for (list, devices) in lists {
        let current: Vec<u32> = devices
            .iter()
            .filter_map(|device| save.uids.get(device).copied())
            .collect();
        let preserved = preserved_uids(world, list, "DeviceList", "devices");
        let same = |a: &[u32], b: &[u32]| {
            a.iter().collect::<BTreeSet<_>>() == b.iter().collect::<BTreeSet<_>>()
        };
        if same(&current, &preserved) {
            continue;
        }
        let value = (!current.is_empty()).then(|| {
            DataNode::seq(
                current
                    .iter()
                    .map(|uid| DataNode::plain(uid.to_string()))
                    .collect(),
            )
        });
        crate::map_commands::set_saved_component_field(
            &mut save.document,
            save.uids[&list],
            "DeviceList",
            "devices",
            value,
        );
    }
}

/// Списки устройств из карты (ADR-0010): номера файла → сущности.
fn adopt_device_lists(world: &mut World, loaded: &MapLoaded) {
    let mut lists: Vec<Entity> = loaded
        .uids
        .values()
        .copied()
        .filter(|e| world.get::<DeviceList>(*e).is_some())
        .collect();
    lists.sort();
    for list in lists {
        let devices: Vec<Entity> = preserved_uids(world, list, "DeviceList", "devices")
            .into_iter()
            .filter_map(|uid| loaded.uids.get(&uid).copied())
            .collect();
        // Как десериализация `Devices` в SS14: список задаётся напрямую, устройство
        // помнит список (`LinkedDeviceNetworkComponent` из карты).
        for device in devices {
            if let Some(mut component) = world.get_mut::<DeviceList>(list) {
                component.devices.insert(device);
            }
            if world.get::<LinkedDeviceNetwork>(device).is_none() {
                world
                    .entity_mut(device)
                    .insert(LinkedDeviceNetwork::default());
            }
            if let Some(mut linked) = world.get_mut::<LinkedDeviceNetwork>(device) {
                linked.device_lists.insert(list);
            }
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<DeviceNetwork>();
    world.register_data_component::<WiredNetwork>();
    world.register_data_component::<WirelessNetwork>();
    world.register_data_component::<ApcNetwork>();
    world.register_data_component::<DeviceList>();
    world.register_data_component::<LinkedDeviceNetwork>();
    if !side.is_server() {
        return;
    }
    world.init_resource::<DeviceNetworks>();
    // Частоты из прототипов и подключение (`OnMapInit`).
    world.subscribe::<DeviceNetwork, MapInit>(|world, entity, _| {
        let Some(component) = world.get::<DeviceNetwork>(entity).cloned() else {
            return;
        };
        let receive = component
            .receive_frequency
            .or_else(|| frequency_of(world, component.receive_frequency_id.as_deref()?));
        let transmit = component
            .transmit_frequency
            .or_else(|| frequency_of(world, component.transmit_frequency_id.as_deref()?));
        if let Some(mut c) = world.get_mut::<DeviceNetwork>(entity) {
            c.receive_frequency = receive;
            c.transmit_frequency = transmit;
        }
        if component.auto_connect {
            connect_device(world, entity);
        }
    });
    world.subscribe::<DeviceNetwork, astrocore_ecs::EntityTerminating>(|world, entity, _| {
        remove_from_network(world, entity);
    });
    world.subscribe::<DeviceNetwork, crate::examine::Examined>(|world, entity, event| {
        if world
            .get::<DeviceNetwork>(entity)
            .is_some_and(|d| d.examinable_address)
        {
            let address = address_of(world, entity);
            let text = crate::examine::loc(
                world,
                "device-address-examine-message",
                &[("address", astrocore_loc::LocArg::from(address.as_str()))],
            );
            event.push_markup(text, 0);
        }
    });
    // Виды сети (`OnBeforePacketSent`).
    world.subscribe::<WiredNetwork, BeforePacketSent>(|world, entity, event| {
        let grid = world.get::<Transform>(entity).and_then(Transform::grid);
        if grid != event.sender_grid {
            event.cancelled = true;
        }
    });
    world.subscribe::<WirelessNetwork, BeforePacketSent>(|world, entity, event| {
        let Some(range) = world.get::<WirelessNetwork>(event.sender).map(|w| w.range) else {
            return;
        };
        let map = world.get::<Transform>(entity).and_then(Transform::map);
        let position = world
            .world_transform(entity)
            .map_or(astrocore_math::Vec2::ZERO, |t| t.position);
        if map != event.sender_map
            || range.is_some_and(|r| (event.sender_position - position).length() > r)
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<ApcNetwork, BeforePacketSent>(|world, entity, event| {
        if world.get::<ApcNetwork>(event.sender).is_none() {
            return;
        }
        let sender = apc_group(world, event.sender);
        if sender.is_none() || sender != apc_group(world, entity) {
            event.cancelled = true;
        }
    });
    world.subscribe::<DeviceList, BeforePacketSent>(|world, entity, event| {
        let Some(list) = world.get::<DeviceList>(entity) else {
            return;
        };
        if list.handle_incoming_packets
            && list.devices.contains(&event.sender) != list.is_allow_list
        {
            event.cancelled = true;
        }
    });
    // Список удалён — устройства забывают о нём; устройство удалено — списки тоже.
    world.subscribe::<DeviceList, astrocore_ecs::EntityTerminating>(|world, list, _| {
        for device in devices_of(world, list) {
            if let Some(mut linked) = world.get_mut::<LinkedDeviceNetwork>(device) {
                linked.device_lists.remove(&list);
            }
        }
    });
    world.subscribe::<LinkedDeviceNetwork, astrocore_ecs::EntityTerminating>(|world, device, _| {
        let lists: Vec<Entity> = world
            .get::<LinkedDeviceNetwork>(device)
            .map(|l| l.device_lists.iter().copied().collect())
            .unwrap_or_default();
        for list in lists {
            if let Some(mut component) = world.get_mut::<DeviceList>(list) {
                component.devices.remove(&device);
            }
        }
    });
    world.subscribe_broadcast::<MapLoaded>(|world, event| adopt_device_lists(world, event));
}

#[cfg(test)]
pub(crate) mod tests;
