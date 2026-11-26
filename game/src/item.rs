//! Предметы: то, что можно взять в руку. Образец — `Content.Shared/Item` SS14 @ 2f4a2aad
//! (`ItemComponent`, `ItemSizePrototype`).

use astrocore_ecs::RegistryWorldExt;
use astrocore_net::NetRegistryWorldExt;
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

fn default_size() -> String {
    "Small".to_owned()
}

/// Слой спрайта предмета в руке или на одежде (`PrototypeLayerData`): заданные поля
/// заменяют поля слоя.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ItemLayer {
    pub sprite: Option<String>,
    pub state: Option<String>,
    pub texture: Option<String>,
    pub color: Option<astrocore_math::Color>,
    pub visible: Option<bool>,
    pub scale: Option<astrocore_math::Vec2>,
    pub offset: Option<astrocore_math::Vec2>,
    pub rotation: Option<astrocore_math::Angle>,
    pub shader: Option<String>,
    pub map: Option<Vec<String>>,
}

/// Предмет. В данных — компонент `Item`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// Размер: прототип вида `itemSize` (`Tiny`, `Small`, `Normal`, `Large`, `Huge`, `Ginormous`).
    #[serde(default = "default_size")]
    pub size: String,
    /// Набор RSI для спрайта в руке; без него — набор спрайта сущности.
    #[serde(default)]
    pub sprite: Option<String>,
    /// Начало имени состояний в руке (`inhand-left`, `<prefix>-inhand-left`).
    #[serde(default)]
    pub held_prefix: Option<String>,
    /// Слои в руке по сторонам (`left`, `right`) вместо состояний по умолчанию.
    #[serde(default)]
    pub inhand_visuals: std::collections::BTreeMap<String, Vec<ItemLayer>>,
    /// Форма в хранилище; без неё — форма размера (`defaultShape`).
    #[serde(default)]
    pub shape: Option<Vec<crate::storage::CellBox>>,
    /// Поворот значка в хранилище, градусы.
    #[serde(default)]
    pub stored_rotation: f32,
}

impl Default for Item {
    fn default() -> Self {
        Self {
            size: default_size(),
            sprite: None,
            held_prefix: None,
            inhand_visuals: std::collections::BTreeMap::new(),
            shape: None,
            stored_rotation: 0.0,
        }
    }
}

/// Размер предмета (`ItemSize`) или `None`, если сущность не предмет.
pub fn item_size(world: &World, entity: Entity) -> Option<String> {
    world.get::<Item>(entity).map(|item| item.size.clone())
}

/// Вес размера (`ItemSizePrototype.Weight`): сколько места предмет занимает в хранилище.
/// Неизвестный размер весит 1.
pub fn size_weight(world: &World, size: &str) -> i32 {
    world
        .get_resource::<Prototypes>()
        .and_then(|prototypes| prototypes.resolved_node("itemSize", size))
        .and_then(|node| node.get("weight"))
        .and_then(|weight| weight.as_str())
        .and_then(|weight| weight.parse().ok())
        .unwrap_or(1)
}

pub(crate) fn install(world: &mut World) {
    world.register_data_component::<Item>();
    world.register_net_serde::<Item>();
}
