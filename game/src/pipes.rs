//! Сети труб: у каждой группы узлов `Pipe` общий воздух объёмом по сумме труб. Образец —
//! `PipeNet`, `PipeNode` (`Air`, `Volume`) и `NodeGroupSystem` SS14 @ 2f4a2aad.
//!
//! Группы пересобираются целиком (см. `node_container`); воздух старых сетей делится по
//! трубам пропорционально объёму и собирается в новые сети. При разделении сети это даёт
//! то же, что `PipeNet.AfterRemake` + `DivideInto`, при удалении трубы — `RemoveNode`.

use std::collections::HashMap;

use astrocore_ecs::{GameWorldExt, Update};
use astrocore_map::Transform;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::schedule::IntoScheduleConfigs;
use bevy_ecs::world::World;

use crate::Side;
use crate::atmos::{GasData, GasMixture, gas};
use crate::node_container::{NodeContainer, NodeGroups, NodeRef, node_info};

/// Группа узлов труб (`NodeGroupID.Pipe`).
pub const PIPE_GROUP: &str = "Pipe";

/// Сеть труб (`PipeNet`).
#[derive(Debug, Clone, PartialEq)]
pub struct PipeNet {
    pub air: GasMixture,
    /// Грид, на котором сеть (`PipeNet.Grid`).
    pub grid: Option<Entity>,
}

/// Сети труб по номеру группы узлов. Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct PipeNets {
    pub nets: HashMap<usize, PipeNet>,
    generation: u64,
    /// Группа каждого узла на прошлой сборке: по ней воздух переходит в новые сети.
    previous: HashMap<NodeRef, usize>,
}

/// Объём узла трубы, литры (`PipeNode.Volume`).
pub fn node_volume(world: &World, entity: Entity, node: &str) -> f32 {
    world
        .get::<NodeContainer>(entity)
        .and_then(|c| c.nodes.get(node))
        .and_then(node_info)
        .and_then(|info| info.pipe)
        .map_or(200.0, |pipe| pipe.volume_ml as f32 / 1000.0)
}

/// Пересобрать сети после пересборки групп.
fn update(world: &mut World) {
    let Some(generation) = world.get_resource::<NodeGroups>().map(|g| g.generation) else {
        return;
    };
    if world
        .get_resource::<PipeNets>()
        .is_some_and(|n| n.generation == generation)
    {
        return;
    }
    let gas_data = crate::atmos::gas_data(world);
    let old = std::mem::take(&mut *world.get_resource_or_insert_with(PipeNets::default));
    let groups: Vec<(usize, Vec<NodeRef>)> = world
        .resource::<NodeGroups>()
        .groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.kind == PIPE_GROUP)
        .map(|(i, g)| (i, g.nodes.clone()))
        .collect();
    let mut nets = HashMap::new();
    let mut previous = HashMap::new();
    for (index, nodes) in groups {
        let volume: f32 = nodes.iter().map(|(e, n)| node_volume(world, *e, n)).sum();
        let mut air = GasMixture::new(volume).with_temperature(gas::T20C);
        for (entity, name) in &nodes {
            previous.insert((*entity, name.clone()), index);
            let Some(old_net) = old
                .previous
                .get(&(*entity, name.clone()))
                .and_then(|g| old.nets.get(g))
            else {
                continue;
            };
            if old_net.air.volume <= 0.0 {
                continue;
            }
            let mut share = old_net.air.clone();
            share.multiply(node_volume(world, *entity, name) / old_net.air.volume);
            merge_keep_volume(&gas_data, &mut air, &share);
        }
        let grid = nodes
            .first()
            .and_then(|(e, _)| world.get::<Transform>(*e))
            .and_then(Transform::grid);
        nets.insert(index, PipeNet { air, grid });
    }
    *world.resource_mut::<PipeNets>() = PipeNets {
        nets,
        generation,
        previous,
    };
}

fn merge_keep_volume(gas_data: &GasData, receiver: &mut GasMixture, giver: &GasMixture) {
    let volume = receiver.volume;
    gas_data.merge(receiver, giver);
    receiver.volume = volume;
}

/// Сеть узла.
pub fn net_of(world: &World, entity: Entity, node: &str) -> Option<usize> {
    let group = world.get_resource::<NodeGroups>()?.group_of(entity, node)?;
    world
        .get_resource::<PipeNets>()?
        .nets
        .contains_key(&group)
        .then_some(group)
}

/// Воздух сети узла (`PipeNode.Air`).
pub fn pipe_air<'w>(world: &'w World, entity: Entity, node: &str) -> Option<&'w GasMixture> {
    let net = net_of(world, entity, node)?;
    world
        .get_resource::<PipeNets>()?
        .nets
        .get(&net)
        .map(|n| &n.air)
}

/// Изменить воздух сети узла.
pub fn with_pipe_air<R>(
    world: &mut World,
    entity: Entity,
    node: &str,
    f: impl FnOnce(&mut GasMixture) -> R,
) -> Option<R> {
    let net = net_of(world, entity, node)?;
    let mut nets = world.get_resource_mut::<PipeNets>()?;
    nets.nets.get_mut(&net).map(|n| f(&mut n.air))
}

/// Воздух двух узлов сразу (насос, клапан): `None`, если узлы в одной сети.
pub fn with_two_pipe_airs<R>(
    world: &mut World,
    entity: Entity,
    first: &str,
    second: &str,
    f: impl FnOnce(&mut GasMixture, &mut GasMixture) -> R,
) -> Option<R> {
    let a = net_of(world, entity, first)?;
    let b = net_of(world, entity, second)?;
    if a == b {
        return None;
    }
    let mut nets = world.get_resource_mut::<PipeNets>()?;
    let mut air_a = nets.nets.get(&a)?.air.clone();
    let mut air_b = nets.nets.get(&b)?.air.clone();
    let result = f(&mut air_a, &mut air_b);
    nets.nets.get_mut(&a)?.air = air_a;
    nets.nets.get_mut(&b)?.air = air_b;
    Some(result)
}

/// Сети на гриде.
pub fn nets_on(world: &World, grid: Entity) -> Vec<usize> {
    let mut nets: Vec<usize> = world
        .get_resource::<PipeNets>()
        .map(|n| {
            n.nets
                .iter()
                .filter(|(_, net)| net.grid == Some(grid))
                .map(|(i, _)| *i)
                .collect()
        })
        .unwrap_or_default();
    nets.sort();
    nets
}

pub fn install(world: &mut World, side: Side) {
    if !side.is_server() {
        return;
    }
    world.init_resource::<PipeNets>();
    world.add_game_systems(Update, update.after(crate::node_container::update_system));
}

#[cfg(test)]
pub(crate) mod tests;
