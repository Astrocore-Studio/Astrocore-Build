//! Сети кабелей: кабели одной группы, лежащие в одной клетке или в соседних по сторонам, образуют
//! одну сеть (`CableNode.GetReachableNodes`).

use std::collections::BTreeMap;

/// Клетка карты: x и z (этажи разнесены по z, как в атмосфере).
pub type Cell = (i32, i32);

/// Группа узлов (`nodeGroupID`): высокое, среднее напряжение и сеть АПЦ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CableGroup {
    High,
    Medium,
    Apc,
}

impl CableGroup {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "hv" | "high" | "hvpower" => Self::High,
            "mv" | "medium" | "mvpower" => Self::Medium,
            "apc" | "lv" | "low" => Self::Apc,
            _ => return None,
        })
    }
}

/// Кабели мира: какие клетки заняты и к какой сети каждая принадлежит.
#[derive(Debug, Default, Clone)]
pub struct Cables {
    nets: BTreeMap<(CableGroup, Cell), usize>,
    count: usize,
}

impl Cables {
    /// Построить сети по клеткам с кабелями каждой группы.
    pub fn build(cells: impl IntoIterator<Item = (CableGroup, Cell)>) -> Self {
        let mut list: Vec<(CableGroup, Cell)> = cells.into_iter().collect();
        list.sort();
        list.dedup();
        let index: BTreeMap<(CableGroup, Cell), usize> =
            list.iter().enumerate().map(|(i, k)| (*k, i)).collect();
        let mut parent: Vec<usize> = (0..list.len()).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        for (i, (g, c)) in list.iter().enumerate() {
            for d in [(1, 0), (0, 1)] {
                if let Some(&j) = index.get(&(*g, (c.0 + d.0, c.1 + d.1))) {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        parent[a] = b;
                    }
                }
            }
        }
        let mut ids: BTreeMap<usize, usize> = BTreeMap::new();
        let mut nets = BTreeMap::new();
        for (i, key) in list.iter().enumerate() {
            let root = find(&mut parent, i);
            let next = ids.len();
            let id = *ids.entry(root).or_insert(next);
            nets.insert(*key, id);
        }
        Self {
            count: ids.len(),
            nets,
        }
    }

    /// Номер сети кабеля группы в клетке.
    pub fn net_at(&self, group: CableGroup, cell: Cell) -> Option<usize> {
        self.nets.get(&(group, cell)).copied()
    }

    pub fn net_count(&self) -> usize {
        self.count
    }

    /// Ближайшая клетка кабеля группы не дальше `range` клеток от `cell` (по прямой).
    pub fn nearest(&self, group: CableGroup, cell: Cell, range: f32) -> Option<(Cell, usize)> {
        let r = range.ceil() as i32;
        let mut best: Option<(f32, Cell, usize)> = None;
        for dx in -r..=r {
            for dz in -r..=r {
                let c = (cell.0 + dx, cell.1 + dz);
                let d = ((dx * dx + dz * dz) as f32).sqrt();
                if d > range {
                    continue;
                }
                if let Some(&n) = self.nets.get(&(group, c))
                    && best.is_none_or(|(bd, ..)| d < bd)
                {
                    best = Some((d, c, n));
                }
            }
        }
        best.map(|(_, c, n)| (c, n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touching_cables_of_one_group_form_a_network() {
        let g = CableGroup::High;
        let c = Cables::build([
            (g, (0, 0)),
            (g, (1, 0)),
            (g, (2, 0)),
            (g, (5, 5)),
            (CableGroup::Medium, (1, 0)),
        ]);
        assert_eq!(c.net_at(g, (0, 0)), c.net_at(g, (2, 0)));
        assert_ne!(c.net_at(g, (0, 0)), c.net_at(g, (5, 5)));
        assert_ne!(
            c.net_at(g, (1, 0)),
            c.net_at(CableGroup::Medium, (1, 0)),
            "группы не смешиваются"
        );
        assert_eq!(c.net_count(), 3);
        let (cell, _) = c.nearest(g, (4, 4), 2.0).unwrap();
        assert_eq!(cell, (5, 5));
        assert!(c.nearest(g, (9, 9), 2.0).is_none());
    }
}
