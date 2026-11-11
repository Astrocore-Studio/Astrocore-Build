//! Геометрия мира для атмосферы: какие клетки есть, где воздух, где космос и какие рёбра
//! закрыты стеной, окном или закрытой дверью (`AirtightComponent` SS14).

use std::collections::BTreeMap;

use astrocore_core::ObjectId;
use astrocore_math::EdgeDir;
use astrocore_world::{AuthoredKind, OpeningKind, Placement, StructureKind, World};
use atmos::{Cell, CellKind, Layout, edge};

/// Метаданные области, которая считается космосом (решётка, пробоина).
pub const ATMOSPHERE_KEY: &str = "atmosphere";

/// Раскладка атмосферы по этажам. `door_open` — состояние двери из движка.
pub fn layouts(world: &World, door_open: &dyn Fn(ObjectId) -> bool) -> BTreeMap<i32, Layout> {
    let mut out: BTreeMap<i32, Layout> = BTreeMap::new();
    for e in world.elements() {
        let l = out.entry(e.floor.0).or_default();
        match (&e.kind, &e.placement) {
            (StructureKind::Floor, Placement::Cell(c)) => {
                l.cells.insert((c.x, c.z), CellKind::Air);
            }
            (StructureKind::Wall, Placement::Edge(g)) => {
                l.blocked.insert(edge_cells(g));
            }
            (StructureKind::Opening(OpeningKind::Window), Placement::Edge(g)) => {
                l.blocked.insert(edge_cells(g));
            }
            (StructureKind::Opening(OpeningKind::Door), Placement::Edge(g)) if !door_open(e.id) => {
                l.blocked.insert(edge_cells(g));
            }
            _ => {}
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
        if let Some(l) = out.get_mut(&o.floor.0) {
            for (cell, kind) in &mut l.cells {
                let (cx, cz) = (cell.0 as f32 + 0.5, cell.1 as f32 + 0.5);
                if cx >= min.x && cx <= max.x && cz >= min.z && cz <= max.z {
                    *kind = CellKind::Space;
                }
            }
        }
    }
    out.retain(|_, l| !l.cells.is_empty());
    out
}

fn edge_cells(g: &astrocore_math::GridEdge) -> atmos::Edge {
    let (a, b): (Cell, Cell) = match g.dir {
        EdgeDir::AlongX => ((g.x, g.z - 1), (g.x, g.z)),
        EdgeDir::AlongZ => ((g.x - 1, g.z), (g.x, g.z)),
    };
    edge(a, b)
}
