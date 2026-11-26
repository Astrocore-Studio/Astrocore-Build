//! Стоит или лежит: лежащий проходит под столами и не мешает другим, роняет вещи, спрайт
//! поворачивается набок. Образец — `Content.Shared/Standing` SS14 @ 2f4a2aad
//! (`StandingStateComponent`, `StandingStateSystem`).

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, GameRng, RegistryWorldExt};
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use astrocore_physics::{CollisionGroups, Fixtures, Physics};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::content::groups;
use crate::friction::TileFriction;
use crate::hands::{Hands, HandsWorldExt};

/// Слой, который лежащий снимает со своей маски (`StandingCollisionLayer`).
pub const STANDING_COLLISION_LAYER: i32 = groups::MID;
/// Ключ внешнего вида: поворот (`RotationVisuals.RotationState`).
pub const VISUALS_ROTATION: &str = "enum.RotationVisuals.RotationState";
/// Разброс брошенных при падении вещей, градусы (`DropHeldItemsSpread`).
const DROP_SPREAD_DEGREES: f32 = 45.0;

fn yes() -> bool {
    true
}

fn body_fall() -> Option<SoundSpecifier> {
    Some(SoundSpecifier::collection("BodyFall"))
}

fn down_friction() -> f32 {
    0.4
}

/// Стоит ли сущность. В данных — компонент `StandingState`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandingState {
    #[serde(default = "body_fall")]
    pub down_sound: Option<SoundSpecifier>,
    #[serde(default = "yes")]
    pub standing: bool,
    #[serde(default = "down_friction")]
    pub down_friction_mod: f32,
    #[serde(default)]
    pub changed_fixtures: Vec<String>,
}

impl Default for StandingState {
    fn default() -> Self {
        Self {
            down_sound: body_fall(),
            standing: true,
            down_friction_mod: down_friction(),
            changed_fixtures: Vec::new(),
        }
    }
}

/// Можно ли лечь (`DownAttemptEvent`).
#[derive(Debug, Clone, Copy, Default)]
pub struct DownAttempt {
    pub cancelled: bool,
}

/// Можно ли встать (`StandAttemptEvent`).
#[derive(Debug, Clone, Copy, Default)]
pub struct StandAttempt {
    pub cancelled: bool,
}

/// Легла (`DownedEvent`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Downed;

/// Встала (`StoodEvent`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Stood;

/// Выронить всё из рук (`DropHandItemsEvent`).
#[derive(Debug, Clone, Copy, Default)]
pub struct DropHandItems;

/// Положение тела на `World`.
pub trait StandingWorldExt {
    fn is_down(&self, entity: Entity) -> bool;
    /// Лечь (`Down`).
    fn down(&mut self, entity: Entity, play_sound: bool, force: bool) -> bool;
    /// Встать (`Stand`).
    fn stand(&mut self, entity: Entity, force: bool) -> bool;
}

impl StandingWorldExt for World {
    fn is_down(&self, entity: Entity) -> bool {
        self.get::<StandingState>(entity)
            .is_some_and(|s| !s.standing)
    }

    fn down(&mut self, entity: Entity, play_sound: bool, force: bool) -> bool {
        let Some(state) = self.get::<StandingState>(entity) else {
            return false;
        };
        if !state.standing {
            return true;
        }
        if !force {
            let mut attempt = DownAttempt::default();
            self.raise(entity, &mut attempt);
            if attempt.cancelled {
                return false;
            }
        }
        if let Some(mut state) = self.get_mut::<StandingState>(entity) {
            state.standing = false;
        }
        self.raise(entity, &mut Downed);
        self.set_appearance(entity, VISUALS_ROTATION, "Horizontal".to_owned());
        change_layers(self, entity);
        if play_sound
            && let Some(sound) = self
                .get::<StandingState>(entity)
                .and_then(|s| s.down_sound.clone())
        {
            self.play_entity(&sound, entity, None);
        }
        true
    }

    fn stand(&mut self, entity: Entity, force: bool) -> bool {
        let Some(state) = self.get::<StandingState>(entity) else {
            return false;
        };
        if state.standing {
            return true;
        }
        if !force {
            let mut attempt = StandAttempt::default();
            self.raise(entity, &mut attempt);
            if attempt.cancelled {
                return false;
            }
        }
        if let Some(mut state) = self.get_mut::<StandingState>(entity) {
            state.standing = true;
        }
        self.raise(entity, &mut Stood);
        self.set_appearance(entity, VISUALS_ROTATION, "Vertical".to_owned());
        revert_layers(self, entity);
        true
    }
}

/// Лежащий не задевает то, что выше пояса: столы, заслонки (`ChangeLayers`).
fn change_layers(world: &mut World, entity: Entity) {
    let mut changed = Vec::new();
    if let Some(mut fixtures) = world.get_mut::<Fixtures>(entity) {
        for (key, fixture) in fixtures.fixtures.iter_mut() {
            if fixture.mask.0 & STANDING_COLLISION_LAYER == 0 || !fixture.hard {
                continue;
            }
            changed.push(key.clone());
            fixture.mask = CollisionGroups(fixture.mask.0 & !STANDING_COLLISION_LAYER);
        }
    }
    if let Some(mut state) = world.get_mut::<StandingState>(entity) {
        state.changed_fixtures.extend(changed);
    }
}

fn revert_layers(world: &mut World, entity: Entity) {
    let changed = world
        .get_mut::<StandingState>(entity)
        .map(|mut state| std::mem::take(&mut state.changed_fixtures))
        .unwrap_or_default();
    if let Some(mut fixtures) = world.get_mut::<Fixtures>(entity) {
        for key in changed {
            if let Some(fixture) = fixtures.fixtures.get_mut(&key)
                && fixture.hard
            {
                fixture.mask = CollisionGroups(fixture.mask.0 | STANDING_COLLISION_LAYER);
            }
        }
    }
}

/// Упавший роняет вещи с разлётом по своей скорости (`HandsSystem.OnDropHandItems`).
fn drop_hand_items(world: &mut World, entity: Entity, _: &mut DropHandItems) {
    let velocity = world
        .get::<Physics>(entity)
        .map_or(astrocore_math::Vec2::ZERO, |body| body.linear_velocity);
    let base_speed = world
        .get::<Hands>(entity)
        .map_or(11.0, |hands| hands.base_throwspeed);
    for hand in world.hands_in_order(entity) {
        let Some(item) = world.held_item(entity, &hand) else {
            continue;
        };
        if !world.try_drop(entity, &hand, None, false) {
            continue;
        }
        let spread = DROP_SPREAD_DEGREES.to_radians();
        let (angle, distance, speed) = {
            let mut rng = world.resource_mut::<GameRng>();
            (
                rng.range_f32(-spread, spread),
                rng.next_f32(),
                rng.range_f32(0.45, 0.55),
            )
        };
        let inv_mass = world
            .get::<Physics>(item)
            .map_or(0.0, |body| body.inv_mass());
        let direction = astrocore_math::Angle(angle).rotate(velocity) * distance * inv_mass;
        crate::throwing::try_throw_with(
            world,
            item,
            direction,
            base_speed * speed,
            Some(entity),
            0.0,
            false,
        );
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<StandingState>();
    world.register_net_serde::<StandingState>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<StandingState, TileFriction>(|world, entity, event| {
        if let Some(state) = world.get::<StandingState>(entity)
            && !state.standing
        {
            event.modifier *= state.down_friction_mod;
        }
    });
    world.subscribe::<Hands, DropHandItems>(drop_hand_items);
}
