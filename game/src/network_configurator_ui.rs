//! Окна настройщика сети на клиенте: сохранённые устройства (убрать, очистить), настройка
//! списка устройства (задать, добавить, очистить, скопировать, показать) и выбор портов связи
//! (порт источника, затем порт приёмника — связь переключается; очистить; связать по
//! умолчанию). Связанные порты соединены линиями между столбцами портов (`LinksRender`);
//! кнопка «Показать» окна настройки рисует на карте линии от списка к его устройствам своим
//! цветом для каждого списка (`NetworkConfiguratorLinkOverlay`).
//! Образец — `NetworkConfiguratorListMenu`, `NetworkConfiguratorDeviceList`,
//! `NetworkConfiguratorConfigurationMenu`, `NetworkConfiguratorLinkMenu`,
//! `NetworkConfiguratorLinkOverlay` клиента SS14 @ 2f4a2aad.

use astrocore_client::{Game, UiAction, UiStyle, WindowKinds, spawn_button, spawn_label};
use astrocore_map::TransformWorldExt;
use astrocore_net::{NetEntities, NetEntity};
use astrocore_ui::{OpenUis, UiWorldExt};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;
use bevy_ecs::entity::Entity as GameEntity;
use bevy_ecs::world::World as GameWorld;

use crate::examine::loc;
use crate::network_configurator::{
    CONFIGURE_UI_KEY, ClientLinkOverlay, DeviceLinkUiState, DeviceListUiState, LINK_UI_KEY,
    LIST_UI_KEY, NetworkConfiguratorMessage,
};

/// Высота строки порта в окне связей: линии идут между серединами строк.
const PORT_ROW: f32 = 30.0;
/// Ширина полосы с линиями между столбцами портов.
const LINK_GAP: f32 = 90.0;
const PORT: Color = Color::srgba(0.16, 0.18, 0.24, 1.0);
const PORT_SELECTED: Color = Color::srgba(0.30, 0.34, 0.50, 1.0);
const LINK_LINE: Color = Color::srgb(0.55, 0.75, 1.0);

/// Выбранный порт окна связей: сущность окна, порт источника (`true`) или приёмника.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkSelection(pub Option<(GameEntity, bool, String)>);

/// Что сделать по нажатию порта (`OnButtonPressed`): выбрать, снять выбор или связать.
pub fn press_port(
    selection: &mut LinkSelection,
    entity: GameEntity,
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

/// Отрезок между точками полосы связей: левый верхний угол узла, длина и угол поворота.
/// Узел шириной в длину отрезка стоит серединой на середине отрезка и повёрнут.
pub fn link_segment(from: Vec2, to: Vec2) -> (Vec2, f32, f32) {
    let delta = to - from;
    let length = delta.length();
    let middle = (from + to) / 2.0;
    (
        Vec2::new(middle.x - length / 2.0, middle.y - 1.0),
        length,
        delta.y.atan2(delta.x),
    )
}

/// Цвет показа списка: свой для каждого списка (в SS14 — случайный).
pub fn overlay_color(list: NetEntity) -> Color {
    let hash = list.0.wrapping_mul(2_654_435_761);
    Color::hsl((hash % 360) as f32, 0.85, 0.6)
}

fn redraw(world: &mut GameWorld) {
    if let Some(mut uis) = world.get_resource_mut::<OpenUis>() {
        uis.revision += 1;
    }
}

fn device_rows(
    commands: &mut Commands,
    content: Entity,
    style: &UiStyle,
    state: &DeviceListUiState,
    entity: GameEntity,
    key: &'static str,
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

fn row(commands: &mut Commands, parent: Entity) -> Entity {
    commands
        .spawn((
            ChildOf(parent),
            Node {
                column_gap: Val::Px(6.0),
                flex_wrap: FlexWrap::Wrap,
                ..default()
            },
        ))
        .id()
}

fn list_window(
    commands: &mut Commands,
    content: Entity,
    ui: &astrocore_ui::OpenUi,
    world: &GameWorld,
    style: &UiStyle,
) {
    let state = ui.state::<DeviceListUiState>().unwrap_or_default();
    spawn_label(
        commands,
        content,
        style,
        &loc(world, "network-configurator-title-saved-devices", &[]),
    );
    device_rows(commands, content, style, &state, ui.entity, LIST_UI_KEY);
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
}

fn configure_window(
    commands: &mut Commands,
    content: Entity,
    ui: &astrocore_ui::OpenUi,
    world: &GameWorld,
    style: &UiStyle,
) {
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
    );
    let buttons = row(commands, content);
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
            buttons,
            style,
            &loc(world, key, &[]),
            UiAction::new(move |world| {
                world.send_ui_message(entity, CONFIGURE_UI_KEY, &message);
            }),
        );
    }
    // «Показать» — нажата, пока список показан на карте.
    if let Some(list) = state.list {
        let shown = world
            .get_resource::<ClientLinkOverlay>()
            .is_some_and(|o| o.is_shown(list));
        let label = loc(world, "network-configurator-text-show", &[]);
        let button = spawn_button(
            commands,
            buttons,
            style,
            &if shown { format!("[ {label} ]") } else { label },
            UiAction::new(move |world| {
                world.send_ui_message(
                    entity,
                    CONFIGURE_UI_KEY,
                    &NetworkConfiguratorMessage::Show(!shown),
                );
            }),
        );
        if shown {
            commands
                .entity(button)
                .insert(BackgroundColor(PORT_SELECTED));
        }
    }
}

fn link_window(
    commands: &mut Commands,
    content: Entity,
    ui: &astrocore_ui::OpenUi,
    world: &GameWorld,
    style: &UiStyle,
) {
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
    let rows = state.sources.len().max(state.sinks.len()).max(1) as f32;
    let columns = commands
        .spawn((
            ChildOf(content),
            Node {
                height: Val::Px(rows * PORT_ROW),
                ..default()
            },
        ))
        .id();
    let column = |commands: &mut Commands, source: bool| {
        let node = commands
            .spawn((
                ChildOf(columns),
                Node {
                    flex_direction: FlexDirection::Column,
                    width: Val::Px(150.0),
                    ..default()
                },
            ))
            .id();
        let ports = if source { &state.sources } else { &state.sinks };
        for (id, name) in ports {
            let picked = selected
                .as_ref()
                .is_some_and(|(_, side, port)| *side == source && port == id);
            let id = id.clone();
            let button = commands
                .spawn((
                    ChildOf(node),
                    Interaction::default(),
                    Node {
                        height: Val::Px(PORT_ROW - 4.0),
                        margin: UiRect::bottom(Val::Px(4.0)),
                        padding: UiRect::horizontal(Val::Px(8.0)),
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(if picked { PORT_SELECTED } else { PORT }),
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
                ))
                .id();
            commands.spawn((ChildOf(button), style.text(name.clone(), 13.0)));
        }
    };
    column(commands, true);
    // Полоса с линиями связей между столбцами (`LinksRender`).
    let gap = commands
        .spawn((
            ChildOf(columns),
            Node {
                width: Val::Px(LINK_GAP),
                height: Val::Px(rows * PORT_ROW),
                ..default()
            },
        ))
        .id();
    for (source, sink) in &state.links {
        let (Some(a), Some(b)) = (
            state.sources.iter().position(|(id, _)| id == source),
            state.sinks.iter().position(|(id, _)| id == sink),
        ) else {
            continue;
        };
        let middle = |index: usize| (index as f32 + 0.5) * PORT_ROW - 2.0;
        let (corner, length, angle) =
            link_segment(Vec2::new(0.0, middle(a)), Vec2::new(LINK_GAP, middle(b)));
        commands.spawn((
            ChildOf(gap),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(corner.x),
                top: Val::Px(corner.y),
                width: Val::Px(length),
                height: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(LINK_LINE),
            UiTransform {
                rotation: Rot2::radians(angle),
                ..default()
            },
        ));
    }
    column(commands, false);
    let buttons = row(commands, content);
    spawn_button(
        commands,
        buttons,
        style,
        &loc(world, "signal-port-selector-menu-clear", &[]),
        UiAction::new(move |world| {
            world.send_ui_message(entity, LINK_UI_KEY, &NetworkConfiguratorMessage::LinkClear);
        }),
    );
    if let Some(defaults) = state.defaults.clone() {
        spawn_button(
            commands,
            buttons,
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
}

#[derive(Component)]
struct OverlayLine;

/// Линии показа на карте (`NetworkConfiguratorLinkOverlay.Draw`): от списка к устройствам на
/// карте игрока; сущности вне карты игрока пропускаются.
fn draw_overlay(mut commands: Commands, game: Res<Game>, lines: Query<Entity, With<OverlayLine>>) {
    for line in &lines {
        commands.entity(line).despawn();
    }
    let world = &game.0;
    let Some(overlay) = world.get_resource::<ClientLinkOverlay>() else {
        return;
    };
    if overlay.overlay.lists.is_empty() {
        return;
    }
    let Some(map) = crate::mapping_ui::player_map(world) else {
        return;
    };
    let position = |net: NetEntity| {
        let entity = world.get_resource::<NetEntities>()?.entity(net)?;
        let at = world.map_coordinates(entity)?;
        (at.map == map).then_some(Vec2::new(at.position.x, at.position.y))
    };
    for (list, devices) in &overlay.overlay.lists {
        let Some(from) = position(*list) else {
            continue;
        };
        let color = overlay_color(*list);
        for device in devices {
            let Some(to) = position(*device) else {
                continue;
            };
            let delta = to - from;
            let middle = (from + to) / 2.0;
            commands.spawn((
                Sprite::from_color(color, Vec2::new(delta.length(), 0.06)),
                Transform::from_xyz(middle.x, middle.y, 950.0)
                    .with_rotation(Quat::from_rotation_z(delta.y.atan2(delta.x))),
                OverlayLine,
            ));
        }
    }
}

/// Окна настройщика сети и показ списков на карте.
pub struct NetworkConfiguratorUiPlugin;

impl Plugin for NetworkConfiguratorUiPlugin {
    fn build(&self, app: &mut App) {
        let mut kinds = app.world_mut().resource_mut::<WindowKinds>();
        kinds.register("NetworkConfiguratorListBoundUserInterface", list_window);
        kinds.register("NetworkConfiguratorBoundUserInterface", configure_window);
        kinds.register("NetworkConfiguratorLinkBoundUserInterface", link_window);
        app.add_systems(Update, draw_overlay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_picked_on_both_sides_before_linking() {
        let entity = GameEntity::from_raw_u32(5).unwrap();
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

    #[test]
    fn link_lines_join_port_rows_and_lists_get_own_colours() {
        let (corner, length, angle) = link_segment(Vec2::new(0.0, 15.0), Vec2::new(90.0, 15.0));
        assert_eq!((corner, length, angle), (Vec2::new(0.0, 14.0), 90.0, 0.0));
        let (_, length, angle) = link_segment(Vec2::new(0.0, 0.0), Vec2::new(30.0, 40.0));
        assert!((length - 50.0).abs() < 1e-4);
        assert!((angle - (40.0f32).atan2(30.0)).abs() < 1e-6);
        assert_ne!(overlay_color(NetEntity(1)), overlay_color(NetEntity(2)));
        assert_eq!(overlay_color(NetEntity(7)), overlay_color(NetEntity(7)));
    }
}
