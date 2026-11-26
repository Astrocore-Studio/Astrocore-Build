mod construction;
mod content;
mod environment;
pub mod server;

pub use content::{Kind, Layout};

#[cfg(feature = "client")]
pub mod editor;
#[cfg(feature = "client")]
pub mod view;

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::Path;

use astrocore_grid2d::session::{Input, PlayerId, Simulation, TICK_HZ};
use astrocore_grid2d::{Cell, Grid, Position, Tile};
use content::{Content, RecoverySpec};
use environment::Environment;
use serde::{Deserialize, Serialize};

const DEBRIEF_TICKS: u32 = 10 * TICK_HZ;
const MAX_ROUND_ITEMS: usize = 512;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Action {
    Interact,
    Drop,
    SwapHand,
    Use,
    Ready,
    SelectRole(u8),
    Construct { cell: Cell, tile: Tile },
    Treat { cell: Cell },
    ClickHand(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Lobby,
    Active,
    Debrief,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Restored,
    TimeExpired,
    CrewLost,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recovery {
    pub breach: Cell,
    pub vent: Cell,
    pub sealed: bool,
    pub powered: bool,
    pub pressure_kpa: u16,
    pub target_kpa: u8,
    pub stable_seconds: u8,
    pub required_seconds: u8,
}

impl From<&RecoverySpec> for Recovery {
    fn from(spec: &RecoverySpec) -> Self {
        Self {
            breach: spec.breach,
            vent: spec.vent,
            sealed: false,
            powered: false,
            pressure_kpa: 0,
            target_kpa: spec.minimum_kpa,
            stable_seconds: 0,
            required_seconds: spec.hold_seconds,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Crew {
    pub position: Position,
    pub role: u8,
    pub ready: bool,
    pub hands: [Option<u16>; 2],
    pub active_hand: bool,
    pub health: u8,
    pub pressure_kpa: u16,
    pub scan: Option<AirSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirSample {
    pub cell: Cell,
    pub pressure_kpa: u16,
    pub oxygen_percent: u8,
    pub temperature_k: u16,
}

impl Crew {
    fn new(position: Position) -> Self {
        Self {
            position,
            role: 0,
            ready: false,
            hands: [None; 2],
            active_hand: false,
            health: 100,
            pressure_kpa: 101,
            scan: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Door {
    pub prototype: String,
    pub open: bool,
    pub powered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Generator {
    pub prototype: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vent {
    pub prototype: String,
    pub enabled: bool,
    pub powered: bool,
    pub reserve_moles: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub prototype: String,
    pub ground: Option<Cell>,
    pub materials: u8,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct State {
    pub players: BTreeMap<PlayerId, Crew>,
    pub doors: BTreeMap<Cell, Door>,
    pub generators: BTreeMap<Cell, Generator>,
    pub vents: BTreeMap<Cell, Vent>,
    pub items: BTreeMap<u16, Item>,
    pub terrain: BTreeMap<Cell, Tile>,
    pub terrain_revision: u32,
    pub phase: Phase,
    pub round: u32,
    pub seconds_left: u32,
    pub recovery: Option<Recovery>,
    pub outcome: Option<Outcome>,
}

pub struct Station {
    layout: Layout,
    terrain: Grid,
    cables: BTreeSet<Cell>,
    state: State,
    initial_doors: BTreeMap<Cell, Door>,
    initial_generators: BTreeMap<Cell, Generator>,
    initial_vents: BTreeMap<Cell, Vent>,
    environment: Environment,
    initial_items: BTreeMap<u16, Item>,
    loadouts: BTreeMap<String, Vec<String>>,
    next_item: u16,
    recovery: Option<RecoverySpec>,
    recovery_ticks: u32,
    round_ticks: u32,
    phase_ticks: u32,
}

impl Station {
    pub fn load(directory: Option<&Path>) -> Result<Self, String> {
        let Content {
            layout,
            placements,
            cables,
            round_seconds,
            loadouts,
            recovery,
        } = Content::load(directory)?;
        let mut doors = BTreeMap::new();
        let mut generators = BTreeMap::new();
        let mut vents = BTreeMap::new();
        let mut items = BTreeMap::new();
        let next_item = placements.len() as u16;
        for (id, placement) in placements.into_iter().enumerate() {
            match layout.prototypes[&placement.prototype].kind {
                Kind::Door { .. } => {
                    doors.insert(
                        placement.cell,
                        Door {
                            prototype: placement.prototype,
                            open: false,
                            powered: false,
                        },
                    );
                }
                Kind::Generator { .. } => {
                    generators.insert(
                        placement.cell,
                        Generator {
                            prototype: placement.prototype,
                            enabled: !placement.starts_off,
                        },
                    );
                }
                Kind::Vent { reserve_moles, .. } => {
                    vents.insert(
                        placement.cell,
                        Vent {
                            prototype: placement.prototype,
                            enabled: false,
                            powered: false,
                            reserve_moles: f32::from(reserve_moles),
                        },
                    );
                }
                Kind::Item { materials, .. } => {
                    items.insert(
                        id as u16,
                        Item {
                            prototype: placement.prototype,
                            ground: Some(placement.cell),
                            materials,
                        },
                    );
                }
            }
        }
        let mut state = State {
            players: BTreeMap::new(),
            doors: doors.clone(),
            generators: generators.clone(),
            vents: vents.clone(),
            items: items.clone(),
            terrain: BTreeMap::new(),
            terrain_revision: 0,
            phase: Phase::Lobby,
            round: 1,
            seconds_left: 0,
            recovery: recovery.as_ref().map(Recovery::from),
            outcome: None,
        };
        let cables: BTreeSet<_> = cables.into_iter().collect();
        let environment = Environment::new(&layout, &cables, &mut state);
        Ok(Self {
            terrain: layout.grid.clone(),
            layout,
            cables,
            state,
            initial_doors: doors,
            initial_generators: generators,
            initial_vents: vents,
            environment,
            initial_items: items,
            loadouts,
            next_item,
            recovery,
            recovery_ticks: 0,
            round_ticks: round_seconds * TICK_HZ,
            phase_ticks: 0,
        })
    }

    fn interact(&mut self, player: PlayerId) {
        let crew = &self.state.players[&player];
        let cell = crew.position.cell();
        let reach = |target: Cell| cell.x.abs_diff(target.x) + cell.y.abs_diff(target.y);
        let hand = usize::from(crew.active_hand);

        let item = self
            .state
            .items
            .iter()
            .filter_map(|(&id, item)| {
                let ground = item.ground?;
                (crew.hands[hand].is_none()
                    && reach(ground) <= 1
                    && self.state.doors.get(&ground).is_none_or(|door| door.open))
                .then_some((reach(ground), id))
            })
            .min();
        let device = self
            .state
            .doors
            .keys()
            .chain(self.state.generators.keys())
            .chain(self.state.vents.keys())
            .copied()
            .map(|target| (reach(target), target))
            .filter(|&(distance, _)| distance <= 1)
            .min();

        if let Some((distance, id)) = item
            && device.is_none_or(|(device_distance, _)| distance <= device_distance)
        {
            self.state
                .items
                .get_mut(&id)
                .expect("выбранный предмет существует")
                .ground = None;
            self.state
                .players
                .get_mut(&player)
                .expect("игрок существует")
                .hands[hand] = Some(id);
        } else if let Some((_, target)) = device {
            if let Some(generator) = self.state.generators.get_mut(&target) {
                generator.enabled = !generator.enabled;
            } else if let Some(vent) = self.state.vents.get_mut(&target) {
                vent.enabled = !vent.enabled;
            } else if let Some(door) = self.state.doors.get_mut(&target)
                && door.powered
                && (!door.open
                    || !self
                        .state
                        .players
                        .values()
                        .any(|crew| crew.position.overlaps(target)))
            {
                door.open = !door.open;
            }
        }
    }

    fn action(&mut self, player: PlayerId, action: Action) {
        let Some(crew) = self.state.players.get_mut(&player) else {
            return;
        };
        match (self.state.phase, action) {
            (Phase::Lobby, Action::Ready) => crew.ready = !crew.ready,
            (Phase::Lobby, Action::SelectRole(role))
                if usize::from(role) < self.layout.roles.len() =>
            {
                crew.role = role;
            }
            (Phase::Active, _) if crew.health == 0 => {}
            (Phase::Active, Action::Interact) => self.interact(player),
            (Phase::Active, Action::Construct { cell, tile }) => self.construct(player, cell, tile),
            (Phase::Active, Action::Drop) => {
                if let Some(id) = crew.hands[usize::from(crew.active_hand)].take() {
                    self.state
                        .items
                        .get_mut(&id)
                        .expect("предмет в руке существует")
                        .ground = Some(crew.position.cell());
                }
            }
            (Phase::Active, Action::SwapHand) => crew.active_hand = !crew.active_hand,
            (Phase::Active, Action::ClickHand(hand @ 0..=1)) => {
                let clicked = usize::from(hand);
                let active = usize::from(crew.active_hand);
                match (clicked == active, crew.hands[active], crew.hands[clicked]) {
                    (true, Some(_), _) => self.use_item(player, None),
                    (false, _, None) => crew.active_hand = hand == 1,
                    (false, None, Some(item)) => {
                        crew.hands[clicked] = None;
                        crew.hands[active] = Some(item);
                    }
                    _ => {}
                }
            }
            (Phase::Active, Action::Use) => self.use_item(player, None),
            (Phase::Active, Action::Treat { cell }) => self.use_item(player, Some(cell)),
            _ => {}
        }
    }

    fn use_item(&mut self, player: PlayerId, target: Option<Cell>) {
        let crew = &self.state.players[&player];
        let hand = usize::from(crew.active_hand);
        let Some(id) = crew.hands[hand] else { return };
        let prototype = &self.state.items[&id].prototype;
        let Kind::Item {
            healing, analyzer, ..
        } = self.layout.prototypes[prototype].kind
        else {
            return;
        };
        let origin = crew.position.cell();
        if analyzer && target.is_none() {
            let sample = self.environment.sample(origin);
            self.state
                .players
                .get_mut(&player)
                .expect("игрок существует")
                .scan = Some(sample);
            return;
        }
        if healing == 0 {
            return;
        }

        let recipient = match target {
            None => player,
            Some(cell)
                if origin
                    .x
                    .abs_diff(cell.x)
                    .saturating_add(origin.y.abs_diff(cell.y))
                    <= 1
                    && self.state.doors.get(&cell).is_none_or(|door| door.open) =>
            {
                let Some((&recipient, _)) = self.state.players.iter().find(|(id, crew)| {
                    **id != player
                        && crew.position.cell() == cell
                        && (1..100).contains(&crew.health)
                }) else {
                    return;
                };
                recipient
            }
            Some(_) => return,
        };
        let recipient = self
            .state
            .players
            .get_mut(&recipient)
            .expect("пациент существует");
        if (1..100).contains(&recipient.health) {
            recipient.health = recipient.health.saturating_add(healing).min(100);
            self.state
                .players
                .get_mut(&player)
                .expect("игрок существует")
                .hands[hand] = None;
            self.state.items.remove(&id);
        }
    }

    fn equip(&mut self, player: PlayerId) {
        let crew = self
            .state
            .players
            .get_mut(&player)
            .expect("игрок существует");
        let role = &self.layout.roles[usize::from(crew.role)];
        let Some(loadout) = self.loadouts.get(role) else {
            return;
        };
        for (hand, prototype) in crew.hands.iter_mut().zip(loadout) {
            if self.state.items.len() >= MAX_ROUND_ITEMS {
                break;
            }
            let Some(next) = self.next_item.checked_add(1) else {
                break;
            };
            let Kind::Item { materials, .. } = self.layout.prototypes[prototype].kind else {
                continue;
            };
            self.state.items.insert(
                self.next_item,
                Item {
                    prototype: prototype.clone(),
                    ground: None,
                    materials,
                },
            );
            *hand = Some(self.next_item);
            self.next_item = next;
        }
    }

    fn check_recovery(&mut self) -> bool {
        let (Some(spec), Some(progress)) = (&self.recovery, &mut self.state.recovery) else {
            return false;
        };
        let (pressure, breathable) = self.environment.pressure(spec.chamber);
        progress.pressure_kpa = pressure.round() as u16;
        progress.sealed = self.terrain.tile(spec.breach) == Tile::Wall;
        let vent = &self.state.vents[&spec.vent];
        progress.powered = vent.powered && vent.enabled;
        if progress.sealed
            && progress.powered
            && breathable
            && pressure >= f32::from(spec.minimum_kpa)
        {
            self.recovery_ticks += 1;
        } else {
            self.recovery_ticks = 0;
        }
        progress.stable_seconds = (self.recovery_ticks / TICK_HZ) as u8;
        progress.stable_seconds >= progress.required_seconds
    }

    fn advance_round(&mut self) {
        match self.state.phase {
            Phase::Lobby => {
                if !self.state.players.is_empty()
                    && self.state.players.values().all(|crew| crew.ready)
                {
                    self.state.phase = Phase::Active;
                    self.phase_ticks = self.round_ticks;
                    for player in self.state.players.keys().copied().collect::<Vec<_>>() {
                        self.equip(player);
                    }
                }
            }
            Phase::Active => {
                self.phase_ticks -= 1;
                let restored = self.check_recovery();
                self.state.outcome = if self.state.players.values().all(|crew| crew.health == 0) {
                    Some(Outcome::CrewLost)
                } else if restored {
                    Some(Outcome::Restored)
                } else if self.phase_ticks == 0 {
                    Some(Outcome::TimeExpired)
                } else {
                    None
                };
                if self.state.outcome.is_some() {
                    self.state.phase = Phase::Debrief;
                    self.phase_ticks = DEBRIEF_TICKS;
                }
            }
            Phase::Debrief => {
                self.phase_ticks -= 1;
                if self.phase_ticks == 0 {
                    self.state.phase = Phase::Lobby;
                    self.state.round += 1;
                    self.state.doors.clone_from(&self.initial_doors);
                    self.state.generators.clone_from(&self.initial_generators);
                    self.state.vents.clone_from(&self.initial_vents);
                    self.state.items.clone_from(&self.initial_items);
                    self.next_item = self
                        .initial_items
                        .last_key_value()
                        .map_or(0, |(&id, _)| id + 1);
                    self.state.outcome = None;
                    self.recovery_ticks = 0;
                    self.state.recovery = self.recovery.as_ref().map(Recovery::from);
                    self.state.terrain.clear();
                    self.state.terrain_revision += 1;
                    self.terrain.clone_from(&self.layout.grid);
                    for (crew, spawn) in self
                        .state
                        .players
                        .values_mut()
                        .zip(self.layout.spawns.iter().cycle())
                    {
                        let role = crew.role;
                        *crew = Crew::new(spawn.center());
                        crew.role = role;
                    }
                    self.environment.reset(&self.layout, &mut self.state);
                }
            }
        }
        self.state.seconds_left = self.phase_ticks.div_ceil(TICK_HZ);
    }
}

impl Simulation for Station {
    type Layout = Layout;
    type State = State;
    type Action = Action;

    fn layout(&self) -> &Layout {
        &self.layout
    }

    fn state(&self) -> &State {
        &self.state
    }

    fn joined(&mut self, player: PlayerId) {
        let spawn = self
            .layout
            .spawns
            .iter()
            .find(|cell| {
                !self
                    .state
                    .players
                    .values()
                    .any(|crew| crew.position.overlaps(**cell))
            })
            .copied()
            .unwrap_or(self.layout.spawns[0]);
        self.state.players.insert(player, Crew::new(spawn.center()));
        if self.state.phase == Phase::Active {
            self.equip(player);
        }
    }

    fn left(&mut self, player: PlayerId) {
        if let Some(crew) = self.state.players.remove(&player) {
            for id in crew.hands.into_iter().flatten() {
                self.state
                    .items
                    .get_mut(&id)
                    .expect("предмет в руке существует")
                    .ground = Some(crew.position.cell());
            }
        }
    }

    fn tick(&mut self, inputs: &BTreeMap<PlayerId, Input<Action>>) {
        if self.state.phase == Phase::Active {
            let doors = &self.state.doors;
            for (player, crew) in &mut self.state.players {
                if crew.health > 0
                    && let Some(input) = inputs.get(player)
                {
                    crew.position = self
                        .terrain
                        .move_body(crew.position, input.movement, |cell| {
                            doors.get(&cell).is_some_and(|door| !door.open)
                        });
                }
            }
        }
        for crew in self.state.players.values_mut() {
            if crew
                .scan
                .is_some_and(|sample| sample.cell != crew.position.cell())
            {
                crew.scan = None;
            }
        }
        let previous_revision = self.state.terrain_revision;
        for (&player, input) in inputs {
            if let Some(action) = input.action {
                self.action(player, action);
            }
        }
        if self.state.phase == Phase::Active {
            if self.state.terrain_revision != previous_revision {
                self.environment.rebuild_hull(&self.terrain, &self.state);
            }
            self.environment.tick(&mut self.state);
        }
        self.advance_round();
    }
}

/// Общий адрес и ключ для локального сервера и клиентов.
pub fn endpoint() -> Result<(SocketAddr, [u8; 32]), String> {
    Ok((
        bind_address(7782)?,
        configured_key()?.ok_or("задайте общий ASTRO_2D_KEY из 64 шестнадцатеричных символов")?,
    ))
}

pub fn bind_address(default_port: u16) -> Result<SocketAddr, String> {
    match std::env::var("ASTRO_2D_ADDR") {
        Ok(value) => value
            .parse()
            .map_err(|error| format!("ASTRO_2D_ADDR: {error}")),
        Err(std::env::VarError::NotPresent) => Ok(SocketAddr::from(([127, 0, 0, 1], default_port))),
        Err(error) => Err(format!("ASTRO_2D_ADDR: {error}")),
    }
}

pub fn configured_key() -> Result<Option<[u8; 32]>, String> {
    match std::env::var("ASTRO_2D_KEY") {
        Ok(value) => astrocore_network::secure::parse_hex32(&value)
            .map(Some)
            .ok_or("ASTRO_2D_KEY должен содержать 64 шестнадцатеричных символа".into()),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("ASTRO_2D_KEY: {error}")),
    }
}
