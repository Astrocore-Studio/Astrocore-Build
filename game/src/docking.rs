//! Стыковка гридов шлюзами: компонент шлюза `Docking`, приоритетные шлюзы `PriorityDock`,
//! подбор положения шаттла у грида (`GetDockingConfig`): все пары шлюзов шаттла и грида, где
//! шаттл встаёт впритык, не ложится тайлами на грид и не задевает чужие гриды; затем —
//! сколько ещё шлюзов совпало при том же положении. Предпочтение: шлюз с нужным тегом, больше
//! пар, угол ближе к углу грида. Стыковка открывает двери, опускает болты и выключает
//! герметизацию; расстыковка — обратно. Образец — `DockingSystem`, `DockingSystem.Shuttle`,
//! `DockingComponent`, `PriorityDockComponent`, `DockingConfig` SS14 @ 2f4a2aad.
//!
//! Без соединения физики (`WeldJoint`): гриды не летают (ADR-0015 Build). Точная форма
//! шаттла — его тайлы (фикстуры грида SS14 покрывают те же тайлы). Посадка на гриды-планеты
//! (`isMap`: проверка закреплённых твёрдых сущностей) — вместе с планетами.

use std::f32::consts::PI;

use astrocore_map::{EntityCoordinates, Map, MapGrid, TileCoord, Transform, TransformWorldExt};
use astrocore_math::{Angle, Box2, Vec2};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::doors::{self, Door, DoorBolt};

/// Знаков после запятой при сравнении положений (`DockRoundingDigits`).
const ROUNDING: f32 = 100.0;

/// Шлюз стыковки (`DockingComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Docking {
    /// С каким шлюзом состыкован (`DockedWith`).
    #[serde(skip)]
    pub docked_with: Option<Entity>,
}

/// Шлюз, предпочитаемый для стыковки шаттлов с тегом (`PriorityDockComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PriorityDock {
    pub tag: Option<String>,
}

/// Положение шаттла у грида (`DockingConfig`).
#[derive(Debug, Clone, PartialEq)]
pub struct DockingConfig {
    /// Пары шлюзов: шаттла и грида.
    pub docks: Vec<(Entity, Entity)>,
    /// Начало шаттла в координатах грида.
    pub coordinates: EntityCoordinates,
    /// Место шаттла в координатах грида.
    pub area: Box2,
    /// Поворот шаттла относительно грида.
    pub angle: Angle,
    pub target_grid: Entity,
}

/// Поворот и сдвиг: из координат шаттла в координаты грида.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub position: Vec2,
    pub rotation: Angle,
}

impl Placement {
    pub fn apply(&self, point: Vec2) -> Vec2 {
        self.rotation.rotate(point) + self.position
    }

    /// Прямоугольник, вмещающий повёрнутый `area` (`TransformBox`).
    pub fn transform_box(&self, area: Box2) -> Box2 {
        let corners = [
            Vec2::new(area.left(), area.bottom()),
            Vec2::new(area.right(), area.bottom()),
            Vec2::new(area.right(), area.top()),
            Vec2::new(area.left(), area.top()),
        ]
        .map(|corner| self.apply(corner));
        corners[1..]
            .iter()
            .fold(Box2::from_corners(corners[0], corners[0]), |acc, c| {
                acc.union(&Box2::from_corners(*c, *c))
            })
    }
}

fn rounded(area: Box2) -> Box2 {
    let round = |v: f32| (v * ROUNDING).round() / ROUNDING;
    Box2::new(
        round(area.left()),
        round(area.bottom()),
        round(area.right()),
        round(area.top()),
    )
}

/// Противоположный угол (`Angle.Opposite`).
fn opposite(angle: Angle) -> Angle {
    Angle(angle.0 + PI).reduced()
}

/// Границы всех тайлов грида в его координатах (`MapGridComponent.LocalAABB`).
pub fn grid_local_aabb(world: &mut World, grid: Entity) -> Option<Box2> {
    let mut map_grid = world.get_mut::<MapGrid>(grid)?;
    let levels: Vec<i16> = map_grid.levels().collect();
    levels
        .into_iter()
        .filter_map(|z| map_grid.local_bounds(z))
        .reduce(|a, b| a.union(&b))
}

/// Тайлы грида.
pub fn grid_tiles(world: &World, grid: Entity) -> Vec<TileCoord> {
    let Some(map_grid) = world.get::<MapGrid>(grid) else {
        return Vec::new();
    };
    map_grid
        .levels()
        .flat_map(|z| map_grid.tiles_on_level(z).map(|(coord, _)| coord))
        .collect()
}

/// Шлюзы стыковки прямо на гриде (`GetDocks`), по порядку сущностей.
pub fn get_docks(world: &mut World, grid: Entity) -> Vec<Entity> {
    let mut docks: Vec<Entity> = world
        .query_filtered::<(Entity, &Transform), bevy_ecs::query::With<Docking>>()
        .iter(world)
        .filter(|(entity, _)| world.parent(*entity) == Some(grid))
        .map(|(entity, _)| entity)
        .collect();
    docks.sort();
    docks
}

fn is_docked(world: &World, dock: Entity) -> bool {
    world
        .get::<Docking>(dock)
        .is_some_and(|d| d.docked_with.is_some())
}

/// Положение шаттла, при котором шлюзы встают впритык (часть `CanDock` без проверки места).
fn dock_placement(world: &World, shuttle_dock: Entity, grid_dock: Entity) -> Option<Placement> {
    let shuttle = world.get::<Transform>(shuttle_dock)?;
    let grid = world.get::<Transform>(grid_dock)?;
    if is_docked(world, shuttle_dock)
        || is_docked(world, grid_dock)
        || !shuttle.anchored()
        || !grid.anchored()
    {
        return None;
    }
    // Точка перед шлюзом шаттла встаёт на место шлюза грида.
    let station_dock =
        shuttle.local_position() + shuttle.local_rotation().rotate(Vec2::new(0.0, -1.0));
    let offset = opposite(grid.local_rotation()) - shuttle.local_rotation();
    Some(Placement {
        position: grid.local_position() - offset.rotate(station_dock),
        rotation: offset,
    })
}

/// Тайлы шаттла не ложатся на тайлы грида (`ValidSpawn`).
fn valid_spawn(world: &World, grid: Entity, placement: Placement, tiles: &[TileCoord]) -> bool {
    let Some(target) = world.get::<MapGrid>(grid) else {
        return false;
    };
    // Тайл чуть меньше клетки (`Enlarged(-0.01)`): соседство краями не мешает.
    const INNER: f32 = 0.49;
    let probes = [
        Vec2::ZERO,
        Vec2::new(-INNER, -INNER),
        Vec2::new(INNER, -INNER),
        Vec2::new(INNER, INNER),
        Vec2::new(-INNER, INNER),
    ];
    !tiles.iter().any(|tile| {
        let center = Vec2::new(tile.x as f32 + 0.5, tile.y as f32 + 0.5);
        probes.iter().any(|probe| {
            let point = placement.apply(center + *probe);
            !target.tile(TileCoord::containing(point, tile.z)).is_empty()
        })
    })
}

/// Шлюзы можно состыковать, поставив шаттл прямо к ним (`CanDock`): положение, место шаттла в
/// координатах грида и поворот.
fn can_dock(
    world: &World,
    shuttle_dock: Entity,
    grid_dock: Entity,
    grid: Entity,
    shuttle_aabb: Box2,
    tiles: &[TileCoord],
) -> Option<(Placement, Box2, Angle)> {
    let placement = dock_placement(world, shuttle_dock, grid_dock)?;
    if !valid_spawn(world, grid, placement, tiles) {
        return None;
    }
    Some((
        placement,
        placement.transform_box(shuttle_aabb),
        placement.rotation.reduced(),
    ))
}

/// Мировые границы грида.
fn world_aabb(world: &mut World, grid: Entity) -> Option<Box2> {
    let local = grid_local_aabb(world, grid)?;
    let placement = world.world_transform(grid)?;
    Some(
        Placement {
            position: placement.position,
            rotation: placement.rotation,
        }
        .transform_box(local),
    )
}

/// Все положения шаттла у грида (`GetDockingConfigs`).
fn get_docking_configs(
    world: &mut World,
    shuttle: Entity,
    target: Entity,
    shuttle_docks: &[Entity],
    grid_docks: &[Entity],
) -> Vec<DockingConfig> {
    let mut configs = Vec::new();
    if grid_docks.is_empty() || shuttle_docks.is_empty() {
        return configs;
    }
    let (Some(target_placement), Some(shuttle_aabb)) = (
        world.world_transform(target),
        grid_local_aabb(world, shuttle),
    ) else {
        return configs;
    };
    let target_map = world.get::<Transform>(target).and_then(Transform::map);
    let tiles = grid_tiles(world, shuttle);
    // Чужие гриды карты цели и их границы.
    let others: Vec<Box2> = target_map
        .and_then(|map| world.get::<Map>(map))
        .map(|map| map.grids().to_vec())
        .unwrap_or_default()
        .into_iter()
        .filter(|grid| *grid != target && *grid != shuttle && Some(*grid) != target_map)
        .filter_map(|grid| world_aabb(world, grid))
        .collect();
    let target_world = Placement {
        position: target_placement.position,
        rotation: target_placement.rotation,
    };
    for &dock in shuttle_docks {
        for &grid_dock in grid_docks {
            let Some((placement, docked_aabb, angle)) =
                can_dock(world, dock, grid_dock, target, shuttle_aabb, &tiles)
            else {
                continue;
            };
            // Шаттл не должен задеть другой грид (`FindGridsIntersecting`).
            let world_bounds = Placement {
                position: target_world.apply(placement.position),
                rotation: Angle(target_placement.rotation.0 + angle.0).reduced(),
            }
            .transform_box(shuttle_aabb);
            if others.iter().any(|other| other.intersects(&world_bounds)) {
                continue;
            }
            // Сколько ещё пар шлюзов совпадает при том же положении.
            let docked_aabb = rounded(docked_aabb);
            let mut docks = vec![(dock, grid_dock)];
            for &other in shuttle_docks {
                if other == dock {
                    continue;
                }
                for &other_grid in grid_docks {
                    if other_grid == grid_dock {
                        continue;
                    }
                    // Совпавшие угол и место — то же положение, уже проверенное целиком.
                    let Some(other_placement) = dock_placement(world, other, other_grid) else {
                        continue;
                    };
                    if other_placement.rotation.reduced() != angle
                        || rounded(other_placement.transform_box(shuttle_aabb)) != docked_aabb
                    {
                        continue;
                    }
                    docks.push((other, other_grid));
                }
            }
            configs.push(DockingConfig {
                docks,
                coordinates: EntityCoordinates::new(target, placement.position),
                area: docked_aabb,
                angle,
                target_grid: target,
            });
        }
    }
    configs
}

/// Шлюз пары на гриде с приоритетом тега (`IsConfigPriority`).
pub fn is_config_priority(world: &World, config: &DockingConfig, tag: Option<&str>) -> bool {
    config.docks.iter().any(|(_, grid_dock)| {
        world
            .get::<PriorityDock>(*grid_dock)
            .is_some_and(|p| tag.is_some() && p.tag.as_deref() == tag)
    })
}

/// Лучшее положение шаттла у грида (`GetDockingConfig`): тег, число пар, угол.
pub fn get_docking_config(
    world: &mut World,
    shuttle: Entity,
    target: Entity,
    priority_tag: Option<&str>,
) -> Option<DockingConfig> {
    let shuttle_docks = get_docks(world, shuttle);
    let grid_docks = get_docks(world, target);
    let mut configs = get_docking_configs(world, shuttle, target, &shuttle_docks, &grid_docks);
    let target_angle = world.world_transform(target)?.rotation.reduced();
    let keys: Vec<(bool, usize, f32)> = configs
        .iter()
        .map(|config| {
            (
                is_config_priority(world, config, priority_tag),
                config.docks.len(),
                Angle(config.angle.0 - target_angle.0).reduced().0.abs(),
            )
        })
        .collect();
    let mut order: Vec<usize> = (0..configs.len()).collect();
    order.sort_by(|a, b| {
        let (ka, kb) = (keys[*a], keys[*b]);
        kb.0.cmp(&ka.0)
            .then(kb.1.cmp(&ka.1))
            .then(ka.2.total_cmp(&kb.2))
    });
    let best = *order.first()?;
    Some(configs.swap_remove(best))
}

/// Положение у грида в заданной точке и под заданным углом (`GetDockingConfigAt`); иначе
/// первое найденное, если `fallback`.
pub fn get_docking_config_at(
    world: &mut World,
    shuttle: Entity,
    target: Entity,
    coordinates: EntityCoordinates,
    angle: Angle,
    fallback: bool,
) -> Option<DockingConfig> {
    let shuttle_docks = get_docks(world, shuttle);
    let grid_docks = get_docks(world, target);
    let mut configs = get_docking_configs(world, shuttle, target, &shuttle_docks, &grid_docks);
    let found = configs.iter().position(|config| {
        config.coordinates.parent == coordinates.parent
            && config.coordinates.position.distance(coordinates.position) < 0.01
            && Angle(config.angle.0 - angle.0).reduced().0.abs() < 0.15
    });
    match found {
        Some(index) => Some(configs.swap_remove(index)),
        None if fallback && !configs.is_empty() => Some(configs.swap_remove(0)),
        None => None,
    }
}

/// Угол от цели к шаттлу относительно цели (`GetAngle`): по центрам гридов.
pub fn get_angle(world: &mut World, shuttle: Entity, target: Entity) -> Option<Angle> {
    let center = |world: &mut World, grid: Entity| -> Option<Vec2> {
        let local = grid_local_aabb(world, grid).map_or(Vec2::ZERO, |a| a.center());
        Some(world.world_transform(grid)?.to_world(local))
    };
    let shuttle_center = center(world, shuttle)?;
    let target_center = center(world, target)?;
    let target_rotation = world.world_transform(target)?.rotation;
    Some(Angle::from_world_vec(shuttle_center - target_center) - target_rotation)
}

/// Открыть дверь шлюза при стыковке.
fn open_dock_door(world: &mut World, dock: Entity) {
    if world.get::<Door>(dock).is_none() {
        return;
    }
    let opened = if world.get::<astrocore_ecs::Paused>(dock).is_none() {
        doors::try_open(world, dock, None, true)
    } else {
        doors::start_opening(world, dock);
        true
    };
    if opened && world.get::<DoorBolt>(dock).is_some() {
        doors::set_bolts_down(world, dock, true);
    }
    if let Some(mut door) = world.get_mut::<Door>(dock) {
        door.change_airtight = false;
    }
}

/// Состыковать два шлюза (`Dock`).
pub fn dock(world: &mut World, a: Entity, b: Entity) {
    if let Some(mut docking) = world.get_mut::<Docking>(a) {
        docking.docked_with = Some(b);
    }
    if let Some(mut docking) = world.get_mut::<Docking>(b) {
        docking.docked_with = Some(a);
    }
    open_dock_door(world, a);
    open_dock_door(world, b);
}

/// Шлюз после расстыковки (`OnUndock`): болты подняты, дверь закрыта и герметична.
fn on_undock(world: &mut World, dock: Entity) {
    if world.get_entity(dock).is_err() {
        return;
    }
    if world.get::<DoorBolt>(dock).is_some() {
        doors::try_set_bolts_down(world, dock, false);
    }
    if world.get::<Door>(dock).is_some()
        && doors::try_close(world, dock, None)
        && let Some(mut door) = world.get_mut::<Door>(dock)
    {
        door.change_airtight = true;
    }
}

/// Расстыковать шлюз (`Undock`).
pub fn undock(world: &mut World, dock: Entity) {
    let Some(other) = world.get::<Docking>(dock).and_then(|d| d.docked_with) else {
        return;
    };
    on_undock(world, dock);
    on_undock(world, other);
    for entity in [dock, other] {
        if let Some(mut docking) = world.get_mut::<Docking>(entity) {
            docking.docked_with = None;
        }
    }
}

/// Расстыковать все шлюзы грида (`UndockDocks`).
pub fn undock_docks(world: &mut World, grid: Entity) {
    for dock in get_docks(world, grid) {
        undock(world, dock);
    }
}

/// Закрыть шлюзы грида и опустить или поднять их болты (`SetDockBolts`).
pub fn set_dock_bolts(world: &mut World, grid: Entity, enabled: bool) {
    for dock in get_docks(world, grid) {
        if world.get::<DoorBolt>(dock).is_none() {
            continue;
        }
        doors::try_close(world, dock, None);
        doors::try_set_bolts_down(world, dock, enabled);
    }
}

/// Шлюз удаляется — пара расстыковывается (`OnShutdown`).
fn on_terminating(world: &mut World, dock: Entity) {
    undock(world, dock);
}

pub fn install(world: &mut World, side: Side) {
    use astrocore_ecs::{EntityTerminating, EventWorldExt, RegistryWorldExt};
    world.register_data_component::<Docking>();
    world.register_data_component::<PriorityDock>();
    if side.is_server() {
        world.subscribe::<Docking, EntityTerminating>(|world, dock, _| on_terminating(world, dock));
    }
}

#[cfg(test)]
pub(crate) mod tests;
