//! SS14 PowerMonitoringConsoleSystem: one-second samples, device collections,
//! battery balance, focused source/load attribution, and cable overlays.
use std::collections::{BTreeMap, BTreeSet};

use astrocore_api::HookContext;
use astrocore_core::{NetworkEntityId, ObjectId, PrototypeId};
use astrocore_math::glam::Vec3;
use astrocore_world::AuthoredKind;
use prototypes::{Data, PrototypeRegistry};
use serde::{Deserialize, Serialize};

use crate::module::{NAMESPACE, State};

pub const COMPONENT: &str = "build.powermonitor";
pub const MAP_COMPONENT: &str = "build.powermonitor.map";
pub const CABLE_COMPONENT: &str = "build.powermonitor.cables";
pub const MAX_BYTES: usize = 16 * 1024;
pub use crate::wire::{CMD_POWER_CLOSE as CMD_CLOSE, CMD_POWER_FOCUS as CMD_FOCUS};
const UPDATE_S: f32 = 1.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Group {
    #[default]
    Generator,
    #[serde(rename = "SMES")]
    Smes,
    Substation,
    #[serde(rename = "APC")]
    Apc,
    Consumer,
}

impl Group {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "Generator" => Some(Self::Generator),
            "SMES" => Some(Self::Smes),
            "Substation" => Some(Self::Substation),
            "APC" => Some(Self::Apc),
            "Consumer" => Some(Self::Consumer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Spec {
    name: String,
    group: Group,
    collection: String,
    icon: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: ObjectId,
    pub power: f32,
    pub battery: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub console: ObjectId,
    pub group: Group,
    pub focus: Option<ObjectId>,
    pub total_sources: f32,
    pub total_battery_usage: f32,
    pub total_loads: f32,
    pub entries: Vec<Entry>,
    pub sources: Vec<Entry>,
    pub loads: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Kind {
    pub name: String,
    pub group: Group,
    pub icon: String,
}

/// Object, map X/Z, kind palette index, collection master, collection size.
pub type MapNode = (ObjectId, [f32; 2], u16, Option<ObjectId>, u16);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapView {
    pub console: ObjectId,
    pub station: String,
    pub kinds: Vec<Kind>,
    pub nodes: Vec<MapNode>,
}

/// Chunk origin and three 25-bit masks: HV, MV, APC. SS14 uses 5x5 chunks.
pub type CableChunk = [i32; 5];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CableView {
    pub console: ObjectId,
    pub all: Vec<CableChunk>,
    pub focus: Vec<CableChunk>,
}

pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusRequest {
    pub group: Group,
    pub device: Option<ObjectId>,
}

pub fn focus_data(group: Group, device: Option<ObjectId>) -> Vec<u8> {
    serde_json::to_vec(&FocusRequest { group, device }).unwrap_or_default()
}

#[derive(Default)]
pub(crate) struct Monitoring {
    pub consoles: BTreeSet<PrototypeId>,
    specs: BTreeMap<PrototypeId, Spec>,
    sessions: BTreeMap<NetworkEntityId, Session>,
    sent: BTreeMap<(NetworkEntityId, &'static str), Vec<u8>>,
    pub station: String,
}

#[derive(Clone)]
struct Session {
    console: ObjectId,
    group: Group,
    focus: Option<ObjectId>,
    next_sample: f32,
}

impl Monitoring {
    pub fn load(registry: &PrototypeRegistry) -> Self {
        let mut result = Self::default();
        let Ok(data) = registry.resolve("PowerMonitoringData") else {
            return result;
        };
        let Some(data) = data.component("PowerMonitoring") else {
            return result;
        };
        for name in data
            .get("consoles")
            .and_then(Data::as_list)
            .unwrap_or_default()
        {
            if let Some(id) = name
                .as_str()
                .and_then(|n| prototypes::engine_id(NAMESPACE, n))
            {
                result.consoles.insert(id);
            }
        }
        for row in data
            .get("devices")
            .and_then(Data::as_list)
            .unwrap_or_default()
        {
            let string = |key| row.get(key).and_then(Data::as_str).unwrap_or_default();
            let (Some(id), Some(group)) = (
                prototypes::engine_id(NAMESPACE, string("prototype")),
                Group::parse(string("group")),
            ) else {
                continue;
            };
            let icon = if string("sprite").is_empty() || string("state").is_empty() {
                String::new()
            } else {
                format!("Textures/{}/{}.png", string("sprite"), string("state"))
            };
            result.specs.insert(
                id,
                Spec {
                    name: string("name").into(),
                    group,
                    collection: string("collection").into(),
                    icon,
                },
            );
        }
        result
    }

    pub fn forget(&mut self, user: NetworkEntityId) {
        self.sessions.remove(&user);
        self.sent.retain(|(owner, _), _| *owner != user);
    }
}

#[derive(Clone)]
struct Device {
    id: ObjectId,
    at: [f32; 2],
    spec: Spec,
    source: Option<usize>,
    load: Option<usize>,
    battery_index: Option<usize>,
    entry: Entry,
    supplied: f32,
    usage: f32,
    battery_usage: f32,
}

impl State {
    fn power_monitor_available(
        &self,
        ctx: &HookContext<'_>,
        user: NetworkEntityId,
        id: ObjectId,
    ) -> bool {
        let (Some(object), Some(feet)) = (ctx.world.authored_object(id), ctx.position_of(user))
        else {
            return false;
        };
        let AuthoredKind::Placement { prototype } = &object.kind else {
            return false;
        };
        let at = (object.min() + object.max()) * 0.5;
        self.power_monitor.consoles.contains(prototype)
            && !self
                .actors
                .get(&user)
                .is_some_and(|actor| !actor.health.state().can_act() || actor.ghost)
            && feet.distance(at) <= self.range_m + 1.0
            && ctx.is_unobstructed(feet + Vec3::Y, at)
            && (!self.power_required || self.powered_map().get(&id) == Some(&true))
    }

    pub(crate) fn open_power_monitor(
        &mut self,
        ctx: &HookContext<'_>,
        user: NetworkEntityId,
        id: ObjectId,
    ) -> bool {
        if self.power_monitor_available(ctx, user, id)
            && !self
                .power_monitor
                .sessions
                .iter()
                .any(|(owner, session)| *owner != user && session.console == id)
        {
            self.power_monitor.sessions.insert(
                user,
                Session {
                    console: id,
                    group: Group::Generator,
                    focus: None,
                    next_sample: self.clock,
                },
            );
        }
        true
    }

    pub(crate) fn close_power_monitor(&mut self, user: NetworkEntityId) {
        self.power_monitor.sessions.remove(&user);
    }

    pub(crate) fn focus_power_monitor(
        &mut self,
        ctx: &HookContext<'_>,
        user: NetworkEntityId,
        data: &[u8],
    ) {
        let Some(request) = decode::<FocusRequest>(data).filter(|_| data.len() <= 128) else {
            return;
        };
        let Some(session) = self.power_monitor.sessions.get(&user).cloned() else {
            return;
        };
        if !self.power_monitor_available(ctx, user, session.console) {
            return;
        }
        if let Some(id) = request.device {
            let devices = self.monitored_devices(ctx, session.console);
            if !devices.iter().any(|d| d.id == id) {
                return;
            }
        }
        if let Some(session) = self.power_monitor.sessions.get_mut(&user) {
            session.group = request.group;
            session.focus = request.device;
            session.next_sample = self.clock;
        }
    }

    fn monitored_devices(&self, ctx: &HookContext<'_>, console: ObjectId) -> Vec<Device> {
        let Some(console) = ctx.world.authored_object(console) else {
            return Vec::new();
        };
        let suppliers: BTreeMap<_, _> = self.power_suppliers.iter().copied().collect();
        let batteries: BTreeMap<_, _> = self.power_batteries.iter().copied().collect();
        let mut result = Vec::new();
        for object in ctx.world.authored().filter(|o| o.floor == console.floor) {
            let AuthoredKind::Placement { prototype } = &object.kind else {
                continue;
            };
            let Some(spec) = self.power_monitor.specs.get(prototype) else {
                continue;
            };
            let at = (object.min() + object.max()) * 0.5;
            let mut device = Device {
                id: object.id,
                at: [at.x, at.z],
                spec: spec.clone(),
                source: None,
                load: None,
                battery_index: batteries.get(&object.id).copied(),
                entry: Entry {
                    id: object.id,
                    power: 0.0,
                    battery: None,
                },
                supplied: 0.0,
                usage: 0.0,
                battery_usage: 0.0,
            };
            if let Some(index) = suppliers.get(&object.id) {
                device.entry.power = self.power.supplies[*index].current_supply;
                device.supplied = device.entry.power;
                device.load = self
                    .power
                    .networks
                    .iter()
                    .position(|n| n.supplies.contains(index));
            }
            if let Some(index) = device.battery_index {
                let battery = &self.power.batteries[index];
                device.source = battery.charging;
                device.load = battery.discharging;
                device.entry.power = battery.current_supply;
                device.entry.battery = Some(battery.current_storage / battery.capacity.max(1.0));
                device.usage = (battery.current_receiving - battery.current_supply).max(0.0);
                device.battery_usage =
                    (battery.current_supply - battery.current_receiving).max(0.0);
                if spec.group == Group::Apc && battery.enabled {
                    device.usage += battery.loading_network_demand;
                }
            } else if let Some(battery) = self
                .power_parts
                .get(prototype)
                .and_then(|p| p.battery.as_ref())
            {
                let charge = self
                    .power_charge
                    .get(&object.id)
                    .copied()
                    .unwrap_or(battery.starting_charge);
                device.entry.battery = Some(charge / battery.capacity.max(1.0));
            }
            result.push(device);
        }
        result.sort_by_key(|d| d.id);
        result
    }

    fn power_monitor_snapshot(
        &self,
        ctx: &HookContext<'_>,
        session: &Session,
    ) -> (View, MapView, CableView) {
        let devices = self.monitored_devices(ctx, session.console);
        let mut masters = BTreeMap::new();
        let mut collections = BTreeMap::new();
        for d in &devices {
            let master =
                if !d.spec.collection.is_empty() && (d.source.is_some() || d.load.is_some()) {
                    *collections
                        .entry((d.spec.collection.clone(), d.source, d.load))
                        .or_insert(d.id)
                } else {
                    d.id
                };
            masters.insert(d.id, master);
        }
        let master_of = |id: ObjectId| masters.get(&id).copied().unwrap_or(id);
        let mut entries: BTreeMap<ObjectId, Entry> = BTreeMap::new();
        for d in &devices {
            let master = master_of(d.id);
            let entry = entries.entry(master).or_insert(Entry {
                id: master,
                power: 0.0,
                battery: d.entry.battery,
            });
            entry.power += d.entry.power;
        }
        let focus = session
            .focus
            .map(master_of)
            .filter(|id| entries.contains_key(id));
        let selected = devices.iter().find(|d| Some(d.id) == focus);
        let selected_members: Vec<_> = devices
            .iter()
            .filter(|d| Some(master_of(d.id)) == focus)
            .collect();
        let mut sources: BTreeMap<ObjectId, f32> = BTreeMap::new();
        let mut loads: BTreeMap<ObjectId, f32> = BTreeMap::new();
        if let Some(selected) = selected {
            if let Some(index) = selected.source {
                let net = &self.power.networks[index];
                let supplied: f32 = net
                    .supplies
                    .iter()
                    .map(|i| self.power.supplies[*i].current_supply)
                    .sum::<f32>()
                    + net
                        .battery_supplies
                        .iter()
                        .map(|i| self.power.batteries[*i].current_supply)
                        .sum::<f32>();
                let demand: f32 = net
                    .loads
                    .iter()
                    .map(|i| self.power.loads[*i].receiving_power)
                    .sum::<f32>()
                    + net
                        .battery_loads
                        .iter()
                        .map(|i| self.power.batteries[*i].current_receiving)
                        .sum::<f32>();
                let receiving: f32 = selected_members
                    .iter()
                    .filter_map(|d| d.battery_index)
                    .map(|i| self.power.batteries[i].current_receiving)
                    .sum();
                let fraction = if supplied > 0.0 && demand > 0.0 {
                    (receiving / supplied).min(1.0) * (supplied / demand).min(1.0)
                } else {
                    1.0
                };
                for d in &devices {
                    if d.load == Some(index) && master_of(d.id) != selected.id {
                        *sources.entry(master_of(d.id)).or_default() += d.entry.power * fraction;
                    }
                }
            }
            if let Some(index) = selected.load {
                let net = &self.power.networks[index];
                let demand: f32 = net
                    .loads
                    .iter()
                    .map(|i| self.power.loads[*i].receiving_power)
                    .sum::<f32>()
                    + net
                        .battery_loads
                        .iter()
                        .map(|i| self.power.batteries[*i].current_receiving)
                        .sum::<f32>();
                let supplied: f32 = selected_members.iter().map(|d| d.entry.power).sum();
                let fraction = if demand > 0.0 {
                    (supplied / demand).min(1.0)
                } else {
                    1.0
                };
                for d in &devices {
                    if d.source == Some(index) && master_of(d.id) != selected.id {
                        let receiving = d
                            .battery_index
                            .map_or(0.0, |i| self.power.batteries[i].current_receiving);
                        *loads.entry(master_of(d.id)).or_default() += receiving * fraction;
                    }
                }
            }
        }
        let focused_entries = |values: BTreeMap<ObjectId, f32>| {
            values
                .into_iter()
                .map(|(id, power)| Entry {
                    id,
                    power,
                    battery: entries.get(&id).and_then(|e| e.battery),
                })
                .collect()
        };
        let view = View {
            console: session.console,
            group: session.group,
            focus,
            total_sources: devices.iter().map(|d| d.supplied).sum(),
            total_battery_usage: devices.iter().map(|d| d.battery_usage).sum(),
            total_loads: devices.iter().map(|d| d.usage).sum(),
            entries: devices
                .iter()
                .filter(|d| master_of(d.id) == d.id && d.spec.group == session.group)
                .filter_map(|d| entries.get(&d.id).cloned())
                .collect(),
            sources: focused_entries(sources),
            loads: focused_entries(loads),
        };
        let mut map = MapView {
            console: session.console,
            station: self.power_monitor.station.clone(),
            kinds: Vec::new(),
            nodes: Vec::new(),
        };
        for d in &devices {
            let kind = Kind {
                name: d.spec.name.clone(),
                group: d.spec.group,
                icon: d.spec.icon.clone(),
            };
            let index = map
                .kinds
                .iter()
                .position(|k| *k == kind)
                .unwrap_or_else(|| {
                    map.kinds.push(kind);
                    map.kinds.len() - 1
                });
            let master = master_of(d.id);
            let count = devices
                .iter()
                .filter(|other| master_of(other.id) == d.id)
                .count();
            map.nodes.push((
                d.id,
                d.at,
                index as u16,
                (master != d.id).then_some(master),
                count as u16,
            ));
        }
        let floor = ctx
            .world
            .authored_object(session.console)
            .map(|o| o.floor.0)
            .unwrap_or(0);
        let mut chunks = BTreeMap::new();
        let mut focused = BTreeMap::new();
        for (group, cell, network) in &self.power_cables {
            let z = cell.1 - floor * atmos::FLOOR_STRIDE;
            if !(-atmos::FLOOR_STRIDE / 2..atmos::FLOOR_STRIDE / 2).contains(&z) {
                continue;
            }
            let origin = (cell.0.div_euclid(5) * 5, z.div_euclid(5) * 5);
            let bit = 1 << (cell.0.rem_euclid(5) * 5 + z.rem_euclid(5));
            let index = *group as usize;
            let chunk = chunks
                .entry(origin)
                .or_insert([origin.0, origin.1, 0, 0, 0]);
            chunk[2 + index] |= bit;
            if selected.is_some_and(|d| d.source == Some(*network) || d.load == Some(*network)) {
                let chunk = focused
                    .entry(origin)
                    .or_insert([origin.0, origin.1, 0, 0, 0]);
                chunk[2 + index] |= bit;
            }
        }
        let cables = CableView {
            console: session.console,
            all: chunks.into_values().collect(),
            focus: focused.into_values().collect(),
        };
        (view, map, cables)
    }

    pub(crate) fn tick_power_monitors(&mut self, ctx: &mut HookContext<'_>) {
        if self.power_monitor.consoles.is_empty() {
            return;
        }
        let players: BTreeSet<_> = ctx.players().collect();
        self.power_monitor
            .sessions
            .retain(|user, _| players.contains(user));
        self.power_monitor
            .sent
            .retain(|(user, _), _| players.contains(user));
        for user in players {
            let session = self.power_monitor.sessions.get(&user).cloned();
            let valid = session
                .as_ref()
                .is_some_and(|s| self.power_monitor_available(ctx, user, s.console));
            if !valid {
                self.power_monitor.sessions.remove(&user);
            } else if session.as_ref().is_some_and(|s| self.clock < s.next_sample) {
                continue;
            }
            let values = if valid {
                let (view, map, cables) =
                    self.power_monitor_snapshot(ctx, session.as_ref().unwrap());
                if let Some(session) = self.power_monitor.sessions.get_mut(&user) {
                    session.next_sample = self.clock + UPDATE_S
                }
                vec![
                    (COMPONENT, serde_json::to_vec(&view).unwrap()),
                    (MAP_COMPONENT, serde_json::to_vec(&map).unwrap()),
                    (CABLE_COMPONENT, serde_json::to_vec(&cables).unwrap()),
                ]
            } else {
                vec![
                    (COMPONENT, Vec::new()),
                    (MAP_COMPONENT, Vec::new()),
                    (CABLE_COMPONENT, Vec::new()),
                ]
            };
            for (kind, bytes) in values {
                if self.power_monitor.sent.get(&(user, kind)) == Some(&bytes) {
                    continue;
                }
                ctx.set_component(user, kind, bytes.clone());
                self.power_monitor.sent.insert((user, kind), bytes);
            }
        }
    }
}
