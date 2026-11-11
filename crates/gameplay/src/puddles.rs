//! Лужи и разлив (`PuddleSystem`, `SpreaderSystem`, `Evaporation`, `SlipperySystem`).
//!
//! * Разлитое остаётся лужей на клетке; в клетке умещается 50 единиц (`OverflowVolume`), лишнее
//!   раз в секунду растекается по соседним клеткам (до 16 растеканий в секунду на мир, как у
//!   `EdgeSpreader` «Puddle»): сначала в свободные клетки поровну, потом выравнивается с лужами
//!   по соседству.
//! * Испарение раз в секунду: для каждого испаряющегося реагента `EvaporationAmount (1) ×
//!   скорость × среднее по скоростям × доля реагента`; пустая лужа исчезает.
//! * Скользкость: лужа меньше 30% объёма (15 ед.) не скользкая; иначе в ней считаются реагенты со
//!   `SlipData`, скользкой она становится, если их больше 15 ед.; порог скорости — среднее по
//!   реагентам (`RequiredSlipSpeed`, у остальных 5,5 — `DefaultSlippery`). Поскользнувшийся
//!   роняет всё из рук и падает: оглушение 0,5 с и сбивание 1,5 с (`SlipData` по умолчанию).

use std::collections::{BTreeMap, BTreeSet};

use atmos::{Cell, CellKind, Layout, edge, pipes};

use crate::reagents::Solution;

pub const OVERFLOW_VOLUME: f32 = 50.0;
pub const LOW_THRESHOLD: f32 = 0.3;
pub const DEFAULT_SLIPPERY: f32 = 5.5;
pub const SPREAD_COOLDOWN_S: f32 = 1.0;
pub const SPREAD_UPDATES_PER_SECOND: i32 = 16;
pub const EVAPORATION_AMOUNT: f32 = 1.0;
/// Время оглушения и сбивания с ног при падении на луже, с (`SlipperyEffectEntry`).
pub const SLIP_STUN_S: f32 = 0.5;
pub const SLIP_KNOCKDOWN_S: f32 = 1.5;

/// Физические свойства реагента в луже (`ReagentPrototype`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Physical {
    pub evaporation: f32,
    pub friction: f32,
    /// `SlipData.RequiredSlipSpeed`; `None` — не скользкий.
    pub slip_speed: Option<f32>,
    pub super_slippery: bool,
    pub color: [u8; 3],
    pub standsout: bool,
}

const DEFAULT: Physical = Physical {
    evaporation: 0.0,
    friction: 1.0,
    slip_speed: None,
    super_slippery: false,
    color: [128, 128, 128],
    standsout: false,
};

/// Свойства реагентов из `Resources/Prototypes/Reagents`.
pub fn physical(reagent: &str) -> Physical {
    match reagent {
        "Water" => Physical {
            evaporation: 0.3,
            friction: 0.4,
            color: [0x75, 0xb1, 0xf0],
            ..DEFAULT
        },
        "Blood" => Physical {
            color: [0x80, 0x00, 0x00],
            standsout: true,
            ..DEFAULT
        },
        "SpaceLube" => Physical {
            evaporation: 0.1,
            friction: 0.05,
            slip_speed: Some(1.0),
            super_slippery: true,
            color: [0x77, 0xb5, 0x8e],
            ..DEFAULT
        },
        "WeldingFuel" => Physical {
            friction: 0.4,
            slip_speed: Some(3.5),
            color: [0xa7, 0x6b, 0x1c],
            ..DEFAULT
        },
        "Nutriment" => Physical {
            color: [0x66, 0x4b, 0x63],
            ..DEFAULT
        },
        "Protein" => Physical {
            color: [0xff, 0xe5, 0xb0],
            ..DEFAULT
        },
        _ => DEFAULT,
    }
}

/// Что делает лужа с тем, кто на неё наступил.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slip {
    pub required_speed: f32,
    pub super_slippery: bool,
}

/// Лужа для клиента.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PuddleView {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Объём к `OverflowVolume` (0…1 и больше при переполнении).
    pub fill: f32,
    pub color: [u8; 3],
    pub slippery: bool,
}

pub const PUDDLES_COMPONENT: &str = "build.puddles";
pub const PUDDLES_MAX_BYTES: usize = 15 * 1024;
pub const MAX_PUDDLES: usize = 150;

pub fn encode_puddles(p: &[PuddleView]) -> Vec<u8> {
    serde_json::to_vec(p).unwrap_or_default()
}

pub fn decode_puddles(b: &[u8]) -> Option<Vec<PuddleView>> {
    if b.len() > PUDDLES_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(b).ok()
}

#[derive(Debug, Default)]
pub struct Puddles {
    cells: BTreeMap<Cell, Solution>,
    active: BTreeSet<Cell>,
    timer: f32,
    rng: u32,
}

fn unbounded() -> Solution {
    Solution::new(1.0e9)
}

impl Puddles {
    pub fn new() -> Self {
        Self {
            rng: 0x9e37_79b9,
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn volume(&self, cell: Cell) -> f32 {
        self.cells.get(&cell).map_or(0.0, Solution::volume)
    }

    pub fn solution(&self, cell: Cell) -> Option<&Solution> {
        self.cells.get(&cell)
    }

    pub fn cells(&self) -> impl Iterator<Item = (&Cell, &Solution)> {
        self.cells.iter()
    }

    fn random(&mut self) -> u32 {
        let mut x = self.rng | 1;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    /// `TrySpillAt`: разлить раствор на клетку. Клетка должна быть полом с воздухом.
    pub fn spill(&mut self, layout: &Layout, cell: Cell, solution: &Solution) -> bool {
        if solution.volume() <= 0.0 || layout.cells.get(&cell) != Some(&CellKind::Air) {
            return false;
        }
        let puddle = self.cells.entry(cell).or_insert_with(unbounded);
        puddle.merge(solution);
        if puddle.volume() > OVERFLOW_VOLUME {
            self.active.insert(cell);
        }
        true
    }

    /// Скользкость лужи (`UpdateSlip`).
    pub fn slip(&self, cell: Cell) -> Option<Slip> {
        let s = self.cells.get(&cell)?;
        let volume = s.volume();
        let small = OVERFLOW_VOLUME * LOW_THRESHOLD;
        if volume <= small {
            return None;
        }
        let (mut trigger, mut slippery, mut super_units) = (0.0, 0.0, 0.0);
        for (r, q) in &s.contents {
            let p = physical(r);
            trigger += q * p.slip_speed.unwrap_or(DEFAULT_SLIPPERY);
            if p.slip_speed.is_some() {
                slippery += q;
                if p.super_slippery {
                    super_units += q;
                }
            }
        }
        (slippery > small).then_some(Slip {
            required_speed: trigger / volume,
            super_slippery: super_units >= small,
        })
    }

    /// Цвет лужи: смесь цветов реагентов по долям, «заметные» (кровь) перебивают остальные.
    pub fn color(&self, cell: Cell) -> [u8; 3] {
        let Some(s) = self.cells.get(&cell) else {
            return DEFAULT.color;
        };
        let volume = s.volume().max(1e-6);
        let mut base = [0.0f32; 3];
        let mut base_volume = 0.0;
        for (r, q) in &s.contents {
            let p = physical(r);
            if !p.standsout {
                for (c, v) in base.iter_mut().zip(p.color) {
                    *c += f32::from(v) * q;
                }
                base_volume += q;
            }
        }
        let mut color = if base_volume > 0.0 {
            base.map(|c| c / base_volume)
        } else {
            [128.0; 3]
        };
        for (r, q) in &s.contents {
            let p = physical(r);
            if p.standsout {
                let t = q / volume;
                for (c, v) in color.iter_mut().zip(p.color) {
                    *c += (f32::from(v) - *c) * t;
                }
            }
        }
        color.map(|c| c.round().clamp(0.0, 255.0) as u8)
    }

    /// Шаг времени: испарение и растекание раз в секунду.
    pub fn tick(&mut self, dt: f32, layout: &Layout) {
        self.cells
            .retain(|c, _| layout.cells.get(c) == Some(&CellKind::Air));
        self.active.retain(|c| self.cells.contains_key(c));
        self.timer += dt;
        while self.timer >= SPREAD_COOLDOWN_S {
            self.timer -= SPREAD_COOLDOWN_S;
            self.evaporate();
            let mut updates = SPREAD_UPDATES_PER_SECOND;
            let run: Vec<Cell> = self.active.iter().copied().collect();
            for c in run {
                if updates <= 0 {
                    break;
                }
                self.spread(c, &mut updates, layout);
            }
        }
    }

    /// `TickEvaporation`.
    fn evaporate(&mut self) {
        let mut empty = Vec::new();
        for (cell, s) in &mut self.cells {
            let speeds: Vec<(String, f32)> = s
                .contents
                .keys()
                .filter_map(|r| {
                    let e = physical(r).evaporation;
                    (e > 0.0).then(|| (r.clone(), e))
                })
                .collect();
            if speeds.is_empty() {
                continue;
            }
            let average = speeds.iter().map(|(_, e)| e).sum::<f32>() / speeds.len() as f32;
            let volume = s.volume();
            for (r, _) in &speeds {
                let proportion = s.amount(r) / volume;
                let tick = EVAPORATION_AMOUNT * average * proportion;
                let amount = s.amount(r).min(tick);
                s.remove(r, amount);
            }
            if s.volume() <= 1e-4 {
                empty.push(*cell);
            }
        }
        for c in empty {
            self.cells.remove(&c);
            self.active.remove(&c);
        }
    }

    fn can_fully_evaporate(s: &Solution) -> bool {
        let evap: f32 = s
            .contents
            .iter()
            .filter(|(r, _)| physical(r).evaporation > 0.0)
            .map(|(_, q)| q)
            .sum();
        (evap - s.volume()).abs() < 1e-4
    }

    /// `OnPuddleSpread`.
    fn spread(&mut self, cell: Cell, updates: &mut i32, layout: &Layout) {
        let Some(vol) = self.cells.get(&cell).map(Solution::volume) else {
            self.active.remove(&cell);
            return;
        };
        if vol <= OVERFLOW_VOLUME {
            self.active.remove(&cell);
            return;
        }
        let mut overflow = self
            .cells
            .get_mut(&cell)
            .unwrap()
            .split(vol - OVERFLOW_VOLUME);
        let neighbours: Vec<Cell> = [pipes::NORTH, pipes::SOUTH, pipes::EAST, pipes::WEST]
            .into_iter()
            .map(|d| pipes::step(cell, d))
            .filter(|n| {
                layout.cells.get(n) == Some(&CellKind::Air)
                    && !layout.blocked.contains(&edge(cell, *n))
            })
            .collect();
        let mut free: Vec<Cell> = neighbours
            .iter()
            .copied()
            .filter(|n| !self.cells.contains_key(n))
            .collect();
        if !free.is_empty() && *updates > 0 {
            for i in (1..free.len()).rev() {
                let j = self.random() as usize % (i + 1);
                free.swap(i, j);
            }
            let each = overflow.volume() / free.len() as f32;
            for n in free {
                let part = overflow.split(each);
                self.spill(layout, n, &part);
                *updates -= 1;
                if *updates <= 0 {
                    break;
                }
            }
            self.active.remove(&cell);
            return;
        }
        let mut with_puddles: Vec<(Cell, f32)> = neighbours
            .iter()
            .filter_map(|n| {
                let s = self.cells.get(n)?;
                (!Self::can_fully_evaporate(s)).then_some((*n, s.volume()))
            })
            .collect();
        with_puddles.sort_by(|a, b| a.1.total_cmp(&b.1));
        for &(n, v) in &with_puddles {
            if v >= overflow.volume() + OVERFLOW_VOLUME {
                continue;
            }
            let remaining = OVERFLOW_VOLUME - v;
            if remaining <= 0.0 || v + remaining >= overflow.volume() + OVERFLOW_VOLUME {
                continue;
            }
            let part = overflow.split(remaining);
            self.cells.get_mut(&n).unwrap().merge(&part);
            *updates -= 1;
            self.active.insert(n);
            if *updates <= 0 {
                break;
            }
        }
        if overflow.volume() <= 0.0 {
            self.active.remove(&cell);
            return;
        }
        if overflow.volume() > 0.0 && !with_puddles.is_empty() && *updates > 0 {
            let total: f32 = with_puddles
                .iter()
                .map(|(n, _)| self.cells[n].volume())
                .sum();
            let count = with_puddles.len() as f32;
            for &(n, _) in &with_puddles {
                let current = overflow.volume() + OVERFLOW_VOLUME;
                let v = self.cells[&n].volume();
                if v >= current {
                    continue;
                }
                let ideal = (total + overflow.volume() + OVERFLOW_VOLUME) / (count + 1.0);
                if ideal > current {
                    continue;
                }
                let part = overflow.split(ideal - v);
                self.cells.get_mut(&n).unwrap().merge(&part);
                self.active.insert(n);
                *updates -= 1;
                if *updates <= 0 {
                    break;
                }
            }
        }
        if let Some(p) = self.cells.get_mut(&cell) {
            p.merge(&overflow);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(w: i32, h: i32) -> Layout {
        let mut l = Layout::default();
        for x in 0..w {
            for z in 0..h {
                l.cells.insert((x, z), CellKind::Air);
            }
        }
        l
    }

    fn sol(reagent: &str, amount: f32) -> Solution {
        let mut s = Solution::new(1.0e9);
        s.add(reagent, amount);
        s
    }

    #[test]
    fn water_evaporates_at_a_third_of_a_unit_per_second_and_blood_stays() {
        let l = room(3, 3);
        let mut p = Puddles::new();
        p.spill(&l, (1, 1), &sol("Water", 10.0));
        p.spill(&l, (0, 0), &sol("Blood", 10.0));
        p.tick(10.0, &l);
        assert!(
            (p.volume((1, 1)) - 7.0).abs() < 0.05,
            "{}",
            p.volume((1, 1))
        );
        assert_eq!(p.volume((0, 0)), 10.0);
        p.tick(40.0, &l);
        assert_eq!(p.volume((1, 1)), 0.0, "вода высохла целиком");
        assert!(p.volume((0, 0)) > 9.9);
    }

    #[test]
    fn an_overflowing_puddle_spreads_into_free_neighbours_and_conserves_volume() {
        let l = room(5, 5);
        let mut p = Puddles::new();
        p.spill(&l, (2, 2), &sol("Blood", 400.0));
        for _ in 0..40 {
            p.tick(1.0, &l);
        }
        assert!(p.len() > 9, "лужа растеклась: {}", p.len());
        let total: f32 = p.cells().map(|(_, s)| s.volume()).sum();
        assert!(total > 380.0, "жидкость не пропала: {total}");
        assert!(
            p.cells().all(|(_, s)| s.volume() < 60.0),
            "в клетке не больше ~50"
        );
    }

    #[test]
    fn walls_stop_the_spreading() {
        let mut l = room(3, 1);
        l.blocked.insert(edge((0, 0), (1, 0)));
        let mut p = Puddles::new();
        p.spill(&l, (0, 0), &sol("Blood", 200.0));
        for _ in 0..30 {
            p.tick(1.0, &l);
        }
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn only_big_enough_puddles_of_slippery_reagents_are_slippery() {
        let l = room(2, 1);
        let mut p = Puddles::new();
        p.spill(&l, (0, 0), &sol("Water", 40.0));
        assert!(p.slip((0, 0)).is_none(), "вода в SS14 не скользкая");
        p.spill(&l, (1, 0), &sol("SpaceLube", 10.0));
        assert!(p.slip((1, 0)).is_none(), "маленькая лужа не скользкая");
        p.spill(&l, (1, 0), &sol("SpaceLube", 20.0));
        let s = p.slip((1, 0)).expect("скользкая");
        assert!(s.super_slippery);
        assert!((s.required_speed - 1.0).abs() < 1e-3);
    }

    #[test]
    fn blood_colours_the_puddle_and_space_is_not_a_floor() {
        let mut l = room(2, 1);
        l.cells.insert((1, 0), CellKind::Space);
        let mut p = Puddles::new();
        assert!(
            !p.spill(&l, (1, 0), &sol("Water", 5.0)),
            "в космосе луж нет"
        );
        p.spill(&l, (0, 0), &sol("Blood", 5.0));
        p.spill(&l, (0, 0), &sol("Water", 5.0));
        let c = p.color((0, 0));
        assert!(c[0] > c[2], "красноватая: {c:?}");
    }
}
