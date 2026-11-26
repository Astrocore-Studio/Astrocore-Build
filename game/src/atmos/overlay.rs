//! Вид газов и пламени на клетках: сервер считает непрозрачность видимых газов и стадию
//! пламени, клиент рисует. Образец — `GasTileOverlayComponent`, `GasTileOverlaySystem`
//! (`GetOverlayData`, `UpdateChunkTile`) и `SharedGasTileOverlaySystem` SS14 @ 2f4a2aad.
//!
//! В отличие от SS14 хранятся только клетки, где что-то видно: обычный воздух станции не
//! занимает места в сетевом состоянии.

use std::collections::BTreeMap;

use astrocore_ecs::{GameTime, GameWorldExt, Update};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use super::gas::{GasData, GasMixture};
use super::grid::GridAtmosphere;

/// Сторона чанка вида (`ChunkSize`).
pub const CHUNK_SIZE: i32 = 8;
/// Число уровней непрозрачности (`net.gasoverlaythresholds`).
pub const THRESHOLDS: i32 = 20;
/// Обновлений в секунду (`net.gasoverlaytickrate`).
pub const TICK_RATE: f32 = 3.0;

/// Вид клетки (`GasOverlayData` без температуры).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayTile {
    /// Стадия пламени: 0 — нет, 1–3 — растёт.
    pub fire: u8,
    /// Непрозрачность видимых газов по порядку [`GasData::visible_gases`].
    pub opacity: Vec<u8>,
}

/// Вид газов грида: чанк (x, y, этаж) → клетка в чанке → вид. Сетевой компонент.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GasTileOverlay {
    pub chunks: BTreeMap<(i32, i32, i16), BTreeMap<u8, OverlayTile>>,
}

/// `ContentHelpers.RoundToLevels`.
fn round_to_levels(actual: f64, max: f64, levels: i32) -> i32 {
    if actual >= max {
        return levels - 1;
    }
    if actual <= 0.0 {
        return 0;
    }
    ((actual / max) * f64::from(levels - 2)).ceil() as i32
}

/// Непрозрачность газа (`GetOpacity`).
pub fn opacity(moles: f32, visible: f32, visible_max: f32) -> u8 {
    let ratio = ((moles - visible) / (visible_max - visible)).clamp(0.0, 1.0);
    (round_to_levels(f64::from(ratio * 255.0), 255.0, THRESHOLDS) * 255 / (THRESHOLDS - 1)) as u8
}

/// Вид клетки по её воздуху и пламени (`GetOverlayData`).
pub fn overlay_tile(
    gas: &GasData,
    visible: &[usize],
    air: Option<&GasMixture>,
    fire: u8,
) -> OverlayTile {
    let opacity = visible
        .iter()
        .map(|&id| {
            let moles = air.map_or(0.0, |a| a.moles[id]);
            if moles < gas.moles_visible[id] {
                0
            } else {
                opacity(
                    moles,
                    gas.moles_visible[id],
                    gas.moles_visible[id] * gas.visibility_factor[id],
                )
            }
        })
        .collect();
    OverlayTile { fire, opacity }
}

fn chunk_key(x: i32, y: i32, z: i16) -> ((i32, i32, i16), u8) {
    let (cx, cy) = (x.div_euclid(CHUNK_SIZE), y.div_euclid(CHUNK_SIZE));
    let (lx, ly) = (x.rem_euclid(CHUNK_SIZE), y.rem_euclid(CHUNK_SIZE));
    ((cx, cy, z), (lx + ly * CHUNK_SIZE) as u8)
}

/// Обновление вида устаревших клеток (`GasTileOverlaySystem.Update`).
fn update(world: &mut World, mut timer: bevy_ecs::system::Local<f32>) {
    *timer += world.resource::<GameTime>().frame_time();
    if *timer < 1.0 / TICK_RATE {
        return;
    }
    *timer -= 1.0 / TICK_RATE;
    let gas = super::gas_data(world);
    let visible = gas.visible_gases();
    let grids: Vec<Entity> = world
        .query::<(Entity, &GridAtmosphere)>()
        .iter(world)
        .filter(|(_, g)| !g.visuals_dirty.is_empty())
        .map(|(e, _)| e)
        .collect();
    for grid in grids {
        let updates: Vec<(TileUpdate, OverlayTile)> = {
            let Some(mut atmos) = world.get_mut::<GridAtmosphere>(grid) else {
                continue;
            };
            let dirty: Vec<u32> = atmos.visuals_dirty.iter().collect();
            atmos.visuals_dirty.clear();
            dirty
                .into_iter()
                .filter_map(|index| {
                    let tile = atmos.tile(index);
                    if tile.removed {
                        return None;
                    }
                    let fire = if tile.hotspot.valid {
                        tile.hotspot.state
                    } else {
                        0
                    };
                    let data = overlay_tile(&gas, &visible, tile.air.as_ref(), fire);
                    Some(((tile.coord.x, tile.coord.y, tile.coord.z), data))
                })
                .collect()
        };
        let mut overlay = world
            .get::<GasTileOverlay>(grid)
            .cloned()
            .unwrap_or_default();
        let mut changed = false;
        for ((x, y, z), data) in updates {
            let (chunk, index) = chunk_key(x, y, z);
            let empty = data.fire == 0 && data.opacity.iter().all(|o| *o == 0);
            if empty {
                if let Some(tiles) = overlay.chunks.get_mut(&chunk)
                    && tiles.remove(&index).is_some()
                {
                    changed = true;
                    if tiles.is_empty() {
                        overlay.chunks.remove(&chunk);
                    }
                }
                continue;
            }
            let tiles = overlay.chunks.entry(chunk).or_default();
            if tiles.get(&index) != Some(&data) {
                tiles.insert(index, data);
                changed = true;
            }
        }
        if changed || world.get::<GasTileOverlay>(grid).is_none() {
            world.entity_mut(grid).insert(overlay);
        }
    }
}

type TileUpdate = (i32, i32, i16);

/// Клетки чанка вида с местами на гриде.
pub fn chunk_tiles(
    chunk: (i32, i32, i16),
    tiles: &BTreeMap<u8, OverlayTile>,
) -> impl Iterator<Item = (i32, i32, &OverlayTile)> {
    tiles.iter().map(move |(index, tile)| {
        let index = i32::from(*index);
        (
            chunk.0 * CHUNK_SIZE + index % CHUNK_SIZE,
            chunk.1 * CHUNK_SIZE + index / CHUNK_SIZE,
            tile,
        )
    })
}

pub fn install(world: &mut World, server: bool) {
    astrocore_net::NetRegistryWorldExt::register_net_serde::<GasTileOverlay>(world);
    if server {
        world.add_game_systems(Update, update);
    }
}
