//! Проверка взрыва на карте раунда: сервер с картой работает секунду, затем у точки
//! появления взрывается заряд силы `сила` (по умолчанию 2000, наклон 5, предел 100).
//! Печатает площадь, разрушенные сущности и сломанный пол, время тактов обработки и
//! давление станции через 10 секунд.
//!
//! `cargo run --release -p astrocore-game --example check_explosion [-- 2000]`

use std::time::Instant;

use astrocore_ecs::{GameWorldExt, MetaData};
use astrocore_game::atmos::GridAtmosphere;
use astrocore_game::explosion::{Explosions, queue_explosion};
use astrocore_game::server;
use astrocore_map::{MapGrid, TransformWorldExt};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

fn counts(world: &mut World) -> (usize, usize, usize) {
    let entities = world.query::<&MetaData>().iter(world).count();
    let tiles = world
        .query::<&MapGrid>()
        .iter(world)
        .map(|g| g.tile_count() as usize)
        .sum();
    let pressurized = world
        .query::<&GridAtmosphere>()
        .iter(world)
        .flat_map(|a| {
            a.live_tiles()
                .map(|(_, t)| {
                    t.air
                        .as_ref()
                        .is_some_and(|air| !t.space && air.pressure() > 50.0)
                })
                .collect::<Vec<_>>()
        })
        .filter(|p| *p)
        .count();
    (entities, tiles, pressurized)
}

fn floor_types(world: &mut World) -> std::collections::HashMap<(i32, i32), u16> {
    let mut map = std::collections::HashMap::new();
    for grid in world.query::<&MapGrid>().iter(world) {
        for (coord, tile) in grid.tiles_on_level(0) {
            map.insert((coord.x, coord.y), tile.type_id);
        }
    }
    map
}

fn main() -> Result<(), String> {
    let intensity: f32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000.0);
    let mut start = server::start()?;
    let world = &mut start.world;
    for _ in 0..30 {
        world.run_tick();
    }
    let t = Instant::now();
    astrocore_game::node_container::rebuild(world);
    println!(
        "Пересборка сетей узлов: {:.1} мс",
        t.elapsed().as_secs_f64() * 1000.0
    );
    let t = Instant::now();
    world.run_tick();
    println!(
        "Такт после пересборки: {:.1} мс",
        t.elapsed().as_secs_f64() * 1000.0
    );
    let spawn = world
        .query::<(Entity, &MetaData)>()
        .iter(world)
        .find(|(_, m)| m.prototype.as_deref() == Some("SpawnPointLatejoin"))
        .map(|(e, _)| e)
        .ok_or("нет точки появления")?;
    let at = world.map_coordinates(spawn).ok_or("точка вне карты")?;
    if let Some(coords) = world.coordinates(spawn) {
        let net = world
            .get::<astrocore_net::NetEntity>(coords.parent)
            .map(|n| n.0);
        println!(
            "Эпицентр: карта {:?} {:?}, грид {net:?} точка {:?}",
            at.map, at.position, coords.position
        );
    }
    let (entities, tiles, pressurized) = counts(world);
    let floor_before = floor_types(world);
    let alive_before: Vec<Entity> = world.query::<Entity>().iter(world).collect();
    println!("До взрыва: сущностей {entities}, тайлов {tiles}, клеток под давлением {pressurized}");
    queue_explosion(
        world,
        at,
        "Default",
        intensity,
        5.0,
        100.0,
        None,
        1.0,
        i32::MAX,
        true,
    );
    let mut ticks = 0;
    let mut worst = std::time::Duration::ZERO;
    let mut area = 0;
    let started = Instant::now();
    loop {
        let tick = Instant::now();
        world.run_tick();
        worst = worst.max(tick.elapsed());
        println!(
            "  такт {ticks}: {:.1} мс",
            tick.elapsed().as_secs_f64() * 1000.0
        );
        ticks += 1;
        if let Some(active) = &world.resource::<Explosions>().active {
            area = active.area;
        }
        let explosions = world.resource::<Explosions>();
        if explosions.active.is_none() && explosions.queue.is_empty() || ticks > 3000 {
            break;
        }
    }
    let (after_entities, after_tiles, _) = counts(world);
    let damaged = world
        .query::<&astrocore_game::damage::Damageable>()
        .iter(world)
        .filter(|d| d.total_damage.float() > 0.0)
        .count();
    let destroyed = alive_before
        .iter()
        .filter(|e| world.get_entity(**e).is_err())
        .count();
    println!("Ранено сущностей {damaged}, удалено {destroyed}");
    println!(
        "Взрыв силы {intensity}: площадь {area} клеток, обработан за {ticks} тактов ({:.0} мс), худший такт {:.1} мс",
        started.elapsed().as_secs_f64() * 1000.0,
        worst.as_secs_f64() * 1000.0
    );
    println!(
        "Сущностей стало {after_entities} (было {entities}), пробито до космоса тайлов {}",
        tiles.saturating_sub(after_tiles)
    );
    let floor_after = floor_types(world);
    let changed_floor = floor_before
        .iter()
        .filter(|(k, v)| floor_after.get(*k) != Some(*v))
        .count();
    println!("Сменилось клеток пола: {changed_floor}");
    for i in 0..300 {
        let tick = Instant::now();
        world.run_tick();
        if i < 3 {
            println!(
                "  после взрыва, такт {}: {:.1} мс",
                i + 1,
                tick.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
    let (_, _, pressurized) = counts(world);
    println!("Через 10 с: клеток под давлением {pressurized}");
    Ok(())
}
