//! Трубы, насосы, клапаны, вентиляция, баллоны и газовые баллончики (`Content.Server/Atmos/Piping`,
//! `PipeNet`, `PipeNode`, `GasCanisterSystem`, `GasTankSystem`, `GasMaxPressureSystem`).
//!
//! Трубы образуют сети: все соединённые узлы делят одну смесь (`PipeNet.Air`) объёмом в сумму
//! объёмов узлов. Узел соединяется с соседним по направлению, если у того есть встречное
//! направление (`PipeNode.LinkableNodesInDirection`). Устройства переливают газ между сетями и
//! воздухом клеток на стадии `AtmosDevices` (раз за полный цикл, `RealAtmosTime`), сети реагируют
//! на стадии `PipeNet`.

use std::collections::BTreeMap;

use crate::gas::*;
use crate::grid::Cell;
use crate::reactions;

pub const NORTH: u8 = 1;
pub const SOUTH: u8 = 2;
pub const EAST: u8 = 4;
pub const WEST: u8 = 8;

/// Сдвиг клетки по направлению (север — `z − 1`).
pub fn step(cell: Cell, dir: u8) -> Cell {
    match dir {
        NORTH => (cell.0, cell.1 - 1),
        SOUTH => (cell.0, cell.1 + 1),
        EAST => (cell.0 + 1, cell.1),
        _ => (cell.0 - 1, cell.1),
    }
}

pub fn opposite(dir: u8) -> u8 {
    match dir {
        NORTH => SOUTH,
        SOUTH => NORTH,
        EAST => WEST,
        _ => EAST,
    }
}

/// Объём трубы по умолчанию, л (`PipeNode.DefaultVolume`).
pub const DEFAULT_PIPE_VOLUME: f32 = 200.0;
/// Предел давления на выходе насосов и вентиляции, кПа (`MaxOutputPressure`).
pub const MAX_OUTPUT_PRESSURE: f32 = 4500.0;
/// Расход скруббера, л/с (`MaxTransferRate`).
pub const MAX_TRANSFER_RATE: f32 = 200.0;
/// Сечение предохранительного клапана баллона и клапана баллончика, м² (`ReleaseArea`).
pub const CANISTER_RELEASE_AREA: f32 = 0.05;
pub const TANK_RELEASE_AREA: f32 = 0.0005;
/// Объём баллона и баллончика, л.
pub const CANISTER_VOLUME: f32 = 1500.0;
pub const TANK_VOLUME: f32 = 5.0;

/// Описание узла трубы, как его задаёт мир.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSpec {
    /// Устойчивый идентификатор (чтобы газ сохранялся при перестройке сетей).
    pub key: u64,
    pub cell: Cell,
    pub dirs: u8,
    pub volume: f32,
}

/// Устройство с состоянием.
#[derive(Debug, Clone, PartialEq)]
pub enum Device {
    /// `GasPressurePump`: качает газ из входа в выход, пока давление на выходе ниже цели.
    PressurePump {
        inlet: u64,
        outlet: u64,
        target: f32,
        enabled: bool,
    },
    /// `GasPassiveVent`: выравнивает трубу и воздух клетки.
    PassiveVent { node: u64 },
    /// `GasVentPump`: нагнетает газ из трубы в помещение или откачивает.
    VentPump(VentPump),
    /// `GasVentScrubber`: вытягивает из воздуха выбранные газы в трубу.
    Scrubber(Scrubber),
    /// `GasValve`: ручной клапан между двумя узлами.
    Valve { a: u64, b: u64, open: bool },
    /// `GasCanister`: баллон на разъёме.
    Canister(Box<Canister>),
    /// `GasFilter`: из входа в выход, выбранный газ уходит в боковой узел (`GasFilterSystem`).
    Filter {
        inlet: u64,
        filter: u64,
        outlet: u64,
        enabled: bool,
        /// Литров в секунду.
        rate: f32,
        gas: Option<Gas>,
    },
    /// `GasThermoMachine`: холодильник (`cp < 0`) или нагреватель (`cp > 0`) газа в трубе.
    Thermo(Thermo),
    /// `CryoPodAir`: воздух капсулы, на каждом шаге реагирует и смешивается с сетью порта.
    CryoPod { node: u64, air: GasMixture },
}

/// Термомашина (`GasThermoMachineComponent`).
#[derive(Debug, Clone, PartialEq)]
pub struct Thermo {
    pub node: u64,
    /// Включена ли (переключатель питания).
    pub enabled: bool,
    pub target: f32,
    pub tolerance: f32,
    /// Теплоёмкость машины, Дж/с при полной мощности.
    pub heat_capacity: f32,
    /// Коэффициент полезного действия: отрицательный у холодильника.
    pub cp: f32,
    pub min_temperature: f32,
    pub max_temperature: f32,
    pub hysteresis: bool,
}

impl Thermo {
    pub fn new(node: u64, cp: f32, target: f32) -> Self {
        Self {
            node,
            enabled: true,
            target,
            tolerance: 2.0,
            heat_capacity: 5000.0,
            cp,
            min_temperature: 73.15,
            max_temperature: 593.15,
            hysteresis: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VentPump {
    pub node: u64,
    pub enabled: bool,
    pub releasing: bool,
    pub external_bound: f32,
    pub target_pressure_change: f32,
    pub pump_power: f32,
    pub under_pressure_lockout: bool,
}

impl VentPump {
    pub fn new(node: u64) -> Self {
        Self {
            node,
            enabled: true,
            releasing: true,
            external_bound: ONE_ATMOSPHERE,
            target_pressure_change: ONE_ATMOSPHERE,
            pump_power: 2.0,
            under_pressure_lockout: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scrubber {
    pub node: u64,
    pub enabled: bool,
    pub siphoning: bool,
    pub filter: Vec<Gas>,
    pub transfer_rate: f32,
    /// `WideNet`: скруббер чистит и соседние клетки.
    pub wide_net: bool,
}

impl Scrubber {
    pub fn new(node: u64) -> Self {
        Self {
            node,
            enabled: true,
            siphoning: false,
            filter: vec![
                Gas::CarbonDioxide,
                Gas::Plasma,
                Gas::Tritium,
                Gas::WaterVapor,
                Gas::Ammonia,
                Gas::NitrousOxide,
                Gas::Frezon,
            ],
            transfer_rate: MAX_TRANSFER_RATE,
            wide_net: false,
        }
    }
}

/// Сосуд под давлением: предохранитель, перегрузка и целостность (`GasMaxPressureHolderComponent`).
#[derive(Debug, Clone, PartialEq)]
pub struct MaxPressure {
    pub safety_pressure: f32,
    pub overpressure: f32,
    pub integrity: f32,
    pub max_integrity: f32,
}

impl MaxPressure {
    /// Значения по умолчанию у баллончика: 15 и 20 атмосфер, целостность 3.
    pub fn tank() -> Self {
        Self {
            safety_pressure: 15.0 * ONE_ATMOSPHERE,
            overpressure: 20.0 * ONE_ATMOSPHERE,
            integrity: 3.0,
            max_integrity: 3.0,
        }
    }

    /// Значения баллона (`GasCanister`): целостность 15, предохранитель 10132,5, перегрузка 20265.
    pub fn canister() -> Self {
        Self {
            safety_pressure: 10_132.5,
            overpressure: 20_265.0,
            integrity: 15.0,
            max_integrity: 15.0,
        }
    }

    /// `GasMaxPressureSystem.CheckStatus`.
    pub fn check(&mut self, pressure: f32, dt: f32) -> PressureStatus {
        if pressure > self.overpressure * (self.integrity + 1.0) {
            return PressureStatus::Ruptured;
        }
        if pressure > self.overpressure {
            self.integrity -= dt;
        } else if self.integrity < self.max_integrity {
            self.integrity = (self.integrity + dt).min(self.max_integrity);
        }
        if pressure > self.safety_pressure {
            PressureStatus::SafetyValve
        } else {
            PressureStatus::Fine
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureStatus {
    Fine,
    /// Надо открыть предохранительный клапан (`SafetyMeasures`).
    SafetyValve,
    /// Сосуд лопнул: газ уходит в окружающий воздух, сосуд уничтожен.
    Ruptured,
}

/// Газовый баллон (`GasCanisterComponent`).
#[derive(Debug, Clone, PartialEq)]
pub struct Canister {
    pub air: GasMixture,
    /// Узел разъёма, к которому баллон присоединён.
    pub port: Option<u64>,
    pub release_pressure: f32,
    pub release_open: bool,
    pub safety_open: bool,
    pub limits: MaxPressure,
    /// Энергия газа в момент разрыва, давление × объём (`GetOverPressure`).
    pub burst_energy: f32,
}

impl Canister {
    pub fn new(air: GasMixture) -> Self {
        Self {
            air,
            port: None,
            release_pressure: ONE_ATMOSPHERE,
            release_open: false,
            safety_open: false,
            limits: MaxPressure::canister(),
            burst_energy: 0.0,
        }
    }
}

/// Газовый баллончик (`GasTankComponent`), переносимый в руках.
#[derive(Debug, Clone, PartialEq)]
pub struct Tank {
    pub air: GasMixture,
    pub release_pressure: f32,
    pub release_open: bool,
    pub limits: MaxPressure,
    /// Энергия газа в момент разрыва, давление × объём.
    pub burst_energy: f32,
}

impl Tank {
    pub fn new(air: GasMixture) -> Self {
        Self {
            air,
            release_pressure: 21.3,
            release_open: false,
            limits: MaxPressure::tank(),
            burst_energy: 0.0,
        }
    }

    /// `DeviceUpdated` баллончика: сброс газа, реакции, проверка прочности. `env` — воздух вокруг.
    pub fn update(&mut self, dt: f32, env: Option<&mut GasMixture>) -> PressureStatus {
        let status = self.limits.check(self.air.pressure(), dt);
        if status == PressureStatus::Ruptured {
            self.burst_energy = self.air.pressure() * self.air.volume;
            if let Some(e) = env {
                e.merge(&self.air);
            }
            self.air.clear();
            return status;
        }
        if status == PressureStatus::SafetyValve {
            self.release_open = true;
        }
        if self.release_open {
            let delta_p = match env.as_ref() {
                None => self.air.pressure(),
                Some(e) => self.air.pressure() - e.pressure(),
            };
            let delta_p = if delta_p < self.limits.safety_pressure {
                delta_p.min(self.release_pressure)
            } else {
                delta_p
            };
            if let Some(removed) = flow_gas(&mut self.air, delta_p, dt, TANK_RELEASE_AREA)
                && let Some(e) = env
            {
                e.merge(&removed);
            }
        }
        reactions::react(&mut self.air, false);
        status
    }
}

/// `GetFlowVolume`: объём, который проходит через отверстие `area` за `dt` при перепаде `delta_p`.
pub fn flow_volume(mix: &GasMixture, delta_p: f32, area: f32, dt: f32) -> f32 {
    let mass = mix.mass();
    if delta_p <= 0.0 || mass <= 0.0 {
        return 0.0;
    }
    dt * area * (2.0 * delta_p * mix.volume / mass).sqrt()
}

/// `FlowGas`: выпустить через отверстие порцию газа при перепаде `delta_p`.
pub fn flow_gas(mix: &mut GasMixture, delta_p: f32, dt: f32, area: f32) -> Option<GasMixture> {
    if delta_p <= 0.0 {
        return None;
    }
    let volume = flow_volume(mix, delta_p, area, dt);
    let pressure = mix.pressure();
    if pressure <= 0.0 || mix.temperature() <= 0.0 {
        return None;
    }
    // RemoveVolumeAtPressure
    let moles = pressure * volume / (R * mix.temperature());
    Some(mix.remove(moles))
}

#[derive(Debug, Clone)]
struct Node {
    cell: Cell,
    dirs: u8,
    volume: f32,
    net: usize,
}

/// Все трубы и устройства мира.
#[derive(Debug, Default)]
pub struct Pipes {
    nodes: BTreeMap<u64, Node>,
    nets: Vec<GasMixture>,
    net_sizes: Vec<usize>,
    devices: BTreeMap<u64, (Cell, Device)>,
    /// Устройства без питания: не работают (`ApcPowerReceiver.Powered`).
    unpowered: std::collections::BTreeSet<u64>,
    dirty: bool,
}

impl Pipes {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.devices.is_empty()
    }

    pub fn net_count(&self) -> usize {
        self.nets.len()
    }

    pub fn device(&self, key: u64) -> Option<&Device> {
        self.devices.get(&key).map(|(_, d)| d)
    }

    /// Воздух капсулы без пересчёта сетей.
    pub fn cryo_air_mut(&mut self, key: u64) -> Option<&mut GasMixture> {
        match self.devices.get_mut(&key).map(|(_, d)| d) {
            Some(Device::CryoPod { air, .. }) => Some(air),
            _ => None,
        }
    }

    pub fn cryo_air(&self, key: u64) -> Option<&GasMixture> {
        match self.devices.get(&key).map(|(_, d)| d) {
            Some(Device::CryoPod { air, .. }) => Some(air),
            _ => None,
        }
    }

    pub fn device_mut(&mut self, key: u64) -> Option<&mut Device> {
        // Любое изменение (клапан, разъём) может поменять сети.
        self.dirty = true;
        self.devices.get_mut(&key).map(|(_, d)| d)
    }

    /// Включить или отключить питание устройства.
    pub fn set_powered(&mut self, key: u64, powered: bool) {
        if powered {
            self.unpowered.remove(&key);
        } else {
            self.unpowered.insert(key);
        }
    }

    pub fn is_powered(&self, key: u64) -> bool {
        !self.unpowered.contains(&key)
    }

    pub fn device_keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.devices.keys().copied()
    }

    pub fn device_cell(&self, key: u64) -> Option<Cell> {
        self.devices.get(&key).map(|(c, _)| *c)
    }

    pub fn add_device(&mut self, key: u64, cell: Cell, device: Device) {
        self.devices.insert(key, (cell, device));
        self.dirty = true;
    }

    pub fn remove_device(&mut self, key: u64) -> Option<Device> {
        self.dirty = true;
        self.devices.remove(&key).map(|(_, d)| d)
    }

    /// Смесь сети, к которой принадлежит узел.
    pub fn air_of(&self, node: u64) -> Option<&GasMixture> {
        self.nets.get(self.nodes.get(&node)?.net)
    }

    pub fn air_of_mut(&mut self, node: u64) -> Option<&mut GasMixture> {
        let n = self.nodes.get(&node)?.net;
        self.nets.get_mut(n)
    }

    /// Задать узлы заново. Газ уцелевших узлов переходит в новые сети пропорционально объёму
    /// (`DivideInto` и слияние сетей).
    pub fn set_nodes(&mut self, specs: &[NodeSpec]) {
        let old_nets = std::mem::take(&mut self.nets);
        let old_nodes = std::mem::take(&mut self.nodes);
        let mut old_volume = vec![0.0f32; old_nets.len()];
        for n in old_nodes.values() {
            old_volume[n.net] += n.volume;
        }
        for s in specs {
            self.nodes.insert(
                s.key,
                Node {
                    cell: s.cell,
                    dirs: s.dirs,
                    volume: s.volume,
                    net: 0,
                },
            );
        }
        self.rebuild_with(&old_nets, &old_nodes, &old_volume);
    }

    fn rebuild(&mut self) {
        let old_nets = std::mem::take(&mut self.nets);
        let old_nodes = self.nodes.clone();
        let mut old_volume = vec![0.0f32; old_nets.len()];
        for n in old_nodes.values() {
            old_volume[n.net] += n.volume;
        }
        self.rebuild_with(&old_nets, &old_nodes, &old_volume);
    }

    fn rebuild_with(
        &mut self,
        old_nets: &[GasMixture],
        old_nodes: &BTreeMap<u64, Node>,
        old_volume: &[f32],
    ) {
        self.dirty = false;
        let keys: Vec<u64> = self.nodes.keys().copied().collect();
        let index: BTreeMap<u64, usize> = keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();
        let mut uf = UnionFind::new(keys.len());
        // Соседние узлы со встречными направлениями.
        let mut by_cell: BTreeMap<Cell, Vec<usize>> = BTreeMap::new();
        for (i, k) in keys.iter().enumerate() {
            by_cell.entry(self.nodes[k].cell).or_default().push(i);
        }
        for (i, k) in keys.iter().enumerate() {
            let n = &self.nodes[k];
            for dir in [NORTH, SOUTH, EAST, WEST] {
                if n.dirs & dir == 0 {
                    continue;
                }
                let Some(others) = by_cell.get(&step(n.cell, dir)) else {
                    continue;
                };
                for &j in others {
                    if self.nodes[&keys[j]].dirs & opposite(dir) != 0 {
                        uf.union(i, j);
                    }
                }
            }
        }
        // Открытые клапаны соединяют свои узлы.
        for (_, d) in self.devices.values() {
            if let Device::Valve { a, b, open: true } = d
                && let (Some(&i), Some(&j)) = (index.get(a), index.get(b))
            {
                uf.union(i, j);
            }
        }
        // Номера сетей и их объёмы.
        let mut net_of_root: BTreeMap<usize, usize> = BTreeMap::new();
        let mut volumes: Vec<f32> = Vec::new();
        self.net_sizes.clear();
        for (i, k) in keys.iter().enumerate() {
            let root = uf.find(i);
            let n = *net_of_root.entry(root).or_insert_with(|| {
                volumes.push(0.0);
                self.net_sizes.push(0);
                volumes.len() - 1
            });
            volumes[n] += self.nodes[k].volume;
            self.net_sizes[n] += 1;
            self.nodes.get_mut(k).unwrap().net = n;
        }
        self.nets = volumes
            .iter()
            .map(|&v| {
                let mut m = GasMixture::new(v);
                m.set_temperature(T20C);
                m
            })
            .collect();
        // Перенос газа от старых сетей к новым.
        for (k, node) in &self.nodes {
            let Some(old) = old_nodes.get(k) else {
                continue;
            };
            if old.net >= old_nets.len() || old_volume[old.net] <= 0.0 {
                continue;
            }
            let mut part = old_nets[old.net].clone();
            part.multiply(old.volume / old_volume[old.net]);
            part.volume = node.volume;
            self.nets[node.net].merge(&part);
        }
    }

    /// Стадия `PipeNet`: перестроить сети, если надо, и дать им прореагировать.
    pub fn process_nets(&mut self) {
        if self.dirty {
            self.rebuild();
        }
        for net in &mut self.nets {
            reactions::react(net, false);
        }
    }

    /// Сколько узлов в сети узла (у разъёма баллона >1, когда подключены трубы).
    pub fn net_size(&self, node: u64) -> usize {
        self.nodes
            .get(&node)
            .and_then(|n| self.net_sizes.get(n.net))
            .copied()
            .unwrap_or(0)
    }

    /// Стадия `AtmosDevices`: каждое устройство отрабатывает `dt` секунд. Возвращает события.
    pub fn process_devices(&mut self, dt: f32, env: &mut dyn TileAccess) -> Vec<DeviceEvent> {
        if self.dirty {
            self.rebuild();
        }
        let mut events = Vec::new();
        let mut devices = std::mem::take(&mut self.devices);
        let mut gone = Vec::new();
        for (key, (cell, device)) in &mut devices {
            if self.unpowered.contains(key)
                && !matches!(
                    device,
                    Device::Canister(_) | Device::CryoPod { .. } | Device::Filter { .. }
                )
            {
                continue;
            }
            match device {
                Device::PressurePump {
                    inlet,
                    outlet,
                    target,
                    enabled,
                } => {
                    if *enabled {
                        self.pressure_pump(*inlet, *outlet, *target);
                    }
                }
                Device::PassiveVent { node } => self.passive_vent(*node, *cell, env),
                Device::VentPump(v) => self.vent_pump(v, *cell, dt, env),
                Device::Scrubber(s) => self.scrub(s, *cell, dt, env),
                Device::Valve { .. } => {}
                Device::Filter {
                    inlet,
                    filter,
                    outlet,
                    enabled,
                    rate,
                    gas,
                } => {
                    if *enabled {
                        self.filter(*inlet, *filter, *outlet, *rate, *gas, dt);
                    }
                }
                Device::Thermo(t) => {
                    if t.enabled {
                        self.thermo(t, dt);
                    }
                }
                Device::CryoPod { node, air } => {
                    // `OnCryoPodUpdateAtmosphere`: реакции и обмен с сетью порта.
                    reactions::react(air, false);
                    if self.net_size(*node) > 1
                        && let Some(net) = self.air_of_mut(*node)
                    {
                        mix_with_net(air, net);
                    }
                }
                Device::Canister(c) => {
                    if self.canister(c, *cell, dt, env) == PressureStatus::Ruptured {
                        gone.push(*key);
                        events.push(DeviceEvent::Ruptured {
                            key: *key,
                            cell: *cell,
                            energy: c.burst_energy,
                        });
                    }
                }
            }
        }
        for k in gone {
            devices.remove(&k);
            self.dirty = true;
        }
        self.devices = devices;
        events
    }

    fn net_of(&self, node: u64) -> Option<usize> {
        self.nodes.get(&node).map(|n| n.net)
    }

    /// `GasPressurePumpSystem.OnPumpUpdated`.
    fn pressure_pump(&mut self, inlet: u64, outlet: u64, target: f32) {
        let (Some(i), Some(o)) = (self.net_of(inlet), self.net_of(outlet)) else {
            return;
        };
        if i == o {
            return;
        }
        let start = self.nets[o].pressure();
        if start >= target {
            return;
        }
        let (src, dst) = two_mut(&mut self.nets, i, o);
        if src.total_moles() > 0.0 && src.temperature() > 0.0 {
            let delta = target - start;
            let moles = delta * dst.volume / (src.temperature() * R);
            let removed = src.remove(moles);
            dst.merge(&removed);
        }
    }

    /// `GasFilterSystem.OnFilterUpdated`.
    fn filter(
        &mut self,
        inlet: u64,
        filter: u64,
        outlet: u64,
        rate: f32,
        gas: Option<Gas>,
        dt: f32,
    ) {
        let (Some(i), Some(f), Some(o)) =
            (self.net_of(inlet), self.net_of(filter), self.net_of(outlet))
        else {
            return;
        };
        if i == f || i == o || f == o {
            return;
        }
        if self.nets[o].pressure() >= MAX_OUTPUT_PRESSURE
            && self.nets[f].pressure() >= MAX_OUTPUT_PRESSURE
        {
            return;
        }
        let volume = rate * dt;
        if volume <= 0.0 {
            return;
        }
        let mut removed = self.nets[i].remove_volume(volume);
        if let Some(g) = gas {
            let limit = moles_to_max_pressure(&removed, &self.nets[f], MAX_OUTPUT_PRESSURE);
            let take = removed.moles(g).min(limit).max(0.0);
            let mut part = GasMixture::new(removed.volume);
            part.set_temperature(removed.temperature());
            part.set_moles(g, take);
            removed.set_moles(g, removed.moles(g) - take);
            self.nets[f].merge(&part);
        }
        let ratio = fraction_to_max_pressure(&removed, &self.nets[o], MAX_OUTPUT_PRESSURE);
        let pass = removed.remove_ratio(ratio);
        self.nets[o].merge(&pass);
        self.nets[i].merge(&removed);
    }

    /// `GasThermoMachineSystem.OnThermoMachineUpdated`: гистерезис, не перегревать за целевую.
    fn thermo(&mut self, t: &mut Thermo, dt: f32) {
        let Some(n) = self.net_of(t.node) else { return };
        let sign = t.cp.signum();
        let temp = self.nets[n].temperature();
        let high = t.target + sign * t.tolerance;
        if sign * temp >= sign * high {
            t.hysteresis = false;
        } else if sign * temp < sign * t.target {
            t.hysteresis = true;
        }
        if !t.hysteresis {
            return;
        }
        let mut dq = t.heat_capacity * t.cp * dt;
        let cin = self.nets[n].heat_capacity();
        let dq_lim = (high - temp) * cin;
        let mut scale = 1.0;
        if dq.abs() > dq_lim.abs() {
            scale = dq_lim / dq;
            t.hysteresis = false;
        }
        dq *= scale;
        let cap = self.nets[n].heat_capacity();
        if cap > 0.0 && self.nets[n].total_moles() > 0.0 {
            let new_t =
                (temp + dq / cap).clamp(t.min_temperature.min(temp), t.max_temperature.max(temp));
            self.nets[n].set_temperature(new_t);
        }
    }

    /// `GasPassiveVentSystem.OnPassiveVentUpdated`.
    fn passive_vent(&mut self, node: u64, cell: Cell, env: &mut dyn TileAccess) {
        let (Some(n), Some(tile)) = (self.net_of(node), env.air(cell)) else {
            return;
        };
        let pipe = &mut self.nets[n];
        let pipe_air = pipe.remove_ratio(1.0);
        let env_air = tile.remove_ratio(1.0);
        let mut mix = GasMixture::new(pipe_air.volume + env_air.volume);
        mix.merge(&pipe_air);
        mix.merge(&env_air);
        let back = mix.remove_volume(pipe_air.volume);
        pipe.merge(&back);
        tile.merge(&mix);
    }

    /// `GasVentPumpSystem.OnGasVentPumpUpdated`: давление в помещении тянется к `external_bound`.
    fn vent_pump(&mut self, v: &mut VentPump, cell: Cell, dt: f32, env: &mut dyn TileAccess) {
        let (Some(n), Some(tile)) = (self.net_of(v.node), env.air(cell)) else {
            return;
        };
        if !v.enabled {
            return;
        }
        let pipe = &mut self.nets[n];
        let mut pressure_delta = dt * v.target_pressure_change;
        v.under_pressure_lockout = tile.pressure() < UNDER_PRESSURE_LOCKOUT_THRESHOLD;
        if v.releasing && pipe.pressure() > 0.0 {
            if tile.pressure() > MAX_OUTPUT_PRESSURE {
                return;
            }
            let supply = (pipe.pressure() * v.pump_power).min(v.external_bound);
            pressure_delta = pressure_delta.min(supply - tile.pressure());
            if pressure_delta <= 0.0 {
                return;
            }
            let mut moles = pressure_delta * tile.volume / (pipe.temperature() * R);
            if v.under_pressure_lockout {
                let pipe_delta = pipe.pressure() - tile.pressure();
                moles = dt * pipe_delta * UNDER_PRESSURE_LOCKOUT_LEAKING;
                if moles < 0.0 {
                    return;
                }
            }
            let removed = pipe.remove(moles);
            tile.merge(&removed);
        } else if !v.releasing && tile.pressure() > 0.0 {
            if pipe.pressure() > MAX_OUTPUT_PRESSURE {
                return;
            }
            let moles = pressure_delta * pipe.volume / (tile.temperature() * R);
            let external_delta = tile.pressure() - v.external_bound;
            if external_delta <= 0.0 {
                return;
            }
            let max = external_delta * tile.volume / (tile.temperature() * R);
            let removed = tile.remove(moles.min(max));
            pipe.merge(&removed);
        }
    }

    /// `GasVentScrubberSystem.Scrub`.
    fn scrub(&mut self, s: &Scrubber, cell: Cell, dt: f32, env: &mut dyn TileAccess) {
        let Some(n) = self.net_of(s.node) else {
            return;
        };
        if !s.enabled {
            return;
        }
        self.scrub_tile(n, s, cell, dt, env);
        if s.wide_net {
            for adjacent in env.adjacent(cell) {
                self.scrub_tile(n, s, adjacent, dt, env);
            }
        }
    }

    fn scrub_tile(
        &mut self,
        n: usize,
        s: &Scrubber,
        cell: Cell,
        dt: f32,
        env: &mut dyn TileAccess,
    ) {
        let Some(tile) = env.air(cell) else {
            return;
        };
        let pipe = &mut self.nets[n];
        if pipe.pressure() >= 50.0 * ONE_ATMOSPHERE {
            return;
        }
        let ratio = (dt * s.transfer_rate / tile.volume).min(1.0);
        let mut removed = tile.remove_ratio(ratio);
        if removed.total_moles() < 1e-6 {
            return;
        }
        if s.siphoning {
            pipe.merge(&removed);
        } else {
            // ScrubInto: выбранные газы уходят в трубу, остальное возвращается.
            let mut buffer = GasMixture::new(removed.volume);
            buffer.set_temperature(removed.temperature());
            for &g in &s.filter {
                buffer.adjust_moles(g, removed.moles(g));
                removed.set_moles(g, 0.0);
            }
            pipe.merge(&buffer);
            tile.merge(&removed);
        }
    }

    /// `GasCanisterSystem.DeviceUpdated`.
    fn canister(
        &mut self,
        c: &mut Canister,
        cell: Cell,
        dt: f32,
        env: &mut dyn TileAccess,
    ) -> PressureStatus {
        let status = c.limits.check(c.air.pressure(), dt);
        if status == PressureStatus::Ruptured {
            c.burst_energy = c.air.pressure() * c.air.volume;
            if let Some(t) = env.air(cell) {
                t.merge(&c.air);
            }
            c.air.clear();
            return status;
        }
        if status == PressureStatus::SafetyValve {
            c.safety_open = true;
        }
        reactions::react(&mut c.air, false);
        if let Some(port) = c.port
            && self.net_size(port) > 1
            && let Some(net) = self.air_of_mut(port)
        {
            mix_with_net(&mut c.air, net);
        }
        let tile = env.air(cell);
        if c.safety_open {
            if let Some(t) = tile {
                let delta = (c.air.pressure() - t.pressure()).min(c.air.pressure());
                if let Some(removed) = flow_gas(&mut c.air, delta, dt, CANISTER_RELEASE_AREA) {
                    t.merge(&removed);
                }
            } else {
                let p = c.air.pressure();
                flow_gas(&mut c.air, p, dt, CANISTER_RELEASE_AREA);
            }
            if c.air.pressure() < c.limits.safety_pressure {
                c.safety_open = false;
            }
        } else if c.release_open {
            release_gas_to(&mut c.air, tile, c.release_pressure);
        }
        status
    }
}

/// События устройств, которые касаются остального мира.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeviceEvent {
    /// Баллон лопнул.
    Ruptured { key: u64, cell: Cell, energy: f32 },
}

/// Доступ устройств к воздуху клеток.
pub trait TileAccess {
    fn air(&mut self, cell: Cell) -> Option<&mut GasMixture>;
    /// Соседние клетки, между которыми проходит воздух (для `WideNet`).
    fn adjacent(&mut self, _cell: Cell) -> Vec<Cell> {
        Vec::new()
    }
}

const UNDER_PRESSURE_LOCKOUT_THRESHOLD: f32 = 80.0;
const UNDER_PRESSURE_LOCKOUT_LEAKING: f32 = 0.0001;

fn two_mut(nets: &mut [GasMixture], a: usize, b: usize) -> (&mut GasMixture, &mut GasMixture) {
    debug_assert_ne!(a, b);
    if a < b {
        let (l, r) = nets.split_at_mut(b);
        (&mut l[a], &mut r[0])
    } else {
        let (l, r) = nets.split_at_mut(a);
        (&mut r[0], &mut l[b])
    }
}

/// `MolesToMaxPressure`: сколько молей `giver` можно добавить в `receiver`, не превысив `max` кПа.
pub fn moles_to_max_pressure(giver: &GasMixture, receiver: &GasMixture, max: f32) -> f32 {
    let t = giver.temperature().max(1.0);
    ((max - receiver.pressure()) * receiver.volume / (t * R)).max(0.0)
}

/// `FractionToMaxPressure`: доля `giver`, которую можно отдать `receiver` до `max` кПа.
pub fn fraction_to_max_pressure(giver: &GasMixture, receiver: &GasMixture, max: f32) -> f32 {
    let total = giver.total_moles();
    if total <= 0.0 {
        return 0.0;
    }
    (moles_to_max_pressure(giver, receiver, max) / total).clamp(0.0, 1.0)
}

/// `MixContainerWithPipeNet`: смесь сосуда и сети перемешивается и делится по объёму.
pub fn mix_with_net(container: &mut GasMixture, net: &mut GasMixture) {
    let mut buffer = GasMixture::new(net.volume + container.volume);
    buffer.merge(net);
    buffer.merge(container);
    let total = buffer.volume;
    net.clear();
    net.merge(&buffer);
    net.multiply(net.volume / total);
    container.clear();
    container.merge(&buffer);
    container.multiply(container.volume / total);
}

/// `ReleaseGasTo`: выпустить газ до давления `target`.
pub fn release_gas_to(
    mixture: &mut GasMixture,
    output: Option<&mut GasMixture>,
    target: f32,
) -> bool {
    let out_pressure = output.as_ref().map_or(0.0, |o| o.pressure());
    let in_pressure = mixture.pressure();
    if out_pressure >= target.min(in_pressure - 10.0) {
        return false;
    }
    if mixture.total_moles() <= 0.0 || mixture.temperature() <= 0.0 {
        return false;
    }
    let delta = (target - out_pressure).min((in_pressure - out_pressure) / 2.0);
    let volume = output.as_ref().map_or(CELL_VOLUME, |o| o.volume);
    let transfer = delta * volume / (mixture.temperature() * R);
    let removed = mixture.remove(transfer);
    if let Some(o) = output {
        o.merge(&removed);
    }
    true
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(n: usize) -> Self {
        Self((0..n).collect())
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a] = b;
        }
    }
}
