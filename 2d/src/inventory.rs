use astrocore_grid2d::Cell;
use astrocore_grid2d::session::PlayerId;

use crate::{Kind, MobState, Station};

impl Station {
    pub(crate) fn toggle_backpack(&mut self, player: PlayerId) {
        let crew = self
            .state
            .players
            .get_mut(&player)
            .expect("игрок существует");
        let hand = &mut crew.hands[usize::from(crew.active_hand)];
        match (*hand, crew.backpack) {
            (Some(item), None)
                if matches!(
                    self.layout.prototypes[&self.state.items[&item].prototype].kind,
                    Kind::Backpack { .. }
                ) =>
            {
                *hand = None;
                crew.backpack = Some(item);
            }
            (None, Some(bag)) => {
                crew.backpack = None;
                *hand = Some(bag);
            }
            _ => {}
        }
    }

    pub(crate) fn store_in_backpack(&mut self, player: PlayerId) {
        let crew = self
            .state
            .players
            .get_mut(&player)
            .expect("игрок существует");
        let hand = &mut crew.hands[usize::from(crew.active_hand)];
        let (Some(item), Some(bag)) = (*hand, crew.backpack) else {
            return;
        };
        let bag = self
            .state
            .items
            .get_mut(&bag)
            .expect("надетый рюкзак существует");
        let Kind::Backpack { slots } = self.layout.prototypes[&bag.prototype].kind else {
            return;
        };
        if bag.contents.len() < usize::from(slots) {
            bag.contents.push(item);
            *hand = None;
        }
    }

    pub(crate) fn take_from_backpack(&mut self, player: PlayerId, item: u16) {
        let crew = self
            .state
            .players
            .get_mut(&player)
            .expect("игрок существует");
        let hand = &mut crew.hands[usize::from(crew.active_hand)];
        let Some(bag) = crew.backpack else { return };
        if hand.is_some() {
            return;
        }
        let bag = self
            .state
            .items
            .get_mut(&bag)
            .expect("надетый рюкзак существует");
        if let Some(index) = bag.contents.iter().position(|&id| id == item) {
            *hand = Some(bag.contents.remove(index));
        }
    }

    pub(crate) fn give_item(&mut self, player: PlayerId, cell: Cell) {
        let giver = &self.state.players[&player];
        let origin = giver.position.cell();
        if origin
            .distance_on_level(cell)
            .is_none_or(|distance| distance > 1)
            || self.state.doors.get(&cell).is_some_and(|door| !door.open)
        {
            return;
        }
        let hand = usize::from(giver.active_hand);
        let Some(item) = giver.hands[hand] else {
            return;
        };
        let Some(receiver) = self.state.players.iter_mut().find_map(|(&id, crew)| {
            if id != player
                && crew.on_duty()
                && crew.damage.state() == MobState::Alive
                && crew.position.cell() == cell
                && crew.hands[usize::from(crew.active_hand)].is_none()
            {
                let slot = usize::from(crew.active_hand);
                Some(&mut crew.hands[slot])
            } else {
                None
            }
        }) else {
            return;
        };
        *receiver = Some(item);
        self.state
            .players
            .get_mut(&player)
            .expect("игрок существует")
            .hands[hand] = None;
    }
}
