use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use astrocore_grid2d::{Cell, Grid, GridLevel, Tile};
use serde::{Deserialize, Serialize};

use crate::access::{AccessReader, JobCredential, valid_tags};
use crate::electricity::{CableSegment, StoragePorts};
use crate::environment::coordinates;
use crate::medical::Damage;
use crate::piping::{PipeDirection, PipePiece, PipeShape};
use crate::prototypes::load_catalog;
use power::CableGroup;

#[derive(Debug, Serialize, Deserialize)]
pub struct Layout {
    pub name: String,
    pub grid: Grid,
    pub prototypes: BTreeMap<String, Prototype>,
    pub roles: Vec<Job>,
    pub links: Vec<LevelLink>,
    pub pipes: BTreeMap<Cell, PipePiece>,
    pub(crate) spawns: Vec<Cell>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Job {
    pub name: String,
    pub slots: Option<u16>,
    pub spawns: Vec<Cell>,
    pub(crate) credential: Option<JobCredential>,
}

#[derive(Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct JobSettings {
    #[serde(default)]
    slots: Option<u16>,
    #[serde(default)]
    pub(crate) spawns: Vec<Cell>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential: Option<JobCredential>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prototype {
    pub name: String,
    pub kind: Kind,
    pub color: (u8, u8, u8),
    #[serde(default)]
    pub access_reader: Option<AccessReader>,
    #[serde(default)]
    pub access_levels: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Door {
        watts: u16,
    },
    Generator {
        watts: u16,
        #[serde(default)]
        voltage: CableGroup,
    },
    Vent {
        watts: u16,
        reserve_moles: u16,
        flow_moles: u16,
    },
    Item {
        healing: u8,
        #[serde(default)]
        materials: u8,
        #[serde(default)]
        analyzer: bool,
    },
    Backpack {
        slots: u8,
    },
    Medicine {
        restores: Damage,
        delay_seconds: u8,
    },
    MedicalStack {
        restores: Damage,
        use_ticks: u16,
        uses: u8,
    },
    HealthAnalyzer,
    CableCoil {
        segments: u8,
        #[serde(default)]
        voltage: CableGroup,
    },
    Wirecutters,
    Piping(PipeShape),
    EnergyStorage {
        capacity_joules: u32,
        initial_joules: u32,
        charge_watts: u32,
        supply_watts: u32,
        #[serde(default)]
        ports: StoragePorts,
    },
    IdCard,
    IdConsole {
        watts: u16,
    },
}

impl Kind {
    pub fn rotatable(self) -> bool {
        match self {
            Self::Piping(_) => true,
            Self::EnergyStorage { ports, .. } => ports.opposite_sides(),
            _ => false,
        }
    }

    pub(crate) fn item_materials(self) -> Option<u8> {
        match self {
            Self::Item { materials, .. } => Some(materials),
            Self::CableCoil { segments, .. } => Some(segments),
            Self::MedicalStack { uses, .. } => Some(uses),
            Self::Backpack { .. }
            | Self::Medicine { .. }
            | Self::HealthAnalyzer
            | Self::IdCard
            | Self::Wirecutters => Some(0),
            _ => None,
        }
    }
}

pub const MAX_BACKPACK_SLOTS: u8 = 8;
pub(crate) const MAX_CABLES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct LevelLink {
    pub lower: Cell,
    pub upper: Cell,
}

impl LevelLink {
    pub fn contains(self, cell: Cell) -> bool {
        self.lower == cell || self.upper == cell
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct MapData {
    pub name: String,
    pub grid: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub levels: BTreeMap<i16, Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LevelLink>,
    pub spawns: Vec<Cell>,
    pub placements: Vec<Placement>,
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub jobs: BTreeMap<String, JobSettings>,
    #[serde(default)]
    pub loadouts: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub recovery: Option<RecoverySpec>,
    #[serde(default)]
    pub cables: Vec<Cell>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub medium_cables: Vec<Cell>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub high_cables: Vec<Cell>,
    pub round_seconds: u32,
}

impl MapData {
    pub(crate) fn cable_cells(&self) -> impl Iterator<Item = CableSegment> + '_ {
        [
            (CableGroup::Apc, &self.cables),
            (CableGroup::Medium, &self.medium_cables),
            (CableGroup::High, &self.high_cables),
        ]
        .into_iter()
        .flat_map(|(voltage, cells)| {
            cells
                .iter()
                .map(move |&cell| CableSegment { cell, voltage })
        })
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Placement {
    pub prototype: String,
    pub cell: Cell,
    #[serde(default)]
    pub starts_off: bool,
    #[serde(default)]
    pub facing: PipeDirection,
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct RecoverySpec {
    pub breach: Cell,
    pub chamber: Cell,
    pub vent: Cell,
    pub minimum_kpa: u8,
    pub hold_seconds: u8,
}

pub(crate) struct Content {
    pub layout: Layout,
    pub placements: Vec<Placement>,
    pub cables: Vec<CableSegment>,
    pub round_seconds: u32,
    pub loadouts: BTreeMap<String, Vec<String>>,
    pub recovery: Option<RecoverySpec>,
}

impl Content {
    pub fn load(directory: Option<&Path>) -> Result<Self, String> {
        let map = match directory {
            Some(directory) => {
                let path = directory.join("station.ron");
                std::fs::read_to_string(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?
                    .into()
            }
            None => std::borrow::Cow::Borrowed(include_str!("../content/station.ron")),
        };
        let data: MapData = ron::from_str(&map).map_err(|error| format!("station.ron: {error}"))?;
        Self::from_data(data, load_catalog(directory)?)
    }

    pub fn from_data(
        data: MapData,
        prototypes: BTreeMap<String, Prototype>,
    ) -> Result<Self, String> {
        let valid_name = |name: &str| !name.trim().is_empty() && name.len() <= 48;
        if !valid_name(&data.name)
            || data.roles.is_empty()
            || data.roles.len() > 9
            || data.roles.iter().any(|role| !valid_name(role))
            || !(10..=3600).contains(&data.round_seconds)
        {
            return Err("неверное название станции, роли или длительность раунда".into());
        }
        if prototypes.len() > 96 || data.placements.len() > 128 || data.spawns.len() > 64 {
            return Err("превышен лимит прототипов, размещений или точек появления".into());
        }
        for (id, prototype) in &prototypes {
            if !valid_name(id)
                || !valid_name(&prototype.name)
                || prototype.access_reader.as_ref().is_some_and(|reader| {
                    !matches!(prototype.kind, Kind::Door { .. } | Kind::IdConsole { .. })
                        || !reader.valid()
                })
                || !valid_tags(&prototype.access_levels)
                || !prototype.access_levels.is_empty()
                    && !matches!(prototype.kind, Kind::IdConsole { .. })
                || matches!(prototype.kind, Kind::IdConsole { watts: 0 })
                || matches!(prototype.kind, Kind::Backpack { slots }
                    if !(1..=MAX_BACKPACK_SLOTS).contains(&slots))
                || matches!(prototype.kind, Kind::Medicine { restores, delay_seconds }
                    if restores.total() == 0 || !(1..=15).contains(&delay_seconds))
                || matches!(prototype.kind, Kind::MedicalStack { restores, use_ticks, uses }
                    if restores.total() == 0 || use_ticks == 0 || use_ticks > 15 * astrocore_grid2d::session::TICK_HZ as u16 || uses == 0)
                || matches!(prototype.kind, Kind::CableCoil { segments: 0, .. })
                || matches!(prototype.kind, Kind::Piping(shape) if !shape.valid())
                || matches!(prototype.kind, Kind::EnergyStorage {
                    capacity_joules, initial_joules, charge_watts, supply_watts, ..
                } if capacity_joules == 0 || initial_joules > capacity_joules
                    || charge_watts == 0 || supply_watts == 0)
            {
                return Err(format!("неверные параметры прототипа {id:?}"));
            }
        }
        for (role, equipment) in &data.loadouts {
            if !data.roles.contains(role)
                || equipment.len() > 2
                || equipment.iter().any(|id| {
                    !prototypes
                        .get(id)
                        .is_some_and(|prototype| prototype.kind.item_materials().is_some())
                })
            {
                return Err(format!("неверное стартовое снаряжение роли {role:?}"));
            }
        }
        let grid = parse_grid(&data)?;
        let mut occupied = BTreeSet::new();
        let mut pipes = BTreeMap::new();
        for placement in &data.placements {
            let prototype = prototypes
                .get(&placement.prototype)
                .ok_or_else(|| format!("неизвестный прототип {}", placement.prototype))?;
            if grid.tile(placement.cell) != Tile::Floor {
                return Err(format!("размещение вне пола: {:?}", placement.cell));
            }
            if prototype.kind.item_materials().is_none() && !occupied.insert(placement.cell) {
                return Err(format!(
                    "два устройства на одной клетке: {:?}",
                    placement.cell
                ));
            }
            if let Kind::Piping(shape) = prototype.kind {
                pipes.insert(
                    placement.cell,
                    PipePiece {
                        shape,
                        facing: placement.facing,
                        starts_off: placement.starts_off,
                    },
                );
            }
        }
        let mut stairs_up = BTreeSet::new();
        let mut stairs_down = BTreeSet::new();
        for link in &data.links {
            if link.lower.z.checked_add(1) != Some(link.upper.z)
                || [link.lower, link.upper]
                    .into_iter()
                    .any(|cell| grid.tile(cell) != Tile::Floor || occupied.contains(&cell))
                || !stairs_up.insert(link.lower)
                || !stairs_down.insert(link.upper)
            {
                return Err(format!("неверный переход между этажами: {link:?}"));
            }
        }
        if let Some(recovery) = &data.recovery
            && (!grid.contains(recovery.breach)
                || grid.tile(recovery.breach) == Tile::Floor
                || grid.tile(recovery.chamber) != Tile::Floor
                || !(1..=100).contains(&recovery.minimum_kpa)
                || !(1..=60).contains(&recovery.hold_seconds)
                || !data.placements.iter().any(|placement| {
                    placement.cell == recovery.vent
                        && matches!(
                            prototypes[&placement.prototype].kind,
                            Kind::Vent {
                                watts: 1..,
                                reserve_moles: 1..,
                                flow_moles: 1..
                            }
                        )
                }))
        {
            return Err(
                "цель восстановления требует пробоины, точки измерения и вентиляции".into(),
            );
        }
        let cables: Vec<_> = data.cable_cells().collect();
        if cables.len() > MAX_CABLES
            || cables
                .iter()
                .any(|cable| grid.tile(cable.cell) != Tile::Floor)
        {
            return Err("кабели должны находиться на полу карты".into());
        }
        if let Some(recovery) = &data.recovery
            && !has_recovery_supply(recovery.vent, &data, &prototypes, &cables)
        {
            return Err("вентиляции цели нужен кабельный путь от генератора".into());
        }
        if data.spawns.is_empty()
            || data
                .spawns
                .iter()
                .any(|cell| grid.tile(*cell) != Tile::Floor || occupied.contains(cell))
        {
            return Err("точки появления должны находиться на свободном полу".into());
        }
        for (name, job) in &data.jobs {
            if data.roles.iter().filter(|role| *role == name).count() != 1
                || job.spawns.len() > 64
                || job.credential.as_ref().is_some_and(|credential| {
                    !valid_tags(&credential.access)
                        || !prototypes
                            .get(&credential.prototype)
                            .is_some_and(|prototype| prototype.kind == Kind::IdCard)
                })
                || job
                    .spawns
                    .iter()
                    .any(|&cell| grid.tile(cell) != Tile::Floor || occupied.contains(&cell))
            {
                return Err(format!("неверная настройка профессии {name:?}"));
            }
        }
        let roles = data
            .roles
            .into_iter()
            .map(|name| {
                let job = data.jobs.get(&name).cloned().unwrap_or_default();
                Job {
                    name,
                    slots: job.slots,
                    spawns: job.spawns,
                    credential: job.credential,
                }
            })
            .collect();
        Ok(Self {
            layout: Layout {
                name: data.name,
                grid,
                prototypes,
                roles,
                links: data.links,
                pipes,
                spawns: data.spawns,
            },
            placements: data.placements,
            cables,
            round_seconds: data.round_seconds,
            loadouts: data.loadouts,
            recovery: data.recovery,
        })
    }
}

fn has_recovery_supply(
    vent: Cell,
    data: &MapData,
    prototypes: &BTreeMap<String, Prototype>,
    segments: &[CableSegment],
) -> bool {
    let cables = power::Cables::build(segments.iter().map(|segment| segment.network_key()));
    let Some(target) = cables.net_at(CableGroup::Apc, coordinates(vent)) else {
        return false;
    };
    let mut sources = Vec::new();
    let mut bridges = BTreeMap::<usize, Vec<usize>>::new();
    for placement in &data.placements {
        match prototypes[&placement.prototype].kind {
            Kind::Generator {
                watts: 1..,
                voltage,
            } => {
                sources.extend(cables.net_at(voltage, coordinates(placement.cell)));
            }
            Kind::EnergyStorage { ports, .. } => {
                if let (Some(input), Some(output)) =
                    ports.nets(&cables, placement.cell, placement.facing)
                    && input != output
                {
                    bridges.entry(input).or_default().push(output);
                }
            }
            _ => {}
        }
    }
    let mut visited = BTreeSet::new();
    while let Some(net) = sources.pop() {
        if net == target {
            return true;
        }
        if visited.insert(net)
            && let Some(next) = bridges.get(&net)
        {
            sources.extend(next);
        }
    }
    false
}

pub(crate) fn parse_grid(data: &MapData) -> Result<Grid, String> {
    let mut levels = BTreeMap::new();
    for (&z, rows) in &data.levels {
        if z == 0 {
            return Err("уровень Z = 0 задаётся полем grid".into());
        }
        levels.insert(z, parse_level(rows)?);
    }
    levels.insert(0, parse_level(&data.grid)?);
    Grid::from_levels(levels).map_err(str::to_owned)
}

fn parse_level(rows: &[String]) -> Result<GridLevel, String> {
    let width = rows.first().map_or(0, String::len);
    if width > 64 || rows.len() > 64 {
        return Err("максимальный размер этажа 64 × 64".into());
    }
    let mut tiles = Vec::new();
    for row in rows.iter().rev() {
        if row.len() != width {
            return Err("все ряды карты должны иметь одинаковую длину".into());
        }
        for symbol in row.bytes() {
            tiles.push(match symbol {
                b'~' => Tile::Void,
                b'#' => Tile::Wall,
                b'.' => Tile::Floor,
                _ => return Err(format!("неизвестный символ карты {:?}", char::from(symbol))),
            });
        }
    }
    GridLevel::new(width as u16, rows.len() as u16, tiles).map_err(str::to_owned)
}
