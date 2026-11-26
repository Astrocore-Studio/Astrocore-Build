use std::fmt::Write;

use astrocore_grid2d::Cell;
use bevy::prelude::*;

use crate::inventory_view::item_label;
use crate::{Action, CardSlot, Crew, IdConsole, IdConsoleControl, Layout, State};

const VISIBLE_LEVELS: usize = 5;

#[derive(Resource, Default)]
pub struct IdConsolePanel {
    pub cell: Option<Cell>,
    selected: usize,
}

impl IdConsolePanel {
    pub fn open(&mut self, cell: Cell) {
        self.cell = Some(cell);
        self.selected = 0;
    }

    pub fn action(
        &mut self,
        keys: &ButtonInput<KeyCode>,
        state: &State,
        levels: &[String],
        console: &IdConsole,
        crew: &Crew,
    ) -> Option<Action> {
        let cell = self.cell?;
        if keys.just_pressed(KeyCode::KeyX) {
            return Some(Action::SwapHand);
        }
        let slot = [
            (KeyCode::Digit1, CardSlot::Authority),
            (KeyCode::Digit2, CardSlot::Target),
        ]
        .into_iter()
        .find_map(|(key, slot)| keys.just_pressed(key).then_some(slot));
        let control = if let Some(slot) = slot {
            IdConsoleControl::Transfer {
                slot,
                expected_card: console.cards[slot.index()],
                expected_hand: crew.hands[usize::from(crew.active_hand)],
            }
        } else if keys.just_pressed(KeyCode::Space) {
            let [Some(authority), Some(target)] = console.cards else {
                return None;
            };
            let tag = levels.get(self.selected)?;
            let granted = !has_tag(state, target, tag);
            IdConsoleControl::SetAccess {
                authority,
                target,
                level: self.selected as u8,
                granted,
            }
        } else {
            if keys.just_pressed(KeyCode::ArrowUp) {
                self.selected = self.selected.saturating_sub(1);
            } else if keys.just_pressed(KeyCode::ArrowDown) {
                self.selected = (self.selected + 1).min(levels.len().saturating_sub(1));
            }
            return None;
        };
        Some(Action::ControlIdConsole { cell, control })
    }
}

#[derive(Component)]
pub struct IdConsoleReadout;

pub fn spawn(commands: &mut Commands) {
    commands
        .spawn((
            IdConsoleReadout,
            Text2d::new(""),
            TextFont::from_font_size(17.0),
            TextColor(Color::srgb_u8(222, 239, 245)),
            Transform::from_xyz(0.0, 0.0, 3.1),
            Visibility::Hidden,
        ))
        .with_children(|panel| {
            panel.spawn((
                Sprite::from_color(Color::srgb_u8(14, 26, 39), Vec2::new(530.0, 335.0)),
                Transform::from_xyz(0.0, 0.0, -0.1),
            ));
        });
}

pub fn sync(
    view: Option<(&Layout, &State, &IdConsole)>,
    panel: &IdConsolePanel,
    mut readout: Single<(&mut Text2d, &mut Visibility), With<IdConsoleReadout>>,
) {
    let Some((layout, state, console)) = view else {
        *readout.1 = Visibility::Hidden;
        return;
    };
    let prototype = &layout.prototypes[&console.prototype];
    let authority = state.console_authority(layout, console);
    let target = console.cards[CardSlot::Target.index()];
    let status = if !console.powered {
        "No power"
    } else if authority.is_none() {
        "Authorized card required"
    } else if target.is_none() {
        "Insert target card"
    } else {
        "Ready to write"
    };
    let label = |slot: CardSlot| {
        console.cards[slot.index()]
            .and_then(|id| state.items.get(&id))
            .map_or_else(|| "Empty".into(), |item| item_label(layout, item))
            .chars()
            .take(25)
            .collect::<String>()
    };
    let mut next = format!(
        "{}\n\n[1] Authority   {}\n[2] Target      {}\n\n{status}\n",
        prototype.name,
        label(CardSlot::Authority),
        label(CardSlot::Target),
    );
    let levels = &prototype.access_levels;
    let start = panel
        .selected
        .saturating_sub(2)
        .min(levels.len().saturating_sub(VISIBLE_LEVELS));
    for (index, tag) in levels.iter().enumerate().skip(start).take(VISIBLE_LEVELS) {
        let cursor = if index == panel.selected { ">" } else { " " };
        let value = if target.is_some_and(|id| has_tag(state, id, tag)) {
            "On"
        } else {
            "Off"
        };
        let locked = if console.powered && authority.is_some_and(|access| access.contains(tag)) {
            ""
        } else {
            "  Locked"
        };
        let name: String = tag.chars().take(25).collect();
        let _ = writeln!(next, "{cursor} {name:<25} {value:<3}{locked}");
    }
    next.push_str("\n[Up / Down] Select   [Space] Change\n[X] Swap hand   [K / Esc] Close");
    *readout.1 = Visibility::Visible;
    if readout.0.0 != next {
        readout.0.0 = next;
    }
}

fn has_tag(state: &State, item: u16, tag: &str) -> bool {
    state
        .items
        .get(&item)
        .and_then(|item| item.identity.as_ref())
        .is_some_and(|card| card.access.iter().any(|access| access == tag))
}
