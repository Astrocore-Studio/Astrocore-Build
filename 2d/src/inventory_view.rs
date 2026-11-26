use bevy::prelude::*;

use crate::{Action, Crew, Kind, Layout, MAX_BACKPACK_SLOTS, State};

const PANEL_WIDTH: f32 = 468.0;
const PANEL_HEIGHT: f32 = 378.0;
const WEAR_Y: f32 = 124.0;
const STORE_Y: f32 = 88.0;
const FIRST_ITEM_Y: f32 = 49.0;
const ROW_HEIGHT: f32 = 25.0;

#[derive(Resource, Default)]
pub struct InventoryPanel {
    pub open: bool,
}

impl InventoryPanel {
    pub fn contains(&self, point: Vec2) -> bool {
        self.open && point.x.abs() <= PANEL_WIDTH / 2.0 && point.y.abs() <= PANEL_HEIGHT / 2.0
    }

    pub fn action(&self, point: Vec2, state: &State, crew: &Crew) -> Option<Action> {
        let y = point.y;
        if (y - WEAR_Y).abs() <= 14.0 {
            return Some(Action::ToggleBackpack);
        }
        if (y - STORE_Y).abs() <= 14.0 {
            return Some(Action::StoreInBackpack);
        }
        let bag = state.items.get(&crew.backpack?)?;
        bag.contents.iter().enumerate().find_map(|(slot, &item)| {
            ((y - item_y(slot)).abs() <= ROW_HEIGHT / 2.0)
                .then_some(Action::TakeFromBackpack { item })
        })
    }
}

#[derive(Component)]
pub struct InventoryBackdrop;

#[derive(Component)]
pub enum InventoryLine {
    Heading,
    Wear,
    Store,
    Item(u8),
}

pub fn spawn(commands: &mut Commands) {
    commands.spawn((
        InventoryBackdrop,
        Sprite::from_color(
            Color::srgb_u8(14, 26, 39),
            Vec2::new(PANEL_WIDTH, PANEL_HEIGHT),
        ),
        Transform::from_xyz(0.0, 0.0, 3.0),
        Visibility::Hidden,
    ));
    for (line, y, font, color) in [
        (
            InventoryLine::Heading,
            159.0,
            20.0,
            Color::srgb_u8(222, 239, 245),
        ),
        (
            InventoryLine::Wear,
            WEAR_Y,
            16.0,
            Color::srgb_u8(121, 214, 194),
        ),
        (
            InventoryLine::Store,
            STORE_Y,
            16.0,
            Color::srgb_u8(121, 214, 194),
        ),
    ] {
        commands.spawn((
            line,
            Text2d::new(""),
            TextFont::from_font_size(font),
            TextColor(color),
            Transform::from_xyz(0.0, y, 3.1),
            Visibility::Hidden,
        ));
    }
    for index in 0..MAX_BACKPACK_SLOTS {
        commands.spawn((
            InventoryLine::Item(index),
            Text2d::new(""),
            TextFont::from_font_size(15.0),
            TextColor(Color::srgb_u8(205, 222, 233)),
            Transform::from_xyz(0.0, item_y(usize::from(index)), 3.1),
            Visibility::Hidden,
        ));
    }
}

pub fn sync(
    player: Option<(&Layout, &State, &Crew)>,
    mut panel: Single<&mut Visibility, With<InventoryBackdrop>>,
    lines: &mut Query<(&InventoryLine, &mut Text2d, &mut Visibility), Without<InventoryBackdrop>>,
) {
    **panel = if player.is_some() {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    for (line, mut text, mut visible) in lines.iter_mut() {
        let Some((layout, state, crew)) = player else {
            *visible = Visibility::Hidden;
            continue;
        };
        let bag = crew.backpack.and_then(|id| state.items.get(&id));
        let prototype = bag.and_then(|item| layout.prototypes.get(&item.prototype));
        let slots = match prototype.map(|p| p.kind) {
            Some(Kind::Backpack { slots }) => slots,
            _ => 0,
        };
        let next = match line {
            InventoryLine::Heading => "INVENTORY    [I] Close".into(),
            InventoryLine::Wear => prototype.map_or_else(
                || "Back empty   /   Equip held backpack".into(),
                |p| format!("{}   /   Remove", short(&p.name)),
            ),
            InventoryLine::Store => {
                let count = bag.map_or(0, |item| item.contents.len());
                if slots > 0 {
                    format!("Store held item   ({count}/{slots})")
                } else {
                    String::new()
                }
            }
            InventoryLine::Item(index) if *index < slots => {
                let name = bag
                    .and_then(|bag| bag.contents.get(usize::from(*index)))
                    .and_then(|item| state.items.get(item))
                    .and_then(|item| layout.prototypes.get(&item.prototype))
                    .map_or("Empty", |item| item.name.as_str());
                format!("{}   {}", index + 1, short(name))
            }
            InventoryLine::Item(_) => {
                *visible = Visibility::Hidden;
                continue;
            }
        };
        *visible = Visibility::Visible;
        if text.0 != next {
            text.0 = next;
        }
    }
}

fn item_y(slot: usize) -> f32 {
    FIRST_ITEM_Y - ROW_HEIGHT * slot as f32
}

fn short(name: &str) -> String {
    name.chars().take(28).collect()
}
