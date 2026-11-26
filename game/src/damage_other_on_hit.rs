//! Урон от брошенного предмета при попадании и звук попадания по цели. Образец —
//! `DamageOtherOnHitSystem` (сервер и общая часть) и `RangedDamageSoundComponent`
//! SS14 @ 2f4a2aad. Вспышки цели и тряски камеры пока нет.

use std::collections::BTreeMap;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, RegistryWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::damage::{DamageSpecifier, DamageableWorldExt, catalog};
use crate::fixed_point::FixedPoint2;
use crate::throwing::ThrowDoHit;

/// Брошенный предмет ранит того, в кого попал. В данных — компонент `DamageOtherOnHit`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DamageOtherOnHit {
    #[serde(default)]
    pub ignore_resistances: bool,
    pub damage: DamageSpecifier,
}

/// Звук попадания по сущности по типу или группе урона. В данных — компонент
/// `RangedDamageSound`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RangedDamageSound {
    #[serde(default)]
    pub sound_groups: Option<BTreeMap<String, SoundSpecifier>>,
    #[serde(default)]
    pub sound_types: Option<BTreeMap<String, SoundSpecifier>>,
}

/// Группа, если урон только одной группы, иначе тип (`GetHighestDamageSound`). Как в SS14,
/// при нескольких типах выбирается последний с уроном больше нуля.
pub fn highest_damage_sound(world: &mut World, damage: &DamageSpecifier) -> Option<String> {
    let groups = catalog(world).per_group(damage);
    if groups.len() == 1 {
        return groups.keys().next().cloned();
    }
    damage
        .types
        .iter()
        .rev()
        .find(|(_, value)| **value > FixedPoint2::ZERO)
        .map(|(damage_type, _)| damage_type.clone())
}

/// Звук попадания: свой у цели, иначе звук оружия (`PlayImpactSound`).
pub fn play_impact_sound(
    world: &mut World,
    target: Entity,
    damage: Option<&DamageSpecifier>,
    weapon_sound: Option<&SoundSpecifier>,
) {
    let mut played = false;
    if let Some(damage) = damage.filter(|d| d.total() > FixedPoint2::ZERO)
        && let Some(sounds) = world.get::<RangedDamageSound>(target).cloned()
        && let Some(kind) = highest_damage_sound(world, damage)
    {
        let sound = sounds
            .sound_types
            .as_ref()
            .and_then(|types| types.get(&kind))
            .or_else(|| {
                sounds
                    .sound_groups
                    .as_ref()
                    .and_then(|groups| groups.get(&kind))
            });
        if let Some(sound) = sound {
            world.play_entity(sound, target, None);
            played = true;
        }
    }
    if !played && let Some(sound) = weapon_sound {
        world.play_entity(sound, target, None);
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<DamageOtherOnHit>();
    world.register_data_component::<RangedDamageSound>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<DamageOtherOnHit, ThrowDoHit>(|world, item, event| {
        let Some(component) = world.get::<DamageOtherOnHit>(item).cloned() else {
            return;
        };
        if !astrocore_ecs::LifecycleWorldExt::is_alive(world, event.target) {
            return;
        }
        let dealt = world.change_damage(
            event.target,
            &component.damage,
            component.ignore_resistances,
            true,
            event.thrower,
        );
        play_impact_sound(world, event.target, Some(&dealt), None);
    });
}
