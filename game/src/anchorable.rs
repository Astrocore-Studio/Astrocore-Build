//! Закрепление гаечным ключом: инструмент с нужным свойством закрепляет сущность в клетке
//! грида или открепляет её после ожидания. Образец — `AnchorableSystem` и
//! `LockSystem.OnUnanchorAttempt` SS14 @ 2f4a2aad.
//!
//! Нет пока: закрепления только на станции (`AnchorOnlyOnStation`) и флагов запрета.

use astrocore_ecs::{EventWorldExt, MapInit, RegistryWorldExt};
use astrocore_map::{
    AnchorStateChanged, EntityCoordinates, MapGrid, TileCoord, Transform, TransformWorldExt,
};
use astrocore_math::Angle;
use astrocore_net::AppearanceWorldExt;
use astrocore_physics::{Fixtures, Physics};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::do_after::DoAfterDone;
use crate::examine::loc;
use crate::interaction::InteractUsing;
use crate::popups::{PopupType, PopupWorldExt};
use crate::pulling::PullingWorldExt;
use crate::tag::TagWorldExt;
use crate::tools::{Tool, ToolWorldExt, register_tool_event};

/// Сущность закрепляют инструментом (`AnchorableComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Anchorable {
    #[serde(default = "anchoring")]
    pub tool: String,
    /// Выровнять по центру клетки при закреплении.
    #[serde(default = "yes")]
    pub snap: bool,
    #[serde(default = "one")]
    pub delay: f64,
}

fn anchoring() -> String {
    "Anchoring".into()
}
fn yes() -> bool {
    true
}
fn one() -> f64 {
    1.0
}

/// Запертое не открепить (`LockedAnchorableComponent`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LockedAnchorable {}

/// Закреплена ли сущность (`AnchorVisuals.Anchored`).
pub const ANCHOR_VISUALS: &str = "enum.AnchorVisuals.Anchored";

/// Попытка закрепить или открепить (`AnchorAttemptEvent`, `UnanchorAttemptEvent`).
/// Поднимается на сущности; можно отменить, задать сообщение и добавить ожидание.
#[derive(Debug, Clone)]
pub struct AnchorAttempt {
    pub user: Entity,
    pub tool: Entity,
    pub anchoring: bool,
    pub cancelled: bool,
    pub fail_message: Option<String>,
    pub delay: f64,
}

/// Окончание ожидания (`TryAnchorCompletedEvent`, `TryUnanchorCompletedEvent`).
#[derive(Debug, Clone, Copy)]
pub struct AnchorDoAfter {
    pub anchoring: bool,
}

/// Клетка, в которой стоит сущность.
fn cell_of(world: &World, entity: Entity) -> Option<(Entity, TileCoord)> {
    let transform = world.get::<Transform>(entity)?;
    let grid = transform.grid()?;
    let local = if world.parent(entity) == Some(grid) {
        transform.local_position()
    } else {
        let at = world.map_coordinates(entity)?;
        world.world_transform(grid)?.to_local(at.position)
    };
    Some((grid, TileCoord::containing(local, transform.level())))
}

fn layers(world: &World, entity: Entity) -> (i32, i32) {
    world.get::<Fixtures>(entity).map_or((0, 0), |f| {
        f.fixtures
            .values()
            .filter(|x| x.hard)
            .fold((0, 0), |(l, m), x| (l | x.layer.0, m | x.mask.0))
    })
}

/// Свободна ли клетка для тела (`TileFree`): закреплённые твёрдые тела в ней не
/// пересекаются с ним по слоям.
pub fn tile_free(world: &World, entity: Entity, grid: Entity, cell: TileCoord) -> bool {
    let (layer, mask) = layers(world, entity);
    let Some(map_grid) = world.get::<MapGrid>(grid) else {
        return false;
    };
    !map_grid.anchored_at(cell).iter().any(|other| {
        if *other == entity || world.get::<Physics>(*other).is_none_or(|b| !b.can_collide) {
            return false;
        }
        let (other_layer, other_mask) = layers(world, *other);
        other_mask & layer != 0 || other_layer & mask != 0
    })
}

/// Есть ли в клетке закреплённое `Unstackable` (`AnyUnstackablesAnchoredAt`).
fn any_unstackable(world: &World, entity: Entity, grid: Entity, cell: TileCoord) -> bool {
    world.has_tag(entity, "Unstackable")
        && world.get::<MapGrid>(grid).is_some_and(|g| {
            g.anchored_at(cell)
                .iter()
                .any(|e| *e != entity && world.has_tag(*e, "Unstackable"))
        })
}

fn popup(world: &mut World, key: &str, entity: Entity, user: Entity) {
    let text = loc(world, key, &[]);
    world.popup_entity_for(&text, entity, user, PopupType::Small);
}

/// Попробовать закрепить или открепить (`TryToggleAnchor`).
pub fn try_toggle_anchor(world: &mut World, entity: Entity, user: Entity, tool: Entity) -> bool {
    let Some(anchorable) = world.get::<Anchorable>(entity).cloned() else {
        return false;
    };
    let anchoring = !world
        .get::<Transform>(entity)
        .is_some_and(Transform::anchored);
    let mut attempt = AnchorAttempt {
        user,
        tool,
        anchoring,
        cancelled: false,
        fail_message: None,
        delay: 0.0,
    };
    world.raise(entity, &mut attempt);
    if attempt.cancelled {
        if let Some(message) = attempt.fail_message {
            world.popup_entity_for(&message, entity, user, PopupType::Small);
        }
        return false;
    }
    if anchoring {
        let Some((grid, cell)) = cell_of(world, entity) else {
            return false;
        };
        if world.get::<Physics>(entity).is_some() && !tile_free(world, entity, grid, cell) {
            popup(world, "anchorable-occupied", entity, user);
            return false;
        }
        if any_unstackable(world, entity, grid, cell) {
            popup(
                world,
                "construction-step-condition-no-unstackable-in-tile",
                entity,
                user,
            );
            return false;
        }
    }
    let qualities: Vec<String> = world
        .get::<Tool>(tool)
        .map(|t| t.qualities.iter().cloned().collect())
        .unwrap_or_default();
    let list: Vec<&str> = qualities.iter().map(String::as_str).collect();
    world.use_tool(
        tool,
        user,
        Some(entity),
        anchorable.delay + attempt.delay,
        &list,
        AnchorDoAfter { anchoring },
        0.0,
    )
}

fn on_complete(world: &mut World, entity: Entity, event: &mut DoAfterDone<AnchorDoAfter>) {
    if event.cancelled || event.used.is_none() {
        return;
    }
    let user = event.user;
    if !event.event.anchoring {
        world.unanchor(entity);
        popup(world, "anchorable-unanchored", entity, user);
        return;
    }
    let Some((grid, cell)) = cell_of(world, entity) else {
        return;
    };
    if world.get::<Physics>(entity).is_some() && !tile_free(world, entity, grid, cell) {
        popup(world, "anchorable-occupied", entity, user);
        return;
    }
    let snap = world.get::<Anchorable>(entity).is_none_or(|a| a.snap);
    let quarter = std::f32::consts::FRAC_PI_2;
    let rotation = world
        .get::<Transform>(entity)
        .map_or(0.0, |t| (t.local_rotation().0 / quarter).round() * quarter);
    if world.puller_of(entity).is_some() {
        world.try_stop_pull(entity, None);
    }
    if snap {
        if any_unstackable(world, entity, grid, cell) {
            popup(
                world,
                "construction-step-condition-no-unstackable-in-tile",
                entity,
                user,
            );
            return;
        }
        let at = EntityCoordinates::new(grid, cell.center());
        let _ = world.set_coordinates(entity, at, Some(Angle(rotation)));
    } else {
        let _ = world.set_local_rotation(entity, Angle(rotation));
    }
    if !world
        .get::<Transform>(entity)
        .is_some_and(Transform::anchored)
    {
        world.anchor(entity);
    }
    popup(world, "anchorable-anchored", entity, user);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Anchorable>();
    world.register_data_component::<LockedAnchorable>();
    if !side.is_server() {
        return;
    }
    register_tool_event::<AnchorDoAfter>(world);
    world.subscribe::<Anchorable, InteractUsing>(|world, entity, event| {
        if event.handled {
            return;
        }
        let Some(quality) = world.get::<Anchorable>(entity).map(|a| a.tool.clone()) else {
            return;
        };
        if !world.has_quality(event.used, &quality) {
            return;
        }
        event.handled = true;
        try_toggle_anchor(world, entity, event.user, event.used);
    });
    world.subscribe::<Anchorable, DoAfterDone<AnchorDoAfter>>(on_complete);
    world.subscribe::<Anchorable, MapInit>(|world, entity, _| {
        let anchored = world
            .get::<Transform>(entity)
            .is_some_and(Transform::anchored);
        world.set_appearance(entity, ANCHOR_VISUALS, anchored);
    });
    world.subscribe::<Anchorable, AnchorStateChanged>(|world, entity, event| {
        world.set_appearance(entity, ANCHOR_VISUALS, event.anchored);
    });
    // `LockSystem.OnUnanchorAttempt`: запертое не открепить.
    world.subscribe::<LockedAnchorable, AnchorAttempt>(|world, entity, event| {
        if event.cancelled || event.anchoring || !crate::lock::is_locked(world, entity) {
            return;
        }
        let name = astrocore_loc::EntityNameWorldExt::entity_name(world, entity);
        event.fail_message = Some(loc(
            world,
            "lock-comp-generic-fail",
            &[("target", name.into())],
        ));
        event.cancelled = true;
    });
}

#[cfg(test)]
mod tests;
