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
                l.cells.insert(fc(c.x, c.z), CellKind::Air);
            }
            (StructureKind::Wall, Placement::Edge(g)) => {
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
    // Области без атмосферы: клетки внутри становятся космосом.
    for o in world.authored() {
        if !matches!(o.kind, AuthoredKind::Zone) {
            continue;
        }
        let is_space = o
            .metadata
            .get(ATMOSPHERE_KEY)
            .is_some_and(|v| v == "space" || v == "vacuum");
        if !is_space {
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
                *kind = CellKind::Space;
            }
        }
    }
    l
}

fn edge_cells(floor: i32, g: &astrocore_math::GridEdge) -> atmos::Edge {
    let (a, b): (Cell, Cell) = match g.dir {
        EdgeDir::AlongX => (floor_cell(floor, g.x, g.z - 1), floor_cell(floor, g.x, g.z)),
        EdgeDir::AlongZ => (floor_cell(floor, g.x - 1, g.z), floor_cell(floor, g.x, g.z)),
    };
    edge(a, b)
}
