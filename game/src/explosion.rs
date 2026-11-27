//! Взрывы (ADR-0008): очередь взрывов, заливка клеток от эпицентра с задержкой на
//! непроницаемых преградах и обработка клеток по тактам — урон сущностям, отбрасывание,
//! ломка пола. Образец — `ExplosionSystem` (`TileFill`, `Airtight`, `Processing`),
//! `ExplosionTileFlood`, `ExplosionGridTileFlood`, `ExplosionPrototype`,
//! `ExplosiveComponent` и `ExplosionResistanceComponent` SS14 @ 2f4a2aad.
//!
//! Отличия (ADR-0008): одна система координат на взрыв — клетки грида эпицентра вместе с
//! пустыми клетками, либо клетки карты в открытом космосе; переход на другие гриды не
//! переносится; стойкость преград считается при взрыве; пол ломается сразу при обработке
//! клетки (`explosion.incremental_tile`).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::Instant;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_console::{CVarDef, CVarFlags, ConfigWorldExt};
use astrocore_data::DataNode;
use astrocore_ecs::{
    EntityTerminating, EventWorldExt, GameWorldExt, LifecycleWorldExt, RegistryWorldExt, Update,
};
use astrocore_map::{
    ContainerWorldExt, Map, MapCoordinates, MapGrid, MapWorldExt, TileCoord, Transform,
    TransformWorldExt,
};
use astrocore_math::{Box2, IVec2, Vec2};
use astrocore_physics::{BodyType, Fixtures, Physics, PhysicsWorldExt, group_bits};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::atmos::grid::{EAST, NORTH, SOUTH, WEST};
use crate::damage::{DamageCatalog, DamageSpecifier, Damageable, DamageableWorldExt, Injurable};
use crate::destructible::{destroyed_at, is_destroyed};
use crate::fixed_point::FixedPoint2;
use crate::random::random;

const SERVER: CVarFlags = CVarFlags::SERVER;
/// Клеток за такт (`explosion.tiles_per_tick`).
pub const TILES_PER_TICK: CVarDef<i32> = CVarDef::new(
    "explosion.tiles_per_tick",
    100,
    SERVER,
    "Клеток взрыва за такт.",
);
/// Отбрасывать во всём взрыве, если он меньше этого (`explosion.throw_limit`).
pub const THROW_LIMIT: CVarDef<i32> = CVarDef::new(
    "explosion.throw_limit",
    400,
    SERVER,
    "Площадь взрыва, до которой отбрасывает везде.",
);
/// Наибольшая площадь (`explosion.max_area`).
pub const MAX_AREA: CVarDef<i32> = CVarDef::new(
    "explosion.max_area",
    3 * 256 * 256,
    SERVER,
    "Наибольшая площадь взрыва, клеток.",
);
/// Наибольшее число волн (`explosion.max_iterations`).
pub const MAX_ITERATIONS: CVarDef<i32> = CVarDef::new(
    "explosion.max_iterations",
    500,
    SERVER,
    "Наибольшее число волн взрыва.",
);
/// Время на взрывы за такт, мс (`explosion.max_tick_time`).
pub const MAX_TICK_TIME: CVarDef<f32> = CVarDef::new(
    "explosion.max_tick_time",
    7.0,
    SERVER,
    "Время на взрывы за такт, мс.",
);
/// Больше этого взрыв начинает обрабатываться со следующего такта
/// (`explosion.single_tick_area_limit`).
pub const SINGLE_TICK_AREA_LIMIT: CVarDef<i32> = CVarDef::new(
    "explosion.single_tick_area_limit",
    400,
    SERVER,
    "Площадь взрыва, которую можно начать в том же такте.",
);
/// Не пересобирать сети узлов во время взрыва (`explosion.node_sleep`).
pub const NODE_SLEEP: CVarDef<bool> = CVarDef::new(
    "explosion.node_sleep",
    true,
    SERVER,
    "Пауза пересборки сетей узлов во время взрыва.",
);
/// Может ли взрыв проломить пол до космоса (`explosion.can_create_vacuum`).
pub const CAN_CREATE_VACUUM: CVarDef<bool> = CVarDef::new(
    "explosion.can_create_vacuum",
    true,
    SERVER,
    "Взрыв может проломить пол в космос.",
);

/// Неразрушимая преграда (`ToleranceValues.Invulnerable`).
const INVULNERABLE: f32 = f32::MAX;

/// Вид взрыва (`ExplosionPrototype`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplosionPrototype {
    #[serde(default)]
    pub damage_per_intensity: DamageSpecifier,
    #[serde(default)]
    pub fire_stacks: Option<f32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default = "break_chance")]
    pub tile_break_chance: Vec<f32>,
    #[serde(default = "break_intensity")]
    pub tile_break_intensity: Vec<f32>,
    #[serde(default = "reroll")]
    pub tile_break_reroll_reduction: f32,
    #[serde(default = "six")]
    pub small_sound_iteration_threshold: usize,
    #[serde(default = "one")]
    pub max_combine_distance: f32,
    #[serde(default = "sound")]
    pub sound: SoundSpecifier,
    #[serde(default = "small_sound")]
    pub small_sound: SoundSpecifier,
    #[serde(default = "fire_texture")]
    pub texture_path: String,
    #[serde(default = "twelve")]
    pub intensity_per_state: f32,
    #[serde(default = "three")]
    pub fire_states: u32,
    #[serde(default)]
    pub fire_color: Option<astrocore_math::Color>,
}

fn break_chance() -> Vec<f32> {
    vec![0.0, 1.0]
}
fn break_intensity() -> Vec<f32> {
    vec![0.0, 15.0]
}
fn reroll() -> f32 {
    10.0
}
fn six() -> usize {
    6
}
fn one() -> f32 {
    1.0
}
fn twelve() -> f32 {
    12.0
}
fn three() -> u32 {
    3
}
fn sound() -> SoundSpecifier {
    SoundSpecifier::collection("Explosion")
}
fn small_sound() -> SoundSpecifier {
    SoundSpecifier::collection("ExplosionSmall")
}
fn fire_texture() -> String {
    "/Textures/Effects/fire.rsi".into()
}

impl ExplosionPrototype {
    /// Вероятность сломать пол при силе (`TileBreakChance`): кусочно-линейная кривая.
    pub fn tile_break_chance(&self, intensity: f32) -> f32 {
        let (chance, points) = (&self.tile_break_chance, &self.tile_break_intensity);
        let (Some(last_point), Some(last_chance)) = (points.last(), chance.last()) else {
            return 0.0;
        };
        if intensity >= *last_point || points.len() == 1 {
            return *last_chance;
        }
        if intensity <= points[0] {
            return chance[0];
        }
        let i = points
            .iter()
            .position(|k| *k >= intensity)
            .unwrap_or(points.len() - 1);
        let slope = (chance[i] - chance[i - 1]) / (points[i] - points[i - 1]);
        chance[i - 1] + slope * (intensity - points[i - 1])
    }
}

/// Вид взрыва по имени.
pub fn explosion_prototype(world: &World, id: &str) -> Option<ExplosionPrototype> {
    let node = world
        .get_resource::<Prototypes>()?
        .resolved_node("explosion", id)?;
    astrocore_data::from_node(node).ok()
}

/// Взрывчатка (`ExplosiveComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Explosive {
    pub explosion_type: String,
    #[serde(default = "four")]
    pub max_intensity: f32,
    #[serde(default = "one")]
    pub intensity_slope: f32,
    #[serde(default = "ten")]
    pub total_intensity: f32,
    #[serde(default = "one")]
    pub tile_break_scale: f32,
    #[serde(default = "max_break")]
    pub max_tile_break: i32,
    #[serde(default = "yes")]
    pub can_create_vacuum: bool,
    #[serde(default)]
    pub delete_after_explosion: Option<bool>,
    #[serde(default)]
    pub repeatable: bool,
    #[serde(skip)]
    pub exploded: bool,
}

fn four() -> f32 {
    4.0
}
fn ten() -> f32 {
    10.0
}
fn max_break() -> i32 {
    i32::MAX
}
fn yes() -> bool {
    true
}

/// Защита от взрывов (`ExplosionResistanceComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplosionResistance {
    #[serde(default = "one")]
    pub damage_coefficient: f32,
    #[serde(default)]
    pub modifiers: BTreeMap<String, f32>,
}

/// Вид взрыва для клиента (`ExplosionVisualsComponent`): сущность-ребёнок грида или карты
/// взрыва в точке ноль; клетки в осях родителя по волнам, сила волн, рисунок огня и сколько
/// волн уже прошло (`ExplosionAppearanceData.Progress`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExplosionVisuals {
    pub level: i16,
    pub tiles: Vec<Vec<(i32, i32)>>,
    pub intensity: Vec<f32>,
    pub texture: String,
    pub intensity_per_state: f32,
    pub fire_states: u32,
    pub fire_color: Option<astrocore_math::Color>,
    pub progress: u32,
}

/// Сколько секунд вид взрыва держится после конца (`explosion.persistence`).
pub const PERSISTENCE: CVarDef<f32> = CVarDef::new(
    "explosion.persistence",
    1.0,
    SERVER,
    "Секунд держится вид взрыва.",
);

/// Взрыв в очереди (`QueuedExplosion`).
#[derive(Debug, Clone)]
pub struct QueuedExplosion {
    pub epicenter: MapCoordinates,
    pub prototype_id: String,
    pub total_intensity: f32,
    pub slope: f32,
    pub max_tile_intensity: f32,
    pub tile_break_scale: f32,
    pub max_tile_break: i32,
    pub can_create_vacuum: bool,
    pub cause: Option<Entity>,
}

/// Идущий взрыв (`Explosion`).
#[derive(Debug)]
pub struct ActiveExplosion {
    pub prototype: ExplosionPrototype,
    pub queued: QueuedExplosion,
    /// Грид или карта, в клетках которого идёт взрыв.
    pub root: Entity,
    pub level: i16,
    pub tiles: BTreeMap<usize, Vec<(i32, i32)>>,
    pub iteration_intensity: Vec<f32>,
    pub area: usize,
    iteration: usize,
    position: usize,
    processed: HashSet<Entity>,
    /// Сущность вида взрыва.
    pub visual: Option<Entity>,
}

/// Очередь взрывов и идущий взрыв. Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct Explosions {
    pub queue: VecDeque<QueuedExplosion>,
    pub active: Option<ActiveExplosion>,
}

// Сила и радиус.

/// Общая сила взрыва радиуса (`RadiusToIntensity`): объём конуса.
pub fn radius_to_intensity(radius: f32, slope: f32, max_intensity: f32) -> f32 {
    let cone = slope * std::f32::consts::PI / 3.0 * radius.powi(3);
    if max_intensity <= 0.0 || slope * radius < max_intensity {
        return cone;
    }
    let h = slope * radius - max_intensity;
    cone - h * std::f32::consts::PI / 3.0 * (h / slope).powi(2)
}

/// Радиус взрыва силы (`IntensityToRadius`).
pub fn intensity_to_radius(total: f32, slope: f32, max_intensity: f32) -> f32 {
    let r0 = max_intensity / slope;
    let v0 = radius_to_intensity(r0, slope, 0.0);
    if total <= v0 {
        return (3.0 * total / (slope * std::f32::consts::PI)).cbrt();
    }
    r0 * ((12.0 * total / v0 - 3.0).sqrt() / 6.0 + 0.5)
}

// Стойкость преград.

/// Сила взрыва, разрушающая сущность (`GetExplosionTolerance`).
fn entity_tolerance(
    world: &World,
    entity: Entity,
    prototype_id: &str,
    prototype: &ExplosionPrototype,
) -> f32 {
    let target = destroyed_at(world, entity);
    let Some(injurable) = world.get::<Injurable>(entity) else {
        return INVULNERABLE;
    };
    if target == FixedPoint2::MAX {
        return INVULNERABLE;
    }
    let damageable = world.get::<Damageable>(entity);
    let catalog = world.get_resource::<DamageCatalog>();
    let modifiers = damageable
        .and_then(|d| d.damage_modifier_set.as_ref())
        .and_then(|set| catalog?.modifier_sets.get(set));
    let resistance = world.get::<ExplosionResistance>(entity);
    let mut per_intensity = 0.0f32;
    let mut thresholds: BTreeMap<FixedPoint2, f32> = BTreeMap::new();
    for (damage_type, value) in &prototype.damage_per_intensity.types {
        let supported =
            catalog.is_none_or(|c| c.supports(injurable.damage_container.as_deref(), damage_type));
        if !supported {
            continue;
        }
        let value = value.float();
        let mut modifier = 1.0;
        if let Some(resistance) = resistance {
            modifier *= resistance.damage_coefficient;
            if let Some(m) = resistance.modifiers.get(prototype_id) {
                modifier *= m;
            }
        }
        if let Some(set) = modifiers {
            if let Some(c) = set.coefficients.get(damage_type) {
                modifier *= c;
            }
            if let Some(flat) = set.flat_reductions.get(damage_type)
                && *flat > 0.0
            {
                let intensity = FixedPoint2::from_f32(flat / value);
                *thresholds.entry(intensity).or_default() += value * modifier.max(0.0);
                continue;
            }
        }
        per_intensity += value * modifier.max(0.0);
    }
    let current = damageable.map_or(0.0, |d| d.total_damage.float());
    let mut damage_target = target.float() - current;
    let mut tolerance = if per_intensity > 0.0 {
        damage_target / per_intensity
    } else {
        INVULNERABLE
    };
    let mut previous = 0.0;
    for (intensity, damage) in thresholds {
        let intensity = intensity.float();
        if intensity > tolerance {
            return tolerance;
        }
        damage_target -= (intensity - previous) * per_intensity;
        per_intensity += damage;
        tolerance = intensity + damage_target / per_intensity;
        previous = intensity;
    }
    tolerance
}

/// Преграды клетки: перекрытые направления и стойкость (`TileData`).
#[derive(Debug, Clone, Copy, Default)]
struct TileData {
    blocked: u8,
    tolerance: f32,
}

// Заливка.

fn offset(tile: (i32, i32), direction: u8) -> (i32, i32) {
    match direction {
        NORTH => (tile.0, tile.1 + 1),
        SOUTH => (tile.0, tile.1 - 1),
        EAST => (tile.0 + 1, tile.1),
        _ => (tile.0 - 1, tile.1),
    }
}

fn opposite(direction: u8) -> u8 {
    match direction {
        NORTH => SOUTH,
        SOUTH => NORTH,
        EAST => WEST,
        _ => EAST,
    }
}

/// Клетка, ждущая разрушения преграды, и направление входа в неё.
type DelayedTile = ((i32, i32), u8);

/// Заливка клеток (`ExplosionGridTileFlood`).
struct Flood<'a> {
    world: &'a World,
    root: Entity,
    level: i16,
    prototype_id: &'a str,
    prototype: &'a ExplosionPrototype,
    max_intensity: f32,
    step: f32,
    tile_lists: HashMap<usize, Vec<(i32, i32)>>,
    blocked_lists: HashMap<usize, Vec<(i32, i32)>>,
    freed_lists: HashMap<usize, HashSet<(i32, i32)>>,
    delayed: HashMap<usize, Vec<DelayedTile>>,
    processed: HashSet<(i32, i32)>,
    unentered_blocked: HashSet<(i32, i32)>,
    entered_blocked: HashSet<(i32, i32)>,
    airtight: HashMap<(i32, i32), Option<TileData>>,
    new_tiles: Vec<(i32, i32)>,
    new_blocked: Vec<(i32, i32)>,
    new_freed: HashSet<(i32, i32)>,
}

impl Flood<'_> {
    fn tile_data(&mut self, tile: (i32, i32)) -> Option<TileData> {
        if let Some(data) = self.airtight.get(&tile) {
            return *data;
        }
        let data = self.world.get::<MapGrid>(self.root).and_then(|grid| {
            let mut data = TileData::default();
            for entity in grid.anchored_at(TileCoord::new(tile.0, tile.1, self.level)) {
                let blocked = crate::atmos::entity_blocked_directions(self.world, *entity);
                if blocked == 0 {
                    continue;
                }
                data.blocked |= blocked;
                let tolerance =
                    entity_tolerance(self.world, *entity, self.prototype_id, self.prototype);
                data.tolerance = if tolerance == INVULNERABLE || data.tolerance == INVULNERABLE {
                    INVULNERABLE
                } else {
                    data.tolerance + tolerance
                };
            }
            (data.blocked != 0).then_some(data)
        });
        self.airtight.insert(tile, data);
        data
    }

    fn unblocked_or_all(&mut self, tile: (i32, i32)) -> u8 {
        !self.tile_data(tile).map_or(0, |d| d.blocked)
    }

    fn init_tile(&mut self, tile: (i32, i32)) {
        self.tile_lists.insert(0, vec![tile]);
        if self.tile_data(tile).is_some() {
            self.entered_blocked.insert(tile);
        } else {
            self.processed.insert(tile);
        }
    }

    fn clear_iteration(&self, iteration: usize, required: f32) -> usize {
        iteration + (required / self.step).ceil() as usize
    }

    fn process_new_tile(&mut self, iteration: usize, tile: (i32, i32), entry: u8) {
        let Some(data) = self.tile_data(tile) else {
            if self.processed.insert(tile) {
                self.new_tiles.push(tile);
            }
            return;
        };
        let blocked = data.blocked & entry == entry;
        if blocked {
            if self.entered_blocked.contains(&tile) || !self.unentered_blocked.insert(tile) {
                return;
            }
            self.new_blocked.push(tile);
            if data.tolerance > self.max_intensity {
                return;
            }
            let clear = self.clear_iteration(iteration, data.tolerance);
            self.freed_lists.entry(clear).or_default().insert(tile);
            return;
        }
        if !self.entered_blocked.insert(tile) {
            return;
        }
        if self.unentered_blocked.contains(&tile) {
            self.new_freed.insert(tile);
            return;
        }
        self.new_tiles.push(tile);
    }

    fn add_adjacent(&mut self, iteration: usize, tiles: &[(i32, i32)], ignore_blockers: bool) {
        for &tile in tiles {
            let data = self.tile_data(tile);
            let blocked = data.map_or(0, |d| d.blocked);
            let seal = data.map_or(0.0, |d| d.tolerance);
            for direction in [NORTH, SOUTH, EAST, WEST] {
                if ignore_blockers || blocked & direction == 0 {
                    self.process_new_tile(iteration, offset(tile, direction), opposite(direction));
                }
            }
            if ignore_blockers || blocked == 0 || seal > self.max_intensity {
                continue;
            }
            let clear = self.clear_iteration(iteration, seal);
            let list = self.delayed.entry(clear).or_default();
            for direction in [NORTH, SOUTH, EAST, WEST] {
                if blocked & direction != 0 {
                    list.push((offset(tile, direction), opposite(direction)));
                }
            }
        }
    }

    fn add_diagonal(&mut self, iteration: usize, tiles: &[(i32, i32)], ignore_local: bool) {
        let set = |flags: u8, bits: u8| flags & bits == bits;
        for &tile in tiles {
            let free = if ignore_local {
                u8::MAX
            } else {
                self.unblocked_or_all(tile)
            };
            let n = self.unblocked_or_all(offset(tile, NORTH));
            let e = self.unblocked_or_all(offset(tile, EAST));
            let s = self.unblocked_or_all(offset(tile, SOUTH));
            let w = self.unblocked_or_all(offset(tile, WEST));
            let mut entry = 0;
            // Северо-восток.
            if set(free, NORTH) && set(n, SOUTH | EAST) {
                entry |= WEST;
            }
            if set(free, EAST) && set(e, NORTH | WEST) {
                entry |= SOUTH;
            }
            if entry != 0 {
                self.process_new_tile(iteration, (tile.0 + 1, tile.1 + 1), entry);
                entry = 0;
            }
            // Северо-запад (как в SS14: второе условие тоже даёт запад).
            if set(free, NORTH) && set(n, SOUTH | WEST) {
                entry |= EAST;
            }
            if set(free, WEST) && set(w, NORTH | EAST) {
                entry |= WEST;
            }
            if entry != 0 {
                self.process_new_tile(iteration, (tile.0 - 1, tile.1 + 1), entry);
                entry = 0;
            }
            // Юго-восток.
            if set(free, SOUTH) && set(s, NORTH | EAST) {
                entry |= WEST;
            }
            if set(free, EAST) && set(e, SOUTH | WEST) {
                entry |= NORTH;
            }
            if entry != 0 {
                self.process_new_tile(iteration, (tile.0 + 1, tile.1 - 1), entry);
                entry = 0;
            }
            // Юго-запад.
            if set(free, SOUTH) && set(s, NORTH | WEST) {
                entry |= WEST;
            }
            if set(free, WEST) && set(w, SOUTH | EAST) {
                entry |= NORTH;
            }
            if entry != 0 {
                self.process_new_tile(iteration, (tile.0 - 1, tile.1 - 1), entry);
            }
        }
    }

    /// Новые клетки волны (`AddNewTiles`).
    fn add_new_tiles(&mut self, iteration: usize) -> usize {
        self.new_tiles = Vec::new();
        self.new_blocked = Vec::new();
        if let Some(mut freed) = self.freed_lists.remove(&iteration) {
            freed.retain(|tile| self.entered_blocked.insert(*tile));
            self.new_freed = freed;
        } else {
            self.new_freed = HashSet::new();
        }
        if iteration >= 2 {
            if let Some(adjacent) = self.tile_lists.get(&(iteration - 2)).cloned() {
                self.add_adjacent(iteration, &adjacent, false);
            }
            if let Some(freed) = self.freed_lists.get(&(iteration - 2)).cloned() {
                let freed: Vec<_> = freed.into_iter().collect();
                self.add_adjacent(iteration, &freed, true);
            }
        }
        if iteration >= 3 {
            if let Some(diagonal) = self.tile_lists.get(&(iteration - 3)).cloned() {
                self.add_diagonal(iteration, &diagonal, false);
            }
            if let Some(freed) = self.freed_lists.get(&(iteration - 3)).cloned() {
                let freed: Vec<_> = freed.into_iter().collect();
                self.add_diagonal(iteration, &freed, true);
            }
        }
        if let Some(delayed) = self.delayed.remove(&iteration) {
            for (tile, direction) in delayed {
                self.process_new_tile(iteration, tile, direction);
            }
        }
        let freed = std::mem::take(&mut self.new_freed);
        self.freed_lists.insert(iteration, freed);
        let count = self.new_tiles.len() + self.new_blocked.len();
        if !self.new_tiles.is_empty() {
            self.tile_lists
                .insert(iteration, std::mem::take(&mut self.new_tiles));
        }
        if !self.new_blocked.is_empty() {
            self.blocked_lists
                .insert(iteration, std::mem::take(&mut self.new_blocked));
        }
        count
    }

    /// Задержанные клетки — в общие списки (`CleanUp`).
    fn clean_up(mut self) -> BTreeMap<usize, Vec<(i32, i32)>> {
        for (iteration, blocked) in std::mem::take(&mut self.blocked_lists) {
            self.tile_lists
                .entry(iteration)
                .or_default()
                .extend(blocked);
        }
        self.tile_lists.into_iter().collect()
    }
}

/// Клетки и сила волн взрыва (`GetExplosionTiles`).
#[allow(clippy::type_complexity)]
pub fn explosion_tiles(
    world: &World,
    epicenter: MapCoordinates,
    prototype_id: &str,
    prototype: &ExplosionPrototype,
    total_intensity: f32,
    slope: f32,
    max_intensity: f32,
) -> Option<(
    Entity,
    i16,
    usize,
    Vec<f32>,
    BTreeMap<usize, Vec<(i32, i32)>>,
)> {
    if total_intensity <= 0.0 || slope <= 0.0 {
        return None;
    }
    // Грид эпицентра или ближайший грид рядом, иначе клетки карты.
    let radius = 0.5 + intensity_to_radius(total_intensity, slope, max_intensity);
    let map = world.map_entity(epicenter.map)?;
    let near_grid = world.find_grid_at(epicenter).or_else(|| {
        world.get::<Map>(map)?.grids().iter().copied().find(|grid| {
            world.world_transform(*grid).is_some_and(|p| {
                let local = p.to_local(epicenter.position);
                let cell = TileCoord::containing(local, epicenter.z - p.level);
                let r = radius.ceil() as i32;
                world.get::<MapGrid>(*grid).is_some_and(|g| {
                    (-r..=r).any(|dx| {
                        (-r..=r).any(|dy| !g.tile(cell.offset(IVec2::new(dx, dy))).is_empty())
                    })
                })
            })
        })
    });
    let (root, local, level) = match near_grid.and_then(|g| Some((g, world.world_transform(g)?))) {
        Some((grid, placement)) => (
            grid,
            placement.to_local(epicenter.position),
            epicenter.z - placement.level,
        ),
        None => (map, epicenter.position, epicenter.z),
    };
    let initial = (local.x.floor() as i32, local.y.floor() as i32);
    let max_iterations = world.cvar(&MAX_ITERATIONS).max(1) as usize;
    let max_area = world.cvar(&MAX_AREA).max(1) as usize;
    let step = slope / 2.0;
    let mut flood = Flood {
        world,
        root,
        level,
        prototype_id,
        prototype,
        max_intensity,
        step,
        tile_lists: HashMap::new(),
        blocked_lists: HashMap::new(),
        freed_lists: HashMap::new(),
        delayed: HashMap::new(),
        processed: HashSet::new(),
        unentered_blocked: HashSet::new(),
        entered_blocked: HashSet::new(),
        airtight: HashMap::new(),
        new_tiles: Vec::new(),
        new_blocked: Vec::new(),
        new_freed: HashSet::new(),
    };
    flood.init_tile(initial);
    if total_intensity < step {
        return Some((root, level, 1, vec![total_intensity], flood.clean_up()));
    }
    let mut tiles_in_iteration = vec![1usize];
    let mut iteration_intensity = vec![step];
    let mut total_tiles = 1usize;
    let mut remaining = total_intensity - step;
    let mut iteration = 1usize;
    let mut max_index = 0usize;
    let mut unchanged_last_loop = false;
    while remaining > 0.0 && iteration <= max_iterations && total_tiles < max_area {
        let previous = remaining;
        // Начало берётся до цикла, как в `for (var i = maxIntensityIndex; …)` SS14.
        let first = max_index;
        for i in first..iteration {
            let increase = step.min(max_intensity - iteration_intensity[i]);
            if tiles_in_iteration[i] as f32 * increase >= remaining {
                iteration_intensity[i] += remaining / tiles_in_iteration[i] as f32;
                remaining = 0.0;
                break;
            }
            iteration_intensity[i] += increase;
            remaining -= tiles_in_iteration[i] as f32 * increase;
            if increase < step {
                max_index += 1;
            }
        }
        if remaining <= 0.0 {
            break;
        }
        let new_count = flood.add_new_tiles(iteration);
        tiles_in_iteration.push(new_count);
        if new_count as f32 * step >= remaining {
            iteration_intensity.push(remaining / new_count as f32);
            break;
        }
        remaining -= new_count as f32 * step;
        iteration_intensity.push(step);
        total_tiles += new_count;
        if unchanged_last_loop && remaining == previous {
            break;
        }
        unchanged_last_loop = remaining == previous;
        iteration += 1;
    }
    Some((
        root,
        level,
        total_tiles,
        iteration_intensity,
        flood.clean_up(),
    ))
}

// Очередь.

/// Поставить взрыв в очередь (`QueueExplosion`); соседние взрывы того же вида сливаются.
#[allow(clippy::too_many_arguments)]
pub fn queue_explosion(
    world: &mut World,
    epicenter: MapCoordinates,
    prototype_id: &str,
    total_intensity: f32,
    slope: f32,
    max_tile_intensity: f32,
    cause: Option<Entity>,
    tile_break_scale: f32,
    max_tile_break: i32,
    can_create_vacuum: bool,
) {
    if total_intensity <= 0.0 || slope <= 0.0 {
        return;
    }
    let Some(prototype) = explosion_prototype(world, prototype_id) else {
        return;
    };
    let mut explosions = world.get_resource_or_insert_with(Explosions::default);
    for queued in explosions.queue.iter_mut() {
        if queued.prototype_id != prototype_id || queued.epicenter.map != epicenter.map {
            continue;
        }
        if (queued.epicenter.position - epicenter.position).length()
            > prototype.max_combine_distance
        {
            continue;
        }
        queued.total_intensity += total_intensity;
        return;
    }
    explosions.queue.push_back(QueuedExplosion {
        epicenter,
        prototype_id: prototype_id.to_owned(),
        total_intensity,
        slope,
        max_tile_intensity,
        tile_break_scale,
        max_tile_break,
        can_create_vacuum,
        cause,
    });
}

/// Взорвать взрывчатку сущности (`TriggerExplosive`).
pub fn trigger_explosive(world: &mut World, entity: Entity, user: Option<Entity>) {
    let Some(explosive) = world.get::<Explosive>(entity).cloned() else {
        return;
    };
    if explosive.exploded {
        return;
    }
    if let Some(mut component) = world.get_mut::<Explosive>(entity) {
        component.exploded = !explosive.repeatable;
    }
    let Some(at) = world.map_coordinates(entity) else {
        return;
    };
    queue_explosion(
        world,
        at,
        &explosive.explosion_type,
        explosive.total_intensity,
        explosive.intensity_slope,
        explosive.max_intensity,
        user.or(Some(entity)),
        explosive.tile_break_scale,
        explosive.max_tile_break,
        explosive.can_create_vacuum,
    );
    if explosive.delete_after_explosion.unwrap_or(true) {
        world.queue_delete(entity);
    }
}

fn spawn_explosion(world: &mut World, queued: QueuedExplosion) -> Option<ActiveExplosion> {
    let prototype = explosion_prototype(world, &queued.prototype_id)?;
    let (root, level, area, iteration_intensity, tiles) = explosion_tiles(
        world,
        queued.epicenter,
        &queued.prototype_id,
        &prototype,
        queued.total_intensity,
        queued.slope,
        queued.max_tile_intensity,
    )?;
    let sound = if iteration_intensity.len() < prototype.small_sound_iteration_threshold {
        &prototype.small_sound
    } else {
        &prototype.sound
    };
    world.play_at(sound, queued.epicenter, None);
    let visual = spawn_visuals(world, root, level, &prototype, &tiles, &iteration_intensity);
    Some(ActiveExplosion {
        prototype,
        queued,
        root,
        level,
        tiles,
        iteration_intensity,
        area,
        iteration: 0,
        position: 0,
        processed: HashSet::new(),
        visual,
    })
}

/// Сущность вида взрыва (`CreateExplosionVisualEntity`).
fn spawn_visuals(
    world: &mut World,
    root: Entity,
    level: i16,
    prototype: &ExplosionPrototype,
    tiles: &BTreeMap<usize, Vec<(i32, i32)>>,
    intensity: &[f32],
) -> Option<Entity> {
    let mut by_iteration = vec![Vec::new(); intensity.len()];
    for (iteration, list) in tiles {
        if let Some(slot) = by_iteration.get_mut(*iteration) {
            slot.extend_from_slice(list);
        }
    }
    let entity = world.spawn_entity();
    world
        .set_coordinates(
            entity,
            astrocore_map::EntityCoordinates::on_level(root, Vec2::ZERO, 0),
            None,
        )
        .ok()?;
    world.entity_mut(entity).insert(ExplosionVisuals {
        level,
        tiles: by_iteration,
        intensity: intensity.to_vec(),
        texture: prototype.texture_path.clone(),
        intensity_per_state: prototype.intensity_per_state,
        fire_states: prototype.fire_states,
        fire_color: prototype.fire_color,
        progress: 1,
    });
    world.initialize_and_start(entity, true);
    // Вид виден всем, как бы далеко ни был (`AddGlobalOverride`).
    if let Some(mut overrides) = world.get_resource_mut::<astrocore_net::PvsOverrides>() {
        overrides.global.push(entity);
    }
    Some(entity)
}

fn set_progress(world: &mut World, visual: Option<Entity>, progress: u32) {
    if let Some(visual) = visual
        && let Some(mut component) = world.get_mut::<ExplosionVisuals>(visual)
        && component.progress != progress
    {
        component.progress = progress;
    }
}

// Обработка.

/// Сущности клетки (`GetEntitiesIntersecting` по деревьям поиска): закреплённые в клетке,
/// тела широкой фазы корня и тела карты над клеткой грида.
fn cell_entities(world: &World, root: Entity, level: i16, tile: (i32, i32)) -> Vec<Entity> {
    let bounds = Box2::from_corners(
        Vec2::new(tile.0 as f32, tile.1 as f32),
        Vec2::new(tile.0 as f32 + 1.0, tile.1 as f32 + 1.0),
    );
    let cell = TileCoord::new(tile.0, tile.1, level);
    let mut found = Vec::new();
    if let Some(grid) = world.get::<MapGrid>(root) {
        found.extend(grid.anchored_at(cell).iter().copied());
        if let (Some(placement), Some(map)) = (
            world.world_transform(root),
            world.get::<Transform>(root).and_then(Transform::map),
        ) {
            let center = placement.to_world(cell.center());
            found.extend(
                world
                    .entities_intersecting(
                        map,
                        level + placement.level,
                        Box2::centered(center, Vec2::ONE),
                    )
                    .into_iter()
                    .filter(|e| world.containing_container(*e).is_none()),
            );
        }
    }
    found.extend(
        world
            .entities_intersecting(root, level, bounds)
            .into_iter()
            .filter(|e| world.containing_container(*e).is_none()),
    );
    found.retain(|e| *e != root);
    found.sort();
    found.dedup();
    found
}

/// Твёрдая закреплённая преграда (`IsBlockingTurf`).
fn is_blocking_turf(world: &World, entity: Entity) -> bool {
    if is_destroyed(world, entity) {
        return false;
    }
    let impassable = group_bits("Impassable").unwrap_or(0);
    world.get::<Physics>(entity).is_some_and(|b| b.can_collide)
        && world.get::<Fixtures>(entity).is_some_and(|f| {
            f.fixtures
                .values()
                .any(|x| x.hard && x.layer.0 & impassable != 0)
        })
}

fn scaled(damage: &DamageSpecifier, factor: f32) -> DamageSpecifier {
    DamageSpecifier {
        types: damage
            .types
            .iter()
            .map(|(t, v)| (t.clone(), v.scale(factor)))
            .collect(),
    }
}

/// Урон и отбрасывание сущности (`ProcessEntity`).
fn process_entity(
    world: &mut World,
    entity: Entity,
    epicenter: Vec2,
    damage: Option<&DamageSpecifier>,
    throw_force: f32,
    prototype_id: &str,
    cause: Option<Entity>,
) {
    if let Some(damage) = damage
        && world.get::<Damageable>(entity).is_some()
    {
        let coefficient = world.get::<ExplosionResistance>(entity).map_or(1.0, |r| {
            r.damage_coefficient * r.modifiers.get(prototype_id).copied().unwrap_or(1.0)
        });
        let damage = scaled(damage, coefficient.max(0.0));
        world.change_damage(entity, &damage, false, true, cause);
    }
    if throw_force <= 0.0 || is_destroyed(world, entity) {
        return;
    }
    let anchored = world
        .get::<Transform>(entity)
        .is_some_and(Transform::anchored);
    let dynamic = world
        .get::<Physics>(entity)
        .is_some_and(|b| b.body_type == BodyType::Dynamic);
    if anchored || !dynamic {
        return;
    }
    let Some(position) = world.map_coordinates(entity).map(|p| p.position) else {
        return;
    };
    let mut direction = position - epicenter;
    if direction.length_squared() < 1e-6 {
        let angle = random(world).next_f32() * std::f32::consts::TAU;
        direction = Vec2::new(angle.cos(), angle.sin());
    }
    crate::throwing::try_throw(world, entity, direction, throw_force, None, false);
}

fn tile_flag(world: &World, id: &str, key: &str) -> bool {
    world
        .get_resource::<Prototypes>()
        .and_then(|p| p.resolved_node("tile", id))
        .and_then(|n| n.get(key).and_then(DataNode::as_str))
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

fn tile_name(world: &World, grid: Entity, cell: TileCoord) -> Option<String> {
    let tile = world.get::<MapGrid>(grid)?.tile(cell);
    if tile.is_empty() {
        return None;
    }
    world
        .get_resource::<astrocore_map::TileDefinitions>()?
        .get(tile.type_id)
        .map(|d| d.id.clone())
}

/// Сломать пол (`DamageFloorTile`): с вероятностью по силе снимать слой за слоем.
fn damage_floor_tile(
    world: &mut World,
    grid: Entity,
    cell: TileCoord,
    intensity: f32,
    active: &ActiveExplosion,
) {
    let Some(mut name) = tile_name(world, grid, cell) else {
        return;
    };
    if tile_flag(world, &name, "indestructible") {
        return;
    }
    let can_create_vacuum = active.queued.can_create_vacuum && world.cvar(&CAN_CREATE_VACUUM)
        || tile_flag(world, &name, "mapAtmosphere");
    let mut effective = intensity;
    let mut breaks = 0;
    while active.queued.max_tile_break > breaks {
        let chance = active.prototype.tile_break_chance(effective);
        if !random(world).prob(chance) {
            break;
        }
        breaks += 1;
        effective -= active.prototype.tile_break_reroll_reduction;
        // Следующий слой: из истории клетки или базовый тайл.
        let next = world
            .get::<crate::floor_tile::TileHistory>(grid)
            .and_then(|h| h.stacks.get(&cell).and_then(|s| s.last().cloned()))
            .or_else(|| crate::floor_tile::tile_def(world, &name).and_then(|d| d.base_turf));
        let Some(next) = next else {
            break;
        };
        let to_space =
            next == astrocore_map::SPACE_TILE || tile_flag(world, &next, "mapAtmosphere");
        if to_space && !can_create_vacuum {
            break;
        }
        if !crate::floor_tile::deconstruct_tile(world, grid, cell, false) {
            break;
        }
        name = next;
        if tile_flag(world, &name, "indestructible") || name == astrocore_map::SPACE_TILE {
            break;
        }
    }
}

/// Обработать клетку (`ExplodeTile`, `ExplodeSpace`).
fn explode_tile(
    world: &mut World,
    active: &mut ActiveExplosion,
    tile: (i32, i32),
    intensity: f32,
    throw_force: f32,
) {
    let damage = scaled(&active.prototype.damage_per_intensity, intensity);
    let epicenter = active.queued.epicenter.position;
    let cell = TileCoord::new(tile.0, tile.1, active.level);
    let on_floor = world
        .get::<MapGrid>(active.root)
        .is_some_and(|g| !g.tile(cell).is_empty());
    let entities: Vec<Entity> = cell_entities(world, active.root, active.level, tile)
        .into_iter()
        .filter(|e| active.processed.insert(*e))
        .collect();
    let mut blocked = false;
    for entity in &entities {
        let anchored = world
            .get::<Transform>(*entity)
            .is_some_and(Transform::anchored);
        if anchored {
            blocked |= is_blocking_turf(world, *entity);
        }
        process_entity(
            world,
            *entity,
            epicenter,
            Some(&damage),
            throw_force,
            &active.queued.prototype_id,
            active.queued.cause,
        );
    }
    if !on_floor {
        return;
    }
    if let Some(temperature) = active.prototype.temperature {
        crate::atmos::hotspot_expose_at(world, active.root, cell, temperature, intensity, true);
    }
    // Тела, которые ещё стоят после урона, решают, ломается ли пол.
    let still_blocked = blocked
        && entities.iter().any(|e| {
            world.get::<Transform>(*e).is_some_and(Transform::anchored)
                && is_blocking_turf(world, *e)
        });
    if !still_blocked {
        damage_floor_tile(
            world,
            active.root,
            cell,
            intensity * active.queued.tile_break_scale,
            active,
        );
    }
}

/// Обработать часть клеток (`Explosion.Process`). `true` — взрыв закончен.
fn process(
    world: &mut World,
    active: &mut ActiveExplosion,
    budget: usize,
    started: Instant,
    max_ms: f32,
) -> (usize, bool) {
    let throw_limit = world.cvar(&THROW_LIMIT).max(0) as usize;
    let mut processed = 0;
    loop {
        if processed >= budget
            || (processed % 10 == 0
                && started.elapsed().as_secs_f32() * 1000.0 > max_ms
                && processed > 0)
        {
            return (processed, false);
        }
        let length = active.tiles.get(&active.iteration).map_or(0, Vec::len);
        if active.position >= length {
            match active
                .tiles
                .keys()
                .find(|k| **k > active.iteration)
                .copied()
            {
                Some(next) => {
                    active.iteration = next;
                    active.position = 0;
                    continue;
                }
                None => return (processed, true),
            }
        }
        let iteration = active.iteration;
        let tile = active.tiles[&iteration][active.position];
        let intensity = active
            .iteration_intensity
            .get(iteration)
            .copied()
            .unwrap_or(0.0);
        // Отбрасывает весь небольшой взрыв или внешние шесть волн большого.
        let do_throw =
            active.area < throw_limit || iteration + 6 > active.iteration_intensity.len();
        let throw_force = if do_throw {
            10.0 * intensity.sqrt()
        } else {
            0.0
        };
        explode_tile(world, active, tile, intensity, throw_force);
        active.position += 1;
        processed += 1;
    }
}

/// Взрывы за такт (`Update`): из очереди по одному, не больше бюджета клеток и времени.
fn update(world: &mut World) {
    let has_work = world
        .get_resource::<Explosions>()
        .is_some_and(|e| e.active.is_some() || !e.queue.is_empty());
    if !has_work {
        return;
    }
    let started = Instant::now();
    // Сети узлов не пересобираются, пока идёт взрыв (`explosion.node_sleep`).
    if world.cvar(&NODE_SLEEP)
        && let Some(mut groups) = world.get_resource_mut::<crate::node_container::NodeGroups>()
    {
        groups.pause_updating = true;
    }
    let max_ms = world.cvar(&MAX_TICK_TIME);
    let single_tick_limit = world.cvar(&SINGLE_TICK_AREA_LIMIT).max(0) as usize;
    let mut remaining = world.cvar(&TILES_PER_TICK).max(1) as usize;
    while remaining > 0 && started.elapsed().as_secs_f32() * 1000.0 < max_ms {
        let active = world.resource_mut::<Explosions>().active.take();
        let mut active = match active {
            Some(active) => active,
            None => {
                let Some(queued) = world.resource_mut::<Explosions>().queue.pop_front() else {
                    break;
                };
                let Some(spawned) = spawn_explosion(world, queued) else {
                    continue;
                };
                if spawned.area > single_tick_limit {
                    world.resource_mut::<Explosions>().active = Some(spawned);
                    break;
                }
                spawned
            }
        };
        let (processed, done) = process(world, &mut active, remaining, started, max_ms);
        remaining = remaining.saturating_sub(processed.max(1));
        if done {
            // Вид держится ещё немного и исчезает сам.
            set_progress(world, active.visual, u32::MAX);
            let lifetime = world.cvar(&PERSISTENCE);
            if let Some(visual) = active.visual {
                world
                    .entity_mut(visual)
                    .insert(crate::projectiles::TimedDespawn { lifetime });
            }
        } else {
            set_progress(world, active.visual, active.iteration as u32 + 1);
            world.resource_mut::<Explosions>().active = Some(active);
        }
    }
    let idle = world
        .get_resource::<Explosions>()
        .is_some_and(|e| e.active.is_none() && e.queue.is_empty());
    if idle && let Some(mut groups) = world.get_resource_mut::<crate::node_container::NodeGroups>()
    {
        groups.pause_updating = false;
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Explosive>();
    world.register_data_component::<ExplosionResistance>();
    astrocore_net::NetRegistryWorldExt::register_net_serde::<ExplosionVisuals>(world);
    if !side.is_server() {
        return;
    }
    world.register_cvar(&PERSISTENCE);
    world.subscribe::<ExplosionVisuals, EntityTerminating>(|world, entity, _| {
        if let Some(mut overrides) = world.get_resource_mut::<astrocore_net::PvsOverrides>() {
            overrides.global.retain(|e| *e != entity);
        }
    });
    for cvar in [
        &TILES_PER_TICK,
        &THROW_LIMIT,
        &MAX_AREA,
        &MAX_ITERATIONS,
        &SINGLE_TICK_AREA_LIMIT,
    ] {
        world.register_cvar(cvar);
    }
    world.register_cvar(&MAX_TICK_TIME);
    world.register_cvar(&CAN_CREATE_VACUUM);
    world.register_cvar(&NODE_SLEEP);
    world.init_resource::<Explosions>();
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
