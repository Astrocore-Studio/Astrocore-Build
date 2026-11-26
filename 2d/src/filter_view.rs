use astrocore_grid2d::Cell;
use atmos::Gas;
use bevy::prelude::*;

use crate::{Action, FilterControl, MAX_FILTER_RATE_LPS, PipeMachine};

#[derive(Resource, Default)]
pub struct FilterPanel(pub Option<Cell>);

impl FilterPanel {
    pub fn action(
        cell: Cell,
        machine: &PipeMachine,
        keys: &ButtonInput<KeyCode>,
    ) -> Option<Action> {
        let settings = machine.filter?;
        let index = settings.gas.map_or(0, |gas| gas as usize + 1);
        let count = Gas::ALL.len() + 1;
        let gas_at = |index: usize| index.checked_sub(1).map(|index| Gas::ALL[index]);
        let control = if keys.just_pressed(KeyCode::Space) {
            FilterControl::Toggle
        } else if keys.just_pressed(KeyCode::ArrowLeft) {
            FilterControl::SetGas(gas_at((index + count - 1) % count))
        } else if keys.just_pressed(KeyCode::ArrowRight) {
            FilterControl::SetGas(gas_at((index + 1) % count))
        } else if keys.just_pressed(KeyCode::ArrowDown) {
            FilterControl::SetRate(settings.rate_lps.saturating_sub(100))
        } else if keys.just_pressed(KeyCode::ArrowUp) {
            FilterControl::SetRate(
                settings
                    .rate_lps
                    .saturating_add(100)
                    .min(MAX_FILTER_RATE_LPS),
            )
        } else {
            return None;
        };
        Some(Action::ControlFilter { cell, control })
    }
}

#[derive(Component)]
pub struct FilterReadout;

pub fn spawn(commands: &mut Commands) {
    commands
        .spawn((
            FilterReadout,
            Text2d::new(""),
            TextFont::from_font_size(19.0),
            TextColor(Color::srgb_u8(222, 239, 245)),
            Transform::from_xyz(0.0, 0.0, 3.1),
            Visibility::Hidden,
        ))
        .with_children(|panel| {
            panel.spawn((
                Sprite::from_color(Color::srgb_u8(14, 26, 39), Vec2::new(440.0, 184.0)),
                Transform::from_xyz(0.0, 0.0, -0.1),
            ));
        });
}

pub fn sync(
    machine: Option<&PipeMachine>,
    mut readout: Single<(&mut Text2d, &mut Visibility), With<FilterReadout>>,
) {
    let Some((enabled, settings)) =
        machine.and_then(|machine| machine.filter.map(|settings| (machine.enabled, settings)))
    else {
        *readout.1 = Visibility::Hidden;
        return;
    };
    *readout.1 = Visibility::Visible;
    let status = if enabled { "On" } else { "Off" };
    let next = format!(
        "Gas filter\n\n[Space] {status}\n[Left / Right] {}\n[Down / Up] {} L/s\n\n[U / Esc] Close",
        gas_name(settings.gas),
        settings.rate_lps,
    );
    if readout.0.0 != next {
        readout.0.0 = next;
    }
}

pub fn gas_name(gas: Option<Gas>) -> &'static str {
    match gas {
        None => "No gas selected",
        Some(Gas::Oxygen) => "Oxygen",
        Some(Gas::Nitrogen) => "Nitrogen",
        Some(Gas::CarbonDioxide) => "Carbon dioxide",
        Some(Gas::Plasma) => "Plasma",
        Some(Gas::Tritium) => "Tritium",
        Some(Gas::WaterVapor) => "Water vapor",
        Some(Gas::Ammonia) => "Ammonia",
        Some(Gas::NitrousOxide) => "Nitrous oxide",
        Some(Gas::Frezon) => "Frezon",
    }
}
