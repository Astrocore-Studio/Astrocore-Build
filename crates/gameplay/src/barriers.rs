use super::*;
use crate::doafter::BarrierJob;
use astrocore_math::GridCell;

impl State {
    pub(super) fn restore_window_grille(
        &mut self,
        ctx: &mut HookContext<'_>,
        floor: FloorId,
        key: (i32, i32),
        walls: &mut BTreeMap<(i32, i32), crate::explosion::Wall>,
    ) -> bool {
        if !self.window_grilles.remove(&key) {
            return false;
        }
        let Some(proto) = walls.get(&key).and_then(|w| w.prototype.as_deref()) else {
            return false;
        };
        self.replace_barrier(ctx, floor, key, proto, "Grille");
        walls.insert(key, construction::barrier_health("Grille"));
        self.barrier_progress.remove(&key);
        true
    }
    pub(super) fn barrier_job(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        cell: GridCell,
        recipe: Option<&str>,
    ) -> Option<BarrierJob> {
        let used = self.used_item(actor)?;
        let feet = ctx.position_of(actor)?;
        let floor = floor_at(ctx, feet.y)?;
        let who = self.actors.get(&actor)?;
        if floor != FloorId(0) || who.ghost || !who.health.state().can_act() {
            return None;
        }
        let key = (cell.x, cell.z);
        let wall = self.walls.as_ref().and_then(|w| w.get(&key));
        let over_grille = wall.and_then(|w| w.prototype.as_deref()) == Some("Grille")
            && recipe.is_some_and(|p| p.contains("Window"));
        let pending = self.barrier_progress.get(&key);
        let recipe = recipe.or_else(|| {
            pending
                .filter(|(_, dismantle, _)| !*dismantle)
                .map(|(proto, _, _)| proto.as_str())
        });
        let recipe = recipe.or_else(|| {
            if wall.is_some() || self.girder_at(ctx, key).is_none() {
                return None;
            }
            construction::CATALOG["barriers"]
                .as_object()?
                .keys()
                .find_map(|proto| {
                    crate::crafting::girder_base(proto)?;
                    let step = &crate::crafting::barrier_edge(proto, false)?["steps"][0];
                    self.material_count(&used.item, step["material"].as_str()?)
                        .map(|_| proto.as_str())
                })
        });
        let (proto, dismantle) = match recipe {
            Some(proto) if wall.is_none() || over_grille => (proto, false),
            None => (wall?.prototype.as_deref()?, true),
            _ => return None,
        };
        let edge = crate::crafting::barrier_edge(proto, dismantle)?;
        let girder = if let Some(base) = crate::crafting::girder_base(proto)
            && !dismantle
        {
            let id = self.girder_at(ctx, key)?;
            if self.loose_girders.contains(&id)
                || self.is_reinforced_girder(ctx, id) != (base == "reinforcedGirder")
            {
                return None;
            }
            Some(id)
        } else {
            None
        };
        let index = match self.barrier_progress.get(&key) {
            None => 0,
            Some((p, d, i)) if p == proto && *d == dismantle => *i,
            _ => return None,
        };
        if dismantle || over_grille {
            let chest = feet + Vec3::Y * CHEST_M;
            let center = Vec3::new(cell.x as f32 + 0.5, chest.y, cell.z as f32 + 0.5);
            let delta = chest - center;
            let face = center + delta * (0.61 / delta.x.abs().max(delta.z.abs()).max(0.61));
            if chest.distance(face) > self.range_m || !ctx.is_unobstructed(chest, face) {
                return None;
            }
        } else {
            let in_cell = |p: Vec3| p.x.floor() as i32 == cell.x && p.z.floor() as i32 == cell.z;
            let occupied = ctx
                .players()
                .filter_map(|p| ctx.position_of(p))
                .any(|p| floor_at(ctx, p.y) == Some(floor) && in_cell(p));
            let fixture = ctx.world.authored().any(|o| Some(o.id) != girder && o.floor == floor && in_cell((o.min()+o.max())*0.5)
                && matches!(&o.kind, AuthoredKind::Placement { prototype } if self.info(prototype).is_none()));
            if occupied || fixture || !self.accessible_tile(ctx, actor, floor, cell) {
                return None;
            }
        }
        let step = edge["steps"].get(index)?;
        let mut delay = step["doAfter"].as_f64().unwrap_or(0.) as f32;
        if let Some(material) = step["material"].as_str() {
            if self.material_count(&used.item, material)?
                < step["amount"].as_u64().unwrap_or(1) as u32
            {
                return None;
            }
        } else {
            if !self
                .info(&used.item.prototype)?
                .qualities
                .contains(step["tool"].as_str()?)
            {
                return None;
            }
            if let Some(w) = self.welding_tools.get(&used.item.prototype) {
                if !self.lit.contains(&used.item.id)
                    || self
                        .solutions
                        .get(&used.item.id)
                        .or_else(|| self.solution_templates.get(&used.item.prototype))?
                        .amount(&w.fuel)
                        < 10.
                {
                    return None;
                }
                delay /= w.speed;
            }
        }
        Some(BarrierJob {
            girder,
            floor,
            cell,
            proto: proto.into(),
            dismantle,
            step: index,
            item: used.item.id,
            delay,
        })
    }

    pub(super) fn begin_barrier(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        at: Vec3,
        recipe: Option<&str>,
    ) -> bool {
        let walls = self.take_walls(ctx);
        self.walls = Some(walls);
        let cell = GridCell::new(at.x.floor() as i32, at.z.floor() as i32);
        let Some(job) = self.barrier_job(ctx, actor, cell, recipe) else {
            return false;
        };
        self.begin(ctx, actor, Action::Barrier(job));
        true
    }

    pub(super) fn can_barrier(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        job: &BarrierJob,
    ) -> bool {
        self.barrier_job(
            ctx,
            actor,
            job.cell,
            (!job.dismantle).then_some(job.proto.as_str()),
        )
        .as_ref()
            == Some(job)
    }

    pub(super) fn finish_barrier(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        job: BarrierJob,
    ) {
        if !self.can_barrier(ctx, actor, &job) {
            return;
        }
        let used = self.used_item(actor).unwrap();
        let key = (job.cell.x, job.cell.z);
        let edge = crate::crafting::barrier_edge(&job.proto, job.dismantle).unwrap();
        let steps = edge["steps"].as_array().unwrap();
        let step = &steps[job.step];
        let feet = ctx.position_of(actor).unwrap();
        if let Some(material) = step["material"].as_str() {
            self.consume_stack(
                ctx,
                actor,
                &used,
                self.material_count(&used.item, material).unwrap(),
                step["amount"].as_u64().unwrap_or(1) as u32,
            );
        } else {
            self.tool_sound(&used.item.prototype, "use", feet);
            if let Some(w) = self.welding_tools.get(&used.item.prototype).copied() {
                self.solution_mut(&used.item).unwrap().remove(&w.fuel, 10.);
            }
        }
        if job.step + 1 < steps.len() {
            self.barrier_progress
                .insert(key, (job.proto, job.dismantle, job.step + 1));
            return;
        }
        self.barrier_progress.remove(&key);
        if !job.dismantle {
            if let Some(id) = job.girder {
                self.girders.remove(&key);
                self.loose_girders.remove(&id);
                self.gone.insert(id);
                ctx.request(WorldCommand::RemoveAuthored { id });
            }
            let material = prototypes::engine_id("ss14", &job.proto).unwrap();
            if self
                .walls
                .as_ref()
                .and_then(|w| w.get(&key))
                .and_then(|w| w.prototype.as_deref())
                == Some("Grille")
            {
                self.replace_barrier(ctx, job.floor, key, "Grille", &job.proto);
                self.window_grilles.insert(key);
                self.walls
                    .as_mut()
                    .unwrap()
                    .insert(key, construction::barrier_health(&job.proto));
                return;
            }
            self.build_barrier(ctx, job.floor, key, &job.proto, material);
            return;
        }
        let mut walls = self.take_walls(ctx);
        if !self.restore_window_grille(ctx, job.floor, key, &mut walls) {
            walls.remove(&key);
            self.open_wall_geometry(ctx, job.floor, key, &walls);
        }
        self.walls = Some(walls);
        if let Some(base) = crate::crafting::girder_base(&job.proto) {
            let proto = if base == "girder" {
                "Girder"
            } else {
                "ReinforcedGirder"
            };
            if let Some(item) = self.create_item(proto) {
                self.place(
                    ctx,
                    &item,
                    Vec3::new(key.0 as f32 + 0.5, feet.y, key.1 as f32 + 0.5),
                );
                self.girders.insert(key, item.id);
            }
        }
        for effect in edge["completed"].as_array().into_iter().flatten() {
            if let Some(item) = effect["prototype"]
                .as_str()
                .and_then(|name| self.create_item(name))
            {
                self.charges
                    .insert(item.id, effect["amount"].as_u64().unwrap_or(1) as u32);
                let give = effect["__tag"] == "GivePrototype";
                if !give || self.actor(actor).hands.pickup_any(item.clone()).is_err() {
                    self.place(
                        ctx,
                        &item,
                        if give {
                            feet
                        } else {
                            Vec3::new(key.0 as f32 + 0.5, feet.y, key.1 as f32 + 0.5)
                        },
                    );
                }
            }
        }
        self.publish(ctx, actor);
    }

    pub(super) fn replace_barrier(
        &mut self,
        ctx: &mut HookContext<'_>,
        floor: FloorId,
        key: (i32, i32),
        from: &str,
        to: &str,
    ) {
        let old = prototypes::engine_id("ss14", from).unwrap();
        let material = prototypes::engine_id("ss14", to).unwrap();
        let edges = construction::edges(key).collect::<Vec<_>>();
        let elements: Vec<_> = ctx
            .world
            .elements()
            .filter(|e| {
                e.floor == floor
                    && e.material == old
                    && match e.placement {
                        Placement::Free(p) => (p.cell.x, p.cell.z) == key,
                        Placement::Edge(g) => edges.contains(&g),
                        _ => false,
                    }
            })
            .cloned()
            .collect();
        for mut e in elements {
            ctx.request(WorldCommand::RemoveElement { id: e.id });
            e.material = material.clone();
            ctx.request(WorldCommand::PlaceElement(e));
        }
    }
}
