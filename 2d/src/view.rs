use astrocore_grid2d::{Cell, Grid, Position, Tile};
use bevy::prelude::*;

use crate::{CableGroup, CableSegment, PipeDirection, PipeShape};

pub struct MapView {
    offset: Vec2,
    z: i16,
    pub scale: f32,
}

impl MapView {
    pub fn new(grid: &Grid, z: i16) -> Self {
        let (width, height) = grid.size_at(z).unwrap_or_else(|| grid.size());
        let size = Vec2::new(f32::from(width), f32::from(height));
        Self {
            offset: size / 2.0,
            z,
            scale: 42.0_f32.min(960.0 / size.x).min(465.0 / size.y),
        }
    }

    pub fn world(&self, position: Position) -> Vec2 {
        (Vec2::new(position.x, position.y) - self.offset) * self.scale
    }

    pub fn cell(&self, point: Vec2) -> Cell {
        let point = point / self.scale + self.offset;
        Position {
            x: point.x,
            y: point.y,
            z: self.z,
        }
        .cell()
    }

    pub fn level_marker(&self, cell: Cell) -> impl Bundle {
        let point = self.world(cell.center()) + Vec2::Y * 0.32 * self.scale;
        (
            Text2d::new("↕"),
            TextFont::from_font_size((self.scale * 0.44).max(11.0)),
            TextColor(Color::srgb_u8(236, 203, 123)),
            Transform::from_translation(point.extend(0.8)),
        )
    }

    pub fn cable(&self, segment: CableSegment) -> (Vec2, Sprite) {
        let (offset, color) = match segment.voltage {
            CableGroup::Apc => (-0.15, Color::srgb_u8(110, 207, 146)),
            CableGroup::Medium => (0.0, Color::srgb_u8(234, 216, 101)),
            CableGroup::High => (0.15, Color::srgb_u8(240, 153, 88)),
        };
        let point = self.world(segment.cell.center()) + Vec2::Y * self.scale * offset;
        let sprite = Sprite::from_color(color, Vec2::new(0.75, 0.09) * self.scale);
        (point, sprite)
    }

    pub fn pipe_segments(
        &self,
        shape: PipeShape,
        facing: PipeDirection,
    ) -> impl Iterator<Item = (Vec2, Sprite)> + '_ {
        shape.ports(facing).map(move |direction| {
            let [x, y] = direction.offset();
            let offset = Vec2::new(x as f32, y as f32) * self.scale * 0.25;
            let dimensions = if x != 0 {
                Vec2::new(0.52, 0.12)
            } else {
                Vec2::new(0.12, 0.52)
            };
            (
                offset,
                Sprite::from_color(Color::srgb_u8(114, 169, 149), dimensions * self.scale),
            )
        })
    }

    pub fn tile_sprite(&self, cell: Cell, tile: Tile) -> Option<Sprite> {
        let color = match tile {
            Tile::Void => return None,
            Tile::Floor if (cell.x + cell.y) % 2 == 0 => Color::srgb_u8(38, 56, 70),
            Tile::Floor => Color::srgb_u8(42, 62, 76),
            Tile::Wall => Color::srgb_u8(74, 105, 125),
        };
        Some(Sprite::from_color(color, Vec2::splat(self.scale - 1.0)))
    }
}
