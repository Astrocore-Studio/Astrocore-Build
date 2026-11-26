//! Теги сущностей: короткие метки, по которым системы и белые списки узнают сущность
//! («Crowbar», «Wall», «Write»). Образец — `Content.Shared/Tag` SS14 @ 2f4a2aad
//! (`TagComponent`, `TagSystem`).

use std::collections::BTreeSet;

use astrocore_ecs::RegistryWorldExt;
use astrocore_net::NetRegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

/// Теги сущности. В данных — компонент `Tag` с полем `tags`.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    #[serde(default)]
    pub tags: BTreeSet<String>,
}

/// Теги на `World`.
pub trait TagWorldExt {
    fn has_tag(&self, entity: Entity, tag: &str) -> bool;

    /// Есть хотя бы один из тегов. Пустой список — `false`.
    fn has_any_tag<'a>(&self, entity: Entity, tags: impl IntoIterator<Item = &'a str>) -> bool;

    /// Есть все теги. Пустой список — `true`.
    fn has_all_tags<'a>(&self, entity: Entity, tags: impl IntoIterator<Item = &'a str>) -> bool;

    /// Добавляет тег; компонент создаётся при необходимости. `true` — тега не было.
    fn add_tag(&mut self, entity: Entity, tag: &str) -> bool;

    /// Убирает тег. `true` — тег был.
    fn remove_tag(&mut self, entity: Entity, tag: &str) -> bool;
}

impl TagWorldExt for World {
    fn has_tag(&self, entity: Entity, tag: &str) -> bool {
        self.get::<Tag>(entity)
            .is_some_and(|t| t.tags.contains(tag))
    }

    fn has_any_tag<'a>(&self, entity: Entity, tags: impl IntoIterator<Item = &'a str>) -> bool {
        let Some(own) = self.get::<Tag>(entity) else {
            return false;
        };
        tags.into_iter().any(|tag| own.tags.contains(tag))
    }

    fn has_all_tags<'a>(&self, entity: Entity, tags: impl IntoIterator<Item = &'a str>) -> bool {
        let own = self.get::<Tag>(entity);
        tags.into_iter()
            .all(|tag| own.is_some_and(|own| own.tags.contains(tag)))
    }

    fn add_tag(&mut self, entity: Entity, tag: &str) -> bool {
        if self.has_tag(entity, tag) {
            return false;
        }
        match self.get_mut::<Tag>(entity) {
            Some(mut own) => own.tags.insert(tag.to_owned()),
            None => {
                let Ok(mut target) = self.get_entity_mut(entity) else {
                    return false;
                };
                target.insert(Tag {
                    tags: BTreeSet::from([tag.to_owned()]),
                });
                true
            }
        }
    }

    fn remove_tag(&mut self, entity: Entity, tag: &str) -> bool {
        if !self.has_tag(entity, tag) {
            return false;
        }
        self.get_mut::<Tag>(entity)
            .is_some_and(|mut own| own.tags.remove(tag))
    }
}

pub(crate) fn install(world: &mut World) {
    world.register_data_component::<Tag>();
    world.register_net_serde::<Tag>();
}
