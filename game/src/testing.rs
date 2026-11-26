//! Мир сервера для юнит-тестов систем: плагины движка, системы контента, прототипы из
//! YAML в формате SS14 и маленькая станция.

use astrocore_data::DataFormat;
use astrocore_ecs::{ComponentRegistry, GameWorldExt};
use astrocore_map::{EntityCoordinates, MapWorldExt, SpawnWorldExt, Tile, TileCoord};
use astrocore_math::Vec2;
use astrocore_movement::MovementServerPlugin;
use astrocore_net::NetServerPlugin;
use astrocore_physics::PhysicsPlugin;
use astrocore_prototypes::{LoadReport, Prototypes};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::{Side, content, install_content};

/// Мир сервера с системами контента и прототипами из текста.
pub fn world_with(prototypes: &str) -> World {
    content::register_names();
    let mut world = World::new();
    world.add_plugin(NetServerPlugin);
    world.add_plugin(PhysicsPlugin::SERVER);
    world.add_plugin(MovementServerPlugin);
    install_content(&mut world, Side::Server);
    let mut loaded = Prototypes::default();
    let mut report = LoadReport::default();
    loaded.load_str("test.yml", DataFormat::Yaml, prototypes, &mut report);
    loaded.resolve(world.resource::<ComponentRegistry>(), &mut report);
    assert!(
        report.errors.is_empty(),
        "прототипы теста: {:?}",
        report.errors
    );
    world.insert_resource(loaded);
    world
}

/// Станция 16×16 тайлов пола; возвращает грид.
pub fn station(world: &mut World) -> Entity {
    let (map, _) = world.create_map(true);
    let grid = world.create_grid(map);
    let floor = world
        .resource::<astrocore_map::TileDefinitions>()
        .tile("Plating")
        .unwrap_or(Tile::new(1));
    world.set_tiles(
        grid,
        (0..16).flat_map(|x| (0..16).map(move |y| (TileCoord::new(x, y, 0), floor))),
    );
    grid
}

/// Создаёт сущность прототипа в центре тайла `(x, y)` грида.
pub fn spawn_at(world: &mut World, prototype: &str, grid: Entity, x: i32, y: i32) -> Entity {
    world
        .spawn_attached(
            prototype,
            EntityCoordinates::new(grid, Vec2::new(x as f32 + 0.5, y as f32 + 0.5)),
        )
        .unwrap_or_else(|error| panic!("{prototype}: {error}"))
}
