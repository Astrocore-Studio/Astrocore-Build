//! Взаимодействие игрока с миром. Пока перенесено одно: клавиша «использовать в мире»
//! (E) открывает и закрывает интерфейс устройства (`ActivatableUISystem` SS14 @ 2f4a2aad).

use astrocore_ecs::{GameWorldExt, RegistryWorldExt};
use astrocore_input::{BoundKeyState, InputPlugin, InputWorldExt};
use astrocore_map::TransformWorldExt;
use astrocore_ui::{UiPlugin, UiWorldExt, UserInterface};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

/// Функция ввода «использовать в мире» (`ContentKeyFunctions.ActivateItemInWorld`).
pub const ACTIVATE_IN_WORLD: &str = "ActivateItemInWorld";

/// Дальность взаимодействия в тайлах (`SharedInteractionSystem.InteractionRange`).
pub const INTERACTION_RANGE: f32 = 1.5;

/// Интерфейс сущности открывается взаимодействием. В данных — компонент `ActivatableUI`.
#[derive(Component, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivatableUi {
    /// Ключ интерфейса в компоненте `UserInterface`.
    pub key: String,
    /// Интерфейс открыт только у одного игрока.
    #[serde(default)]
    pub single_user: bool,
}

/// Игрок достаёт до сущности: та же карта и этаж, не дальше дальности взаимодействия.
/// Преграды между ними пока не учитываются (`InRangeUnobstructed` — задача 3.1).
pub fn in_range(world: &World, player: Entity, target: Entity) -> bool {
    match (world.map_coordinates(player), world.map_coordinates(target)) {
        (Some(a), Some(b)) => {
            a.map == b.map && a.z == b.z && a.position.distance(b.position) <= INTERACTION_RANGE
        }
        _ => false,
    }
}

/// Открывает интерфейс сущности игроку или закрывает уже открытый.
fn toggle_ui(world: &mut World, player: Entity, target: Entity) -> bool {
    let Some(activatable) = world.get::<ActivatableUi>(target).cloned() else {
        return false;
    };
    if !in_range(world, player, target) {
        return false;
    }
    if world.ui_open_for(target, &activatable.key, player) {
        return world.close_ui(target, &activatable.key, player);
    }
    let busy = activatable.single_user
        && world
            .get::<UserInterface>(target)
            .is_some_and(|ui| !ui.actors(&activatable.key).is_empty());
    !busy && world.open_ui(target, &activatable.key, player)
}

/// Подключает взаимодействие. На сервере функции ввода исполняются, на клиенте только
/// регистрируются компоненты и сетевые сообщения.
pub fn install(world: &mut World, server: bool) {
    world.add_plugin(InputPlugin);
    world.add_plugin(UiPlugin);
    world.register_data_component_as::<ActivatableUi>("ActivatableUI");
    if server {
        world.bind_input(ACTIVATE_IN_WORLD, |world, args| {
            match (args.state, args.player, args.target) {
                (BoundKeyState::Down, Some(player), Some(target)) => {
                    toggle_ui(world, player, target)
                }
                _ => false,
            }
        });
    }
}
