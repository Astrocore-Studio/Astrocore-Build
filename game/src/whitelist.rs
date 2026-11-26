//! Белые списки сущностей: условие «подходит ли сущность» из данных SS14 — по компонентам,
//! размеру предмета и тегам. Образец — `Content.Shared/Whitelist` SS14 @ 2f4a2aad
//! (`EntityWhitelist`, `EntityWhitelistSystem.IsValid`).

use astrocore_ecs::{ComponentRegistry, MetaData};
use astrocore_prototypes::Prototypes;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::tag::TagWorldExt;

/// Белый список. В данных — поля `components`, `sizes`, `tags`, `requireAll`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityWhitelist {
    #[serde(default)]
    pub components: Option<Vec<String>>,
    /// Размеры предмета (`ItemSize`): подходит предмет одного из этих размеров.
    #[serde(default)]
    pub sizes: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Нужны все компоненты и все теги, а не любой из них.
    #[serde(default)]
    pub require_all: bool,
}

/// У сущности есть компонент с именем из данных. Компонент, ещё не перенесённый в Astrocore,
/// ищется в прототипе сущности (ADR-0005, п. 7).
pub fn has_component_named(world: &World, entity: Entity, name: &str) -> bool {
    let registered = world
        .get_resource::<ComponentRegistry>()
        .and_then(|registry| registry.get(name));
    if let Some(info) = registered {
        return world
            .get_entity(entity)
            .is_ok_and(|target| target.contains_id(info.id));
    }
    let prototype = world
        .get::<MetaData>(entity)
        .and_then(|meta| meta.prototype.clone());
    match (prototype, world.get_resource::<Prototypes>()) {
        (Some(prototype), Some(prototypes)) => prototypes
            .entity_or_abstract(&prototype)
            .is_some_and(|proto| proto.component(name).is_some()),
        _ => false,
    }
}

impl EntityWhitelist {
    /// Сущность подходит под список (`EntityWhitelistSystem.IsValid`). Порядок проверок —
    /// как в SS14: компоненты, размер, теги; список без тегов при `requireAll` пропускает
    /// сущность, у которой нашлись все компоненты.
    pub fn is_valid(&self, world: &World, entity: Entity) -> bool {
        if let Some(components) = &self.components {
            for name in components {
                if has_component_named(world, entity, name) {
                    if !self.require_all {
                        return true;
                    }
                } else if self.require_all {
                    return false;
                }
            }
        }
        if let (Some(sizes), Some(size)) = (&self.sizes, crate::item::item_size(world, entity))
            && sizes.contains(&size)
        {
            return true;
        }
        if let Some(tags) = &self.tags {
            let tags = tags.iter().map(String::as_str);
            return if self.require_all {
                world.has_all_tags(entity, tags)
            } else {
                world.has_any_tag(entity, tags)
            };
        }
        self.require_all
    }
}

/// Сущность не в чёрном списке и в белом (`CheckBoth`). Отсутствующий список не мешает.
pub fn check_both(
    world: &World,
    entity: Entity,
    blacklist: Option<&EntityWhitelist>,
    whitelist: Option<&EntityWhitelist>,
) -> bool {
    if blacklist.is_some_and(|list| list.is_valid(world, entity)) {
        return false;
    }
    whitelist.is_none_or(|list| list.is_valid(world, entity))
}

#[cfg(test)]
mod tests {
    use astrocore_data::{from_node, parse_yaml};
    use astrocore_prototypes::PrototypeWorldExt;

    use super::*;
    use crate::tag::Tag;
    use crate::testing::{spawn_at, station, world_with};

    const PROTOTYPES: &str = "
- type: entity
  id: Crowbar
  components:
  - type: Tag
    tags: [Crowbar, Multitool]
  - type: Paper
- type: entity
  id: Rock
";

    fn list(yaml: &str) -> EntityWhitelist {
        from_node(&parse_yaml(yaml).unwrap()).unwrap()
    }

    #[test]
    fn tags_components_and_require_all_follow_ss14() {
        let mut world = world_with(PROTOTYPES);
        let grid = station(&mut world);
        let crowbar = spawn_at(&mut world, "Crowbar", grid, 1, 1);
        let rock = spawn_at(&mut world, "Rock", grid, 2, 1);
        assert!(world.get::<Tag>(crowbar).is_some());

        let any = list("tags: [Crowbar, Wrench]");
        assert!(any.is_valid(&world, crowbar));
        assert!(!any.is_valid(&world, rock));
        let all = list("tags: [Crowbar, Wrench]\nrequireAll: true");
        assert!(!all.is_valid(&world, crowbar));
        assert!(list("tags: [Crowbar, Multitool]\nrequireAll: true").is_valid(&world, crowbar));

        // Компонент движка — по сущности, неперенесённый `Paper` — по прототипу.
        assert!(list("components: [Transform]").is_valid(&world, rock));
        assert!(list("components: [Paper]").is_valid(&world, crowbar));
        assert!(!list("components: [Paper]").is_valid(&world, rock));
        // Все компоненты есть, тегов в списке нет: при `requireAll` — подходит.
        assert!(list("components: [Paper, Transform]\nrequireAll: true").is_valid(&world, crowbar));
        assert!(!list("components: [Paper, Transform]\nrequireAll: true").is_valid(&world, rock));
        // Пустой список ничего не пропускает, если не нужны «все».
        assert!(!EntityWhitelist::default().is_valid(&world, crowbar));

        let crowbars = any.clone();
        assert!(check_both(&world, crowbar, None, Some(&crowbars)));
        assert!(!check_both(&world, crowbar, Some(&crowbars), None));
        assert!(check_both(&world, rock, Some(&crowbars), None));
        assert!(!check_both(&world, rock, None, Some(&crowbars)));
    }

    #[test]
    fn tags_can_be_added_and_removed() {
        let mut world = world_with(PROTOTYPES);
        let rock = world.spawn_prototype("Rock").unwrap();
        assert!(world.add_tag(rock, "Ore"));
        assert!(!world.add_tag(rock, "Ore"));
        assert!(world.has_tag(rock, "Ore"));
        assert!(world.has_all_tags(rock, ["Ore"]));
        assert!(world.has_all_tags(rock, []));
        assert!(!world.has_any_tag(rock, []));
        assert!(world.remove_tag(rock, "Ore"));
        assert!(!world.remove_tag(rock, "Ore"));
    }
}
