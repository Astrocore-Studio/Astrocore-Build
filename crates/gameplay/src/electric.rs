//! Электричество в мире (`PowerNetSystem`, `ApcSystem`, `ExtensionCableSystem`): кабели под полом
//! задаются зонами `power_cable` (`hv`, `mv`, `apc`), устройства — объектами с компонентами
//! `PowerSupplier`, `PowerBattery`, `PowerTerminal`, `ApcPowerReceiver` (см. `content/prototypes/power.ron`).
//!
//! Потребитель подключается к сети АПЦ ближайшего кабеля не дальше 3 клеток (`ReceptionRange`).
//! Устройство с `ApcPowerReceiver` считается запитанным, когда получает всю заявленную мощность.
//! Если на карте нет ни кабелей, ни источников, питание не требуется (полигон без проводки).

use std::collections::BTreeMap;

use astrocore_core::ObjectId;
use power::{Battery, CableGroup, Load, PowerState, Supply};
use prototypes::{Data, Resolved};

/// Дальность подключения потребителя к сети АПЦ, клеток (`ReceptionRange`).
pub const RECEPTION_RANGE: f32 = 3.0;
/// Мощность потребителя по умолчанию, Вт.
pub const DEFAULT_LOAD_W: f32 = 5.0;
/// Ключ метаданных зоны-кабеля.
pub const CABLE_KEY: &str = "power_cable";

#[derive(Debug, Clone, PartialEq)]
pub struct SupplierSpec {
    pub group: CableGroup,
    pub max_supply: f32,
    pub ramp_rate: f32,
    pub ramp_tolerance: f32,
}

/// Откуда заряжается аккумулятор.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Group(CableGroup),
    /// По ближайшей клемме (`CableTerminal`): у СМЭС вход отдельно от выхода.
    Terminal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatterySpec {
    pub from: Source,
    pub to: CableGroup,
    pub capacity: f32,
    pub starting_charge: f32,
    pub max_charge_rate: f32,
    pub max_supply: f32,
    pub ramp_rate: f32,
    pub ramp_tolerance: f32,
}

/// Электрические компоненты прототипа.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PowerPart {
    pub supplier: Option<SupplierSpec>,
    pub battery: Option<BatterySpec>,
    pub terminal: bool,
    pub receiver_load: Option<f32>,
    /// Светильник: цвет, сила и радиус (`PointLight` светильника SS14).
    pub light: Option<LightSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LightSpec {
    pub color: [u8; 3],
    pub energy: f32,
    pub radius: f32,
}

/// Светильники для клиентов: палитра видов и список (x, y, z, вид, включён).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub struct LightsView {
    /// Виды: `[r, g, b, энергия, радиус]`.
    pub kinds: Vec<[f32; 5]>,
    pub lights: Vec<(f32, f32, f32, u8, bool)>,
}

pub const LIGHTS_COMPONENT: &str = "build.lights";
pub const LIGHTS_MAX_BYTES: usize = 15 * 1024;
/// Не больше стольких светильников уходит клиентам.
pub const MAX_LIGHTS: usize = 500;

pub fn encode_lights(v: &LightsView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_lights(b: &[u8]) -> Option<LightsView> {
    if b.len() > LIGHTS_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(b).ok()
}

/// Цвет `#RRGGBB` из прототипа.
fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() < 6 {
        return None;
    }
    Some([
        u8::from_str_radix(&h[0..2], 16).ok()?,
        u8::from_str_radix(&h[2..4], 16).ok()?,
        u8::from_str_radix(&h[4..6], 16).ok()?,
    ])
}

fn f(c: &Data, key: &str, d: f32) -> f32 {
    c.get(key).and_then(Data::as_f64).map_or(d, |v| v as f32)
}

impl PowerPart {
    pub fn from_resolved(r: &Resolved) -> Option<Self> {
        let supplier = r.component("PowerSupplier").map(|c| SupplierSpec {
            group: c
                .get("group")
                .and_then(Data::as_str)
                .and_then(CableGroup::parse)
                .unwrap_or(CableGroup::High),
            max_supply: f(c, "maxSupply", 0.0),
            ramp_rate: f(c, "rampRate", 5000.0),
            ramp_tolerance: f(c, "rampTolerance", 5000.0),
        });
        let battery = r.component("PowerBattery").map(|c| BatterySpec {
            from: match c.get("from").and_then(Data::as_str) {
                Some("terminal") => Source::Terminal,
                Some(g) => Source::Group(CableGroup::parse(g).unwrap_or(CableGroup::Medium)),
                None => Source::Group(CableGroup::Medium),
            },
            to: c
                .get("to")
                .and_then(Data::as_str)
                .and_then(CableGroup::parse)
                .unwrap_or(CableGroup::Apc),
            capacity: f(c, "capacity", 0.0),
            starting_charge: f(c, "startingCharge", 0.0),
            max_charge_rate: f(c, "maxChargeRate", 0.0),
            max_supply: f(c, "maxSupply", 0.0),
            ramp_rate: f(c, "rampRate", 5000.0),
            ramp_tolerance: f(c, "rampTolerance", 5000.0),
        });
        let terminal = r.component("PowerTerminal").is_some();
        let receiver_load = r
            .component("ApcPowerReceiver")
            .map(|c| f(c, "load", DEFAULT_LOAD_W));
        let light = r.component("Light").map(|c| LightSpec {
            color: c
                .get("color")
                .and_then(Data::as_str)
                .and_then(parse_color)
                .unwrap_or([255, 228, 206]),
            energy: f(c, "energy", 0.8),
            radius: f(c, "radius", 10.0),
        });
        (supplier.is_some()
            || battery.is_some()
            || terminal
            || receiver_load.is_some()
            || light.is_some())
        .then_some(Self {
            supplier,
            battery,
            terminal,
            receiver_load,
            light,
        })
    }
}

/// Результат сборки: решатель и привязки объектов к его элементам.
pub struct Built {
    pub state: PowerState,
    /// (объект, номер аккумулятора).
    pub batteries: Vec<(ObjectId, usize)>,
    /// (объект, номер потребителя, заявленная мощность).
    pub receivers: Vec<(ObjectId, usize, f32)>,
}

/// Собрать сети из клеток кабелей и устройств; `charges` — сохранённые заряды аккумуляторов.
pub fn build(
    cables: &[(CableGroup, atmos::Cell)],
    parts: &[(ObjectId, atmos::Cell, PowerPart)],
    charges: &BTreeMap<ObjectId, f32>,
) -> Built {
    let grid = power::Cables::build(cables.iter().copied());
    let mut state = PowerState::default();
    let mut net_of: BTreeMap<(CableGroup, usize), usize> = BTreeMap::new();
    fn net(
        state: &mut PowerState,
        map: &mut BTreeMap<(CableGroup, usize), usize>,
        g: CableGroup,
        n: usize,
    ) -> usize {
        *map.entry((g, n)).or_insert_with(|| state.add_network())
    }
    for (g, c) in cables {
        if let Some(n) = grid.net_at(*g, *c) {
            net(&mut state, &mut net_of, *g, n);
        }
    }
    let terminals: Vec<atmos::Cell> = parts
        .iter()
        .filter(|(_, _, p)| p.terminal)
        .map(|(_, c, _)| *c)
        .collect();
    let (mut batteries, mut receivers) = (Vec::new(), Vec::new());
    let near = |g: CableGroup, c: atmos::Cell, r: f32| grid.nearest(g, c, r).map(|(_, n)| n);
    for (id, cell, p) in parts {
        if let Some(s) = &p.supplier
            && let Some(n) = near(s.group, *cell, 1.5)
        {
            let mut supply = Supply::new(s.max_supply);
            supply.ramp_rate = s.ramp_rate;
            supply.ramp_tolerance = s.ramp_tolerance;
            let nid = net(&mut state, &mut net_of, s.group, n);
            state.add_supply(nid, supply);
        }
        if let Some(b) = &p.battery {
            let from = match b.from {
                Source::Group(g) => near(g, *cell, 1.5).map(|n| (g, n)),
                Source::Terminal => terminals
                    .iter()
                    .copied()
                    .filter(|t| {
                        (((t.0 - cell.0).pow(2) + (t.1 - cell.1).pow(2)) as f32).sqrt() <= 1.5
                    })
                    .find_map(|t| {
                        grid.nearest(CableGroup::High, t, 1.0)
                            .map(|(_, n)| (CableGroup::High, n))
                    }),
            };
            let to = near(b.to, *cell, 1.5).map(|n| (b.to, n));
            if let (Some((fg, fnet)), Some((tg, tnet))) = (from, to) {
                let storage = charges
                    .get(id)
                    .copied()
                    .unwrap_or(b.starting_charge)
                    .min(b.capacity);
                let mut battery = Battery::new(b.capacity, storage);
                battery.max_charge_rate = b.max_charge_rate;
                battery.max_supply = b.max_supply;
                battery.supply_ramp_rate = b.ramp_rate;
                battery.supply_ramp_tolerance = b.ramp_tolerance;
                let cn = net(&mut state, &mut net_of, fg, fnet);
                let dn = net(&mut state, &mut net_of, tg, tnet);
                batteries.push((*id, state.add_battery(cn, dn, battery)));
            }
        }
        if let Some(load) = p.receiver_load
            && let Some(n) = near(CableGroup::Apc, *cell, RECEPTION_RANGE)
        {
            let nid = net(&mut state, &mut net_of, CableGroup::Apc, n);
            receivers.push((*id, state.add_load(nid, Load::new(load)), load));
        }
    }
    Built {
        state,
        batteries,
        receivers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(n: u128) -> ObjectId {
        ObjectId::from_u128(n)
    }

    type Chain = (
        Vec<(CableGroup, atmos::Cell)>,
        Vec<(ObjectId, atmos::Cell, PowerPart)>,
    );

    fn chain(generator: bool) -> Chain {
        let mut cables = Vec::new();
        // ВН: клетки 0..4 в строке 0; СН: 5..8; АПЦ: 9..12.
        cables.extend((0..5).map(|x| (CableGroup::High, (x, 0))));
        cables.extend((5..9).map(|x| (CableGroup::Medium, (x, 0))));
        cables.extend((9..13).map(|x| (CableGroup::Apc, (x, 0))));
        let mut parts = Vec::new();
        if generator {
            parts.push((
                oid(1),
                (0, 0),
                PowerPart {
                    supplier: Some(SupplierSpec {
                        group: CableGroup::High,
                        max_supply: 3000.0,
                        ramp_rate: 500.0,
                        ramp_tolerance: 500.0,
                    }),
                    ..PowerPart::default()
                },
            ));
        }
        let battery = |from, to, cap: f32, charge: f32, rate: f32, supply: f32| BatterySpec {
            from: Source::Group(from),
            to,
            capacity: cap,
            starting_charge: charge,
            max_charge_rate: rate,
            max_supply: supply,
            ramp_rate: 1000.0,
            ramp_tolerance: 5000.0,
        };
        // Подстанция на стыке ВН и СН (клетка 4–5), АПЦ на стыке СН и АПЦ (8–9).
        parts.push((
            oid(2),
            (4, 0),
            PowerPart {
                battery: Some(BatterySpec {
                    to: CableGroup::Medium,
                    ..battery(
                        CableGroup::High,
                        CableGroup::Medium,
                        2.5e6,
                        0.0,
                        150_000.0,
                        150_000.0,
                    )
                }),
                ..PowerPart::default()
            },
        ));
        parts.push((
            oid(3),
            (8, 0),
            PowerPart {
                battery: Some(battery(
                    CableGroup::Medium,
                    CableGroup::Apc,
                    50_000.0,
                    0.0,
                    5000.0,
                    10_000.0,
                )),
                ..PowerPart::default()
            },
        ));
        parts.push((
            oid(4),
            (12, 2),
            PowerPart {
                receiver_load: Some(200.0),
                ..PowerPart::default()
            },
        ));
        parts.push((
            oid(5),
            (30, 30),
            PowerPart {
                receiver_load: Some(200.0),
                ..PowerPart::default()
            },
        ));
        (cables, parts)
    }

    fn run(b: &mut Built, seconds: f32) {
        for _ in 0..(seconds * 20.0) as usize {
            b.state.tick(0.05);
        }
    }

    #[test]
    fn a_generator_lights_a_receiver_through_substation_and_apc() {
        let (cables, parts) = chain(true);
        let mut b = build(&cables, &parts, &BTreeMap::new());
        assert_eq!(b.batteries.len(), 2, "подстанция и АПЦ нашли свои сети");
        run(&mut b, 120.0);
        let near = b
            .receivers
            .iter()
            .find(|r| r.0 == oid(4))
            .expect("рядом с проводом");
        assert!(
            b.state.loads[near.1].receiving_power >= 199.0,
            "{}",
            b.state.loads[near.1].receiving_power
        );
        assert!(
            b.receivers.iter().all(|r| r.0 != oid(5)),
            "далеко от кабелей — без сети"
        );
    }

    #[test]
    fn without_a_source_nothing_is_powered_and_charges_survive_a_rebuild() {
        let (cables, parts) = chain(false);
        let mut b = build(&cables, &parts, &BTreeMap::new());
        run(&mut b, 30.0);
        let r = b.receivers[0].1;
        assert!(b.state.loads[r].receiving_power < 1.0);
        // Заряды из прошлой сборки подставляются обратно.
        let charges: BTreeMap<ObjectId, f32> = [(oid(2), 1_000_000.0)].into();
        let b2 = build(&cables, &parts, &charges);
        let sub = b2.batteries.iter().find(|x| x.0 == oid(2)).unwrap().1;
        assert_eq!(b2.state.batteries[sub].current_storage, 1_000_000.0);
    }
}
