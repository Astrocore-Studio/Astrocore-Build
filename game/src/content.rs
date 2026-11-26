//! Контент игры: каталоги ресурсов, таблицы имён SS14 и загрузка прототипов.

use std::path::{Path, PathBuf};

use astrocore_console::{CVarDef, CVarFlags, ConfigWorldExt};
use astrocore_ecs::ComponentRegistry;
use astrocore_loc::{EntityNameWorldExt, Loc};
use astrocore_map::{EntityMigrations, TileDefinitions};
use astrocore_net::register_visibility_layer;
use astrocore_physics::register_collision_group;
use astrocore_prototypes::{LoadReport, Prototypes};
use astrocore_resources::{Resources, ResourcesWorldExt};
use bevy_ecs::world::{Mut, World};

/// Карта раунда: путь от каталога ресурсов.
pub const MAP: CVarDef<&str> = CVarDef::new(
    "game.map",
    "Maps/saltern.yml",
    CVarFlags::SERVERONLY.union(CVarFlags::ARCHIVE),
    "Карта раунда: путь от каталога ресурсов.",
);

/// Прототип сущности, которую получает вошедший игрок.
pub const PLAYER_PROTOTYPE: CVarDef<&str> = CVarDef::new(
    "game.player_prototype",
    "MobCorgi",
    CVarFlags::SERVERONLY.union(CVarFlags::ARCHIVE),
    "Прототип сущности игрока. Человек появится с переносом внешности гуманоидов (задача 3.10).",
);

/// Каталог `Resources` Space Station 14 по умолчанию: референс рядом с чекаутом Build
/// (`scripts/fetch_reference.sh`).
pub const DEFAULT_SS14_RESOURCES: &str = "../reference/space-station-14/Resources";

/// Каталоги контента: свой каталог `Resources` игры, затем каталог SS14. Каталог SS14
/// задаёт переменная окружения `ASTRO_SS14_RESOURCES`.
pub fn mount_resources() -> Result<(Resources, PathBuf), String> {
    let ss14 =
        std::env::var("ASTRO_SS14_RESOURCES").unwrap_or_else(|_| DEFAULT_SS14_RESOURCES.into());
    let ss14 = PathBuf::from(ss14);
    if !ss14.join("Prototypes").is_dir() {
        return Err(format!(
            "каталог ресурсов SS14 не найден: {}. Запустите scripts/fetch_reference.sh или задайте ASTRO_SS14_RESOURCES.",
            ss14.display()
        ));
    }
    let mut resources = Resources::default();
    if Path::new("Resources").is_dir() {
        resources.mount("Resources");
    }
    resources.mount(&ss14);
    Ok((resources, ss14))
}

/// Группы столкновений SS14 (`Content.Shared/Physics/CollisionGroup.cs` @ 2f4a2aad).
/// Биты групп столкновений SS14 (`Content.Shared/Physics/CollisionGroup.cs` @ 2f4a2aad).
pub mod groups {
    pub const OPAQUE: i32 = 1 << 0;
    pub const IMPASSABLE: i32 = 1 << 1;
    pub const MID: i32 = 1 << 2;
    pub const HIGH: i32 = 1 << 3;
    pub const LOW: i32 = 1 << 4;
    pub const GHOST: i32 = 1 << 5;
    pub const BULLET: i32 = 1 << 6;
    pub const INTERACT: i32 = 1 << 7;
    pub const DOOR: i32 = 1 << 8;
    /// Маска луча досягаемости (`SharedInteractionSystem.InRangeUnobstructedMask`).
    pub const IN_RANGE_UNOBSTRUCTED: i32 = IMPASSABLE | INTERACT;
}

pub fn register_collision_groups() {
    use groups::*;
    for (name, bits) in [
        ("None", 0),
        ("Opaque", OPAQUE),
        ("Impassable", IMPASSABLE),
        ("MidImpassable", MID),
        ("HighImpassable", HIGH),
        ("LowImpassable", LOW),
        ("GhostImpassable", GHOST),
        ("BulletImpassable", BULLET),
        ("InteractImpassable", INTERACT),
        ("DoorPassable", DOOR),
        ("MapGrid", 1 << 30),
        ("AllMask", -1),
        (
            "SingularityLayer",
            OPAQUE | IMPASSABLE | MID | HIGH | LOW | BULLET | INTERACT | DOOR,
        ),
        ("MobMask", IMPASSABLE | HIGH | MID | LOW),
        ("MobLayer", OPAQUE | BULLET),
        ("SmallMobMask", IMPASSABLE | LOW),
        ("SmallMobLayer", OPAQUE | BULLET),
        ("FlyingMobMask", IMPASSABLE | HIGH),
        ("FlyingMobLayer", OPAQUE | BULLET),
        ("LargeMobMask", IMPASSABLE | HIGH | MID | LOW),
        ("LargeMobLayer", OPAQUE | HIGH | MID | LOW | BULLET),
        ("MachineMask", IMPASSABLE | MID | LOW),
        ("MachineLayer", OPAQUE | MID | LOW | BULLET),
        ("ConveyorMask", IMPASSABLE | MID | LOW | DOOR),
        ("CrateMask", IMPASSABLE | HIGH | LOW),
        ("TableMask", IMPASSABLE | MID),
        ("TableLayer", MID),
        ("TabletopMachineMask", IMPASSABLE | HIGH),
        ("TabletopMachineLayer", OPAQUE | BULLET),
        ("GlassAirlockLayer", HIGH | MID | BULLET | INTERACT),
        ("AirlockLayer", OPAQUE | HIGH | MID | BULLET | INTERACT),
        ("HumanoidBlockLayer", HIGH | MID),
        ("SlipLayer", MID | LOW),
        ("ItemMask", IMPASSABLE | HIGH),
        ("ThrownItem", IMPASSABLE | HIGH | BULLET),
        (
            "WallLayer",
            OPAQUE | IMPASSABLE | HIGH | MID | LOW | BULLET | INTERACT,
        ),
        (
            "GlassLayer",
            IMPASSABLE | HIGH | MID | LOW | BULLET | INTERACT,
        ),
        ("HalfWallLayer", MID | LOW),
        ("FlimsyLayer", OPAQUE | HIGH | MID | LOW | INTERACT),
        ("SpecialWallLayer", OPAQUE | HIGH | MID | LOW | BULLET),
        ("FullTileMask", IMPASSABLE | HIGH | MID | LOW | INTERACT),
        (
            "FullTileLayer",
            OPAQUE | HIGH | MID | LOW | BULLET | INTERACT,
        ),
        ("SubfloorMask", IMPASSABLE | LOW),
    ] {
        register_collision_group(name, bits);
    }
}

/// Глубины отрисовки SS14 (`Content.Shared/DrawDepth/DrawDepth.cs` @ 2f4a2aad).
pub const DRAW_DEPTHS: [(&str, i32); 29] = [
    ("LowFloors", -22),
    ("ThickPipe", -21),
    ("ThickWire", -20),
    ("ThinPipe", -17),
    ("ThinWire", -16),
    ("BelowFloor", -15),
    ("FloorTiles", -14),
    ("FloorObjects", -13),
    ("Puddles", -12),
    ("HighFloorObjects", -11),
    ("DeadMobs", -10),
    ("SmallMobs", -9),
    ("Walls", -8),
    ("WallTops", -1),
    ("Objects", 0),
    ("SmallObjects", 1),
    ("WallMountedItems", 2),
    ("LargeObjects", 3),
    ("Items", 4),
    ("BelowMobs", 5),
    ("Mobs", 6),
    ("OverMobs", 7),
    ("Doors", 8),
    ("BlastDoors", 9),
    ("Overdoors", 10),
    ("Gasses", 17),
    ("Effects", 18),
    ("Ghosts", 19),
    ("Overlays", 20),
];

/// Загружает прототипы из каталогов ресурсов и из прототипов движка Robust, определения
/// тайлов из прототипов вида `tile` и таблицу переименований `migration.yml`.
pub fn load_content(world: &mut World, resources: &Resources, ss14: &Path) -> LoadReport {
    world.insert_resource(resources.clone());
    let mut report = world.load_prototypes_from_resources();
    // Прототипы движка Robust лежат рядом с контентом SS14.
    let engine = ss14.join("../RobustToolbox/Resources/EnginePrototypes");
    if engine.is_dir() {
        world.resource_scope(|world, mut prototypes: Mut<Prototypes>| {
            prototypes.load_dir(&engine, &mut report);
            prototypes.resolve(world.resource::<ComponentRegistry>(), &mut report);
        });
    }
    let tiles: Vec<(String, Option<String>, u8, f32)> = {
        let prototypes = world.resource::<Prototypes>();
        prototypes
            .ids("tile")
            .filter_map(|id| {
                let node = prototypes.resolved_node("tile", id)?;
                let sprite = node
                    .get("sprite")
                    .and_then(|n| n.as_str())
                    .map(str::to_owned);
                let variants = node
                    .get("variants")
                    .and_then(|n| n.as_str())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                let friction = node
                    .get("friction")
                    .and_then(|n| n.as_str())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1.0);
                Some((id.to_owned(), sprite, variants, friction))
            })
            .collect()
    };
    let mut frictions = crate::friction::TileFrictions::default();
    for (id, _, _, friction) in &tiles {
        frictions.set(id, *friction);
    }
    world.insert_resource(frictions);
    let mut definitions = world.get_resource_or_insert_with(TileDefinitions::default);
    for (id, sprite, variants, _) in tiles {
        definitions.register(&id, sprite, variants);
    }
    load_locale(world, resources);
    if let Ok(text) = resources.read_to_string("migration.yml")
        && let Ok(node) = astrocore_data::parse_yaml(&text)
    {
        world.insert_resource(EntityMigrations::from_node(&node));
    }
    report
}

/// Язык игры по умолчанию: язык контента SS14.
pub const DEFAULT_CULTURE: &str = "en-US";

/// Загружает сообщения языка и считает названия сущностей. Язык задаёт переменная
/// окружения `ASTRO_LANG` (например `ru-RU`, если перевод лежит в `Resources/Locale/ru-RU`
/// игры); чего в нём нет, берётся из языка SS14.
pub fn load_locale(world: &mut World, resources: &Resources) {
    let wanted = std::env::var("ASTRO_LANG").unwrap_or_else(|_| DEFAULT_CULTURE.to_owned());
    let mut loc = Loc::default();
    let mut cultures = vec![wanted.as_str()];
    if wanted != DEFAULT_CULTURE {
        cultures.push(DEFAULT_CULTURE);
    }
    for culture in cultures {
        match loc.load_culture(resources, culture) {
            Ok(report) if report.messages == 0 => {
                eprintln!("язык {culture}: сообщений нет — проверьте каталог Locale/{culture}");
            }
            Ok(_) => {}
            Err(error) => eprintln!("язык {culture}: {error}"),
        }
    }
    world.insert_resource(loc);
    world.build_entity_names();
}

/// Таблицы имён SS14, общие для сервера и клиента: группы столкновений и слои видимости.
/// Вызывается до загрузки прототипов: по этим именам читаются их данные.
pub fn register_names() {
    register_collision_groups();
    register_visibility_layers();
}

/// Слои видимости SS14 (`Content.Shared/Eye/VisibilityFlags.cs` @ 2f4a2aad).
pub fn register_visibility_layers() {
    for (name, bits) in [
        ("None", 0),
        ("Normal", 1 << 0),
        ("Ghost", 1 << 1),
        ("Subfloor", 1 << 2),
        ("Admin", 1 << 3),
    ] {
        register_visibility_layer(name, bits);
    }
}

/// Регистрирует настройки игры.
pub fn register_cvars(world: &mut World) {
    world.register_cvar(&MAP);
    world.register_cvar(&PLAYER_PROTOTYPE);
}
