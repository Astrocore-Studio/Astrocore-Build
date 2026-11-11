use astrocore_core::{ObjectId, PrototypeId};
use serde::{Deserialize, Serialize};

/// Предмет в руке: идентичность сохраняется, поэтому выброшенный предмет остаётся тем же объектом.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldItem {
    pub id: ObjectId,
    pub prototype: PrototypeId,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandsError {
    #[error("все руки заняты")]
    Full,
    #[error("такой руки нет")]
    NoSuchHand,
    #[error("рука пуста")]
    Empty,
}

/// Руки сущности: именованные слоты и активная рука (как в SS14: словарь рук и `ActiveHandId`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hands {
    names: Vec<String>,
    slots: Vec<Option<HeldItem>>,
    active: usize,
}

impl Hands {
    /// Руки с заданными именами (не меньше одной).
    pub fn new(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut names: Vec<String> = names.into_iter().map(Into::into).collect();
        if names.is_empty() {
            names.push("hand".into());
        }
        let slots = vec![None; names.len()];
        Self {
            names,
            slots,
            active: 0,
        }
    }

    /// Две руки гуманоида.
    pub fn humanoid() -> Self {
        Self::new(["left", "right"])
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn name(&self, hand: usize) -> Option<&str> {
        self.names.get(hand).map(String::as_str)
    }

    pub fn get(&self, hand: usize) -> Option<&HeldItem> {
        self.slots.get(hand)?.as_ref()
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, &str, Option<&HeldItem>)> {
        self.names
            .iter()
            .zip(&self.slots)
            .enumerate()
            .map(|(i, (n, s))| (i, n.as_str(), s.as_ref()))
    }

    pub fn held(&self) -> impl Iterator<Item = &HeldItem> {
        self.slots.iter().flatten()
    }

    pub fn select(&mut self, hand: usize) -> Result<(), HandsError> {
        if hand >= self.slots.len() {
            return Err(HandsError::NoSuchHand);
        }
        self.active = hand;
        Ok(())
    }

    /// Взять предмет: сначала в активную руку, иначе в первую свободную. Возвращает руку.
    pub fn pickup(&mut self, item: HeldItem) -> Result<usize, HandsError> {
        let order =
            std::iter::once(self.active).chain((0..self.slots.len()).filter(|&i| i != self.active));
        for i in order {
            if self.slots[i].is_none() {
                self.slots[i] = Some(item);
                return Ok(i);
            }
        }
        Err(HandsError::Full)
    }

    /// Выпустить предмет из руки.
    pub fn drop_hand(&mut self, hand: usize) -> Result<HeldItem, HandsError> {
        self.slots
            .get_mut(hand)
            .ok_or(HandsError::NoSuchHand)?
            .take()
            .ok_or(HandsError::Empty)
    }

    /// Все предметы разом (отключение игрока).
    pub fn take_all(&mut self) -> Vec<HeldItem> {
        self.slots.iter_mut().filter_map(Option::take).collect()
    }

    /// Переложить предмет из одной руки в другую свободную руку.
    pub fn move_between(&mut self, from: usize, to: usize) -> Result<(), HandsError> {
        if from >= self.slots.len() || to >= self.slots.len() {
            return Err(HandsError::NoSuchHand);
        }
        if self.slots[from].is_none() {
            return Err(HandsError::Empty);
        }
        if from != to && self.slots[to].is_some() {
            return Err(HandsError::Full);
        }
        if from != to {
            self.slots[to] = self.slots[from].take();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(n: u128) -> HeldItem {
        HeldItem {
            id: ObjectId::from_u128(n),
            prototype: PrototypeId::new("ss14:crowbar").unwrap(),
        }
    }

    #[test]
    fn pickup_prefers_the_active_hand_then_any_free_one() {
        let mut h = Hands::humanoid();
        assert_eq!(h.pickup(item(1)), Ok(0));
        assert_eq!(h.pickup(item(2)), Ok(1), "активная занята — вторая рука");
        assert_eq!(h.pickup(item(3)), Err(HandsError::Full));
        h.select(1).unwrap();
        h.drop_hand(0).unwrap();
        assert_eq!(
            h.pickup(item(4)),
            Ok(0),
            "активная рука 1 занята — свободная 0"
        );
        assert_eq!(h.held().count(), 2);
    }

    #[test]
    fn drop_select_and_move_validate_their_arguments() {
        let mut h = Hands::humanoid();
        assert_eq!(h.drop_hand(0), Err(HandsError::Empty));
        assert_eq!(h.drop_hand(9), Err(HandsError::NoSuchHand));
        assert_eq!(h.select(2), Err(HandsError::NoSuchHand));
        h.pickup(item(1)).unwrap();
        assert_eq!(h.move_between(0, 1), Ok(()));
        assert!(h.get(0).is_none() && h.get(1).is_some());
        h.pickup(item(2)).unwrap();
        assert_eq!(h.move_between(1, 0), Err(HandsError::Full));
        assert_eq!(h.move_between(0, 0), Ok(()));
        assert_eq!(h.name(1), Some("right"));
        assert_eq!(h.take_all().len(), 2);
        assert_eq!(h.held().count(), 0);
    }

    #[test]
    fn custom_hand_sets_and_a_minimum_of_one_hand() {
        let mut h = Hands::new(["a", "b", "c", "d"]);
        for i in 0..4 {
            h.pickup(item(i)).unwrap();
        }
        assert_eq!(h.pickup(item(9)), Err(HandsError::Full));
        assert_eq!(Hands::new(Vec::<String>::new()).len(), 1);
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(serde_json::from_str::<Hands>(&json).unwrap(), h);
    }
}
