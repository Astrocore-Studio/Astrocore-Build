use std::collections::BTreeSet;

use prototypes::{Data, Resolved};

use crate::storage::StorageSpec;

/// Категории размера предметов (`item_size.yml` в SS14): вес в сумке и форма в клетках.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ItemSize {
    Tiny,
    Small,
    Normal,
    Large,
    Huge,
    Ginormous,
    Colossal,
}

impl ItemSize {
    /// Размер по имени (без учёта регистра).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "tiny" => Self::Tiny,
            "small" => Self::Small,
            "normal" => Self::Normal,
            "large" => Self::Large,
            "huge" => Self::Huge,
            "ginormous" => Self::Ginormous,
            "colossal" => Self::Colossal,
            _ => return None,
        })
    }

    /// Сколько места занимает в сумке.
    pub fn weight(self) -> u32 {
        match self {
            Self::Tiny => 1,
            Self::Small => 2,
            Self::Normal => 4,
            Self::Large => 8,
            Self::Huge => 16,
            Self::Ginormous => 32,
            Self::Colossal => 64,
        }
    }

    /// Форма по умолчанию, клеток: (ширина, высота) — `defaultShape` из SS14.
    pub fn shape(self) -> (i32, i32) {
        match self {
            Self::Tiny => (1, 1),
            Self::Small => (1, 2),
            Self::Normal => (2, 2),
            Self::Large => (4, 2),
            Self::Huge => (4, 4),
            Self::Ginormous => (6, 6),
            Self::Colossal => (7, 6),
        }
    }
}

/// Размер по умолчанию, если у `Item` его нет (как `ItemComponent.Size` в SS14).
pub const DEFAULT_ITEM_SIZE: ItemSize = ItemSize::Small;

/// Что игровой код знает о предмете из его прототипа.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemInfo {
    pub size: ItemSize,
    /// Слоты одежды, куда предмет можно надеть (`Clothing.slots`), в нижнем регистре.
    pub slots: BTreeSet<String>,
    /// Если предмет — сумка или ящик, его хранилище.
    pub storage: Option<StorageSpec>,
    /// Качества инструмента (`Tool.qualities`: Prying, Screwing…).
    pub qualities: BTreeSet<String>,
    pub tags: BTreeSet<String>,
    pub components: BTreeSet<String>,
}

fn strings(d: Option<&Data>) -> Vec<String> {
    d.and_then(Data::as_list)
        .map(|l| {
            l.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Сведения о предмете; `None`, если у прототипа нет компонента `Item`.
pub fn item_info(r: &Resolved) -> Option<ItemInfo> {
    let item = r.component("Item")?;
    let size = item
        .get("size")
        .and_then(Data::as_str)
        .and_then(ItemSize::parse)
        .unwrap_or(DEFAULT_ITEM_SIZE);
    let slots = r
        .component("Clothing")
        .map(|c| {
            strings(c.get("slots"))
                .into_iter()
                .map(|s| s.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let tags = r
        .component("Tag")
        .map(|t| strings(t.get("tags")).into_iter().collect())
        .unwrap_or_default();
    Some(ItemInfo {
        size,
        slots,
        storage: r.component("Storage").and_then(StorageSpec::from_data),
        qualities: r
            .component("Tool")
            .map(|t| strings(t.get("qualities")).into_iter().collect())
            .unwrap_or_default(),
        tags,
        components: r.components.keys().cloned().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_order_by_weight_and_have_ss14_shapes() {
        assert!(ItemSize::Tiny < ItemSize::Small && ItemSize::Normal < ItemSize::Large);
        assert_eq!(ItemSize::Normal.weight(), 4);
        assert_eq!(ItemSize::Large.shape(), (4, 2));
        assert_eq!(ItemSize::parse("SMALL"), Some(ItemSize::Small));
        assert_eq!(ItemSize::parse("giant"), None);
        let area = |s: ItemSize| s.shape().0 * s.shape().1;
        for s in [
            ItemSize::Tiny,
            ItemSize::Small,
            ItemSize::Normal,
            ItemSize::Large,
            ItemSize::Huge,
        ] {
            assert!(
                area(s) as u32 <= s.weight() * 2,
                "форма {s:?} соразмерна весу"
            );
        }
    }
}
