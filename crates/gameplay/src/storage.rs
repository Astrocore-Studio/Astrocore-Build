use std::collections::BTreeSet;

use astrocore_core::ObjectId;
use prototypes::Data;
use serde::{Deserialize, Serialize};

use crate::hands::HeldItem;
use crate::items::{ItemInfo, ItemSize};

/// Размер самого большого предмета, если хранилище его не задаёт (`DefaultStorageMaxItemSize`).
pub const DEFAULT_MAX_ITEM_SIZE: ItemSize = ItemSize::Normal;

/// Допуск предметов: подходит предмет с любым из тегов или компонентов.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub tags: BTreeSet<String>,
    pub components: BTreeSet<String>,
}

impl Filter {
    fn from_data(d: &Data) -> Self {
        let list = |k: &str| -> BTreeSet<String> {
            d.get(k)
                .and_then(Data::as_list)
                .map(|l| {
                    l.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            tags: list("tags"),
            components: list("components"),
        }
    }

    pub fn matches(&self, item: &ItemInfo) -> bool {
        self.tags.iter().any(|t| item.tags.contains(t))
            || self.components.iter().any(|c| item.components.contains(c))
    }
}

/// Описание хранилища из прототипа: сетка клеток, предел размера, допуски.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageSpec {
    /// Клетки сетки (прямоугольники `x1,y1,x2,y2` включительно, как `Grid` в SS14).
    pub cells: BTreeSet<(i32, i32)>,
    pub max_item_size: ItemSize,
    pub whitelist: Option<Filter>,
    pub blacklist: Option<Filter>,
}

fn parse_rect(s: &str) -> Option<[i32; 4]> {
    let v: Vec<i32> = s
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    <[i32; 4]>::try_from(v).ok()
}

/// Предел площади сетки хранилища из прототипа: данные недоверенные.
pub const MAX_GRID_CELLS: usize = 1024;

impl StorageSpec {
    /// `None`, если сетка пуста, не разбирается или слишком велика.
    pub fn from_data(d: &Data) -> Option<Self> {
        let mut cells = BTreeSet::new();
        for r in d.get("grid")?.as_list()? {
            let [x0, y0, x1, y1] = parse_rect(r.as_str()?)?;
            if x1 < x0 || y1 < y0 {
                return None;
            }
            let area = i64::from(x1 - x0 + 1) * i64::from(y1 - y0 + 1);
            if area > MAX_GRID_CELLS as i64 {
                return None;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    cells.insert((x, y));
                }
            }
            if cells.len() > MAX_GRID_CELLS {
                return None;
            }
        }
        if cells.is_empty() {
            return None;
        }
        Some(Self {
            cells,
            max_item_size: d
                .get("maxItemSize")
                .and_then(Data::as_str)
                .and_then(ItemSize::parse)
                .unwrap_or(DEFAULT_MAX_ITEM_SIZE),
            whitelist: d.get("whitelist").map(Filter::from_data),
            blacklist: d.get("blacklist").map(Filter::from_data),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StorageError {
    #[error("предмет не допускается в это хранилище")]
    NotAllowed,
    #[error("предмет слишком большой")]
    TooBig,
    #[error("вложенное хранилище должно быть меньше внешнего")]
    NestedTooBig,
    #[error("нет места")]
    NoSpace,
    #[error("предмета нет в хранилище")]
    NotFound,
    #[error("хранилище нельзя положить в само себя")]
    SelfInsert,
}

/// Место предмета в сетке: левый верхний угол и поворот на 90°.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub x: i32,
    pub y: i32,
    pub rotated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub item: HeldItem,
    pub location: Location,
    /// Занятые клетки (для проверки и показа).
    cells: Vec<(i32, i32)>,
}

/// Содержимое хранилища: предметы на клетках сетки.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Storage {
    items: Vec<Stored>,
}

impl Stored {
    /// Клетки сетки, которые занимает предмет.
    pub fn cells(&self) -> &[(i32, i32)] {
        &self.cells
    }
}

fn cells_of(size: ItemSize, loc: Location) -> Vec<(i32, i32)> {
    let (w, h) = size.shape();
    let (w, h) = if loc.rotated { (h, w) } else { (w, h) };
    (0..w)
        .flat_map(|dx| (0..h).map(move |dy| (loc.x + dx, loc.y + dy)))
        .collect()
}

impl Storage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(&self) -> &[Stored] {
        &self.items
    }

    pub fn contains(&self, id: ObjectId) -> bool {
        self.items.iter().any(|s| s.item.id == id)
    }

    /// Суммарный вес содержимого (`StorageUsed`).
    pub fn used_weight(&self, weight_of: impl Fn(&HeldItem) -> u32) -> u32 {
        self.items.iter().map(|s| weight_of(&s.item)).sum()
    }

    fn occupied(&self) -> BTreeSet<(i32, i32)> {
        self.items
            .iter()
            .flat_map(|s| s.cells.iter().copied())
            .collect()
    }

    /// Проверка в порядке `CanInsert` из SS14: допуски, размер, вложенность, место в сетке.
    pub fn can_insert(
        &self,
        spec: &StorageSpec,
        item: &ItemInfo,
    ) -> Result<Location, StorageError> {
        if spec.whitelist.as_ref().is_some_and(|f| !f.matches(item))
            || spec.blacklist.as_ref().is_some_and(|f| f.matches(item))
        {
            return Err(StorageError::NotAllowed);
        }
        if item.size > spec.max_item_size {
            return Err(StorageError::TooBig);
        }
        // Вложенное хранилище обязано быть строго меньше внешнего: так нет циклов и «сумок в сумках
        // того же размера».
        if let Some(inner) = &item.storage
            && inner.max_item_size >= spec.max_item_size
        {
            return Err(StorageError::NestedTooBig);
        }
        self.find_space(spec, item.size)
            .ok_or(StorageError::NoSpace)
    }

    /// Первое подходящее место: сначала без поворота, затем с поворотом на 90°; перебор по
    /// строкам слева направо.
    fn find_space(&self, spec: &StorageSpec, size: ItemSize) -> Option<Location> {
        let occupied = self.occupied();
        let (min_x, max_x) = (
            spec.cells.iter().map(|c| c.0).min()?,
            spec.cells.iter().map(|c| c.0).max()?,
        );
        let (min_y, max_y) = (
            spec.cells.iter().map(|c| c.1).min()?,
            spec.cells.iter().map(|c| c.1).max()?,
        );
        let (w, h) = size.shape();
        let rotations: &[bool] = if w == h { &[false] } else { &[false, true] };
        for &rotated in rotations {
            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    let loc = Location { x, y, rotated };
                    if cells_of(size, loc)
                        .iter()
                        .all(|c| spec.cells.contains(c) && !occupied.contains(c))
                    {
                        return Some(loc);
                    }
                }
            }
        }
        None
    }

    /// Положить предмет; возвращает его место.
    pub fn insert(
        &mut self,
        spec: &StorageSpec,
        item: HeldItem,
        info: &ItemInfo,
    ) -> Result<Location, StorageError> {
        let location = self.can_insert(spec, info)?;
        let cells = cells_of(info.size, location);
        self.items.push(Stored {
            item,
            location,
            cells,
        });
        Ok(location)
    }

    pub fn remove(&mut self, id: ObjectId) -> Result<HeldItem, StorageError> {
        let i = self
            .items
            .iter()
            .position(|s| s.item.id == id)
            .ok_or(StorageError::NotFound)?;
        Ok(self.items.remove(i).item)
    }

    /// Достать всё (сумка уничтожена или игрок вышел и сумка выпадает с содержимым — тогда
    /// содержимое остаётся внутри, этот метод для явного опустошения).
    pub fn take_all(&mut self) -> Vec<HeldItem> {
        self.items.drain(..).map(|s| s.item).collect()
    }
}

#[cfg(test)]
mod tests {
    use astrocore_core::PrototypeId;

    use super::*;

    fn spec(grid: &[&str], max: ItemSize) -> StorageSpec {
        let d = Data::Map(
            [
                (
                    "grid".to_string(),
                    Data::List(grid.iter().map(|g| Data::Str((*g).into())).collect()),
                ),
                ("maxItemSize".to_string(), Data::Str(format!("{max:?}"))),
            ]
            .into(),
        );
        StorageSpec::from_data(&d).unwrap()
    }

    fn info(size: ItemSize) -> ItemInfo {
        ItemInfo {
            size,
            slots: BTreeSet::new(),
            storage: None,
            qualities: BTreeSet::new(),
            tags: BTreeSet::new(),
            components: BTreeSet::from(["Item".to_string()]),
        }
    }

    fn held(n: u128) -> HeldItem {
        HeldItem {
            id: ObjectId::from_u128(n),
            prototype: PrototypeId::new("ss14:thing").unwrap(),
        }
    }

    #[test]
    fn items_fill_the_grid_first_fit_and_rotate_to_fit() {
        // Сетка 4×2: один Large (4×2) занимает её целиком.
        let s = spec(&["0,0,3,1"], ItemSize::Large);
        let mut st = Storage::new();
        assert_eq!(
            st.insert(&s, held(1), &info(ItemSize::Large)).unwrap(),
            Location {
                x: 0,
                y: 0,
                rotated: false
            }
        );
        assert_eq!(
            st.insert(&s, held(2), &info(ItemSize::Tiny)),
            Err(StorageError::NoSpace)
        );
        // Сетка 2×4: Large (4×2) влезает только повёрнутым.
        let tall = spec(&["0,0,1,3"], ItemSize::Large);
        let mut st = Storage::new();
        assert_eq!(
            st.insert(&tall, held(1), &info(ItemSize::Large)).unwrap(),
            Location {
                x: 0,
                y: 0,
                rotated: true
            }
        );
        // Малые предметы раскладываются без перекрытий.
        let s = spec(&["0,0,3,3"], ItemSize::Normal);
        let mut st = Storage::new();
        for i in 0..8 {
            st.insert(&s, held(i), &info(ItemSize::Small)).unwrap();
        }
        assert_eq!(
            st.insert(&s, held(99), &info(ItemSize::Tiny)),
            Err(StorageError::NoSpace)
        );
        let mut cells: Vec<_> = st.items().iter().flat_map(|x| x.cells.clone()).collect();
        let n = cells.len();
        cells.sort();
        cells.dedup();
        assert_eq!(cells.len(), n, "клетки не пересекаются");
    }

    #[test]
    fn size_limit_filters_and_nesting_follow_ss14() {
        let s = spec(&["0,0,5,5"], ItemSize::Normal);
        let mut st = Storage::new();
        assert_eq!(
            st.insert(&s, held(1), &info(ItemSize::Large)),
            Err(StorageError::TooBig)
        );
        // Допуски: белый список по тегу, чёрный по компоненту.
        let mut wl = s.clone();
        wl.whitelist = Some(Filter {
            tags: BTreeSet::from(["Crowbar".into()]),
            components: BTreeSet::new(),
        });
        assert_eq!(
            st.insert(&wl, held(2), &info(ItemSize::Small)),
            Err(StorageError::NotAllowed)
        );
        let mut tagged = info(ItemSize::Small);
        tagged.tags.insert("Crowbar".into());
        assert!(st.insert(&wl, held(3), &tagged).is_ok());
        let mut bl = s.clone();
        bl.blacklist = Some(Filter {
            tags: BTreeSet::new(),
            components: BTreeSet::from(["Item".into()]),
        });
        assert_eq!(
            st.insert(&bl, held(4), &info(ItemSize::Tiny)),
            Err(StorageError::NotAllowed)
        );
        // Вложенное хранилище: строго меньше внешнего.
        let mut bag_small = info(ItemSize::Small);
        bag_small.storage = Some(spec(&["0,0,1,1"], ItemSize::Small));
        assert!(st.insert(&s, held(5), &bag_small).is_ok(), "Small < Normal");
        let mut bag_same = info(ItemSize::Small);
        bag_same.storage = Some(spec(&["0,0,1,1"], ItemSize::Normal));
        assert_eq!(
            st.insert(&s, held(6), &bag_same),
            Err(StorageError::NestedTooBig)
        );
    }

    #[test]
    fn remove_frees_cells_and_unknown_items_are_errors() {
        let s = spec(&["0,0,1,1"], ItemSize::Normal);
        let mut st = Storage::new();
        st.insert(&s, held(1), &info(ItemSize::Normal)).unwrap();
        assert!(st.contains(ObjectId::from_u128(1)));
        assert_eq!(
            st.remove(ObjectId::from_u128(9)),
            Err(StorageError::NotFound)
        );
        assert_eq!(
            st.remove(ObjectId::from_u128(1)).unwrap().id,
            ObjectId::from_u128(1)
        );
        assert!(
            st.insert(&s, held(2), &info(ItemSize::Normal)).is_ok(),
            "место освободилось"
        );
        assert_eq!(st.used_weight(|_| 4), 4);
        assert_eq!(st.take_all().len(), 1);
    }

    #[test]
    fn hostile_grid_definitions_are_rejected() {
        let mk = |g: &str| {
            StorageSpec::from_data(&Data::Map(
                [("grid".to_string(), Data::List(vec![Data::Str(g.into())]))].into(),
            ))
        };
        assert!(mk("0,0,3,1").is_some());
        for bad in [
            "",
            "1,2,3",
            "3,0,0,0",
            "a,b,c,d",
            "0,0,100000,100000",
            "0,0,0,0,0",
        ] {
            assert!(mk(bad).is_none(), "{bad:?}");
        }
        assert!(StorageSpec::from_data(&Data::Map(Default::default())).is_none());
    }
}
