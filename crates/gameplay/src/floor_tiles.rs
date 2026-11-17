use super::*;
use astrocore_math::GridCell;
use astrocore_world::{SlotKey, StructureElement};
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Deserialize)]
struct Tile {
    name: String,
    subfloor: bool,
    sturdy: bool,
    base: Option<String>,
    whitelist: Vec<String>,
    tools: Vec<String>,
    drop: Option<String>,
}

static TILES: LazyLock<BTreeMap<String, Tile>> = LazyLock::new(|| {
    serde_json::from_value(construction::CATALOG["tiles"].clone()).expect("tile definitions")
});

impl State {
    pub(super) fn pipe_recipe(
        &self,
        object: &AuthoredObject,
    ) -> Option<&'static crate::crafting::PipeRecipe> {
        let name = object
            .metadata
            .get("pipe_proto")
            .or_else(|| match &object.kind {
                AuthoredKind::Placement { prototype } => self.ss14_names.get(prototype),
                _ => None,
            })?;
        crate::crafting::pipes().iter().find(|r| &r.id == name)
    }

    fn attached_ports(&self, object: &AuthoredObject) -> u8 {
        if object.metadata.contains_key("loose_pipe_dirs") {
            return 0;
        }
        if let Some(dirs) = object.metadata.get(crate::piping::PIPE_KEY) {
            return crate::piping::parse_dirs(dirs);
        }
        self.pipe_recipe(object).map_or(0, |r| {
            crate::piping::rotate_dirs(
                r.dirs,
                object
                    .metadata
                    .get("rot")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            )
        })
    }
    pub(super) fn pipe_work(
        &self,
        ctx: &HookContext<'_>,
        cell: (i32, i32),
        tool: &HeldItem,
        layer: Option<u8>,
    ) -> Option<crate::doafter::StructureWork> {
        use crate::doafter::StructureWork;
        let info = self.info(&tool.prototype)?;
        let floor = FloorId(0);
        let base = ctx
            .world
            .element_at(floor, SlotKey::FloorSlab(GridCell::new(cell.0, cell.1)))?;
        if !base
            .material
            .as_str()
            .strip_prefix("ss14:tile_")
            .and_then(|k| TILES.get(k))
            .is_some_and(|t| t.subfloor)
        {
            return None;
        }
        if info.qualities.contains("Cutting") {
            for cable in ctx.world.authored().filter(|o| {
                o.floor == floor
                    && !self.gone.contains(&o.id)
                    && crate::piping::cell_at(o.floor.0, (o.min() + o.max()) * 0.5) == cell
            }) {
                let Some(name) = cable.metadata.get("cable_proto") else {
                    continue;
                };
                let spec = &construction::CATALOG["cableObjects"][name];
                if spec["tool"]
                    .as_str()
                    .is_some_and(|q| info.qualities.contains(q))
                {
                    return Some(StructureWork::CutCable(
                        cable.id,
                        spec["delay"].as_f64()? as f32,
                    ));
                }
            }
        }
        let pipe = ctx.world.authored().find(|o| {
            let at = (o.min() + o.max()) * 0.5;
            o.floor == floor
                && at.x.floor() as i32 == cell.0
                && at.z.floor() as i32 == cell.1
                && self.pipe_recipe(o).is_some()
                && layer.is_none_or(|wanted| crate::piping::pipe_layer(o) == wanted)
                && !self.gone.contains(&o.id)
        })?;
        let loose = pipe.metadata.contains_key("loose_pipe_dirs");
        if info.qualities.contains("Anchoring") {
            let pressure = self
                .pipe_keys
                .existing(pipe.id)
                .and_then(|key| self.atmos.pipes().air_of(crate::piping::node_key(key, 0)))
                .map_or(0., |a| a.pressure());
            let delay = if !loose
                && pressure - self.atmos.air(cell).pressure() > 2. * atmos::ONE_ATMOSPHERE
            {
                3.
            } else {
                1.
            };
            return Some(StructureWork::PipeAnchor(pipe.id, loose, delay));
        }
        if loose {
            let recipe = self.pipe_recipe(pipe)?;
            let index = pipe
                .metadata
                .get("pipe_step")
                .and_then(|n| n.parse().ok())
                .unwrap_or(0usize);
            let step = recipe.reverse["steps"].get(index)?;
            let quality = step["tool"].as_str()?;
            if !info.qualities.contains(quality) {
                return None;
            }
            if quality == "Welding"
                && let Some(spec) = self.welding_tools.get(&tool.prototype)
            {
                let solution = self
                    .solutions
                    .get(&tool.id)
                    .or_else(|| self.solution_templates.get(&tool.prototype))?;
                if !self.lit.contains(&tool.id) || solution.amount(&spec.fuel) < 10. {
                    return None;
                }
            }
            return Some(StructureWork::PipeDismantle(
                pipe.id,
                index,
                step["doAfter"].as_f64().unwrap_or(0.) as f32,
            ));
        }
        None
    }

    pub(super) fn cut_cable(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
        at: Vec3,
    ) {
        let Some(cable) = ctx.world.authored_object(id) else {
            return;
        };
        let Some(group) = cable
            .metadata
            .get(crate::electric::CABLE_KEY)
            .and_then(|s| power::CableGroup::parse(s))
        else {
            return;
        };
        let cell = crate::piping::cell_at(cable.floor.0, at);
        let powered = self.power_cables.iter().any(|&(g, p, net)| {
            g == group && cell == p && self.power.networks[net].last_combined_max_supply > 0.
        });
        if powered {
            let coefficient: f32 = self.actors[&actor]
                .inventory
                .iter()
                .filter(|(slot, _)| slot.flag != "pocket")
                .filter_map(|(_, item)| self.ss14_names.get(&item?.prototype))
                .filter_map(|name| construction::CATALOG["insulation"][name].as_f64())
                .map(|value| value as f32)
                .product();
            let (damage, duration): (f32, f32) = match group {
                power::CableGroup::High => (22., 10.),
                power::CableGroup::Medium => (15., 7.5),
                power::CableGroup::Apc => (7., 5.),
            };
            let damage = (damage * coefficient).trunc();
            if damage > 0. {
                let until = self.clock + duration;
                let person = self.actor(actor);
                person.health.apply(&[("Shock".into(), damage)].into());
                if coefficient > 0.5 {
                    person.stun_until = person.stun_until.max(until);
                }
                self.popup(actor, "Вас ударило током!");
                self.after_health_change(ctx, actor);
                return;
            }
        }
        let Some(name) = cable.metadata.get("cable_proto") else {
            return;
        };
        let Some(drop) = construction::CATALOG["cableObjects"][name]["drop"]
            .as_str()
            .and_then(|name| self.create_item(name))
        else {
            return;
        };
        self.gone.insert(id);
        ctx.request(WorldCommand::RemoveAuthored { id });
        self.place(ctx, &drop, at);
    }

    pub(super) fn toggle_pipe(&mut self, ctx: &mut HookContext<'_>, id: ObjectId, loose: bool) {
        let Some(mut pipe) = ctx.world.authored_object(id).cloned() else {
            return;
        };
        let (from, to) = if loose {
            ("loose_pipe_dirs", crate::piping::PIPE_KEY)
        } else {
            (crate::piping::PIPE_KEY, "loose_pipe_dirs")
        };
        let Some(dirs) = pipe.metadata.remove(from).or_else(|| {
            if loose {
                return None;
            }
            let mask = self.attached_ports(&pipe);
            (mask != 0).then(|| {
                "NSEW"
                    .chars()
                    .filter(|c| crate::piping::parse_dirs(&c.to_string()) & mask != 0)
                    .collect()
            })
        }) else {
            return;
        };
        let cell = crate::piping::cell_at(pipe.floor.0, (pipe.min() + pipe.max()) * 0.5);
        if !loose && let Some(key) = self.pipe_keys.existing(id) {
            let volumes = match &pipe.kind {
                AuthoredKind::Placement { prototype } => self
                    .pipe_parts
                    .get(prototype)
                    .map(|p| p.nodes.iter().map(|n| n.1).collect::<Vec<_>>()),
                _ => None,
            }
            .unwrap_or_else(|| {
                vec![
                    pipe.metadata
                        .get(crate::piping::PIPE_VOLUME_KEY)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(atmos::pipes::DEFAULT_PIPE_VOLUME),
                ]
            });
            for (index, volume) in volumes.into_iter().enumerate() {
                if let Some(air) = self
                    .atmos
                    .pipes()
                    .air_of(crate::piping::node_key(key, index))
                {
                    let gas = air.clone().remove_volume(volume);
                    self.atmos.merge_into(cell, &gas);
                }
            }
        }
        if loose
            && ctx.world.authored().any(|obj| {
                obj.id != id
                    && crate::piping::pipe_layer(obj) == crate::piping::pipe_layer(&pipe)
                    && crate::piping::cell_at(obj.floor.0, (obj.min() + obj.max()) * 0.5) == cell
                    && self.attached_ports(obj) & crate::piping::parse_dirs(&dirs) != 0
            })
        {
            return;
        }
        pipe.metadata.remove("pipe_step");
        pipe.metadata.insert(to.into(), dirs);
        ctx.request(WorldCommand::RemoveAuthored { id });
        ctx.request(WorldCommand::PlaceAuthored(pipe));
    }
    pub(super) fn start_placement(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        data: &[u8],
    ) {
        let Some((coordinates, recipe)) = data.split_at_checked(13) else {
            return;
        };
        let Some((rotation, Some(point))) = crate::wire::parse_drop(coordinates) else {
            return;
        };
        if rotation >= 4 {
            return;
        }
        let Some(used) = self.used_item(actor) else {
            return;
        };
        let point = Vec3::from_array(point);
        if !point.is_finite() {
            return;
        }
        if let Some((_, proto)) =
            crate::crafting::girder_walls().find(|(id, _)| id.as_bytes() == recipe)
        {
            self.begin_barrier(ctx, actor, point, Some(proto));
            return;
        }
        if let Ok(id) = std::str::from_utf8(recipe)
            && crate::crafting::barriers().any(|r| r == id)
        {
            self.begin_barrier(ctx, actor, point, Some(id));
            return;
        }
        if matches!(
            recipe,
            b"Computer" | b"MachineFrame" | b"Airlock" | b"AirlockGlass"
        ) {
            self.start_frame(
                ctx,
                actor,
                point,
                match recipe {
                    b"Computer" => "Computer",
                    b"MachineFrame" => "Machine",
                    _ => "Airlock",
                },
                rotation,
            );
            return;
        }
        if [
            b"Girder".as_slice(),
            b"Wall",
            b"ReinforcedGirder",
            b"ReinforcedWall",
        ]
        .contains(&recipe)
        {
            let frame = self.girder_at(ctx, (point.x.floor() as i32, point.z.floor() as i32));
            let reinforced = recipe.starts_with(b"Reinforced");
            if frame.is_some() && recipe == b"Girder"
                || recipe == b"ReinforcedGirder"
                    && frame.is_some_and(|id| self.is_reinforced_girder(ctx, id))
            {
                return;
            }
            if self
                .material_count(
                    &used.item,
                    if reinforced && frame.is_some() {
                        "Plasteel"
                    } else {
                        "Steel"
                    },
                )
                .is_some()
            {
                self.begin_construction(ctx, actor, used.hand, point);
            }
            return;
        }
        let Some(recipe) = crate::crafting::pipes()
            .iter()
            .position(|r| r.id.as_bytes() == recipe)
        else {
            return;
        };
        let Some(floor) = ctx.position_of(actor).and_then(|p| floor_at(ctx, p.y)) else {
            return;
        };
        let cell = GridCell::new(point.x.floor() as i32, point.z.floor() as i32);
        self.begin(
            ctx,
            actor,
            Action::BuildPipe {
                recipe,
                rotation,
                floor,
                cell,
                stack: used.item.id,
            },
        );
    }

    pub(super) fn can_build_pipe(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        tile: (FloorId, GridCell),
        recipe: usize,
        rotation: u8,
        stack: ObjectId,
    ) -> bool {
        let (floor, cell) = tile;
        let Some(used) = self.used_item(actor) else {
            return false;
        };
        let r = &crate::crafting::pipes()[recipe];
        let mask = crate::piping::rotate_dirs(r.dirs, rotation);
        (!r.device || self.proto_by_name.contains_key(&r.id))
            && self.accessible_tile(ctx, actor, floor, cell)
            && used.item.id == stack
            && self
                .material_count(&used.item, &r.material)
                .is_some_and(|n| n >= r.amount)
            && ctx
                .world
                .element_at(floor, SlotKey::FloorSlab(cell))
                .is_some()
            && !ctx.world.authored().any(|o| {
                let p = (o.min() + o.max()) * 0.5;
                o.floor == floor
                    && p.x.floor() as i32 == cell.x
                    && p.z.floor() as i32 == cell.z
                    && crate::piping::pipe_layer(o) == r.layer
                    && (self.attached_ports(o) & mask != 0
                        || r.device && self.pipe_recipe(o).is_some_and(|p| p.device))
            })
    }

    pub(super) fn build_pipe(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        tile: (FloorId, GridCell),
        recipe: usize,
        rotation: u8,
        stack: ObjectId,
    ) {
        let (floor, cell) = tile;
        if !self.can_build_pipe(ctx, actor, (floor, cell), recipe, rotation, stack) {
            return;
        }
        let r = &crate::crafting::pipes()[recipe];
        let mask = crate::piping::rotate_dirs(r.dirs, rotation);
        let dirs: String = "NSEW"
            .chars()
            .filter(|c| crate::piping::parse_dirs(&c.to_string()) & mask != 0)
            .collect();
        let base = Vec3::new(
            cell.x as f32,
            ctx.world.floor(floor).unwrap().base_y,
            cell.z as f32,
        );
        self.wall_serial += 1;
        let mut pipe = AuthoredObject::new(
            ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial),
            AuthoredKind::Zone,
            floor,
            base + Vec3::new(0.01, 0.0, 0.01),
            base + Vec3::new(0.99, 2.3, 0.99),
        );
        pipe.metadata.insert(crate::piping::PIPE_KEY.into(), dirs);
        pipe.metadata
            .insert(crate::piping::PIPE_VOLUME_KEY.into(), r.volume.to_string());
        pipe.metadata.insert("pipe_proto".into(), r.id.clone());
        if r.device {
            pipe.kind = AuthoredKind::Placement {
                prototype: self.proto_by_name[&r.id].clone(),
            };
            pipe.metadata.insert("rot".into(), rotation.to_string());
        }
        pipe.metadata
            .insert("pipe_layer".into(), r.layer.to_string());
        ctx.request(WorldCommand::PlaceAuthored(pipe));
        let used = self.used_item(actor).unwrap();
        self.consume_stack(
            ctx,
            actor,
            &used,
            self.stack_count(&used.item).unwrap(),
            r.amount,
        );
    }
    pub(super) fn tile_delay(&self, tool: &PrototypeId) -> Option<f32> {
        construction::CATALOG["tileTools"][self.ss14_names.get(tool)?]
            .as_f64()
            .map(|v| v as f32)
    }

    pub(super) fn accessible_tile(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        floor: FloorId,
        cell: GridCell,
    ) -> bool {
        let Some(feet) = ctx.position_of(actor) else {
            return false;
        };
        let chest = feet + Vec3::Y * CHEST_M;
        let center = Vec3::new(cell.x as f32 + 0.5, chest.y, cell.z as f32 + 0.5);
        self.actors
            .get(&actor)
            .is_some_and(|a| !a.ghost && a.health.state().can_act())
            && floor_at(ctx, feet.y) == Some(floor)
            && chest.distance(center) <= self.range_m
            && ctx.is_unobstructed(chest, center)
            && !ctx.world.elements().any(|e| {
                e.floor == floor && matches!(e.placement, Placement::Free(p) if p.cell == cell)
            })
    }

    pub(super) fn use_floor_tile(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        point: Vec3,
    ) -> bool {
        if !point.is_finite() {
            return false;
        }
        let Some(used) = self.used_item(actor).filter(|u| u.hand == hand) else {
            return false;
        };
        let Some(name) = self.ss14_names.get(&used.item.prototype) else {
            return false;
        };
        let outputs = construction::CATALOG["floorItems"][name].as_array();
        let cable = &construction::CATALOG["cables"][name];
        if outputs.is_none()
            && !cable.is_object()
            && self.tile_delay(&used.item.prototype).is_none()
        {
            return false;
        }
        let Some(floor) = ctx.position_of(actor).and_then(|p| floor_at(ctx, p.y)) else {
            return false;
        };
        let cell = GridCell::new(point.x.floor() as i32, point.z.floor() as i32);
        if !self.accessible_tile(ctx, actor, floor, cell) {
            return false;
        }
        let slab = ctx.world.element_at(floor, SlotKey::FloorSlab(cell));
        if let (Some(group), Some(entity)) = (cable["group"].as_str(), cable["entity"].as_str()) {
            let Some(tile) = slab
                .and_then(|e| e.material.as_str().strip_prefix("ss14:tile_"))
                .and_then(|k| TILES.get(k))
            else {
                return false;
            };
            if !tile.sturdy || (!tile.subfloor && cable["overTile"] != true) {
                return false;
            }
            let pcell = atmos::floor_cell(floor.0, cell.x, cell.z);
            if ctx.world.authored().any(|o| {
                o.metadata.get("power_cable").is_some_and(|g| g == group)
                    && crate::piping::cell_at(o.floor.0, (o.min() + o.max()) * 0.5) == pcell
            }) {
                return true;
            }
            let Some(count) = self.stack_count(&used.item).filter(|n| *n > 0) else {
                return false;
            };
            let at = Vec3::new(
                cell.x as f32,
                ctx.world.floor(floor).unwrap().base_y,
                cell.z as f32,
            );
            self.wall_serial += 1;
            let mut wire = AuthoredObject::new(
                ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial),
                AuthoredKind::Zone,
                floor,
                at + Vec3::new(0.01, 0.0, 0.01),
                at + Vec3::new(0.99, 2.3, 0.99),
            );
            wire.metadata.insert("power_cable".into(), group.into());
            wire.metadata.insert("cable_proto".into(), entity.into());
            ctx.request(WorldCommand::PlaceAuthored(wire));
            self.consume_stack(ctx, actor, &used, count, 1);
            return true;
        }
        if let Some(outputs) = outputs {
            let current = match slab {
                None => "Space",
                Some(e) => {
                    let Some(tile) = e
                        .material
                        .as_str()
                        .strip_prefix("ss14:tile_")
                        .and_then(|key| TILES.get(key))
                    else {
                        return false;
                    };
                    &tile.name
                }
            };
            let next = outputs
                .iter()
                .filter_map(|s| TILES.get(&s.as_str()?.to_lowercase()))
                .find(|t| {
                    t.base.as_deref() == Some(current) || t.whitelist.iter().any(|s| s == current)
                });
            let (Some(next), Some(count)) = (next, self.stack_count(&used.item)) else {
                return false;
            };
            if count == 0 {
                return false;
            }
            self.replace_tile(ctx, floor, cell, Some(&next.name));
            self.consume_stack(ctx, actor, &used, count, 1);
            return true;
        }
        let Some(slab) = slab else { return false };
        let action = Action::RemoveTile {
            floor,
            cell,
            slab: slab.id,
            hand,
            tool: used.item.id,
        };
        if !self.can_start(ctx, actor, &action) {
            return false;
        }
        self.begin(ctx, actor, action);
        true
    }

    pub(super) fn can_remove_tile(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        tile: (FloorId, GridCell),
        slab: ObjectId,
        hand: usize,
        tool: ObjectId,
    ) -> bool {
        let (floor, cell) = tile;
        let Some(used) = self.used_item(actor) else {
            return false;
        };
        if used.hand != hand
            || used.item.id != tool
            || self.tile_delay(&used.item.prototype).is_none()
            || !self.accessible_tile(ctx, actor, floor, cell)
        {
            return false;
        }
        let Some(element) = ctx
            .world
            .element_at(floor, SlotKey::FloorSlab(cell))
            .filter(|e| e.id == slab)
        else {
            return false;
        };
        let Some(tile) = element
            .material
            .as_str()
            .strip_prefix("ss14:tile_")
            .and_then(|key| TILES.get(key))
        else {
            return false;
        };
        tile.base.is_some()
            && self
                .info(&used.item.prototype)
                .is_some_and(|info| tile.tools.iter().any(|q| info.qualities.contains(q)))
    }

    pub(super) fn remove_tile(
        &mut self,
        ctx: &mut HookContext<'_>,
        floor: FloorId,
        cell: GridCell,
        slab: ObjectId,
    ) {
        let Some(element) = ctx
            .world
            .element_at(floor, SlotKey::FloorSlab(cell))
            .filter(|e| e.id == slab)
        else {
            return;
        };
        let Some(tile) = element
            .material
            .as_str()
            .strip_prefix("ss14:tile_")
            .and_then(|k| TILES.get(k))
        else {
            return;
        };
        self.replace_tile(ctx, floor, cell, tile.base.as_deref());
        if let Some(item) = tile.drop.as_deref().and_then(|name| self.create_item(name)) {
            let at = Vec3::new(
                cell.x as f32 + 0.5,
                ctx.world.floor(floor).unwrap().base_y + 0.2,
                cell.z as f32 + 0.5,
            );
            self.charges.insert(item.id, 1);
            self.place(ctx, &item, at);
        }
    }

    pub(super) fn replace_tile(
        &mut self,
        ctx: &mut HookContext<'_>,
        floor: FloorId,
        cell: GridCell,
        next: Option<&str>,
    ) {
        if let Some(e) = ctx.world.element_at(floor, SlotKey::FloorSlab(cell)) {
            ctx.request(WorldCommand::RemoveElement { id: e.id });
        }
        if let Some(name) = next.filter(|name| *name != "Space") {
            self.wall_serial += 1;
            ctx.request(WorldCommand::PlaceElement(StructureElement {
                id: ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial),
                kind: StructureKind::Floor,
                placement: Placement::Cell(cell),
                floor,
                material: PrototypeId::new(format!("ss14:tile_{}", name.to_lowercase())).unwrap(),
                thickness: 0.2,
            }));
        }
    }
}
