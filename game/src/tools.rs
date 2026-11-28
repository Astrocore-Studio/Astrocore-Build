//! Инструменты и их качества: лом поддевает, отвёртка крутит, кусачки режут. Действие
//! инструментом идёт с задержкой, по окончании звучит инструмент и система получает своё
//! событие. Мультитул переключает качества. Образец — `Content.Shared/Tools` SS14 @ 2f4a2aad
//! (`ToolComponent`, `ToolQualityPrototype`, `SharedToolSystem`, `MultipleToolComponent`).
//! Сварка ждёт растворов (фаза 7.2), работа с плитками — строительства (5.7).

use std::collections::BTreeSet;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{ComponentInit, EventWorldExt, RegistryWorldExt};
use astrocore_net::NetRegistryWorldExt;
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterStart, DoAfterWorldExt};
use crate::examine::{Examined, loc};
use crate::interaction::ActivateInWorld;

/// Качество «резать» (`CutQuality`).
pub const CUT_QUALITY: &str = "Cutting";
/// Качество «прозванивать» (`PulseQuality`).
pub const PULSE_QUALITY: &str = "Pulsing";

fn one() -> f32 {
    1.0
}

/// Инструмент. В данных — компонент `Tool`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    #[serde(default)]
    pub qualities: BTreeSet<String>,
    #[serde(default = "one")]
    pub speed_modifier: f32,
    #[serde(default)]
    pub use_sound: Option<SoundSpecifier>,
}

/// Режим мультитула (`MultipleToolComponent.ToolEntry`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEntry {
    pub behavior: BTreeSet<String>,
    #[serde(default)]
    pub use_sound: Option<SoundSpecifier>,
    #[serde(default)]
    pub change_sound: Option<SoundSpecifier>,
    /// Спрайт режима: смена вида мультитула пока не перенесена.
    #[serde(skip)]
    pub sprite: Option<String>,
}

fn yes() -> bool {
    true
}

/// Инструмент с несколькими режимами. В данных — компонент `MultipleTool`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultipleTool {
    pub entries: Vec<ToolEntry>,
    #[serde(default)]
    pub current_entry: u32,
    #[serde(default)]
    pub current_quality_name: String,
    #[serde(default = "yes")]
    pub status_show_behavior: bool,
}

/// Может ли действующий пользоваться инструментом (`ToolUserAttemptUseEvent`). На
/// действующем.
#[derive(Debug, Clone, Copy)]
pub struct ToolUserAttemptUse {
    pub target: Option<Entity>,
    pub cancelled: bool,
}

/// Позволяет ли инструмент (и цель) действие (`ToolUseAttemptEvent`). На инструменте и цели.
#[derive(Debug, Clone, Copy)]
pub struct ToolUseAttempt {
    pub user: Entity,
    pub fuel: f32,
    pub cancelled: bool,
}

/// Действие инструментом в процессе (`ToolDoAfterEvent`): обёртка события системы.
#[derive(Debug, Clone)]
pub struct ToolDoAfter<E> {
    pub fuel: f32,
    pub original_target: Option<Entity>,
    pub wrapped: E,
}

/// Действие инструментом продолжается (`DoAfterAttemptEvent<ToolDoAfterEvent>`): инструмент
/// может отменить (сварка без топлива). На инструменте.
#[derive(Debug, Clone, Copy)]
pub struct ToolDoAfterAttempt {
    pub user: Entity,
    pub fuel: f32,
    pub cancelled: bool,
}

/// Действие инструментом закончилось (`ToolDoAfterEvent` на инструменте): сварка тратит
/// топливо. На инструменте.
#[derive(Debug, Clone, Copy)]
pub struct ToolDoAfterFinished {
    pub user: Entity,
    pub fuel: f32,
    pub cancelled: bool,
}

/// Название качества по прототипу `tool`.
pub fn quality_name(world: &World, quality: &str) -> String {
    let key = world
        .get_resource::<Prototypes>()
        .and_then(|p| p.resolved_node("tool", quality))
        .and_then(|node| node.get("name"))
        .and_then(|name| name.as_str())
        .map(str::to_owned);
    match key {
        Some(key) => loc(world, &key, &[]),
        None => quality.to_owned(),
    }
}

/// Список через запятую с «и» (`FormatList`).
pub fn format_list(world: &World, items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [a, b] => format!("{a} {} {b}", loc(world, "generic-and", &[])),
        [rest @ .., last] => format!(
            "{}, {} {last}",
            rest.join(", "),
            loc(world, "generic-and", &[])
        ),
    }
}

/// Названия качеств (`GetQualitiesText`).
pub fn qualities_text<'a>(
    world: &World,
    qualities: impl IntoIterator<Item = &'a String>,
    lowercase: bool,
) -> String {
    let names: Vec<String> = qualities
        .into_iter()
        .map(|quality| {
            let name = quality_name(world, quality);
            if lowercase { name.to_lowercase() } else { name }
        })
        .collect();
    format_list(world, &names)
}

/// Инструменты на `World`.
pub trait ToolWorldExt {
    fn has_quality(&self, tool: Entity, quality: &str) -> bool;
    fn has_all_qualities(&self, tool: Entity, qualities: &[&str]) -> bool;
    /// Начать действие инструментом (`UseTool`): итог `DoAfterDone<E>` придёт на цель
    /// (без цели — широковещательно). Тип `E` регистрируется [`register_tool_event`].
    #[allow(clippy::too_many_arguments)]
    fn use_tool<E: Clone + Send + Sync + 'static>(
        &mut self,
        tool: Entity,
        user: Entity,
        target: Option<Entity>,
        delay: f64,
        qualities: &[&str],
        event: E,
        fuel: f32,
    ) -> bool;
    /// Следующий режим мультитула (`CycleMultipleTool`).
    fn cycle_multiple_tool(&mut self, tool: Entity, user: Option<Entity>) -> bool;
}

impl ToolWorldExt for World {
    fn has_quality(&self, tool: Entity, quality: &str) -> bool {
        self.get::<Tool>(tool)
            .is_some_and(|t| t.qualities.contains(quality))
    }

    fn has_all_qualities(&self, tool: Entity, qualities: &[&str]) -> bool {
        self.get::<Tool>(tool)
            .is_some_and(|t| qualities.iter().all(|q| t.qualities.contains(*q)))
    }

    fn use_tool<E: Clone + Send + Sync + 'static>(
        &mut self,
        tool: Entity,
        user: Entity,
        target: Option<Entity>,
        delay: f64,
        qualities: &[&str],
        event: E,
        fuel: f32,
    ) -> bool {
        let Some(speed) = self.get::<Tool>(tool).map(|t| t.speed_modifier) else {
            return false;
        };
        if !can_start_tool_use(self, tool, user, target, fuel, qualities) {
            return false;
        }
        let mut args = DoAfterArgs::new(
            user,
            delay / f64::from(speed),
            target,
            Some(tool),
            Some(tool),
        );
        args.break_on_damage = true;
        args.break_on_move = true;
        args.need_hand = tool != user;
        let wrapped = ToolDoAfter {
            fuel,
            original_target: target,
            wrapped: event,
        };
        !matches!(
            self.try_start_do_after(args, wrapped),
            DoAfterStart::Refused
        )
    }

    fn cycle_multiple_tool(&mut self, tool: Entity, user: Option<Entity>) -> bool {
        let Some(mut multiple) = self.get_mut::<MultipleTool>(tool) else {
            return false;
        };
        if multiple.entries.is_empty() {
            return false;
        }
        multiple.current_entry = (multiple.current_entry + 1) % multiple.entries.len() as u32;
        set_multiple_tool(self, tool, true, user);
        true
    }
}

/// Подходит ли инструмент и не против ли действующий, инструмент и цель
/// (`CanStartToolUse`).
fn can_start_tool_use(
    world: &mut World,
    tool: Entity,
    user: Entity,
    target: Option<Entity>,
    fuel: f32,
    qualities: &[&str],
) -> bool {
    if !world.has_all_qualities(tool, qualities) {
        return false;
    }
    let mut user_attempt = ToolUserAttemptUse {
        target,
        cancelled: false,
    };
    world.raise(user, &mut user_attempt);
    if user_attempt.cancelled {
        return false;
    }
    let mut attempt = ToolUseAttempt {
        user,
        fuel,
        cancelled: false,
    };
    world.raise(tool, &mut attempt);
    if attempt.cancelled {
        return false;
    }
    if let Some(target) = target.filter(|t| *t != tool) {
        world.raise(target, &mut attempt);
    }
    !attempt.cancelled
}

/// Применить текущий режим мультитула к инструменту (`SetMultipleTool`).
fn set_multiple_tool(world: &mut World, tool: Entity, play_sound: bool, _user: Option<Entity>) {
    let Some(multiple) = world.get::<MultipleTool>(tool).cloned() else {
        return;
    };
    if world.get::<Tool>(tool).is_none() {
        return;
    }
    let Some(current) = multiple
        .entries
        .get(multiple.current_entry as usize)
        .cloned()
    else {
        let none = loc(world, "multiple-tool-component-no-behavior", &[]);
        if let Some(mut multiple) = world.get_mut::<MultipleTool>(tool) {
            multiple.current_quality_name = none;
        }
        return;
    };
    if let Some(mut component) = world.get_mut::<Tool>(tool) {
        component.use_sound = current.use_sound.clone();
        component.qualities = current.behavior.clone();
    }
    if play_sound && let Some(sound) = &current.change_sound {
        world.play_entity(sound, tool, None);
    }
    if let Some(first) = current.behavior.iter().next() {
        let name = quality_name(world, first);
        if let Some(mut multiple) = world.get_mut::<MultipleTool>(tool) {
            multiple.current_quality_name = name;
        }
    }
}

/// Подписывает событие системы на окончание действия инструментом: звук инструмента и
/// `DoAfterDone<E>` на исходной цели (`OnDoAfter`).
pub fn register_tool_event<E: Clone + Send + Sync + 'static>(world: &mut World) {
    world.subscribe::<Tool, crate::do_after::DoAfterAttempt<ToolDoAfter<E>>>(
        |world, tool, event| {
            let mut attempt = ToolDoAfterAttempt {
                user: event.user,
                fuel: event.event.fuel,
                cancelled: false,
            };
            world.raise(tool, &mut attempt);
            event.cancelled |= attempt.cancelled;
        },
    );
    world.subscribe::<Tool, DoAfterDone<ToolDoAfter<E>>>(|world, tool, event| {
        world.raise(
            tool,
            &mut ToolDoAfterFinished {
                user: event.user,
                fuel: event.event.fuel,
                cancelled: event.cancelled,
            },
        );
        if !event.cancelled
            && let Some(sound) = world.get::<Tool>(tool).and_then(|t| t.use_sound.clone())
        {
            world.play_entity(&sound, tool, None);
        }
        let mut inner = DoAfterDone {
            event: event.event.wrapped.clone(),
            user: event.user,
            target: event.target,
            used: event.used,
            cancelled: event.cancelled,
            handled: false,
            repeat: false,
            delay: event.delay,
        };
        match event.event.original_target {
            Some(target) => world.raise(target, &mut inner),
            None => world.raise_broadcast(&mut inner),
        }
        event.handled |= inner.handled;
        event.repeat = inner.repeat;
        event.delay = inner.delay;
    });
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<Tool>();
    world.register_data_component::<MultipleTool>();
    world.register_net_serde::<Tool>();
    world.register_net_serde::<MultipleTool>();
    world.subscribe::<Tool, Examined>(|world, tool, event| {
        let Some(qualities) = world.get::<Tool>(tool).map(|t| t.qualities.clone()) else {
            return;
        };
        if qualities.is_empty() {
            return;
        }
        let text = qualities_text(world, &qualities, false);
        let line = loc(
            world,
            "tool-component-qualities",
            &[("qualities", text.as_str().into())],
        );
        event.push_markup(line, 0);
    });
    if !side.is_server() {
        return;
    }
    world.subscribe::<MultipleTool, ComponentInit>(|world, tool, _| {
        set_multiple_tool(world, tool, false, None);
    });
    world.subscribe::<MultipleTool, ActivateInWorld>(|world, tool, event| {
        if event.handled || !event.complex {
            return;
        }
        event.handled = world.cycle_multiple_tool(tool, Some(event.user));
    });
}

#[cfg(test)]
mod tests;
