//! Стопки: листы, бинты, плитки и другие предметы, которые складываются в одну сущность с
//! числом. Образец — `Content.Shared/Stacks` и `Content.Server/Stack` SS14 @ 2f4a2aad
//! (`StackComponent`, `StackPrototype`, `SharedStackSystem`, `StackSystem.Split`).

use astrocore_ecs::{EventWorldExt, LifecycleWorldExt, MapInit, RegistryWorldExt};
use astrocore_map::{SpawnWorldExt, TransformWorldExt};
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::examine::{Examined, loc};
use crate::hands::HandsWorldExt;
use crate::interaction::InteractUsing;
use crate::popups::{PopupType, PopupWorldExt};
use crate::verbs::{GetVerbs, Verb, VerbKind};

/// Ключи внешнего вида (`StackVisuals`).
pub const VISUALS_ACTUAL: &str = "enum.StackVisuals.Actual";
pub const VISUALS_MAX_COUNT: &str = "enum.StackVisuals.MaxCount";
pub const VISUALS_HIDE: &str = "enum.StackVisuals.Hide";
/// Сколько отделить через меню (`DefaultSplitAmounts`).
pub const SPLIT_AMOUNTS: [i32; 6] = [1, 5, 10, 20, 30, 50];

fn thirty() -> i32 {
    30
}

fn yes() -> bool {
    true
}

/// Как слои спрайта следуют числу (`StackLayerFunction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StackLayerFunction {
    #[default]
    None,
    Threshold,
}

/// Стопка. В данных — компонент `Stack`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stack {
    pub stack_type: String,
    #[serde(default = "thirty")]
    pub count: i32,
    #[serde(default)]
    pub max_count_override: Option<i32>,
    #[serde(default)]
    pub unlimited: bool,
    #[serde(default)]
    pub throw_individually: bool,
    #[serde(default)]
    pub base_layer: String,
    #[serde(default, rename = "composite")]
    pub is_composite: bool,
    #[serde(default)]
    pub layer_states: Vec<String>,
    #[serde(default)]
    pub layer_function: StackLayerFunction,
    #[serde(default = "yes")]
    pub half_on_alt_interact: bool,
    #[serde(default = "yes")]
    pub animate_pickup: bool,
}

/// Пороги слоёв спрайта по числу. В данных — компонент `StackLayerThreshold`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StackLayerThreshold {
    #[serde(default)]
    pub thresholds: Vec<i32>,
}

/// Число в стопке изменилось (`StackCountChangedEvent`). На стопке.
#[derive(Debug, Clone, Copy)]
pub struct StackCountChanged {
    pub old_count: i32,
    pub new_count: i32,
}

/// От стопки отделили новую (`StackSplitEvent`). На старой стопке.
#[derive(Debug, Clone, Copy)]
pub struct StackSplit {
    pub new_stack: Entity,
    pub user: Option<Entity>,
}

/// Прототип стопки: что создавать при разделении и наибольшее число.
fn stack_prototype(world: &World, stack_type: &str) -> Option<(String, Option<i32>)> {
    let node = world
        .get_resource::<Prototypes>()?
        .resolved_node("stack", stack_type)?;
    let spawn = node.get("spawn")?.as_str()?.to_owned();
    let max = node
        .get("maxCount")
        .and_then(|n| n.as_str())
        .and_then(|t| t.parse().ok());
    Some((spawn, max))
}

/// Стопки на `World`.
pub trait StackWorldExt {
    /// Число в стопке; не стопка — 1 (`GetCount`).
    fn stack_count(&self, entity: Entity) -> i32;
    /// Наибольшее число (`GetMaxCount`).
    fn stack_max_count(&self, entity: Entity) -> i32;
    /// Свободное место (`GetAvailableSpace`).
    fn stack_space(&self, entity: Entity) -> i32;
    /// Задать число; ноль удаляет стопку (`SetCount`).
    fn set_stack_count(&mut self, entity: Entity, amount: i32);
    /// Уменьшить, если стопка не бесконечная (`ReduceCount`).
    fn reduce_stack(&mut self, entity: Entity, amount: i32);
    /// Потратить, если хватает (`TryUse`).
    fn try_use_stack(&mut self, entity: Entity, amount: i32) -> bool;
    /// Переложить из одной стопки в другую того же вида (`TryMergeStacks`); сколько
    /// переложено.
    fn try_merge_stacks(
        &mut self,
        donor: Entity,
        recipient: Entity,
        amount: Option<i32>,
    ) -> Option<i32>;
    /// Отделить новую стопку рядом с сущностью (`Split`).
    fn split_stack(
        &mut self,
        entity: Entity,
        amount: i32,
        at: Entity,
        user: Option<Entity>,
    ) -> Option<Entity>;
    /// Отделить в руку (`UserSplit`).
    fn user_split(&mut self, entity: Entity, user: Entity, amount: i32);
}

impl StackWorldExt for World {
    fn stack_count(&self, entity: Entity) -> i32 {
        self.get::<Stack>(entity).map_or(1, |s| s.count)
    }

    fn stack_max_count(&self, entity: Entity) -> i32 {
        let Some(stack) = self.get::<Stack>(entity) else {
            return 1;
        };
        if let Some(max) = stack.max_count_override {
            return max;
        }
        stack_prototype(self, &stack.stack_type)
            .and_then(|(_, max)| max)
            .unwrap_or(i32::MAX)
    }

    fn stack_space(&self, entity: Entity) -> i32 {
        self.stack_max_count(entity)
            .saturating_sub(self.stack_count(entity))
    }

    fn set_stack_count(&mut self, entity: Entity, amount: i32) {
        let Some(old) = self.get::<Stack>(entity).map(|s| s.count) else {
            return;
        };
        if amount == old {
            return;
        }
        let amount = amount.min(self.stack_max_count(entity)).max(0);
        if let Some(mut stack) = self.get_mut::<Stack>(entity) {
            stack.count = amount;
        }
        self.set_appearance(entity, VISUALS_ACTUAL, i64::from(amount));
        self.raise(
            entity,
            &mut StackCountChanged {
                old_count: old,
                new_count: amount,
            },
        );
        if amount <= 0 {
            self.queue_delete(entity);
        }
    }

    fn reduce_stack(&mut self, entity: Entity, amount: i32) {
        let Some(stack) = self.get::<Stack>(entity) else {
            return;
        };
        if stack.unlimited {
            return;
        }
        let count = stack.count;
        self.set_stack_count(entity, count - amount);
    }

    fn try_use_stack(&mut self, entity: Entity, amount: i32) -> bool {
        let Some(stack) = self.get::<Stack>(entity) else {
            return false;
        };
        if stack.unlimited {
            return true;
        }
        if amount > stack.count {
            return false;
        }
        let count = stack.count;
        self.set_stack_count(entity, count - amount);
        true
    }

    fn try_merge_stacks(
        &mut self,
        donor: Entity,
        recipient: Entity,
        amount: Option<i32>,
    ) -> Option<i32> {
        if donor == recipient || !self.is_alive(recipient) {
            return None;
        }
        let (Some(from), Some(to)) = (self.get::<Stack>(donor), self.get::<Stack>(recipient))
        else {
            return None;
        };
        if from.stack_type != to.stack_type {
            return None;
        }
        let (donor_count, recipient_count) = (from.count, to.count);
        let mut transferred = donor_count.min(self.stack_space(recipient));
        if transferred <= 0 {
            return None;
        }
        if let Some(amount) = amount.filter(|a| *a > 0) {
            transferred = transferred.min(amount);
        }
        self.set_stack_count(donor, donor_count - transferred);
        self.set_stack_count(recipient, recipient_count + transferred);
        Some(transferred)
    }

    fn split_stack(
        &mut self,
        entity: Entity,
        amount: i32,
        at: Entity,
        user: Option<Entity>,
    ) -> Option<Entity> {
        let stack_type = self.get::<Stack>(entity)?.stack_type.clone();
        let (spawn, _) = stack_prototype(self, &stack_type)?;
        let coordinates = self.coordinates(at)?;
        if !self.try_use_stack(entity, amount) {
            return None;
        }
        let new_stack = self.spawn_attached(&spawn, coordinates).ok()?;
        self.set_stack_count(new_stack, amount);
        // Отделённое от бесконечной стопки не бесконечно.
        if let Some(mut stack) = self.get_mut::<Stack>(new_stack) {
            stack.unlimited = false;
        }
        self.raise(entity, &mut StackSplit { new_stack, user });
        Some(new_stack)
    }

    fn user_split(&mut self, entity: Entity, user: Entity, amount: i32) {
        if amount <= 0 {
            let text = loc(self, "comp-stack-split-too-small", &[]);
            self.popup_cursor(&text, user, PopupType::Medium);
            return;
        }
        if let Some(held) = self.active_item(user)
            && self.try_merge_stacks(entity, held, Some(amount)).is_some()
        {
            let text = loc(self, "comp-stack-split", &[]);
            self.popup_cursor(&text, user, PopupType::Small);
            return;
        }
        if self.stack_count(entity) <= amount {
            self.pickup_or_drop(Some(user), entity);
            return;
        }
        let Some(split) = self.split_stack(entity, amount, user, Some(user)) else {
            return;
        };
        self.pickup_or_drop(Some(user), split);
        let text = loc(self, "comp-stack-split", &[]);
        self.popup_cursor(&text, user, PopupType::Small);
    }
}

/// Клик стопкой по стопке того же вида перекладывает в неё (`OnStackInteractUsing`).
fn interact_using(world: &mut World, target: Entity, event: &mut InteractUsing) {
    if event.handled || world.get::<Stack>(event.used).is_none() {
        return;
    }
    let Some(transferred) = world.try_merge_stacks(target, event.used, None) else {
        return;
    };
    event.handled = true;
    let at = event.click;
    let user = event.user;
    if transferred > 0 {
        world.popup_coordinates_for(&format!("+{transferred}"), at, user, PopupType::Small);
        if world.stack_space(event.used) == 0 {
            let text = loc(world, "comp-stack-becomes-full", &[]);
            let mut lower = at;
            lower.position.y -= 0.5;
            world.popup_coordinates_for(&text, lower, user, PopupType::Small);
        }
    } else if world.stack_space(event.used) == 0 {
        let text = loc(world, "comp-stack-already-full", &[]);
        world.popup_coordinates_for(&text, at, user, PopupType::Small);
    }
}

/// Меню «Разделить»: столько, сколько есть, и половина (`OnStackAlternativeInteract`).
fn split_verbs(world: &mut World, stack: Entity, event: &mut GetVerbs) {
    if event.kind != VerbKind::Alternative || !event.can_access || !event.can_interact {
        return;
    }
    if world.get::<crate::hands::Hands>(event.user).is_none() {
        return;
    }
    let Some(component) = world.get::<Stack>(stack).cloned() else {
        return;
    };
    let user = event.user;
    let category = loc(world, "verb-categories-split", &[]);
    let mut priority = 0;
    for amount in SPLIT_AMOUNTS {
        if amount > component.count {
            continue;
        }
        let mut verb = Verb::new(VerbKind::Alternative, amount.to_string(), move |world| {
            world.user_split(stack, user, amount);
        });
        verb.category = Some(category.clone());
        verb.priority = priority;
        priority -= 1;
        event.verbs.push(verb);
    }
    let half = (component.count + 1) / 2;
    let text = loc(world, "comp-stack-split-halve", &[]);
    let mut verb = Verb::new(VerbKind::Alternative, text, move |world| {
        world.user_split(stack, user, half);
    });
    verb.category = Some(category);
    verb.priority = if component.half_on_alt_interact {
        1
    } else {
        priority - 1
    };
    event.verbs.push(verb);
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<Stack>();
    world.register_net_serde::<Stack>();
    world.register_data_component::<StackLayerThreshold>();
    world.register_net_serde::<StackLayerThreshold>();
    world.subscribe::<Stack, Examined>(|world, stack, event| {
        if !event.is_in_details_range {
            return;
        }
        let count = world.stack_count(stack);
        let text = loc(
            world,
            "comp-stack-examine-detail-count",
            &[
                ("count", count.into()),
                ("markupCountColor", "lightgray".into()),
            ],
        );
        event.push_markup(text, 0);
    });
    if !side.is_server() {
        return;
    }
    world.subscribe::<Stack, MapInit>(|world, stack, _| {
        let count = world.stack_count(stack);
        let max = world.stack_max_count(stack);
        world.set_appearance(stack, VISUALS_ACTUAL, i64::from(count));
        world.set_appearance(stack, VISUALS_MAX_COUNT, i64::from(max));
        world.set_appearance(stack, VISUALS_HIDE, false);
    });
    world.subscribe::<Stack, InteractUsing>(interact_using);
    world.subscribe::<Stack, GetVerbs>(split_verbs);
}

#[cfg(test)]
mod tests;
