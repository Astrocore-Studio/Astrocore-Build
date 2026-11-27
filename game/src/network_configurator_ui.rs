//! Окна настройщика сети на клиенте: сохранённые устройства (убрать, очистить), настройка
//! списка устройства (задать, добавить, очистить, скопировать) и выбор портов связи (порт
//! источника, затем порт приёмника — связь переключается; очистить; связать по умолчанию).
//! Образец — `NetworkConfiguratorListMenu`, `NetworkConfiguratorDeviceList`,
//! `NetworkConfiguratorConfigurationMenu`, `NetworkConfiguratorLinkMenu` клиента SS14 @ 2f4a2aad.
//!
//! Нет пока: линий между связанными портами и голографического показа списка («Показать»).

use astrocore_client::{UiAction, WindowKinds, spawn_button, spawn_label};
use astrocore_ui::{OpenUis, UiWorldExt};
use bevy::prelude::*;
use bevy_ecs::world::World as GameWorld;

use crate::examine::loc;
use crate::network_configurator::{
    CONFIGURE_UI_KEY, DeviceLinkUiState, DeviceListUiState, LINK_UI_KEY, LIST_UI_KEY,
    NetworkConfiguratorMessage,
};

/// Выбранный порт окна связей: сущность окна, порт источника (`true`) или приёмника.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkSelection(pub Option<(bevy_ecs::entity::Entity, bool, String)>);

/// Что сделать по нажатию порта (`OnButtonPressed`): выбрать, снять выбор или связать.
pub fn press_port(
    selection: &mut LinkSelection,
    entity: bevy_ecs::entity::Entity,
    source: bool,
    id: &str,
) -> Option<(String, String)> {
    match selection.0.take() {
        Some((window, side, selected)) if window == entity => {
            if side == source {
                // Тот же порт — снять выбор; другой порт той же стороны — не менять.
                if selected != id {
                    selection.0 = Some((window, side, selected));
                }
                None
            } else if source {
                Some((id.to_owned(), selected))
            } else {
                Some((selected, id.to_owned()))
            }
        }
        _ => {
            selection.0 = Some((entity, source, id.to_owned()));
            None
        }
    }
}

fn redraw(world: &mut GameWorld) {
    if let Some(mut uis) = world.get_resource_mut::<OpenUis>() {
        uis.revision += 1;
    }
}

fn device_rows(
    commands: &mut Commands,
    content: Entity,
    style: &astrocore_client::UiStyle,
    state: &DeviceListUiState,
    entity: bevy_ecs::entity::Entity,
    key: &'static str,
    removable: bool,
) {
    for device in &state.devices {
        let row = commands
            .spawn((
                ChildOf(content),
                Node {
                    column_gap: Val::Px(8.0),
                    align_items: AlignItems::Center,
                    ..default()
                },
            ))
            .id();
        // Имя — первые 11 знаков, как в SS14.
        let short: String = device.name.chars().take(11).collect();
        spawn_label(
            commands,
            row,
            style,
            &format!("{short}  {}", device.address),
        );
        if removable {
            let address = device.address_id;
            spawn_button(
                commands,
                row,
                style,
                "X",
                UiAction::new(move |world| {
                    world.send_ui_message(
                        entity,
                        key,
                        &NetworkConfiguratorMessage::RemoveDevice(address),
                    );
                }),
            );
        }
    }
}

/// Окна настройщика сети.
pub struct NetworkConfiguratorUiPlugin;

impl Plugin for NetworkConfiguratorUiPlugin {
    fn build(&self, app: &mut App) {
        let mut kinds = app.world_mut().resource_mut::<WindowKinds>();
        kinds.register(
            "NetworkConfiguratorListBoundUserInterface",
            |commands, content, ui, world, style| {
                let state = ui.state::<DeviceListUiState>().unwrap_or_default();
                spawn_label(
                    commands,
                    content,
                    style,
                    &loc(world, "network-configurator-title-saved-devices", &[]),
                );
                device_rows(
                    commands,
                    content,
                    style,
                    &state,
                    ui.entity,
                    LIST_UI_KEY,
                    true,
                );
                spawn_label(
                    commands,
                    content,
                    style,
                    &loc(
                        world,
                        "network-configurator-ui-count-label",
                        &[(
                            "count",
                            astrocore_loc::LocArg::from(state.devices.len() as i64),
                        )],
                    ),
                );
                let entity = ui.entity;
                spawn_button(
                    commands,
                    content,
                    style,
                    &loc(world, "network-configurator-ui-clear-button", &[]),
                    UiAction::new(move |world| {
                        world.send_ui_message(
                            entity,
                            LIST_UI_KEY,
                            &NetworkConfiguratorMessage::ClearDevices,
                        );
                    }),
                );
            },
        );
        kinds.register(
            "NetworkConfiguratorBoundUserInterface",
            |commands, content, ui, world, style| {
                let state = ui.state::<DeviceListUiState>().unwrap_or_default();
                spawn_label(
                    commands,
                    content,
                    style,
                    &loc(
                        world,
                        "network-configurator-title-device-configuration",
                        &[],
                    ),
                );
                device_rows(
                    commands,
                    content,
                    style,
                    &state,
                    ui.entity,
                    CONFIGURE_UI_KEY,
                    false,
                );
                let row = commands
                    .spawn((
                        ChildOf(content),
                        Node {
                            column_gap: Val::Px(6.0),
                            flex_wrap: FlexWrap::Wrap,
                            ..default()
                        },
                    ))
                    .id();
                let entity = ui.entity;
                for (key, message) in [
                    (
                        "network-configurator-text-set",
                        NetworkConfiguratorMessage::Set,
                    ),
                    (
                        "network-configurator-text-add",
                        NetworkConfiguratorMessage::Add,
                    ),
                    (
                        "network-configurator-text-clear",
                        NetworkConfiguratorMessage::Clear,
                    ),
                    (
                        "network-configurator-text-copy",
                        NetworkConfiguratorMessage::Copy,
                    ),
                ] {
                    spawn_button(
                        commands,
                        row,
                        style,
                        &loc(world, key, &[]),
                        UiAction::new(move |world| {
                            world.send_ui_message(entity, CONFIGURE_UI_KEY, &message);
                        }),
                    );
                }
            },
        );
        kinds.register(
            "NetworkConfiguratorLinkBoundUserInterface",
            |commands, content, ui, world, style| {
                let Some(state) = ui.state::<DeviceLinkUiState>() else {
                    return;
                };
                let entity = ui.entity;
                let selected = world
                    .get_resource::<LinkSelection>()
                    .and_then(|s| s.0.clone())
                    .filter(|(window, _, _)| *window == entity);
                spawn_label(
                    commands,
                    content,
                    style,
                    &loc(world, "signal-port-selector-help", &[]),
                );
                spawn_label(
                    commands,
                    content,
                    style,
                    &format!("{}  →  {}", state.source_address, state.sink_address),
                );
                let columns = commands
                    .spawn((
                        ChildOf(content),
                        Node {
                            column_gap: Val::Px(24.0),
                            ..default()
                        },
                    ))
                    .id();
                for (source, ports) in [(true, &state.sources), (false, &state.sinks)] {
                    let column = commands
                        .spawn((
                            ChildOf(columns),
                            Node {
                                flex_direction: FlexDirection::Column,
                                ..default()
                            },
                        ))
                        .id();
                    for (id, name) in ports {
                        // Связанный порт отмечен числом связей, выбранный — скобками.
                        let count = state
                            .links
                            .iter()
                            .filter(|(a, b)| if source { a == id } else { b == id })
                            .count();
                        let mut label = name.clone();
                        if count > 0 {
                            label = format!("{label} ({count})");
                        }
                        if selected
                            .as_ref()
                            .is_some_and(|(_, side, port)| *side == source && port == id)
                        {
                            label = format!("[ {label} ]");
                        }
                        let id = id.clone();
                        spawn_button(
                            commands,
                            column,
                            style,
                            &label,
                            UiAction::new(move |world| {
                                let mut selection = world
                                    .get_resource_or_insert_with(LinkSelection::default)
                                    .clone();
                                let link = press_port(&mut selection, entity, source, &id);
                                world.insert_resource(selection);
                                if let Some((a, b)) = link {
                                    world.send_ui_message(
                                        entity,
                                        LINK_UI_KEY,
                                        &NetworkConfiguratorMessage::LinkToggle(a, b),
                                    );
                                }
                                redraw(world);
                            }),
                        );
                    }
                }
                for (a, b) in &state.links {
                    spawn_label(commands, content, style, &format!("{a} → {b}"));
                }
                let row = commands
                    .spawn((
                        ChildOf(content),
                        Node {
                            column_gap: Val::Px(6.0),
                            ..default()
                        },
                    ))
                    .id();
                spawn_button(
                    commands,
                    row,
                    style,
                    &loc(world, "signal-port-selector-menu-clear", &[]),
                    UiAction::new(move |world| {
                        world.send_ui_message(
                            entity,
                            LINK_UI_KEY,
                            &NetworkConfiguratorMessage::LinkClear,
                        );
                    }),
                );
                if let Some(defaults) = state.defaults.clone() {
                    spawn_button(
                        commands,
                        row,
                        style,
                        &loc(world, "signal-port-selector-menu-link-defaults", &[]),
                        UiAction::new(move |world| {
                            world.send_ui_message(
                                entity,
                                LINK_UI_KEY,
                                &NetworkConfiguratorMessage::LinkSave(defaults.clone()),
                            );
                        }),
                    );
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_picked_on_both_sides_before_linking() {
        let entity = bevy_ecs::entity::Entity::from_raw_u32(5).unwrap();
        let mut selection = LinkSelection::default();
        assert_eq!(press_port(&mut selection, entity, true, "Pressed"), None);
        // Другой порт той же стороны выбор не меняет; тот же — снимает.
        assert_eq!(press_port(&mut selection, entity, true, "Status"), None);
        assert_eq!(selection.0.as_ref().unwrap().2, "Pressed");
        assert_eq!(press_port(&mut selection, entity, true, "Pressed"), None);
        assert_eq!(selection.0, None);
        // Сначала приёмник, затем источник — связь всё равно «источник → приёмник».
        press_port(&mut selection, entity, false, "Toggle");
        assert_eq!(
            press_port(&mut selection, entity, true, "Pressed"),
            Some(("Pressed".into(), "Toggle".into()))
        );
        assert_eq!(selection.0, None);
    }
}
