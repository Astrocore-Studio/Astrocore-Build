//! Меню строительства на клиенте: окно рецептов по категориям с поиском (G), призрак
//! постройки под курсором, поворот средней кнопкой, отмена правой; левая кнопка просит
//! сервер начать постройку (`construction::StartStructureConstruction`). Образец —
//! `ConstructionMenuPresenter`, `ConstructionPlacementHijack` и `ConstructionSystem` клиента
//! SS14 @ 2f4a2aad. Рецепты предметов (`objectType: Item`) пока не показываются.

use astrocore_client::{
    Cursor, Game, GameResources, KeyboardFocus, Textures, UiAction, UiHover, UiStyle, spawn_button,
};
use astrocore_loc::EntityNames;
use astrocore_net::NetEventWorldExt;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;

use crate::construction::{
    Recipe, StartStructureConstruction, recipe, recipe_entity, recipe_first_entity, recipes,
};
use crate::dev_console::DevConsole;
use crate::examine::loc;
use crate::mapping_ui::{DemoCursor, cursor_point, player_map, prototype_image, snap_to_cell};

/// Сколько строк рецептов видно.
const ROWS: usize = 24;
const PANEL: Color = Color::srgba(0.07, 0.08, 0.10, 0.94);
const SELECTED: Color = Color::srgb(0.30, 0.45, 0.75);
const ROW: Color = Color::srgba(0.12, 0.13, 0.16, 0.9);
const GHOST: Color = Color::srgba(0.55, 0.80, 1.0, 0.55);

/// Строка меню: рецепт, название и категория на языке игры.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntry {
    pub id: String,
    pub name: String,
    pub category: String,
}

/// Состояние меню. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default)]
pub struct ConstructionMenu {
    pub open: bool,
    pub search: String,
    pub typing: bool,
    /// Выбранная категория (ключ локализации); `None` — все.
    pub category: Option<String>,
    /// Рецепт, место для которого выбирается.
    pub placing: Option<String>,
    /// Поворот в четвертях оборота против часовой стрелки.
    pub rotation: u8,
    pub next_ack: u32,
    pub version: u64,
}

impl ConstructionMenu {
    fn changed(&mut self) {
        self.version += 1;
    }
}

fn menu(world: &mut bevy_ecs::world::World) -> bevy_ecs::world::Mut<'_, ConstructionMenu> {
    world.get_resource_or_insert_with(ConstructionMenu::default)
}

/// Мышь занята выбором места постройки.
pub fn placing(world: &bevy_ecs::world::World) -> bool {
    world
        .get_resource::<ConstructionMenu>()
        .is_some_and(|m| m.placing.is_some())
}

/// Строки меню: рецепты построек с названиями, по категории и названию.
pub fn entries(world: &mut bevy_ecs::world::World) -> Vec<MenuEntry> {
    let mut list: Vec<MenuEntry> = recipes(world)
        .into_iter()
        .filter(|r| r.object_type == "Structure")
        .map(|r| {
            let name = recipe_name(world, &r);
            let category = if r.category.is_empty() {
                String::new()
            } else {
                loc(world, &r.category, &[])
            };
            MenuEntry {
                id: r.id,
                name,
                category,
            }
        })
        .collect();
    list.sort_by(|a, b| (&a.category, &a.name).cmp(&(&b.category, &b.name)));
    list
}

fn recipe_name(world: &mut bevy_ecs::world::World, recipe: &Recipe) -> String {
    if let Some(key) = &recipe.name {
        return loc(world, key, &[]);
    }
    recipe_entity(world, recipe)
        .and_then(|proto| {
            world
                .get_resource::<EntityNames>()
                .and_then(|n| n.get(&proto)?.name.clone())
        })
        .unwrap_or_else(|| recipe.id.clone())
}

/// Строки по категории и поиску без учёта регистра.
pub fn filter<'a>(
    items: &'a [MenuEntry],
    category: Option<&str>,
    search: &str,
) -> Vec<&'a MenuEntry> {
    let needle = search.to_lowercase();
    items
        .iter()
        .filter(|e| category.is_none_or(|c| e.category == c))
        .filter(|e| {
            needle.is_empty()
                || e.name.to_lowercase().contains(&needle)
                || e.id.to_lowercase().contains(&needle)
        })
        .take(ROWS)
        .collect()
}

/// G открывает и закрывает меню (`OpenCraftingMenu`); поиск печатается с клавиатуры.
fn keys(
    mut events: MessageReader<KeyboardInput>,
    mut game: ResMut<Game>,
    console: Res<DevConsole>,
    mut focus: ResMut<KeyboardFocus>,
) {
    let world = &mut game.0;
    let was_typing = menu(world).typing;
    for event in events.read() {
        if event.state != ButtonState::Pressed || console.open {
            continue;
        }
        let mut state = menu(world);
        if state.typing {
            match &event.logical_key {
                Key::Escape | Key::Enter => state.typing = false,
                Key::Backspace => {
                    state.search.pop();
                }
                Key::Space => state.search.push(' '),
                Key::Character(text) => state
                    .search
                    .extend(text.chars().filter(|c| !c.is_control() && *c != '`')),
                _ => continue,
            }
            state.changed();
            continue;
        }
        if focus.0 {
            continue;
        }
        match event.key_code {
            KeyCode::KeyG => {
                state.open = !state.open;
                state.changed();
            }
            KeyCode::Escape if state.placing.is_some() || state.open => {
                if state.placing.take().is_none() {
                    state.open = false;
                }
                state.changed();
            }
            _ => {}
        }
    }
    let typing = menu(world).typing;
    if typing != was_typing {
        focus.0 = typing;
    }
}

/// Мышь при выборе места: левая — строить, средняя — повернуть, правая — отменить.
fn pointer(
    mouse: Res<ButtonInput<MouseButton>>,
    cursor: Res<Cursor>,
    ui_hover: Res<UiHover>,
    mut game: ResMut<Game>,
) {
    let world = &mut game.0;
    let Some(id) = world
        .get_resource::<ConstructionMenu>()
        .and_then(|m| m.placing.clone())
    else {
        return;
    };
    if ui_hover.0 {
        return;
    }
    if mouse.just_pressed(MouseButton::Right) {
        let mut state = menu(world);
        state.placing = None;
        state.changed();
        return;
    }
    if mouse.just_pressed(MouseButton::Middle) {
        let mut state = menu(world);
        state.rotation = (state.rotation + 1) % 4;
        state.changed();
    }
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let (Some(point), Some(map)) = (cursor.world, player_map(world)) else {
        return;
    };
    let at = snap_to_cell(world, map, point);
    let can_rotate = recipe(world, &id).is_some_and(|r| r.can_rotate);
    let mut state = menu(world);
    state.next_ack = state.next_ack.wrapping_add(1);
    let request = StartStructureConstruction {
        prototype: id,
        x: at.x,
        y: at.y,
        angle: if can_rotate {
            f32::from(state.rotation) * std::f32::consts::FRAC_PI_2
        } else {
            0.0
        },
        ack: state.next_ack,
    };
    world.send_net_event_to_server(&request);
}

#[derive(Component)]
struct GhostView;

/// Призрак постройки под курсором.
#[allow(clippy::too_many_arguments)]
fn ghost(
    mut commands: Commands,
    mut game: ResMut<Game>,
    cursor: Res<Cursor>,
    demo: Option<Res<DemoCursor>>,
    resources: Option<Res<GameResources>>,
    mut textures: ResMut<Textures>,
    mut images: ResMut<Assets<Image>>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>,
    shown: Query<Entity, With<GhostView>>,
) {
    for entity in &shown {
        commands.entity(entity).despawn();
    }
    let world = &mut game.0;
    let Some(state) = world.get_resource::<ConstructionMenu>().cloned() else {
        return;
    };
    let Some(id) = state.placing else {
        return;
    };
    // Демо-курсор проверки без человека важнее курсора виртуального экрана.
    let point = match demo.as_deref() {
        Some(_) => cursor_point(world, &Cursor::default(), demo.as_deref()),
        None => cursor_point(world, &cursor, None),
    };
    let (Some(point), Some(map), Some(resources)) = (point, player_map(world), resources) else {
        return;
    };
    let Some(recipe) = recipe(world, &id) else {
        return;
    };
    let Some(prototype) = recipe_first_entity(world, &recipe) else {
        return;
    };
    let cell = snap_to_cell(world, map, point);
    let turn = if recipe.can_rotate {
        f32::from(state.rotation) * std::f32::consts::FRAC_PI_2
    } else {
        0.0
    };
    if let Some(mut sprite) = prototype_image(
        world,
        &prototype,
        &resources,
        &mut textures,
        &mut images,
        &mut layouts,
    ) {
        sprite.color = GHOST;
        commands.spawn((
            sprite,
            Transform::from_xyz(cell.x, cell.y, 900.0).with_rotation(Quat::from_rotation_z(turn)),
            GhostView,
        ));
    }
}

#[derive(Component)]
struct MenuPanel;

/// Окно меню: поиск, категории, рецепты.
fn window(
    mut commands: Commands,
    mut game: ResMut<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<MenuPanel>>,
    mut catalog: Local<Option<Vec<MenuEntry>>>,
    mut shown: Local<Option<u64>>,
) {
    let world = &mut game.0;
    let Some(state) = world.get_resource::<ConstructionMenu>().cloned() else {
        return;
    };
    if *shown == Some(state.version) {
        return;
    }
    *shown = Some(state.version);
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    if !state.open {
        return;
    }
    let catalog = catalog.get_or_insert_with(|| entries(world));
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(8.0),
                left: Val::Px(8.0),
                width: Val::Px(360.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(8.0)),
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(PANEL),
            Interaction::default(),
            GlobalZIndex(50),
            MenuPanel,
        ))
        .id();
    let title = loc(world, "construction-menu-title", &[]);
    commands.spawn((ChildOf(panel), style.text(format!("{title} (G)"), 16.0)));
    let cursor = if state.typing { "_" } else { "" };
    spawn_button(
        &mut commands,
        panel,
        &style,
        &format!("Поиск: {}{cursor}", state.search),
        UiAction::new(|world| {
            let mut state = menu(world);
            state.typing = !state.typing;
            state.changed();
        }),
    );
    let row = commands
        .spawn((
            ChildOf(panel),
            Node {
                column_gap: Val::Px(4.0),
                row_gap: Val::Px(2.0),
                flex_wrap: FlexWrap::Wrap,
                ..default()
            },
        ))
        .id();
    let mut categories: Vec<String> = catalog.iter().map(|e| e.category.clone()).collect();
    categories.dedup();
    let all = loc(world, "construction-category-all", &[]);
    for category in std::iter::once(None).chain(categories.into_iter().map(Some)) {
        let label = category.clone().unwrap_or_else(|| all.clone());
        let selected = state.category == category;
        let button = commands
            .spawn((
                ChildOf(row),
                Interaction::default(),
                Node {
                    padding: UiRect::axes(Val::Px(4.0), Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(if selected { SELECTED } else { ROW }),
                UiAction::new(move |world| {
                    let mut state = menu(world);
                    state.category = category.clone();
                    state.changed();
                }),
            ))
            .id();
        commands.spawn((ChildOf(button), style.text(label, 12.0)));
    }
    for entry in filter(catalog, state.category.as_deref(), &state.search) {
        let id = entry.id.clone();
        let selected = state.placing.as_deref() == Some(id.as_str());
        let button = commands
            .spawn((
                ChildOf(panel),
                Interaction::default(),
                Node {
                    padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(if selected { SELECTED } else { ROW }),
                UiAction::new(move |world| {
                    let mut state = menu(world);
                    state.placing =
                        (state.placing.as_deref() != Some(id.as_str())).then(|| id.clone());
                    state.changed();
                }),
            ))
            .id();
        commands.spawn((ChildOf(button), style.text(entry.name.clone(), 13.0)));
    }
}

/// Меню строительства.
pub struct ConstructionUiPlugin;

impl Plugin for ConstructionUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, (keys, pointer).after(bevy::input::InputSystems));
        app.add_systems(Update, (ghost, window));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, category: &str) -> MenuEntry {
        MenuEntry {
            id: id.into(),
            name: name.into(),
            category: category.into(),
        }
    }

    #[test]
    fn filters_by_category_and_search() {
        let items = [
            entry("Girder", "балка", "Конструкции"),
            entry("Wall", "стена", "Конструкции"),
            entry("Table", "стол", "Мебель"),
        ];
        let ids = |list: Vec<&MenuEntry>| list.iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(filter(&items, None, "")), ["Girder", "Wall", "Table"]);
        assert_eq!(ids(filter(&items, Some("Мебель"), "")), ["Table"]);
        assert_eq!(ids(filter(&items, Some("Конструкции"), "СТЕ")), ["Wall"]);
        assert_eq!(ids(filter(&items, None, "girder")), ["Girder"]);
    }
}
