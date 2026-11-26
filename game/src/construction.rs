//! Строительство и разборка по графам: сущность стоит в узле графа, рёбра ведут в другие
//! узлы через шаги — вставить материал, применить инструмент, вставить предмет с тегом или
//! компонентом. Образец — `ConstructionComponent`, `ConstructionGraphPrototype`,
//! `ConstructionGraphNode`/`Edge`/шаги, `ConstructionSystem.Interactions` (`HandleEvent`,
//! `HandleNode`, `HandleEdge`, `HandleStep`, `HandleInteraction`) и `.Graph` (`ChangeNode`,
//! `ChangeEntity`), действия (`Completions`) и условия (`Conditions`) SS14 @ 2f4a2aad.
//!
//! Действия и условия, которых ещё нет, пропускаются (действие) или запрещают ребро
//! (условие): так незаконченные графы не дают разобрать то, что нельзя.

use std::collections::{BTreeSet, HashMap};

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_data::DataNode;
use astrocore_ecs::{EventWorldExt, LifecycleWorldExt, RegistryWorldExt};
use astrocore_map::{
    ContainerKind, ContainerManager, ContainerWorldExt, EntityCoordinates, SpawnWorldExt,
    TileCoord, Transform, TransformWorldExt,
};
use astrocore_math::Angle;
use astrocore_net::{AppearanceWorldExt, NetEventWorldExt};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterStart, DoAfterWorldExt};
use crate::examine::loc;
use crate::hands::HandsWorldExt;
use crate::interaction::InteractUsing;
use crate::popups::{PopupType, PopupWorldExt};
use crate::stack::{Stack, StackWorldExt};
use crate::tag::TagWorldExt;
use crate::tools::{ToolWorldExt, register_tool_event};

/// Сущность в графе строительства (`ConstructionComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Construction {
    pub graph: String,
    #[serde(default)]
    pub node: String,
    #[serde(skip)]
    pub edge_index: Option<usize>,
    #[serde(skip)]
    pub step_index: usize,
    #[serde(default)]
    pub containers: BTreeSet<String>,
    #[serde(default)]
    pub target_node: Option<String>,
    #[serde(default = "start")]
    pub deconstruction_node: Option<String>,
}

fn start() -> Option<String> {
    Some("start".into())
}

/// Что надо сделать на шаге (`ConstructionGraphStep` и наследники).
#[derive(Debug, Clone, PartialEq)]
pub enum StepKind {
    /// Материал из стопки (`MaterialConstructionGraphStep`).
    Material { material: String, amount: i32 },
    /// Инструмент (`ToolConstructionGraphStep`).
    Tool { tool: String, fuel: f32 },
    /// Предмет с тегом (`TagConstructionGraphStep`).
    Tag(String),
    /// Предмет со всеми или любым из тегов (`MultipleTagsConstructionGraphStep`).
    Tags { all: Vec<String>, any: Vec<String> },
    /// Предмет с компонентом (`ComponentConstructionGraphStep`).
    Component(String),
    /// Шаг, которого ещё нет (температура, сборка из частей).
    Unsupported,
}

/// Шаг ребра.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub kind: StepKind,
    pub do_after: f64,
    pub store: Option<String>,
    pub completed: Vec<DataNode>,
    pub name: Option<String>,
    /// Сколько предметов вставить (`ArbitraryInsertConstructionGraphStep.Amount`).
    pub amount: i32,
}

/// Ребро (`ConstructionGraphEdge`).
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub to: String,
    pub steps: Vec<Step>,
    pub conditions: Vec<DataNode>,
    pub completed: Vec<DataNode>,
}

/// Узел (`ConstructionGraphNode`).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    pub name: String,
    pub entity: Option<String>,
    pub edges: Vec<Edge>,
    pub actions: Vec<DataNode>,
}

/// Граф (`ConstructionGraphPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    pub id: String,
    pub start: String,
    pub nodes: HashMap<String, GraphNode>,
}

fn text(node: &DataNode, key: &str) -> Option<String> {
    node.get(key).and_then(DataNode::as_str).map(str::to_owned)
}

fn list(node: &DataNode, key: &str) -> Vec<DataNode> {
    node.get(key)
        .and_then(DataNode::as_seq)
        .map(<[DataNode]>::to_vec)
        .unwrap_or_default()
}

fn strings(node: &DataNode, key: &str) -> Vec<String> {
    list(node, key)
        .iter()
        .filter_map(|n| n.as_str().map(str::to_owned))
        .collect()
}

fn parse_step(node: &DataNode) -> Step {
    let number = |key: &str| text(node, key).and_then(|v| v.parse::<f64>().ok());
    let kind = if let Some(material) = text(node, "material") {
        StepKind::Material {
            material,
            amount: number("amount").map_or(1, |a| a as i32),
        }
    } else if let Some(tool) = text(node, "tool") {
        StepKind::Tool {
            tool,
            fuel: number("fuel").map_or(10.0, |f| f as f32),
        }
    } else if let Some(tag) = text(node, "tag") {
        StepKind::Tag(tag)
    } else if node.get("allTags").is_some() || node.get("anyTags").is_some() {
        StepKind::Tags {
            all: strings(node, "allTags"),
            any: strings(node, "anyTags"),
        }
    } else if let Some(component) = text(node, "component") {
        StepKind::Component(component)
    } else {
        StepKind::Unsupported
    };
    Step {
        kind,
        do_after: number("doAfter").unwrap_or(0.0),
        store: text(node, "store"),
        completed: list(node, "completed"),
        name: text(node, "name"),
        amount: number("amount").map_or(1, |a| a as i32),
    }
}

impl Graph {
    pub fn from_node(id: &str, node: &DataNode) -> Self {
        let mut nodes = HashMap::new();
        for item in list(node, "graph") {
            let Some(name) = text(&item, "node") else {
                continue;
            };
            let edges = list(&item, "edges")
                .iter()
                .filter_map(|edge| {
                    Some(Edge {
                        to: text(edge, "to")?,
                        steps: list(edge, "steps").iter().map(parse_step).collect(),
                        conditions: list(edge, "conditions"),
                        completed: list(edge, "completed"),
                    })
                })
                .collect();
            nodes.insert(
                name.clone(),
                GraphNode {
                    name,
                    // `BoardNodeEntity` и подобные пока не переносятся: узел без смены сущности.
                    entity: item
                        .get("entity")
                        .and_then(DataNode::as_str)
                        .map(str::to_owned),
                    edges,
                    actions: list(&item, "actions"),
                },
            );
        }
        Self {
            id: id.to_owned(),
            start: text(node, "start").unwrap_or_else(|| "start".into()),
            nodes,
        }
    }

    /// Путь узлов от `from` до `to` (`ConstructionSystem.Pathfinding`, поиск в ширину).
    pub fn path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        let mut previous: HashMap<String, String> = HashMap::new();
        let mut queue = std::collections::VecDeque::from([from.to_owned()]);
        let mut seen = BTreeSet::from([from.to_owned()]);
        while let Some(node) = queue.pop_front() {
            if node == to {
                let mut path = vec![node.clone()];
                let mut current = node;
                while let Some(prev) = previous.get(&current) {
                    path.push(prev.clone());
                    current = prev.clone();
                }
                path.reverse();
                return Some(path);
            }
            for edge in self
                .nodes
                .get(&node)
                .map(|n| n.edges.as_slice())
                .unwrap_or_default()
            {
                if seen.insert(edge.to.clone()) {
                    previous.insert(edge.to.clone(), node.clone());
                    queue.push_back(edge.to.clone());
                }
            }
        }
        None
    }
}

/// Графы из прототипов `constructionGraph`. Ресурс, заполняется при обращении.
#[derive(Resource, Debug, Default)]
pub struct ConstructionGraphs(HashMap<String, Graph>);

/// Граф по имени.
pub fn graph(world: &mut World, id: &str) -> Option<Graph> {
    if let Some(graph) = world
        .get_resource::<ConstructionGraphs>()
        .and_then(|g| g.0.get(id))
    {
        return Some(graph.clone());
    }
    let graph = Graph::from_node(
        id,
        world
            .get_resource::<Prototypes>()?
            .resolved_node("constructionGraph", id)?,
    );
    world
        .get_resource_or_insert_with(ConstructionGraphs::default)
        .0
        .insert(id.to_owned(), graph.clone());
    Some(graph)
}

/// Итог обработки (`HandleResult`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleResult {
    False,
    Validated,
    True,
    DoAfter,
}

/// Действие игрока над сущностью: предмет в руке и (после ожидания) признак завершения.
#[derive(Debug, Clone, Copy)]
struct Interaction {
    user: Entity,
    used: Entity,
    completed: bool,
}

/// Ожидание шага строительства (`ConstructionInteractDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
pub struct ConstructionDoAfter;

// Условия.

fn flag(node: &DataNode, key: &str, default: bool) -> bool {
    text(node, key).map_or(default, |v| v.eq_ignore_ascii_case("true"))
}

/// Условие ребра (`IGraphCondition`).
pub fn check_condition(world: &World, entity: Entity, condition: &DataNode) -> bool {
    match condition.type_tag() {
        Some("EntityAnchored") => {
            let anchored = world
                .get::<Transform>(entity)
                .is_some_and(Transform::anchored);
            anchored == flag(condition, "anchored", true)
        }
        Some("WirePanel") => {
            let open = world
                .get::<crate::wires::WiresPanel>(entity)
                .is_none_or(|p| p.open);
            open == flag(condition, "open", true)
        }
        Some("ContainerNotEmpty") | Some("ContainerEmpty") => {
            let Some(container) = text(condition, "container") else {
                return false;
            };
            let empty = world
                .container(entity, &container)
                .is_none_or(|c| c.entities().is_empty());
            empty == (condition.type_tag() == Some("ContainerEmpty"))
        }
        Some("StorageWelded") => {
            crate::weldable::is_welded(world, entity) == flag(condition, "welded", true)
        }
        Some("DoorWelded") => {
            crate::weldable::is_welded(world, entity) == flag(condition, "welded", true)
        }
        Some("DoorBolted") => {
            crate::doors::is_bolted(world, entity) == flag(condition, "value", true)
        }
        Some("Locked") => crate::lock::is_locked(world, entity) == flag(condition, "locked", true),
        Some("HasTag") => text(condition, "tag").is_some_and(|tag| world.has_tag(entity, &tag)),
        Some("AllWiresCut") => {
            let value = flag(condition, "value", true);
            world
                .get::<crate::wires::Wires>(entity)
                .is_none_or(|wires| wires.wires.iter().all(|w| w.is_cut == value))
        }
        Some("AllConditions") => list(condition, "conditions")
            .iter()
            .all(|c| check_condition(world, entity, c)),
        Some("AnyConditions") => list(condition, "conditions")
            .iter()
            .any(|c| check_condition(world, entity, c)),
        _ => false,
    }
}

fn check_conditions(world: &World, entity: Entity, conditions: &[DataNode]) -> bool {
    conditions.iter().all(|c| check_condition(world, entity, c))
}

// Действия.

fn spawn_prototype(
    world: &mut World,
    prototype: &str,
    amount: i32,
    at: EntityCoordinates,
) -> Vec<Entity> {
    let is_stack = world
        .get_resource::<Prototypes>()
        .and_then(|p| p.entity(prototype))
        .is_some_and(|p| p.component("Stack").is_some());
    if is_stack {
        let Ok(stack) = world.spawn_attached(prototype, at) else {
            return Vec::new();
        };
        world.set_stack_count(stack, amount);
        vec![stack]
    } else {
        (0..amount)
            .filter_map(|_| world.spawn_attached(prototype, at).ok())
            .collect()
    }
}

/// Действие (`IGraphAction.PerformAction`).
pub fn perform_action(world: &mut World, entity: Entity, user: Option<Entity>, action: &DataNode) {
    let number = |key: &str| text(action, key).and_then(|v| v.parse::<i32>().ok());
    match action.type_tag() {
        Some("SpawnPrototype") => {
            let (Some(prototype), Some(at)) =
                (text(action, "prototype"), world.coordinates(entity))
            else {
                return;
            };
            spawn_prototype(world, &prototype, number("amount").unwrap_or(1), at);
        }
        Some("GivePrototype") => {
            let Some(prototype) = text(action, "prototype") else {
                return;
            };
            let target = user.unwrap_or(entity);
            let Some(at) = world.coordinates(target) else {
                return;
            };
            for item in spawn_prototype(world, &prototype, number("amount").unwrap_or(1), at) {
                world.pickup_or_drop(user, item);
            }
        }
        Some("DeleteEntity") | Some("DestroyEntity") => {
            let mut event = ConstructionBeforeDelete {
                user,
                cancelled: false,
            };
            world.raise(entity, &mut event);
            if !event.cancelled {
                world.queue_delete(entity);
            }
        }
        Some("SnapToGrid") => {
            let anchored = world
                .get::<Transform>(entity)
                .is_some_and(Transform::anchored);
            if anchored {
                return;
            }
            let Some(coordinates) = world.coordinates(entity) else {
                return;
            };
            let level = world.get::<Transform>(entity).map_or(0, Transform::level);
            let cell = TileCoord::containing(coordinates.position, level);
            let snapped =
                EntityCoordinates::on_level(coordinates.parent, cell.center(), coordinates.z);
            let rotation = flag(action, "southRotation", false).then_some(Angle::ZERO);
            let _ = world.set_coordinates(entity, snapped, rotation);
        }
        Some("SetAnchor") => {
            let value = flag(action, "value", true);
            let anchored = world
                .get::<Transform>(entity)
                .is_some_and(Transform::anchored);
            if anchored == value {
                return;
            }
            if value {
                world.anchor(entity);
            } else {
                world.unanchor(entity);
            }
        }
        Some("PlaySound") => {
            if let Some(sound) = action
                .get("sound")
                .and_then(|s| astrocore_data::from_node::<SoundSpecifier>(s).ok())
            {
                world.play_entity(&sound, entity, None);
            }
        }
        Some("EmptyAllContainers") => {
            let pickup = flag(action, "pickup", false);
            let names: Vec<String> = world
                .get::<ContainerManager>(entity)
                .map(|m| m.containers.keys().cloned().collect())
                .unwrap_or_default();
            for name in names {
                for item in world.empty_container(entity, &name) {
                    if pickup {
                        world.pickup_or_drop(user, item);
                    }
                }
            }
        }
        Some("EmptyContainer") => {
            if let Some(name) = text(action, "container") {
                let pickup = flag(action, "pickup", false);
                for item in world.empty_container(entity, &name) {
                    if pickup {
                        world.pickup_or_drop(user, item);
                    }
                }
            }
        }
        Some("DeleteEntitiesInContainer") => {
            if let Some(name) = text(action, "container") {
                for item in world.empty_container(entity, &name) {
                    world.queue_delete(item);
                }
            }
        }
        Some("SetStackCount") => world.set_stack_count(entity, number("amount").unwrap_or(1)),
        Some("PopupUser") | Some("PopupEveryone") => {
            if let Some(text) = text(action, "text") {
                let message = loc(world, &text, &[]);
                match (action.type_tag(), user) {
                    (Some("PopupUser"), Some(user)) => {
                        world.popup_entity_for(&message, entity, user, PopupType::Small)
                    }
                    _ => world.popup_entity(&message, entity, PopupType::Small),
                }
            }
        }
        Some("VisualizerDataInt") => {
            if let (Some(key), Some(value)) = (text(action, "key"), number("data")) {
                world.set_appearance(entity, &key, i64::from(value));
            }
        }
        Some("SetWiresPanelSecurity") => {
            if let Some(mut security) = world.get_mut::<crate::wires::WiresPanelSecurity>(entity) {
                security.examine = text(action, "examine");
                security.wires_accessible = flag(action, "wiresAccessible", true);
            }
        }
        Some("AppearanceChange") => {
            let key = text(action, "key").unwrap_or_else(|| "enum.ConstructionVisuals.Key".into());
            let value = text(action, "data").or_else(|| {
                let construction = world.get::<Construction>(entity)?.clone();
                let target = construction.edge_index.and_then(|index| {
                    graph(world, &construction.graph)?
                        .nodes
                        .get(&construction.node)?
                        .edges
                        .get(index)
                        .map(|e| e.to.clone())
                });
                Some(target.unwrap_or(construction.node))
            });
            if let Some(value) = value {
                world.set_appearance(entity, &key, value);
            }
        }
        // Журнал администратора ещё не перенесён (фаза 8).
        Some("AdminLog") => {}
        Some("AddContainer") => {
            if let Some(container) = text(action, "container") {
                add_container(world, entity, &container);
            }
        }
        Some("DamageEntity") => {
            if let Some(damage) = action
                .get("damage")
                .and_then(|d| astrocore_data::from_node::<crate::damage::DamageSpecifier>(d).ok())
            {
                crate::damage::DamageableWorldExt::change_damage(
                    world, entity, &damage, false, true, user,
                );
            }
        }
        Some("AttemptElectrocute") => {
            let Some(user) = user else {
                return;
            };
            let Some(enabled) = world
                .get::<crate::electrocution::Electrified>(entity)
                .map(|e| e.enabled)
            else {
                return;
            };
            if let Some(mut electrified) =
                world.get_mut::<crate::electrocution::Electrified>(entity)
            {
                electrified.enabled = true;
            }
            crate::electrocution::try_do_electrified_act(world, entity, user, 1.0);
            if let Some(mut electrified) =
                world.get_mut::<crate::electrocution::Electrified>(entity)
            {
                electrified.enabled = enabled;
            }
        }
        Some("ConditionalAction") => {
            let passed = action
                .get("condition")
                .is_some_and(|c| check_condition(world, entity, c));
            let key = if passed { "action" } else { "else" };
            if let Some(next) = action.get(key).cloned() {
                perform_action(world, entity, user, &next);
            }
        }
        _ => {}
    }
}

/// Контейнер, который переходит в новую сущность при смене (`AddContainer`).
pub fn add_container(world: &mut World, entity: Entity, container: &str) {
    if let Some(mut construction) = world.get_mut::<Construction>(entity) {
        construction.containers.insert(container.to_owned());
    }
}

fn perform_actions(world: &mut World, entity: Entity, user: Option<Entity>, actions: &[DataNode]) {
    for action in actions {
        if !world.is_alive(entity) {
            break;
        }
        perform_action(world, entity, user, action);
    }
}

/// Перед удалением при строительстве (`ConstructionBeforeDeleteEvent`).
#[derive(Debug, Clone, Copy)]
pub struct ConstructionBeforeDelete {
    pub user: Option<Entity>,
    pub cancelled: bool,
}

/// Сущность сменилась (`ConstructionChangeEntityEvent`): поднимается на старой и новой.
#[derive(Debug, Clone, Copy)]
pub struct ConstructionChangeEntity {
    pub new: Entity,
    pub old: Entity,
}

// Шаги.

fn entity_valid(world: &World, step: &StepKind, item: Entity) -> bool {
    match step {
        StepKind::Material { material, amount } => world
            .get::<Stack>(item)
            .is_some_and(|stack| &stack.stack_type == material && stack.count >= *amount),
        StepKind::Tag(tag) => world.has_tag(item, tag),
        StepKind::Tags { all, any } => {
            all.iter().all(|t| world.has_tag(item, t))
                && (any.is_empty() || any.iter().any(|t| world.has_tag(item, t)))
        }
        StepKind::Component(name) => crate::whitelist::has_component_named(world, item, name),
        StepKind::Tool { .. } | StepKind::Unsupported => false,
    }
}

/// Шаг (`HandleInteraction`).
fn handle_interaction(
    world: &mut World,
    entity: Entity,
    interaction: Interaction,
    step: &Step,
    validation: bool,
) -> HandleResult {
    match &step.kind {
        StepKind::Tool { tool, fuel } => {
            if validation {
                return if world.has_quality(interaction.used, tool) {
                    HandleResult::Validated
                } else {
                    HandleResult::False
                };
            }
            if interaction.completed {
                return HandleResult::True;
            }
            if world.use_tool(
                interaction.used,
                interaction.user,
                Some(entity),
                step.do_after,
                &[tool],
                ConstructionDoAfter,
                *fuel,
            ) {
                HandleResult::DoAfter
            } else {
                HandleResult::False
            }
        }
        StepKind::Unsupported => HandleResult::False,
        kind => {
            let item = interaction.used;
            if !entity_valid(world, kind, item) {
                return HandleResult::False;
            }
            if validation {
                return HandleResult::Validated;
            }
            if !interaction.completed && step.do_after > 0.0 {
                let mut args = DoAfterArgs::new(
                    interaction.user,
                    step.do_after,
                    Some(entity),
                    Some(item),
                    Some(entity),
                );
                args.break_on_damage = false;
                args.break_on_move = true;
                args.need_hand = true;
                return match world.try_start_do_after(args, ConstructionDoAfter) {
                    DoAfterStart::Refused => HandleResult::False,
                    _ => HandleResult::DoAfter,
                };
            }
            let mut insert = item;
            if let StepKind::Material { amount, .. } = kind {
                let Some(split) =
                    world.split_stack(item, *amount, interaction.user, Some(interaction.user))
                else {
                    return HandleResult::False;
                };
                insert = split;
            }
            match &step.store {
                Some(store) => {
                    if let Some(mut construction) = world.get_mut::<Construction>(entity) {
                        construction.containers.insert(store.clone());
                    }
                    world.ensure_container(entity, store, ContainerKind::List);
                    let _ = world.force_insert_into_container(insert, entity, store);
                }
                None => world.queue_delete(insert),
            }
            HandleResult::True
        }
    }
}

fn handle_step(
    world: &mut World,
    entity: Entity,
    interaction: Interaction,
    step: &Step,
    validation: bool,
) -> HandleResult {
    let result = handle_interaction(world, entity, interaction, step, validation);
    if result == HandleResult::True {
        perform_actions(world, entity, Some(interaction.user), &step.completed);
    }
    result
}

fn handle_edge(
    world: &mut World,
    entity: Entity,
    interaction: Interaction,
    edge: &Edge,
    validation: bool,
) -> HandleResult {
    let Some(step_index) = world.get::<Construction>(entity).map(|c| c.step_index) else {
        return HandleResult::False;
    };
    let Some(step) = edge.steps.get(step_index) else {
        return HandleResult::False;
    };
    if !check_conditions(world, entity, &edge.conditions) {
        return HandleResult::False;
    }
    let result = handle_step(world, entity, interaction, step, validation);
    if result != HandleResult::True {
        return result;
    }
    let finished = {
        let Some(mut construction) = world.get_mut::<Construction>(entity) else {
            return HandleResult::True;
        };
        construction.step_index += 1;
        construction.step_index >= edge.steps.len()
    };
    if finished {
        perform_actions(world, entity, Some(interaction.user), &edge.completed);
        if !world.is_alive(entity) || world.get::<Construction>(entity).is_none() {
            return HandleResult::True;
        }
        if let Some(mut construction) = world.get_mut::<Construction>(entity) {
            construction.edge_index = None;
            construction.step_index = 0;
        }
        change_node(world, entity, Some(interaction.user), &edge.to, true);
    }
    HandleResult::True
}

fn handle_node(
    world: &mut World,
    entity: Entity,
    interaction: Interaction,
    node: &GraphNode,
    validation: bool,
) -> HandleResult {
    if let Some(mut construction) = world.get_mut::<Construction>(entity) {
        construction.step_index = 0;
    }
    for (index, edge) in node.edges.iter().enumerate() {
        let result = handle_edge(world, entity, interaction, edge, validation);
        if result == HandleResult::False {
            continue;
        }
        if result == HandleResult::DoAfter {
            if let Some(mut construction) = world.get_mut::<Construction>(entity) {
                construction.edge_index = Some(index);
            }
        } else if result == HandleResult::True
            && world
                .get::<Construction>(entity)
                .is_some_and(|c| c.node == node.name)
            && let Some(mut construction) = world.get_mut::<Construction>(entity)
        {
            construction.edge_index = Some(index);
        }
        return result;
    }
    HandleResult::False
}

/// Обработать действие игрока (`HandleEvent`).
fn handle_event(
    world: &mut World,
    entity: Entity,
    interaction: Interaction,
    validation: bool,
) -> HandleResult {
    let Some(construction) = world.get::<Construction>(entity).cloned() else {
        return HandleResult::False;
    };
    let Some(graph) = graph(world, &construction.graph) else {
        return HandleResult::False;
    };
    let Some(node) = graph.nodes.get(&construction.node) else {
        return HandleResult::False;
    };
    if let Some(edge) = construction.edge_index.and_then(|i| node.edges.get(i)) {
        let result = handle_edge(world, entity, interaction, edge, validation);
        if !validation
            && result == HandleResult::False
            && world
                .get::<Construction>(entity)
                .is_some_and(|c| c.step_index == 0)
            && let Some(mut construction) = world.get_mut::<Construction>(entity)
        {
            construction.edge_index = None;
        }
        return result;
    }
    handle_node(world, entity, interaction, node, validation)
}

/// Перейти в узел (`ChangeNode`): при другой сущности узла — заменить сущность.
pub fn change_node(
    world: &mut World,
    entity: Entity,
    user: Option<Entity>,
    node: &str,
    perform: bool,
) -> bool {
    let Some(graph_id) = world.get::<Construction>(entity).map(|c| c.graph.clone()) else {
        return false;
    };
    let Some(graph) = graph(world, &graph_id) else {
        return false;
    };
    let Some(target) = graph.nodes.get(node) else {
        return false;
    };
    if let Some(mut construction) = world.get_mut::<Construction>(entity) {
        construction.node = node.to_owned();
    }
    if let Some(prototype) = &target.entity
        && change_entity(world, entity, user, prototype).is_some()
    {
        return true;
    }
    if perform {
        perform_actions(world, entity, user, &target.actions);
    }
    world.is_alive(entity)
}

/// Заменить сущность сущностью узла (`ChangeEntity`).
fn change_entity(
    world: &mut World,
    entity: Entity,
    user: Option<Entity>,
    prototype: &str,
) -> Option<Entity> {
    let current = world
        .get::<astrocore_ecs::MetaData>(entity)
        .and_then(|m| m.prototype.as_deref().map(str::to_owned));
    let exists = world
        .get_resource::<Prototypes>()
        .is_some_and(|p| p.entity(prototype).is_some());
    if current.as_deref() == Some(prototype) || !exists {
        return None;
    }
    let construction = world.get::<Construction>(entity).cloned()?;
    let coordinates = world.coordinates(entity)?;
    let rotation = world
        .get::<Transform>(entity)
        .map(Transform::local_rotation);
    let anchored = world
        .get::<Transform>(entity)
        .is_some_and(Transform::anchored);
    let new = world.spawn_attached(prototype, coordinates).ok()?;
    if let Some(rotation) = rotation {
        let _ = world.set_coordinates(new, coordinates, Some(rotation));
    }
    if anchored && !world.get::<Transform>(new).is_some_and(Transform::anchored) {
        world.anchor(new);
    } else if !anchored && world.get::<Transform>(new).is_some_and(Transform::anchored) {
        world.unanchor(new);
    }
    let mut new_construction = world
        .get::<Construction>(new)
        .cloned()
        .unwrap_or_else(|| construction.clone());
    new_construction
        .containers
        .extend(construction.containers.iter().cloned());
    if new_construction.graph == construction.graph {
        new_construction.node = construction.node.clone();
        new_construction.target_node = construction.target_node.clone();
    }
    world.entity_mut(new).insert(new_construction);
    for container in &construction.containers {
        let items: Vec<Entity> = world
            .container(entity, container)
            .map(|c| c.entities().to_vec())
            .unwrap_or_default();
        world.ensure_container(new, container, ContainerKind::List);
        for item in items {
            let _ = world.remove_from_container(item, None);
            let _ = world.force_insert_into_container(item, new, container);
        }
    }
    let mut event = ConstructionChangeEntity { new, old: entity };
    world.raise(entity, &mut event);
    world.raise_and_broadcast(new, &mut event);
    world.queue_delete(entity);
    let _ = user;
    Some(new)
}

// Постройка с нуля (`ConstructionSystem.Initial`).

/// Рецепт меню строительства (`ConstructionPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub graph: String,
    pub start_node: String,
    pub target_node: String,
    pub category: String,
    pub object_type: String,
    pub can_rotate: bool,
    pub can_build_in_impassable: bool,
    pub conditions: Vec<DataNode>,
    pub hide: bool,
}

/// Рецепт по имени.
pub fn recipe(world: &World, id: &str) -> Option<Recipe> {
    let node = world
        .get_resource::<Prototypes>()?
        .resolved_node("construction", id)?;
    Some(Recipe {
        id: id.to_owned(),
        graph: text(node, "graph")?,
        start_node: text(node, "startNode").unwrap_or_else(|| "start".into()),
        target_node: text(node, "targetNode")?,
        category: text(node, "category").unwrap_or_default(),
        object_type: text(node, "objectType").unwrap_or_else(|| "Structure".into()),
        can_rotate: flag(node, "canRotate", true),
        can_build_in_impassable: flag(node, "canBuildInImpassable", false),
        conditions: list(node, "conditions"),
        hide: flag(node, "hide", false),
    })
}

/// Все рецепты, не скрытые и не абстрактные, по порядку имён.
pub fn recipes(world: &World) -> Vec<Recipe> {
    let Some(prototypes) = world.get_resource::<Prototypes>() else {
        return Vec::new();
    };
    let mut ids: Vec<String> = prototypes.ids("construction").map(str::to_owned).collect();
    ids.sort();
    ids.into_iter()
        .filter(|id| {
            !prototypes
                .resolved_node("construction", id)
                .is_some_and(|n| flag(n, "abstract", false))
        })
        .filter_map(|id| recipe(world, &id))
        .filter(|r| !r.hide)
        .collect()
}

/// Где ставится постройка из меню: грид, клетка, направление, строитель.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub user: Entity,
    pub grid: Entity,
    pub cell: TileCoord,
    pub angle: Angle,
}

fn anchored(world: &World, grid: Entity, cell: TileCoord) -> Vec<Entity> {
    world
        .get::<astrocore_map::MapGrid>(grid)
        .map(|g| g.anchored_at(cell).to_vec())
        .unwrap_or_default()
}

fn tile_name(world: &World, grid: Entity, cell: TileCoord) -> Option<String> {
    let tile = world.get::<astrocore_map::MapGrid>(grid)?.tile(cell);
    world
        .get_resource::<astrocore_map::TileDefinitions>()?
        .get(tile.type_id)
        .map(|d| d.id.clone())
}

fn is_wall(world: &World, entity: Entity) -> bool {
    crate::whitelist::has_component_named(world, entity, "Wall")
}

/// Условие места постройки (`IConstructionCondition`). Неизвестное условие не пускает.
pub fn check_place_condition(world: &World, at: Placement, condition: &DataNode) -> bool {
    let Placement { grid, cell, .. } = at;
    match condition.type_tag() {
        Some("TileNotBlocked") => tile_not_blocked(world, grid, cell, condition),
        Some("EmptyOrWindowValidInTile") => {
            anchored(world, grid, cell)
                .iter()
                .any(|e| crate::whitelist::has_component_named(world, *e, "CanBuildWindowOnTop"))
                || tile_not_blocked(
                    world,
                    grid,
                    cell,
                    condition.get("tileNotBlocked").unwrap_or(&DataNode::null()),
                )
        }
        Some("NoWindowsInTile") => !anchored(world, grid, cell)
            .iter()
            .any(|e| world.has_tag(*e, "Window")),
        Some("NoUnstackableInTile") => !anchored(world, grid, cell)
            .iter()
            .any(|e| world.has_tag(*e, "Unstackable")),
        Some("TileType") => tile_name(world, grid, cell).is_some_and(|name| {
            list(condition, "targets")
                .iter()
                .any(|t| t.as_str() == Some(&name))
        }),
        Some("AgainstWall") => {
            let offset = condition
                .get("offset")
                .and_then(|n| astrocore_data::from_node::<Angle>(n).ok())
                .unwrap_or(Angle::degrees(180.0));
            let towards = (at.angle + offset).cardinal_direction();
            anchored(world, grid, cell.offset(towards.int_vec()))
                .iter()
                .any(|e| is_wall(world, *e))
        }
        Some("WallmountCondition") => wallmount(world, at),
        _ => false,
    }
}

/// `TileNotBlocked`: клетка не космос, прочная и без твёрдых закреплённых сущностей.
fn tile_not_blocked(world: &World, grid: Entity, cell: TileCoord, condition: &DataNode) -> bool {
    let Some(map_grid) = world.get::<astrocore_map::MapGrid>(grid) else {
        return false;
    };
    if map_grid.tile(cell).type_id == 0 && flag(condition, "failIfSpace", true) {
        return false;
    }
    if flag(condition, "failIfNotSturdy", true)
        && let Some(name) = tile_name(world, grid, cell)
        && world
            .get_resource::<crate::cables::TileProperties>()
            .is_some_and(|p| !p.is_sturdy(&name))
    {
        return false;
    }
    crate::flatpack::tile_free(world, grid, cell)
}

/// `WallmountCondition`: вещь вешается на стену лицом к строителю, и соседняя клетка по
/// направлению вещи — не стена. Луч SS14 заменён проверкой клеток: строят на соседней клетке.
fn wallmount(world: &World, at: Placement) -> bool {
    let Some(user_cell) = world.get::<Transform>(at.user).and_then(|t| {
        (t.grid() == Some(at.grid)).then(|| TileCoord::containing(t.local_position(), at.cell.z))
    }) else {
        return false;
    };
    let to_object = astrocore_math::Vec2::new(
        (at.cell.x - user_cell.x) as f32,
        (at.cell.y - user_cell.y) as f32,
    );
    if to_object == astrocore_math::Vec2::ZERO || at.angle.to_world_vec().dot(to_object) > 0.0 {
        return false;
    }
    let facing = at.angle.cardinal_direction();
    anchored(world, at.grid, at.cell)
        .iter()
        .any(|e| is_wall(world, *e))
        && !anchored(world, at.grid, at.cell.offset(facing.int_vec()))
            .iter()
            .any(|e| is_wall(world, *e))
}

/// Запрос клиента: построить рецепт в точке (`TryStartStructureConstructionMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartStructureConstruction {
    pub prototype: String,
    /// Точка на карте глаза игрока.
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    pub ack: u32,
}

/// Постройка ждёт окончания (`Construct`, ожидание).
#[derive(Debug, Clone)]
struct PendingBuild {
    ack: u32,
    recipe: Recipe,
    edge: Edge,
    grid: Entity,
    cell: TileCoord,
    angle: f32,
    /// Материалы шагов по порядку и контейнеры, куда их положить.
    items: Vec<(Entity, Option<String>)>,
}

/// Постройки игрока, ждущие окончания.
#[derive(Component, Debug, Clone, Default)]
pub struct PendingBuilds(Vec<PendingBuild>);

/// Ожидание постройки (`AwaitedDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
struct BuildDoAfter {
    ack: u32,
}

/// Предметы под рукой (`EnumerateNearby`): в руках, в слотах одежды (с содержимым сумок) и
/// рядом в пределах 2 м.
fn nearby_items(world: &mut World, user: Entity) -> Vec<Entity> {
    let mut items: Vec<Entity> = Vec::new();
    let with_storage = |world: &World, items: &mut Vec<Entity>, item: Entity| {
        if let Some(storage) = world.container(item, crate::storage::STORAGE_CONTAINER) {
            items.extend_from_slice(storage.entities());
        }
        items.push(item);
    };
    for item in world.held_items(user) {
        with_storage(world, &mut items, item);
    }
    let slots: Vec<String> = world
        .get::<crate::inventory::Inventory>(user)
        .map(|i| i.slots.iter().map(|s| s.name.clone()).collect())
        .unwrap_or_default();
    for slot in slots {
        if let Some(item) = crate::inventory::InventoryWorldExt::slot_entity(world, user, &slot) {
            with_storage(world, &mut items, item);
        }
    }
    if let Some(at) = world.map_coordinates(user) {
        for near in crate::entity_storage::entities_near(world, at, 2.0) {
            if near != user
                && !items.contains(&near)
                && world.containing_container(near).is_none()
                && crate::interaction::InteractionWorldExt::in_range_unobstructed_point(
                    world,
                    user,
                    world.map_coordinates(near).unwrap_or(at),
                    2.0,
                )
            {
                items.push(near);
            }
        }
    }
    items
}

/// Контейнер строителя для материалов шага: общий удаляется, именованные переходят в постройку.
fn holder_name(ack: u32, store: Option<&str>) -> String {
    match store {
        Some(store) => format!("construction-{ack}-{store}"),
        None => format!("construction-{ack}"),
    }
}

/// Собрать материалы ребра в контейнеры строителя (`Construct`). `None` — не хватило.
fn gather_materials(
    world: &mut World,
    user: Entity,
    ack: u32,
    edge: &Edge,
) -> Option<Vec<(Entity, Option<String>)>> {
    let mut used: Vec<(Entity, Option<String>)> = Vec::new();
    let put = |world: &mut World,
               used: &mut Vec<(Entity, Option<String>)>,
               item: Entity,
               store: &Option<String>| {
        let holder = holder_name(ack, store.as_deref());
        world.ensure_container(user, &holder, ContainerKind::List);
        let inserted = world.insert_into_container(item, user, &holder).is_ok()
            || world
                .force_insert_into_container(item, user, &holder)
                .is_ok();
        if inserted {
            used.push((item, store.clone()));
        }
        inserted
    };
    for step in &edge.steps {
        let handled = match &step.kind {
            StepKind::Material { amount, .. } => {
                let mut handled = false;
                for item in nearby_items(world, user) {
                    if used.iter().any(|(u, _)| *u == item)
                        || !entity_valid(world, &step.kind, item)
                    {
                        continue;
                    }
                    let Some(split) = world.split_stack(item, *amount, user, None) else {
                        continue;
                    };
                    if put(world, &mut used, split, &step.store) {
                        handled = true;
                        break;
                    }
                }
                handled
            }
            StepKind::Tag(_) | StepKind::Tags { .. } | StepKind::Component(_) => {
                let mut to_find = step.amount.max(1);
                for item in nearby_items(world, user) {
                    if to_find == 0 {
                        break;
                    }
                    if used.iter().any(|(u, _)| *u == item)
                        || !entity_valid(world, &step.kind, item)
                    {
                        continue;
                    }
                    if world
                        .container(item, crate::storage::STORAGE_CONTAINER)
                        .is_some()
                    {
                        world.empty_container(item, crate::storage::STORAGE_CONTAINER);
                    }
                    if put(world, &mut used, item, &step.store) {
                        to_find -= 1;
                    }
                }
                to_find == 0
            }
            StepKind::Tool { .. } | StepKind::Unsupported => false,
        };
        if !handled {
            release_materials(world, &used);
            return None;
        }
    }
    Some(used)
}

/// Вернуть собранное на пол у строителя (`FailCleanup`).
fn release_materials(world: &mut World, items: &[(Entity, Option<String>)]) {
    for (item, _) in items {
        let _ = world.remove_from_container(*item, None);
    }
}

/// Начать постройку (`HandleStartStructureConstruction`).
pub fn start_structure_construction(
    world: &mut World,
    user: Entity,
    request: &StartStructureConstruction,
) -> bool {
    let Some(recipe) = recipe(world, &request.prototype) else {
        return false;
    };
    let Some(graph) = graph(world, &recipe.graph) else {
        return false;
    };
    if world.containing_container(user).is_some() {
        let text = loc(world, "construction-system-inside-container", &[]);
        world.popup_entity_for(&text, user, user, PopupType::Small);
        return false;
    }
    let Some(eye_map) = world.map_coordinates(user) else {
        return false;
    };
    let point = astrocore_map::MapCoordinates::on_level(
        eye_map.map,
        astrocore_math::Vec2::new(request.x, request.y),
        eye_map.z,
    );
    let Some(grid) = astrocore_map::MapWorldExt::find_grid_at(world, point) else {
        return false;
    };
    let local = world
        .world_transform(grid)
        .map_or(point.position, |t| t.to_local(point.position));
    let level = world.world_transform(grid).map_or(0, |t| t.level);
    let cell = TileCoord::containing(local, point.z - level);
    let placement = Placement {
        user,
        grid,
        cell,
        angle: Angle(request.angle),
    };
    if !recipe
        .conditions
        .iter()
        .all(|c| check_place_condition(world, placement, c))
    {
        return false;
    }
    let Some(holding) = world.active_item(user) else {
        return false;
    };
    let center = astrocore_map::EntityCoordinates::new(grid, cell.center());
    let Some(target) = world.to_map_coordinates(center) else {
        return false;
    };
    if !crate::interaction::InteractionWorldExt::in_range_unobstructed_point(
        world,
        user,
        target,
        crate::interaction::INTERACTION_RANGE,
    ) && !recipe.can_build_in_impassable
    {
        return false;
    }
    let Some(path) = graph.path(&recipe.start_node, &recipe.target_node) else {
        return false;
    };
    let Some(edge) = path
        .get(1)
        .and_then(|next| {
            graph
                .nodes
                .get(&recipe.start_node)?
                .edges
                .iter()
                .find(|e| &e.to == next)
        })
        .cloned()
    else {
        return false;
    };
    // Предмет в руке должен подходить к какому-то шагу.
    if !edge
        .steps
        .iter()
        .any(|step| entity_valid(world, &step.kind, holding))
    {
        return false;
    }
    if world
        .get::<PendingBuilds>(user)
        .is_some_and(|b| b.0.iter().any(|b| b.ack == request.ack))
    {
        let text = loc(
            world,
            "construction-system-construct-cannot-start-another-construction",
            &[],
        );
        world.popup_entity_for(&text, user, user, PopupType::Small);
        return false;
    }
    let total_time: f64 = edge.steps.iter().map(|s| s.do_after).sum();
    let Some(used) = gather_materials(world, user, request.ack, &edge) else {
        let text = loc(world, "construction-system-construct-no-materials", &[]);
        world.popup_entity_for(&text, user, user, PopupType::Small);
        return false;
    };
    let pending = PendingBuild {
        ack: request.ack,
        recipe: recipe.clone(),
        edge,
        grid,
        cell,
        angle: if recipe.can_rotate {
            request.angle
        } else {
            0.0
        },
        items: used,
    };
    let mut builds = world
        .get::<PendingBuilds>(user)
        .cloned()
        .unwrap_or_default();
    builds.0.push(pending);
    world.entity_mut(user).insert(builds);
    let mut args = DoAfterArgs::new(user, total_time, None, None, Some(user));
    args.break_on_damage = true;
    args.break_on_move = true;
    args.need_hand = false;
    args.cancel_duplicate = false;
    args.block_duplicate = false;
    if matches!(
        world.try_start_do_after(args, BuildDoAfter { ack: request.ack }),
        DoAfterStart::Refused
    ) {
        finish_build(world, user, request.ack, true);
        return false;
    }
    true
}

/// Ожидание закончилось: поставить сущность или вернуть материалы.
fn finish_build(world: &mut World, user: Entity, ack: u32, cancelled: bool) {
    let Some(mut builds) = world.get::<PendingBuilds>(user).cloned() else {
        return;
    };
    let Some(index) = builds.0.iter().position(|b| b.ack == ack) else {
        return;
    };
    let build = builds.0.remove(index);
    world.entity_mut(user).insert(builds);
    if cancelled {
        release_materials(world, &build.items);
        return;
    }
    let prototype = graph(world, &build.recipe.graph)
        .and_then(|g| g.nodes.get(&build.edge.to).and_then(|n| n.entity.clone()));
    let at = astrocore_map::EntityCoordinates::new(build.grid, build.cell.center());
    let Some(entity) = prototype.and_then(|p| world.spawn_attached(&p, at).ok()) else {
        release_materials(world, &build.items);
        return;
    };
    let _ = world.set_coordinates(entity, at, Some(Angle(build.angle)));
    if world.get::<Construction>(entity).is_none() {
        release_materials(world, &build.items);
        world.queue_delete(entity);
        return;
    }
    if let Some(mut construction) = world.get_mut::<Construction>(entity) {
        construction.target_node = Some(build.recipe.target_node.clone());
    }
    for (item, store) in &build.items {
        let _ = world.remove_from_container(*item, None);
        match store {
            Some(store) => {
                if let Some(mut construction) = world.get_mut::<Construction>(entity) {
                    construction.containers.insert(store.clone());
                }
                world.ensure_container(entity, store, ContainerKind::List);
                let _ = world.force_insert_into_container(*item, entity, store);
            }
            None => world.queue_delete(*item),
        }
    }
    for step in &build.edge.steps {
        perform_actions(world, entity, Some(user), &step.completed);
    }
    perform_actions(world, entity, Some(user), &build.edge.completed);
}

// Сверка с данными.

const KNOWN_CONDITIONS: &[&str] = &[
    "EntityAnchored",
    "WirePanel",
    "ContainerNotEmpty",
    "ContainerEmpty",
    "DoorWelded",
    "DoorBolted",
    "Locked",
    "HasTag",
    "AllWiresCut",
    "AllConditions",
    "AnyConditions",
    "StorageWelded",
];
const KNOWN_ACTIONS: &[&str] = &[
    "SpawnPrototype",
    "GivePrototype",
    "DeleteEntity",
    "DestroyEntity",
    "SnapToGrid",
    "SetAnchor",
    "PlaySound",
    "EmptyAllContainers",
    "EmptyContainer",
    "DeleteEntitiesInContainer",
    "SetStackCount",
    "PopupUser",
    "PopupEveryone",
    "VisualizerDataInt",
    "SetWiresPanelSecurity",
    "ConditionalAction",
    "AppearanceChange",
    "AdminLog",
    "AddContainer",
    "DamageEntity",
    "AttemptElectrocute",
];
const KNOWN_PLACE_CONDITIONS: &[&str] = &[
    "TileNotBlocked",
    "EmptyOrWindowValidInTile",
    "NoWindowsInTile",
    "NoUnstackableInTile",
    "TileType",
    "AgainstWall",
    "WallmountCondition",
];

/// Что из данных строительства не разобралось.
#[derive(Debug, Default)]
pub struct Audit {
    pub graphs: usize,
    pub recipes: usize,
    /// Рецепты без пути от начала к цели или с узлом без сущности.
    pub broken_recipes: Vec<String>,
    /// Неизвестные условия, действия и шаги: число вхождений.
    pub unknown: std::collections::BTreeMap<String, usize>,
}

/// Проверить все графы и рецепты на данных.
pub fn audit(world: &mut World) -> Audit {
    let mut report = Audit::default();
    let ids: Vec<String> = world
        .get_resource::<Prototypes>()
        .map(|p| p.ids("constructionGraph").map(str::to_owned).collect())
        .unwrap_or_default();
    let note = |report: &mut Audit, key: String| *report.unknown.entry(key).or_default() += 1;
    fn walk_actions(actions: &[DataNode], out: &mut Vec<String>) {
        for action in actions {
            match action.type_tag() {
                Some(tag) if KNOWN_ACTIONS.contains(&tag) => {}
                Some(tag) => out.push(format!("действие {tag}")),
                None => {}
            }
        }
    }
    fn walk_conditions(conditions: &[DataNode], out: &mut Vec<String>) {
        for condition in conditions {
            match condition.type_tag() {
                Some(tag) if KNOWN_CONDITIONS.contains(&tag) => {}
                Some(tag) => out.push(format!("условие {tag}")),
                None => {}
            }
        }
    }
    for id in &ids {
        let Some(graph) = graph(world, id) else {
            continue;
        };
        report.graphs += 1;
        let mut found = Vec::new();
        for node in graph.nodes.values() {
            walk_actions(&node.actions, &mut found);
            for edge in &node.edges {
                walk_conditions(&edge.conditions, &mut found);
                walk_actions(&edge.completed, &mut found);
                for step in &edge.steps {
                    walk_actions(&step.completed, &mut found);
                    if step.kind == StepKind::Unsupported {
                        found.push("шаг температуры или сборки из частей".into());
                    }
                }
            }
        }
        for key in found {
            note(&mut report, key);
        }
    }
    for recipe in recipes(world) {
        report.recipes += 1;
        for condition in &recipe.conditions {
            if let Some(tag) = condition.type_tag()
                && !KNOWN_PLACE_CONDITIONS.contains(&tag)
            {
                note(&mut report, format!("условие места {tag}"));
            }
        }
        // Первый узел после начала ставится в мир, поэтому у него должна быть сущность.
        let ok = graph(world, &recipe.graph).is_some_and(|g| {
            g.path(&recipe.start_node, &recipe.target_node)
                .is_some_and(|path| {
                    path.get(1)
                        .is_some_and(|n| g.nodes.get(n).is_some_and(|n| n.entity.is_some()))
                })
        });
        if !ok {
            report.broken_recipes.push(recipe.id);
        }
    }
    report
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Construction>();
    if !side.is_server() {
        world.register_net_event::<StartStructureConstruction>();
        return;
    }
    register_tool_event::<ConstructionDoAfter>(world);
    world.register_net_event::<StartStructureConstruction>();
    world.subscribe_net_event::<StartStructureConstruction>(|world, event| {
        let Some(user) = event
            .session
            .and_then(|s| world.get::<astrocore_net::Session>(s))
            .and_then(|s| s.attached)
        else {
            return;
        };
        let request = event.event.clone();
        start_structure_construction(world, user, &request);
    });
    world.subscribe::<PendingBuilds, DoAfterDone<BuildDoAfter>>(|world, user, event| {
        finish_build(world, user, event.event.ack, event.cancelled);
    });
    world.subscribe::<Construction, InteractUsing>(|world, entity, event| {
        if event.handled {
            return;
        }
        let interaction = Interaction {
            user: event.user,
            used: event.used,
            completed: false,
        };
        if handle_event(world, entity, interaction, true) != HandleResult::Validated {
            return;
        }
        event.handled = true;
        handle_event(world, entity, interaction, false);
    });
    world.subscribe::<Construction, DoAfterDone<ConstructionDoAfter>>(|world, entity, event| {
        let Some(used) = event.used else {
            return;
        };
        if event.cancelled {
            // Прерванный шаг сбрасывает выбранное ребро, если оно ещё не начато.
            if let Some(mut construction) = world.get_mut::<Construction>(entity)
                && construction.step_index == 0
            {
                construction.edge_index = None;
            }
            return;
        }
        let interaction = Interaction {
            user: event.user,
            used,
            completed: true,
        };
        handle_event(world, entity, interaction, false);
    });
}

#[cfg(test)]
mod tests;
