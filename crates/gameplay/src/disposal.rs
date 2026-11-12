//! Диспозал: трубы, холдеры и ёмкости (`DisposalTubeSystem`, `SharedDisposalHolderSystem`,
//! `SharedDisposalUnitSystem`, снимок SS14 `910154a7`).
//!
//! Трубы лежат под полом и на карте заданы зонами `disposal_tube = "SWN"` — выходы трубы по порядку
//! прототипа, уже повёрнутые (`DisposalTubeComponent.Exits`); `disposal_entry = "1"` — начало трассы
//! (`DisposalEntryComponent`, труба-ствол под ёмкостью). Север — `z − 1`.
//!
//! Холдер ведёт предметы клетка за клеткой со скоростью `traversalSpeed` (10 клеток/с у
//! `DisposalHolder`). Не перенесено: звук, урон на поворотах (в прототипе 0), оглушение, срыв плитки
//! при выходе в открытое место, газ внутри холдера, метки (`DisposalTagger`), роутеры и сигналы,
//! почтовые ёмкости, мобы внутри.

use std::collections::{BTreeMap, BTreeSet};

use atmos::Cell;

use crate::hands::HeldItem;

/// Ключи метаданных зоны-трубы.
pub const TUBE_KEY: &str = "disposal_tube";
pub const ENTRY_KEY: &str = "disposal_entry";

/// `DisposalHolder.traversalSpeed`, клеток в секунду.
pub const TRAVERSAL_SPEED: f32 = 10.0;
/// `DisposalHolder.exitDistanceMultiplier` (дальность выброса из открой трубы), клеток.
pub const EXIT_DISTANCE: f32 = 1.5;
/// `DisposalHolderComponent.DirectionBiasThreshold`.
const DIRECTION_BIAS_THRESHOLD: f32 = 8.0;
/// `DisposalHolderComponent.EscapeChance`.
const ESCAPE_CHANCE: f32 = 0.2;

/// Стороны света в порядке поворота против часовой: юг → восток → север → запад.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dir {
    South,
    East,
    North,
    West,
}

impl Dir {
    pub fn parse(c: char) -> Option<Self> {
        Some(match c.to_ascii_uppercase() {
            'S' => Self::South,
            'E' => Self::East,
            'N' => Self::North,
            'W' => Self::West,
            _ => return None,
        })
    }

    fn index(self) -> i32 {
        match self {
            Self::South => 0,
            Self::East => 1,
            Self::North => 2,
            Self::West => 3,
        }
    }

    pub fn opposite(self) -> Self {
        match self {
            Self::South => Self::North,
            Self::North => Self::South,
            Self::East => Self::West,
            Self::West => Self::East,
        }
    }

    /// Сдвиг по клеткам: север — `z − 1`.
    pub fn delta(self) -> (i32, i32) {
        match self {
            Self::South => (0, 1),
            Self::North => (0, -1),
            Self::East => (1, 0),
            Self::West => (-1, 0),
        }
    }

    /// Поворот в четвертях `−1…1` от `self` к `to` (`Angle.ShortestDistance`, масштаб `2θ/π`).
    fn turn_to(self, to: Self) -> f32 {
        let d = (to.index() - self.index() + 2).rem_euclid(4) - 2;
        d.clamp(-1, 1) as f32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tube {
    pub exits: Vec<Dir>,
    pub entry: bool,
}

/// Сеть труб диспозала.
#[derive(Debug, Clone, Default)]
pub struct Net {
    tubes: BTreeMap<Cell, Tube>,
}

/// Выходы трубы из строки метаданных.
pub fn parse_exits(s: &str) -> Vec<Dir> {
    s.chars().filter_map(Dir::parse).collect()
}

fn step(cell: Cell, dir: Dir) -> Cell {
    let (dx, dz) = dir.delta();
    (cell.0 + dx, cell.1 + dz)
}

/// Простой генератор для случайных выборов (воспроизводимый).
#[derive(Debug, Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn pick(&mut self, n: usize) -> usize {
        ((self.next_f32() * n as f32) as usize).min(n.saturating_sub(1))
    }
}

impl Net {
    pub fn new(tubes: impl IntoIterator<Item = (Cell, Tube)>) -> Self {
        Self {
            tubes: tubes.into_iter().collect(),
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.tubes.len()
    }

    pub fn tube(&self, cell: Cell) -> Option<&Tube> {
        self.tubes.get(&cell)
    }

    /// Убрать трубу (холдер сорвал её, `Unanchor`).
    pub fn remove(&mut self, cell: Cell) {
        self.tubes.remove(&cell);
    }

    /// `GetTubeInDirection`: соседняя труба, у которой есть выход навстречу.
    pub fn tube_in_direction(&self, cell: Cell, dir: Dir) -> Option<Cell> {
        let next = step(cell, dir);
        self.tubes
            .get(&next)
            .filter(|t| t.exits.contains(&dir.opposite()))
            .map(|_| next)
    }

    /// `SelectNextDirection`: первый выход по умолчанию; обратный движению пропускается.
    pub fn next_direction(&self, cell: Cell, current: Option<Dir>, rng: &mut Rng) -> Option<Dir> {
        let exits = &self.tubes.get(&cell)?.exits;
        let first = *exits.first()?;
        let back = current.map(Dir::opposite);
        match exits.len() {
            1 => (Some(first) != back).then_some(first),
            2 => Some(if Some(first) == back { exits[1] } else { first }),
            _ => {
                if Some(first) == back {
                    Some(exits[1 + rng.pick(exits.len() - 1)])
                } else {
                    Some(first)
                }
            }
        }
    }
}

/// Холдер: предметы в пути.
#[derive(Debug, Clone)]
pub struct Holder {
    pub items: Vec<HeldItem>,
    pub current: Cell,
    pub next: Option<Cell>,
    pub dir: Option<Dir>,
    /// Пройденная доля пути от `current` к `next`, клетки.
    pub progress: f32,
    direction_bias: f32,
    suspect: BTreeSet<Cell>,
    can_escape: bool,
}

/// Чем кончился ход холдера.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Идёт дальше.
    Moving,
    /// Вышел из трубы в клетке, направление выброса (`None` — на месте).
    Exited { at: Cell, dir: Option<Dir> },
}

impl Holder {
    pub fn new(items: Vec<HeldItem>, at: Cell) -> Self {
        Self {
            items,
            current: at,
            next: None,
            dir: None,
            progress: 0.0,
            direction_bias: 0.0,
            suspect: BTreeSet::new(),
            can_escape: false,
        }
    }

    /// `TryEnterTube`: `Ok(true)` — вошёл и поехал дальше, `Ok(false)` — выход или побег.
    pub fn enter(&mut self, net: &mut Net, tube: Cell, rng: &mut Rng) -> bool {
        let Some(next) = net.next_direction(tube, self.dir, rng) else {
            return false;
        };
        if let Some(cur) = self.dir
            && cur != next
        {
            if self.suspect.contains(&tube) {
                self.can_escape = true;
            }
            self.direction_bias += cur.turn_to(next);
            if self.direction_bias.abs() >= DIRECTION_BIAS_THRESHOLD {
                self.suspect.insert(tube);
                self.direction_bias = 0.0;
            }
            // Побег: холдер срывает трубу и вылетает (`TryEscaping`).
            if self.can_escape && rng.next_f32() <= ESCAPE_CHANCE {
                net.remove(tube);
                self.current = tube;
                self.next = None;
                return false;
            }
        }
        self.dir = Some(next);
        self.current = tube;
        self.next = net.tube_in_direction(tube, next);
        true
    }

    /// `UpdateDisposalHolder`: двигаться `dt` секунд.
    pub fn advance(&mut self, net: &mut Net, dt: f32, rng: &mut Rng) -> Outcome {
        self.progress += TRAVERSAL_SPEED * dt;
        loop {
            let Some(next) = self.next else {
                return Outcome::Exited {
                    at: self.current,
                    dir: self.dir,
                };
            };
            if self.progress < 1.0 {
                return Outcome::Moving;
            }
            self.progress -= 1.0;
            if !self.enter(net, next, rng) {
                return Outcome::Exited {
                    at: self.current,
                    dir: self.dir,
                };
            }
        }
    }
}

/// `DisposalsPressureState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pressure {
    #[default]
    Ready,
    Flushed,
    Pressurizing,
}

/// Параметры `DisposalUnitComponent` по умолчанию.
pub const PRESSURE_PER_SECOND: f32 = 0.05;
pub const FLUSH_DELAY_S: f32 = 3.0;
pub const MANUAL_FLUSH_S: f32 = 2.0;
pub const AUTOMATIC_ENGAGE_S: f32 = 30.0;
pub const MAX_CAPACITY: usize = 30;

/// Ёмкость диспозала.
#[derive(Debug, Clone, Default)]
pub struct Unit {
    pub items: Vec<HeldItem>,
    pub engaged: bool,
    pub next_pressurized: f32,
    pub next_flush: Option<f32>,
    pub state: Pressure,
    pub powered: bool,
}

impl Unit {
    /// `GetState`.
    pub fn pressure_at(&self, now: f32) -> Pressure {
        let next = self.next_pressurized - now;
        let duration = 1.0 / PRESSURE_PER_SECOND - FLUSH_DELAY_S;
        if next > duration {
            Pressure::Flushed
        } else if next > 0.0 {
            Pressure::Pressurizing
        } else {
            Pressure::Ready
        }
    }

    /// `RecalculateFlushTime`.
    pub fn recalculate(&mut self, now: f32, reset: bool) {
        if reset {
            self.next_flush = None;
        }
        if self.engaged || !self.items.is_empty() {
            let mut new = now
                + if self.engaged {
                    MANUAL_FLUSH_S
                } else {
                    AUTOMATIC_ENGAGE_S
                };
            if self.pressure_at(now) != Pressure::Ready {
                new += self.next_pressurized;
            }
            self.next_flush = Some(self.next_flush.map_or(new, |f| f.min(new)));
        } else {
            self.next_flush = None;
        }
    }

    pub fn set_engage(&mut self, engaged: bool, now: f32) {
        self.engaged = engaged;
        self.recalculate(now, false);
    }

    /// `Insert` предмета (место есть).
    pub fn insert(&mut self, item: HeldItem, now: f32) -> Result<(), HeldItem> {
        if self.items.len() >= MAX_CAPACITY {
            return Err(item);
        }
        self.items.push(item);
        self.recalculate(now, false);
        Ok(())
    }

    /// `EjectContents`: вынуть всё.
    pub fn eject(&mut self, now: f32) -> Vec<HeldItem> {
        let out = std::mem::take(&mut self.items);
        self.recalculate(now, false);
        out
    }

    /// Обновление состояния давления (`UpdateState`); `true` — пора сбрасывать.
    pub fn update(&mut self, now: f32) -> bool {
        let state = self.pressure_at(now);
        let due = self.next_pressurized <= now && self.next_flush.is_some_and(|f| f < now);
        if self.state != state {
            self.state = state;
            if state == Pressure::Ready {
                self.next_pressurized = 0.0;
            }
            self.recalculate(now, true);
        }
        due
    }

    /// `CanFlush`.
    pub fn can_flush(&self, now: f32) -> bool {
        self.pressure_at(now) == Pressure::Ready && self.powered
    }

    /// После сброса: давление набирается заново, ёмкость отпущена.
    pub fn flushed(&mut self, now: f32) {
        self.next_pressurized = now + 1.0 / PRESSURE_PER_SECOND;
        self.set_engage(false, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrocore_core::PrototypeId;

    fn item(n: u128) -> HeldItem {
        HeldItem {
            id: astrocore_core::ObjectId::from_u128(n),
            prototype: PrototypeId::new("ss14:pen").unwrap(),
        }
    }

    fn tube(exits: &str) -> Tube {
        Tube {
            exits: parse_exits(exits),
            entry: false,
        }
    }

    /// Ствол (0,0) выходом на юг, труба по оси z до (0,3).
    fn straight() -> Net {
        Net::new([
            ((0, 0), tube("S")),
            ((0, 1), tube("SN")),
            ((0, 2), tube("SN")),
            ((0, 3), tube("SN")),
        ])
    }

    #[test]
    fn a_holder_follows_the_pipe_at_ten_cells_per_second_and_leaves_at_the_open_end() {
        let mut net = straight();
        let mut rng = Rng(7);
        let mut h = Holder::new(vec![item(1)], (0, 0));
        assert!(h.enter(&mut net, (0, 0), &mut rng));
        assert_eq!(h.dir, Some(Dir::South));
        // 0,25 с — прошло 2,5 клетки.
        assert_eq!(h.advance(&mut net, 0.25, &mut rng), Outcome::Moving);
        assert_eq!(h.current, (0, 2));
        // Ещё 0,1 с — доехал до (0,3), дальше трубы нет: выход на юг.
        assert_eq!(
            h.advance(&mut net, 0.1, &mut rng),
            Outcome::Exited {
                at: (0, 3),
                dir: Some(Dir::South)
            }
        );
    }

    #[test]
    fn a_bend_turns_the_holder_and_a_junction_keeps_the_default_exit() {
        // Поворот (0,1) «SW»: вошли с севера (идём на юг) — выходим первым выходом S? Нет: S — вперёд.
        let net = Net::new([
            ((0, 1), tube("SW")),
            ((0, 0), tube("S")),
            ((0, 2), tube("N")),
        ]);
        let mut rng = Rng(1);
        // Идём на восток: обратное — запад. Первый выход S, не обратный: на юг.
        assert_eq!(
            net.next_direction((0, 1), Some(Dir::East), &mut rng),
            Some(Dir::South)
        );
        // Идём на север: обратное — юг, у поворота два выхода → второй, запад.
        assert_eq!(
            net.next_direction((0, 1), Some(Dir::North), &mut rng),
            Some(Dir::West)
        );
        // Одиночный выход, идём ему навстречу — выход из сети.
        assert_eq!(net.next_direction((0, 2), Some(Dir::South), &mut rng), None);
    }

    #[test]
    fn a_junction_with_three_exits_picks_among_the_rest_when_the_default_is_blocked() {
        let net = Net::new([((0, 0), tube("SWN"))]);
        let mut rng = Rng(3);
        let mut seen = BTreeSet::new();
        for _ in 0..50 {
            // Идём на север: первый выход S — обратный, выбирается из W/N.
            seen.insert(net.next_direction((0, 0), Some(Dir::North), &mut rng));
        }
        assert_eq!(seen, BTreeSet::from([Some(Dir::West), Some(Dir::North)]));
        // Идём на восток: S не обратный — всегда по умолчанию.
        assert_eq!(
            net.next_direction((0, 0), Some(Dir::East), &mut rng),
            Some(Dir::South)
        );
    }

    #[test]
    fn a_tube_only_connects_to_a_neighbour_with_an_exit_back() {
        let net = Net::new([((0, 0), tube("S")), ((0, 1), tube("EW"))]);
        assert_eq!(net.tube_in_direction((0, 0), Dir::South), None);
        let net = Net::new([((0, 0), tube("S")), ((0, 1), tube("NS"))]);
        assert_eq!(net.tube_in_direction((0, 0), Dir::South), Some((0, 1)));
    }

    #[test]
    fn a_unit_pressurises_for_twenty_seconds_and_flushes_after_the_manual_delay() {
        let mut u = Unit {
            powered: true,
            ..Unit::default()
        };
        u.insert(item(1), 100.0).unwrap();
        // Автосброс через 30 с.
        assert_eq!(u.next_flush, Some(130.0));
        assert!(!u.update(120.0));
        assert!(u.update(131.0));
        u.set_engage(true, 100.0);
        assert_eq!(u.next_flush, Some(102.0));
        u.flushed(102.0);
        assert!(!u.engaged);
        assert_eq!(u.pressure_at(102.0), Pressure::Flushed);
        assert_eq!(u.pressure_at(110.0), Pressure::Pressurizing);
        assert_eq!(u.pressure_at(122.5), Pressure::Ready);
        assert!(!u.can_flush(110.0));
        assert!(u.can_flush(123.0));
    }

    #[test]
    fn a_unit_holds_thirty_items() {
        let mut u = Unit::default();
        for i in 0..30 {
            assert!(u.insert(item(i), 0.0).is_ok());
        }
        assert!(u.insert(item(99), 0.0).is_err());
        assert_eq!(u.eject(0.0).len(), 30);
        assert!(u.items.is_empty() && u.next_flush.is_none());
    }

    #[test]
    fn a_holder_stuck_in_a_loop_eventually_tears_a_tube_out_and_escapes() {
        // Кольцо 2×2 из поворотов: идти по нему можно бесконечно.
        let mut net = Net::new([
            ((0, 0), tube("SE")),
            ((1, 0), tube("SW")),
            ((1, 1), tube("NW")),
            ((0, 1), tube("NE")),
        ]);
        let mut rng = Rng(11);
        let mut h = Holder::new(vec![item(1)], (0, 0));
        assert!(h.enter(&mut net, (0, 0), &mut rng));
        let mut out = Outcome::Moving;
        for _ in 0..5000 {
            out = h.advance(&mut net, 0.1, &mut rng);
            if out != Outcome::Moving {
                break;
            }
        }
        assert!(matches!(out, Outcome::Exited { .. }));
        assert!(net.len() < 4, "одна труба сорвана");
    }
}
