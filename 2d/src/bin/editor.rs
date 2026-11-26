use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;

use astrocore_build_2d::editor::{Brush, Document};
use astrocore_build_2d::view::MapView;
use astrocore_grid2d::{Cell, Tile};
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowResolution};

#[derive(Resource)]
struct Editor {
    document: Document,
    brush: Brush,
    prototypes: Vec<String>,
    selected: usize,
    feedback: String,
    redraw: bool,
    level: i16,
    link_start: Option<Cell>,
}

#[derive(Component)]
struct MapShape;

#[derive(Component)]
struct Cursor;

#[derive(Component)]
struct Status;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let directory = args
        .next()
        .map(PathBuf::from)
        .ok_or("astrocore-2d-editor <каталог контента>")?;
    if args.next().is_some() {
        return Err("astrocore-2d-editor <каталог контента>".into());
    }
    let document = Document::open(&directory)?;
    let prototypes = document.prototypes().keys().cloned().collect();
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Astrocore Map Editor".into(),
            resolution: WindowResolution::new(1040, 620),
            resizable: false,
            ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(Color::srgb_u8(9, 16, 26)))
    .insert_resource(Editor {
        document,
        brush: Brush::Tile(Tile::Floor),
        prototypes,
        selected: 0,
        feedback: String::new(),
        redraw: true,
        level: 0,
        link_start: None,
    })
    .add_systems(Startup, setup)
    .add_systems(Update, (input, draw).chain());
    match app.run() {
        AppExit::Success => Ok(()),
        AppExit::Error(code) => Err(format!("редактор завершился с кодом {code}")),
    }
}

fn setup(mut commands: Commands, editor: Res<Editor>) {
    commands.spawn(Camera2d);
    commands.spawn((
        Text2d::new(format!("MAP EDITOR  /  {}", editor.document.name())),
        TextFont::from_font_size(24.0),
        TextColor(Color::srgb_u8(217, 235, 245)),
        Transform::from_xyz(0.0, 270.0, 3.0),
    ));
    commands.spawn((
        Text2d::new("1 Floor  2 Wall  3 Space  4 Spawn  5 Cable  6 Object  7 Link  Tab Next"),
        TextFont::from_font_size(15.0),
        TextColor(Color::srgb_u8(157, 182, 195)),
        Transform::from_xyz(0.0, -249.0, 3.0),
    ));
    commands.spawn((
        Text2d::new("PgUp/PgDn Deck  Shift+PgUp/PgDn Add  Z Undo  Y Redo  F2 Save"),
        TextFont::from_font_size(15.0),
        TextColor(Color::srgb_u8(157, 182, 195)),
        Transform::from_xyz(0.0, -268.0, 3.0),
    ));
    commands.spawn((
        Status,
        Text2d::new(""),
        TextFont::from_font_size(15.0),
        TextColor(Color::srgb_u8(112, 210, 202)),
        Transform::from_xyz(0.0, -291.0, 3.0),
    ));
    let size = MapView::new(editor.document.grid(), editor.level).scale;
    commands.spawn((
        Cursor,
        Sprite::from_color(Color::srgba(0.36, 0.88, 0.77, 0.30), Vec2::splat(size)),
        Transform::from_xyz(0.0, 0.0, 2.0),
        Visibility::Hidden,
    ));
}

fn input(
    keys: Res<ButtonInput<KeyCode>>,
    clicks: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    cursor: Single<(&mut Transform, &mut Visibility, &mut Sprite), With<Cursor>>,
    mut editor: ResMut<Editor>,
) {
    for (key, brush) in [
        (KeyCode::Digit1, Brush::Tile(Tile::Floor)),
        (KeyCode::Digit2, Brush::Tile(Tile::Wall)),
        (KeyCode::Digit3, Brush::Tile(Tile::Void)),
        (KeyCode::Digit4, Brush::Spawn),
        (KeyCode::Digit5, Brush::Cable),
        (KeyCode::Digit7, Brush::Link),
    ] {
        if keys.just_pressed(key) {
            editor.brush = brush;
            editor.link_start = None;
            editor.feedback.clear();
        }
    }
    if keys.just_pressed(KeyCode::Tab) && !editor.prototypes.is_empty() {
        editor.selected = (editor.selected + 1) % editor.prototypes.len();
        editor.brush = Brush::Object(editor.prototypes[editor.selected].clone());
        editor.link_start = None;
    }
    if keys.just_pressed(KeyCode::Digit6)
        && let Some(prototype) = editor.prototypes.get(editor.selected)
    {
        editor.brush = Brush::Object(prototype.clone());
        editor.link_start = None;
    }
    if keys.just_pressed(KeyCode::Escape) {
        editor.link_start = None;
        editor.feedback.clear();
    }
    change_level(&mut editor, &keys);
    let view = MapView::new(editor.document.grid(), editor.level);
    let cell = window
        .cursor_position()
        .and_then(|point| camera.0.viewport_to_world_2d(camera.1, point).ok())
        .map(|point| view.cell(point))
        .filter(|&cell| editor.document.grid().contains(cell));
    let (mut cursor_transform, mut cursor_visibility, mut cursor_sprite) = cursor.into_inner();
    cursor_sprite.custom_size = Some(Vec2::splat(view.scale));
    if let Some(cell) = cell {
        cursor_transform.translation = view.world(cell.center()).extend(2.0);
        *cursor_visibility = Visibility::Visible;
    } else {
        *cursor_visibility = Visibility::Hidden;
    }
    let brush = editor.brush.clone();
    let erase = clicks.just_pressed(MouseButton::Right);
    let changed = if keys.just_pressed(KeyCode::KeyZ) {
        editor.document.undo()
    } else if keys.just_pressed(KeyCode::KeyY) {
        editor.document.redo()
    } else if (erase || clicks.just_pressed(MouseButton::Left))
        && let Some(cell) = cell
    {
        if matches!(brush, Brush::Link) && !erase {
            select_link_endpoint(&mut editor, cell)
        } else {
            editor.link_start = None;
            editor.document.paint(cell, &brush, erase)
        }
    } else {
        false
    };
    if changed {
        if editor.document.grid().size_at(editor.level).is_none() {
            editor.level = 0;
        }
        editor.feedback.clear();
        editor.redraw = true;
    }
    if keys.just_pressed(KeyCode::F2) {
        editor.feedback = match editor.document.save() {
            Ok(()) => "Saved".into(),
            Err(error) => {
                eprintln!("Карта не сохранена: {error}");
                "Validation failed. See terminal.".into()
            }
        };
    }
}

fn change_level(editor: &mut Editor, keys: &ButtonInput<KeyCode>) {
    let up = keys.just_pressed(KeyCode::PageUp);
    if !up && !keys.just_pressed(KeyCode::PageDown) {
        return;
    }
    let level = editor.level;
    let shifted = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let next = if shifted {
        let next = level.checked_add(if up { 1 } else { -1 });
        if let Some(z) = next {
            editor.document.add_level(z);
        }
        next
    } else if up {
        editor.document.grid().levels().find(|&z| z > level)
    } else {
        editor.document.grid().levels().rev().find(|&z| z < level)
    };
    if let Some(z) = next {
        editor.level = z;
        editor.redraw = true;
    }
}

fn select_link_endpoint(editor: &mut Editor, cell: Cell) -> bool {
    if editor.document.grid().tile(cell) != Tile::Floor {
        editor.feedback = "Choose a floor tile".into();
        return false;
    }
    let Some(first) = editor.link_start else {
        editor.link_start = Some(cell);
        editor.feedback = "Choose a tile on the adjacent deck".into();
        return false;
    };
    let linked = editor.document.connect_levels(first, cell);
    if linked {
        editor.link_start = None;
    } else {
        editor.feedback = "Use an available tile on the adjacent deck".into();
    }
    linked
}

fn draw(
    mut commands: Commands,
    mut editor: ResMut<Editor>,
    shapes: Query<Entity, With<MapShape>>,
    mut status: Single<&mut Text2d, With<Status>>,
) {
    let label = match &editor.brush {
        Brush::Tile(Tile::Floor) => "Floor",
        Brush::Tile(Tile::Wall) => "Wall",
        Brush::Tile(Tile::Void) => "Space",
        Brush::Spawn => "Spawn point",
        Brush::Cable => "Power cable",
        Brush::Link => "Deck link",
        Brush::Object(id) => editor.document.prototypes()[id].name.as_str(),
    };
    let message = if editor.feedback.is_empty() {
        if editor.document.dirty() {
            "Unsaved"
        } else {
            "Ready"
        }
    } else {
        &editor.feedback
    };
    let line = format!("Z {}    /    {label}    /    {message}", editor.level);
    if status.0 != line {
        status.0 = line;
    }
    if !editor.redraw {
        return;
    }
    editor.redraw = false;
    let level = editor.level;
    let document = &editor.document;
    let view = MapView::new(document.grid(), level);
    for shape in &shapes {
        commands.entity(shape).despawn();
    }
    let mut shape = |cell: astrocore_grid2d::Cell, sprite: Sprite, z: f32| {
        commands.spawn((
            MapShape,
            sprite,
            Transform::from_translation(view.world(cell.center()).extend(z)),
        ));
    };
    for (cell, tile) in document.grid().cells_at(level) {
        let sprite = view.tile_sprite(cell, tile).unwrap_or_else(|| {
            Sprite::from_color(Color::srgb_u8(16, 26, 40), Vec2::splat(view.scale - 1.0))
        });
        shape(cell, sprite, 0.0);
    }
    for &cell in document.cables().iter().filter(|cell| cell.z == level) {
        shape(cell, view.cable_sprite(), 0.2);
    }
    for &cell in document.spawns().iter().filter(|cell| cell.z == level) {
        shape(
            cell,
            Sprite::from_color(Color::srgb_u8(91, 224, 151), Vec2::splat(view.scale * 0.27)),
            0.3,
        );
    }
    for (cell, prototype) in document.objects().filter(|(cell, _)| cell.z == level) {
        let (r, g, b) = prototype.color;
        shape(
            cell,
            Sprite::from_color(Color::srgb_u8(r, g, b), Vec2::new(0.50, 0.55) * view.scale),
            0.5,
        );
    }
    let markers: BTreeSet<_> = document
        .link_cells()
        .filter(|cell| cell.z == level)
        .collect();
    for cell in markers {
        commands.spawn((MapShape, view.level_marker(cell)));
    }
}
