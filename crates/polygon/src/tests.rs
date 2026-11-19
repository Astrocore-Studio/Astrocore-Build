use astrocore_editor::validate::{Severity, validate_map};
use astrocore_math::GridCell;
use astrocore_spatial::Topology;

use super::*;
use astrocore_math::glam::Vec3;
use astrocore_world::FloorId;

fn world_with_items() -> (World, PrototypeRegistry) {
    let registry = load_registry(&content_dir()).unwrap();
    let mut world = build_world().unwrap();
    place_items(&mut world, &registry).unwrap();
    (world, registry)
}

#[test]
fn the_map_passes_the_engine_validator_without_errors() {
    let (world, _) = world_with_items();
    let findings = validate_map(&world);
    let errors: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(world.element_count() > 80);
    assert_eq!(world.floors().count(), 2);
}

#[test]
fn two_rooms_are_joined_by_a_door_and_a_stair_reaches_the_upper_floor() {
    let (world, _) = world_with_items();
    let topo = Topology::build(&world, FloorId(0)).unwrap();
    assert_eq!(topo.interior_rooms().count(), 2, "комнаты A и B");
    assert_eq!(topo.portals.len(), 1, "одна дверь между ними");
    assert!(
        world
            .vertical_links()
            .iter()
            .any(|l| l.lower.0 == FloorId(0) && l.upper.0 == FloorId(1)),
        "лестница ведёт на второй этаж"
    );
    // Двери и стены используют материалы SS14.
    let materials: std::collections::BTreeSet<_> =
        world.elements().map(|e| e.material.to_string()).collect();
    for m in [WALL, FLOOR, DOOR] {
        assert!(materials.contains(m), "{m} не использован: {materials:?}");
    }
}

#[test]
fn items_are_spread_deterministically_and_every_spawn_is_inside_the_map() {
    let (a, registry) = world_with_items();
    let (b, _) = world_with_items();
    assert_eq!(a.content_hash(), b.content_hash(), "одна карта — один хеш");
    assert_eq!(a.authored_count(), map().items.len() + map().zones.len());
    let ids: Vec<_> = a.authored().map(|o| o.id).collect();
    assert_eq!(
        ids.len(),
        ids.iter().collect::<std::collections::BTreeSet<_>>().len()
    );
    // Точка появления лежит в комнате A и на этаже 0.
    let topo = Topology::build(&a, FloorId(0)).unwrap();
    let spawn_cell = GridCell::from_world(Vec3::from_array(spawn()), 1.0);
    assert!(
        topo.room_at(spawn_cell).is_some(),
        "спавн в закрытой комнате"
    );
    // Неизвестный предмет — ошибка, а не пропуск.
    let mut empty = PrototypeRegistry::new();
    empty.load_ron_str("[(id: \"Other\")]", "t").unwrap();
    assert!(matches!(
        place_items(&mut build_world().unwrap(), &empty),
        Err(PolygonError::UnknownItem(_))
    ));
    drop(registry);
}

#[test]
fn content_prototypes_are_valid_and_every_item_has_a_sprite_that_exists() {
    let registry = load_registry(&content_dir()).unwrap();
    let sprites = sprites_of(&registry).unwrap();
    for name in [
        "Crowbar",
        "Wrench",
        "Screwdriver",
        "Welder",
        "OxygenCanister",
        "GasValve",
        "PlasmaTank",
        "Pen",
        "Box",
        "ClothingBackpack",
        "Brutepack",
        "Ointment",
        "Gauze",
        "FoodBurgerPlain",
        "DrinkWaterBottleFull",
    ] {
        assert!(registry.spawnable(name).is_ok(), "{name}");
    }
    assert!(sprites.len() >= 126, "{}", sprites.len());
    // Манифест строится только из существующих файлов.
    let json = render_manifest(&content_dir(), &registry).unwrap();
    for needle in [
        "ss14:crowbar",
        "ss14:clothing_backpack",
        "Textures/Structures/Walls/solid.rsi/full.png",
        "Textures/Tiles/Basic/Dark/dark.png",
    ] {
        assert!(json.contains(needle), "нет {needle}");
    }
}

#[test]
fn the_committed_manifest_is_up_to_date() {
    let registry = load_registry(&content_dir()).unwrap();
    let fresh = render_manifest(&content_dir(), &registry).unwrap();
    let committed = std::fs::read_to_string(assets_dir().join("render-manifest.json"))
        .expect("assets/ss14/render-manifest.json: `server --write-manifest assets/ss14/render-manifest.json`");
    assert_eq!(
        fresh.trim(),
        committed.trim(),
        "манифест устарел: перегенерируйте его"
    );
}

/// Дерево прототипов сущностей SS14 лежит в `content/prototypes/ss14` целиком: с наследованием,
/// абстрактными узлами, названиями, описаниями и категориями.
#[test]
fn the_ss14_entity_tree_is_part_of_the_content() {
    let registry = load_registry(&content_dir()).unwrap();
    assert!(registry.len() > 10_000, "прототипов: {}", registry.len());
    // Вариант предмета наследует компоненты родителя и держит свои название и описание.
    let child = registry.resolve("AccessConfiguratorUniversal").unwrap();
    assert!(child.component("Item").is_some(), "Item от родителя");
    assert!(child.name.is_some() && child.description.is_some());
    // Абстрактные узлы не создаются, но разрешаются.
    let abstract_nodes = registry
        .ids()
        .filter(|id| registry.spawnable(id).is_err() && registry.resolve(id).is_ok())
        .count();
    assert!(abstract_nodes > 500, "абстрактных узлов: {abstract_nodes}");
    // Категории SS14 сохраняются и наследуются.
    let hidden = registry
        .ids()
        .filter(|id| {
            registry
                .resolve(id)
                .is_ok_and(|p| p.categories.iter().any(|c| c == "HideSpawnMenu"))
        })
        .count();
    assert!(hidden > 100, "скрытых из меню: {hidden}");
}
