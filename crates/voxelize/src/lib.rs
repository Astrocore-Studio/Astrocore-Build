//! Конвертер спрайтов в воксельные модели (крейт общий для инструмента `voxelize` и клиента) (ADR-0002 Build, формат — ADR-0011 движка).
//!
//! * Четыре направления (S, N, E, W) — объём пересечением силуэтов спереди, сзади и с боков;
//!   цвет вокселя — с той стороны, откуда он виден.
//! * Одно направление, стоячая вещь (мебель, устройства) — выдавливание вглубь примерно на 0,7
//!   ширины силуэта; бока и верх берут цвет чуть внутри силуэта, без тёмной обводки спрайта.
//! * Лежащая вещь (предметы на полу) — рисунок лежит лицом вверх, толщина — из манифеста.
//!
//! Оси — как в движке: X слева направо на виде спереди, Y вверх, перед смотрит в +Z.

use astrocore_voxels::{Rgba, VoxelModel};
use image::RgbaImage;

/// Воксели до модели: размер и цвет каждой клетки.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub size: [usize; 3],
    cells: Vec<Option<Rgba>>,
}

impl Grid {
    fn new(size: [usize; 3]) -> Self {
        Self {
            size,
            cells: vec![None; size[0] * size[1] * size[2]],
        }
    }

    fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.size[0] * (y + self.size[1] * z)
    }

    pub fn get(&self, x: i64, y: i64, z: i64) -> Option<Rgba> {
        let [sx, sy, sz] = self.size.map(|s| s as i64);
        if x < 0 || y < 0 || z < 0 || x >= sx || y >= sy || z >= sz {
            return None;
        }
        self.cells[self.idx(x as usize, y as usize, z as usize)]
    }

    fn set(&mut self, x: usize, y: usize, z: usize, c: Rgba) {
        let i = self.idx(x, y, z);
        self.cells[i] = Some(c);
    }

    pub fn filled(&self) -> usize {
        self.cells.iter().filter(|c| c.is_some()).count()
    }
}

fn opaque(img: &RgbaImage, x: i64, y: i64) -> bool {
    x >= 0
        && y >= 0
        && (x as u32) < img.width()
        && (y as u32) < img.height()
        && img.get_pixel(x as u32, y as u32)[3] > 127
}

fn px(img: &RgbaImage, x: i64, y: i64) -> Rgba {
    img.get_pixel(x as u32, y as u32).0
}

/// Цвет пикселя чуть внутри силуэта (на 2, затем 1 пиксель в сторону `d`): у спрайтов SS14
/// тёмная обводка, и бока, покрашенные ею, выглядят чёрными.
fn inner(img: &RgbaImage, x: i64, y: i64, d: (i64, i64)) -> Rgba {
    for k in [2, 1, 0] {
        let (xx, yy) = (x + d.0 * k, y + d.1 * k);
        if opaque(img, xx, yy) {
            return px(img, xx, yy);
        }
    }
    px(img, x, y)
}

fn shade(c: Rgba, k: f32) -> Rgba {
    [
        (f32::from(c[0]) * k) as u8,
        (f32::from(c[1]) * k) as u8,
        (f32::from(c[2]) * k) as u8,
        c[3],
    ]
}

/// Четыре вида (S, N, E, W) одного размера → объём. `z = 0` у нас — перед.
pub fn carve(views: [&RgbaImage; 4]) -> Grid {
    let [s, n, e, w] = views;
    let (wd, ht) = (s.width() as usize, s.height() as usize);
    let depth = e.width() as usize;
    let mut occ = vec![false; wd * ht * depth];
    let at = |x: usize, y: usize, z: usize| x + wd * (y + ht * z);
    let (wi, di) = (wd as i64, depth as i64);
    for x in 0..wd {
        for y in 0..ht {
            let (xi, yi) = (x as i64, y as i64);
            if !(opaque(s, xi, yi) && opaque(n, wi - 1 - xi, yi)) {
                continue;
            }
            for z in 0..depth {
                let zi = z as i64;
                // E: персонаж смотрит вправо, перед — справа картинки; W — наоборот.
                if opaque(e, di - 1 - zi, yi) && opaque(w, zi, yi) {
                    occ[at(x, y, z)] = true;
                }
            }
        }
    }
    let filled = |x: i64, y: i64, z: i64| {
        x >= 0
            && y >= 0
            && z >= 0
            && (x as usize) < wd
            && (y as usize) < ht
            && (z as usize) < depth
            && occ[at(x as usize, y as usize, z as usize)]
    };
    let mut g = Grid::new([wd, ht, depth]);
    for z in 0..depth {
        for y in 0..ht {
            for x in 0..wd {
                if !occ[at(x, y, z)] {
                    continue;
                }
                let (xi, yi, zi) = (x as i64, y as i64, z as i64);
                let c = if !filled(xi, yi, zi - 1) {
                    px(s, xi, yi)
                } else if !filled(xi, yi, zi + 1) {
                    px(n, wi - 1 - xi, yi)
                } else if !filled(xi - 1, yi, zi) {
                    px(e, di - 1 - zi, yi)
                } else if !filled(xi + 1, yi, zi) {
                    px(w, zi, yi)
                } else {
                    px(s, xi, yi)
                };
                // В оси движка: Y вверх, перед в +Z.
                g.set(x, ht - 1 - y, depth - 1 - z, c);
            }
        }
    }
    g
}

/// Ширина силуэта картинки (непрозрачные столбцы).
fn silhouette_width(img: &RgbaImage) -> usize {
    (0..img.width())
        .filter(|&x| (0..img.height()).any(|y| img.get_pixel(x, y)[3] > 127))
        .count()
}

/// Высота силуэта картинки (непрозрачные строки).
pub fn silhouette_height(img: &RgbaImage) -> usize {
    (0..img.height())
        .filter(|&y| (0..img.width()).any(|x| img.get_pixel(x, y)[3] > 127))
        .count()
}

/// Одно направление, стоячая вещь: выдавливание вглубь по центру.
pub fn extrude_upright(img: &RgbaImage) -> Grid {
    let (wd, ht) = (img.width() as usize, img.height() as usize);
    let depth = ((silhouette_width(img) as f32 * 0.7).round() as usize).clamp(4, 24);
    let mut g = Grid::new([wd, ht, depth]);
    let last = depth as i64 - 1;
    for y in 0..ht {
        for x in 0..wd {
            let (xi, yi) = (x as i64, y as i64);
            if !opaque(img, xi, yi) {
                continue;
            }
            let side = if !opaque(img, xi - 1, yi) {
                Some(inner(img, xi, yi, (1, 0)))
            } else if !opaque(img, xi + 1, yi) {
                Some(inner(img, xi, yi, (-1, 0)))
            } else if !opaque(img, xi, yi - 1) {
                Some(inner(img, xi, yi, (0, 1)))
            } else if !opaque(img, xi, yi + 1) {
                Some(inner(img, xi, yi, (0, -1)))
            } else {
                None
            };
            for z in 0..depth {
                let zi = z as i64;
                let c = if zi == last {
                    px(img, xi, yi)
                } else if zi == 0 {
                    shade(px(img, xi, yi), 0.85)
                } else {
                    side.unwrap_or_else(|| px(img, xi, yi))
                };
                g.set(x, ht - 1 - y, z, c);
            }
        }
    }
    g
}

/// Лежащая вещь: рисунок лицом вверх (верх картинки — к +Z), `thickness` вокселей.
pub fn extrude_flat(img: &RgbaImage, thickness: usize) -> Grid {
    let (wd, ht) = (img.width() as usize, img.height() as usize);
    let t = thickness.max(1);
    let mut g = Grid::new([wd, t, ht]);
    for y in 0..ht {
        for x in 0..wd {
            let (xi, yi) = (x as i64, y as i64);
            if !opaque(img, xi, yi) {
                continue;
            }
            let edge = !opaque(img, xi - 1, yi)
                || !opaque(img, xi + 1, yi)
                || !opaque(img, xi, yi - 1)
                || !opaque(img, xi, yi + 1);
            for k in 0..t {
                let c = if k + 1 == t {
                    px(img, xi, yi)
                } else if edge {
                    let d = if !opaque(img, xi - 1, yi) {
                        (1, 0)
                    } else if !opaque(img, xi + 1, yi) {
                        (-1, 0)
                    } else if !opaque(img, xi, yi - 1) {
                        (0, 1)
                    } else {
                        (0, -1)
                    };
                    shade(inner(img, xi, yi, d), 0.85)
                } else {
                    shade(px(img, xi, yi), 0.7)
                };
                g.set(x, k, y, c);
            }
        }
    }
    g
}

/// Палитра до 255 цветов: самые частые цвета, остальные — к ближайшему из них.
fn palette_of(grid: &Grid) -> (Vec<Rgba>, std::collections::HashMap<Rgba, u8>) {
    let mut counts: std::collections::HashMap<Rgba, usize> = std::collections::HashMap::new();
    for c in grid.cells.iter().flatten() {
        *counts.entry([c[0], c[1], c[2], 255]).or_default() += 1;
    }
    let mut colors: Vec<(Rgba, usize)> = counts.into_iter().collect();
    colors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let palette: Vec<Rgba> = colors.iter().take(255).map(|(c, _)| *c).collect();
    let mut index = std::collections::HashMap::new();
    for (c, _) in &colors {
        let best = palette
            .iter()
            .enumerate()
            .min_by_key(|(_, p)| {
                (0..3)
                    .map(|i| (i32::from(p[i]) - i32::from(c[i])).pow(2))
                    .sum::<i32>()
            })
            .map_or(0, |(i, _)| i);
        index.insert(*c, (best + 1) as u8);
    }
    (palette, index)
}

/// Модель из вокселей: масштаб — вокселей на метр, опора — центр основания.
pub fn to_model(grid: &Grid, voxels_per_meter: u16) -> Option<VoxelModel> {
    if grid.filled() == 0 {
        return None;
    }
    let size = grid.size.map(|s| s.min(256) as u16);
    let (palette, index) = palette_of(grid);
    let mut m = VoxelModel::new(size, palette).ok()?;
    m.set_voxels_per_meter(voxels_per_meter.clamp(1, 1024))
        .ok()?;
    for z in 0..size[2] {
        for y in 0..size[1] {
            for x in 0..size[0] {
                if let Some(c) = grid.get(i64::from(x), i64::from(y), i64::from(z)) {
                    let i = index[&[c[0], c[1], c[2], 255]];
                    m.set(x, y, z, i).ok()?;
                }
            }
        }
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32, f: impl Fn(u32, u32) -> Option<[u8; 3]>) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| match f(x, y) {
            Some([r, g, b]) => image::Rgba([r, g, b, 255]),
            None => image::Rgba([0, 0, 0, 0]),
        })
    }

    #[test]
    fn four_views_carve_a_box_with_each_side_coloured_from_its_view() {
        // Спереди 4×4 красный, сзади синий, сбоку 2 в глубину: зелёный (E) и жёлтый (W).
        let s = img(4, 4, |_, _| Some([255, 0, 0]));
        let n = img(4, 4, |_, _| Some([0, 0, 255]));
        let e = img(4, 4, |x, _| (1..3).contains(&x).then_some([0, 255, 0]));
        let w = img(4, 4, |x, _| (1..3).contains(&x).then_some([255, 255, 0]));
        let g = carve([&s, &n, &e, &w]);
        assert_eq!(g.filled(), 4 * 4 * 2);
        // Перед — в +Z: передний слой красный, задний синий.
        assert_eq!(g.get(1, 1, 2), Some([255, 0, 0, 255]));
        assert_eq!(g.get(1, 1, 1), Some([0, 0, 255, 255]));
        assert_eq!(g.get(0, 1, 0), None, "за силуэтом сбоку пусто");
    }

    #[test]
    fn a_single_view_is_extruded_and_sides_skip_the_dark_outline() {
        // Светло-серый ящик 10×10 с чёрной обводкой.
        let i = img(10, 10, |x, y| {
            if x == 0 || y == 0 || x == 9 || y == 9 {
                Some([0, 0, 0])
            } else {
                Some([200, 200, 200])
            }
        });
        let g = extrude_upright(&i);
        assert_eq!(g.size[2], 7, "глубина — 0,7 ширины силуэта");
        // Бок посередине глубины — цвет изнутри, не чёрный.
        assert_eq!(g.get(0, 5, 3), Some([200, 200, 200, 255]));
        // Лежащий: рисунок сверху, толщина — заданная.
        let f = extrude_flat(&i, 3);
        assert_eq!(f.size, [10, 3, 10]);
        assert_eq!(
            f.get(5, 2, 5),
            Some([200, 200, 200, 255]),
            "верх — сам рисунок"
        );
    }

    #[test]
    fn a_model_keeps_at_most_255_colours_and_the_scale() {
        let i = img(32, 32, |x, y| Some([(x * 8) as u8, (y * 8) as u8, 7]));
        let m = to_model(&extrude_flat(&i, 1), 64).unwrap();
        assert_eq!(m.palette().len(), 255, "1024 цвета сведены к 255");
        assert_eq!(m.voxels_per_meter(), 64);
        assert_eq!(m.filled(), 32 * 32);
        assert!(to_model(&extrude_flat(&img(2, 2, |_, _| None), 1), 32).is_none());
    }
}
