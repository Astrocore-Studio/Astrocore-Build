use std::path::PathBuf;
use std::process::ExitCode;

use astrocore_build_2d::editor::{Brush, Document};
use astrocore_build_2d::view::MapView;
use astrocore_grid2d::Tile;
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
        Text2d::new("1 Floor  2 Wall  3 Space  4 Spawn  5 Cable  6 Object  Tab Next"),
        TextFont::from_font_size(15.0),
        TextColor(Color::srgb_u8(157, 182, 195)),
        Transform::from_xyz(0.0, -249.0, 3.0),
    ));
    commands.spawn((
        Text2d::new("Left Paint  Right Erase  Z Undo  Y Redo  F2 Save"),
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
    let size = MapView::new(editor.document.grid()).scale;
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
    cursor: Single<(&mut Transform, &mut Visibility), With<Cursor>>,
    mut editor: ResMut<Editor>,
) {
    for (key, brush) in [
        (KeyCode::Digit1, Brush::Tile(Tile::Floor)),
        (KeyCode::Digit2, Brush::Tile(Tile::Wall)),
        (KeyCode::Digit3, Brush::Tile(Tile::Void)),
        (KeyCode::Digit4, Brush::Spawn),
        (KeyCode::Digit5, Brush::Cable),
    ] {
        if keys.just_pressed(key) {
            editor.brush = brush;
        }
    }
    if keys.just_pressed(KeyCode::Tab) && !editor.prototypes.is_empty() {
        editor.selected = (editor.selected + 1) % editor.prototypes.len();
        editor.brush = Brush::Object(editor.prototypes[editor.selected].clone());
    }
    if keys.just_pressed(KeyCode::Digit6)
        && let Some(prototype) = editor.prototypes.get(editor.selected)
    {
        editor.brush = Brush::Object(prototype.clone());
    }
    let view = MapView::new(editor.document.grid());
    let cell = window
        .cursor_position()
        .and_then(|point| camera.0.viewport_to_world_2d(camera.1, point).ok())
        .map(|point| view.cell(point))
        .filter(|&cell| editor.document.grid().contains(cell));
    let (mut cursor_transform, mut cursor_visibility) = cursor.into_inner();
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
        editor.document.paint(cell, &brush, erase)
    } else {
        false
    };
    if changed {
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
    let line = format!("{label}    /    {message}");
    if status.0 != line {
        status.0 = line;
    }
    if !editor.redraw {
        return;
    }
    editor.redraw = false;
    let document = &editor.document;
    let view = MapView::new(document.grid());
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
    for (cell, tile) in document.grid().cells() {
        let sprite = view.tile_sprite(cell, tile).unwrap_or_else(|| {
            Sprite::from_color(Color::srgb_u8(16, 26, 40), Vec2::splat(view.scale - 1.0))
        });
        shape(cell, sprite, 0.0);
    }
    for &cell in document.cables() {
        shape(
            cell,
            Sprite::from_color(
                Color::srgb_u8(114, 189, 208),
                Vec2::new(0.75, 0.09) * view.scale,
            ),
            0.2,
        );
    }
    for &cell in document.spawns() {
        shape(
            cell,
            Sprite::from_color(Color::srgb_u8(91, 224, 151), Vec2::splat(view.scale * 0.27)),
            0.3,
        );
    }
    for (cell, prototype) in document.objects() {
        let (r, g, b) = prototype.color;
        shape(
            cell,
            Sprite::from_color(Color::srgb_u8(r, g, b), Vec2::new(0.50, 0.55) * view.scale),
            0.5,
        );
    }
}
