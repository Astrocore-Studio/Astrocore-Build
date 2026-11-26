use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use astrocore_grid2d::{Cell, Grid, Tile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Layout {
    pub name: String,
    pub grid: Grid,
    pub prototypes: BTreeMap<String, Prototype>,
    pub roles: Vec<String>,
    pub(crate) spawns: Vec<Cell>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prototype {
    pub name: String,
    pub kind: Kind,
    pub color: (u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Door {
        watts: u16,
    },
    Generator {
        watts: u16,
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
}

impl Kind {
    pub(crate) fn item_materials(self) -> Option<u8> {
        match self {
            Self::Item { materials, .. } => Some(materials),
            Self::Backpack { .. } => Some(0),
            _ => None,
        }
    }
}

pub const MAX_BACKPACK_SLOTS: u8 = 8;

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct MapData {
    pub name: String,
    pub grid: Vec<String>,
    pub spawns: Vec<Cell>,
    pub placements: Vec<Placement>,
    pub roles: Vec<String>,
    #[serde(default)]
    pub loadouts: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub recovery: Option<RecoverySpec>,
    #[serde(default)]
    pub cables: Vec<Cell>,
    pub round_seconds: u32,
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Placement {
    pub prototype: String,
    pub cell: Cell,
    #[serde(default)]
    pub starts_off: bool,
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
    pub cables: Vec<Cell>,
    pub round_seconds: u32,
    pub loadouts: BTreeMap<String, Vec<String>>,
    pub recovery: Option<RecoverySpec>,
}

impl Content {
    pub fn load(directory: Option<&Path>) -> Result<Self, String> {
        match directory {
            Some(directory) => {
                let read = |file: &str| {
                    let path = directory.join(file);
                    std::fs::read_to_string(&path)
                        .map_err(|error| format!("{}: {error}", path.display()))
                };
                Self::parse(&read("station.ron")?, &read("prototypes.ron")?)
            }
            None => Self::parse(
                include_str!("../content/station.ron"),
                include_str!("../content/prototypes.ron"),
            ),
        }
    }

    fn parse(map: &str, prototypes: &str) -> Result<Self, String> {
        let data: MapData = ron::from_str(map).map_err(|error| format!("station.ron: {error}"))?;
        let prototypes: BTreeMap<String, Prototype> =
            ron::from_str(prototypes).map_err(|error| format!("prototypes.ron: {error}"))?;
        Self::from_data(data, prototypes)
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
                || matches!(prototype.kind, Kind::Backpack { slots }
                    if !(1..=MAX_BACKPACK_SLOTS).contains(&slots))
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
        let grid = parse_grid(&data.grid)?;
        let mut occupied = BTreeSet::new();
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
        if data.cables.len() > 4096
            || data
                .cables
                .iter()
                .any(|cell| grid.tile(*cell) != Tile::Floor)
        {
            return Err("кабели должны находиться на полу карты".into());
        }
        if let Some(recovery) = &data.recovery {
            let group = power::CableGroup::Apc;
            let coordinates = |cell: Cell| (cell.x, cell.y);
            let cables =
                power::Cables::build(data.cables.iter().map(|&cell| (group, coordinates(cell))));
            let vent_net = cables.net_at(group, coordinates(recovery.vent));
            if vent_net.is_none()
                || !data.placements.iter().any(|placement| {
                    matches!(
                        prototypes[&placement.prototype].kind,
                        Kind::Generator { watts: 1.. }
                    ) && cables.net_at(group, coordinates(placement.cell)) == vent_net
                })
            {
                return Err("вентиляция цели должна быть соединена кабелем с генератором".into());
            }
        }
        if data.spawns.is_empty()
            || data
                .spawns
                .iter()
                .any(|cell| grid.tile(*cell) != Tile::Floor || occupied.contains(cell))
        {
            return Err("точки появления должны находиться на свободном полу".into());
        }
        Ok(Self {
            layout: Layout {
                name: data.name,
                grid,
                prototypes,
                roles: data.roles,
                spawns: data.spawns,
            },
            placements: data.placements,
            cables: data.cables,
            round_seconds: data.round_seconds,
            loadouts: data.loadouts,
            recovery: data.recovery,
        })
    }
}

pub(crate) fn parse_grid(rows: &[String]) -> Result<Grid, String> {
    let width = rows.first().map_or(0, String::len);
    if width > 64 || rows.len() > 64 {
        return Err("максимальный размер карты 64 × 64".into());
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
    Grid::new(width as u16, rows.len() as u16, tiles).map_err(str::to_owned)
}
