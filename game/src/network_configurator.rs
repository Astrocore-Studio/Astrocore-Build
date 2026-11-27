//! Настройщик сети (мультитул и настройщик): два режима. В режиме списка клик по устройству
//! сети запоминает его в настройщике, клик по устройству со списком (воздушная тревога)
//! открывает окно настройки: задать список сохранёнными устройствами, добавить их, очистить
//! список, скопировать его в настройщик. В режиме связей клик по источнику или приёмнику
//! сигналов начинает связь, клик по второму открывает окно портов: переключить связь пары
//! портов, очистить связи, связать по умолчанию. Режим меняется глаголом и сам — по цели.
//! В руке в режиме списка открывается окно сохранённых устройств. Проверяется доступ к цели,
//! действия пишутся в журнал администратора.
//! Образец — `NetworkConfiguratorComponent`, `NetworkConfiguratorSystem` (с `.UI`, `.Verb`),
//! `LinkedDeviceNetworkComponent`, сообщения `NetworkConfigurator*Message` SS14 @ 2f4a2aad.
//!
//! Нет пока: показа связей списка на карте (кнопка «Показать», `NetworkConfiguratorLinkOverlay`),
//! паузы использования (`UseDelay`), сохранения запомненных устройств настройщика в карту.

use std::collections::BTreeMap;

use astrocore_ecs::{EntityTerminating, EventWorldExt, LifecycleWorldExt, RegistryWorldExt};
use astrocore_loc::{EntityNameWorldExt, LocArg};
use astrocore_map::{Map, Transform};
use astrocore_net::AppearanceWorldExt;
use astrocore_ui::{BoundUiClosed, BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::activatable_ui::ActivatableUiOpenAttempt;
use crate::admin_logs::{LogImpact, LogType, add_admin_log, player_of};
use crate::device_linking::{self, DeviceLink, DeviceLinkSink, DeviceLinkSource};
use crate::device_network::{
    self, DeviceList, DeviceListUpdateResult, DeviceNetwork, LinkedDeviceNetwork,
};
use crate::examine::{Examined, loc};
use crate::interaction::AfterInteract;
use crate::popups::{PopupType, PopupWorldExt};
use crate::verbs::{GetVerbs, Verb, VerbKind};

pub const LIST_UI_KEY: &str = "enum.NetworkConfiguratorUiKey.List";
pub const CONFIGURE_UI_KEY: &str = "enum.NetworkConfiguratorUiKey.Configure";
pub const LINK_UI_KEY: &str = "enum.NetworkConfiguratorUiKey.Link";
const MODE_VISUALS: &str = "enum.NetworkConfiguratorVisuals.Mode";

fn yes() -> bool {
    true
}

/// Настройщик сети (`NetworkConfiguratorComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkConfigurator {
    /// Режим связей; иначе — режим списка.
    #[serde(default = "yes")]
    pub link_mode_active: bool,
    /// Список устройств, открытый в окне настройки.
    #[serde(skip)]
    pub active_device_list: Option<Entity>,
    /// Устройство, с которого начата связь.
    #[serde(skip)]
    pub active_device_link: Option<Entity>,
    /// Второе устройство связи (окно портов).
    #[serde(skip)]
    pub device_link_target: Option<Entity>,
    /// Сохранённые устройства: адрес → устройство.
    #[serde(skip)]
    pub devices: BTreeMap<i32, Entity>,
    /// Адрес → (префикс адреса, имя) для окна.
    #[serde(skip)]
    pub named_devices: BTreeMap<i32, (Option<String>, String)>,
}

impl Default for NetworkConfigurator {
    fn default() -> Self {
        Self {
            link_mode_active: true,
            active_device_list: None,
            active_device_link: None,
            device_link_target: None,
            devices: BTreeMap::new(),
            named_devices: BTreeMap::new(),
        }
    }
}

/// Сохранённое устройство для окна.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDevice {
    pub address_id: i32,
    /// Адрес для показа (с префиксом или «UID: …» до инициализации карты).
    pub address: String,
    pub name: String,
}

/// Состояние окон списка и настройки (`NetworkConfiguratorUserInterfaceState`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeviceListUiState {
    pub devices: Vec<SavedDevice>,
}

/// Состояние окна портов (`DeviceLinkUserInterfaceState`): порты как (идентификатор, имя).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeviceLinkUiState {
    pub sources: Vec<(String, String)>,
    pub sinks: Vec<(String, String)>,
    pub links: Vec<(String, String)>,
    pub defaults: Option<Vec<(String, String)>>,
    pub source_address: String,
    pub sink_address: String,
}

/// Действия в окнах настройщика.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkConfiguratorMessage {
    /// Убрать устройство из сохранённых (`NetworkConfiguratorRemoveDeviceMessage`).
    RemoveDevice(i32),
    /// Очистить сохранённые (`NetworkConfiguratorListClearDevicesMessage`).
    ClearDevices,
    /// Список = сохранённые (`NetworkConfiguratorSetMessage`).
    Set,
    /// Добавить сохранённые в список (`NetworkConfiguratorAddMessage`).
    Add,
    /// Очистить список (`NetworkConfiguratorClearMessage`).
    Clear,
    /// Скопировать список в сохранённые (`NetworkConfiguratorCopyMessage`).
    Copy,
    /// Переключить связь портов (`NetworkConfiguratorLinkToggleMessage`).
    LinkToggle(String, String),
    /// Убрать все связи пары (`NetworkConfiguratorLinkClearMessage`).
    LinkClear,
    /// Задать связи пары (`NetworkConfiguratorLinkSaveMessage`).
    LinkSave(Vec<(String, String)>),
}

fn is_source(world: &World, entity: Entity) -> bool {
    world.get::<DeviceLinkSource>(entity).is_some()
}

fn is_sink(world: &World, entity: Entity) -> bool {
    world.get::<DeviceLinkSink>(entity).is_some()
}

fn has_linking(world: &World, entity: Entity) -> bool {
    is_source(world, entity) || is_sink(world, entity)
}

fn name(world: &World, entity: Entity) -> String {
    world.entity_name(entity)
}

fn log(world: &mut World, user: Entity, message: String) {
    let players: Vec<_> = player_of(world, user).into_iter().collect();
    add_admin_log(
        world,
        LogType::DeviceLinking,
        LogImpact::Low,
        &message,
        &[],
        &players,
    );
}

/// Карта сущности уже инициализирована (`MapInitialized`).
fn map_initialized(world: &World, entity: Entity) -> bool {
    world
        .get::<Transform>(entity)
        .and_then(Transform::map)
        .and_then(|map| world.get::<Map>(map))
        .is_some_and(|map| map.initialized)
}

fn configurator(world: &World, entity: Entity) -> Option<NetworkConfigurator> {
    world.get::<NetworkConfigurator>(entity).cloned()
}

fn edit(world: &mut World, entity: Entity, change: impl FnOnce(&mut NetworkConfigurator)) {
    if let Some(mut configurator) = world.get_mut::<NetworkConfigurator>(entity) {
        change(&mut configurator);
    }
}

/// Устройства для окна; несуществующие убираются (`ClearInvalidDevices`).
fn saved_devices(world: &mut World, entity: Entity) -> Vec<SavedDevice> {
    let invalid: Vec<i32> = configurator(world, entity)
        .map(|c| {
            c.devices
                .iter()
                .filter(|(_, device)| !world.is_alive(**device))
                .map(|(address, _)| *address)
                .collect()
        })
        .unwrap_or_default();
    edit(world, entity, |c| {
        for address in &invalid {
            c.devices.remove(address);
            c.named_devices.remove(address);
        }
    });
    let Some(configurator) = configurator(world, entity) else {
        return Vec::new();
    };
    configurator
        .named_devices
        .iter()
        .map(|(address_id, (prefix, name))| {
            let device = configurator.devices.get(address_id).copied();
            let address = match device {
                Some(device) if device_network_address(world, device) != 0 => {
                    let bare = device_network::address_string(*address_id);
                    match prefix.as_deref().filter(|p| !p.trim().is_empty()) {
                        Some(prefix) => format!("{}{bare}", loc(world, prefix, &[])),
                        None => bare,
                    }
                }
                _ => format!("UID: {address_id}"),
            };
            SavedDevice {
                address_id: *address_id,
                address,
                name: name.clone(),
            }
        })
        .collect()
}

fn device_network_address(world: &World, device: Entity) -> i32 {
    world.get::<DeviceNetwork>(device).map_or(0, |d| d.address)
}

/// Обновить окна списка и настройки (`UpdateListUiState`).
pub fn update_list_ui_state(world: &mut World, entity: Entity) {
    let state = DeviceListUiState {
        devices: saved_devices(world, entity),
    };
    world.set_ui_state(entity, LIST_UI_KEY, &state);
    world.set_ui_state(entity, CONFIGURE_UI_KEY, &state);
}

/// Добавить устройство (`AddDevice`): запомнить имя и связать устройство с настройщиком.
fn add_device(world: &mut World, entity: Entity, target: Entity, address_id: i32) {
    let Some(prefix) = world.get::<DeviceNetwork>(target).map(|d| d.prefix.clone()) else {
        return;
    };
    let name = name(world, target);
    edit(world, entity, |c| {
        c.devices.insert(address_id, target);
        c.named_devices.insert(address_id, (prefix, name));
    });
    if world.get::<LinkedDeviceNetwork>(target).is_none() {
        world
            .entity_mut(target)
            .insert(LinkedDeviceNetwork::default());
    }
    if let Some(mut linked) = world.get_mut::<LinkedDeviceNetwork>(target) {
        linked.configurators.insert(entity);
    }
}

/// Запомнить устройство сети (`TryAddNetworkDevice`).
pub fn try_add_network_device(world: &mut World, entity: Entity, target: Entity, user: Entity) {
    let Some(device) = world.get::<DeviceNetwork>(target).cloned() else {
        return;
    };
    // Адрес не сохраняется (КПК и т. п.) — в списки не попадает.
    if !device.savable_address {
        return;
    }
    let mut address = device_network::address_of(world, target);
    let mut address_id = device.address;
    if device.address == 0 {
        // До инициализации карты адресов нет: устройство помнится по номеру сущности.
        if map_initialized(world, target) {
            let text = loc(
                world,
                "network-configurator-device-failed",
                &[("device", LocArg::from(name(world, target)))],
            );
            world.popup_entity_for(&text, target, user, PopupType::Small);
            return;
        }
        address_id = target.index_u32() as i32;
        address = format!("UID: {address_id}");
    }
    if configurator(world, entity).is_some_and(|c| c.devices.values().any(|d| *d == target)) {
        let text = loc(
            world,
            "network-configurator-device-already-saved",
            &[("device", LocArg::from(name(world, target)))],
        );
        world.popup_entity_for(&text, user, user, PopupType::Small);
        return;
    }
    add_device(world, entity, target, address_id);
    let text = loc(
        world,
        "network-configurator-device-saved",
        &[
            ("address", LocArg::from(address)),
            ("device", LocArg::from(name(world, target))),
        ],
    );
    world.popup_entity_for(&text, target, user, PopupType::Medium);
    let message = format!(
        "{} saved {} to {}",
        name(world, user),
        name(world, target),
        name(world, entity)
    );
    log(world, user, message);
    update_list_ui_state(world, entity);
}

/// Режим и звук (`SetMode`, `UpdateModeAppearance`).
fn set_mode(world: &mut World, entity: Entity, link_mode: bool) {
    edit(world, entity, |c| {
        c.link_mode_active = link_mode;
        if !link_mode {
            c.active_device_link = None;
        }
    });
    world.set_appearance(entity, MODE_VISUALS, link_mode);
}

/// Переключить режим (`SwitchMode`).
pub fn switch_mode(world: &mut World, entity: Entity) {
    let Some(link_mode) = world
        .get::<NetworkConfigurator>(entity)
        .map(|c| c.link_mode_active)
    else {
        return;
    };
    set_mode(world, entity, !link_mode);
}

/// Режим по цели (`DetermineMode`): устройство только сети — список, только связей — связи.
fn determine_mode(world: &mut World, entity: Entity, target: Entity) {
    let Some(link_mode) = world
        .get::<NetworkConfigurator>(entity)
        .map(|c| c.link_mode_active)
    else {
        return;
    };
    let linking = has_linking(world, target);
    if linking && world.get::<DeviceList>(target).is_some() || linking == link_mode {
        return;
    }
    if world.get::<DeviceNetwork>(target).is_some() {
        set_mode(world, entity, false);
    } else if linking {
        set_mode(world, entity, true);
    }
}

/// Доступ к цели (`AccessCheck`).
fn access_check(world: &mut World, target: Entity, user: Entity) -> bool {
    if crate::access::is_allowed(world, user, target) {
        return true;
    }
    let text = loc(world, "network-configurator-device-access-denied", &[]);
    world.popup_entity_for(&text, target, user, PopupType::Small);
    false
}

/// Начать связь или открыть окно портов (`TryLinkDevice`).
fn try_link_device(world: &mut World, entity: Entity, target: Entity, user: Entity) {
    if !has_linking(world, target) {
        return;
    }
    let active = configurator(world, entity).and_then(|c| c.active_device_link);
    if active == Some(target) {
        let text = loc(world, "network-configurator-link-mode-stopped", &[]);
        world.popup_entity_for(&text, target, user, PopupType::Small);
        edit(world, entity, |c| c.active_device_link = None);
        return;
    }
    if let Some(active) = active
        && (is_source(world, target) && is_sink(world, active)
            || is_sink(world, target) && is_source(world, active))
    {
        open_device_link_ui(world, entity, target, user);
        return;
    }
    if active.is_some_and(|active| {
        is_source(world, target) && is_source(world, active)
            || is_sink(world, target) && is_sink(world, active)
    }) {
        return;
    }
    let text = loc(
        world,
        "network-configurator-link-mode-started",
        &[("device", LocArg::from(name(world, target)))],
    );
    world.popup_entity_for(&text, target, user, PopupType::Small);
    edit(world, entity, |c| c.active_device_link = Some(target));
}

/// Источник и приёмник пары связи настройщика.
fn link_pair(world: &World, entity: Entity) -> Option<(Entity, Entity)> {
    let c = world.get::<NetworkConfigurator>(entity)?;
    let (active, target) = (c.active_device_link?, c.device_link_target?);
    if is_source(world, active) && is_sink(world, target) {
        Some((active, target))
    } else if is_source(world, target) && is_sink(world, active) {
        Some((target, active))
    } else {
        None
    }
}

/// Состояние окна портов (`UpdateLinkUiState`).
pub fn update_link_ui_state(world: &mut World, entity: Entity) {
    if let Some(state) = link_ui_state(world, entity) {
        world.set_ui_state(entity, LINK_UI_KEY, &state);
    }
}

/// Порты, связи и связи по умолчанию пары настройщика.
pub fn link_ui_state(world: &World, entity: Entity) -> Option<DeviceLinkUiState> {
    let (source, sink) = link_pair(world, entity)?;
    let source_ports = world
        .get::<DeviceLinkSource>(source)
        .map(|s| s.ports.clone())
        .unwrap_or_default();
    let sink_ports = world
        .get::<DeviceLinkSink>(sink)
        .map(|s| s.ports.clone())
        .unwrap_or_default();
    let ports = |kind: &str, ids: &std::collections::BTreeSet<String>| -> Vec<(String, String)> {
        ids.iter()
            .map(|id| (id.clone(), device_linking::port_name(world, kind, id)))
            .collect()
    };
    let pair = |link: &DeviceLink| (link.source.clone(), link.sink.clone());
    let defaults: Vec<(String, String)> = device_linking::default_links(world, &source_ports)
        .iter()
        .filter(|link| sink_ports.contains(&link.sink))
        .map(pair)
        .collect();
    let address = |device: Entity| {
        let address = device_network::address_of(world, device);
        if address.is_empty() {
            name(world, device)
        } else {
            address
        }
    };
    Some(DeviceLinkUiState {
        sources: ports("sourcePort", &source_ports),
        sinks: ports("sinkPort", &sink_ports),
        links: device_linking::links_of(world, source, sink)
            .iter()
            .map(pair)
            .collect(),
        defaults: (!defaults.is_empty()).then_some(defaults),
        source_address: address(source),
        sink_address: address(sink),
    })
}

/// Окно портов (`OpenDeviceLinkUi`).
fn open_device_link_ui(world: &mut World, entity: Entity, target: Entity, user: Entity) {
    if !access_check(world, target, user) {
        return;
    }
    edit(world, entity, |c| c.device_link_target = Some(target));
    update_link_ui_state(world, entity);
    world.open_ui(entity, LINK_UI_KEY, user);
}

/// Окно настройки списка (`OpenDeviceListUi`).
fn open_device_list_ui(world: &mut World, entity: Entity, target: Entity, user: Entity) {
    if configurator(world, entity).and_then(|c| c.active_device_link) == Some(target) {
        return;
    }
    if !access_check(world, target, user) || world.get::<DeviceList>(target).is_none() {
        return;
    }
    edit(world, entity, |c| c.active_device_list = Some(target));
    update_list_ui_state(world, entity);
    world.open_ui(entity, CONFIGURE_UI_KEY, user);
}

/// Клик настройщиком (`OnUsed`).
pub fn on_used(
    world: &mut World,
    entity: Entity,
    target: Option<Entity>,
    user: Entity,
    can_reach: bool,
) {
    let Some(target) = target.filter(|_| can_reach) else {
        return;
    };
    determine_mode(world, entity, target);
    if configurator(world, entity).is_some_and(|c| c.link_mode_active) {
        try_link_device(world, entity, target, user);
        return;
    }
    if world.get::<DeviceList>(target).is_none() {
        try_add_network_device(world, entity, target, user);
        return;
    }
    open_device_list_ui(world, entity, target, user);
}

/// Связать по умолчанию (`TryLinkDefaults`).
fn try_link_defaults(world: &mut World, entity: Entity, target: Entity, user: Entity) {
    let Some(c) = configurator(world, entity) else {
        return;
    };
    let Some(active) = c
        .active_device_link
        .filter(|a| c.link_mode_active && *a != target)
    else {
        return;
    };
    if is_source(world, active) && is_sink(world, target) {
        device_linking::link_defaults(world, Some(user), active, target);
    } else if is_sink(world, active) && is_source(world, target) {
        device_linking::link_defaults(world, Some(user), target, active);
    }
}

fn after_button(world: &mut World, entity: Entity, actor: Entity, result: DeviceListUpdateResult) {
    let key = match result {
        DeviceListUpdateResult::TooManyDevices => "network-configurator-too-many-devices",
        DeviceListUpdateResult::UpdateOk => "network-configurator-update-ok",
        _ => return,
    };
    let text = loc(world, key, &[]);
    world.popup_entity_for(&text, actor, actor, PopupType::Medium);
    update_list_ui_state(world, entity);
}

/// Действие в окне (`On…Message` системы настройщика).
pub fn handle_message(
    world: &mut World,
    entity: Entity,
    actor: Entity,
    message: &NetworkConfiguratorMessage,
) {
    let Some(c) = configurator(world, entity) else {
        return;
    };
    let devices: Vec<Entity> = c.devices.values().copied().collect();
    let list = c.active_device_list;
    let tool = name(world, entity);
    let user = name(world, actor);
    match message {
        NetworkConfiguratorMessage::RemoveDevice(address) => {
            if let Some(removed) = c.devices.get(address).copied() {
                let device = name(world, removed);
                log(
                    world,
                    actor,
                    format!("{user} removed buffered device {device} from {tool}"),
                );
                if let Some(mut linked) = world.get_mut::<LinkedDeviceNetwork>(removed) {
                    linked.configurators.remove(&entity);
                }
            }
            edit(world, entity, |c| {
                c.devices.remove(address);
                c.named_devices.remove(address);
            });
            update_list_ui_state(world, entity);
        }
        NetworkConfiguratorMessage::ClearDevices => {
            log(
                world,
                actor,
                format!("{user} cleared buffered devices from {tool}"),
            );
            clear_devices(world, entity);
            update_list_ui_state(world, entity);
        }
        NetworkConfiguratorMessage::Set | NetworkConfiguratorMessage::Add => {
            let Some(list) = list else {
                return;
            };
            let merge = *message == NetworkConfiguratorMessage::Add;
            let target = name(world, list);
            let verb = if merge {
                "added device links to"
            } else {
                "set device links to"
            };
            log(world, actor, format!("{user} {verb} {target} with {tool}"));
            let result = device_network::update_device_list(world, list, &devices, merge);
            after_button(world, entity, actor, result);
        }
        NetworkConfiguratorMessage::Clear => {
            let Some(list) = list else {
                return;
            };
            let target = name(world, list);
            log(
                world,
                actor,
                format!("{user} cleared device links from {target} with {tool}"),
            );
            let result = device_network::update_device_list(world, list, &[], false);
            after_button(world, entity, actor, result);
        }
        NetworkConfiguratorMessage::Copy => {
            let Some(list) = list else {
                return;
            };
            let target = name(world, list);
            log(
                world,
                actor,
                format!("{user} copied devices from {target} to {tool}"),
            );
            clear_devices(world, entity);
            for device in device_network::devices_of(world, list) {
                let address = device_network_address(world, device);
                add_device(world, entity, device, address);
            }
            after_button(world, entity, actor, DeviceListUpdateResult::UpdateOk);
        }
        NetworkConfiguratorMessage::LinkToggle(source_port, sink_port) => {
            if let Some((source, sink)) = link_pair(world, entity) {
                device_linking::toggle_link(
                    world,
                    Some(actor),
                    source,
                    sink,
                    source_port,
                    sink_port,
                );
                update_link_ui_state(world, entity);
            }
        }
        NetworkConfiguratorMessage::LinkClear => {
            if let Some((source, sink)) = link_pair(world, entity) {
                let (a, b) = (name(world, source), name(world, sink));
                log(
                    world,
                    actor,
                    format!("{user} cleared links between {a} and {b} with {tool}"),
                );
                device_linking::remove_sink_from_source(world, source, sink);
                update_link_ui_state(world, entity);
            }
        }
        NetworkConfiguratorMessage::LinkSave(links) => {
            if let Some((source, sink)) = link_pair(world, entity) {
                let links: Vec<DeviceLink> =
                    links.iter().map(|(a, b)| DeviceLink::new(a, b)).collect();
                device_linking::save_links(world, Some(actor), source, sink, &links);
                update_link_ui_state(world, entity);
            }
        }
    }
}

/// Забыть все сохранённые устройства (`ClearDevices`).
fn clear_devices(world: &mut World, entity: Entity) {
    let devices: Vec<Entity> = configurator(world, entity)
        .map(|c| c.devices.values().copied().collect())
        .unwrap_or_default();
    for device in devices {
        if let Some(mut linked) = world.get_mut::<LinkedDeviceNetwork>(device) {
            linked.configurators.remove(&entity);
        }
    }
    edit(world, entity, |c| {
        c.devices.clear();
        c.named_devices.clear();
    });
}

fn verb_icon(name: &str) -> Option<String> {
    Some(format!(
        "/Textures/Interface/VerbIcons/{name}.svg.192dpi.png"
    ))
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<NetworkConfigurator>();
    world.register_ui_message::<NetworkConfiguratorMessage>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<NetworkConfigurator, AfterInteract>(|world, entity, event| {
        let (target, user, can_reach) = (event.target, event.user, event.can_reach);
        on_used(world, entity, target, user, can_reach);
    });
    // В режиме связей окно списка из руки не открывается (`OnUiOpenAttempt`).
    world.subscribe::<NetworkConfigurator, ActivatableUiOpenAttempt>(|world, entity, event| {
        if world
            .get::<NetworkConfigurator>(entity)
            .is_some_and(|c| c.link_mode_active)
        {
            event.cancelled = true;
        } else {
            update_list_ui_state(world, entity);
        }
    });
    world.subscribe::<NetworkConfigurator, Examined>(|world, entity, event| {
        let mode = if world
            .get::<NetworkConfigurator>(entity)
            .is_some_and(|c| c.link_mode_active)
        {
            "network-configurator-examine-mode-link"
        } else {
            "network-configurator-examine-mode-list"
        };
        let mode = loc(world, mode, &[]);
        let text = loc(
            world,
            "network-configurator-examine-current-mode",
            &[("mode", LocArg::from(mode))],
        );
        event.push_markup(text, 0);
    });
    world.subscribe::<NetworkConfigurator, BoundUiMessage<NetworkConfiguratorMessage>>(
        |world, entity, event| {
            let (actor, message) = (event.actor, event.message.clone());
            handle_message(world, entity, actor, &message);
        },
    );
    // Окно закрыто — список и связь забываются (`OnUiClosed`).
    world.subscribe::<NetworkConfigurator, BoundUiClosed>(|world, entity, event| {
        let link = event.key == LINK_UI_KEY;
        edit(world, entity, |c| {
            c.active_device_list = None;
            if link {
                c.active_device_link = None;
                c.device_link_target = None;
            }
        });
    });
    // Удалённое устройство пропадает из сохранённых (`OnDeviceShutdown`).
    world.subscribe::<LinkedDeviceNetwork, EntityTerminating>(|world, entity, _| {
        let configurators: Vec<Entity> = world
            .get::<LinkedDeviceNetwork>(entity)
            .map(|l| l.configurators.iter().copied().collect())
            .unwrap_or_default();
        for configurator in configurators {
            edit(world, configurator, |c| {
                let removed: Vec<i32> = c
                    .devices
                    .iter()
                    .filter(|(_, d)| **d == entity)
                    .map(|(a, _)| *a)
                    .collect();
                for address in removed {
                    c.devices.remove(&address);
                    c.named_devices.remove(&address);
                }
            });
            update_list_ui_state(world, configurator);
        }
    });
    world.subscribe::<NetworkConfigurator, GetVerbs>(|world, entity, event| {
        let (user, target) = (event.user, event.target);
        match event.kind {
            // Глагол настройщиком в руке (`OnAddInteractVerb`).
            VerbKind::Utility if event.can_access && event.can_interact && target != entity => {
                let Some(c) = configurator(world, entity) else {
                    return;
                };
                let (key, icon) = if c.link_mode_active && has_linking(world, target) {
                    let key = if c.active_device_link.is_some() {
                        "network-configurator-link"
                    } else {
                        "network-configurator-start-link"
                    };
                    (key, "in")
                } else if world.get::<DeviceNetwork>(target).is_some() {
                    if world.get::<DeviceList>(target).is_some() {
                        ("network-configurator-configure", "settings")
                    } else {
                        ("network-configurator-save-device", "in")
                    }
                } else {
                    return;
                };
                let text = loc(world, key, &[]);
                let mut verb = Verb::new(VerbKind::Utility, text, move |world| {
                    on_used(world, entity, Some(target), user, true);
                });
                verb.icon = verb_icon(icon);
                event.verbs.push(verb);
            }
            // Сменить режим (`OnAddSwitchModeVerb`).
            VerbKind::Alternative
                if target == entity
                    && event.can_access
                    && event.can_interact
                    && event.using.is_some() =>
            {
                let text = loc(world, "network-configurator-switch-mode", &[]);
                let mut verb = Verb::new(VerbKind::Alternative, text, move |world| {
                    switch_mode(world, entity);
                });
                verb.icon = verb_icon("settings");
                event.verbs.push(verb);
            }
            _ => {}
        }
    });
    // Alt-глаголы на устройстве с настройщиком в руке (`OnAddAlternativeSaveDeviceVerb`).
    world.subscribe::<DeviceNetwork, GetVerbs>(|world, target, event| {
        if event.kind != VerbKind::Alternative || !event.can_access || !event.can_interact {
            return;
        }
        let Some(using) = event.using else {
            return;
        };
        let Some(c) = configurator(world, using) else {
            return;
        };
        let user = event.user;
        if !c.link_mode_active && world.get::<DeviceList>(target).is_some() {
            let text = loc(world, "network-configurator-save-device", &[]);
            let mut verb = Verb::new(VerbKind::Alternative, text, move |world| {
                try_add_network_device(world, using, target, user);
            });
            verb.icon = verb_icon("in");
            event.verbs.push(verb);
            return;
        }
        if c.link_mode_active && c.active_device_link.is_some() && has_linking(world, target) {
            let text = loc(world, "network-configurator-link-defaults", &[]);
            let mut verb = Verb::new(VerbKind::Alternative, text, move |world| {
                try_link_defaults(world, using, target, user);
            });
            verb.icon = verb_icon("in");
            event.verbs.push(verb);
        }
    });
}

#[cfg(test)]
mod tests;
