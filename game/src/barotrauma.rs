//! Урон от давления: в вакууме и при слишком высоком давлении. Скафандр и шлем (слоты
//! `head` и `outerClothing`) защищают, если надеты оба. Образец — `BarotraumaComponent`,
//! `PressureProtectionComponent`, `PressureImmunityComponent`, `BarotraumaSystem` и
//! `AtmosphereSystem.GetContainingMixture` SS14 @ 2f4a2aad.
//!
//! Значки тревоги (`Alerts`) — вместе с интерфейсом состояния; здесь только урон.

use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update};
use astrocore_map::{ContainerWorldExt, TileCoord, Transform, TransformWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::atmos::gas::*;
use crate::atmos::{GasMixture, tile_mixture};
use crate::damage::{DamageSpecifier, Damageable, DamageableWorldExt};
use crate::fixed_point::FixedPoint2;
use crate::inventory::InventoryWorldExt;

fn max_damage() -> FixedPoint2 {
    FixedPoint2::from_f32(200.0)
}
fn protection_slots() -> Vec<String> {
    vec!["head".into(), "outerClothing".into()]
}
fn one() -> f32 {
    1.0
}

/// Получает урон от давления (`BarotraumaComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Barotrauma {
    pub damage: DamageSpecifier,
    #[serde(default = "max_damage")]
    pub max_damage: FixedPoint2,
    #[serde(skip)]
    pub taking_damage: bool,
    #[serde(default = "protection_slots")]
    pub protection_slots: Vec<String>,
    #[serde(skip, default = "one")]
    pub high_pressure_multiplier: f32,
    #[serde(skip)]
    pub high_pressure_modifier: f32,
    #[serde(skip, default = "one")]
    pub low_pressure_multiplier: f32,
    #[serde(skip)]
    pub low_pressure_modifier: f32,
    #[serde(skip)]
    pub has_immunity: bool,
}

/// Защищает от давления (`PressureProtectionComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PressureProtection {
    #[serde(default = "one")]
    pub high_pressure_multiplier: f32,
    #[serde(default)]
    pub high_pressure_modifier: f32,
    #[serde(default = "one")]
    pub low_pressure_multiplier: f32,
    #[serde(default)]
    pub low_pressure_modifier: f32,
}

/// Давление не действует (`PressureImmunityComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PressureImmunity;

/// Воздух, в котором находится сущность, у контейнера (`AtmosExposedGetAirEvent`).
/// Поднимается на контейнере-владельце; обработчик (шкаф со своим воздухом) кладёт смесь.
#[derive(Debug, Clone, Default)]
pub struct ExposedGetAir {
    pub entity: Option<Entity>,
    pub mixture: Option<GasMixture>,
    pub handled: bool,
}

/// Где воздух вокруг сущности.
enum AirAt {
    /// Свой воздух контейнера (`AtmosExposedGetAirEvent`): копия.
    Holder(GasMixture),
    /// Клетка грида.
    Tile(Entity, TileCoord),
    /// Вне грида — космос.
    Space,
}

fn air_at(world: &mut World, entity: Entity) -> Option<AirAt> {
    let mut current = entity;
    while let Some(holder) = world.containing_container(current).map(|c| c.owner) {
        let mut event = ExposedGetAir {
            entity: Some(entity),
            ..Default::default()
        };
        world.raise(holder, &mut event);
        if event.handled {
            return event.mixture.map(AirAt::Holder);
        }
        current = holder;
    }
    let transform = world.get::<Transform>(current)?;
    let Some(grid) = transform.grid() else {
        return transform.map().map(|_| AirAt::Space);
    };
    let level = transform.level();
    let coordinates = world.coordinates(current)?;
    let local = if coordinates.parent == grid {
        coordinates.position
    } else {
        let world_position = world.world_transform(current)?.position;
        world.world_transform(grid)?.to_local(world_position)
    };
    Some(AirAt::Tile(grid, TileCoord::containing(local, level)))
}

/// Воздух вокруг сущности (`GetContainingMixture`): контейнера со своим воздухом или клетки
/// грида под ней; вне грида — космос.
pub fn containing_mixture(world: &mut World, entity: Entity) -> Option<GasMixture> {
    Some(match air_at(world, entity)? {
        AirAt::Holder(mixture) => mixture,
        AirAt::Tile(grid, coord) => tile_mixture(world, grid, coord)
            .cloned()
            .unwrap_or_else(GasMixture::space),
        AirAt::Space => GasMixture::space(),
    })
}

/// Изменить воздух вокруг сущности (`GetContainingMixture`, изменяемая смесь). Космос не
/// меняется: правка уходит в копию. Свой воздух контейнера пока тоже только копия — у
/// шкафов его ещё нет. `excite` — разбудить клетку для атмосферы.
pub fn with_containing_mixture<R>(
    world: &mut World,
    entity: Entity,
    excite: bool,
    f: impl FnOnce(&mut GasMixture) -> R,
) -> Option<R> {
    match air_at(world, entity)? {
        AirAt::Holder(mut mixture) => Some(f(&mut mixture)),
        AirAt::Tile(grid, coord) => {
            if tile_mixture(world, grid, coord).is_some() {
                crate::atmos::modify_tile_mixture(world, grid, coord, excite, f)
            } else {
                Some(f(&mut GasMixture::space()))
            }
        }
        AirAt::Space => Some(f(&mut GasMixture::space())),
    }
}

/// Значения защиты предмета (`TryGetPressureProtectionValues`).
fn protection_values(world: &World, entity: Entity) -> Option<PressureProtection> {
    world.get::<PressureProtection>(entity).cloned()
}

/// Пересчитать защиту от надетого (`UpdateCachedResistances`).
pub fn update_cached_resistances(world: &mut World, entity: Entity) {
    let Some(slots) = world
        .get::<Barotrauma>(entity)
        .map(|b| b.protection_slots.clone())
    else {
        return;
    };
    let (mut hp_mod, mut hp_mul, mut lp_mod, mut lp_mul) = (0.0, 1.0, 0.0, 1.0);
    if !slots.is_empty() {
        let (mut a, mut b, mut c, mut d) = (f32::MIN, f32::MIN, f32::MAX, f32::MAX);
        let mut all = true;
        for slot in &slots {
            let Some(values) = world
                .slot_entity(entity, slot)
                .and_then(|item| protection_values(world, item))
            else {
                all = false;
                break;
            };
            a = a.max(values.high_pressure_modifier);
            b = b.max(values.high_pressure_multiplier);
            c = c.min(values.low_pressure_modifier);
            d = d.min(values.low_pressure_multiplier);
        }
        if all {
            (hp_mod, hp_mul, lp_mod, lp_mul) = (a, b, c, d);
        }
    }
    if let Some(own) = protection_values(world, entity) {
        hp_mod += own.high_pressure_modifier;
        hp_mul *= own.high_pressure_multiplier;
        lp_mod += own.low_pressure_modifier;
        lp_mul *= own.low_pressure_multiplier;
    }
    if let Some(mut barotrauma) = world.get_mut::<Barotrauma>(entity) {
        barotrauma.high_pressure_modifier = hp_mod;
        barotrauma.high_pressure_multiplier = hp_mul;
        barotrauma.low_pressure_modifier = lp_mod;
        barotrauma.low_pressure_multiplier = lp_mul;
    }
}

/// Ощущаемое низкое давление (`GetFeltLowPressure`).
pub fn felt_low_pressure(barotrauma: &Barotrauma, pressure: f32) -> f32 {
    if barotrauma.has_immunity {
        return ONE_ATMOSPHERE;
    }
    ((pressure + barotrauma.low_pressure_modifier) * barotrauma.low_pressure_multiplier)
        .min(ONE_ATMOSPHERE)
}

/// Ощущаемое высокое давление (`GetFeltHighPressure`).
pub fn felt_high_pressure(barotrauma: &Barotrauma, pressure: f32) -> f32 {
    if barotrauma.has_immunity {
        return ONE_ATMOSPHERE;
    }
    ((pressure + barotrauma.high_pressure_modifier) * barotrauma.high_pressure_multiplier)
        .max(ONE_ATMOSPHERE)
}

/// Раз в секунду (`BarotraumaSystem.Update`).
fn update(world: &mut World, mut timer: bevy_ecs::system::Local<f32>) {
    *timer += world.resource::<GameTime>().frame_time();
    if *timer < 1.0 {
        return;
    }
    *timer -= 1.0;
    let entities: Vec<Entity> = world
        .query::<(Entity, &Barotrauma, &Damageable)>()
        .iter(world)
        .map(|(e, _, _)| e)
        .collect();
    for entity in entities {
        update_cached_resistances(world, entity);
        let (Some(barotrauma), Some(damageable)) = (
            world.get::<Barotrauma>(entity).cloned(),
            world.get::<Damageable>(entity),
        ) else {
            continue;
        };
        let taken: f32 = barotrauma
            .damage
            .types
            .keys()
            .filter_map(|kind| damageable.damage.types.get(kind))
            .map(|v| v.float())
            .sum();
        if taken >= barotrauma.max_damage.float() {
            continue;
        }
        let mut pressure = containing_mixture(world, entity).map_or(1.0, |m| m.pressure().max(1.0));
        if pressure <= WARNING_LOW_PRESSURE {
            pressure = felt_low_pressure(&barotrauma, pressure);
        } else if pressure >= WARNING_HIGH_PRESSURE {
            pressure = felt_high_pressure(&barotrauma, pressure);
        }
        let damage = if pressure <= HAZARD_LOW_PRESSURE {
            Some(barotrauma.damage.scale(LOW_PRESSURE_DAMAGE))
        } else if pressure >= HAZARD_HIGH_PRESSURE {
            let scale = ((pressure / HAZARD_HIGH_PRESSURE - 1.0) * PRESSURE_DAMAGE_COEFFICIENT)
                .min(MAX_HIGH_PRESSURE_DAMAGE);
            Some(barotrauma.damage.scale(scale))
        } else {
            None
        };
        let taking = damage.is_some();
        if let Some(damage) = damage {
            world.change_damage(entity, &damage, true, false, None);
        }
        if let Some(mut component) = world.get_mut::<Barotrauma>(entity) {
            component.taking_damage = taking;
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Barotrauma>();
    world.register_data_component::<PressureProtection>();
    world.register_data_component::<PressureImmunity>();
    if !side.is_server() {
        return;
    }
    // `RefreshPressureImmunity`: иммунитет от компонента.
    world.subscribe::<Barotrauma, MapInit>(|world, entity, _| {
        let immune = world.get::<PressureImmunity>(entity).is_some();
        if let Some(mut barotrauma) = world.get_mut::<Barotrauma>(entity) {
            barotrauma.has_immunity = immune;
        }
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
