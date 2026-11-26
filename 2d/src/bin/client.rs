use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::ExitCode;

use astrocore_build_2d::inventory_view::{self, InventoryBackdrop, InventoryLine, InventoryPanel};
use astrocore_build_2d::server::LocalServer;
use astrocore_build_2d::view::MapView;
use astrocore_build_2d::{
    Action, Crew, Kind, Layout, MobState, Outcome, Participation, Phase, State, WIRE_VERSION,
    bind_address, configured_key, endpoint,
};
use astrocore_grid2d::session::{Client, PlayerId, TICK_HZ, TICK_PERIOD};
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
    level: Option<i16>,
    terrain_revision: Option<u32>,
    terrain: BTreeMap<Cell, (Entity, Tile)>,
    cables: BTreeMap<Cell, Entity>,
    pipe_valves: BTreeMap<Cell, (Entity, bool)>,
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
    .insert_non_send(Network::connect(
        address,
        u64::from_le_bytes(id),
        key,
        WIRE_VERSION,
    )?)
    .init_resource::<Visuals>()
    .init_resource::<InventoryPanel>()
    .add_systems(Startup, setup)
    .add_systems(FixedUpdate, send_movement)
    .add_systems(Update, (receive, draw, draw_inventory).chain());
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
    inventory_view::spawn(&mut commands);
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
    if network.session().is_none_or(|session| {
        session.state.phase != Phase::Active
            || !session
                .state
                .players
                .get(&session.player)
                .is_some_and(Crew::on_duty)
    }) {
        return;
    }
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
    mut inventory: ResMut<InventoryPanel>,
) {
    network.step();
    if network.session().is_none_or(|session| {
        session.state.phase != Phase::Active
            || !session
                .state
                .players
                .get(&session.player)
                .is_some_and(Crew::on_duty)
    }) {
        inventory.open = false;
    } else if keys.just_pressed(KeyCode::KeyI) {
        inventory.open = !inventory.open;
        return;
    }
    let keys_to_actions = [
        (KeyCode::Enter, Action::Ready),
        (KeyCode::KeyE, Action::Interact),
        (KeyCode::KeyQ, Action::Drop),
        (KeyCode::KeyX, Action::SwapHand),
        (KeyCode::KeyF, Action::Use),
        (KeyCode::KeyO, Action::Observe),
        (KeyCode::PageUp, Action::ChangeLevel { up: true }),
        (KeyCode::PageDown, Action::ChangeLevel { up: false }),
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
    if keys.just_pressed(KeyCode::KeyT)
        && network.session().is_some_and(|session| {
            session
                .state
                .players
                .get(&session.player)
                .is_some_and(|crew| crew.pulling.is_some())
        })
    {
        network.act(Action::ReleasePull);
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
        if inventory.contains(point) {
            if buttons.just_pressed(MouseButton::Left)
                && let Some(crew) = session.state.players.get(&session.player)
                && let Some(action) = inventory.action(point, &session.state, crew)
            {
                network.act(action);
            }
            return;
        }
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
        let Some(crew) = session.state.players.get(&session.player) else {
            return;
        };
        let cell = MapView::new(&session.layout.grid, crew.position.z).cell(point);
        if keys.just_pressed(KeyCode::KeyG) {
            network.act(Action::Treat { cell });
        } else if keys.just_pressed(KeyCode::KeyH) {
            network.act(Action::Give { cell });
        } else if keys.just_pressed(KeyCode::KeyT) {
            network.act(Action::Pull { cell });
        } else if keys.just_pressed(KeyCode::KeyC) {
            network.act(Action::LayCable { cell });
        } else if keys.just_pressed(KeyCode::KeyR) {
            network.act(Action::CutCable { cell });
        } else if keys.just_pressed(KeyCode::KeyP) {
            network.act(Action::ScanPipe { cell });
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
    let Some(crew) = state.players.get(&session.player) else {
        return;
    };
    let level = crew.position.z;
    let held = crew.hands[usize::from(crew.active_hand)]
        .and_then(|id| state.items.get(&id))
        .and_then(|item| layout.prototypes.get(&item.prototype))
        .map(|prototype| prototype.kind);
    let view = MapView::new(&layout.grid, level);
    let scale = view.scale;
    if !visuals.initialized {
        visuals.initialized = true;
        println!("Astrocore 2D: connected");
    }
    if visuals.level != Some(level) {
        for entity in &map_entities {
            commands.entity(entity).despawn();
        }
        *visuals = Visuals {
            initialized: true,
            level: Some(level),
            ..default()
        };
        let markers: BTreeSet<_> = layout
            .links
            .iter()
            .flat_map(|link| [link.lower, link.upper])
            .filter(|cell| cell.z == level)
            .collect();
        for cell in markers {
            commands.spawn((MapVisual, view.level_marker(cell)));
        }
        for (&cell, piece) in layout.pipes.iter().filter(|(cell, _)| cell.z == level) {
            let point = view.world(cell.center());
            for (offset, sprite) in view.pipe_segments(piece.shape, piece.facing) {
                commands.spawn((
                    MapVisual,
                    sprite,
                    Transform::from_translation((point + offset).extend(0.24)),
                ));
            }
        }
    }
    if visuals.terrain_revision != Some(state.terrain_revision) {
        for (cell, base) in layout.grid.cells_at(level) {
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
    let show_cables = matches!(held, Some(Kind::CableCoil { .. } | Kind::Wirecutters));
    visuals.cables.retain(|cell, entity| {
        if show_cables && state.cables.contains_key(cell) {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
    if show_cables {
        for &cell in state.cables.keys().filter(|cell| cell.z == level) {
            visuals.cables.entry(cell).or_insert_with(|| {
                commands
                    .spawn((
                        MapVisual,
                        view.cable_sprite(),
                        Transform::from_translation(view.world(cell.center()).extend(0.2)),
                    ))
                    .id()
            });
        }
    }
    for (&cell, &open) in state.pipe_valves.iter().filter(|(cell, _)| cell.z == level) {
        let indicator = || {
            let color = if open {
                Color::srgb_u8(108, 209, 166)
            } else {
                Color::srgb_u8(219, 150, 97)
            };
            Sprite::from_color(color, Vec2::splat(scale * 0.20))
        };
        let (entity, previous) = visuals.pipe_valves.entry(cell).or_insert_with(|| {
            (
                commands
                    .spawn((
                        MapVisual,
                        indicator(),
                        Transform::from_translation(view.world(cell.center()).extend(0.5)),
                    ))
                    .id(),
                open,
            )
        });
        if *previous != open {
            commands.entity(*entity).insert(indicator());
            *previous = open;
        }
    }
    for (&cell, door) in &state.doors {
        if cell.z != level {
            continue;
        }
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
        if cell.z != level {
            continue;
        }
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
            .is_some_and(|item| item.ground.is_some_and(|cell| cell.z == level))
        {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
    for (&id, item) in &state.items {
        let Some(cell) = item.ground.filter(|cell| cell.z == level) else {
            continue;
        };
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

    let show_crew = |crew: &Crew| {
        crew.position.z == level
            && (crew.on_duty()
                || state.phase == Phase::Lobby && crew.participation != Participation::Observing)
    };
    visuals.avatars.retain(|player, entity| {
        if state.players.get(player).is_some_and(show_crew) {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
    for (&player, crew) in &state.players {
        if !show_crew(crew) {
            continue;
        }
        let location = view.world(crew.position).extend(1.0);
        let color = match crew.damage.state() {
            MobState::Dead => Color::srgb_u8(115, 93, 109),
            MobState::Critical => Color::srgb_u8(233, 122, 108),
            MobState::Alive if player == session.player => Color::srgb_u8(115, 230, 209),
            MobState::Alive => Color::srgb_u8(175, 155, 245),
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

    let role = layout
        .roles
        .get(usize::from(crew.role))
        .map_or("", |job| job.name.as_str());
    let (help, status) = match state.phase {
        Phase::Lobby => job_labels(layout, state, crew),
        Phase::Active if !crew.on_duty() => job_labels(layout, state, crew),
        Phase::Active => {
            let construction = matches!(held, Some(Kind::Item { materials: 1.., .. }));
            let cell = crew.position.cell();
            let up = layout.links.iter().any(|link| link.lower == cell);
            let down = layout.links.iter().any(|link| link.upper == cell);
            let life = crew.damage.state();
            let help = if life == MobState::Dead {
                "Await the next round".into()
            } else if life == MobState::Critical {
                "Await medical assistance".into()
            } else if let Some(ticks) = crew.cutting_cable {
                format!(
                    "Cutting cable {:.1}s   /   Hold still",
                    f32::from(ticks) / TICK_HZ as f32
                )
            } else if let Some(progress) = crew.medical_progress {
                let verb = if progress.scanning {
                    "Scanning"
                } else {
                    "Treating"
                };
                format!(
                    "{verb} {:.1}s   /   Hold still",
                    f32::from(progress.ticks_left) / TICK_HZ as f32
                )
            } else if let Some(reading) = crew.medical_reading {
                format!(
                    "Health scan   /   Trauma {}   /   Asphyxiation {}",
                    reading.blunt, reading.asphyxiation
                )
            } else if up && down {
                "PgUp Upper deck   /   PgDn Lower deck".into()
            } else if up {
                "PgUp Upper deck".into()
            } else if down {
                "PgDn Lower deck".into()
            } else if matches!(held, Some(Kind::CableCoil { .. })) {
                "C Lay cable   /   X Swap   Q Drop".into()
            } else if held == Some(Kind::Wirecutters) {
                "R Cut cable   /   X Swap   Q Drop".into()
            } else if construction {
                "B Wall   N Floor   V Dismantle   X Swap   Q Drop   I Inventory".into()
            } else if let Some(scan) = crew.scan {
                format!(
                    "Scan {} kPa   /   O2 {}%   /   {} K",
                    scan.pressure_kpa, scan.oxygen_percent, scan.temperature_k
                )
            } else if let Some(scan) = crew.pipe_scan {
                scan.opposite_kpa.map_or_else(
                    || format!("Pipe   /   {} kPa", scan.pressure_kpa),
                    |other| format!("Valve   /   {} kPa   |   {other} kPa", scan.pressure_kpa),
                )
            } else if matches!(held, Some(Kind::Item { analyzer: true, .. })) {
                "F Sample room   /   P Sample pipe   /   E Interact".into()
            } else if held == Some(Kind::HealthAnalyzer) {
                "F Scan self   /   G Scan nearby crew   /   X Swap   Q Drop".into()
            } else {
                let drag = if crew.pulling.is_none() {
                    "  T Drag"
                } else {
                    ""
                };
                format!("E Interact  X Swap  Q Drop  F Use  G Treat  H Give{drag}  I Inventory")
            };
            let help = if crew.pulling.is_some() {
                format!("{help}   /   T Release")
            } else {
                help
            };
            let role: String = role.chars().take(12).collect();
            let condition = match life {
                MobState::Alive => "DMG",
                MobState::Critical => "CRITICAL",
                MobState::Dead => "DEAD",
            };
            (
                help,
                format!(
                    "R{} / {role} / {:02}:{:02} / Z {level} / {condition} {} / {} kPa",
                    state.round,
                    state.seconds_left / 60,
                    state.seconds_left % 60,
                    crew.damage.total(),
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
                "Seal the hull at {},{},{}   /   Activate the air reserve at {},{},{}",
                goal.breach.x, goal.breach.y, goal.breach.z, goal.vent.x, goal.vent.y, goal.vent.z
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
            Label::Hand(hand) if state.phase == Phase::Active && crew.on_duty() => {
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
                let stock = if prototype.is_some_and(|item| {
                    matches!(
                        item.kind,
                        Kind::Item { materials: 1.., .. } | Kind::CableCoil { .. }
                    )
                }) {
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

fn job_labels(layout: &Layout, state: &State, crew: &Crew) -> (String, String) {
    let help = layout
        .roles
        .iter()
        .enumerate()
        .map(|(index, job)| {
            let number = index + 1;
            let name = &job.name;
            match state.remaining_slots(index, job) {
                Some(free) => format!("{number} {name} ({free} free)"),
                None => format!("{number} {name}"),
            }
        })
        .collect::<Vec<_>>()
        .join("   ");
    let selected = usize::from(crew.role);
    let job = &layout.roles[selected];
    let hint = match crew.participation {
        Participation::Ready => "Ready   /   O Observe",
        Participation::Observing => "Observing   /   O Choose job",
        _ if state.remaining_slots(selected, job) == Some(0) => "Job full   /   O Observe",
        _ if state.phase == Phase::Active => "Enter Join   /   O Observe",
        _ => "Enter Ready   /   O Observe",
    };
    let progress = if state.phase == Phase::Lobby {
        let waiting = state
            .players
            .values()
            .filter(|other| other.participation != Participation::Observing)
            .count();
        let ready = state
            .players
            .values()
            .filter(|other| other.participation == Participation::Ready)
            .count();
        format!("{ready}/{waiting} ready")
    } else {
        "In progress".into()
    };
    (
        help,
        format!(
            "Round {}   /   {}   /   {progress}   /   {hint}",
            state.round, job.name
        ),
    )
}

fn draw_inventory(
    network: NonSend<Network>,
    inventory: Res<InventoryPanel>,
    backdrop: Single<&mut Visibility, With<InventoryBackdrop>>,
    mut lines: Query<(&InventoryLine, &mut Text2d, &mut Visibility), Without<InventoryBackdrop>>,
) {
    let player = network.session().and_then(|session| {
        if !inventory.open || session.state.phase != Phase::Active {
            return None;
        }
        Some((
            &session.layout,
            &session.state,
            session.state.players.get(&session.player)?,
        ))
    });
    inventory_view::sync(player, backdrop, &mut lines);
}

fn capture_frame(
    mut commands: Commands,
    capture: Res<Capture>,
    visuals: Res<Visuals>,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
) {
    if !visuals.initialized {
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
