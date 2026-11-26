//! Можно ли сущности действовать: взаимодействовать, пользоваться предметом в руке,
//! выполнять сложные действия. Образец — `Content.Shared/ActionBlocker/ActionBlockerSystem.cs`
//! SS14 @ 2f4a2aad: системы запрещают действие, отменяя событие попытки.

use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use astrocore_net::NetRegistryWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

/// Сущность умеет сложные действия: пользоваться предметами и устройствами, а не только
/// трогать. В данных — компонент `ComplexInteraction`.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplexInteraction {}

/// Может ли сущность действовать сознательно (`ConsciousAttemptEvent`): мёртвые и
/// без сознания — нет. Поднимается на сущности.
#[derive(Debug, Clone, Copy)]
pub struct ConsciousAttempt {
    pub user: Entity,
    pub cancelled: bool,
}

/// Попытка взаимодействия (`InteractionAttemptEvent`). Поднимается на действующем.
#[derive(Debug, Clone, Copy)]
pub struct InteractionAttempt {
    pub user: Entity,
    pub target: Option<Entity>,
    pub cancelled: bool,
}

/// С сущностью пытаются взаимодействовать (`GettingInteractedWithAttemptEvent`).
/// Поднимается на цели.
#[derive(Debug, Clone, Copy)]
pub struct GettingInteractedWithAttempt {
    pub user: Entity,
    pub target: Entity,
    pub cancelled: bool,
}

/// Попытка воспользоваться предметом в руке (`UseAttemptEvent`). Поднимается на действующем.
#[derive(Debug, Clone, Copy)]
pub struct UseAttempt {
    pub user: Entity,
    pub used: Entity,
    pub cancelled: bool,
}

/// Предметом пытаются воспользоваться (`GettingUsedAttemptEvent`). Поднимается на предмете.
#[derive(Debug, Clone, Copy)]
pub struct GettingUsedAttempt {
    pub user: Entity,
    pub cancelled: bool,
}

/// Попытка поднять предмет (`PickupAttemptEvent`). Поднимается на действующем, затем
/// (`GettingPickedUpAttemptEvent`) на предмете.
#[derive(Debug, Clone, Copy)]
pub struct PickupAttempt {
    pub user: Entity,
    pub item: Entity,
    pub cancelled: bool,
}

/// Попытка выпустить предмет из рук (`DropAttemptEvent`). Поднимается на действующем.
#[derive(Debug, Clone, Copy)]
pub struct DropAttempt {
    pub user: Entity,
    pub cancelled: bool,
}

/// Попытка бросить (`ThrowAttemptEvent`). Поднимается на действующем, затем
/// (`ThrowItemAttemptEvent`) на предмете.
#[derive(Debug, Clone, Copy)]
pub struct ThrowAttempt {
    pub user: Entity,
    pub item: Entity,
    pub cancelled: bool,
}

/// Может ли сущность двигаться сама (`UpdateCanMoveEvent`). Поднимается на сущности.
#[derive(Debug, Clone, Copy)]
pub struct UpdateCanMove {
    pub entity: Entity,
    pub cancelled: bool,
}

/// Пересчитывает `InputMover.can_move` по событию `UpdateCanMove` (`UpdateCanMove`).
pub fn update_can_move(world: &mut World, entity: Entity) -> bool {
    if world
        .get::<astrocore_movement::InputMover>(entity)
        .is_none()
    {
        return false;
    }
    let mut event = UpdateCanMove {
        entity,
        cancelled: false,
    };
    world.raise(entity, &mut event);
    let can_move = !event.cancelled;
    if let Some(mut mover) = world.get_mut::<astrocore_movement::InputMover>(entity)
        && mover.can_move != can_move
    {
        mover.can_move = can_move;
    }
    can_move
}

/// Проверки действий на `World`.
pub trait ActionBlockerWorldExt {
    fn can_consciously_perform_action(&mut self, user: Entity) -> bool;

    /// Может ли сущность взаимодействовать с целью (или вообще, без цели).
    fn can_interact(&mut self, user: Entity, target: Option<Entity>) -> bool;

    fn can_use_held_entity(&mut self, user: Entity, used: Entity) -> bool;

    fn can_complex_interact(&self, user: Entity) -> bool;

    fn can_pickup(&mut self, user: Entity, item: Entity) -> bool;

    fn can_drop(&mut self, user: Entity) -> bool;

    fn can_throw(&mut self, user: Entity, item: Entity) -> bool;
}

/// Поднимает отменяемое событие на действующем и, если нужно, на предмете.
fn attempt<E: Copy + 'static>(
    world: &mut World,
    targets: &[Entity],
    mut event: E,
    cancelled: fn(&E) -> bool,
) -> bool {
    for target in targets {
        world.raise(*target, &mut event);
        if cancelled(&event) {
            return false;
        }
    }
    true
}

impl ActionBlockerWorldExt for World {
    fn can_consciously_perform_action(&mut self, user: Entity) -> bool {
        let mut event = ConsciousAttempt {
            user,
            cancelled: false,
        };
        self.raise(user, &mut event);
        !event.cancelled
    }

    fn can_interact(&mut self, user: Entity, target: Option<Entity>) -> bool {
        if !self.can_consciously_perform_action(user) {
            return false;
        }
        let mut event = InteractionAttempt {
            user,
            target,
            cancelled: false,
        };
        self.raise(user, &mut event);
        if event.cancelled {
            return false;
        }
        let Some(target) = target.filter(|target| *target != user) else {
            return true;
        };
        let mut event = GettingInteractedWithAttempt {
            user,
            target,
            cancelled: false,
        };
        self.raise(target, &mut event);
        !event.cancelled
    }

    fn can_use_held_entity(&mut self, user: Entity, used: Entity) -> bool {
        let mut event = UseAttempt {
            user,
            used,
            cancelled: false,
        };
        self.raise(user, &mut event);
        if event.cancelled {
            return false;
        }
        let mut event = GettingUsedAttempt {
            user,
            cancelled: false,
        };
        self.raise(used, &mut event);
        !event.cancelled
    }

    fn can_complex_interact(&self, user: Entity) -> bool {
        self.get::<ComplexInteraction>(user).is_some()
    }

    fn can_pickup(&mut self, user: Entity, item: Entity) -> bool {
        let event = PickupAttempt {
            user,
            item,
            cancelled: false,
        };
        attempt(self, &[user, item], event, |e| e.cancelled)
    }

    fn can_drop(&mut self, user: Entity) -> bool {
        let event = DropAttempt {
            user,
            cancelled: false,
        };
        attempt(self, &[user], event, |e| e.cancelled)
    }

    fn can_throw(&mut self, user: Entity, item: Entity) -> bool {
        let event = ThrowAttempt {
            user,
            item,
            cancelled: false,
        };
        attempt(self, &[user, item], event, |e| e.cancelled)
    }
}

pub(crate) fn install(world: &mut World) {
    world.register_data_component::<ComplexInteraction>();
    world.register_net_serde::<ComplexInteraction>();
}
