//! Прыжок шаттла (FTL) без полёта: разгон (`shuttle.startup_time`), затем шаттл переносится
//! на карту перехода `FTL` и через время перехода — к цели: стыковкой к гриду
//! (`GetDockingConfigAt`), рядом с ним (`TryFTLProximity`) или в точку. Стыковка без прыжка
//! (`TryFTLDock`): сразу к шлюзам, иначе рядом. Образец — `ShuttleSystem.FasterThanLight`,
//! `FTLComponent`, `FTLMapComponent`, `FTLStartedEvent`, `FTLCompletedEvent` SS14 @ 2f4a2aad.
//!
//! Телепорт вместо полёта, без состояний прибытия и перезарядки (ADR-0015 Build). Сбивание с
//! ног (`DoTheDinosaur`), раздавливание под гридом (`Smimsh`), двигатели, назначения FTL и
//! фон гиперпространства — вместе с полётом.

use std::time::Duration;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_console::{CVarDef, CVarFlags, ConfigWorldExt};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MetaData, Update};
use astrocore_map::{EntityCoordinates, Map, MapWorldExt, Transform, TransformWorldExt};
use astrocore_math::{Angle, Box2, Vec2};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

use crate::Side;
use crate::docking::{self, DockingConfig, Placement};

pub const FTL_STARTUP_TIME: CVarDef<f32> = CVarDef::new(
    "shuttle.startup_time",
    5.5,
    CVarFlags::SERVERONLY,
    "Сколько секунд шаттл разгоняется перед прыжком.",
);
pub const FTL_TRAVEL_TIME: CVarDef<f32> = CVarDef::new(
    "shuttle.travel_time",
    20.0,
    CVarFlags::SERVERONLY,
    "Сколько секунд длится переход по умолчанию.",
);

/// Промежуток между шаттлами на карте перехода (`Buffer`).
const BUFFER: f32 = 5.0;
/// Сколько раз расширяется область поиска места рядом (`FTLProximityIterations`).
const PROXIMITY_ITERATIONS: usize = 5;
/// Наибольший отступ от области при посадке рядом (`maxOffset`).
const MAX_OFFSET: f32 = 64.0;

const STARTUP_SOUND: &str = "/Audio/Effects/Shuttle/hyperspace_begin.ogg";
const TRAVEL_SOUND: &str = "/Audio/Effects/Shuttle/hyperspace_progress.ogg";
const ARRIVAL_SOUND: &str = "/Audio/Effects/Shuttle/hyperspace_end.ogg";

/// Карта перехода (`FTLMapComponent`).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct FtlMap;

/// Стадия прыжка (`FTLState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtlState {
    /// Разгон.
    Starting,
    /// На карте перехода.
    Travelling,
}

/// Шаттл в прыжке (`FTLComponent`).
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Ftl {
    pub state: FtlState,
    /// Когда кончается стадия.
    pub state_end: Duration,
    pub travel_time: f32,
    /// Куда прибыть: грид (стыковка) или карта.
    pub target: EntityCoordinates,
    pub target_angle: Angle,
}

/// Шаттл ушёл в переход (`FTLStartedEvent`). На шаттле.
#[derive(Debug, Clone, Copy)]
pub struct FtlStarted {
    pub from_map: Option<Entity>,
    pub target: EntityCoordinates,
}

/// Шаттл прибыл (`FTLCompletedEvent`). На шаттле.
#[derive(Debug, Clone, Copy)]
pub struct FtlCompleted {
    pub map: Option<Entity>,
}

/// Левый край свободного места на карте перехода (`_index`).
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct FtlIndex(pub f32);

fn now(world: &World) -> Duration {
    world
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default()
}

fn seconds(value: f32) -> Duration {
    Duration::from_secs_f32(value.max(0.0))
}

fn placement_of(world: &World, entity: Entity) -> Option<Placement> {
    let transform = world.world_transform(entity)?;
    Some(Placement {
        position: transform.position,
        rotation: transform.rotation,
    })
}

/// Карта перехода; создаётся при первом прыжке (`EnsureFTLMap`).
pub fn ensure_ftl_map(world: &mut World) -> Entity {
    let existing = world
        .query_filtered::<Entity, bevy_ecs::query::With<FtlMap>>()
        .iter(world)
        .next();
    if let Some(map) = existing {
        return map;
    }
    let (map, _) = world.create_map(true);
    world.entity_mut(map).insert(FtlMap);
    if let Some(mut meta) = world.get_mut::<MetaData>(map) {
        meta.name = Some("FTL".into());
    }
    map
}

/// Поставить шаттл по положению стыковки и состыковать шлюзы (`FTLDock`).
pub fn ftl_dock(world: &mut World, shuttle: Entity, config: &DockingConfig) {
    let Some(target) = placement_of(world, config.target_grid) else {
        return;
    };
    let Some(map) = world
        .get::<Transform>(config.target_grid)
        .and_then(Transform::map)
    else {
        return;
    };
    let position = target.apply(config.coordinates.position);
    let rotation = Angle(config.angle.0 + target.rotation.0);
    let _ = world.set_coordinates(
        shuttle,
        EntityCoordinates::new(map, position),
        Some(rotation),
    );
    for &(a, b) in &config.docks {
        docking::dock(world, a, b);
    }
}

/// Стыковка сразу, без перехода (`TryFTLDock`): к шлюзам, иначе рядом. Положение стыковки,
/// если она удалась.
pub fn try_ftl_dock(
    world: &mut World,
    shuttle: Entity,
    target: Entity,
    priority_tag: Option<&str>,
) -> Option<DockingConfig> {
    world.get::<Transform>(target).and_then(Transform::map)?;
    match docking::get_docking_config(world, shuttle, target, priority_tag) {
        Some(config) => {
            ftl_dock(world, shuttle, &config);
            Some(config)
        }
        None => {
            try_ftl_proximity(world, shuttle, target);
            None
        }
    }
}

/// Место рядом с целью, не задевая гридов (`TryGetFTLProximity`): координаты на карте цели и
/// поворот.
pub fn get_ftl_proximity(
    world: &mut World,
    shuttle: Entity,
    target: EntityCoordinates,
) -> Option<(EntityCoordinates, Angle)> {
    let target_entity = target.parent;
    let map = world.get::<Transform>(target_entity).and_then(|t| {
        if world.get::<Map>(target_entity).is_some() {
            Some(target_entity)
        } else {
            t.map()
        }
    })?;
    let shuttle_aabb = docking::grid_local_aabb(world, shuttle)?;
    let target_placement = placement_of(world, target_entity)?;
    let expansion = (shuttle_aabb.size().x / 2.0).max(shuttle_aabb.size().y / 2.0);
    let mut area = target_placement
        .transform_box(Box2::centered(target.position, Vec2::ONE))
        .enlarged(expansion);
    let grids: Vec<Entity> = world
        .get::<Map>(map)
        .map(|m| m.grids().to_vec())
        .unwrap_or_default()
        .into_iter()
        .filter(|grid| *grid != shuttle)
        .collect();
    let bounds: Vec<(Entity, Box2)> = grids
        .into_iter()
        .filter_map(|grid| {
            let local = docking::grid_local_aabb(world, grid)?;
            Some((grid, placement_of(world, grid)?.transform_box(local)))
        })
        .collect();
    let mut nearby: Vec<Entity> = Vec::new();
    let mut iteration = 0;
    while iteration < PROXIMITY_ITERATIONS {
        let query = area.enlarged(MAX_OFFSET);
        let before = nearby.len();
        for (grid, aabb) in &bounds {
            if aabb.intersects(&query) && !nearby.contains(grid) {
                nearby.push(*grid);
                area = area.union(&aabb.enlarged(expansion));
            }
        }
        if nearby.len() == before {
            break;
        }
        iteration += 1;
        // Слишком тесно — вся карта.
        if iteration == PROXIMITY_ITERATIONS {
            for (grid, aabb) in &bounds {
                if !nearby.contains(grid) {
                    area = area.union(&aabb.enlarged(expansion));
                }
            }
        }
    }
    let target_is_map = world.get::<Map>(target_entity).is_some();
    let spawn = if nearby.len() > 1 || !target_is_map {
        let mut random = crate::random::random(world);
        let offset_angle = Angle(random.next_f32() * std::f32::consts::TAU);
        let min_radius = (area.size().x / 2.0).max(area.size().y / 2.0);
        let distance = min_radius + random.next_f32() * MAX_OFFSET;
        area.center() + offset_angle.rotate(Vec2::new(distance, 0.0))
    } else {
        target_placement.position
    };
    let angle = if target_is_map {
        Angle::ZERO
    } else {
        Angle(crate::random::random(world).next_f32() * std::f32::consts::TAU)
    };
    // Центр шаттла встаёт в найденную точку.
    let origin = spawn - angle.rotate(shuttle_aabb.center());
    Some((EntityCoordinates::new(map, origin), angle))
}

/// Встать рядом с гридом (`TryFTLProximity`).
pub fn try_ftl_proximity(world: &mut World, shuttle: Entity, target: Entity) -> bool {
    let Some((coordinates, angle)) =
        get_ftl_proximity(world, shuttle, EntityCoordinates::new(target, Vec2::ZERO))
    else {
        return false;
    };
    world
        .set_coordinates(shuttle, coordinates, Some(angle))
        .is_ok()
}

/// Начать прыжок (`TrySetupFTL`): расстыковка, карта перехода, звук разгона.
fn setup_ftl(world: &mut World, shuttle: Entity) -> bool {
    if world.get::<Ftl>(shuttle).is_some() {
        eprintln!("шаттл {shuttle:?} уже в прыжке");
        return false;
    }
    docking::undock_docks(world, shuttle);
    world.play_entity(&SoundSpecifier::path(STARTUP_SOUND), shuttle, None);
    ensure_ftl_map(world);
    true
}

/// Прыжок к гриду со стыковкой (`FTLToDock`): место выбирается сейчас.
pub fn ftl_to_dock(
    world: &mut World,
    shuttle: Entity,
    target: Entity,
    startup_time: Option<f32>,
    travel_time: Option<f32>,
    priority_tag: Option<&str>,
) {
    if !setup_ftl(world, shuttle) {
        return;
    }
    let startup = startup_time.unwrap_or_else(|| world.cvar(&FTL_STARTUP_TIME));
    let travel = travel_time.unwrap_or_else(|| world.cvar(&FTL_TRAVEL_TIME));
    let (target_coordinates, target_angle) =
        match docking::get_docking_config(world, shuttle, target, priority_tag) {
            Some(config) => (config.coordinates, config.angle),
            None => {
                match get_ftl_proximity(world, shuttle, EntityCoordinates::new(target, Vec2::ZERO))
                {
                    Some(found) => found,
                    None => {
                        eprintln!("шаттлу {shuttle:?} некуда прыгнуть");
                        let own = world
                            .coordinates(shuttle)
                            .unwrap_or(EntityCoordinates::new(target, Vec2::ZERO));
                        (own, Angle::ZERO)
                    }
                }
            }
        };
    let state_end = now(world) + seconds(startup);
    world.entity_mut(shuttle).insert(Ftl {
        state: FtlState::Starting,
        state_end,
        travel_time: travel,
        target: target_coordinates,
        target_angle,
    });
}

/// Прыжок в точку карты (`FTLToCoordinates`).
pub fn ftl_to_coordinates(
    world: &mut World,
    shuttle: Entity,
    coordinates: EntityCoordinates,
    angle: Angle,
    startup_time: Option<f32>,
    travel_time: Option<f32>,
) {
    if !setup_ftl(world, shuttle) {
        return;
    }
    let startup = startup_time.unwrap_or_else(|| world.cvar(&FTL_STARTUP_TIME));
    let travel = travel_time.unwrap_or_else(|| world.cvar(&FTL_TRAVEL_TIME));
    let state_end = now(world) + seconds(startup);
    world.entity_mut(shuttle).insert(Ftl {
        state: FtlState::Starting,
        state_end,
        travel_time: travel,
        target: coordinates,
        target_angle: angle,
    });
}

/// Конец разгона (`UpdateFTLStarting`): шаттл на карте перехода.
fn start_travelling(world: &mut World, shuttle: Entity) {
    let from_map = world.get::<Transform>(shuttle).and_then(Transform::map);
    let ftl_map = ensure_ftl_map(world);
    let aabb = docking::grid_local_aabb(world, shuttle).unwrap_or_default();
    let index = world.get_resource_or_insert_with(FtlIndex::default).0;
    let start = Vec2::new(index + aabb.size().x / 2.0, 0.0) - aabb.center();
    let _ = world.set_coordinates(
        shuttle,
        EntityCoordinates::new(ftl_map, start),
        Some(Angle::ZERO),
    );
    world.resource_mut::<FtlIndex>().0 += aabb.size().x + BUFFER;
    docking::set_dock_bolts(world, shuttle, true);
    let time = now(world);
    let Some(mut ftl) = world.get_mut::<Ftl>(shuttle) else {
        return;
    };
    ftl.state = FtlState::Travelling;
    ftl.state_end = time + seconds(ftl.travel_time);
    let target = ftl.target;
    world.raise(shuttle, &mut FtlStarted { from_map, target });
    world.play_entity(&SoundSpecifier::path(TRAVEL_SOUND), shuttle, None);
}

/// Конец перехода (`UpdateFTLArriving`): к гриду со стыковкой, рядом или в точку.
fn arrive(world: &mut World, shuttle: Entity) {
    let Some(ftl) = world.entity_mut(shuttle).take::<Ftl>() else {
        return;
    };
    docking::set_dock_bolts(world, shuttle, false);
    let target = ftl.target.parent;
    let mut map = None;
    if world.get_entity(target).is_err() {
        eprintln!("цель прыжка шаттла {shuttle:?} исчезла");
    } else if world.get::<astrocore_map::MapGrid>(target).is_some()
        && world.get::<Map>(target).is_none()
    {
        match docking::get_docking_config_at(
            world,
            shuttle,
            target,
            ftl.target,
            ftl.target_angle,
            true,
        ) {
            Some(config) => ftl_dock(world, shuttle, &config),
            None => {
                try_ftl_proximity(world, shuttle, target);
            }
        }
        map = world.get::<Transform>(target).and_then(Transform::map);
    } else {
        let _ = world.set_coordinates(shuttle, ftl.target, Some(ftl.target_angle));
        map = world.get::<Transform>(shuttle).and_then(Transform::map);
    }
    world.play_entity(&SoundSpecifier::path(ARRIVAL_SOUND), shuttle, None);
    world.raise(shuttle, &mut FtlCompleted { map });
}

/// Такт прыжков (`UpdateHyperspace`).
pub fn update(world: &mut World) {
    let time = now(world);
    let due: Vec<(Entity, FtlState)> = world
        .query::<(Entity, &Ftl)>()
        .iter(world)
        .filter(|(_, ftl)| ftl.state_end <= time)
        .map(|(entity, ftl)| (entity, ftl.state))
        .collect();
    for (shuttle, state) in due {
        match state {
            FtlState::Starting => start_travelling(world, shuttle),
            FtlState::Travelling => arrive(world, shuttle),
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    if !side.is_server() {
        return;
    }
    world.register_cvar(&FTL_STARTUP_TIME);
    world.register_cvar(&FTL_TRAVEL_TIME);
    world.init_resource::<FtlIndex>();
    world.subscribe_broadcast::<crate::game_ticker::RoundRestartCleanup>(|world, _| {
        world.insert_resource(FtlIndex::default());
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
