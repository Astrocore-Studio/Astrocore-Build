//! Взрывы (`ExplosionSystem` SS14): заливка клеток интенсивностью и урон по людям.
//!
//! Алгоритм `ExplosionSystem.TileFill`: от клетки взрыва кольцо за кольцом (по четырём соседям,
//! через открытые рёбра) набирается площадь. Шаг интенсивности — половина `intensitySlope`; в каждом
//! кольце интенсивность растёт на шаг, но не выше `maxIntensity`, а весь «объём» ограничен
//! `totalIntensity`. Урон клетки — `damagePerIntensity` типа взрыва × интенсивность кольца.
//!
//! Не перенесено: разрушение стен и плитки (`Structural`, `tileBreakChance`), урон предметам,
//! отбрасывание, огонь от взрыва, космические клетки без пола, свет и звук взрыва.

use std::collections::BTreeSet;

use atmos::{Cell, Layout, edge};

/// Предел числа колец и площади (`MaxIterations` и `MaxArea` SS14 по умолчанию).
pub const MAX_ITERATIONS: usize = 500;
pub const MAX_AREA: usize = 3000;

/// Кольца взрыва: клетки каждого кольца и интенсивность кольца.
#[derive(Debug, Clone, PartialEq)]
pub struct Blast {
    pub rings: Vec<Vec<Cell>>,
    pub intensity: Vec<f32>,
}

impl Blast {
    /// Интенсивность в клетке, 0 — вне взрыва.
    pub fn at(&self, cell: Cell) -> f32 {
        self.rings
            .iter()
            .zip(&self.intensity)
            .find(|(ring, _)| ring.contains(&cell))
            .map_or(0.0, |(_, i)| *i)
    }

    pub fn tiles(&self) -> usize {
        self.rings.iter().map(Vec::len).sum()
    }
}

fn neighbours(layout: &Layout, c: Cell) -> impl Iterator<Item = Cell> + '_ {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .map(move |(dx, dz)| (c.0 + dx, c.1 + dz))
        .filter(move |n| layout.cells.contains_key(n) && !layout.blocked.contains(&edge(c, *n)))
}

/// Залить взрыв от клетки `start` (`ExplosionSystem.GetExplosionTiles`).
pub fn fill(layout: &Layout, start: Cell, total: f32, slope: f32, max_intensity: f32) -> Blast {
    let step = (slope / 2.0).max(1e-3);
    let mut rings: Vec<Vec<Cell>> = vec![vec![start]];
    // Взрыв слабее одного шага: одна клетка с остатком.
    if total < step {
        return Blast {
            rings,
            intensity: vec![total],
        };
    }
    let mut seen: BTreeSet<Cell> = BTreeSet::from([start]);
    let mut intensity = vec![step];
    let mut remaining = total - step;
    let mut total_tiles = 1usize;
    let mut iteration = 1usize;
    let mut max_index = 0usize;
    let mut unchanged_last = false;

    while remaining > 0.0 && iteration <= MAX_ITERATIONS && total_tiles < MAX_AREA {
        let previous = remaining;
        // Сначала растёт интенсивность уже открытых колец.
        let mut broke = false;
        for i in max_index..iteration {
            let inc = step.min(max_intensity - intensity[i]);
            let n = rings[i].len() as f32;
            if n * inc >= remaining {
                intensity[i] += remaining / n;
                remaining = 0.0;
                broke = true;
                break;
            }
            intensity[i] += inc;
            remaining -= n * inc;
            if inc < step {
                max_index += 1;
            }
        }
        if broke || remaining <= 0.0 {
            break;
        }
        // Потом открывается новое кольцо соседей.
        let mut fresh: Vec<Cell> = Vec::new();
        for &c in rings.last().into_iter().flatten() {
            for n in neighbours(layout, c) {
                if seen.insert(n) {
                    fresh.push(n);
                }
            }
        }
        let count = fresh.len();
        rings.push(fresh);
        if count as f32 * step >= remaining && count > 0 {
            intensity.push(remaining / count as f32);
            break;
        }
        remaining -= count as f32 * step;
        intensity.push(step);
        total_tiles += count;
        // Запертый в неразрушимой комнате взрыв: остаток не меняется два круга подряд.
        if unchanged_last && remaining == previous {
            break;
        }
        unchanged_last = remaining == previous;
        iteration += 1;
    }
    Blast { rings, intensity }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atmos::CellKind;

    fn open_room(w: i32, h: i32) -> Layout {
        let mut l = Layout::default();
        for x in 0..w {
            for z in 0..h {
                l.cells.insert((x, z), CellKind::Air);
            }
        }
        l
    }

    #[test]
    fn the_blast_spreads_in_rings_and_is_strongest_at_the_centre() {
        let l = open_room(41, 41);
        let b = fill(&l, (20, 20), 120.0, 3.0, 10.0);
        assert!(b.tiles() > 20, "{}", b.tiles());
        let centre = b.at((20, 20));
        let edge = b.at((20, 20 + b.rings.len() as i32 - 2));
        assert!(centre > edge && centre <= 10.0 + 1e-3, "{centre} {edge}");
        // Объём сохраняется: сумма интенсивности по клеткам не больше заданной.
        let sum: f32 = b
            .rings
            .iter()
            .zip(&b.intensity)
            .map(|(r, i)| r.len() as f32 * i)
            .sum();
        assert!(sum <= 120.0 + 1.0, "{sum}");
        assert_eq!(b.at((0, 0)), 0.0);
    }

    #[test]
    fn walls_stop_the_blast_and_a_door_lets_it_through() {
        let mut l = open_room(11, 3);
        // Стена между x=4 и x=5 по всем z.
        for z in 0..3 {
            l.blocked.insert(edge((4, z), (5, z)));
        }
        let b = fill(&l, (2, 1), 200.0, 4.0, 20.0);
        assert!(b.at((4, 1)) > 0.0);
        assert_eq!(b.at((5, 1)), 0.0, "за стену не прошёл");
        // Открыли проём — ушёл дальше.
        l.blocked.remove(&edge((4, 1), (5, 1)));
        let b = fill(&l, (2, 1), 200.0, 4.0, 20.0);
        assert!(b.at((5, 1)) > 0.0);
    }

    #[test]
    fn a_weak_blast_stays_in_one_tile() {
        let l = open_room(5, 5);
        let b = fill(&l, (2, 2), 1.0, 5.0, 10.0);
        assert_eq!(b.tiles(), 1);
        assert_eq!(b.intensity, vec![1.0]);
    }

    #[test]
    fn a_sealed_room_does_not_loop_forever() {
        let mut l = open_room(3, 3);
        for x in 0..3 {
            for z in 0..3 {
                for (dx, dz) in [(1, 0), (0, 1)] {
                    l.blocked.insert(edge((x, z), (x + dx, z + dz)));
                }
            }
        }
        let b = fill(&l, (1, 1), 100000.0, 1.0, 1.0);
        assert_eq!(b.tiles(), 1);
    }
}
