//! Плитки пола: уложить плитку из стопки и снять плитку инструментом. Образец —
//! `FloorTileSystem` (Content.Shared/Tiles), `SharedToolSystem.Tile` и `TileSystem`
//! (`ReplaceTile`, `DeconstructTile`, история слоёв `TileHistoryComponent`) SS14 @ 2f4a2aad.
//!
//! История слоёв держится в памяти сервера и в карту не пишется: после загрузки снятая
//! плитка открывает свой `baseTurf`, как в SS14 для клеток без истории.

use std::collections::HashMap;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_data::DataNode;
use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_map::{
    EntityCoordinates, MapCoordinates, MapGrid, MapWorldExt, SpawnWorldExt, Tile, TileCoord,
    TileDefinitions, TransformWorldExt,
};
use astrocore_math::{Angle, Vec2};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::do_after::DoAfterDone;
use crate::examine::loc;
use crate::interaction::{AfterInteract, INTERACTION_RANGE, InteractionWorldExt};
use crate::popups::{PopupType, PopupWorldExt};
use crate::random::random;
use crate::stack::StackWorldExt;
use crate::tools::{Tool, ToolWorldExt, register_tool_event};

/// Стопка плиток, которую кладут на пол (`FloorTileComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloorTile {
    /// Какие тайлы можно положить, по порядку попыток.
    #[serde(default)]
    pub outputs: Option<Vec<String>>,
    #[serde(default = "place_sound")]
    pub place_tile_sound: SoundSpecifier,
}

fn place_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Items/genhit.ogg")
}

/// Инструмент снимает тайлы с подходящими `deconstructTools` (`ToolTileCompatibleComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolTileCompatible {
    #[serde(default = "one")]
    pub delay: f64,
    #[serde(default = "yes")]
    pub requires_unobstructed: bool,
}

fn one() -> f64 {
    1.0
}

fn yes() -> bool {
    true
}

/// Слои тайлов под текущим по клеткам грида (`TileHistoryComponent`).
#[derive(Component, Debug, Clone, Default)]
pub struct TileHistory {
    pub stacks: HashMap<TileCoord, Vec<String>>,
}

/// Предел слоёв на клетке (`game.tile_stack_limit`).
pub const TILE_STACK_LIMIT: usize = 5;

/// Окончание снятия тайла (`TileToolDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
pub struct TileToolDoAfter {
    pub grid: Entity,
    pub cell: TileCoord,
}

/// Данные тайла для строительства (`ContentTileDefinition`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TileDef {
    pub id: String,
    pub base_turf: Option<String>,
    pub base_whitelist: Vec<String>,
    pub deconstruct_tools: Vec<String>,
    pub item_drop: Option<String>,
    pub variants: u8,
    pub placement_variants: Vec<f32>,
}

/// Данные тайла по имени прототипа.
pub fn tile_def(world: &World, id: &str) -> Option<TileDef> {
    let node = world
        .get_resource::<Prototypes>()?
        .resolved_node("tile", id)?;
    let text = |key: &str| node.get(key).and_then(DataNode::as_str).map(str::to_owned);
    let strings = |key: &str| -> Vec<String> {
        node.get(key)
            .and_then(DataNode::as_seq)
            .map(|s| {
                s.iter()
                    .filter_map(|n| n.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(TileDef {
        id: id.to_owned(),
        base_turf: text("baseTurf"),
        base_whitelist: strings("baseWhitelist"),
        deconstruct_tools: strings("deconstructTools"),
        item_drop: text("itemDrop"),
        variants: text("variants").and_then(|v| v.parse().ok()).unwrap_or(1),
        placement_variants: strings("placementVariants")
            .iter()
            .filter_map(|v| v.parse().ok())
            .collect(),
    })
}

fn tile_name(world: &World, grid: Entity, cell: TileCoord) -> String {
    let type_id = world
        .get::<MapGrid>(grid)
        .map_or(0, |g| g.tile(cell).type_id);
    world
        .get_resource::<TileDefinitions>()
        .and_then(|d| d.get(type_id))
        .map_or_else(|| astrocore_map::SPACE_TILE.to_owned(), |d| d.id.clone())
}

/// Вариант рисунка с весами (`PickVariant`).
fn pick_variant(world: &mut World, def: &TileDef) -> u8 {
    let mut rng = random(world);
    if def.placement_variants.is_empty() {
        return rng.range(0, i32::from(def.variants.max(1))) as u8;
    }
    let total: f32 = def.placement_variants.iter().sum();
    let mut roll = rng.next_f32() * total;
    for (index, weight) in def.placement_variants.iter().enumerate() {
        if roll < *weight {
            return index as u8;
        }
        roll -= weight;
    }
    (def.placement_variants.len() - 1) as u8
}

fn set_tile_named(world: &mut World, grid: Entity, cell: TileCoord, id: &str, variant: u8) {
    let Some(type_id) = world
        .get_resource::<TileDefinitions>()
        .and_then(|d| d.by_id(id))
        .map(|d| d.type_id)
    else {
        return;
    };
    let tile = Tile {
        type_id,
        variant,
        ..Tile::default()
    };
    world.set_tile(grid, cell, tile);
    let center = cell.center();
    crate::decals::remove_in(
        world,
        grid,
        center - Vec2::splat(0.5),
        center + Vec2::splat(0.5),
    );
}

/// Положить тайл поверх текущего (`ReplaceTile`). `false` — слоёв уже предел.
pub fn replace_tile(
    world: &mut World,
    grid: Entity,
    cell: TileCoord,
    replacement: &TileDef,
) -> bool {
    let current = tile_name(world, grid, cell);
    let history_exists = world
        .get::<TileHistory>(grid)
        .is_some_and(|h| h.stacks.contains_key(&cell));
    if replacement.base_turf.as_deref() != Some(current.as_str()) || history_exists {
        let mut history = world.get::<TileHistory>(grid).cloned().unwrap_or_default();
        let stack = history.stacks.entry(cell).or_default();
        if stack.len() >= TILE_STACK_LIMIT {
            return false;
        }
        if current != astrocore_map::SPACE_TILE {
            stack.push(current);
        }
        world.entity_mut(grid).insert(history);
    }
    let variant = pick_variant(world, replacement);
    set_tile_named(world, grid, cell, &replacement.id, variant);
    true
}

/// Снять тайл: выпадает его предмет, открывается слой ниже (`DeconstructTile`).
pub fn deconstruct_tile(
    world: &mut World,
    grid: Entity,
    cell: TileCoord,
    spawn_item: bool,
) -> bool {
    let current = tile_name(world, grid, cell);
    if current == astrocore_map::SPACE_TILE {
        return false;
    }
    let Some(def) = tile_def(world, &current) else {
        return false;
    };
    let Some(base) = def.base_turf.clone() else {
        return false;
    };
    let previous = world
        .get_mut::<TileHistory>(grid)
        .and_then(|mut history| {
            let stack = history.stacks.get_mut(&cell)?;
            let previous = stack.pop();
            if stack.is_empty() {
                history.stacks.remove(&cell);
            }
            previous
        })
        .unwrap_or(base);
    if spawn_item && let Some(item) = def.item_drop.clone() {
        let (dx, dy, angle) = {
            let mut rng = random(world);
            (rng.next_f32() - 0.5, rng.next_f32() - 0.5, rng.next_f32())
        };
        let bounds = 1.0 - 0.1 * 2.0;
        let at = EntityCoordinates::new(grid, cell.center() + Vec2::new(dx, dy) * bounds);
        if let Ok(entity) = world.spawn_attached(&item, at) {
            let _ = world.set_coordinates(entity, at, Some(Angle(angle * std::f32::consts::TAU)));
        }
    }
    let variant = tile_def(world, &previous).map_or(0, |d| pick_variant(world, &d));
    set_tile_named(world, grid, cell, &previous, variant);
    true
}

/// Клетка и грид для точки клика: грид под точкой или ближайший тайл в пределах 1.5 м
/// (`AlignWithClosestGridTile`).
fn align_with_closest_grid_tile(
    world: &World,
    click: MapCoordinates,
) -> Option<(Entity, TileCoord)> {
    let cell_of = |grid: Entity| {
        let placement = world.world_transform(grid)?;
        Some((
            placement,
            TileCoord::containing(
                placement.to_local(click.position),
                click.z - placement.level,
            ),
        ))
    };
    if let Some(grid) = world.find_grid_at(click) {
        return cell_of(grid).map(|(_, cell)| (grid, cell));
    }
    let map = world.map_entity(click.map)?;
    let grids: Vec<Entity> = world.get::<astrocore_map::Map>(map)?.grids().to_vec();
    let mut best: Option<(f32, Entity, TileCoord)> = None;
    for grid in grids {
        let (Some((placement, cell)), Some(map_grid)) = (cell_of(grid), world.get::<MapGrid>(grid))
        else {
            continue;
        };
        for dx in -2..=2 {
            for dy in -2..=2 {
                let near = cell.offset(astrocore_math::IVec2::new(dx, dy));
                if map_grid.tile(near).is_empty() {
                    continue;
                }
                let local = placement.to_local(click.position);
                let delta = local - near.center();
                if delta.x.abs() > 1.5 || delta.y.abs() > 1.5 {
                    continue;
                }
                let distance = delta.length();
                if best.is_none_or(|(d, _, _)| distance < d) {
                    best = Some((distance, grid, cell));
                }
            }
        }
    }
    best.map(|(_, grid, cell)| (grid, cell))
}

/// Можно ли положить тайл на клетку с этим тайлом (`CanPlaceOn`).
fn can_place_on(def: &TileDef, current: &str) -> bool {
    def.base_turf.as_deref() == Some(current) || def.base_whitelist.iter().any(|t| t == current)
}

/// Клетку закрывает твёрдая статичная сущность (стена).
fn blocked_by_wall(world: &World, grid: Entity, cell: TileCoord) -> bool {
    !crate::flatpack::tile_free(world, grid, cell)
}

/// Положить плитку из стопки (`OnAfterInteract` `FloorTileSystem`).
fn on_after_interact(world: &mut World, item: Entity, event: &mut AfterInteract) {
    if !event.can_reach || event.handled {
        return;
    }
    let Some(outputs) = world.get::<FloorTile>(item).and_then(|f| f.outputs.clone()) else {
        return;
    };
    if world.get::<crate::stack::Stack>(item).is_none() {
        return;
    }
    let user = event.user;
    let sound = world
        .get::<FloorTile>(item)
        .map(|f| f.place_tile_sound.clone());
    let Some((grid, cell)) = align_with_closest_grid_tile(world, event.click) else {
        place_new_grid(world, item, event, &outputs, sound);
        return;
    };
    // Рядом другой грид — класть нельзя, слишком легко промахнуться.
    if other_grid_near(world, grid, event.click) {
        let text = loc(world, "invalid-floor-placement", &[]);
        world.popup_entity_for(&text, user, user, PopupType::Small);
        return;
    }
    // Центр клетки виден строителю — можно; иначе клетку не должна закрывать стена.
    let center = world.to_map_coordinates(EntityCoordinates::new(grid, cell.center()));
    let can_access_center =
        center.is_some_and(|c| world.in_range_unobstructed_point(user, c, INTERACTION_RANGE));
    if !can_access_center && blocked_by_wall(world, grid, cell) {
        return;
    }
    if world
        .get::<TileHistory>(grid)
        .and_then(|h| h.stacks.get(&cell))
        .is_some_and(|s| s.len() >= TILE_STACK_LIMIT)
    {
        let text = loc(world, "invalid-floor-placement", &[]);
        world.popup_entity_for(&text, user, user, PopupType::Small);
        return;
    }
    let current = tile_name(world, grid, cell);
    for output in &outputs {
        let Some(def) = tile_def(world, output) else {
            continue;
        };
        if !can_place_on(&def, &current) {
            continue;
        }
        if !world.try_use_stack(item, 1) {
            continue;
        }
        replace_tile(world, grid, cell, &def);
        if let Some(sound) = &sound
            && let Some(at) = center
        {
            world.play_at(sound, at, None);
        }
        event.handled = true;
        return;
    }
}

fn other_grid_near(world: &World, grid: Entity, click: MapCoordinates) -> bool {
    let Some(map) = world.map_entity(click.map) else {
        return false;
    };
    let Some(grids) = world
        .get::<astrocore_map::Map>(map)
        .map(|m| m.grids().to_vec())
    else {
        return false;
    };
    grids.into_iter().filter(|g| *g != grid).any(|other| {
        let (Some(placement), Some(map_grid)) =
            (world.world_transform(other), world.get::<MapGrid>(other))
        else {
            return false;
        };
        let local = placement.to_local(click.position);
        let cell = TileCoord::containing(local, click.z - placement.level);
        (-1..=1).any(|dx| {
            (-1..=1).any(|dy| {
                !map_grid
                    .tile(cell.offset(astrocore_math::IVec2::new(dx, dy)))
                    .is_empty()
            })
        })
    })
}

/// В открытом космосе плитка с основой «космос» начинает новый грид.
fn place_new_grid(
    world: &mut World,
    item: Entity,
    event: &mut AfterInteract,
    outputs: &[String],
    sound: Option<SoundSpecifier>,
) {
    let Some(first) = outputs.first().and_then(|o| tile_def(world, o)) else {
        return;
    };
    let base_space = outputs
        .iter()
        .filter_map(|o| tile_def(world, o))
        .any(|d| d.base_turf.as_deref() == Some(astrocore_map::SPACE_TILE));
    if !base_space || event.click.z != 0 || !world.try_use_stack(item, 1) {
        return;
    }
    event.handled = true;
    let Some(map) = world.map_entity(event.click.map) else {
        return;
    };
    let grid = world.create_grid(map);
    let _ = world.set_world_position(grid, event.click.position);
    let cell = TileCoord::new(0, 0, 0);
    let variant = pick_variant(world, &first);
    set_tile_named(world, grid, cell, &first.id, variant);
    if let Some(sound) = sound {
        world.play_at(&sound, event.click, None);
    }
}

/// Снять тайл инструментом (`OnToolTileAfterInteract`, `UseToolOnTile`).
fn on_tool_after_interact(world: &mut World, tool: Entity, event: &mut AfterInteract) {
    if event.handled || event.target.is_some() {
        return;
    }
    let Some(component) = world.get::<ToolTileCompatible>(tool).cloned() else {
        return;
    };
    let Some(qualities) = world.get::<Tool>(tool).map(|t| t.qualities.clone()) else {
        return;
    };
    let Some(tile) = world.tile_at(event.click) else {
        return;
    };
    let name = tile_name(world, tile.grid, tile.coord);
    let Some(def) = tile_def(world, &name) else {
        return;
    };
    if !def.deconstruct_tools.iter().any(|q| qualities.contains(q)) || def.base_turf.is_none() {
        return;
    }
    if component.requires_unobstructed && blocked_by_mobs(world, tile.grid, tile.coord) {
        return;
    }
    let Some(center) =
        world.to_map_coordinates(EntityCoordinates::new(tile.grid, tile.coord.center()))
    else {
        return;
    };
    if !world.in_range_unobstructed_point(event.user, center, INTERACTION_RANGE) {
        return;
    }
    let list: Vec<&str> = qualities.iter().map(String::as_str).collect();
    let _ = world.use_tool(
        tool,
        event.user,
        Some(tool),
        component.delay,
        &list,
        TileToolDoAfter {
            grid: tile.grid,
            cell: tile.coord,
        },
        0.0,
    );
    event.handled = true;
}

/// Клетку закрывает твёрдая сущность или существо (`IsTileBlocked` с `MobMask`).
fn blocked_by_mobs(world: &World, grid: Entity, cell: TileCoord) -> bool {
    if blocked_by_wall(world, grid, cell) {
        return true;
    }
    let Some(center) = world.to_map_coordinates(EntityCoordinates::new(grid, cell.center())) else {
        return false;
    };
    crate::entity_storage::entities_near(world, center, 0.45)
        .into_iter()
        .any(|e| crate::whitelist::has_component_named(world, e, "MobState"))
}

fn on_tool_done(world: &mut World, tool: Entity, event: &mut DoAfterDone<TileToolDoAfter>) {
    if event.handled || event.cancelled {
        return;
    }
    let TileToolDoAfter { grid, cell } = event.event;
    let Some(component) = world.get::<ToolTileCompatible>(tool).cloned() else {
        return;
    };
    let Some(qualities) = world.get::<Tool>(tool).map(|t| t.qualities.clone()) else {
        return;
    };
    if component.requires_unobstructed && blocked_by_mobs(world, grid, cell) {
        return;
    }
    let name = tile_name(world, grid, cell);
    let Some(def) = tile_def(world, &name) else {
        return;
    };
    if def.deconstruct_tools.iter().any(|q| qualities.contains(q))
        && deconstruct_tile(world, grid, cell, true)
    {
        event.handled = true;
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<FloorTile>();
    world.register_data_component::<ToolTileCompatible>();
    if !side.is_server() {
        return;
    }
    register_tool_event::<TileToolDoAfter>(world);
    world.subscribe::<FloorTile, AfterInteract>(on_after_interact);
    world.subscribe::<ToolTileCompatible, AfterInteract>(on_tool_after_interact);
    world.subscribe::<ToolTileCompatible, DoAfterDone<TileToolDoAfter>>(on_tool_done);
}

#[cfg(test)]
mod tests;
