use super::*;
use crate::doafter::RcdJob;
use astrocore_math::GridCell;
use astrocore_world::{SlotKey, StructureElement};

impl State {
    pub(super) fn tick_rcd_recharge(&mut self, ctx: &mut HookContext<'_>) {
        if self.rcd_recharge.is_empty() {
            return;
        }
        let mut changed = BTreeSet::new();
        self.rcd_recharge.retain(|id, (max, period, next)| {
            let count = self.charges.entry(*id).or_default();
            if self.clock >= *next && *count < *max {
                let gain = (1 + ((self.clock - *next) / *period) as u32).min(*max - *count);
                *count += gain;
                *next += gain as f32 * *period;
                changed.insert(*id);
            }
            *count < *max
        });
        if !changed.is_empty() {
            let owners: Vec<_> = self
                .actors
                .iter()
                .filter(|(_, a)| {
                    a.hands
                        .held()
                        .chain(a.inventory.worn())
                        .any(|i| changed.contains(&i.id))
                })
                .map(|(id, _)| *id)
                .collect();
            for owner in owners {
                self.publish(ctx, owner);
            }
        }
    }
    pub(super) fn rcd_charge(&self, item: &HeldItem) -> Option<u32> {
        let name = self.ss14_names.get(&item.prototype)?;
        let initial = construction::CATALOG["rcdDevices"][name]["initial"]
            .as_u64()
            .or_else(|| construction::CATALOG["rcdAmmo"][name].as_u64())?;
        Some(
            self.charges
                .get(&item.id)
                .copied()
                .unwrap_or(initial as u32),
        )
    }

    pub(super) fn select_rcd_mode(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        data: &[u8],
    ) {
        let Some(used) = self.used_item(actor) else {
            return;
        };
        let Ok(mode) = std::str::from_utf8(data) else {
            return;
        };
        if crate::crafting::rcd_modes(used.item.prototype.as_str()).any(|m| m == mode) {
            if self.rcd_modes.get(&used.item.id).is_some_and(|m| m != mode)
                && self.actors[&actor]
                    .doafter
                    .as_ref()
                    .is_some_and(|d| matches!(d.action, Action::Rcd(_)))
            {
                self.cancel(ctx, actor);
            }
            self.rcd_modes.insert(used.item.id, mode.into());
        }
    }

    pub(super) fn refill_rcd(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        used: &Used,
        id: ObjectId,
        proto: &PrototypeId,
    ) -> bool {
        let Some(ammo) = self
            .ss14_names
            .get(&used.item.prototype)
            .and_then(|n| construction::CATALOG["rcdAmmo"][n].as_u64())
        else {
            return false;
        };
        let Some(max) = self
            .ss14_names
            .get(proto)
            .and_then(|n| construction::CATALOG["rcdDevices"][n]["max"].as_u64())
        else {
            return false;
        };
        let available = self
            .charges
            .get(&used.item.id)
            .copied()
            .unwrap_or(ammo as u32);
        let device = HeldItem {
            id,
            prototype: proto.clone(),
        };
        let current = self.rcd_charge(&device).unwrap();
        let transfer = available.min((max as u32).saturating_sub(current));
        if transfer > 0 {
            self.charges.insert(device.id, current + transfer);
            if available == transfer {
                let _ = self.actor(actor).hands.drop_hand(used.hand);
                self.charges.remove(&used.item.id);
            } else {
                self.charges.insert(used.item.id, available - transfer);
            }
            self.publish(ctx, actor);
        }
        true
    }

    pub(super) fn start_rcd(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        data: &[u8],
    ) {
        let Some((rot, Some(point))) = crate::wire::parse_drop(data) else {
            return;
        };
        if rot > 3 {
            return;
        }
        let walls = self.take_walls(ctx);
        self.walls = Some(walls);
        let cell = GridCell::new(point[0].floor() as i32, point[2].floor() as i32);
        if let Some(job) = self.rcd_job(ctx, actor, cell, rot) {
            self.begin(ctx, actor, Action::Rcd(job));
        }
    }

    fn rcd_job(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        cell: GridCell,
        rotation: u8,
    ) -> Option<RcdJob> {
        let used = self.used_item(actor)?;
        let mode = self.rcd_modes.get(&used.item.id)?;
        let spec = &construction::CATALOG["rcdModes"][mode];
        let feet = ctx.position_of(actor)?;
        let floor = floor_at(ctx, feet.y)?;
        let who = self.actors.get(&actor)?;
        if floor != FloorId(0) || who.ghost || !who.health.state().can_act() {
            return None;
        }
        let key = (cell.x, cell.z);
        let wall = self
            .walls
            .as_ref()
            .and_then(|w| w.get(&key))
            .and_then(|w| w.prototype.as_deref());
        let chest = feet + Vec3::Y * CHEST_M;
        let center = Vec3::new(cell.x as f32 + 0.5, chest.y, cell.z as f32 + 0.5);
        let delta = chest - center;
        let face = if wall.is_some() {
            center + delta * (0.61 / delta.x.abs().max(delta.z.abs()).max(0.61))
        } else {
            center
        };
        if crate::module::reach_distance(chest - Vec3::Y * crate::module::CHEST_M, face)
            > self.range_m
            || !ctx.is_unobstructed(chest, face)
        {
            return None;
        }
        let slab = ctx.world.element_at(floor, SlotKey::FloorSlab(cell));
        let tile = slab
            .and_then(|e| e.material.as_str().strip_prefix("ss14:tile_"))
            .and_then(|k| construction::CATALOG["tiles"].get(k));
        let tile_name = tile.and_then(|t| t["name"].as_str()).unwrap_or("Space");
        let objects = || {
            ctx.world.authored().filter(|o| {
                let p = (o.min() + o.max()) * 0.5;
                o.floor == floor
                    && p.x.floor() as i32 == cell.x
                    && p.z.floor() as i32 == cell.z
                    && !self.gone.contains(&o.id)
            })
        };
        let mut target = spec["prototype"].as_str().unwrap_or("");
        let mut cost_spec = spec;
        let mut object = None;
        if mode == "Deconstruct" {
            if let Some(proto) = wall {
                target = proto;
                cost_spec = &construction::CATALOG["rcdTargets"][proto];
            } else if let Some((o, proto)) = objects().find_map(|o| {
                let proto = if let AuthoredKind::Placement { prototype } = &o.kind {
                    self.ss14_names.get(prototype)?.as_str()
                } else if let Some(name) = o.metadata.get("rcd_proto") {
                    name.as_str()
                } else if o.metadata.contains_key("door_element") {
                    if o.metadata.get("construction_node")?.as_str() == "glassAirlock" {
                        "AirlockGlass"
                    } else {
                        "Airlock"
                    }
                } else if tile.is_some_and(|t| t["subfloor"] == true) {
                    o.metadata.get("cable_proto")?.as_str()
                } else {
                    return None;
                };
                construction::CATALOG["rcdTargets"][proto]
                    .is_object()
                    .then_some((o, proto))
            }) {
                target = proto;
                object = Some(o.id);
                cost_spec = &construction::CATALOG["rcdTargets"][proto];
            } else {
                let t = tile?;
                if t["base"].as_str().is_none()
                    || t["indestructible"] == true
                    || objects().any(|o| o.metadata.contains_key("door_element") || o.metadata.contains_key("inflatable_element")
                        || matches!(&o.kind, AuthoredKind::Placement {prototype} if self.info(prototype).is_none() && !self.pipe_parts.contains_key(prototype)))
                    || !self.accessible_tile(ctx, actor, floor, cell)
                {
                    return None;
                }
                target = t["base"].as_str()?;
                object = Some(slab?.id);
                cost_spec = &construction::CATALOG["rcdModes"][if tile_name == "Lattice" {
                    "DeconstructLattice"
                } else {
                    "DeconstructTile"
                }];
            }
            if !cost_spec.is_object() {
                return None;
            }
        } else if spec["mode"] == "ConstructTile" {
            let dest = &construction::CATALOG["tiles"][target.to_lowercase()];
            if !self.accessible_tile(ctx, actor, floor, cell)
                || (dest["base"] != tile_name
                    && !dest["whitelist"]
                        .as_array()
                        .is_some_and(|v| v.iter().any(|n| n == tile_name)))
            {
                return None;
            }
            object = slab.map(|e| e.id);
        } else {
            slab?;
            if spec["rules"]
                .as_array()
                .is_some_and(|rules| rules.iter().any(|r| r == "MustBuildOnSubfloor"))
                && !tile.is_some_and(|t| t["subfloor"] == true)
            {
                return None;
            }
            let cable = construction::CATALOG["cableObjects"][target].is_object();
            let over_grille =
                wall == Some("Grille") && matches!(target, "Window" | "ReinforcedWindow");
            if !over_grille && (!self.accessible_tile(ctx, actor, floor, cell) || wall.is_some()) {
                return None;
            }
            if cable {
                if !tile.is_some_and(|t| t["subfloor"] == true)
                    || objects().any(|o| o.metadata.get("cable_proto").is_some_and(|n| n == target))
                {
                    return None;
                }
            } else {
                let person_in_cell = ctx.players().filter_map(|p| ctx.position_of(p)).any(|p| {
                    floor_at(ctx, p.y) == Some(floor)
                        && p.x.floor() as i32 == cell.x
                        && p.z.floor() as i32 == cell.z
                });
                let fixture_in_cell = objects().any(|o| o.metadata.contains_key("door_element") || o.metadata.contains_key("inflatable_element")
                    || matches!(&o.kind, AuthoredKind::Placement { prototype } if self.info(prototype).is_none()));
                if person_in_cell || fixture_in_cell {
                    return None;
                }
                if matches!(
                    target,
                    "Airlock"
                        | "AirlockGlass"
                        | "Firelock"
                        | "WindowDirectional"
                        | "WindowReinforcedDirectional"
                ) && ctx
                    .world
                    .edge_element(
                        floor,
                        super::computer_construction::door_edge(cell, rotation),
                    )
                    .is_some()
                {
                    return None;
                }
                if !(construction::CATALOG["barriers"][target].is_object()
                    || matches!(
                        target,
                        "Airlock"
                            | "AirlockGlass"
                            | "Firelock"
                            | "WindowDirectional"
                            | "WindowReinforcedDirectional"
                    )
                    || self.proto_by_name.contains_key(target))
                {
                    return None;
                }
            }
        }
        let cost = cost_spec["cost"].as_u64().unwrap_or(1) as u32;
        if self.rcd_charge(&used.item)? < cost {
            return None;
        }
        Some(RcdJob {
            device: used.item.id,
            floor,
            cell,
            rotation,
            mode: mode.clone(),
            target: target.into(),
            object,
            cost,
            delay: cost_spec["delay"].as_f64().unwrap_or(1.) as f32,
        })
    }

    pub(super) fn can_rcd(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        j: &RcdJob,
    ) -> bool {
        self.rcd_job(ctx, actor, j.cell, j.rotation).as_ref() == Some(j)
    }

    pub(super) fn finish_rcd(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        j: RcdJob,
    ) {
        if !self.can_rcd(ctx, actor, &j) {
            return;
        }
        let used = self.used_item(actor).unwrap();
        let key = (j.cell.x, j.cell.z);
        let name = j.target.as_str();
        let pos = Vec3::new(
            j.cell.x as f32 + 0.5,
            ctx.world.floor(j.floor).unwrap().base_y + 0.2,
            j.cell.z as f32 + 0.5,
        );
        if j.mode == "Deconstruct" {
            if let Some(id) = j.object {
                if let Some(o) = ctx.world.authored_object(id) {
                    if let Some(door) = o
                        .metadata
                        .get("door_element")
                        .or_else(|| o.metadata.get("rcd_element"))
                        .and_then(|s| u128::from_str_radix(s, 16).ok())
                    {
                        ctx.request(WorldCommand::RemoveElement {
                            id: ObjectId::from_u128(door),
                        });
                    }
                    self.gone.insert(id);
                    ctx.request(WorldCommand::RemoveAuthored { id });
                } else {
                    self.replace_tile(ctx, j.floor, j.cell, Some(name));
                }
            } else {
                let mut walls = self.take_walls(ctx);
                if !self.restore_window_grille(ctx, j.floor, key, &mut walls) {
                    walls.remove(&key);
                    self.open_wall_geometry(ctx, j.floor, key, &walls);
                }
                self.walls = Some(walls);
            }
        } else if construction::CATALOG["rcdModes"][&j.mode]["mode"] == "ConstructTile" {
            self.replace_tile(ctx, j.floor, j.cell, Some(name));
        } else if construction::CATALOG["barriers"][name].is_object() {
            if self
                .walls
                .as_ref()
                .and_then(|w| w.get(&key))
                .and_then(|w| w.prototype.as_deref())
                == Some("Grille")
            {
                self.replace_barrier(ctx, j.floor, key, "Grille", name);
                self.window_grilles.insert(key);
                self.walls
                    .as_mut()
                    .unwrap()
                    .insert(key, construction::barrier_health(name));
            } else {
                self.build_barrier(
                    ctx,
                    j.floor,
                    key,
                    name,
                    prototypes::engine_id("ss14", name).unwrap(),
                );
            }
        } else {
            self.wall_serial += 1;
            let id = ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial);
            let mut o = AuthoredObject::new(
                id,
                AuthoredKind::Zone,
                j.floor,
                pos - Vec3::new(0.35, 0., 0.35),
                pos + Vec3::new(0.35, 0.8, 0.35),
            );
            match name {
                "CableHV" | "CableMV" | "CableApcExtension" => {
                    o.metadata.insert("cable_proto".into(), name.into());
                    o.metadata.insert(
                        "power_cable".into(),
                        match name {
                            "CableHV" => "hv",
                            "CableMV" => "mv",
                            _ => "apc",
                        }
                        .into(),
                    );
                }
                "Airlock"
                | "AirlockGlass"
                | "Firelock"
                | "WindowDirectional"
                | "WindowReinforcedDirectional" => {
                    self.wall_serial += 1;
                    let element = ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial);
                    let window = name.starts_with("Window");
                    let material = match name {
                        "WindowDirectional" => "Window",
                        "WindowReinforcedDirectional" => "ReinforcedWindow",
                        n => n,
                    };
                    ctx.request(WorldCommand::PlaceElement(StructureElement {
                        id: element,
                        floor: j.floor,
                        kind: StructureKind::Opening(if window {
                            OpeningKind::Window
                        } else {
                            OpeningKind::Door
                        }),
                        placement: Placement::Edge(super::computer_construction::door_edge(
                            j.cell, j.rotation,
                        )),
                        material: prototypes::engine_id("ss14", material).unwrap(),
                        thickness: 0.1,
                    }));
                    o.metadata.insert("rcd_proto".into(), name.into());
                    o.metadata.insert(
                        "rcd_element".into(),
                        format!("{:x}", element.as_uuid().as_u128()),
                    );
                    if matches!(name, "Airlock" | "AirlockGlass") {
                        o.metadata.insert(
                            "door_element".into(),
                            format!("{:x}", element.as_uuid().as_u128()),
                        );
                        o.metadata
                            .insert("construction_graph".into(), "Airlock".into());
                        o.metadata.insert(
                            "construction_node".into(),
                            if name == "Airlock" {
                                "airlock"
                            } else {
                                "glassAirlock"
                            }
                            .into(),
                        );
                        o.metadata
                            .insert("frame_board".into(), "DoorElectronics".into());
                    }
                }
                _ => {
                    o.kind = AuthoredKind::Placement {
                        prototype: prototypes::engine_id("ss14", name).unwrap(),
                    }
                }
            }
            o.metadata.insert("rot".into(), j.rotation.to_string());
            ctx.request(WorldCommand::PlaceAuthored(o));
        }
        let remaining = self.rcd_charge(&used.item).unwrap() - j.cost;
        self.charges.insert(j.device, remaining);
        let device = &construction::CATALOG["rcdDevices"][&self.ss14_names[&used.item.prototype]];
        if let Some(period) = device["recharge"]
            .as_f64()
            .map(|d| d as f32)
            .filter(|d| *d > 0.)
        {
            self.rcd_recharge.entry(j.device).or_insert((
                device["max"].as_u64().unwrap() as u32,
                period,
                self.clock + period,
            ));
        }
        self.engineering_sound(
            &construction::CATALOG["rcdDevices"][&self.ss14_names[&used.item.prototype]]["sound"],
            pos,
        );
        self.publish(ctx, actor);
    }
}
