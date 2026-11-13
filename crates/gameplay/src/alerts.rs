//! SS14 AtmosAlertsComputerSystem, AtmosAlarmableSystem and FireAlarmSystem.
//! DeviceList links are imported by entity UID. Air alarms read registered sensors;
//! silencing belongs to a console and never resets an alarmable's network state.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use astrocore_api::HookContext;
use astrocore_core::{NetworkEntityId, ObjectId, PrototypeId};
use astrocore_math::glam::Vec3;
use astrocore_world::AuthoredKind;
use atmos::{Gas, GasMixture};
use prototypes::{Data, PrototypeRegistry};
use serde::{Deserialize, Serialize};

use crate::{
    module::{NAMESPACE, State},
    monitor::Threshold,
};

pub const COMPONENT: &str = "build.atmosalerts";
pub const VISUAL_COMPONENT: &str = "build.atmosalerts.visuals";
pub const MAX_BYTES: usize = 16 * 1024;
pub const CMD_FOCUS: &str = "atmosalerts.focus";
pub const CMD_SILENCE: &str = "atmosalerts.silence";
pub const CMD_CLOSE: &str = "atmosalerts.close";
const GASES: [Gas; 9] = [
    Gas::Oxygen,
    Gas::Nitrogen,
    Gas::CarbonDioxide,
    Gas::Plasma,
    Gas::Tritium,
    Gas::WaterVapor,
    Gas::Ammonia,
    Gas::NitrousOxide,
    Gas::Frezon,
];
const GAS_NAMES: [&str; 9] = [
    "Oxygen",
    "Nitrogen",
    "CarbonDioxide",
    "Plasma",
    "Tritium",
    "WaterVapor",
    "Ammonia",
    "NitrousOxide",
    "Frezon",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum Level {
    #[default]
    Invalid,
    Normal,
    Warning,
    Danger,
}
impl From<crate::monitor::AlarmState> for Level {
    fn from(value: crate::monitor::AlarmState) -> Self {
        match value {
            crate::monitor::AlarmState::Normal => Self::Normal,
            crate::monitor::AlarmState::Warning => Self::Warning,
            crate::monitor::AlarmState::Danger => Self::Danger,
        }
    }
}
impl Level {
    pub(crate) fn alarm_state(self) -> crate::monitor::AlarmState {
        match self {
            Self::Warning => crate::monitor::AlarmState::Warning,
            Self::Danger => crate::monitor::AlarmState::Danger,
            _ => crate::monitor::AlarmState::Normal,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Group {
    AirAlarm,
    FireAlarm,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: ObjectId,
    pub at: [f32; 2],
    pub group: Group,
    pub level: Level,
    pub name: String,
    pub address: String,
    pub silenced: bool,
    pub seeds: Vec<[i32; 2]>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub temperature: (f32, Level),
    pub pressure: (f32, Level),
    /// Gas index, average moles, fraction of total sensor moles, threshold state.
    pub gases: Vec<(u8, f32, f32, Level)>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub console: ObjectId,
    pub station: String,
    pub entries: Vec<Entry>,
    pub focus: Option<ObjectId>,
    pub reading: Option<Reading>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Visuals {
    pub states: Vec<(ObjectId, Level)>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Focus {
    pub device: Option<ObjectId>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Silence {
    pub device: ObjectId,
    pub silenced: bool,
}
pub fn focus_data(device: Option<ObjectId>) -> Vec<u8> {
    serde_json::to_vec(&Focus { device }).unwrap_or_default()
}
pub fn silence_data(device: ObjectId, silenced: bool) -> Vec<u8> {
    serde_json::to_vec(&Silence { device, silenced }).unwrap_or_default()
}
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    (!bytes.is_empty() && bytes.len() <= MAX_BYTES)
        .then(|| serde_json::from_slice(bytes).ok())
        .flatten()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Console,
    AirAlarm,
    FireAlarm,
    Sensor,
}
impl Role {
    fn group(self) -> Option<Group> {
        match self {
            Self::AirAlarm => Some(Group::AirAlarm),
            Self::FireAlarm => Some(Group::FireAlarm),
            _ => None,
        }
    }
}
#[derive(Clone)]
struct Spec {
    role: Role,
    name: String,
    tags: BTreeSet<String>,
    sync: BTreeSet<String>,
    temperature_only: bool,
    receive_only: bool,
    visual: bool,
    temperature: Threshold,
    pressure: Threshold,
    gases: BTreeMap<usize, Threshold>,
}
#[derive(Clone)]
struct Node {
    map_at: [f32; 2],
    id: ObjectId,
    prototype: PrototypeId,
    floor: i32,
    at: Vec3,
    cell: atmos::Cell,
    name: String,
    address: String,
    targets: BTreeSet<ObjectId>,
    on: bool,
}
#[derive(Clone)]
struct Sensor {
    air: GasMixture,
    level: Level,
    flags: u8,
}
#[derive(Default)]
struct Alarm {
    states: BTreeMap<ObjectId, Level>,
    last: Level,
    powered: bool,
}
#[derive(Default)]
struct Console {
    focus: Option<ObjectId>,
    silenced: BTreeSet<ObjectId>,
}
#[derive(Default)]
pub(crate) struct Alerts {
    specs: BTreeMap<PrototypeId, Spec>,
    nodes: BTreeMap<ObjectId, Node>,
    sensors: BTreeMap<ObjectId, Sensor>,
    alarms: BTreeMap<ObjectId, Alarm>,
    consoles: BTreeMap<ObjectId, Console>,
    sessions: BTreeMap<NetworkEntityId, ObjectId>,
    sent: BTreeMap<(NetworkEntityId, &'static str), Vec<u8>>,
    pub station: String,
    next_sample: f32,
}
fn threshold(row: &Data) -> Threshold {
    let number = |key| row.get(key).and_then(Data::as_f64).map(|n| n as f32);
    Threshold {
        upper: number("upperBound"),
        lower: number("lowerBound"),
        upper_warn: number("upperWarnAround"),
        lower_warn: number("lowerWarnAround"),
    }
}
impl Alerts {
    pub fn load(registry: &PrototypeRegistry) -> Self {
        let mut out = Self::default();
        let Ok(data) = registry.resolve("AtmosAlertsData") else {
            return out;
        };
        let Some(data) = data.component("AtmosAlerts") else {
            return out;
        };
        for row in data
            .get("devices")
            .and_then(Data::as_list)
            .unwrap_or_default()
        {
            let text = |key| row.get(key).and_then(Data::as_str).unwrap_or_default();
            let Some(id) = prototypes::engine_id(NAMESPACE, text("prototype")) else {
                continue;
            };
            let role = match text("role") {
                "Console" => Role::Console,
                "AirAlarm" => Role::AirAlarm,
                "FireAlarm" => Role::FireAlarm,
                "Sensor" => Role::Sensor,
                _ => continue,
            };
            let strings = |key| {
                row.get(key)
                    .and_then(Data::as_list)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Data::as_str)
                    .map(String::from)
                    .collect::<BTreeSet<_>>()
            };
            let mut gases = BTreeMap::new();
            for gas in row.get("gases").and_then(Data::as_list).unwrap_or_default() {
                if let Some(index) = gas
                    .get("gas")
                    .and_then(Data::as_str)
                    .and_then(|name| GAS_NAMES.iter().position(|n| *n == name))
                {
                    gases.insert(index, threshold(gas));
                }
            }
            out.specs.insert(
                id,
                Spec {
                    role,
                    name: text("name").into(),
                    tags: strings("tags"),
                    sync: strings("syncWith"),
                    temperature_only: strings("monitorAlertTypes").contains("Temperature"),
                    receive_only: row
                        .get("receiveOnly")
                        .and_then(Data::as_bool)
                        .unwrap_or(false),
                    visual: row.get("visual").and_then(Data::as_bool).unwrap_or(true),
                    temperature: row.get("temperature").map(threshold).unwrap_or_default(),
                    pressure: row.get("pressure").map(threshold).unwrap_or_default(),
                    gases,
                },
            );
        }
        out
    }
    pub(crate) fn is_alarm(&self, prototype: &PrototypeId) -> bool {
        self.specs
            .get(prototype)
            .is_some_and(|s| s.role.group().is_some())
    }
    pub(crate) fn level(&self, id: ObjectId) -> Option<Level> {
        self.nodes
            .get(&id)
            .filter(|n| self.specs[&n.prototype].role.group().is_some())
            .map(|n| {
                if n.on {
                    self.alarms.get(&id).map_or(Level::Normal, |a| a.last)
                } else {
                    Level::Invalid
                }
            })
    }
    pub fn forget(&mut self, user: NetworkEntityId) {
        self.sessions.remove(&user);
        self.sent.retain(|(owner, _), _| *owner != user);
    }
    fn sync_nodes(&mut self, state: &State, ctx: &HookContext<'_>) -> bool {
        let powered = state.powered_map();
        let uid: BTreeMap<_, _> = ctx
            .world
            .authored()
            .filter_map(|o| o.metadata.get("ss14_uid").map(|uid| (uid.as_str(), o.id)))
            .collect();
        let nodes: BTreeMap<ObjectId, Node> = ctx
            .world
            .authored()
            .filter_map(|o| {
                let AuthoredKind::Placement { prototype } = &o.kind else {
                    return None;
                };
                let spec = self.specs.get(prototype)?;
                let at = (o.min() + o.max()) * 0.5;
                let map_at = o
                    .metadata
                    .get("wallmount_origin")
                    .and_then(|value| {
                        let (x, z) = value.split_once(',')?;
                        Some([x.parse::<f32>().ok()?, z.parse::<f32>().ok()?])
                    })
                    .unwrap_or([at.x, at.z]);
                // Exact UID links are required for newly imported maps. The old cell list is
                // only a compatibility path for old maps which predate UID preservation.
                let targets = if let Some(ids) = o.metadata.get("device_ids") {
                    ids.split(';')
                        .filter_map(|id| uid.get(id).copied())
                        .collect()
                } else {
                    let cells: BTreeSet<_> = o
                        .metadata
                        .get("links")
                        .into_iter()
                        .flat_map(|s| s.split(';'))
                        .filter_map(|pair| {
                            let (x, z) = pair.split_once(',')?;
                            Some(atmos::floor_cell(
                                o.floor.0,
                                x.parse().ok()?,
                                z.parse().ok()?,
                            ))
                        })
                        .collect();
                    ctx.world
                        .authored()
                        .filter(|other| {
                            other.floor == o.floor
                                && cells.contains(&crate::piping::cell_at(
                                    other.floor.0,
                                    (other.min() + other.max()) * 0.5,
                                ))
                        })
                        .map(|other| other.id)
                        .collect()
                };
                Some((
                    o.id,
                    Node {
                        map_at,
                        id: o.id,
                        prototype: prototype.clone(),
                        floor: o.floor.0,
                        at,
                        cell: crate::piping::cell_at(o.floor.0, at),
                        name: o
                            .metadata
                            .get("name")
                            .cloned()
                            .unwrap_or_else(|| spec.name.clone()),
                        address: o
                            .metadata
                            .get("device_address")
                            .or_else(|| o.metadata.get("ss14_uid"))
                            .cloned()
                            .unwrap_or_else(|| o.id.to_string()),
                        targets,
                        on: !state.power_required || powered.get(&o.id) == Some(&true),
                    },
                ))
            })
            .collect();
        let changed = nodes.len() != self.nodes.len()
            || nodes.values().any(|node| {
                self.nodes.get(&node.id).is_none_or(|old| {
                    old.prototype != node.prototype
                        || old.cell != node.cell
                        || old.on != node.on
                        || old.targets != node.targets
                })
            });
        self.nodes = nodes;
        self.sensors.retain(|id, _| self.nodes.contains_key(id));
        self.alarms.retain(|id, _| self.nodes.contains_key(id));
        self.consoles.retain(|id, _| self.nodes.contains_key(id));
        for console in self.consoles.values_mut() {
            console.silenced.retain(|id| self.nodes.contains_key(id));
            console.focus = console.focus.filter(|id| self.nodes.contains_key(id));
        }
        changed
    }
    fn recipients(&self, sender: ObjectId) -> Vec<ObjectId> {
        let Some(node) = self.nodes.get(&sender) else {
            return Vec::new();
        };
        let spec = &self.specs[&node.prototype];
        self.nodes
            .values()
            .filter(|target| {
                let dest = &self.specs[&target.prototype];
                // DeviceNetworkSystem.SendToConnections skips the sender.
                target.id != sender
                    && target.on
                    && target.floor == node.floor
                    && dest.role.group().is_some()
                    && !dest.sync.is_disjoint(&spec.tags)
                    && if spec.role == Role::Sensor {
                        target.targets.contains(&sender)
                    } else {
                        node.targets.contains(&target.id)
                    }
            })
            .map(|n| n.id)
            .collect()
    }
    // AtmosAlarmableSyncAlertsPayload copies cached states; the threshold filter
    // applies only to direct monitor packets, just as in AtmosAlarmableSystem.
    fn deliver(
        &mut self,
        mut queue: VecDeque<(ObjectId, BTreeMap<ObjectId, Level>)>,
    ) -> Vec<(String, Vec3, f32)> {
        let mut sounds = Vec::new();
        while let Some((target, incoming)) = queue.pop_front() {
            let Some(node) = self.nodes.get(&target) else {
                continue;
            };
            if !node.on {
                continue;
            }
            let alarm = self.alarms.entry(target).or_default();
            alarm.states.extend(incoming);
            let next = alarm
                .states
                .values()
                .copied()
                .max()
                .unwrap_or(Level::Normal);
            if next == alarm.last {
                continue;
            }
            let states = alarm.states.clone();
            alarm.last = next;
            if next == Level::Danger {
                sounds.push(("Machines/alarm.ogg".into(), node.at, -10.0));
            }
            if !self.specs[&node.prototype].receive_only {
                for recipient in self.recipients(target) {
                    queue.push_back((recipient, states.clone()));
                }
            }
        }
        sounds
    }
    fn sensor_packet(
        &self,
        id: ObjectId,
        sensor: &Sensor,
        queue: &mut VecDeque<(ObjectId, BTreeMap<ObjectId, Level>)>,
    ) {
        for recipient in self.recipients(id) {
            let spec = &self.specs[&self.nodes[&recipient].prototype];
            let invalid = spec.temperature_only && sensor.flags & 1 == 0;
            if invalid
                && !self
                    .alarms
                    .get(&recipient)
                    .is_some_and(|a| a.states.contains_key(&id))
            {
                continue;
            }
            queue.push_back((
                recipient,
                [(id, if invalid { Level::Normal } else { sensor.level })].into(),
            ));
        }
    }
    fn sample(&mut self, state: &State, ctx: &HookContext<'_>) -> Vec<(String, Vec3, f32)> {
        let resync = self.sync_nodes(state, ctx);
        for node in self
            .nodes
            .values()
            .filter(|n| self.specs[&n.prototype].role.group().is_some())
        {
            let alarm = self.alarms.entry(node.id).or_insert_with(|| Alarm {
                last: Level::Normal,
                ..Default::default()
            });
            if !node.on {
                alarm.states.clear();
                alarm.last = Level::Normal;
            }
            alarm.powered = node.on;
        }
        let nodes: Vec<_> = self
            .nodes
            .values()
            .filter(|n| self.specs[&n.prototype].role == Role::Sensor && n.on)
            .cloned()
            .collect();
        let mut sounds = Vec::new();
        for node in nodes {
            let spec = &self.specs[&node.prototype];
            let air = state.atmos.air(node.cell);
            let temperature: Level = spec.temperature.check(air.temperature()).into();
            let pressure: Level = spec.pressure.check(air.pressure()).into();
            let mut level = temperature.max(pressure);
            let mut flags =
                u8::from(temperature > Level::Normal) | (u8::from(pressure > Level::Normal) << 1);
            if air.total_moles() > 0.0 {
                for (gas, threshold) in &spec.gases {
                    let gas_level: Level = threshold
                        .check(air.moles(GASES[*gas]) / air.total_moles())
                        .into();
                    if gas_level > level {
                        level = gas_level;
                        flags |= 4;
                    }
                }
            }
            let reading = Sensor { air, level, flags };
            // Sync requests refresh sensor data while a console is open as in GetFocusAlarmData.
            if self
                .sensors
                .get(&node.id)
                .is_none_or(|s| s.level != level || s.flags != flags)
                || resync
                || !self.sessions.is_empty()
            {
                // DeviceNetworkSystem dispatches one sender's packets before the
                // next monitor updates. Do not mix stale sync snapshots from
                // several sensors into the same delivery cascade.
                let mut queue = VecDeque::new();
                self.sensor_packet(node.id, &reading, &mut queue);
                sounds.extend(self.deliver(queue));
            }
            self.sensors.insert(node.id, reading);
        }
        sounds
    }
    fn reading(&self, node: &Node) -> Reading {
        let sensors: Vec<_> = node
            .targets
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .filter(|n| n.floor == node.floor)
            .filter_map(|n| Some((self.sensors.get(&n.id)?, self.specs.get(&n.prototype)?)))
            .collect();
        let count = sensors.len() as f32;
        let average = |sum: f32| if count > 0.0 { sum / count } else { 0.0 };
        let temperature = (
            average(sensors.iter().map(|(s, _)| s.air.temperature()).sum()),
            sensors
                .iter()
                .map(|(s, p)| Level::from(p.temperature.check(s.air.temperature())))
                .max()
                .unwrap_or(Level::Normal),
        );
        let pressure = (
            average(sensors.iter().map(|(s, _)| s.air.pressure()).sum()),
            sensors
                .iter()
                .map(|(s, p)| Level::from(p.pressure.check(s.air.pressure())))
                .max()
                .unwrap_or(Level::Normal),
        );
        let total: f32 = sensors.iter().map(|(s, _)| s.air.total_moles()).sum();
        let mut gases = Vec::new();
        for (index, gas) in GASES.into_iter().enumerate() {
            let moles: f32 = sensors.iter().map(|(s, _)| s.air.moles(gas)).sum();
            if total <= 1e-8 || average(moles) < 1e-8 {
                continue;
            }
            let fraction = moles / total;
            let level = sensors
                .iter()
                .filter_map(|(_, p)| p.gases.get(&index))
                .map(|t| Level::from(t.check(fraction)))
                .max()
                .unwrap_or(Level::Normal);
            gases.push((index as u8, average(moles), fraction, level));
        }
        Reading {
            temperature,
            pressure,
            gases,
        }
    }
    fn view(&self, console: ObjectId) -> View {
        let node = &self.nodes[&console];
        let settings = self.consoles.get(&console);
        let entries: Vec<_> = self
            .nodes
            .values()
            .filter(|n| n.floor == node.floor)
            .filter_map(|n| {
                let group = self.specs[&n.prototype].role.group()?;
                let seeds = n
                    .targets
                    .iter()
                    .filter_map(|id| self.nodes.get(id))
                    .filter(|sensor| {
                        sensor.floor == n.floor
                            && self.specs[&sensor.prototype].role == Role::Sensor
                    })
                    .map(|sensor| {
                        [
                            sensor.cell.0,
                            sensor.cell.1 - sensor.floor * atmos::FLOOR_STRIDE,
                        ]
                    })
                    .collect();
                Some(Entry {
                    id: n.id,
                    at: n.map_at,
                    group,
                    level: self.level(n.id).unwrap_or(Level::Invalid),
                    name: n.name.clone(),
                    address: n.address.clone(),
                    silenced: settings.is_some_and(|c| c.silenced.contains(&n.id)),
                    seeds,
                })
            })
            .collect();
        let focus = settings
            .and_then(|c| c.focus)
            .filter(|id| entries.iter().any(|e| e.id == *id));
        let reading = focus
            .and_then(|id| self.nodes.get(&id))
            .filter(|n| n.on && self.specs[&n.prototype].role == Role::AirAlarm)
            .map(|n| self.reading(n));
        View {
            console,
            station: self.station.clone(),
            entries,
            focus,
            reading,
        }
    }
    fn visuals(&self) -> Visuals {
        Visuals {
            states: self
                .nodes
                .values()
                .filter(|n| self.specs[&n.prototype].visual)
                .map(|n| {
                    let spec = &self.specs[&n.prototype];
                    let level = if !n.on {
                        Level::Invalid
                    } else if spec.role == Role::Console {
                        self.nodes
                            .values()
                            .filter(|a| {
                                a.floor == n.floor
                                    && !self
                                        .consoles
                                        .get(&n.id)
                                        .is_some_and(|c| c.silenced.contains(&a.id))
                            })
                            .filter_map(|a| self.level(a.id))
                            .max()
                            .unwrap_or(Level::Normal)
                            .max(Level::Normal)
                    } else {
                        self.level(n.id).unwrap_or(Level::Normal)
                    };
                    (n.id, level)
                })
                .collect(),
        }
    }
}

impl State {
    fn alerts_available(
        &self,
        ctx: &HookContext<'_>,
        user: NetworkEntityId,
        id: ObjectId,
        console: bool,
    ) -> bool {
        let (Some(object), Some(feet)) = (ctx.world.authored_object(id), ctx.position_of(user))
        else {
            return false;
        };
        let AuthoredKind::Placement { prototype } = &object.kind else {
            return false;
        };
        let at = (object.min() + object.max()) * 0.5;
        self.alerts.specs.get(prototype).is_some_and(|s| {
            if console {
                s.role == Role::Console
            } else {
                s.role == Role::FireAlarm
            }
        }) && !self
            .actors
            .get(&user)
            .is_some_and(|a| a.ghost || !a.health.state().can_act())
            && feet.distance(at) <= self.range_m + 1.0
            && ctx.is_unobstructed(feet + Vec3::Y, at)
            && (!self.power_required || self.powered_map().get(&id) == Some(&true))
    }
    pub(crate) fn use_atmos_alerts(
        &mut self,
        ctx: &HookContext<'_>,
        user: NetworkEntityId,
        id: ObjectId,
        prototype: &PrototypeId,
    ) -> bool {
        let Some(spec) = self.alerts.specs.get(prototype) else {
            return false;
        };
        if spec.role == Role::Console {
            if self.alerts_available(ctx, user, id, true)
                && !self
                    .alerts
                    .sessions
                    .iter()
                    .any(|(owner, c)| *owner != user && *c == id)
            {
                self.alerts.sessions.insert(user, id);
                self.alerts.consoles.entry(id).or_default();
                self.alerts.next_sample = self.clock;
            }
            return true;
        }
        if spec.role != Role::FireAlarm {
            return false;
        }
        if !self.alerts_available(ctx, user, id, false) {
            return true;
        }
        let mut alerts = std::mem::take(&mut self.alerts);
        alerts.sync_nodes(self, ctx);
        let normal = alerts
            .alarms
            .get(&id)
            .is_none_or(|a| a.last <= Level::Normal);
        let alarm = alerts.alarms.entry(id).or_default();
        let mut queue = VecDeque::new();
        if normal {
            alarm.last = Level::Danger;
            alarm.states.insert(id, Level::Danger);
            for target in alerts.recipients(id) {
                queue.push_back((target, [(id, Level::Danger)].into()));
            }
            self.sound_volume("Machines/alarm.ogg", alerts.nodes[&id].at, -10.0);
        } else {
            alarm.states.insert(id, Level::Normal);
            let states = alarm.states.clone();
            alarm.last = Level::Normal;
            alarm.states.clear();
            for target in alerts.recipients(id) {
                queue.push_back((target, states.clone()));
            }
            for sensor in alerts.nodes[&id].targets.clone() {
                if let Some(reading) = alerts.sensors.get_mut(&sensor) {
                    reading.level = Level::Normal;
                }
                if let Some(reading) = alerts.sensors.get(&sensor) {
                    alerts.sensor_packet(sensor, reading, &mut queue);
                }
            }
        }
        for (path, at, volume) in alerts.deliver(queue) {
            self.sound_volume(&path, at, volume);
        }
        alerts.next_sample = self.clock;
        self.alerts = alerts;
        self.alarm_timer = 1.0;
        true
    }
    pub(crate) fn alerts_command(
        &mut self,
        ctx: &HookContext<'_>,
        user: NetworkEntityId,
        kind: &str,
        data: &[u8],
    ) {
        if kind == CMD_CLOSE {
            self.alerts.sessions.remove(&user);
            self.alerts.next_sample = self.clock;
            return;
        }
        let Some(console) = self.alerts.sessions.get(&user).copied() else {
            return;
        };
        if !self.alerts_available(ctx, user, console, true) || data.len() > 128 {
            return;
        }
        let valid = |id| {
            self.alerts.nodes.get(&id).is_some_and(|n| {
                n.floor == self.alerts.nodes[&console].floor && self.alerts.is_alarm(&n.prototype)
            })
        };
        if kind == CMD_FOCUS {
            if let Some(request) = decode::<Focus>(data)
                && request.device.is_none_or(valid)
            {
                self.alerts.consoles.entry(console).or_default().focus = request.device;
            }
        } else if kind == CMD_SILENCE
            && let Some(request) = decode::<Silence>(data)
            && valid(request.device)
        {
            let set = &mut self.alerts.consoles.entry(console).or_default().silenced;
            if request.silenced {
                set.insert(request.device);
            } else {
                set.remove(&request.device);
            }
        }
        self.alerts.next_sample = self.clock;
    }
    pub(crate) fn sample_atmos_alerts(&mut self, ctx: &HookContext<'_>) {
        if self.alerts.specs.is_empty() {
            return;
        }
        let mut alerts = std::mem::take(&mut self.alerts);
        for (path, at, volume) in alerts.sample(self, ctx) {
            self.sound_volume(&path, at, volume);
        }
        self.alerts = alerts;
    }
    pub(crate) fn publish_atmos_alerts(&mut self, ctx: &mut HookContext<'_>) {
        if self.alerts.specs.is_empty() {
            return;
        }
        let powered = self.powered_map();
        if self
            .alerts
            .nodes
            .values()
            .any(|node| node.on != (!self.power_required || powered.get(&node.id) == Some(&true)))
        {
            self.sample_atmos_alerts(ctx);
            self.alerts.next_sample = self.clock;
            self.alarm_timer = 1.0;
        }
        let players: BTreeSet<_> = ctx.players().collect();
        let invalid: Vec<_> = self
            .alerts
            .sessions
            .iter()
            .filter(|(user, id)| {
                !players.contains(user) || !self.alerts_available(ctx, **user, **id, true)
            })
            .map(|(user, _)| *user)
            .collect();
        for user in invalid {
            self.alerts.sessions.remove(&user);
            self.alerts.next_sample = self.clock;
        }
        if self.clock < self.alerts.next_sample {
            return;
        }
        self.alerts.next_sample = self.clock + 1.0;
        self.alerts
            .sent
            .retain(|(user, _), _| players.contains(user));
        let visuals = serde_json::to_vec(&self.alerts.visuals()).unwrap();
        for user in players {
            let bytes = self
                .alerts
                .sessions
                .get(&user)
                .filter(|id| self.alerts.nodes.contains_key(id))
                .map(|id| serde_json::to_vec(&self.alerts.view(*id)).unwrap())
                .unwrap_or_default();
            for (kind, bytes) in [(COMPONENT, bytes), (VISUAL_COMPONENT, visuals.clone())] {
                if self.alerts.sent.get(&(user, kind)) == Some(&bytes) {
                    continue;
                }
                ctx.set_component(user, kind, bytes.clone());
                self.alerts.sent.insert((user, kind), bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_self_link_cannot_echo_stale_sync_states_between_sensor_updates() {
        let mut registry = PrototypeRegistry::new();
        registry
            .load_ron_str(
                r#"[
            (id:"AtmosAlertsData",abstract:true,components:{"AtmosAlerts":(devices:[
                (prototype:"AirAlarm",role:"AirAlarm",tags:["AirAlarm"],syncWith:["AirAlarm"])
            ])})
        ]"#,
                "self-link regression",
            )
            .unwrap();
        let mut alerts = Alerts::load(&registry);
        let id = ObjectId::from_u128(1);
        let first = ObjectId::from_u128(2);
        let second = ObjectId::from_u128(3);
        alerts.nodes.insert(
            id,
            Node {
                id,
                prototype: prototypes::engine_id(NAMESPACE, "AirAlarm").unwrap(),
                floor: 0,
                at: Vec3::ZERO,
                map_at: [0.0; 2],
                cell: (0, 0),
                name: String::new(),
                address: String::new(),
                targets: [id].into(),
                on: true,
            },
        );
        alerts.alarms.insert(
            id,
            Alarm {
                states: [(first, Level::Danger), (second, Level::Normal)].into(),
                last: Level::Danger,
                ..Default::default()
            },
        );
        // Fail before delivery if self-echo is reintroduced, rather than hanging the test.
        assert!(alerts.recipients(id).is_empty());
        alerts.deliver(
            [
                (id, [(first, Level::Normal)].into()),
                (id, [(second, Level::Danger)].into()),
            ]
            .into(),
        );
        assert_eq!(alerts.alarms[&id].last, Level::Danger);
        assert_eq!(alerts.alarms[&id].states[&first], Level::Normal);
        assert_eq!(alerts.alarms[&id].states[&second], Level::Danger);
    }
}
