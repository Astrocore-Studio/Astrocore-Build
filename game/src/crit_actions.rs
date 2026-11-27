//! Действия по состоянию моба и в критическом состоянии: при смене состояния сущность
//! получает действия этого состояния (`MobStateActions`, у людей в крите — «Сдаться»,
//! «Притвориться мёртвым», «Последние слова»). «Сдаться» — стать призраком; «Притвориться
//! мёртвым» — предсмертный вздох; «Последние слова» — окно ввода до 30 символов, шёпот с
//! «...» и призрак. Предсмертный вздох звучит и при переходе из крита в смерть. Образец —
//! `MobStateActionsSystem`, `MobStateActionsComponent`, `CritMobActionsSystem`,
//! `DeathgaspSystem`, `DeathgaspComponent` SS14 @ 2f4a2aad.
//!
//! Немота (`MutedStatusEffect`) не проверяется: статус-эффектов пока нет.

use std::collections::BTreeMap;

use astrocore_ecs::{ComponentInit, EventWorldExt, LifecycleWorldExt, RegistryWorldExt};
use astrocore_net::Actor;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::actions::{ActionEvent, ActionsWorldExt};
use crate::chat::{ChatTransmitRange, InGameIcChatType};
use crate::examine::loc;
use crate::mobs::{MobState, MobStateChanged, MobStateComponent, MobStateWorldExt};

/// Наибольшая длина последних слов (`MaxLastWordsLength`).
pub const MAX_LAST_WORDS_LENGTH: usize = 30;

/// Действия по состояниям (`MobStateActionsComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MobStateActions {
    pub actions: BTreeMap<MobState, Vec<String>>,
    #[serde(skip)]
    pub granted: Vec<Entity>,
}

fn default_deathgasp() -> String {
    "DefaultDeathgasp".into()
}

/// Предсмертный вздох (`DeathgaspComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Deathgasp {
    pub prototype: String,
}

impl Default for Deathgasp {
    fn default() -> Self {
        Self {
            prototype: default_deathgasp(),
        }
    }
}

/// Выдать действия состояния, забрав прежние (`ComposeActions`).
fn compose_actions(world: &mut World, entity: Entity, state: MobState) {
    let Some(component) = world.get::<MobStateActions>(entity).cloned() else {
        return;
    };
    for action in component.granted {
        world.remove_action(entity, action);
        world.delete_entity(action);
    }
    let granted: Vec<Entity> = component
        .actions
        .get(&state)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|prototype| world.add_action(entity, prototype, None))
        .collect();
    if let Some(mut component) = world.get_mut::<MobStateActions>(entity) {
        component.granted = granted;
    }
}

/// Предсмертный вздох (`Deathgasp`).
pub fn deathgasp(world: &mut World, entity: Entity) -> bool {
    let Some(emote) = world.get::<Deathgasp>(entity).map(|d| d.prototype.clone()) else {
        return false;
    };
    crate::chat::try_emote_with_chat_ext(world, entity, &emote, ChatTransmitRange::Normal, true);
    true
}

fn session_of(world: &World, entity: Entity) -> Option<Entity> {
    world.get::<Actor>(entity).map(|a| a.session)
}

/// «Сдаться» (`OnSuccumb`): в крите — призрак.
fn succumb(world: &mut World, entity: Entity) -> bool {
    let Some(session) = session_of(world, entity) else {
        return false;
    };
    if !world.is_mob_critical(entity) {
        return false;
    }
    let _ = crate::ghost::ghost_for_session(world, session);
    true
}

/// «Притвориться мёртвым» (`OnFakeDeath`).
fn fake_death(world: &mut World, entity: Entity) -> bool {
    if !world.is_mob_critical(entity) {
        return false;
    }
    deathgasp(world, entity)
}

/// Последние слова обрезаются до 30 символов и кончаются многоточием.
pub fn last_words_text(text: &str) -> String {
    let mut words: String = text.chars().take(MAX_LAST_WORDS_LENGTH).collect();
    words.push_str("...");
    words
}

/// «Последние слова» (`OnLastWords`): окно ввода, затем шёпот и призрак.
fn last_words(world: &mut World, entity: Entity) -> bool {
    let Some(session) = session_of(world, entity) else {
        return false;
    };
    let title = loc(world, "action-name-crit-last-words", &[]);
    crate::quick_dialog::open_text_dialog(world, session, &title, "", move |world, text| {
        // Разорванный или удалённый не говорит.
        if world.get_entity(entity).is_err() {
            return;
        }
        let attached = world
            .get::<astrocore_net::Session>(session)
            .and_then(|s| s.attached);
        if attached != Some(entity) || !world.is_mob_critical(entity) {
            return;
        }
        let words = last_words_text(&text);
        crate::chat::try_send_in_game_ic_message(
            world,
            entity,
            &words,
            InGameIcChatType::Whisper,
            ChatTransmitRange::Normal,
            false,
            None,
            None,
            false,
            true,
        );
        let _ = crate::ghost::ghost_for_session(world, session);
    });
    true
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<MobStateActions>();
    world.register_data_component::<Deathgasp>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<MobStateActions, MobStateChanged>(|world, entity, event| {
        compose_actions(world, entity, event.new_state);
    });
    world.subscribe::<MobStateComponent, ComponentInit>(|world, entity, _| {
        if let Some(state) = world.mob_state(entity) {
            compose_actions(world, entity, state);
        }
    });
    // Вздох при смерти прямо из крита (`DeathgaspSystem.OnMobStateChanged`).
    world.subscribe::<Deathgasp, MobStateChanged>(|world, entity, event| {
        if event.new_state == MobState::Dead && event.old_state == MobState::Critical {
            deathgasp(world, entity);
        }
    });
    world.subscribe::<MobStateActions, ActionEvent>(|world, entity, event| {
        if event.handled || event.performer != entity {
            return;
        }
        event.handled = match event.kind.as_str() {
            "CritSuccumbEvent" => succumb(world, entity),
            "CritFakeDeathEvent" => fake_death(world, entity),
            "CritLastWordsEvent" => last_words(world, entity),
            _ => return,
        };
    });
}

#[cfg(test)]
mod tests;
