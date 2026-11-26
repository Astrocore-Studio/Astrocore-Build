//! Маппинг в игре на клиенте: палитры сущностей (F5), тайлов (F6) и декалей (F8) с поиском, призрак
//! размещаемого под курсором, привязка к клеткам, поворот (средняя кнопка), заливка линией
//! (Shift) и прямоугольником (Ctrl), ластик (сущность под курсором или прямоугольник с Shift),
//! копирование сущности под курсором (P), отмена — правая кнопка. Запросы уходят серверу
//! (`placement`). Образец — `Robust.Client/Placement/PlacementManager.cs`,
//! `EntitySpawningUIController` и `TileSpawningUIController` SS14 @ 2f4a2aad.

use astrocore_client::{
    Cursor, Game, GameResources, Hovered, KeyboardFocus, LayerSource, PointerCapture,
    Sprite as GameSprite, Textures, UiAction, UiHover, UiStyle, spawn_button,
};
use astrocore_ecs::MetaData;
use astrocore_map::{MapCoordinates, MapId, MapWorldExt, TileDefinitions, TransformWorldExt};
use astrocore_net::{ClientNet, NetEntity, NetEventWorldExt};
use astrocore_prototypes::Prototypes;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::ui::DeprecatedInteraction as Interaction;

use crate::dev_console::DevConsole;
use crate::placement::PlacementRequest;

/// Сколько строк палитры видно.
const ROWS: usize = 28;
/// Наибольшее число клеток одной заливки.
const MAX_FILL: usize = 1024;
const PANEL: Color = Color::srgba(0.07, 0.08, 0.10, 0.94);
const SELECTED: Color = Color::srgb(0.30, 0.45, 0.75);
const ROW: Color = Color::srgba(0.12, 0.13, 0.16, 0.9);
const GHOST: Color = Color::srgba(1.0, 1.0, 1.0, 0.55);
const ERASER: Color = Color::srgba(0.95, 0.30, 0.25, 0.35);
const OUTLINE: Color = Color::srgba(0.45, 0.75, 1.0, 0.25);

/// Что делает левая кнопка.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    None,
    Entity(String),
    Tile(String),
    Decal(String),
    Erase,
}

/// Открытая палитра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    Entities,
    Tiles,
    Decals,
}

/// Инструменты маппинга. Ресурс мира клиента: кнопки окна меняют его через `UiAction`.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default)]
pub struct MappingTools {
    pub tool: Tool,
    /// Поворот в четвертях оборота против часовой стрелки.
    pub rotation: u8,
    /// Свободное размещение: без привязки к центру клетки.
    pub free: bool,
    pub palette: Option<Palette>,
    pub search: String,
    /// Клавиатура пишет в поле поиска.
    pub typing: bool,
    /// Растёт с каждым изменением: окно перерисовывается.
    pub version: u64,
}

impl MappingTools {
    fn changed(&mut self) {
        self.version += 1;
    }

    pub fn select(&mut self, tool: Tool) {
        self.tool = tool;
        self.changed();
    }
}

fn tools_of(world: &mut bevy_ecs::world::World) -> bevy_ecs::world::Mut<'_, MappingTools> {
    world.get_resource_or_insert_with(MappingTools::default)
}

/// Карта игрока.
fn player_map(world: &bevy_ecs::world::World) -> Option<MapId> {
    let player = world.resource::<ClientNet>().attached?;
    world.map_coordinates(player).map(|at| at.map)
}

/// Центр клетки под точкой карты: клетка грида под точкой или клетка карты.
pub fn snap_to_cell(world: &bevy_ecs::world::World, map: MapId, point: Vec2) -> Vec2 {
    let point = astrocore_math::Vec2::new(point.x, point.y);
    let snapped = match world
        .find_grid_at(MapCoordinates::new(map, point))
        .and_then(|grid| world.world_transform(grid))
    {
        Some(grid) => {
            let local = grid.to_local(point);
            grid.to_world(local.floor() + astrocore_math::Vec2::splat(0.5))
        }
        None => point.floor() + astrocore_math::Vec2::splat(0.5),
    };
    Vec2::new(snapped.x, snapped.y)
}

/// Клетки заливки от `from` до `to` с шагом в клетку: линия по большей оси или прямоугольник.
pub fn fill_cells(from: Vec2, to: Vec2, rectangle: bool) -> Vec<Vec2> {
    let steps = |a: f32, b: f32| ((b - a).abs().round() as i32, (b - a).signum());
    let (dx, sx) = steps(from.x, to.x);
    let (dy, sy) = steps(from.y, to.y);
    let mut cells = Vec::new();
    if rectangle {
        for i in 0..=dx {
            for j in 0..=dy {
                cells.push(Vec2::new(from.x + sx * i as f32, from.y + sy * j as f32));
            }
        }
    } else if dx >= dy {
        cells.extend((0..=dx).map(|i| Vec2::new(from.x + sx * i as f32, from.y)));
    } else {
        cells.extend((0..=dy).map(|j| Vec2::new(from.x, from.y + sy * j as f32)));
    }
    cells.truncate(MAX_FILL);
    cells
}

/// Прототипы для палитры: без абстрактных и скрытых из меню спавна.
fn spawnable(prototypes: &Prototypes) -> Vec<(String, String)> {
    let hidden = |id: &str| {
        let mut current = prototypes.entity(id);
        let mut depth = 0;
        while let Some(proto) = current {
            if proto.categories.iter().any(|c| c == "HideSpawnMenu") {
                return true;
            }
            depth += 1;
            if depth > 32 {
                break;
            }
            current = proto.parents.first().and_then(|p| prototypes.entity(p));
        }
        false
    };
    let mut list: Vec<(String, String)> = prototypes
        .entities()
        .filter(|proto| !proto.is_abstract && !hidden(&proto.id))
        .map(|proto| (proto.id.to_string(), proto.name.clone().unwrap_or_default()))
        .collect();
    list.sort();
    list
}

/// Строки палитры по поиску: совпадение в идентификаторе или имени без учёта регистра.
pub fn matching<'a>(items: &'a [(String, String)], search: &str) -> Vec<&'a (String, String)> {
    let needle = search.to_lowercase();
    items
        .iter()
        .filter(|(id, name)| {
            needle.is_empty()
                || id.to_lowercase().contains(&needle)
                || name.to_lowercase().contains(&needle)
        })
        .take(ROWS)
        .collect()
}

/// Проверка без человека: курсор стоит со сдвигом от игрока, если мыши в окне нет.
#[derive(Resource, Debug, Clone, Copy)]
pub struct DemoCursor(pub Vec2);

fn cursor_point(
    world: &bevy_ecs::world::World,
    cursor: &Cursor,
    demo: Option<&DemoCursor>,
) -> Option<Vec2> {
    cursor.world.or_else(|| {
        let player = world.resource::<ClientNet>().attached?;
        let at = world.map_coordinates(player)?.position;
        Some(Vec2::new(at.x, at.y) + demo?.0)
    })
}

#[derive(Default)]
struct Catalog {
    entities: Vec<(String, String)>,
    tiles: Vec<(String, String)>,
    decals: Vec<(String, String)>,
}

/// Декали для палитры: без скрытых из меню (`showMenu: false`).
fn decal_ids(prototypes: &Prototypes) -> Vec<(String, String)> {
    let mut ids: Vec<(String, String)> = prototypes
        .ids("decal")
        .filter(|id| {
            prototypes
                .resolved_node("decal", id)
                .and_then(|node| {
                    node.get("showMenu")
                        .and_then(astrocore_data::DataNode::as_str)
                        .map(str::to_owned)
                })
                .is_none_or(|show| !show.eq_ignore_ascii_case("false"))
        })
        .map(|id| (id.to_owned(), String::new()))
        .collect();
    ids.sort();
    ids
}

/// Клавиши маппинга: палитры, поиск, копирование, отмена.
fn keys(
    mut events: MessageReader<KeyboardInput>,
    mut game: ResMut<Game>,
    console: Res<DevConsole>,
    hovered: Res<Hovered>,
    mut focus: ResMut<KeyboardFocus>,
) {
    let world = &mut game.0;
    let was_typing = tools_of(world).typing;
    for event in events.read() {
        if event.state != ButtonState::Pressed || console.open {
            continue;
        }
        let mut tools = tools_of(world);
        if tools.typing {
            match &event.logical_key {
                Key::Escape | Key::Enter => tools.typing = false,
                Key::Backspace => {
                    tools.search.pop();
                }
                Key::Space => tools.search.push(' '),
                Key::Character(text) => tools
                    .search
                    .extend(text.chars().filter(|c| !c.is_control() && *c != '`')),
                _ => continue,
            }
            tools.changed();
            continue;
        }
        let toggle = |tools: &mut MappingTools, palette: Palette| {
            tools.palette = (tools.palette != Some(palette)).then_some(palette);
            tools.search.clear();
            tools.changed();
        };
        match event.key_code {
            KeyCode::F5 => toggle(&mut tools, Palette::Entities),
            KeyCode::F6 => toggle(&mut tools, Palette::Tiles),
            KeyCode::F8 => toggle(&mut tools, Palette::Decals),
            KeyCode::Escape if tools.tool != Tool::None || tools.palette.is_some() => {
                tools.tool = Tool::None;
                tools.palette = None;
                tools.changed();
            }
            // `EditorCopyObject`: размещать то же, что под курсором.
            KeyCode::KeyP => {
                let copied = hovered.entities.first().and_then(|entity| {
                    let proto = world.get::<MetaData>(*entity)?.prototype.clone()?;
                    let quarter = world
                        .get::<astrocore_map::Transform>(*entity)
                        .map(|t| t.local_rotation().0 / std::f32::consts::FRAC_PI_2)
                        .unwrap_or(0.0);
                    Some((proto.to_string(), quarter.round().rem_euclid(4.0) as u8))
                });
                if let Some((proto, rotation)) = copied {
                    let mut tools = tools_of(world);
                    tools.rotation = rotation;
                    tools.select(Tool::Entity(proto));
                }
            }
            _ => {}
        }
    }
    let typing = tools_of(world).typing;
    if typing != was_typing {
        focus.0 = typing;
    }
}

/// Начало протягивания: точка и вид заливки.
#[derive(Default)]
struct Drag(Option<(Vec2, bool)>);

/// Кнопки мыши: размещение, заливка, ластик, поворот, отмена.
#[allow(clippy::too_many_arguments)]
fn pointer(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    cursor: Res<Cursor>,
    ui_hover: Res<UiHover>,
    hovered: Res<Hovered>,
    mut capture: ResMut<PointerCapture>,
    mut game: ResMut<Game>,
    mut drag: Local<Drag>,
) {
    let world = &mut game.0;
    let current = tools_of(world).clone();
    let active = current.tool != Tool::None;
    if capture.0 != active {
        capture.0 = active;
    }
    if !active {
        drag.0 = None;
        return;
    }
    if mouse.just_pressed(MouseButton::Right) && !ui_hover.0 {
        tools_of(world).select(Tool::None);
        drag.0 = None;
        return;
    }
    if mouse.just_pressed(MouseButton::Middle) {
        let mut tools = tools_of(world);
        tools.rotation = (tools.rotation + 1) % 4;
        tools.changed();
    }
    let (Some(point), Some(map)) = (cursor.world, player_map(world)) else {
        return;
    };
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let at = if current.free && matches!(current.tool, Tool::Entity(_) | Tool::Decal(_)) {
        point
    } else {
        snap_to_cell(world, map, point)
    };
    if mouse.just_pressed(MouseButton::Left) && !ui_hover.0 {
        if shift || ctrl {
            // Ластик протягивает только прямоугольник.
            drag.0 = Some((at, ctrl || current.tool == Tool::Erase));
            return;
        }
        match &current.tool {
            Tool::Erase => {
                match hovered
                    .entities
                    .first()
                    .and_then(|entity| world.get::<NetEntity>(*entity).copied())
                {
                    Some(net) => world
                        .send_net_event_to_server(&PlacementRequest::EraseEntity { entity: net }),
                    // Под курсором нет сущности: стираются декали клетки.
                    None => world.send_net_event_to_server(&PlacementRequest::EraseDecals {
                        map: map.0,
                        min: astrocore_math::Vec2::new(at.x - 0.5, at.y - 0.5),
                        max: astrocore_math::Vec2::new(at.x + 0.5, at.y + 0.5),
                    }),
                }
            }
            tool => send_place(world, tool, map, at, current.rotation),
        }
    }
    if mouse.just_released(MouseButton::Left)
        && let Some((start, rectangle)) = drag.0.take()
    {
        if current.tool == Tool::Erase {
            let (min, max) = (start.min(at), start.max(at));
            world.send_net_event_to_server(&PlacementRequest::EraseRect {
                map: map.0,
                min: astrocore_math::Vec2::new(min.x - 0.5, min.y - 0.5),
                max: astrocore_math::Vec2::new(max.x + 0.5, max.y + 0.5),
            });
            return;
        }
        for cell in fill_cells(start, at, rectangle) {
            send_place(world, &current.tool, map, cell, current.rotation);
        }
    }
}

fn send_place(world: &mut bevy_ecs::world::World, tool: &Tool, map: MapId, at: Vec2, rotation: u8) {
    let position = astrocore_math::Vec2::new(at.x, at.y);
    let request = match tool {
        Tool::Entity(prototype) => PlacementRequest::Entity {
            prototype: prototype.clone(),
            map: map.0,
            position,
            rotation: f32::from(rotation) * std::f32::consts::FRAC_PI_2,
        },
        Tool::Tile(tile) => PlacementRequest::Tile {
            tile: tile.clone(),
            map: map.0,
            position,
        },
        // Точка — центр декали; сервер ждёт её угол.
        Tool::Decal(id) => PlacementRequest::Decal {
            id: id.clone(),
            map: map.0,
            corner: position - astrocore_math::Vec2::splat(0.5),
            rotation: f32::from(rotation) * std::f32::consts::FRAC_PI_2,
            color: None,
        },
        Tool::None | Tool::Erase => return,
    };
    world.send_net_event_to_server(&request);
}

#[derive(Component)]
struct GhostView;

/// Призрак размещаемого под курсором.
#[allow(clippy::too_many_arguments)]
fn ghost(
    mut commands: Commands,
    game: Res<Game>,
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
    let world = &game.0;
    let Some(current) = world.get_resource::<MappingTools>() else {
        return;
    };
    let point = cursor_point(world, &cursor, demo.as_deref());
    let (Some(point), Some(map), Some(resources)) = (point, player_map(world), resources) else {
        return;
    };
    let cell = snap_to_cell(world, map, point);
    let rotation = Quat::from_rotation_z(f32::from(current.rotation) * std::f32::consts::FRAC_PI_2);
    let image = match &current.tool {
        Tool::None => return,
        Tool::Erase => {
            commands.spawn((
                Sprite::from_color(ERASER, Vec2::ONE),
                Transform::from_xyz(cell.x, cell.y, 900.0),
                GhostView,
            ));
            return;
        }
        Tool::Entity(prototype) => {
            let at = if current.free { point } else { cell };
            commands.spawn((
                Sprite::from_color(OUTLINE, Vec2::ONE),
                Transform::from_xyz(cell.x, cell.y, 899.0),
                GhostView,
            ));
            prototype_image(
                world,
                prototype,
                &resources,
                &mut textures,
                &mut images,
                &mut layouts,
            )
            .map(|sprite| (sprite, at))
        }
        Tool::Tile(tile) => tile_image(world, tile, &resources, &mut textures, &mut images)
            .map(|sprite| (sprite, cell)),
        Tool::Decal(id) => {
            let at = if current.free { point } else { cell };
            crate::decal_visuals::decal_image(
                world,
                id,
                None,
                &resources,
                &mut textures,
                &mut images,
                &mut layouts,
            )
            .map(|sprite| (sprite, at))
        }
    };
    if let Some((mut sprite, at)) = image {
        sprite.color = GHOST;
        commands.spawn((
            sprite,
            Transform::from_xyz(at.x, at.y, 900.0).with_rotation(rotation),
            GhostView,
        ));
    }
}

/// Картинка прототипа: первый слой его спрайта.
fn prototype_image(
    world: &bevy_ecs::world::World,
    prototype: &str,
    resources: &GameResources,
    textures: &mut Textures,
    images: &mut Assets<Image>,
    layouts: &mut Assets<TextureAtlasLayout>,
) -> Option<Sprite> {
    let prototypes = world.get_resource::<Prototypes>()?;
    let parsed = prototypes
        .entity(prototype)?
        .component("Sprite")?
        .parsed
        .clone()?;
    let sprite = parsed.as_any().downcast_ref::<GameSprite>()?;
    let layer = sprite.resolved_layers().into_iter().next()?;
    match layer.source {
        LayerSource::Rsi { rsi, state } => {
            let texture = textures.state(&resources.0, images, layouts, &rsi, &state)?;
            let index = texture.state.sheet_index(0, 0);
            let mut sprite = Sprite::from_atlas_image(
                texture.image.clone(),
                TextureAtlas {
                    layout: texture.layout.clone(),
                    index,
                },
            );
            sprite.custom_size = Some(texture.size);
            Some(sprite)
        }
        LayerSource::Texture(path) => {
            let (image, size) = textures.single(&resources.0, images, &path)?;
            let mut sprite = Sprite::from_image(image);
            sprite.custom_size = Some(size);
            Some(sprite)
        }
    }
}

/// Картинка тайла: первый вариант.
fn tile_image(
    world: &bevy_ecs::world::World,
    tile: &str,
    resources: &GameResources,
    textures: &mut Textures,
    images: &mut Assets<Image>,
) -> Option<Sprite> {
    let definitions = world.get_resource::<TileDefinitions>()?;
    let path = definitions.by_id(tile)?.sprite.clone()?;
    let (image, size) = textures.single(&resources.0, images, path.trim_start_matches('/'))?;
    // Варианты тайла лежат в рисунке в ряд квадратами.
    let side = size.y * astrocore_client::PIXELS_PER_TILE;
    let mut sprite = Sprite::from_image(image);
    sprite.rect = Some(Rect::new(0.0, 0.0, side, side));
    sprite.custom_size = Some(Vec2::ONE);
    Some(sprite)
}

#[derive(Component)]
struct PalettePanel;

/// Окно палитры: поиск, кнопки инструментов, строки прототипов или тайлов.
fn palette(
    mut commands: Commands,
    game: Res<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<PalettePanel>>,
    mut catalog: Local<Option<Catalog>>,
    mut shown: Local<Option<u64>>,
) {
    let world = &game.0;
    let Some(current) = world.get_resource::<MappingTools>() else {
        return;
    };
    if *shown == Some(current.version) {
        return;
    }
    *shown = Some(current.version);
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    let Some(kind) = current.palette else {
        return;
    };
    let catalog = catalog.get_or_insert_with(|| Catalog {
        entities: world
            .get_resource::<Prototypes>()
            .map(spawnable)
            .unwrap_or_default(),
        tiles: world
            .get_resource::<TileDefinitions>()
            .map(|d| d.iter().map(|t| (t.id.clone(), String::new())).collect())
            .unwrap_or_default(),
        decals: world
            .get_resource::<Prototypes>()
            .map(decal_ids)
            .unwrap_or_default(),
    });
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(8.0),
                right: Val::Px(8.0),
                width: Val::Px(340.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(8.0)),
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(PANEL),
            Interaction::default(),
            GlobalZIndex(50),
            PalettePanel,
        ))
        .id();
    let title = match kind {
        Palette::Entities => "Сущности (F5)",
        Palette::Tiles => "Тайлы (F6)",
        Palette::Decals => "Декали (F8)",
    };
    commands.spawn((ChildOf(panel), style.text(title, 16.0)));
    let cursor = if current.typing { "_" } else { "" };
    spawn_button(
        &mut commands,
        panel,
        &style,
        &format!("Поиск: {}{cursor}", current.search),
        UiAction::new(|world| {
            let mut tools = tools_of(world);
            tools.typing = !tools.typing;
            tools.changed();
        }),
    );
    let row = commands
        .spawn((
            ChildOf(panel),
            Node {
                column_gap: Val::Px(6.0),
                flex_wrap: FlexWrap::Wrap,
                ..default()
            },
        ))
        .id();
    let erase = if current.tool == Tool::Erase {
        "Ластик: вкл"
    } else {
        "Ластик"
    };
    spawn_button(
        &mut commands,
        row,
        &style,
        erase,
        UiAction::new(|world| {
            let mut tools = tools_of(world);
            let next = if tools.tool == Tool::Erase {
                Tool::None
            } else {
                Tool::Erase
            };
            tools.select(next);
        }),
    );
    let snap = if current.free {
        "Свободно"
    } else {
        "По клеткам"
    };
    spawn_button(
        &mut commands,
        row,
        &style,
        snap,
        UiAction::new(|world| {
            let mut tools = tools_of(world);
            tools.free = !tools.free;
            tools.changed();
        }),
    );
    spawn_button(
        &mut commands,
        row,
        &style,
        &format!("Поворот {}°", u32::from(current.rotation) * 90),
        UiAction::new(|world| {
            let mut tools = tools_of(world);
            tools.rotation = (tools.rotation + 1) % 4;
            tools.changed();
        }),
    );
    let items = match kind {
        Palette::Entities => &catalog.entities,
        Palette::Tiles => &catalog.tiles,
        Palette::Decals => &catalog.decals,
    };
    for (id, name) in matching(items, &current.search) {
        let tool = match kind {
            Palette::Entities => Tool::Entity(id.clone()),
            Palette::Tiles => Tool::Tile(id.clone()),
            Palette::Decals => Tool::Decal(id.clone()),
        };
        let label = if name.is_empty() {
            id.clone()
        } else {
            format!("{name} ({id})")
        };
        let selected = current.tool == tool;
        let button = commands
            .spawn((
                ChildOf(panel),
                Interaction::default(),
                Node {
                    padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(if selected { SELECTED } else { ROW }),
                UiAction::new(move |world| tools_of(world).select(tool.clone())),
            ))
            .id();
        commands.spawn((ChildOf(button), style.text(label, 13.0)));
    }
}

/// Маппинг в игре.
pub struct MappingUiPlugin;

impl Plugin for MappingUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, (keys, pointer).after(bevy::input::InputSystems));
        app.add_systems(Update, (ghost, palette));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_lines_along_the_longer_axis_and_rectangles() {
        let line = fill_cells(Vec2::new(0.5, 0.5), Vec2::new(3.5, 1.5), false);
        assert_eq!(
            line,
            [0.5, 1.5, 2.5, 3.5].map(|x| Vec2::new(x, 0.5)).to_vec()
        );
        let down = fill_cells(Vec2::new(0.5, 0.5), Vec2::new(0.5, -1.5), false);
        assert_eq!(down, [0.5, -0.5, -1.5].map(|y| Vec2::new(0.5, y)).to_vec());
        let rect = fill_cells(Vec2::new(0.5, 0.5), Vec2::new(1.5, 2.5), true);
        assert_eq!(rect.len(), 6);
        let huge = fill_cells(Vec2::ZERO, Vec2::new(100.0, 100.0), true);
        assert_eq!(huge.len(), MAX_FILL);
    }

    #[test]
    fn search_matches_ids_and_names() {
        let items = vec![
            ("Crowbar".to_owned(), "crowbar".to_owned()),
            ("Wrench".to_owned(), "wrench".to_owned()),
            ("WallSolid".to_owned(), "solid wall".to_owned()),
        ];
        let ids = |search: &str| -> Vec<&str> {
            matching(&items, search)
                .into_iter()
                .map(|(id, _)| id.as_str())
                .collect()
        };
        assert_eq!(ids("w"), ["Crowbar", "Wrench", "WallSolid"]);
        assert_eq!(ids("WALL"), ["WallSolid"]);
        assert_eq!(ids("solid"), ["WallSolid"]);
    }
}
