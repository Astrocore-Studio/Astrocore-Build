//! Химические машины без окон: перемешивание (палочкой в руке или машиной по ёмкости — с
//! задержкой, реакции особых способов смешивания), настольная мешалка (центрифуга,
//! электролиз: включается при питании, держит ёмкость до конца и перемешивает её раствор),
//! плитка (`SolutionHeater`: пока на ней лежит предмет и есть питание, греет все его
//! растворы). Образец — `ReactionMixerSystem`, `SolutionContainerMixerSystem` (общий и
//! серверный), `SolutionContainerMixerComponent`, `SolutionHeaterSystem`,
//! `SolutionHeaterComponent`, `ActiveSolutionHeaterComponent` SS14 @ 2f4a2aad.
//!
//! Звук перемешивания играет один раз: зацикленных звуков с остановкой у нас пока нет.
//! Нет пока: смены нагрузки машины во время работы (`PowerStateSystem.SetWorkingState`),
//! встряхивания (`Shake`).

use std::time::Duration;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, RegistryWorldExt, Update};
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::{ContainerIsRemovingAttempt, ContainerWorldExt};
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterWorldExt};
use crate::examine::loc;
use crate::interaction::{ActivateInWorld, AfterInteract, UseInHand};
use crate::placeable::{ItemPlaced, ItemPlacer, ItemRemoved};
use crate::popups::{PopupType, PopupWorldExt};
use crate::power::{PowerChanged, is_powered};
use crate::reactions::{ReactionMixer, ReactionMixerType, update_chemicals_mixed};
use crate::solution_container as containers;

/// Ключ вида мешалки (`SolutionContainerMixerVisuals.Mixing`).
pub const MIXER_VISUAL: &str = "enum.SolutionContainerMixerVisuals.Mixing";
/// Ключ вида плитки (`SolutionHeaterVisuals.IsOn`).
pub const HEATER_VISUAL: &str = "enum.SolutionHeaterVisuals.IsOn";

/// Можно ли перемешать (`MixingAttemptEvent`). На мешающем.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixingAttempt {
    pub mixed: Entity,
    pub cancelled: bool,
}

/// Перемешано (`AfterMixingEvent`). На мешающем.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AfterMixing {
    pub mixed: Entity,
    pub mixer: Entity,
}

/// Действие «перемешать» (`ReactionMixDoAfterEvent`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReactionMixDoAfter;

/// Можно ли перемешать цель (`CanMix`): есть непустой смешиваемый раствор.
pub fn can_mix(world: &mut World, mixer: Entity, target: Entity) -> bool {
    if world.get::<ReactionMixer>(mixer).is_none() {
        return false;
    }
    let Some(soln) = containers::mixable_solution(world, target) else {
        return false;
    };
    if containers::solution(world, soln)
        .is_none_or(|s| s.volume <= crate::fixed_point::FixedPoint2::ZERO)
    {
        return false;
    }
    let mut attempt = MixingAttempt {
        mixed: mixer,
        cancelled: false,
    };
    world.raise(mixer, &mut attempt);
    !attempt.cancelled
}

/// Перемешать (`TryMix`).
pub fn try_mix(world: &mut World, mixer: Entity, target: Entity) -> bool {
    let Some(types) = world
        .get::<ReactionMixer>(mixer)
        .map(|m| m.reaction_types.clone())
    else {
        return false;
    };
    let mut attempt = MixingAttempt {
        mixed: mixer,
        cancelled: false,
    };
    world.raise(mixer, &mut attempt);
    if attempt.cancelled {
        return false;
    }
    let Some(soln) = containers::mixable_solution(world, target) else {
        return false;
    };
    update_chemicals_mixed(world, soln, &types);
    world.raise(
        mixer,
        &mut AfterMixing {
            mixed: mixer,
            mixer: target,
        },
    );
    true
}

fn start_mix_do_after(
    world: &mut World,
    mixer: Entity,
    user: Entity,
    target: Entity,
    handheld: bool,
) -> bool {
    let Some(component) = world.get::<ReactionMixer>(mixer).cloned() else {
        return false;
    };
    let mut args = DoAfterArgs::new(
        user,
        f64::from(component.time_to_mix),
        Some(target),
        Some(mixer),
        Some(mixer),
    );
    if handheld {
        args.need_hand = true;
        args.break_on_damage = true;
        args.break_on_drop_item = true;
        args.break_on_hand_change = true;
        args.break_on_move = true;
    }
    let started = world.try_start_do_after(args, ReactionMixDoAfter)
        != crate::do_after::DoAfterStart::Refused;
    if started && let Some(sound) = &component.mixing_sound {
        world.play_entity(sound, mixer, None);
    }
    started
}

fn on_mix_done(world: &mut World, mixer: Entity, event: &mut DoAfterDone<ReactionMixDoAfter>) {
    if event.cancelled {
        return;
    }
    let Some(target) = event.target else {
        return;
    };
    if !try_mix(world, mixer, target) {
        return;
    }
    let Some(message) = world
        .get::<ReactionMixer>(mixer)
        .map(|m| m.mix_message.clone())
    else {
        return;
    };
    let text = loc(
        world,
        &message,
        &[
            ("mixed", world.entity_arg(target).into()),
            ("mixer", world.entity_arg(mixer).into()),
        ],
    );
    world.popup_entity_for(&text, event.user, event.user, PopupType::Small);
}

fn mixer_type(world: &World, mixer: Entity) -> Option<ReactionMixerType> {
    world.get::<ReactionMixer>(mixer).map(|m| m.mixer_type)
}

fn container_id() -> String {
    "mixer".into()
}

/// Настольная мешалка (`SolutionContainerMixerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionContainerMixer {
    #[serde(default = "container_id")]
    pub container_id: String,
    #[serde(default)]
    pub mixing: bool,
    /// С.
    #[serde(default)]
    pub mix_duration: f32,
    #[serde(skip)]
    pub mix_time_end: Duration,
    #[serde(default)]
    pub mixing_sound: Option<SoundSpecifier>,
}

fn now(world: &World) -> Duration {
    world.resource::<GameTime>().cur_time()
}

/// Включить мешалку (`TryStartMix`).
pub fn try_start_mix(world: &mut World, mixer: Entity, user: Option<Entity>) {
    let Some(component) = world.get::<SolutionContainerMixer>(mixer).cloned() else {
        return;
    };
    if component.mixing {
        return;
    }
    let popup = |world: &mut World, key: &str| {
        if let Some(user) = user {
            let text = loc(world, key, &[]);
            world.popup_entity_for(&text, mixer, user, PopupType::Small);
        }
    };
    if !is_powered(world, mixer) {
        popup(world, "solution-container-mixer-no-power");
        return;
    }
    let empty = world
        .container(mixer, &component.container_id)
        .is_none_or(|c| c.entities().is_empty());
    if empty {
        popup(world, "solution-container-mixer-popup-nothing-to-mix");
        return;
    }
    let end = now(world) + Duration::from_secs_f32(component.mix_duration);
    if let Some(mut component) = world.get_mut::<SolutionContainerMixer>(mixer) {
        component.mixing = true;
        component.mix_time_end = end;
    }
    if let Some(sound) = &component.mixing_sound {
        world.play_entity(sound, mixer, None);
    }
    world.set_appearance(mixer, MIXER_VISUAL, true);
}

/// Выключить (`StopMix`).
pub fn stop_mix(world: &mut World, mixer: Entity) {
    let Some(mut component) = world.get_mut::<SolutionContainerMixer>(mixer) else {
        return;
    };
    if !component.mixing {
        return;
    }
    component.mixing = false;
    world.set_appearance(mixer, MIXER_VISUAL, false);
}

/// Закончить: перемешать растворы всех ёмкостей внутри (`FinishMix`).
pub fn finish_mix(world: &mut World, mixer: Entity) {
    let Some(component) = world.get::<SolutionContainerMixer>(mixer).cloned() else {
        return;
    };
    if !component.mixing {
        return;
    }
    stop_mix(world, mixer);
    let Some(types) = world
        .get::<ReactionMixer>(mixer)
        .map(|m| m.reaction_types.clone())
    else {
        return;
    };
    let Some(inside) = world
        .container(mixer, &component.container_id)
        .map(|c| c.entities().to_vec())
    else {
        return;
    };
    for entity in inside {
        if let Some(soln) = containers::fits_in_dispenser_solution(world, entity) {
            update_chemicals_mixed(world, soln, &types);
        }
    }
}

fn update_mixers(world: &mut World) {
    let time = now(world);
    let done: Vec<Entity> = world
        .query::<(Entity, &SolutionContainerMixer)>()
        .iter(world)
        .filter(|(_, m)| m.mixing && time >= m.mix_time_end)
        .map(|(e, _)| e)
        .collect();
    for mixer in done {
        finish_mix(world, mixer);
    }
}

/// Плитка (`SolutionHeaterComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionHeater {
    /// Дж/с.
    #[serde(default)]
    pub heat_per_second: f32,
}

/// Плитка включена (`ActiveSolutionHeaterComponent`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct ActiveSolutionHeater;

fn turn_on_heater(world: &mut World, heater: Entity) {
    world.set_appearance(heater, HEATER_VISUAL, true);
    world.entity_mut(heater).insert(ActiveSolutionHeater);
}

/// Включить, если есть что греть и питание (`TryTurnOn`).
pub fn try_turn_on_heater(world: &mut World, heater: Entity) -> bool {
    let Some(placer) = world.get::<ItemPlacer>(heater) else {
        return false;
    };
    if placer.placed_entities.is_empty() || !is_powered(world, heater) {
        return false;
    }
    turn_on_heater(world, heater);
    true
}

/// Выключить (`TurnOff`).
pub fn turn_off_heater(world: &mut World, heater: Entity) {
    world.set_appearance(heater, HEATER_VISUAL, false);
    world.entity_mut(heater).remove::<ActiveSolutionHeater>();
}

fn update_heaters(world: &mut World) {
    let frame = world.resource::<GameTime>().frame_time();
    let active: Vec<(f32, Vec<Entity>)> = world
        .query::<(&ActiveSolutionHeater, &SolutionHeater, &ItemPlacer)>()
        .iter(world)
        .map(|(_, heater, placer)| {
            (
                heater.heat_per_second * frame,
                placer.placed_entities.iter().copied().collect(),
            )
        })
        .collect();
    for (energy, placed) in active {
        for entity in placed {
            for (_, soln) in containers::enumerate_solutions(world, entity) {
                containers::add_thermal_energy(world, soln, energy);
            }
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<SolutionContainerMixer>();
    world.register_net_serde::<SolutionContainerMixer>();
    world.register_data_component::<SolutionHeater>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<ReactionMixer, UseInHand>(|world, mixer, event| {
        if event.handled || mixer_type(world, mixer) != Some(ReactionMixerType::Handheld) {
            return;
        }
        event.handled = true;
        if !can_mix(world, mixer, mixer) {
            return;
        }
        start_mix_do_after(world, mixer, event.user, mixer, true);
    });
    world.subscribe::<ReactionMixer, AfterInteract>(|world, mixer, event| {
        let Some(target) = event.target else {
            return;
        };
        if !event.can_reach || mixer_type(world, mixer) != Some(ReactionMixerType::Machine) {
            return;
        }
        if !can_mix(world, mixer, target) {
            return;
        }
        start_mix_do_after(world, mixer, event.user, target, false);
        event.handled = true;
    });
    world.subscribe::<ReactionMixer, DoAfterDone<ReactionMixDoAfter>>(on_mix_done);
    world.subscribe::<SolutionContainerMixer, ActivateInWorld>(|world, mixer, event| {
        if event.handled || !event.complex {
            return;
        }
        try_start_mix(world, mixer, Some(event.user));
        event.handled = true;
    });
    world.subscribe::<SolutionContainerMixer, ContainerIsRemovingAttempt>(|world, mixer, event| {
        if world
            .get::<SolutionContainerMixer>(mixer)
            .is_some_and(|m| m.mixing && *event.container == *m.container_id)
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<SolutionContainerMixer, PowerChanged>(|world, mixer, event| {
        if !event.powered {
            stop_mix(world, mixer);
        }
    });
    world.subscribe::<SolutionHeater, PowerChanged>(|world, heater, event| {
        let placed = world
            .get::<ItemPlacer>(heater)
            .is_some_and(|p| !p.placed_entities.is_empty());
        if event.powered && placed {
            turn_on_heater(world, heater);
        } else {
            turn_off_heater(world, heater);
        }
    });
    world.subscribe::<SolutionHeater, ItemPlaced>(|world, heater, _| {
        try_turn_on_heater(world, heater);
    });
    world.subscribe::<SolutionHeater, ItemRemoved>(|world, heater, _| {
        if world
            .get::<ItemPlacer>(heater)
            .is_some_and(|p| p.placed_entities.is_empty())
        {
            turn_off_heater(world, heater);
        }
    });
    world.add_game_systems(Update, (update_mixers, update_heaters));
}

#[cfg(test)]
mod tests;
