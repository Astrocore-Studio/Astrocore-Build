use astrocore_grid2d::session::PlayerId;
use astrocore_grid2d::{Cell, Tile};

use crate::Station;
use crate::content::Kind;

impl Station {
    pub(crate) fn construct(&mut self, player: PlayerId, cell: Cell, tile: Tile) {
        let crew = &self.state.players[&player];
        let origin = crew.position.cell();
        if origin
            .distance_on_level(cell)
            .is_none_or(|distance| distance > 1)
            || !self.terrain.contains(cell)
        {
            return;
        }
        let Some(kit) = crew.hands[usize::from(crew.active_hand)] else {
            return;
        };
        let item = &self.state.items[&kit];
        let Kind::Item {
            materials: capacity,
            ..
        } = self.layout.prototypes[&item.prototype].kind
        else {
            return;
        };
        if capacity == 0
            || self.state.cables.contains_key(&cell)
            || self.layout.spawns.contains(&cell)
            || self
                .layout
                .roles
                .iter()
                .any(|role| role.spawns.contains(&cell))
            || self.layout.links.iter().any(|link| link.contains(cell))
            || self.state.doors.contains_key(&cell)
            || self.state.generators.contains_key(&cell)
            || self.state.storages.contains_key(&cell)
            || self.state.vents.contains_key(&cell)
            || self.layout.pipes.contains_key(&cell)
            || self
                .state
                .items
                .values()
                .any(|item| item.ground == Some(cell))
            || self
                .state
                .players
                .values()
                .filter(|crew| crew.on_duty())
                .any(|crew| crew.position.overlaps(cell))
        {
            return;
        }

        let next_stock = match (self.terrain.tile(cell), tile) {
            (Tile::Floor | Tile::Wall, Tile::Void) => {
                item.materials.saturating_add(1).min(capacity)
            }
            (Tile::Void, Tile::Floor | Tile::Wall) | (Tile::Floor, Tile::Wall) => {
                let Some(stock) = item.materials.checked_sub(1) else {
                    return;
                };
                stock
            }
            _ => return,
        };

        self.terrain.set_tile(cell, tile);
        if self.layout.grid.tile(cell) == tile {
            self.state.terrain.remove(&cell);
        } else {
            self.state.terrain.insert(cell, tile);
        }
        self.state.terrain_revision += 1;
        self.state
            .items
            .get_mut(&kit)
            .expect("активный инструмент существует")
            .materials = next_stock;
    }
}
