//! Призраки на сервере: появление призрака для разума, попытка стать призраком (с правом
//! вернуться в тело, если тело мертво; в критическом состоянии тело добивается), удаление
//! призрака, когда игрок ушёл или вернулся в тело; призраки видны только призракам и видят
//! друг друга. Образец — `GhostSystem` (`SpawnGhost`, `OnGhostAttempt`, удаление,
//! `OnGhostStartup`, `GetVisMaskEvent`), `SharedGhostSystem`, `GhostComponent`,
//! `GhostHearingComponent`, `CCVars.GhostKillCrit` SS14 @ 2f4a2aad.
//!
//! Команды, возвращение в тело, телепорт, действия призрака и `GhostOnMove` — в 6.5б.

use std::time::Duration;

use astrocore_console::{CVarDef, CVarFlags, ConfigWorldExt};
use astrocore_ecs::{
    ComponentStartup, EventWorldExt, GameTime, LifecycleWorldExt, MetaData, RegistryWorldExt,
};
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::EntityCoordinates;
use astrocore_map::SpawnWorldExt;
use astrocore_net::{
    Eye, NetRegistryWorldExt, NetUserId, PlayerDetached, Session, VisibilityWorldExt,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::damage::{DamageSpecifier, DamageableWorldExt};
use crate::mind::{self, Mind, MindRemoved, MindUnvisited};
use crate::mobs::{MobState, MobStateWorldExt};

/// Слои видимости SS14 (`VisibilityFlags`).
pub const VISIBILITY_NORMAL: u32 = 1 << 0;
pub const VISIBILITY_GHOST: u32 = 1 << 1;

pub const GHOST_KILL_CRIT: CVarDef<bool> = CVarDef::new(
    "ghost.kill_crit",
    true,
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Призрак из критического состояния добивает тело и может вернуться в него.",
);

/// Призрак (`GhostComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Ghost {
    /// Может взаимодействовать с миром (`CanGhostInteract`, в данных `canInteract`).
    #[serde(rename = "canInteract")]
    pub can_ghost_interact: bool,
    pub can_return_to_body: bool,
    /// Время смерти по часам сервера.
    pub time_of_death: Duration,
}

/// Слышит всю речь на карте (`GhostHearingComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GhostHearing {}

/// Можно ли стать призраком (`GhostAttemptEvent`): на теле, отменяемое.
#[derive(Debug, Clone, Copy)]
pub struct GhostAttempt {
    pub mind: Entity,
    pub cancelled: bool,
}

/// Попытку обработал кто-то другой (`GhostAttemptHandleEvent`). Широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct GhostAttemptHandle {
    pub mind: Entity,
    pub can_return_global: bool,
    pub handled: bool,
    pub result: bool,
}

/// Может ли призрак вернуться в тело.
pub fn can_return_to_body(world: &World, entity: Entity) -> bool {
    world
        .get::<Ghost>(entity)
        .is_some_and(|g| g.can_return_to_body)
}

/// `SetCanReturnToBody`.
pub fn set_can_return_to_body(world: &mut World, entity: Entity, value: bool) {
    if let Some(mut ghost) = world.get_mut::<Ghost>(entity) {
        ghost.can_return_to_body = value;
    }
}

/// `SetTimeOfDeath`.
pub fn set_time_of_death(world: &mut World, entity: Entity, value: Duration) {
    if let Some(mut ghost) = world.get_mut::<Ghost>(entity) {
        ghost.time_of_death = value;
    }
}

fn valid_position(world: &World, at: &EntityCoordinates) -> bool {
    world.is_alive(at.parent)
        && world
            .get::<MetaData>(at.parent)
            .is_some_and(|m| m.stage < astrocore_ecs::LifeStage::Terminating)
}

/// Призрак для разума (`SpawnGhost`): на месте `at` или у точки наблюдателей. С `can_return`
/// разум посещает призрака, иначе переносится в него.
pub fn spawn_ghost(
    world: &mut World,
    mind_id: Entity,
    at: Option<EntityCoordinates>,
    can_return: bool,
) -> Option<Entity> {
    let state = world.get::<Mind>(mind_id)?.clone();
    let at = match at.filter(|at| valid_position(world, at)) {
        Some(at) => at,
        None => match crate::game_ticker::observer_spawn_point(world)
            .filter(|at| valid_position(world, at))
        {
            Some(at) => at,
            None => {
                eprintln!("нет места для призрака {:?}", state.character_name);
                mind::transfer_to(world, mind_id, None, false, false);
                return None;
            }
        },
    };
    let ghost = world
        .spawn_attached(crate::game_ticker::OBSERVER_PROTOTYPE, at)
        .ok()?;
    // Имя призрака — имя персонажа, иначе имя игрока.
    let name = state
        .character_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| {
            let user = state.user_id?;
            let session = mind::session_of_user(world, user)?;
            world.get::<Session>(session).map(|s| s.name.clone())
        });
    if let Some(name) = name
        && let Some(mut meta) = world.get_mut::<MetaData>(ghost)
    {
        meta.name = Some(name);
    }
    if let Some(time) = state.time_of_death {
        set_time_of_death(world, ghost, time);
    }
    set_can_return_to_body(world, ghost, can_return);
    if can_return {
        mind::visit(world, mind_id, ghost);
    } else {
        mind::transfer_to(world, mind_id, Some(ghost), false, true);
    }
    Some(ghost)
}

/// Стать призраком (`OnGhostAttempt`). `can_return_global` — вернуться можно, если тело
/// мертво; `forced` — без проверок тела и запрета.
pub fn on_ghost_attempt(
    world: &mut World,
    mind_id: Entity,
    can_return_global: bool,
    via_command: bool,
    forced: bool,
) -> bool {
    let Some(state) = world.get::<Mind>(mind_id).cloned() else {
        return false;
    };
    let player_entity = state.current_entity();
    let users: Vec<NetUserId> = state.user_id.into_iter().collect();
    if let Some(entity) = player_entity
        && via_command
    {
        let name = world.entity_name(entity);
        let text = if forced {
            format!("{name} was forced to ghost via command")
        } else {
            format!("{name} is attempting to ghost via command")
        };
        add_admin_log(world, LogType::Mind, LogImpact::Medium, &text, &[], &users);
    }
    if let Some(entity) = player_entity
        && !forced
    {
        let mut attempt = GhostAttempt {
            mind: mind_id,
            cancelled: false,
        };
        world.raise(entity, &mut attempt);
        if attempt.cancelled {
            return false;
        }
    }
    let mut handle = GhostAttemptHandle {
        mind: mind_id,
        can_return_global,
        handled: false,
        result: false,
    };
    world.raise_broadcast(&mut handle);
    if handle.handled {
        return handle.result;
    }
    if state.prevent_ghosting && !forced {
        if let Some(user) = state.user_id
            && let Some(session) = mind::session_of_user(world, user)
        {
            let text = crate::examine::loc(world, "comp-mind-ghosting-prevented", &[]);
            crate::chat::dispatch_server_message(world, session, &text);
        }
        return false;
    }
    if player_entity.is_some_and(|e| world.get::<Ghost>(e).is_some_and(|g| !g.can_ghost_interact)) {
        return false;
    }
    if state.visiting_entity.is_some() {
        mind::unvisit(world, mind_id);
    }
    let position = match player_entity.filter(|e| world.is_alive(*e)) {
        Some(entity) => mind::grid_or_map_coordinates(world, entity),
        None => crate::game_ticker::observer_spawn_point(world),
    };
    let Some(position) = position else {
        return false;
    };
    // Вернуться можно, если тело мертво; из крита — тело добивается (`GhostKillCrit`).
    let mut can_return = can_return_global && mind::is_character_dead_physically(world, mind_id);
    if world.cvar(&GHOST_KILL_CRIT)
        && can_return_global
        && let Some(entity) = player_entity
        && world.is_mob_critical(entity)
    {
        can_return = true;
        let dead = world.threshold_for_state(entity, MobState::Dead);
        let dealt = match dead {
            Some(threshold) => threshold - world.total_damage(entity),
            None => crate::fixed_point::FixedPoint2::from_f32(200.0),
        };
        let damage = DamageSpecifier::new([("Asphyxiation", dealt)]);
        world.change_damage(entity, &damage, true, true, None);
    }
    if let Some(entity) = player_entity {
        let name = world.entity_name(entity);
        let suffix = if can_return { "" } else { " (non-returnable)" };
        add_admin_log(
            world,
            LogType::Mind,
            LogImpact::Medium,
            &format!("{name} ghosted{suffix}"),
            &[],
            &users,
        );
    }
    spawn_ghost(world, mind_id, Some(position), can_return).is_some()
}

fn delete_ghost(world: &mut World, entity: Entity) {
    if world.is_alive(entity) {
        world.queue_delete(entity);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_cvar(&GHOST_KILL_CRIT);
    world.register_data_component::<Ghost>();
    world.register_net_serde::<Ghost>();
    world.register_data_component::<GhostHearing>();
    if !side.is_server() {
        return;
    }
    // Призрака видят только призраки, а он видит их (`OnGhostStartup`, `OnGhostVis`).
    world.subscribe::<Ghost, ComponentStartup>(|world, entity, _| {
        let post_round =
            crate::game_ticker::run_level(world) == crate::game_ticker::GameRunLevel::PostRound;
        if !post_round {
            world.set_visibility_layer(entity, VISIBILITY_GHOST);
        }
        if let Some(mut eye) = world.get_mut::<Eye>(entity) {
            eye.vis_mask.0 |= VISIBILITY_GHOST;
        }
        let now = world.resource::<GameTime>().cur_time();
        set_time_of_death(world, entity, now);
    });
    world.subscribe::<Ghost, MindRemoved>(|world, entity, _| delete_ghost(world, entity));
    world.subscribe::<Ghost, MindUnvisited>(|world, entity, _| delete_ghost(world, entity));
    world.subscribe::<Ghost, PlayerDetached>(|world, entity, _| delete_ghost(world, entity));
}

#[cfg(test)]
mod tests;
