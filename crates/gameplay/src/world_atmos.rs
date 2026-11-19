//! Геометрия мира для атмосферы: какие клетки есть, где воздух, где космос и какие рёбра
//! закрыты стеной, окном или закрытой дверью (`AirtightComponent` SS14).

use astrocore_core::ObjectId;
use astrocore_math::{EdgeDir, Facing};
use astrocore_world::{AuthoredKind, OpeningKind, Placement, StructureKind, World};
use atmos::{Cell, CellKind, FLOOR_STRIDE, Layout, edge, floor_cell};

/// Метаданные области, которая считается космосом (решётка, пробоина).
pub const ATMOSPHERE_KEY: &str = "atmosphere";

/// Раскладка атмосферы всего мира: этажи лежат в одной сетке (`atmos::floor_cell`), лестницы —
/// вертикальные связи. `door_open` — состояние двери из движка.
pub fn layout(world: &World, door_open: &dyn Fn(ObjectId) -> bool) -> Layout {
    let mut l = Layout::default();
    for e in world.elements() {
        let f = e.floor.0;
        let fc = |x: i32, z: i32| floor_cell(f, x, z);
        match (&e.kind, &e.placement) {
            (StructureKind::Floor, Placement::Cell(c)) => {
                let kind = if e.material.as_str() == "ss14:tile_lattice" {
                    CellKind::Space
                } else {
                    CellKind::Air
                };
                l.cells.insert(fc(c.x, c.z), kind);
            }
            (StructureKind::Wall, Placement::Edge(g))
                if !matches!(e.material.as_str(), "ss14:grille" | "ss14:grille_broken") =>
            {
                l.blocked.insert(edge_cells(f, g));
            }
            (StructureKind::Opening(OpeningKind::Window), Placement::Edge(g)) => {
                l.blocked.insert(edge_cells(f, g));
            }
            (StructureKind::Opening(OpeningKind::Door), Placement::Edge(g)) if !door_open(e.id) => {
                l.blocked.insert(edge_cells(f, g));
            }
            _ => {}
        }
    }
    // Лестницы соединяют нижнюю клетку марша с клеткой, где марш выходит на верхнем этаже.
    for e in world.elements() {
        if let (StructureKind::Stair { to }, Placement::Run { start, dir, length }) =
            (&e.kind, &e.placement)
        {
            let (dx, dz) = match dir {
                Facing::North => (0, -1),
                Facing::South => (0, 1),
                Facing::East => (1, 0),
                Facing::West => (-1, 0),
            };
            let n = i32::from(*length);
            let bottom = floor_cell(e.floor.0, start.x, start.z);
            let top = floor_cell(to.0, start.x + dx * n, start.z + dz * n);
            let (lo, hi) = if e.floor.0 <= to.0 {
                (bottom, top)
            } else {
                (top, bottom)
            };
            l.links.insert((lo, hi));
        }
    }
    // Рёбра, запертые для воздуха сущностями `Airtight` (импортёр: записи `airtight_<n>` с
    // `<X|Z>:<x>:<z>`): проходу людей они не мешают, воздух не пропускают.
    for o in world.authored() {
        if !matches!(o.kind, AuthoredKind::Zone) {
            continue;
        }
        for (key, value) in &o.metadata {
            if !key.starts_with("airtight_") {
                continue;
            }
            for token in value.split(' ') {
                let mut parts = token.split(':');
                let (Some(dir), Some(Ok(x)), Some(Ok(z))) = (
                    parts.next(),
                    parts.next().map(str::parse::<i32>),
                    parts.next().map(str::parse::<i32>),
                ) else {
                    continue;
                };
                let g = match dir {
                    "X" => astrocore_math::GridEdge::along_x(x, z),
                    "Z" => astrocore_math::GridEdge::along_z(x, z),
                    _ => continue,
                };
                l.blocked.insert(edge_cells(o.floor.0, &g));
            }
        }
    }
    // Области без атмосферы: клетки внутри становятся космосом.
    for o in world.authored() {
        if !matches!(o.kind, AuthoredKind::Zone) {
            continue;
        }
        let value = o.metadata.get(ATMOSPHERE_KEY).map(String::as_str);
        let is_space = matches!(value, Some("space" | "vacuum"));
        let fill = value.and_then(fill_of);
        if !is_space && fill.is_none() {
            continue;
        }
        let (min, max) = (o.min(), o.max());
        let f = o.floor.0;
        for (cell, kind) in &mut l.cells {
            if (cell.1 + FLOOR_STRIDE / 2).div_euclid(FLOOR_STRIDE) != f {
                continue;
            }
            let z = cell.1 - f * FLOOR_STRIDE;
            let (cx, cz) = (cell.0 as f32 + 0.5, z as f32 + 0.5);
            if cx >= min.x && cx <= max.x && cz >= min.z && cz <= max.z {
                if is_space {
                    *kind = CellKind::Space;
                } else if let Some(f) = &fill {
                    l.fills.insert(*cell, f.clone());
                }
            }
        }
    }
    l
}

/// Состав воздуха области по значению `atmosphere`: `plasma` — плазма в воздухе станции,
/// `tritium`, `frezon`, `n2o`, `ammonia`, `co2`.
fn fill_of(value: &str) -> Option<atmos::Fill> {
    let gas = match value {
        "plasma" => atmos::Gas::Plasma,
        "tritium" => atmos::Gas::Tritium,
        "frezon" => atmos::Gas::Frezon,
        "n2o" => atmos::Gas::NitrousOxide,
        "ammonia" => atmos::Gas::Ammonia,
        "co2" => atmos::Gas::CarbonDioxide,
        _ => return None,
    };
    Some(atmos::Fill {
        extra: vec![(gas, atmos::MOLES_CELL_STANDARD * 0.25)],
        temperature: 0.0,
    })
}

fn edge_cells(floor: i32, g: &astrocore_math::GridEdge) -> atmos::Edge {
    let (a, b): (Cell, Cell) = match g.dir {
        EdgeDir::AlongX => (floor_cell(floor, g.x, g.z - 1), floor_cell(floor, g.x, g.z)),
        EdgeDir::AlongZ => (floor_cell(floor, g.x - 1, g.z), floor_cell(floor, g.x, g.z)),
    };
    edge(a, b)
}

/// Этаж клетки общей сетки (обратное к `atmos::floor_cell`).
pub fn cell_floor(cell: Cell) -> astrocore_world::FloorId {
    astrocore_world::FloorId((cell.1 + FLOOR_STRIDE / 2).div_euclid(FLOOR_STRIDE))
}

/// Клетка на этаже замороженной карты.
pub fn cell_floor_paused(world: &World, cell: Cell) -> bool {
    world.floor_paused(cell_floor(cell))
}

/// Раскладка без клеток, рёбер и связей замороженных карт (ADR-0012): они не симулируются.
pub fn without_frozen(world: &World, full: &Layout) -> Layout {
    if world.maps().iter().all(|m| !m.paused) {
        return full.clone();
    }
    let live = |c: &Cell| !cell_floor_paused(world, *c);
    Layout {
        cells: full
            .cells
            .iter()
            .filter(|(c, _)| live(c))
            .map(|(c, k)| (*c, *k))
            .collect(),
        blocked: full
            .blocked
            .iter()
            .filter(|e| live(&e.0) && live(&e.1))
            .copied()
            .collect(),
        links: full
            .links
            .iter()
            .filter(|(a, b)| live(a) && live(b))
            .copied()
            .collect(),
        fills: full
            .fills
            .iter()
            .filter(|(c, _)| live(c))
            .map(|(c, f)| (*c, f.clone()))
            .collect(),
    }
}
