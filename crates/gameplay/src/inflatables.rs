use super::*;
use astrocore_math::GridCell;
use astrocore_world::{SlotKey, StructureElement};

impl State {
    pub(crate) fn inflatable_sound(&mut self, proto: &str, event: &str, at: Vec3) {
        self.engineering_sound(&construction::CATALOG["inflatables"][proto][event], at);
    }
    pub(super) fn is_balloon_popper(&self, prototype: &PrototypeId) -> bool {
        self.ss14_names.get(prototype).is_some_and(|name| {
            construction::CATALOG["balloonPoppers"]
                .as_array()
                .is_some_and(|names| names.iter().any(|n| n == name))
        })
    }

    pub(super) fn use_inflatable_at(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        point: Vec3,
    ) -> bool {
        let Some(used) = self.used_item(actor).filter(|u| u.hand == hand) else {
            return false;
        };
        let Some(name) = self.ss14_names.get(&used.item.prototype) else {
            return false;
        };
        let spec = &construction::CATALOG["inflatableItems"][name];
        let Some(proto) = spec["result"].as_str() else {
            return false;
        };
        let Some(floor) = ctx.position_of(actor).and_then(|p| floor_at(ctx, p.y)) else {
            return true;
        };
        let cell = GridCell::new(point.x.floor() as i32, point.z.floor() as i32);
        if self.can_inflate(ctx, actor, floor, cell, used.item.id, proto) {
            self.begin(
                ctx,
                actor,
                Action::Inflate {
                    floor,
                    cell,
                    stack: used.item.id,
                    proto: proto.into(),
                    delay: spec["duration"].as_f64().unwrap() as f32,
                },
            );
        }
        true
    }

    pub(super) fn can_inflate(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        floor: FloorId,
        cell: GridCell,
        stack: ObjectId,
        proto: &str,
    ) -> bool {
        floor == FloorId(0) && self.used_item(actor).is_some_and(|u| u.item.id == stack && self.material_count(&u.item, proto).is_some_and(|n| n > 0))
            && self.accessible_tile(ctx, actor, floor, cell)
            && ctx.world.element_at(floor, SlotKey::FloorSlab(cell)).is_some()
            && !self.inflated_this_tick.contains(&(floor, cell))
            && !self.walls.as_ref().is_some_and(|walls| walls.contains_key(&(cell.x, cell.z)))
            && !ctx.world.authored().any(|o| {
                let at = (o.min() + o.max()) * 0.5;
                o.floor == floor && at.x.floor() as i32 == cell.x && at.z.floor() as i32 == cell.z
                    && (o.metadata.contains_key("inflatable_element") || o.metadata.contains_key("door_element") ||
                        matches!(&o.kind, AuthoredKind::Placement { prototype } if self.info(prototype).is_none()))
            })
            && (proto != "InflatableDoor" || ctx.world.edge_element(floor, computer_construction::door_edge(cell, 0)).is_none())
    }

    pub(super) fn finish_inflate(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        floor: FloorId,
        cell: GridCell,
        stack: ObjectId,
        proto: &str,
    ) {
        if !self.can_inflate(ctx, actor, floor, cell, stack, proto) {
            return;
        }
        self.inflated_this_tick.insert((floor, cell));
        let used = self.used_item(actor).unwrap();
        self.consume_stack(
            ctx,
            actor,
            &used,
            self.material_count(&used.item, proto).unwrap(),
            1,
        );
        let material = prototypes::engine_id("ss14", proto).unwrap();
        if proto == "InflatableWall" {
            self.build_barrier(ctx, floor, (cell.x, cell.z), proto, material);
            return;
        }
        self.wall_serial += 1;
        let door = ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial);
        ctx.request(WorldCommand::PlaceElement(StructureElement {
            id: door,
            floor,
            kind: StructureKind::Opening(OpeningKind::Door),
            placement: Placement::Edge(computer_construction::door_edge(cell, 0)),
            material,
            thickness: 0.1,
        }));
        self.wall_serial += 1;
        let base = cell.min_corner(1., ctx.world.floor(floor).unwrap().base_y);
        let mut marker = AuthoredObject::new(
            ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial),
            AuthoredKind::Zone,
            floor,
            base,
            base + Vec3::ONE,
        );
        marker.metadata.insert(
            "inflatable_element".into(),
            format!("{:x}", door.as_uuid().as_u128()),
        );
        ctx.request(WorldCommand::PlaceAuthored(marker));
    }

    pub(crate) fn inflatable_target(
        &self,
        ctx: &HookContext<'_>,
        id: ObjectId,
    ) -> Option<(&'static str, FloorId, GridCell, Vec3)> {
        let e = ctx.world.element(id)?;
        let y = ctx.world.floor(e.floor)?.base_y + CHEST_M;
        match (e.material.as_str(), e.placement) {
            ("ss14:inflatable_wall", Placement::Free(p)) => Some((
                "InflatableWall",
                e.floor,
                p.cell,
                Vec3::new(p.cell.x as f32 + 0.5, y, p.cell.z as f32 + 0.5),
            )),
            ("ss14:inflatable_door", Placement::Edge(edge)) => {
                let (a, b) = edge.endpoints(ctx.world.config().grid_step_m, y);
                let at = (a + b) * 0.5;
                Some((
                    "InflatableDoor",
                    e.floor,
                    GridCell::new(at.x.floor() as i32, at.z.floor() as i32),
                    at,
                ))
            }
            _ => None,
        }
    }

    pub(super) fn can_deflate(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) -> bool {
        let Some((proto, floor, cell, at)) = self.inflatable_target(ctx, id) else {
            return false;
        };
        let Some(feet) = ctx.position_of(actor) else {
            return false;
        };
        if self.gone.contains(&id)
            || floor_at(ctx, feet.y) != Some(floor)
            || !self
                .actors
                .get(&actor)
                .is_some_and(|a| !a.ghost && a.health.state().can_act())
            || (proto == "InflatableWall"
                && self.walls.as_ref().is_some_and(|w| {
                    w.get(&(cell.x, cell.z))
                        .and_then(|w| w.prototype.as_deref())
                        != Some(proto)
                }))
        {
            return false;
        }
        let chest = feet + Vec3::Y * CHEST_M;
        let delta = chest - at;
        let margin = if proto == "InflatableWall" {
            0.62
        } else {
            0.16
        };
        let face = at + delta * (margin / delta.x.abs().max(delta.z.abs()).max(margin));
        crate::module::reach_distance(chest - Vec3::Y * crate::module::CHEST_M, face)
            <= self.range_m
            && ctx.is_unobstructed(chest, face)
    }

    pub(super) fn begin_deflate(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) -> bool {
        if !self.can_deflate(ctx, actor, id) {
            return false;
        }
        let (proto, ..) = self.inflatable_target(ctx, id).unwrap();
        self.begin(
            ctx,
            actor,
            Action::Deflate {
                target: id,
                delay: construction::CATALOG["inflatables"][proto]["duration"]
                    .as_f64()
                    .unwrap() as f32,
            },
        );
        true
    }

    pub(super) fn finish_deflate(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) {
        if !self.can_deflate(ctx, actor, id) {
            return;
        }
        let (proto, floor, cell, at) = self.inflatable_target(ctx, id).unwrap();
        self.gone.insert(id);
        if proto == "InflatableDoor" {
            let key = format!("{:x}", id.as_uuid().as_u128());
            if let Some(marker) = ctx
                .world
                .authored()
                .find(|o| o.metadata.get("inflatable_element") == Some(&key))
                .map(|o| o.id)
            {
                ctx.request(WorldCommand::RemoveAuthored { id: marker });
            }
            ctx.request(WorldCommand::RemoveElement { id });
        } else {
            let mut walls = self.take_walls(ctx);
            let key = (cell.x, cell.z);
            walls.remove(&key);
            self.open_wall_geometry(ctx, floor, key, &walls);
            self.walls = Some(walls);
        }
        let single = construction::CATALOG["inflatables"][proto]["returns"]
            .as_str()
            .unwrap();
        if let Some(item) = self.create_item(single)
            && self.actor(actor).hands.pickup_any(item.clone()).is_err()
        {
            self.place(ctx, &item, at);
        }
        self.publish(ctx, actor);
    }
}
