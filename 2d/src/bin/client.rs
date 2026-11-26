use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use astrocore_build_2d::server::LocalServer;
use astrocore_build_2d::view::MapView;
use astrocore_build_2d::{
    Action, Kind, Layout, Outcome, Phase, State, bind_address, configured_key, endpoint,
};
use astrocore_grid2d::session::{Client, PlayerId, TICK_PERIOD};
use astrocore_grid2d::{Cell, Tile};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::{PrimaryWindow, WindowResolution};

type Network = Client<Layout, State, Action>;
const HAND_LABEL_Y: f32 = -282.0;
const HAND_LABELS_X: [f32; 2] = [-348.0, 348.0];

#[derive(Resource, Default)]
struct Visuals {
    avatars: BTreeMap<PlayerId, Entity>,
    initialized: bool,
    terrain_revision: Option<u32>,
    terrain: BTreeMap<Cell, (Entity, Tile)>,
    doors: BTreeMap<Cell, (Entity, bool, bool)>,
    generators: BTreeMap<Cell, (Entity, bool)>,
    items: BTreeMap<u16, (Entity, Cell)>,
}

#[derive(Resource)]
struct Capture(PathBuf);

#[derive(Component)]
struct MapVisual;

#[derive(Component)]
struct Avatar;

#[derive(Component)]
enum Label {
    Title,
    Help,
    Status,
    Objective,
    Hand(u8),
}

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
    const USAGE: &str = "astrocore-2d-client [--host] [--screenshot путь.png]";
    let mut args = std::env::args_os().skip(1);
    let mut host = false;
    let mut capture = None;
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--host") if !host => host = true,
            Some("--screenshot") if capture.is_none() => {
                capture = Some(PathBuf::from(args.next().ok_or("укажите путь к PNG")?));
            }
            Some("--help" | "-h") => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => return Err(USAGE.into()),
        }
    }

    let mut id = [0; 8];
    getrandom::getrandom(&mut id).map_err(|error| error.to_string())?;
    let mut local_server = None;
    let (address, key) = if host {
        let key = match configured_key()? {
            Some(key) => key,
            None => {
                let mut key = [0; 32];
                getrandom::getrandom(&mut key).map_err(|error| error.to_string())?;
                key
            }
        };
        let server = LocalServer::launch(bind_address(0)?, key)?;
        let address = server.address;
        local_server = Some(server);
        (address, key)
    } else {
        endpoint()?
    };

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Astrocore 2D".into(),
            resolution: WindowResolution::new(1040, 620),
            resizable: false,
            ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(Color::srgb_u8(9, 16, 26)))
    .insert_resource(Time::<Fixed>::from_duration(TICK_PERIOD))
    .insert_non_send(Network::connect(address, u64::from_le_bytes(id), key)?)
    .init_resource::<Visuals>()
    .add_systems(Startup, setup)
    .add_systems(FixedUpdate, send_movement)
    .add_systems(Update, (receive, draw).chain());
    if let Some(path) = capture {
        app.insert_resource(Capture(path))
            .add_systems(Update, capture_frame.after(draw));
    }
    let result = app.run();
    drop(local_server);
    match result {
        AppExit::Success => Ok(()),
        AppExit::Error(code) => Err(format!("графическое приложение завершилось с кодом {code}")),
    }
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        Label::Title,
        Text2d::new("ASTROCORE"),
        TextFont::from_font_size(30.0),
        TextColor(Color::srgb_u8(217, 235, 245)),
        Transform::from_xyz(0.0, 270.0, 2.0),
    ));
    commands.spawn((
        Label::Objective,
        Text2d::new(""),
        TextFont::from_font_size(14.0),
        TextColor(Color::srgb_u8(162, 197, 201)),
        Transform::from_xyz(0.0, 244.0, 2.0),
    ));
    commands.spawn((
        Label::Help,
        Text2d::new(""),
        TextFont::from_font_size(17.0),
        TextColor(Color::srgb_u8(157, 182, 195)),
        Transform::from_xyz(0.0, -254.0, 2.0),
    ));
    commands.spawn((
        Label::Status,
        Text2d::new("Connecting"),
        TextFont::from_font_size(16.0),
        TextColor(Color::srgb_u8(112, 210, 202)),
        Transform::from_xyz(0.0, -282.0, 2.0),
    ));
    for (hand, x) in HAND_LABELS_X.into_iter().enumerate() {
        commands.spawn((
            Label::Hand(hand as u8),
            Text2d::new(""),
            TextFont::from_font_size(15.0),
            TextColor(Color::srgb_u8(212, 228, 229)),
            Transform::from_xyz(x, HAND_LABEL_Y, 2.0),
        ));
    }
}

fn send_movement(keys: Res<ButtonInput<KeyCode>>, mut network: NonSendMut<Network>) {
    let pressed = |left, right| i8::from(keys.pressed(left) || keys.pressed(right));
    network.move_player([
        pressed(KeyCode::KeyD, KeyCode::ArrowRight) - pressed(KeyCode::KeyA, KeyCode::ArrowLeft),
        pressed(KeyCode::KeyW, KeyCode::ArrowUp) - pressed(KeyCode::KeyS, KeyCode::ArrowDown),
    ]);
}

fn receive(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    mut network: NonSendMut<Network>,
) {
    network.step();
    let keys_to_actions = [
        (KeyCode::Enter, Action::Ready),
        (KeyCode::KeyE, Action::Interact),
        (KeyCode::KeyQ, Action::Drop),
        (KeyCode::KeyX, Action::SwapHand),
        (KeyCode::KeyF, Action::Use),
        (KeyCode::Digit1, Action::SelectRole(0)),
        (KeyCode::Digit2, Action::SelectRole(1)),
        (KeyCode::Digit3, Action::SelectRole(2)),
        (KeyCode::Digit4, Action::SelectRole(3)),
        (KeyCode::Digit5, Action::SelectRole(4)),
        (KeyCode::Digit6, Action::SelectRole(5)),
        (KeyCode::Digit7, Action::SelectRole(6)),
        (KeyCode::Digit8, Action::SelectRole(7)),
        (KeyCode::Digit9, Action::SelectRole(8)),
    ];
    if let Some((_, action)) = keys_to_actions
        .into_iter()
        .find(|(key, _)| keys.just_pressed(*key))
    {
        network.act(action);
        return;
    }
    let brush = [
        (KeyCode::KeyB, Tile::Wall),
        (KeyCode::KeyN, Tile::Floor),
        (KeyCode::KeyV, Tile::Void),
    ];
    if let Some(cursor) = window.cursor_position()
        && let Some(session) = network.session()
        && let Ok(point) = camera.0.viewport_to_world_2d(camera.1, cursor)
    {
        if session.state.phase == Phase::Active
            && buttons.just_pressed(MouseButton::Left)
            && (point.y - HAND_LABEL_Y).abs() <= 16.0
            && let Some(hand) = HAND_LABELS_X
                .iter()
                .position(|x| (point.x - x).abs() <= 125.0)
        {
            network.act(Action::ClickHand(hand as u8));
            return;
        }
        let cell = MapView::new(&session.layout.grid).cell(point);
        if keys.just_pressed(KeyCode::KeyG) {
            network.act(Action::Treat { cell });
        } else if let Some((_, tile)) = brush.into_iter().find(|(key, _)| keys.just_pressed(*key)) {
            network.act(Action::Construct { cell, tile });
        }
    }
}

fn door_sprite(open: bool, powered: bool, color: Color, scale: f32) -> Sprite {
    let (color, size) = if open {
        (Color::srgb_u8(88, 210, 179), Vec2::new(0.86, 0.10))
    } else {
        let color = if powered {
            color
        } else {
            Color::srgb_u8(88, 94, 108)
        };
        (color, Vec2::new(0.35, 0.90))
    };
    Sprite::from_color(color, size * scale)
}

fn draw(
    mut commands: Commands,
    network: NonSend<Network>,
    mut visuals: ResMut<Visuals>,
    map_entities: Query<Entity, With<MapVisual>>,
    mut avatars: Query<(&mut Transform, &mut Sprite), With<Avatar>>,
    mut labels: Query<(&Label, &mut Text2d)>,
) {
    let Some(session) = network.session() else {
        if visuals.initialized {
            for entity in &map_entities {
                commands.entity(entity).despawn();
            }
            *visuals = Visuals::default();
            for (label, mut text) in &mut labels {
                text.0 = match label {
                    Label::Title => "ASTROCORE".into(),
                    Label::Help => String::new(),
                    Label::Status => "Connection lost".into(),
                    Label::Hand(_) | Label::Objective => String::new(),
                };
            }
        }
        return;
    };
    let layout = &session.layout;
    let state = &session.state;
    let view = MapView::new(&layout.grid);
    let scale = view.scale;
    if !visuals.initialized {
        visuals.initialized = true;
        println!("Astrocore 2D: connected");
    }
    if visuals.terrain_revision != Some(state.terrain_revision) {
        for (cell, base) in layout.grid.cells() {
            let tile = state.terrain.get(&cell).copied().unwrap_or(base);
            if let Some((_, previous)) = visuals.terrain.get(&cell)
                && *previous == tile
            {
                continue;
            }
            if let Some((entity, _)) = visuals.terrain.remove(&cell) {
                commands.entity(entity).despawn();
            }
            if let Some(sprite) = view.tile_sprite(cell, tile) {
                let entity = commands
                    .spawn((
                        MapVisual,
                        sprite,
                        Transform::from_translation(view.world(cell.center()).extend(0.0)),
                    ))
                    .id();
                visuals.terrain.insert(cell, (entity, tile));
            }
        }
        visuals.terrain_revision = Some(state.terrain_revision);
    }
    for (&cell, door) in &state.doors {
        let Some(prototype) = layout.prototypes.get(&door.prototype) else {
            continue;
        };
        let (r, g, b) = prototype.color;
        let sprite = || door_sprite(door.open, door.powered, Color::srgb_u8(r, g, b), scale);
        let (entity, open, powered) = visuals.doors.entry(cell).or_insert_with(|| {
            let entity = commands
                .spawn((
                    MapVisual,
                    sprite(),
                    Transform::from_translation(view.world(cell.center()).extend(0.5)),
                ))
                .id();
            (entity, door.open, door.powered)
        });
        if *open != door.open || *powered != door.powered {
            commands.entity(*entity).insert(sprite());
            *open = door.open;
            *powered = door.powered;
        }
    }
    let equipment =
        state
            .generators
            .iter()
            .map(|(&cell, device)| (cell, &device.prototype, device.enabled))
            .chain(state.vents.iter().map(|(&cell, device)| {
                (cell, &device.prototype, device.enabled && device.powered)
            }));
    for (cell, prototype, enabled) in equipment {
        let Some(prototype) = layout.prototypes.get(prototype) else {
            continue;
        };
        let (r, g, b) = prototype.color;
        let tint = if enabled {
            Color::srgb_u8(r, g, b)
        } else {
            Color::srgb_u8(85, 85, 92)
        };
        let sprite = || Sprite::from_color(tint, Vec2::new(0.61, 0.50) * scale);
        let (entity, previous) = visuals.generators.entry(cell).or_insert_with(|| {
            let entity = commands
                .spawn((
                    MapVisual,
                    sprite(),
                    Transform::from_translation(view.world(cell.center()).extend(0.58)),
                ))
                .id();
            (entity, enabled)
        });
        if *previous != enabled {
            commands.entity(*entity).insert(sprite());
            *previous = enabled;
        }
    }

    visuals.items.retain(|id, (entity, _)| {
        if state
            .items
            .get(id)
            .is_some_and(|item| item.ground.is_some())
        {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
    for (&id, item) in &state.items {
        let Some(cell) = item.ground else { continue };
        let Some(prototype) = layout.prototypes.get(&item.prototype) else {
            continue;
        };
        let (r, g, b) = prototype.color;
        let location = Transform::from_translation(view.world(cell.center()).extend(0.6));
        let (entity, previous_cell) = visuals.items.entry(id).or_insert_with(|| {
            let entity = commands
                .spawn((
                    MapVisual,
                    Sprite::from_color(Color::srgb_u8(r, g, b), Vec2::new(0.43, 0.33) * scale),
                    location,
                ))
                .id();
            (entity, cell)
        });
        if *previous_cell != cell {
            commands.entity(*entity).insert(location);
            *previous_cell = cell;
        }
    }

    visuals.avatars.retain(|player, entity| {
        if state.players.contains_key(player) {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
    for (&player, crew) in &state.players {
        let location = view.world(crew.position).extend(1.0);
        let color = if crew.health == 0 {
            Color::srgb_u8(115, 93, 109)
        } else if player == session.player {
            Color::srgb_u8(115, 230, 209)
        } else {
            Color::srgb_u8(175, 155, 245)
        };
        let entity = *visuals.avatars.entry(player).or_insert_with(|| {
            commands
                .spawn((
                    MapVisual,
                    Avatar,
                    Transform::from_translation(location),
                    Sprite::from_color(color, Vec2::new(0.5, 0.52) * scale),
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Sprite::from_color(
                            Color::srgb_u8(21, 32, 49),
                            Vec2::new(0.36, 0.14) * scale,
                        ),
                        Transform::from_xyz(0.0, 0.07 * scale, 0.1),
                    ));
                })
                .id()
        });
        if let Ok((mut transform, mut sprite)) = avatars.get_mut(entity) {
            transform.translation = location;
            sprite.color = color;
        }
    }

    let Some(crew) = state.players.get(&session.player) else {
        return;
    };
    let role = layout
        .roles
        .get(usize::from(crew.role))
        .map_or("", String::as_str);
    let (help, status) = match state.phase {
        Phase::Lobby => {
            let help = layout
                .roles
                .iter()
                .enumerate()
                .map(|(i, name)| format!("{} {name}", i + 1))
                .collect::<Vec<_>>()
                .join("   ");
            let ready_count = state.players.values().filter(|player| player.ready).count();
            let readiness = if crew.ready {
                "Ready"
            } else {
                "Press Enter to ready"
            };
            (
                help,
                format!(
                    "Round {}  /  {role}  /  {ready_count}/{} ready  /  {readiness}",
                    state.round,
                    state.players.len()
                ),
            )
        }
        Phase::Active => {
            let hand = usize::from(crew.active_hand);
            let item = crew.hands[hand].and_then(|id| state.items.get(&id));
            let prototype = item.and_then(|item| layout.prototypes.get(&item.prototype));
            let construction = prototype.is_some_and(|prototype| {
                matches!(prototype.kind, Kind::Item { materials: 1.., .. })
            });
            let help = if construction {
                "Aim mouse   B Wall   N Floor   V Dismantle   X Swap hand   Q Drop".into()
            } else if let Some(scan) = crew.scan {
                format!(
                    "Scan {} kPa   /   O2 {}%   /   {} K",
                    scan.pressure_kpa, scan.oxygen_percent, scan.temperature_k
                )
            } else {
                "WASD Move   E Interact   X Swap hand   Q Drop   F Use   G Treat".into()
            };
            let role: String = role.chars().take(12).collect();
            (
                help,
                format!(
                    "R{} / {role} / {:02}:{:02} / HP {} / {} kPa",
                    state.round,
                    state.seconds_left / 60,
                    state.seconds_left % 60,
                    crew.health,
                    crew.pressure_kpa
                ),
            )
        }
        Phase::Debrief => (
            match state.outcome {
                Some(Outcome::Restored) => "Compartment restored",
                Some(Outcome::TimeExpired) if state.recovery.is_some() => {
                    "Recovery failed: time expired"
                }
                Some(Outcome::CrewLost) => "Crew lost",
                _ => "Shift complete",
            }
            .into(),
            format!(
                "Round {} ended  /  Next lobby in {}s",
                state.round, state.seconds_left
            ),
        ),
    };
    let objective = state.recovery.as_ref().map_or_else(String::new, |goal| {
        if state.phase == Phase::Lobby {
            return format!(
                "Seal the hull at {},{}   /   Activate the air reserve at {},{}",
                goal.breach.x, goal.breach.y, goal.vent.x, goal.vent.y
            );
        }
        let hull = if goal.sealed { "sealed" } else { "open" };
        let (vent, reserve) = state.vents.get(&goal.vent).map_or(("unavailable", 0.0), |device| {
            let status = if !device.powered { "no power" } else if !device.enabled { "off" } else if device.reserve_moles <= 0.0 { "empty" } else { "online" };
            (status, device.reserve_moles)
        });
        format!(
            "Hull {hull}  /  Air reserve {vent} ({reserve:.0} mol)  /  {} / {} kPa  /  Stable {} / {}s",
            goal.pressure_kpa, goal.target_kpa, goal.stable_seconds, goal.required_seconds
        )
    });
    for (label, mut text) in &mut labels {
        let name;
        let next = match *label {
            Label::Title => &layout.name,
            Label::Help => &help,
            Label::Status => &status,
            Label::Objective => &objective,
            Label::Hand(hand) if state.phase == Phase::Active => {
                let item = crew.hands[usize::from(hand)].and_then(|id| state.items.get(&id));
                let prototype = item.and_then(|item| layout.prototypes.get(&item.prototype));
                let hand_label = if hand == 0 { "L" } else { "R" };
                let selected = if hand == u8::from(crew.active_hand) {
                    ">"
                } else {
                    " "
                };
                let item_name: String = prototype
                    .map_or("Empty", |item| item.name.as_str())
                    .chars()
                    .take(18)
                    .collect();
                let stock = if prototype
                    .is_some_and(|item| matches!(item.kind, Kind::Item { materials: 1.., .. }))
                {
                    item.map_or_else(String::new, |item| format!(" ({})", item.materials))
                } else {
                    String::new()
                };
                name = format!("{selected}{hand_label}  {item_name}{stock}");
                &name
            }
            Label::Hand(_) => {
                text.0.clear();
                continue;
            }
        };
        if text.0 != *next {
            text.0.clone_from(next);
        }
    }
}

fn capture_frame(
    mut commands: Commands,
    capture: Res<Capture>,
    visuals: Res<Visuals>,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
) {
    if visuals.avatars.is_empty() {
        return;
    }
    *frames += 1;
    if *frames == 30 {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(capture.0.clone()));
    }
    if *frames > 40 && capture.0.is_file() {
        exit.write(AppExit::Success);
    }
}
