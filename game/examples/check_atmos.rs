//! Проверка атмосферы на карте раунда: сервер с картой работает `секунды` игрового
//! времени; печатает клетки с воздухом, среднее давление внутри станции и изменение
//! количества газа, затем пробивает стену у космоса и смотрит, как уходит воздух.
//!
//! `cargo run --release -p astrocore-game --example check_atmos [-- 20]`

use std::time::Instant;

use astrocore_ecs::{GameTime, GameWorldExt, LifecycleWorldExt, MetaData};
use astrocore_game::airtight::Airtight;
use astrocore_game::atmos::{self, GridAtmosphere};
use astrocore_game::server;
use astrocore_map::{TileCoord, Transform};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

struct Stats {
    tiles: usize,
    pressurized: usize,
    average: f32,
    moles: f64,
}

fn stats(world: &mut World) -> Stats {
    let mut stats = Stats {
        tiles: 0,
        pressurized: 0,
        average: 0.0,
        moles: 0.0,
    };
    let mut sum = 0.0;
    for atmos in world.query::<&GridAtmosphere>().iter(world) {
        for (_, tile) in atmos.live_tiles() {
            let Some(air) = &tile.air else {
                continue;
            };
            if tile.space {
                continue;
            }
            stats.tiles += 1;
            stats.moles += f64::from(air.total_moles());
            if air.pressure() > 50.0 {
                stats.pressurized += 1;
                sum += air.pressure();
            }
        }
    }
    stats.average = sum / stats.pressurized.max(1) as f32;
    stats
}

fn run(world: &mut World, seconds: f32) -> std::time::Duration {
    let ticks = (seconds * f32::from(world.resource::<GameTime>().tick_rate)) as u32;
    let started = Instant::now();
    for _ in 0..ticks {
        world.run_tick();
    }
    started.elapsed() / ticks.max(1)
}

fn print(label: &str, stats: &Stats) {
    println!(
        "{label}: клеток с воздухом {}, под давлением {} (среднее {:.1} кПа), газа {:.0} моль",
        stats.tiles, stats.pressurized, stats.average, stats.moles
    );
}

/// Стена станции, за которой космос.
fn outer_wall(world: &mut World) -> Option<(Entity, Entity, TileCoord)> {
    let walls: Vec<(Entity, Entity, TileCoord)> = world
        .query::<(Entity, &Airtight, &Transform, &MetaData)>()
        .iter(world)
        .filter(|(_, _, _, meta)| {
            meta.prototype
                .as_deref()
                .is_some_and(|p| p.starts_with("WallSolid") || p.starts_with("WallReinforced"))
        })
        .filter_map(|(entity, _, transform, _)| {
            let grid = transform.grid()?;
            Some((
                entity,
                grid,
                TileCoord::containing(transform.local_position(), transform.level()),
            ))
        })
        .collect();
    walls.into_iter().find(|(_, grid, coord)| {
        let space = |dx: i32, dy: i32| {
            atmos::is_tile_space(
                world,
                *grid,
                TileCoord::new(coord.x + dx, coord.y + dy, coord.z),
            )
        };
        let inside = |dx: i32, dy: i32| {
            atmos::tile_mixture(
                world,
                *grid,
                TileCoord::new(coord.x + dx, coord.y + dy, coord.z),
            )
            .is_some_and(|air| air.pressure() > 90.0)
        };
        (space(-1, 0) && inside(1, 0))
            || (space(1, 0) && inside(-1, 0))
            || (space(0, -1) && inside(0, 1))
            || (space(0, 1) && inside(0, -1))
    })
}

fn main() -> Result<(), String> {
    let seconds: f32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20.0);
    let mut start = server::start()?;
    let world = &mut start.world;
    // Где появится игрок: номер грида и клетка — для `addgas` в консоли.
    if let Some((entity, _)) = world
        .query::<(Entity, &MetaData)>()
        .iter(world)
        .find(|(_, m)| m.prototype.as_deref() == Some("SpawnPointLatejoin"))
        && let Some(transform) = world.get::<Transform>(entity)
        && let Some(grid) = transform.grid()
    {
        let cell = TileCoord::containing(transform.local_position(), transform.level());
        let net = world
            .get::<astrocore_net::NetEntity>(grid)
            .map_or(0, |n| n.0);
        println!("Точка появления: грид {net}, клетка {},{}", cell.x, cell.y);
    }
    let first = run(world, 1.0);
    print("Через 1 с", &stats(world));
    let before = stats(world);
    let per_tick = run(world, seconds);
    let after = stats(world);
    print(&format!("Через {} с", seconds + 1.0), &after);
    println!(
        "Изменение газа: {:+.3} % (такт сервера: первые {:.1} мс, далее {:.2} мс)",
        (after.moles - before.moles) / before.moles * 100.0,
        first.as_secs_f64() * 1000.0,
        per_tick.as_secs_f64() * 1000.0
    );
    let Some((wall, grid, coord)) = outer_wall(world) else {
        println!("Внешняя стена не найдена");
        return Ok(());
    };
    println!("Пробиваю стену на {},{}", coord.x, coord.y);
    world.delete_entity(wall);
    for step in 1..=6 {
        let per_tick = run(world, 5.0);
        let stats = stats(world);
        let near = atmos::tile_mixture(world, grid, coord).map_or(0.0, |a| a.pressure());
        println!(
            "  +{} с: под давлением {}, газа {:.0} моль, в проломе {:.1} кПа, такт {:.2} мс",
            step * 5,
            stats.pressurized,
            stats.moles,
            near,
            per_tick.as_secs_f64() * 1000.0
        );
    }
    Ok(())
}
