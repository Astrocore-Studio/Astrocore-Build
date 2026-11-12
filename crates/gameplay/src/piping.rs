//! Трубы и газовые устройства в мире (`NodeContainer`, `GasPressurePump`, `GasVentPump`,
//! `GasVentScrubber`, `GasPassiveVent`, `GasValve`, `GasCanister`, `GasTank`).
//!
//! Трубы лежат под полом, как подпольная проводка SS14: это зоны-метки 1×1 с
//! `gas_pipe = "NS"` (стороны света, `N` — север, `z − 1`) и необязательным `gas_pipe_volume`.
//! Устройства — обычные объекты с компонентами:
//!
//! * `GasPipe: (nodes: [(dirs: "NS", volume: 200.0), …])` — узлы устройства, номера идут по порядку;
//! * `GasPressurePump: (inlet: 0, outlet: 1, targetPressure: 101.325, enabled: false)`;
//! * `GasPassiveVent: (node: 0)`, `GasVentPump: (node: 0)`, `GasVentScrubber: (node: 0)`;
//! * `GasValve: (a: 0, b: 1, open: false)`;
//! * `GasPort` — разъём для баллона (узел `nodes[0]`);
//! * `GasCanister: (volume: 1500.0, temperature: 293.15, moles: (Oxygen: 2769.36), releasePressure: 101.325)`;
//! * `GasTank: (volume: 5.0, temperature: 293.15, releasePressure: 21.3, moles: (...))` — для
//!   переносимых баллончиков.

use std::collections::BTreeMap;

use atmos::{
    Canister, Cell, Device, Gas, GasMixture, NodeSpec, Scrubber, Tank, VentPump, floor_cell,
    pipes::{DEFAULT_PIPE_VOLUME, EAST, NORTH, SOUTH, WEST},
};
use prototypes::{Data, Resolved};

/// Ключ метаданных зоны-трубы.
pub const PIPE_KEY: &str = "gas_pipe";
pub const PIPE_VOLUME_KEY: &str = "gas_pipe_volume";

/// Сколько узлов может быть у одного объекта.
pub const MAX_NODES: u64 = 16;

/// Стороны трубы по строке `NSEW`.
pub fn parse_dirs(s: &str) -> u8 {
    s.chars().fold(0, |m, c| {
        m | match c.to_ascii_uppercase() {
            'N' => NORTH,
            'S' => SOUTH,
            'E' => EAST,
            'W' => WEST,
            _ => 0,
        }
    })
}

/// Повернуть стороны на `quarters` четвертей против часовой стрелки (`RotatePipeDirection`):
/// юг → восток → север → запад → юг.
pub fn rotate_dirs(dirs: u8, quarters: u8) -> u8 {
    let mut d = dirs;
    for _ in 0..quarters % 4 {
        let mut n = 0;
        if d & SOUTH != 0 {
            n |= EAST;
        }
        if d & EAST != 0 {
            n |= NORTH;
        }
        if d & NORTH != 0 {
            n |= WEST;
        }
        if d & WEST != 0 {
            n |= SOUTH;
        }
        d = n;
    }
    d
}

/// Устройство прототипа до того, как оно получит ключи.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceSpec {
    Pump {
        inlet: usize,
        outlet: usize,
        target: f32,
        enabled: bool,
    },
    Passive {
        node: usize,
    },
    Vent {
        node: usize,
    },
    Scrubber {
        node: usize,
    },
    Valve {
        a: usize,
        b: usize,
        open: bool,
    },
    Canister(GasMixture, f32),
    Port,
    /// Воздух криокапсулы заданного объёма, л.
    Cryo {
        node: usize,
        volume: f32,
    },
}

/// Узлы и устройство объекта.
#[derive(Debug, Clone, PartialEq)]
pub struct PipePart {
    pub nodes: Vec<(u8, f32)>,
    pub device: Option<DeviceSpec>,
}

fn gas_by_name(name: &str) -> Option<Gas> {
    Some(match name {
        "Oxygen" => Gas::Oxygen,
        "Nitrogen" => Gas::Nitrogen,
        "CarbonDioxide" => Gas::CarbonDioxide,
        "Plasma" => Gas::Plasma,
        "Tritium" => Gas::Tritium,
        "WaterVapor" => Gas::WaterVapor,
        "Ammonia" => Gas::Ammonia,
        "NitrousOxide" => Gas::NitrousOxide,
        "Frezon" => Gas::Frezon,
        _ => return None,
    })
}

/// Смесь из `volume`, `temperature` и `moles` компонента (баллон, баллончик).
pub fn container_mixture(c: &Data, default_volume: f32) -> GasMixture {
    let volume = c
        .get("volume")
        .and_then(Data::as_f64)
        .map_or(default_volume, |v| v.max(0.0) as f32);
    let mut m = GasMixture::new(volume);
    m.set_temperature(
        c.get("temperature")
            .and_then(Data::as_f64)
            .map_or(atmos::T20C, |v| v as f32),
    );
    if let Some(map) = c.get("moles").and_then(Data::as_map) {
        for (k, v) in map {
            if let (Some(g), Some(q)) = (gas_by_name(k), v.as_f64()) {
                m.adjust_moles(g, q as f32);
            }
        }
    }
    m
}

/// Шаблон переносимого баллончика: смесь и давление выпуска.
pub fn tank_template(r: &Resolved) -> Option<Tank> {
    let c = r.component("GasTank")?;
    let mut t = Tank::new(container_mixture(c, atmos::pipes::TANK_VOLUME));
    if let Some(p) = c.get("releasePressure").and_then(Data::as_f64) {
        t.release_pressure = p as f32;
    }
    Some(t)
}

impl PipePart {
    pub fn from_resolved(r: &Resolved) -> Option<Self> {
        let index = |c: &Data, key: &str| {
            c.get(key)
                .and_then(Data::as_int)
                .map_or(0, |v| v.max(0) as usize)
        };
        let float =
            |c: &Data, key: &str, d: f32| c.get(key).and_then(Data::as_f64).map_or(d, |v| v as f32);
        let bool_of =
            |c: &Data, key: &str, d: bool| c.get(key).and_then(Data::as_bool).unwrap_or(d);
        let nodes: Vec<(u8, f32)> = r
            .component("GasPipe")
            .and_then(|c| c.get("nodes"))
            .and_then(Data::as_list)
            .map(|list| {
                list.iter()
                    .take(MAX_NODES as usize)
                    .map(|n| {
                        (
                            n.get("dirs").and_then(Data::as_str).map_or(0, parse_dirs),
                            n.get("volume")
                                .and_then(Data::as_f64)
                                .map_or(DEFAULT_PIPE_VOLUME, |v| v as f32),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let device = if let Some(c) = r.component("GasPressurePump") {
            Some(DeviceSpec::Pump {
                inlet: index(c, "inlet"),
                outlet: index(c, "outlet"),
                target: float(c, "targetPressure", atmos::ONE_ATMOSPHERE),
                enabled: bool_of(c, "enabled", false),
            })
        } else if let Some(c) = r.component("GasPassiveVent") {
            Some(DeviceSpec::Passive {
                node: index(c, "node"),
            })
        } else if let Some(c) = r.component("GasVentPump") {
            Some(DeviceSpec::Vent {
                node: index(c, "node"),
            })
        } else if let Some(c) = r.component("GasVentScrubber") {
            Some(DeviceSpec::Scrubber {
                node: index(c, "node"),
            })
        } else if let Some(c) = r.component("GasValve") {
            Some(DeviceSpec::Valve {
                a: index(c, "a"),
                b: index(c, "b"),
                open: bool_of(c, "open", false),
            })
        } else if let Some(c) = r.component("GasCanister") {
            Some(DeviceSpec::Canister(
                container_mixture(c, atmos::pipes::CANISTER_VOLUME),
                float(c, "releasePressure", atmos::ONE_ATMOSPHERE),
            ))
        } else if let Some(c) = r.component("CryoPodAir") {
            Some(DeviceSpec::Cryo {
                node: index(c, "node"),
                volume: float(c, "volume", 2000.0),
            })
        } else if r.component("GasPort").is_some() {
            Some(DeviceSpec::Port)
        } else {
            None
        };
        if nodes.is_empty() && device.is_none() {
            return None;
        }
        Some(Self { nodes, device })
    }
}

/// Ключи объектов и узлов: порядковые номера, чтобы не зависеть от устройства идентификаторов.
#[derive(Debug, Default)]
pub struct PipeKeys {
    keys: BTreeMap<astrocore_core::ObjectId, u64>,
    next: u64,
}

impl PipeKeys {
    pub fn key_of(&mut self, id: astrocore_core::ObjectId) -> u64 {
        *self.keys.entry(id).or_insert_with(|| {
            self.next += 1;
            self.next
        })
    }

    pub fn keys_iter(&self) -> impl Iterator<Item = (astrocore_core::ObjectId, u64)> + '_ {
        self.keys.iter().map(|(a, b)| (*a, *b))
    }

    pub fn existing(&self, id: astrocore_core::ObjectId) -> Option<u64> {
        self.keys.get(&id).copied()
    }
}

pub fn node_key(object_key: u64, index: usize) -> u64 {
    object_key * MAX_NODES + index as u64
}

/// Что мир хочет видеть в атмосфере труб.
#[derive(Debug, Default, PartialEq)]
pub struct PipeLayout {
    pub nodes: Vec<NodeSpec>,
    pub devices: Vec<(u64, Cell, Device, astrocore_core::ObjectId)>,
    /// Разъёмы по клеткам.
    pub ports: BTreeMap<Cell, u64>,
}

/// Начальное состояние устройства по описанию.
pub fn device_of(spec: &DeviceSpec, object_key: u64, port: Option<u64>) -> Option<Device> {
    let n = |i: usize| node_key(object_key, i);
    Some(match spec {
        DeviceSpec::Pump {
            inlet,
            outlet,
            target,
            enabled,
        } => Device::PressurePump {
            inlet: n(*inlet),
            outlet: n(*outlet),
            target: *target,
            enabled: *enabled,
        },
        DeviceSpec::Passive { node } => Device::PassiveVent { node: n(*node) },
        DeviceSpec::Vent { node } => Device::VentPump(VentPump::new(n(*node))),
        DeviceSpec::Scrubber { node } => Device::Scrubber(Scrubber::new(n(*node))),
        DeviceSpec::Valve { a, b, open } => Device::Valve {
            a: n(*a),
            b: n(*b),
            open: *open,
        },
        DeviceSpec::Canister(air, release) => {
            let mut c = Canister::new(air.clone());
            c.release_pressure = *release;
            c.port = port;
            Device::Canister(Box::new(c))
        }
        DeviceSpec::Port => return None,
        DeviceSpec::Cryo { node, volume } => Device::CryoPod {
            node: n(*node),
            air: GasMixture::new(*volume),
        },
    })
}

/// Клетка атмосферы объекта по центру его рамки.
pub fn cell_at(floor: i32, centre: astrocore_math::glam::Vec3) -> Cell {
    floor_cell(floor, centre.x.floor() as i32, centre.z.floor() as i32)
}
