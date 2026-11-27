//! Связи устройств (сигналы): у источника есть выходные порты, у приёмника — входные; связь
//! соединяет порт источника с портом приёмника. Сигнал на выходной порт доходит до всех
//! приёмников, связанных с этим портом: напрямую событием, а если у обоих есть сеть
//! устройств — пакетом по беспроводной сети (дальность и карта решают, дойдёт ли).
//! Логический сигнал (высокий или низкий уровень) приходит только пакетом; источник помнит
//! последний уровень порта и посылает его новой связи. Приёмник, которого дёргают чаще
//! предела за такт, перегружается и теряет все связи.
//! Образец — `DeviceLinkSourceComponent`, `DeviceLinkSinkComponent`, `DevicePortPrototype`,
//! `DeviceLinkSystem` (с `.Links`, `.Packets`, `.Ports`), `SignalReceivedEvent`,
//! `NewLinkEvent`, `LinkAttemptEvent`, `DeviceLinkOverloadedEvent` SS14 @ 2f4a2aad
//! (ADR-0010).
//!
//! Связи из карты (`linkedPorts`: номер приёмника в файле → пары портов) разбираются после
//! загрузки, как списки устройств. Нет пока: настройщика сети (`NetworkConfigurator`), которым
//! игрок связывает устройства, сохранения изменённых в игре связей в карту, эффектов
//! перегрузки (`EntityEffectOnDeviceOverload`), автосвязывания (`AutoLink`), сигнала
//! уровня при загрузке карты до разбора связей (воздушная тревога шлёт его на `MapInit`).

use std::collections::{BTreeMap, BTreeSet};

use astrocore_data::DataNode;
use astrocore_ecs::{EntityTerminating, EventWorldExt, GameTime, LifecycleWorldExt};
use astrocore_map::{MapLoaded, MapPreserved, Transform, TransformWorldExt};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::device_network::{DeviceNetwork, DeviceNetworkPacket, WIRELESS, send_packet};
use crate::popups::{PopupType, PopupWorldExt};

/// Связь «порт источника → порт приёмника» (`DeviceLink`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceLink {
    pub source: String,
    pub sink: String,
}

impl DeviceLink {
    pub fn new(source: &str, sink: &str) -> Self {
        Self {
            source: source.into(),
            sink: sink.into(),
        }
    }
}

/// Уровень логического сигнала (`SignalState`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalState {
    /// Мгновенный импульс — для совместимости со старыми сигналами.
    Momentary,
    #[default]
    Low,
    High,
}

impl SignalState {
    pub fn of(high: bool) -> Self {
        if high { Self::High } else { Self::Low }
    }
}

fn default_range() -> f32 {
    30.0
}

fn default_invoke_limit() -> i32 {
    10
}

/// Источник сигналов (`DeviceLinkSourceComponent`). Связи из карты — номера файла в
/// сохранённых полях (`linkedPorts`, ADR-0010).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLinkSource {
    #[serde(default)]
    pub ports: BTreeSet<String>,
    /// Последний уровень порта (`LastSignals`).
    #[serde(default)]
    pub last_signals: BTreeMap<String, bool>,
    /// Порт → связанные приёмники (`Outputs`).
    #[serde(skip)]
    pub outputs: BTreeMap<String, BTreeSet<Entity>>,
    /// Приёмник → связи с ним (`LinkedPorts`).
    #[serde(skip)]
    pub linked_ports: BTreeMap<Entity, BTreeSet<DeviceLink>>,
    #[serde(default = "default_range")]
    pub range: f32,
}

impl Default for DeviceLinkSource {
    fn default() -> Self {
        Self {
            ports: BTreeSet::new(),
            last_signals: BTreeMap::new(),
            outputs: BTreeMap::new(),
            linked_ports: BTreeMap::new(),
            range: default_range(),
        }
    }
}

/// Приёмник сигналов (`DeviceLinkSinkComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLinkSink {
    #[serde(default)]
    pub ports: BTreeSet<String>,
    #[serde(skip)]
    pub linked_sources: BTreeSet<Entity>,
    /// Сколько раз дёрнули недавно: убывает на единицу за такт.
    #[serde(default)]
    pub invoke_counter: i32,
    #[serde(skip)]
    pub invoke_counter_tick: u32,
    #[serde(default = "default_invoke_limit")]
    pub invoke_limit: i32,
}

impl Default for DeviceLinkSink {
    fn default() -> Self {
        Self {
            ports: BTreeSet::new(),
            linked_sources: BTreeSet::new(),
            invoke_counter: 0,
            invoke_counter_tick: 0,
            invoke_limit: default_invoke_limit(),
        }
    }
}

// События.

/// Сигнал пришёл на порт (`SignalReceivedEvent`). На приёмнике.
#[derive(Debug, Clone)]
pub struct SignalReceived {
    pub port: String,
    pub trigger: Option<Entity>,
}

/// Логический сигнал пришёл на порт (`SignalReceivedEvent<LogicStatePayload>`). На приёмнике.
#[derive(Debug, Clone)]
pub struct LogicSignalReceived {
    pub port: String,
    pub state: SignalState,
    pub trigger: Option<Entity>,
}

/// Новая связь (`NewLinkEvent`). На источнике и на приёмнике.
#[derive(Debug, Clone)]
pub struct NewLink {
    pub user: Option<Entity>,
    pub source: Entity,
    pub source_port: String,
    pub sink: Entity,
    pub sink_port: String,
}

/// Можно ли связать (`LinkAttemptEvent`). На источнике, затем на приёмнике.
#[derive(Debug, Clone)]
pub struct LinkAttempt {
    pub user: Option<Entity>,
    pub source: Entity,
    pub source_port: String,
    pub sink: Entity,
    pub sink_port: String,
    pub cancelled: bool,
}

/// Порт источника отсоединён (`SourcePortDisconnectedEvent`).
#[derive(Debug, Clone)]
pub struct SourcePortDisconnected {
    pub port: String,
}

/// Порт приёмника отсоединён (`SinkPortDisconnectedEvent`).
#[derive(Debug, Clone)]
pub struct SinkPortDisconnected {
    pub port: String,
}

/// Приёмник перегружен и потерял связи (`DeviceLinkOverloadedEvent`).
#[derive(Debug, Clone, Default)]
pub struct DeviceLinkOverloaded;

/// Пакет сигнала (`SignalPayload`).
#[derive(Debug, Clone)]
pub struct SignalPayload {
    pub invoked_port: String,
}

/// Пакет логического сигнала (`SignalPayload<LogicStatePayload>`).
#[derive(Debug, Clone)]
pub struct LogicSignalPayload {
    pub invoked_port: String,
    pub state: SignalState,
}

// Порты.

/// Добавить выходные порты (`EnsureSourcePorts`). Неизвестный порт пропускается.
pub fn ensure_source_ports(world: &mut World, entity: Entity, ports: &[&str]) {
    if ports.is_empty() {
        return;
    }
    world.ensure_comp::<DeviceLinkSource>(entity);
    let known: Vec<String> = ports
        .iter()
        .filter(|p| port_known(world, "sourcePort", p))
        .map(|p| (*p).to_owned())
        .collect();
    if let Some(mut source) = world.get_mut::<DeviceLinkSource>(entity) {
        source.ports.extend(known);
    }
}

/// Добавить входные порты (`EnsureSinkPorts`). Неизвестный порт пропускается.
pub fn ensure_sink_ports(world: &mut World, entity: Entity, ports: &[&str]) {
    if ports.is_empty() {
        return;
    }
    world.ensure_comp::<DeviceLinkSink>(entity);
    let known: Vec<String> = ports
        .iter()
        .filter(|p| port_known(world, "sinkPort", p))
        .map(|p| (*p).to_owned())
        .collect();
    if let Some(mut sink) = world.get_mut::<DeviceLinkSink>(entity) {
        sink.ports.extend(known);
    }
}

fn port_known(world: &World, kind: &str, id: &str) -> bool {
    let known = world
        .get_resource::<Prototypes>()
        .is_some_and(|p| p.resolved_node(kind, id).is_some());
    if !known {
        eprintln!("нет порта {kind} «{id}»");
    }
    known
}

/// Имя порта для игрока (`PortName`).
pub fn port_name(world: &World, kind: &str, id: &str) -> String {
    world
        .get_resource::<Prototypes>()
        .and_then(|p| {
            p.resolved_node(kind, id)?
                .get("name")?
                .as_str()
                .map(str::to_owned)
        })
        .map_or_else(
            || id.to_owned(),
            |key| crate::examine::loc(world, &key, &[]),
        )
}

/// Связи по умолчанию для портов (`GetDefaults`): порт без связей по умолчанию отменяет все.
pub fn default_links(world: &World, ports: &BTreeSet<String>) -> Vec<DeviceLink> {
    let Some(prototypes) = world.get_resource::<Prototypes>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for port in ports {
        let Some(links) = prototypes
            .resolved_node("sourcePort", port)
            .and_then(|n| n.get("defaultLinks"))
            .and_then(DataNode::as_seq)
        else {
            return Vec::new();
        };
        out.extend(
            links
                .iter()
                .filter_map(DataNode::as_str)
                .map(|sink| DeviceLink::new(port, sink)),
        );
    }
    out
}

// Связи.

/// Связи источника с приёмником (`GetLinks`).
pub fn links_of(world: &World, source: Entity, sink: Entity) -> BTreeSet<DeviceLink> {
    world
        .get::<DeviceLinkSource>(source)
        .and_then(|s| s.linked_ports.get(&sink).cloned())
        .unwrap_or_default()
}

/// Приёмники порта (`GetLinkedSinks`).
pub fn linked_sinks(world: &World, source: Entity, port: &str) -> BTreeSet<Entity> {
    world
        .get::<DeviceLinkSource>(source)
        .and_then(|s| s.outputs.get(port).cloned())
        .unwrap_or_default()
}

fn in_range(world: &World, source: Entity, sink: Entity, range: f32) -> bool {
    let map = |e| world.get::<Transform>(e).and_then(Transform::map);
    let position = |e| world.world_transform(e).map(|t| t.position);
    match (position(source), position(sink)) {
        (Some(a), Some(b)) => map(source) == map(sink) && (a - b).length() <= range,
        _ => false,
    }
}

fn popup_cursor(world: &mut World, user: Option<Entity>, key: &str, args: &[(&str, &str)]) {
    let Some(user) = user else {
        return;
    };
    let args: Vec<(&str, astrocore_loc::LocArg)> = args
        .iter()
        .map(|(k, v)| (*k, astrocore_loc::LocArg::from(*v)))
        .collect();
    let text = crate::examine::loc(world, key, &args);
    world.popup_cursor(&text, user, PopupType::Small);
}

fn entity_name(world: &World, entity: Entity) -> String {
    astrocore_loc::EntityNameWorldExt::entity_name(world, entity)
}

/// Можно ли связать (`CanLink`): дальность и согласие источника и приёмника.
fn can_link(
    world: &mut World,
    user: Option<Entity>,
    source: Entity,
    sink: Entity,
    source_port: &str,
    sink_port: &str,
    check_range: bool,
) -> bool {
    let Some(range) = world.get::<DeviceLinkSource>(source).map(|s| s.range) else {
        return false;
    };
    if check_range && !in_range(world, source, sink, range) {
        popup_cursor(world, user, "signal-linker-component-out-of-range", &[]);
        return false;
    }
    let mut event = LinkAttempt {
        user,
        source,
        source_port: source_port.into(),
        sink,
        sink_port: sink_port.into(),
        cancelled: false,
    };
    world.raise(source, &mut event);
    if event.cancelled && user.is_some() {
        popup_cursor(
            world,
            user,
            "signal-linker-component-connection-refused",
            &[("machine", source_port)],
        );
        return false;
    }
    world.raise(sink, &mut event);
    if event.cancelled && user.is_some() {
        popup_cursor(
            world,
            user,
            "signal-linker-component-connection-refused",
            &[("machine", source_port)],
        );
        return false;
    }
    !event.cancelled
}

fn send_new_link(
    world: &mut World,
    user: Option<Entity>,
    source: Entity,
    source_port: &str,
    sink: Entity,
    sink_port: &str,
) {
    let mut event = NewLink {
        user,
        source,
        source_port: source_port.into(),
        sink,
        sink_port: sink_port.into(),
    };
    world.raise(source, &mut event);
    world.raise(sink, &mut event);
}

/// Связать по умолчанию (`LinkDefaults`).
pub fn link_defaults(world: &mut World, user: Option<Entity>, source: Entity, sink: Entity) {
    let Some(ports) = world
        .get::<DeviceLinkSource>(source)
        .map(|s| s.ports.clone())
    else {
        return;
    };
    if world.get::<DeviceLinkSink>(sink).is_none() {
        return;
    }
    let links = default_links(world, &ports);
    save_links(world, user, source, sink, &links);
    if user.is_some() {
        let name = entity_name(world, source);
        popup_cursor(
            world,
            user,
            "signal-linking-verb-success",
            &[("machine", &name)],
        );
    }
}

/// Заменить связи источника с приёмником (`SaveLinks`).
pub fn save_links(
    world: &mut World,
    user: Option<Entity>,
    source: Entity,
    sink: Entity,
    links: &[DeviceLink],
) {
    let Some(range) = world.get::<DeviceLinkSource>(source).map(|s| s.range) else {
        return;
    };
    if world.get::<DeviceLinkSink>(sink).is_none() {
        return;
    }
    if !in_range(world, source, sink, range) {
        popup_cursor(world, user, "signal-linker-component-out-of-range", &[]);
        return;
    }
    remove_sink_from_source(world, source, sink);
    for link in links {
        let valid = world
            .get::<DeviceLinkSource>(source)
            .is_some_and(|s| s.ports.contains(&link.source))
            && world
                .get::<DeviceLinkSink>(sink)
                .is_some_and(|s| s.ports.contains(&link.sink));
        if !valid || !can_link(world, user, source, sink, &link.source, &link.sink, false) {
            continue;
        }
        if let Some(mut component) = world.get_mut::<DeviceLinkSource>(source) {
            component
                .outputs
                .entry(link.source.clone())
                .or_default()
                .insert(sink);
            component
                .linked_ports
                .entry(sink)
                .or_default()
                .insert(link.clone());
        }
        send_new_link(world, user, source, &link.source, sink, &link.sink);
    }
    if !links.is_empty()
        && let Some(mut component) = world.get_mut::<DeviceLinkSink>(sink)
    {
        component.linked_sources.insert(source);
    }
}

/// Отвязать приёмник от всех источников (`RemoveAllFromSink`).
pub fn remove_all_from_sink(world: &mut World, sink: Entity) {
    let sources: Vec<Entity> = world
        .get::<DeviceLinkSink>(sink)
        .map(|s| s.linked_sources.iter().copied().collect())
        .unwrap_or_default();
    for source in sources {
        remove_sink_from_source(world, source, sink);
    }
}

/// Убрать связи источника с приёмником (`RemoveSinkFromSource`).
pub fn remove_sink_from_source(world: &mut World, source: Entity, sink: Entity) {
    let has_source = world.get::<DeviceLinkSource>(source).is_some();
    let has_sink = world.get::<DeviceLinkSink>(sink).is_some();
    match (has_source, has_sink) {
        (true, true) => remove_sink_from_source_internal(world, source, sink),
        (false, true) => {
            if let Some(mut s) = world.get_mut::<DeviceLinkSink>(sink) {
                s.linked_sources.remove(&source);
            }
        }
        (true, false) => {
            if let Some(mut s) = world.get_mut::<DeviceLinkSource>(source) {
                s.linked_ports.remove(&sink);
            }
        }
        (false, false) => {}
    }
}

fn remove_sink_from_source_internal(world: &mut World, source: Entity, sink: Entity) {
    for link in links_of(world, source, sink) {
        world.raise(
            source,
            &mut SourcePortDisconnected {
                port: link.source.clone(),
            },
        );
        world.raise(sink, &mut SinkPortDisconnected { port: link.sink });
    }
    if let Some(mut s) = world.get_mut::<DeviceLinkSink>(sink) {
        s.linked_sources.remove(&source);
    }
    if let Some(mut s) = world.get_mut::<DeviceLinkSource>(source) {
        s.linked_ports.remove(&sink);
        for outputs in s.outputs.values_mut() {
            outputs.remove(&sink);
        }
    }
}

/// Связать или развязать пару портов (`ToggleLink`). `false` — не вышло.
pub fn toggle_link(
    world: &mut World,
    user: Option<Entity>,
    source: Entity,
    sink: Entity,
    source_port: &str,
    sink_port: &str,
) -> bool {
    if world.get::<DeviceLinkSource>(source).is_none()
        || world.get::<DeviceLinkSink>(sink).is_none()
    {
        return false;
    }
    let link = DeviceLink::new(source_port, sink_port);
    if links_of(world, source, sink).contains(&link) {
        world.raise(
            source,
            &mut SourcePortDisconnected {
                port: source_port.into(),
            },
        );
        world.raise(
            sink,
            &mut SinkPortDisconnected {
                port: sink_port.into(),
            },
        );
        let mut empty = false;
        if let Some(mut s) = world.get_mut::<DeviceLinkSource>(source) {
            if let Some(outputs) = s.outputs.get_mut(source_port) {
                outputs.remove(&sink);
            }
            if let Some(links) = s.linked_ports.get_mut(&sink) {
                links.remove(&link);
                empty = links.is_empty();
            }
            if empty {
                s.linked_ports.remove(&sink);
            }
        }
        if !empty {
            return true;
        }
        if let Some(mut s) = world.get_mut::<DeviceLinkSink>(sink) {
            s.linked_sources.remove(&source);
        }
        link_popup(world, user, source, source_port, sink, sink_port, true);
    } else {
        let valid = world
            .get::<DeviceLinkSource>(source)
            .is_some_and(|s| s.ports.contains(source_port))
            && world
                .get::<DeviceLinkSink>(sink)
                .is_some_and(|s| s.ports.contains(sink_port));
        if !valid || !can_link(world, user, source, sink, source_port, sink_port, true) {
            return false;
        }
        if let Some(mut s) = world.get_mut::<DeviceLinkSource>(source) {
            s.outputs
                .entry(source_port.into())
                .or_default()
                .insert(sink);
            s.linked_ports.entry(sink).or_default().insert(link);
        }
        if let Some(mut s) = world.get_mut::<DeviceLinkSink>(sink) {
            s.linked_sources.insert(source);
        }
        send_new_link(world, user, source, source_port, sink, sink_port);
        link_popup(world, user, source, source_port, sink, sink_port, false);
    }
    true
}

fn link_popup(
    world: &mut World,
    user: Option<Entity>,
    source: Entity,
    source_port: &str,
    sink: Entity,
    sink_port: &str,
    removed: bool,
) {
    if user.is_none() {
        return;
    }
    let key = if removed {
        "signal-linker-component-unlinked-port"
    } else {
        "signal-linker-component-linked-port"
    };
    let machine1 = entity_name(world, source);
    let machine2 = entity_name(world, sink);
    let port1 = port_name(world, "sourcePort", source_port);
    let port2 = port_name(world, "sinkPort", sink_port);
    popup_cursor(
        world,
        user,
        key,
        &[
            ("machine1", &machine1),
            ("port1", &port1),
            ("machine2", &machine2),
            ("port2", &port2),
        ],
    );
}

// Сигналы.

fn current_tick(world: &World) -> u32 {
    world.get_resource::<GameTime>().map_or(0, |t| t.tick.0)
}

/// Сколько раз приёмник дёрнули с учётом убывания (`GetEffectiveInvokeCounter`).
pub fn effective_invoke_counter(world: &World, sink: &DeviceLinkSink) -> i32 {
    let tick = current_tick(world);
    if tick < sink.invoke_counter_tick {
        return 0;
    }
    let delta = i64::from(tick - sink.invoke_counter_tick);
    if delta >= i64::from(sink.invoke_counter) {
        return 0;
    }
    (i64::from(sink.invoke_counter) - delta).max(0) as i32
}

fn set_invoke_counter(world: &mut World, sink: Entity, value: i32) {
    let tick = current_tick(world);
    if let Some(mut s) = world.get_mut::<DeviceLinkSink>(sink) {
        s.invoke_counter_tick = tick;
        s.invoke_counter = value;
    }
}

/// Дёрнуть выходной порт (`InvokePort`): импульс без уровня.
pub fn invoke_port(world: &mut World, source: Entity, port: &str) {
    invoke(world, source, port, None);
}

fn invoke(world: &mut World, source: Entity, port: &str, state: Option<SignalState>) {
    let Some(sinks) = world
        .get::<DeviceLinkSource>(source)
        .and_then(|s| s.outputs.get(port).cloned())
    else {
        return;
    };
    for sink in sinks {
        if world.get::<DeviceLinkSink>(sink).is_none() {
            continue;
        }
        for link in links_of(world, source, sink) {
            if link.source == port {
                invoke_direct(world, source, sink, &link.sink, state);
            }
        }
    }
}

/// Сигнал одному приёмнику (`InvokeDirect`): перегрузка, затем событие или пакет.
fn invoke_direct(
    world: &mut World,
    source: Entity,
    sink: Entity,
    sink_port: &str,
    state: Option<SignalState>,
) {
    let Some(component) = world.get::<DeviceLinkSink>(sink) else {
        return;
    };
    let counter = effective_invoke_counter(world, component);
    if counter > component.invoke_limit {
        set_invoke_counter(world, sink, 0);
        world.raise(sink, &mut DeviceLinkOverloaded);
        remove_all_from_sink(world, sink);
        return;
    }
    set_invoke_counter(world, sink, counter + 1);
    let sink_network = world
        .get::<DeviceNetwork>(sink)
        .map(|d| (d.address, d.receive_frequency));
    let Some((address, frequency)) =
        sink_network.filter(|_| world.get::<DeviceNetwork>(source).is_some())
    else {
        // Без сети — сразу событием, и уровень сигнала не передаётся (как в SS14).
        world.raise(
            sink,
            &mut SignalReceived {
                port: sink_port.into(),
                trigger: Some(source),
            },
        );
        return;
    };
    match state {
        None => {
            let payload = SignalPayload {
                invoked_port: sink_port.into(),
            };
            send_packet(
                world,
                source,
                Some(address),
                payload,
                frequency,
                Some(WIRELESS),
            );
        }
        Some(state) => {
            let payload = LogicSignalPayload {
                invoked_port: sink_port.into(),
                state,
            };
            send_packet(
                world,
                source,
                Some(address),
                payload,
                frequency,
                Some(WIRELESS),
            );
        }
    }
}

/// Логический сигнал на порт (`SendSignal`): уровень запоминается.
pub fn send_signal(world: &mut World, source: Entity, port: &str, signal: bool) {
    if world.get::<DeviceLinkSource>(source).is_none() {
        return;
    }
    invoke(world, source, port, Some(SignalState::of(signal)));
    if let Some(mut s) = world.get_mut::<DeviceLinkSource>(source) {
        s.last_signals.insert(port.into(), signal);
    }
}

/// Забыть уровень порта (`ClearSignal`).
pub fn clear_signal(world: &mut World, source: Entity, port: &str) {
    if let Some(mut s) = world.get_mut::<DeviceLinkSource>(source) {
        s.last_signals.remove(port);
    }
}

// Связи из карты.

/// Связи источника из сохранённого поля `linkedPorts`: номер приёмника → пары портов.
/// Связи, изменённые в игре, — в сохранённую карту номерами её сущностей (ADR-0014).
/// Связи, совпадающие с загруженными, остаются как в файле.
pub fn write_references(world: &mut World, save: &mut astrocore_map::MapSave) {
    let mut sources: Vec<(Entity, BTreeMap<Entity, BTreeSet<DeviceLink>>)> = world
        .query::<(Entity, &DeviceLinkSource)>()
        .iter(world)
        .filter(|(entity, _)| save.uids.contains_key(entity))
        .map(|(entity, source)| (entity, source.linked_ports.clone()))
        .collect();
    sources.sort_by_key(|(entity, _)| *entity);
    for (source, linked) in sources {
        let current: BTreeMap<u32, BTreeSet<DeviceLink>> = linked
            .into_iter()
            .filter(|(_, links)| !links.is_empty())
            .filter_map(|(sink, links)| Some((*save.uids.get(&sink)?, links)))
            .collect();
        let preserved: BTreeMap<u32, BTreeSet<DeviceLink>> = preserved_links(world, source)
            .into_iter()
            .map(|(uid, links)| (uid, links.into_iter().collect()))
            .filter(|(_, links): &(u32, BTreeSet<DeviceLink>)| !links.is_empty())
            .collect();
        if current == preserved {
            continue;
        }
        let value = (!current.is_empty()).then(|| {
            let mut map = astrocore_data::DataMap::new();
            for (uid, links) in &current {
                let pairs = links
                    .iter()
                    .map(|link| {
                        DataNode::seq(vec![
                            DataNode::plain(link.source.clone()),
                            DataNode::plain(link.sink.clone()),
                        ])
                    })
                    .collect();
                map.insert(uid.to_string(), DataNode::seq(pairs));
            }
            DataNode::map(map)
        });
        crate::map_commands::set_saved_component_field(
            &mut save.document,
            save.uids[&source],
            "DeviceLinkSource",
            "linkedPorts",
            value,
        );
    }
}

fn preserved_links(world: &World, source: Entity) -> Vec<(u32, Vec<DeviceLink>)> {
    let Some(preserved) = world.get::<MapPreserved>(source) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (_, fields) in preserved
        .fields
        .iter()
        .filter(|(name, _)| name == "DeviceLinkSource")
    {
        let Some(map) = fields.get("linkedPorts").and_then(DataNode::as_map) else {
            continue;
        };
        for (uid, pairs) in map.iter() {
            let Ok(uid) = uid.parse::<u32>() else {
                continue;
            };
            let links = pairs
                .as_seq()
                .unwrap_or_default()
                .iter()
                .filter_map(|pair| {
                    let pair = pair.as_seq()?;
                    Some(DeviceLink::new(
                        pair.first()?.as_str()?,
                        pair.get(1)?.as_str()?,
                    ))
                })
                .collect();
            out.push((uid, links));
        }
    }
    out
}

/// Связи из карты (ADR-0010), с проверкой как при запуске источника (`OnSourceStartup`):
/// неизвестный приёмник или порт отбрасывается.
fn adopt_links(world: &mut World, loaded: &MapLoaded) {
    let mut sources: Vec<Entity> = loaded
        .uids
        .values()
        .copied()
        .filter(|e| world.get::<DeviceLinkSource>(*e).is_some())
        .collect();
    sources.sort();
    for source in sources {
        for (uid, links) in preserved_links(world, source) {
            let Some(sink) = loaded.uids.get(&uid).copied() else {
                continue;
            };
            let Some(sink_ports) = world.get::<DeviceLinkSink>(sink).map(|s| s.ports.clone())
            else {
                continue;
            };
            let Some(mut component) = world.get_mut::<DeviceLinkSource>(source) else {
                continue;
            };
            let mut kept = BTreeSet::new();
            for link in links {
                if sink_ports.contains(&link.sink) && component.ports.contains(&link.source) {
                    component
                        .outputs
                        .entry(link.source.clone())
                        .or_default()
                        .insert(sink);
                    kept.insert(link);
                }
            }
            if kept.is_empty() {
                continue;
            }
            component.linked_ports.insert(sink, kept);
            if let Some(mut s) = world.get_mut::<DeviceLinkSink>(sink) {
                s.linked_sources.insert(source);
            }
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    use astrocore_ecs::RegistryWorldExt;
    world.register_data_component::<DeviceLinkSource>();
    world.register_data_component::<DeviceLinkSink>();
    if !side.is_server() {
        return;
    }
    // Источник или приёмник удалён — связи рвутся (`OnSourceRemoved`, `OnSinkRemoved`).
    world.subscribe::<DeviceLinkSource, EntityTerminating>(|world, source, _| {
        let sinks: Vec<Entity> = world
            .get::<DeviceLinkSource>(source)
            .map(|s| s.linked_ports.keys().copied().collect())
            .unwrap_or_default();
        for sink in sinks {
            if world.get::<DeviceLinkSink>(sink).is_some() {
                remove_sink_from_source_internal(world, source, sink);
            }
        }
    });
    world.subscribe::<DeviceLinkSink, EntityTerminating>(|world, sink, _| {
        let sources: Vec<Entity> = world
            .get::<DeviceLinkSink>(sink)
            .map(|s| s.linked_sources.iter().copied().collect())
            .unwrap_or_default();
        for source in sources {
            if world.get::<DeviceLinkSource>(source).is_some() {
                remove_sink_from_source_internal(world, source, sink);
            }
        }
    });
    // Пакеты сигналов (`OnPacketReceived`).
    world.subscribe::<DeviceLinkSink, DeviceNetworkPacket<SignalPayload>>(|world, sink, packet| {
        let port = packet.data.invoked_port.clone();
        if !world
            .get::<DeviceLinkSink>(sink)
            .is_some_and(|s| s.ports.contains(&port))
        {
            return;
        }
        let trigger = Some(packet.sender);
        world.raise(sink, &mut SignalReceived { port, trigger });
    });
    world.subscribe::<DeviceLinkSink, DeviceNetworkPacket<LogicSignalPayload>>(
        |world, sink, packet| {
            let port = packet.data.invoked_port.clone();
            if !world
                .get::<DeviceLinkSink>(sink)
                .is_some_and(|s| s.ports.contains(&port))
            {
                return;
            }
            let state = packet.data.state;
            let trigger = Some(packet.sender);
            world.raise(
                sink,
                &mut LogicSignalReceived {
                    port,
                    state,
                    trigger,
                },
            );
        },
    );
    // Новой связи — последний уровень порта (`OnNewLink`).
    world.subscribe::<DeviceLinkSource, NewLink>(|world, entity, event| {
        if event.source != entity {
            return;
        }
        let Some(signal) = world
            .get::<DeviceLinkSource>(entity)
            .and_then(|s| s.last_signals.get(&event.source_port).copied())
        else {
            return;
        };
        let (sink, port) = (event.sink, event.sink_port.clone());
        invoke_direct(world, entity, sink, &port, Some(SignalState::of(signal)));
    });
    world.subscribe_broadcast::<MapLoaded>(|world, event| adopt_links(world, event));
}

#[cfg(test)]
mod tests;
