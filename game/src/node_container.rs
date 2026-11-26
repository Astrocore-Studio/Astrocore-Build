//! Узлы и группы узлов: кабели, устройства на кабелях, клеммы — всё, что соединяется в сети
//! по соседству привязанных к клеткам сущностей. Образец — `NodeContainerComponent`,
//! `NodeGroupSystem`, `NodeHelpers`, `CableNode`, `CableDeviceNode`, `CableTerminalNode`,
//! `CableTerminalPortNode`, `AdjacentNode` SS14 @ 2f4a2aad.
//!
//! Группа — связное множество узлов с одним `nodeGroupID`. Связи узлов несимметричны: узел A
//! может достать B, а B не достаёт A; группа строится по неориентированному графу, как в SS14.
//! Группы пересобираются целиком в такте, когда что-то изменилось: появился или исчез узел,
//! сущность привязали или отвязали от клетки. Узлы неизвестных видов в группы не входят.

use std::collections::{BTreeMap, HashMap};

use astrocore_data::DataNode;
use astrocore_ecs::{
    ComponentInit, EntityTerminating, EventWorldExt, GameWorldExt, RegistryWorldExt, Update,
};
use astrocore_map::{AnchorStateChanged, MapGrid, TileCoord, Transform};
use astrocore_math::Direction;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;

/// Узлы сущности (`NodeContainerComponent`): имя узла → запись узла с тегом вида
/// (`!type:CableNode`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeContainer {
    pub nodes: BTreeMap<String, DataNode>,
    pub examinable: bool,
}

/// Вид узла: правило, кого он достаёт.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Кабель: кабели соседних по сторонам клеток и устройства своей клетки.
    Cable,
    /// Устройство на кабеле: кабели своей клетки.
    CableDevice,
    /// Клемма СМЭС или подстанции: как устройство, плюс порт в клетке по направлению.
    CableTerminal,
    /// Порт клеммы: клеммы соседних клеток, смотрящие на него.
    CableTerminalPort,
    /// Любые узлы соседних по сторонам клеток (`AdjacentNode`).
    Adjacent,
}

impl NodeKind {
    fn parse(tag: &str) -> Option<Self> {
        Some(match tag {
            "CableNode" => Self::Cable,
            "CableDeviceNode" => Self::CableDevice,
            "CableTerminalNode" => Self::CableTerminal,
            "CableTerminalPortNode" => Self::CableTerminalPort,
            "AdjacentNode" => Self::Adjacent,
            _ => return None,
        })
    }
}

/// Разобранный узел.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    pub kind: NodeKind,
    pub group: String,
    pub enabled: bool,
}

/// Узел по записи данных: `None` — вид узла не перенесён.
pub fn node_info(node: &DataNode) -> Option<NodeInfo> {
    let kind = NodeKind::parse(node.type_tag()?)?;
    let group = node.get("nodeGroupID")?.as_str()?.to_owned();
    let enabled = node
        .get("enabled")
        .and_then(DataNode::as_str)
        .is_none_or(|v| !v.eq_ignore_ascii_case("false"));
    Some(NodeInfo {
        kind,
        group,
        enabled,
    })
}

/// Узел сущности: сущность и имя узла.
pub type NodeRef = (Entity, String);

/// Группа узлов (`BaseNodeGroup`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeGroup {
    /// `nodeGroupID` узлов группы: `HVPower`, `MVPower`, `Apc`…
    pub kind: String,
    pub nodes: Vec<NodeRef>,
}

/// Группы узлов мира. Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct NodeGroups {
    pub groups: Vec<NodeGroup>,
    by_node: HashMap<NodeRef, usize>,
    /// Группы надо пересобрать.
    pub dirty: bool,
    /// Растёт при каждой пересборке: потребители групп (электросеть) узнают о переменах.
    pub generation: u64,
}

impl NodeGroups {
    /// Номер группы узла.
    pub fn group_of(&self, entity: Entity, node: &str) -> Option<usize> {
        self.by_node.get(&(entity, node.to_owned())).copied()
    }
}

/// Узел в клетке: сущность, имя узла, разбор.
type CellNode = (Entity, String, NodeInfo);

/// Клетка привязанной к гриду сущности.
fn anchored_cell(world: &World, entity: Entity) -> Option<(Entity, TileCoord)> {
    let transform = world.get::<Transform>(entity)?;
    if !transform.anchored() {
        return None;
    }
    let grid = transform.grid()?;
    let local = transform.local_position();
    Some((grid, TileCoord::containing(local, transform.level())))
}

/// Узлы сущностей, привязанных к клетке.
fn nodes_in(world: &World, grid: Entity, cell: TileCoord) -> Vec<(Entity, String, NodeInfo)> {
    let Some(map_grid) = world.get::<MapGrid>(grid) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for &entity in map_grid.anchored_at(cell) {
        let Some(container) = world.get::<NodeContainer>(entity) else {
            continue;
        };
        for (name, node) in &container.nodes {
            if let Some(info) = node_info(node) {
                found.push((entity, name.clone(), info));
            }
        }
    }
    found
}

/// Сторона, в которую повёрнута сущность (`GetCardinalDir`).
fn facing(world: &World, entity: Entity) -> Direction {
    let rotation = world
        .get::<Transform>(entity)
        .map_or(0.0, |t| t.local_rotation().0);
    astrocore_math::Angle(rotation).cardinal_direction()
}

const CARDINALS: [Direction; 4] = [
    Direction::North,
    Direction::South,
    Direction::East,
    Direction::West,
];

/// Узлы, которых достаёт узел (`GetReachableNodes`).
fn reachable(world: &World, entity: Entity, info: &NodeInfo) -> Vec<NodeRef> {
    let Some((grid, cell)) = anchored_cell(world, entity) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    match info.kind {
        NodeKind::Cable => {
            // Клеммы на своей клетке и смотрящие на неё из соседних запрещают направления.
            let mut blocked = [false; 8];
            let mut candidates: Vec<(Option<Direction>, NodeRef)> = Vec::new();
            let same = nodes_in(world, grid, cell);
            let around: Vec<(Option<Direction>, Vec<CellNode>)> = std::iter::once((None, same))
                .chain(CARDINALS.iter().map(|dir| {
                    (
                        Some(*dir),
                        nodes_in(world, grid, cell.offset(dir.int_vec())),
                    )
                }))
                .collect();
            for (dir, nodes) in around {
                for (other, name, other_info) in nodes {
                    match other_info.kind {
                        NodeKind::Cable if !(other == entity && dir.is_none()) => {
                            if dir.is_some() {
                                candidates.push((dir, (other, name)));
                            }
                        }
                        NodeKind::CableDevice | NodeKind::CableTerminal if dir.is_none() => {
                            candidates.push((None, (other, name.clone())));
                            if other_info.kind == NodeKind::CableTerminal {
                                blocked[facing(world, other) as usize] = true;
                            }
                        }
                        NodeKind::CableTerminal => {
                            let dir = dir.expect("соседняя клетка");
                            if facing(world, other).opposite() == dir {
                                blocked[dir as usize] = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            for (dir, node) in candidates {
                if dir.is_some_and(|d| blocked[d as usize]) {
                    continue;
                }
                found.push(node);
            }
        }
        NodeKind::CableDevice | NodeKind::CableTerminal => {
            if info.kind == NodeKind::CableTerminal {
                let target = cell.offset(facing(world, entity).int_vec());
                for (other, name, other_info) in nodes_in(world, grid, target) {
                    if other_info.kind == NodeKind::CableTerminalPort {
                        found.push((other, name));
                    }
                }
            }
            for (other, name, other_info) in nodes_in(world, grid, cell) {
                if other_info.kind == NodeKind::Cable {
                    found.push((other, name));
                }
            }
        }
        NodeKind::CableTerminalPort => {
            for dir in CARDINALS {
                for (other, name, other_info) in nodes_in(world, grid, cell.offset(dir.int_vec())) {
                    if other_info.kind == NodeKind::CableTerminal
                        && facing(world, other).opposite() == dir
                    {
                        found.push((other, name));
                    }
                }
            }
        }
        NodeKind::Adjacent => {
            for dir in CARDINALS {
                for (other, name, _) in nodes_in(world, grid, cell.offset(dir.int_vec())) {
                    found.push((other, name));
                }
            }
        }
    }
    found
}

/// Пересобирает все группы (`NodeGroupSystem.DoGroupUpdates`).
pub fn rebuild(world: &mut World) {
    let mut nodes: Vec<(Entity, String, NodeInfo)> = world
        .query::<(Entity, &NodeContainer)>()
        .iter(world)
        .filter(|(entity, _)| {
            // Удаляемые сущности в группы уже не входят.
            world
                .get::<astrocore_ecs::MetaData>(*entity)
                .is_none_or(|meta| meta.stage < astrocore_ecs::LifeStage::Terminating)
        })
        .flat_map(|(entity, container)| {
            container
                .nodes
                .iter()
                .filter_map(move |(name, node)| Some((entity, name.clone(), node_info(node)?)))
        })
        .filter(|(_, _, info)| info.enabled)
        .collect();
    nodes.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let index: HashMap<NodeRef, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, (entity, name, _))| ((*entity, name.clone()), i))
        .collect();
    // Неориентированный граф совместимых узлов.
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (i, (entity, _, info)) in nodes.iter().enumerate() {
        for reach in reachable(world, *entity, info) {
            let Some(&j) = index.get(&reach) else {
                continue;
            };
            if j == i || nodes[j].2.group != info.group {
                continue;
            }
            edges[i].push(j);
            edges[j].push(i);
        }
    }
    let mut group_of = vec![usize::MAX; nodes.len()];
    let mut groups = Vec::new();
    for root in 0..nodes.len() {
        if group_of[root] != usize::MAX {
            continue;
        }
        let id = groups.len();
        let mut members = Vec::new();
        let mut stack = vec![root];
        group_of[root] = id;
        while let Some(node) = stack.pop() {
            members.push((nodes[node].0, nodes[node].1.clone()));
            for &next in &edges[node] {
                if group_of[next] == usize::MAX {
                    group_of[next] = id;
                    stack.push(next);
                }
            }
        }
        members.sort();
        groups.push(NodeGroup {
            kind: nodes[root].2.group.clone(),
            nodes: members,
        });
    }
    let mut state = world.get_resource_or_insert_with(NodeGroups::default);
    state.by_node = index
        .into_iter()
        .map(|(node, i)| (node, group_of[i]))
        .collect();
    state.groups = groups;
    state.dirty = false;
    state.generation += 1;
}

/// Отмечает, что группы надо пересобрать.
pub fn mark_dirty(world: &mut World) {
    world.get_resource_or_insert_with(NodeGroups::default).dirty = true;
}

/// Система: пересобирает группы, если что-то изменилось.
pub fn update_system(world: &mut World) {
    if world.get_resource::<NodeGroups>().is_some_and(|g| g.dirty) {
        rebuild(world);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<NodeContainer>();
    if !side.is_server() {
        return;
    }
    world.init_resource::<NodeGroups>();
    world.subscribe::<NodeContainer, ComponentInit>(|world, _, _| mark_dirty(world));
    world.subscribe::<NodeContainer, AnchorStateChanged>(|world, _, _| mark_dirty(world));
    world.subscribe::<NodeContainer, EntityTerminating>(|world, _, _| mark_dirty(world));
    world.add_game_systems(Update, update_system);
}

#[cfg(test)]
mod tests;
