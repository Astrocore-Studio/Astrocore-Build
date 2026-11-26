//! Упакованные конструкции: мультитулом (`Pulsing`) по упаковке на свободной клетке — и на
//! её месте стоит конструкция (части щита AME, машины). Образец — `FlatpackComponent`,
//! `SharedFlatpackSystem.OnFlatpackInteractUsing` SS14 @ 2f4a2aad.

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, LifecycleWorldExt, RegistryWorldExt};
use astrocore_map::{
    ContainerWorldExt, EntityCoordinates, MapGrid, SpawnWorldExt, TileCoord, Transform,
};
use astrocore_physics::Fixtures;
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::examine::loc;
use crate::interaction::InteractUsing;
use crate::popups::{PopupType, PopupWorldExt};
use crate::tools::ToolWorldExt;

fn pulsing() -> String {
    "Pulsing".into()
}

fn unpack_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Items/Ratchet.ogg")
}

/// Упаковка (`FlatpackComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flatpack {
    /// Что внутри.
    #[serde(default)]
    pub entity: Option<String>,
    #[serde(default = "pulsing")]
    pub quality_needed: String,
    #[serde(default = "unpack_sound")]
    pub unpack_sound: SoundSpecifier,
    /// Цвет полосы упаковки по отделу платы.
    #[serde(default)]
    pub board_colors: std::collections::BTreeMap<String, String>,
}

/// Есть ли у прототипа твёрдые фикстуры.
fn prototype_is_hard(world: &World, prototype: &str) -> bool {
    world
        .get_resource::<Prototypes>()
        .and_then(|p| p.entity(prototype))
        .and_then(|proto| proto.component("Fixtures"))
        .and_then(|c| c.parsed.clone())
        .and_then(|parsed| {
            parsed
                .as_any()
                .downcast_ref::<Fixtures>()
                .map(|f| f.fixtures.values().any(|fixture| fixture.hard))
        })
        .unwrap_or(false)
}

/// Свободна ли клетка для твёрдой конструкции (`AnchorableSystem.TileFree`): на ней нет
/// привязанных сущностей с твёрдыми фикстурами.
pub fn tile_free(world: &World, grid: Entity, cell: TileCoord) -> bool {
    let Some(map_grid) = world.get::<MapGrid>(grid) else {
        return false;
    };
    !map_grid.anchored_at(cell).iter().any(|entity| {
        world
            .get::<Fixtures>(*entity)
            .is_some_and(|f| f.fixtures.values().any(|fixture| fixture.hard))
    })
}

fn on_interact_using(world: &mut World, pack: Entity, event: &mut InteractUsing) {
    let Some(flatpack) = world.get::<Flatpack>(pack).cloned() else {
        return;
    };
    if event.handled
        || !world.has_quality(event.used, &flatpack.quality_needed)
        || world.containing_container(pack).is_some()
    {
        return;
    }
    let Some(transform) = world.get::<Transform>(pack) else {
        return;
    };
    let Some(grid) = transform.grid() else {
        return;
    };
    let cell = TileCoord::containing(transform.local_position(), transform.level());
    event.handled = true;
    let Some(prototype) = flatpack.entity else {
        world.queue_delete(pack);
        return;
    };
    if prototype_is_hard(world, &prototype) && !tile_free(world, grid, cell) {
        let text = loc(world, "flatpack-unpack-no-room", &[]);
        world.popup_entity_for(&text, pack, event.user, PopupType::Small);
        return;
    }
    if world
        .spawn_attached(&prototype, EntityCoordinates::new(grid, cell.center()))
        .is_ok()
    {
        world.queue_delete(pack);
        world.play_entity(&flatpack.unpack_sound, event.used, None);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Flatpack>();
    if side.is_server() {
        world.subscribe::<Flatpack, InteractUsing>(on_interact_using);
    }
}
