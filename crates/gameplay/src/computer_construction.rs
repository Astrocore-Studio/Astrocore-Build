use super::*;
use crate::doafter::FrameJob;
use astrocore_math::GridCell;
use astrocore_world::SlotKey;
use serde_json::Value;

pub(super) struct AirlockWire {
    pub(super) action: &'static str,
    pub(super) letter: char,
    pub(super) color: &'static str,
    pub(super) original_position: usize,
    pub(super) spec: Option<&'static Value>,
}

pub(super) fn wire_layouts() -> BTreeMap<String, Vec<AirlockWire>> {
    use std::hash::{BuildHasher, RandomState};
    let mut rng = crate::disposal::Rng(RandomState::new().hash_one("airlock wires") | 1);
    fn shuffle<T>(values: &mut [T], rng: &mut crate::disposal::Rng) {
        for last in (1..values.len()).rev() {
            let index = (rng.next_f32() * (last + 1) as f32) as usize;
            values.swap(last, index.min(last));
        }
    }
    construction::CATALOG["wireLayouts"]
        .as_object()
        .expect("wire layouts")
        .iter()
        .map(|(name, spec)| {
            let mut letters: Vec<_> = "αβγδεζηθικλμνξοπρστυφχψω".chars().collect();
            let mut colors = [
                "Red", "Blue", "Green", "Orange", "Brown", "Gold", "Gray", "Cyan", "Navy",
                "Purple", "Pink", "Fuchsia",
            ];
            shuffle(&mut letters, &mut rng);
            shuffle(&mut colors, &mut rng);
            let dummy = spec["dummyWires"].as_u64().unwrap_or(0) as usize;
            let mut wires: Vec<_> = spec["wires"]
                .as_array()
                .unwrap()
                .iter()
                .map(Some)
                .chain(std::iter::repeat_n(None, dummy))
                .enumerate()
                .map(|(i, spec)| AirlockWire {
                    action: spec.and_then(|s| s["__tag"].as_str()).unwrap_or(""),
                    original_position: i,
                    spec,
                    letter: letters[i],
                    color: colors[i],
                })
                .collect();
            shuffle(&mut wires, &mut rng);
            (name.clone(), wires)
        })
        .collect()
}

pub(super) fn cut_mask(o: &AuthoredObject) -> u32 {
    o.metadata
        .get("cut_wires")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn node(graph: &str, id: &str) -> Option<&'static Value> {
    construction::CATALOG["graphs"][graph]["graph"]
        .as_array()?
        .iter()
        .find(|n| n["node"] == id)
}

fn frame_state(object: &AuthoredObject) -> Option<(&str, &str)> {
    if let (Some(graph), Some(node)) = (
        object.metadata.get("construction_graph"),
        object.metadata.get("construction_node"),
    ) {
        return Some((graph, node));
    }
    let AuthoredKind::Placement { prototype } = &object.kind else {
        return None;
    };
    Some(match prototype.as_str() {
        "ss14:unfinished_machine_frame" => ("Machine", "missingWires"),
        "ss14:machine_frame" => ("Machine", "machineFrame"),
        "ss14:machine_frame_destroyed" => ("Machine", "destroyedMachineFrame"),
        "ss14:computer_frame" => ("Computer", "frameUnsecured"),
        "ss14:computer_broken" => ("Computer", "monitorBroken"),
        "ss14:airlock_assembly" => ("Airlock", "assembly"),
        _ => return None,
    })
}

fn requirements(board: &str) -> BTreeMap<String, u32> {
    let spec = &construction::CATALOG["machineBoards"][board];
    let mut result = BTreeMap::new();
    for (field, prefix) in [
        ("stackRequirements", "stack"),
        ("componentRequirements", "component"),
        ("tagRequirements", "tag"),
    ] {
        for (key, value) in spec[field].as_object().into_iter().flatten() {
            if let Some(n) = value.as_u64().or_else(|| value["amount"].as_u64()) {
                result.insert(format!("{prefix}/{key}"), n as u32);
            }
        }
    }
    result
}

fn progress(frame: &AuthoredObject, key: &str) -> u32 {
    frame
        .metadata
        .get(&format!("required/{key}"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

impl State {
    fn device_node<'a>(&self, o: &'a AuthoredObject) -> Option<(&'a str, &'a str)> {
        if let Some(state) = frame_state(o) {
            return Some(state);
        }
        let AuthoredKind::Placement { prototype } = &o.kind else {
            return None;
        };
        let name = self.ss14_names.get(prototype)?;
        if construction::CATALOG["computers"][name].is_string() {
            Some(("Computer", "computer"))
        } else if construction::CATALOG["machines"][name].is_object() {
            Some(("Machine", "machine"))
        } else {
            None
        }
    }

    fn frame_job(
        &self,
        used: &Used,
        graph: &str,
        current: &str,
        location: (FloorId, GridCell),
        object: Option<&AuthoredObject>,
    ) -> Option<FrameJob> {
        let (floor, cell) = location;
        let spec = node(graph, current)?;
        let name = self.ss14_names.get(&used.item.prototype)?;
        let qualities = &self.info(&used.item.prototype)?.qualities;
        let anchored = object.is_some_and(|o| !o.metadata.contains_key("loose_frame"));
        let machine = object
            .and_then(|o| match &o.kind {
                AuthoredKind::Placement { prototype } => self.ss14_names.get(prototype),
                _ => None,
            })
            .map(|name| &construction::CATALOG["machines"][name]);
        let panel = machine.map(|m| &m["panel"]).filter(|p| p.is_object());
        let door = graph == "Airlock" && object.is_some_and(|o| element_of_door(o).is_some());
        let panel_open = object
            .and_then(|o| o.metadata.get("panel_open"))
            .map(|v| v == "1")
            .unwrap_or_else(|| panel.is_some_and(|p| p["open"] == true));
        let mut job = FrameJob {
            rotation: object
                .and_then(|o| o.metadata.get("rot"))
                .and_then(|r| r.parse().ok())
                .unwrap_or(0),
            floor,
            cell,
            target: object.map(|o| o.id),
            item: used.item.id,
            graph: graph.into(),
            node: current.into(),
            next: String::new(),
            delay: 0.,
        };
        if door && qualities.contains("Screwing") {
            job.next = if panel_open {
                "closePanel"
            } else {
                "openPanel"
            }
            .into();
            job.delay = 1.;
            return Some(job);
        }
        if door
            && matches!(current, "airlock" | "glassAirlock" | "highSecDoor")
            && qualities.contains("Welding")
            && let Some(spec) = self.welding_tools.get(&used.item.prototype)
            && self.lit.contains(&used.item.id)
            && self
                .solutions
                .get(&used.item.id)
                .or_else(|| self.solution_templates.get(&used.item.prototype))
                .is_some_and(|s| s.amount(&spec.fuel) >= 5.)
        {
            job.next = if object?.metadata.get("welded").is_some_and(|v| v == "1") {
                "unweldDoor"
            } else {
                "weldDoor"
            }
            .into();
            job.delay = 3. / spec.speed;
            return Some(job);
        }
        if current == "machine"
            && let Some(panel) = panel
            && qualities.contains(panel["tool"].as_str()?)
        {
            job.next = if panel_open {
                "closePanel"
            } else {
                "openPanel"
            }
            .into();
            job.delay = panel["delay"].as_f64()? as f32;
            return Some(job);
        }
        if current == "machineFrame"
            && let Some(o) = object
        {
            if !o.metadata.contains_key("frame_board")
                && construction::CATALOG["machineBoards"][name].is_object()
            {
                job.next = "insertBoard".into();
                return Some(job);
            }
            if !self.part_contributions(o, &used.item).is_empty() {
                job.next = "insertPart".into();
                return Some(job);
            }
        }
        if object.is_some()
            && current != "computer"
            && current != "machine"
            && !door
            && qualities.contains("Anchoring")
        {
            job.next = if anchored { "unanchor" } else { "anchor" }.into();
            job.delay = 1.;
            return Some(job);
        }
        for edge in spec["edges"].as_array()? {
            let next = edge["to"].as_str()?;
            if object
                .and_then(|o| o.metadata.get("pending_edge"))
                .is_some_and(|pending| pending != next)
            {
                continue;
            }
            if current == "start" && matches!(next, "destroyedMachineFrame") {
                continue;
            }
            let conditions = edge["conditions"]
                .as_array()
                .into_iter()
                .flatten()
                .all(|c| match c["__tag"].as_str() {
                    Some("EntityAnchored") => anchored == c["anchored"].as_bool().unwrap_or(true),
                    Some("WirePanel") => {
                        (!door && panel.is_none())
                            || panel_open == c["open"].as_bool().unwrap_or(true)
                    }
                    Some("DoorWelded") => object.is_some_and(|o| {
                        o.metadata.get("welded").is_some_and(|v| v == "1")
                            == c["welded"].as_bool().unwrap_or(true)
                    }),
                    Some("DoorBolted") => object.is_some_and(|o| {
                        o.metadata.get("bolted").is_some_and(|v| v == "1")
                            == c["value"].as_bool().unwrap_or(true)
                    }),
                    Some("AllWiresCut") => object
                        .is_some_and(|o| cut_mask(o) == (1u32 << self.airlock_wires(o).len()) - 1),
                    Some("HasTag") => object.is_some_and(|o| {
                        self.airlock_spec(o)["tags"]
                            .as_array()
                            .is_some_and(|tags| tags.contains(&c["tag"]))
                    }),
                    Some("Locked") => {
                        machine
                            .and_then(|m| m["locked"].as_bool())
                            .is_none_or(|initial| {
                                object
                                    .and_then(|o| o.metadata.get("locked"))
                                    .map_or(initial, |v| v == "1")
                                    == c["locked"].as_bool().unwrap_or(true)
                            })
                    }
                    Some("MachineFrameComplete") => object.is_some_and(|o| {
                        o.metadata.get("frame_board").is_some_and(|board| {
                            requirements(board)
                                .iter()
                                .all(|(key, n)| progress(o, key) >= *n)
                        })
                    }),
                    Some("ContainerEmpty") => {
                        object.is_some_and(|o| !o.metadata.contains_key("frame_board"))
                    }
                    Some("ContainerNotEmpty") => {
                        object.is_some_and(|o| o.metadata.contains_key("frame_board"))
                    }
                    _ => false,
                });
            if !conditions {
                continue;
            }
            let index = object
                .and_then(|o| o.metadata.get("edge_step"))
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            let Some(step) = edge["steps"].get(index) else {
                continue;
            };
            let matches = if let Some(material) = step["material"].as_str() {
                self.material_count(&used.item, material)
                    .is_some_and(|n| n >= step["amount"].as_u64().unwrap_or(1) as u32)
            } else if let Some(tool) = step["tool"].as_str() {
                qualities.contains(tool)
            } else {
                (step["component"] == "ComputerBoard"
                    && construction::CATALOG["computerBoards"][name].is_string())
                    || (step["component"] == "DoorElectronics"
                        && construction::CATALOG["doorBoards"][name].is_array())
            };
            if !matches {
                continue;
            }
            if next == "computer"
                && !object
                    .and_then(|o| o.metadata.get("frame_board"))
                    .is_some_and(|b| {
                        construction::CATALOG["computerBoards"][b]
                            .as_str()
                            .is_some_and(|n| self.proto_by_name.contains_key(n))
                    })
            {
                continue;
            }
            job.next = next.into();
            job.delay = step["doAfter"].as_f64().unwrap_or(0.) as f32;
            if let Some(speed) = self.welding_tools.get(&used.item.prototype) {
                if !self.lit.contains(&used.item.id)
                    || self
                        .solutions
                        .get(&used.item.id)
                        .or_else(|| self.solution_templates.get(&used.item.prototype))
                        .is_none_or(|s| s.amount(&speed.fuel) < 10.)
                {
                    continue;
                }
                job.delay /= speed.speed;
            }
            return Some(job);
        }
        None
    }

    fn part_contributions(&self, o: &AuthoredObject, item: &HeldItem) -> Vec<(String, u32)> {
        let Some(board) = o.metadata.get("frame_board") else {
            return Vec::new();
        };
        let Some(name) = self.ss14_names.get(&item.prototype) else {
            return Vec::new();
        };
        let traits = construction::CATALOG["partTraits"][name].as_array();
        requirements(board)
            .into_iter()
            .filter_map(|(key, n)| {
                let needed = n.saturating_sub(progress(o, &key));
                let add = if let Some(kind) = key.strip_prefix("stack/") {
                    self.material_count(item, kind).unwrap_or(0).min(needed)
                } else {
                    u32::from(
                        needed > 0
                            && self.stack_count(item).is_none()
                            && traits.is_some_and(|t| t.iter().any(|v| v == key.as_str())),
                    )
                };
                (add > 0).then_some((key, add))
            })
            .collect()
    }

    fn populate_machine(&mut self, object: &mut AuthoredObject, board: &str) {
        let spec = &construction::CATALOG["machineBoards"][board];
        object.metadata.insert("frame_board".into(), board.into());
        for (key, amount) in requirements(board) {
            let (kind, name) = key.split_once('/').unwrap();
            let (proto, instances, count) = match kind {
                "stack" => {
                    let spawn = construction::CATALOG["stacks"]
                        .as_object()
                        .and_then(|s| s.values().find(|v| v["kind"] == name))
                        .and_then(|v| v["spawn"].as_str());
                    (spawn, 1, amount)
                }
                "component" => (
                    spec["componentRequirements"][name]["defaultPrototype"].as_str(),
                    amount,
                    0,
                ),
                "tag" => (
                    spec["tagRequirements"][name]["defaultPrototype"].as_str(),
                    amount,
                    0,
                ),
                _ => continue,
            };
            if let Some(proto) = proto {
                for _ in 0..instances {
                    if let Some(item) = self.create_item(proto) {
                        object.metadata.insert(
                            format!("part/{:x}", item.id.as_uuid().as_u128()),
                            format!("{proto}:{count}"),
                        );
                        let filled = progress(object, &key) + count.max(1);
                        object
                            .metadata
                            .insert(format!("required/{key}"), filled.to_string());
                    }
                }
            }
        }
    }

    fn empty_frame(
        &mut self,
        ctx: &mut HookContext<'_>,
        frame: &mut AuthoredObject,
        at: Vec3,
        pickup: Option<NetworkEntityId>,
    ) {
        if let Some(name) = frame.metadata.remove("frame_board")
            && let Some(mut board) = self.create_item(&name)
        {
            if let Some(id) = frame
                .metadata
                .remove("frame_board_id")
                .and_then(|s| u128::from_str_radix(&s, 16).ok())
            {
                board.id = ObjectId::from_u128(id);
            }
            if let Some(access) = frame.metadata.remove("installed_access") {
                self.board_access.insert(board.id, access);
            }
            if pickup.is_none_or(|who| self.actor(who).hands.pickup_any(board.clone()).is_err()) {
                self.place(ctx, &board, at);
            }
        }
        for (key, value) in frame.metadata.clone() {
            if let Some(id) = key
                .strip_prefix("part/")
                .and_then(|s| u128::from_str_radix(s, 16).ok())
                && let Some((name, n)) = value.split_once(':')
                && let Some(mut part) = self.create_item(name)
            {
                part.id = ObjectId::from_u128(id);
                if let Ok(n) = n.parse::<u32>()
                    && n > 0
                {
                    self.charges.insert(part.id, n);
                }
                self.place(ctx, &part, at);
            }
        }
        frame
            .metadata
            .retain(|k, _| !k.starts_with("part/") && !k.starts_with("required/"));
    }

    pub(super) fn start_frame(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        at: Vec3,
        graph: &str,
        rotation: u8,
    ) {
        let Some(used) = self.used_item(actor) else {
            return;
        };
        let Some(floor) = ctx.position_of(actor).and_then(|p| floor_at(ctx, p.y)) else {
            return;
        };
        let cell = GridCell::new(at.x.floor() as i32, at.z.floor() as i32);
        if let Some(mut job) = self.frame_job(&used, graph, "start", (floor, cell), None) {
            job.rotation = rotation;
            self.begin(ctx, actor, Action::Frame(job));
        }
    }

    pub(super) fn begin_frame_interaction(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) -> bool {
        let Some(o) = ctx.world.authored_object(id) else {
            return false;
        };
        let Some((graph, n)) = self.device_node(o) else {
            return false;
        };
        let Some(used) = self.used_item(actor) else {
            return false;
        };
        let pos = (o.min() + o.max()) * 0.5;
        let cell = GridCell::new(pos.x.floor() as i32, pos.z.floor() as i32);
        let Some(job) = self.frame_job(&used, graph, n, (o.floor, cell), Some(o)) else {
            return false;
        };
        self.begin(ctx, actor, Action::Frame(job));
        true
    }

    pub(super) fn use_frame_at(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        point: Vec3,
    ) -> bool {
        if !self.used_item(actor).is_some_and(|u| u.hand == hand) {
            return false;
        }
        let Some(floor) = ctx.position_of(actor).and_then(|p| floor_at(ctx, p.y)) else {
            return false;
        };
        let target = ctx
            .world
            .authored()
            .find(|o| {
                let at = (o.min() + o.max()) * 0.5;
                o.floor == floor
                    && at.x.floor() == point.x.floor()
                    && at.z.floor() == point.z.floor()
                    && self.device_node(o).is_some()
                    && !self.gone.contains(&o.id)
            })
            .map(|o| o.id);
        target.is_some_and(|id| self.begin_frame_interaction(ctx, actor, id))
    }

    pub(super) fn can_frame(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        job: &FrameJob,
    ) -> bool {
        let Some(used) = self.used_item(actor) else {
            return false;
        };
        if used.item.id != job.item || !self.accessible_tile(ctx, actor, job.floor, job.cell) {
            return false;
        }
        let object = job.target.and_then(|id| ctx.world.authored_object(id));
        if matches!(job.next.as_str(), "weldDoor" | "unweldDoor")
            && (object
                .and_then(element_of_door)
                .is_some_and(|id| self.airlock_moving(id))
                || object
                    .and_then(element_of_door)
                    .and_then(|id| ctx.door_open(id))
                    != Some(false))
        {
            return false;
        }
        if let Some(id) = job.target {
            let Some(o) = object.filter(|o| !self.gone.contains(&o.id) && o.id == id) else {
                return false;
            };
            let current = self.device_node(o);
            if current != Some((job.graph.as_str(), job.node.as_str())) {
                return false;
            }
        } else {
            if ctx
                .world
                .element_at(job.floor, SlotKey::FloorSlab(job.cell))
                .is_none()
            {
                return false;
            }
            if ctx.world.authored().any(|o| o.floor==job.floor && matches!(&o.kind,AuthoredKind::Placement{prototype} if self.info(prototype).is_none() || frame_state(o).is_some()) && {
                let at=(o.min()+o.max())*0.5; at.x.floor() as i32==job.cell.x && at.z.floor() as i32==job.cell.z
            }) {return false;}
        }
        let Some(mut candidate) =
            self.frame_job(&used, &job.graph, &job.node, (job.floor, job.cell), object)
        else {
            return false;
        };
        if job.target.is_none() {
            candidate.rotation = job.rotation;
        }
        if matches!(job.next.as_str(), "airlock" | "glassAirlock")
            && object.and_then(element_of_door).is_none()
            && ctx
                .world
                .edge_element(job.floor, door_edge(job.cell, job.rotation))
                .is_some()
        {
            return false;
        }
        candidate == *job
    }

    pub(super) fn finish_frame(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        job: &FrameJob,
    ) {
        if !self.can_frame(ctx, actor, job) {
            return;
        }
        let used = self.used_item(actor).unwrap();
        let candidate = job
            .target
            .and_then(|id| ctx.world.authored_object(id))
            .cloned();
        let at = Vec3::new(
            job.cell.x as f32 + 0.5,
            ctx.world.floor(job.floor).unwrap().base_y + 0.2,
            job.cell.z as f32 + 0.5,
        );
        let mut object = if let Some(o) = candidate {
            o
        } else {
            self.wall_serial += 1;
            let proto = node(&job.graph, &job.next).unwrap()["entity"]
                .as_str()
                .unwrap();
            AuthoredObject::new(
                ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial),
                AuthoredKind::Placement {
                    prototype: prototypes::engine_id("ss14", proto).unwrap(),
                },
                job.floor,
                at - Vec3::new(0.35, 0., 0.35),
                at + Vec3::new(0.35, 0.8, 0.35),
            )
        };
        if job.target.is_some() {
            ctx.request(WorldCommand::RemoveAuthored { id: object.id });
        } else {
            object
                .metadata
                .insert("rot".into(), job.rotation.to_string());
        }
        if let Some(feet) = ctx.position_of(actor) {
            self.tool_sound(&used.item.prototype, "use", feet);
        }
        match job.next.as_str() {
            "weldDoor" | "unweldDoor" => {
                let spec = self.welding_tools[&used.item.prototype];
                self.solution_mut(&used.item)
                    .unwrap()
                    .remove(&spec.fuel, 5.);
                object.metadata.insert(
                    "welded".into(),
                    u8::from(job.next == "weldDoor").to_string(),
                );
            }
            "openPanel" | "closePanel" => {
                if job.graph == "Airlock" {
                    self.sound(
                        if job.next == "openPanel" {
                            "Machines/screwdriveropen.ogg"
                        } else {
                            "Machines/screwdriverclose.ogg"
                        },
                        at,
                    );
                }
                if let AuthoredKind::Placement { prototype } = &object.kind
                    && let Some(name) = self.ss14_names.get(prototype)
                {
                    let event = if job.next == "openPanel" {
                        "openSound"
                    } else {
                        "closeSound"
                    };
                    self.engineering_sound(
                        &construction::CATALOG["machines"][name]["panel"][event],
                        at,
                    );
                }
                object.metadata.insert(
                    "panel_open".into(),
                    u8::from(job.next == "openPanel").to_string(),
                );
            }
            "insertBoard" => {
                object.metadata.insert(
                    "frame_board".into(),
                    self.ss14_names[&used.item.prototype].clone(),
                );
                object.metadata.insert(
                    "frame_board_id".into(),
                    format!("{:x}", used.item.id.as_uuid().as_u128()),
                );
                let _ = self.actor(actor).hands.drop_hand(used.hand);
            }
            "insertPart" => {
                let contribution = self.part_contributions(&object, &used.item);
                let name = self.ss14_names[&used.item.prototype].clone();
                let mut stored_id = used.item.id;
                let mut stored_count = 0;
                if let Some(total) = self.stack_count(&used.item) {
                    stored_count = contribution[0].1;
                    if stored_count < total {
                        stored_id = self.create_item(&name).unwrap().id;
                    }
                    self.consume_stack(ctx, actor, &used, total, stored_count);
                } else {
                    let _ = self.actor(actor).hands.drop_hand(used.hand);
                }
                for (key, n) in contribution {
                    let total = progress(&object, &key) + n;
                    object
                        .metadata
                        .insert(format!("required/{key}"), total.to_string());
                }
                object.metadata.insert(
                    format!("part/{:x}", stored_id.as_uuid().as_u128()),
                    format!("{name}:{stored_count}"),
                );
            }
            "anchor" => {
                object.metadata.remove("loose_frame");
            }
            "unanchor" => {
                object.metadata.insert("loose_frame".into(), "true".into());
            }
            _ => {
                let edge = node(&job.graph, &job.node).unwrap()["edges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["to"] == job.next)
                    .unwrap();
                let index = object
                    .metadata
                    .get("edge_step")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0);
                let steps = edge["steps"].as_array().unwrap();
                let step = &steps[index];
                if let Some(material) = step["material"].as_str() {
                    self.consume_stack(
                        ctx,
                        actor,
                        &used,
                        self.material_count(&used.item, material).unwrap(),
                        step["amount"].as_u64().unwrap_or(1) as u32,
                    );
                } else if matches!(
                    step["component"].as_str(),
                    Some("ComputerBoard" | "DoorElectronics")
                ) {
                    if let Some(access) = self.board_access.remove(&used.item.id) {
                        object.metadata.insert("installed_access".into(), access);
                    } else {
                        object.metadata.remove("installed_access");
                    }
                    object.metadata.insert(
                        "frame_board".into(),
                        self.ss14_names[&used.item.prototype].clone(),
                    );
                    object.metadata.insert(
                        "frame_board_id".into(),
                        format!("{:x}", used.item.id.as_uuid().as_u128()),
                    );
                    let _ = self.actor(actor).hands.drop_hand(used.hand);
                } else if let Some(spec) = self.welding_tools.get(&used.item.prototype).copied() {
                    self.solution_mut(&used.item)
                        .unwrap()
                        .remove(&spec.fuel, 10.);
                }
                if index + 1 < steps.len() {
                    object
                        .metadata
                        .insert("pending_edge".into(), job.next.clone());
                    object
                        .metadata
                        .insert("edge_step".into(), (index + 1).to_string());
                    ctx.request(WorldCommand::PlaceAuthored(object));
                    self.publish(ctx, actor);
                    return;
                }
                object.metadata.remove("pending_edge");
                object.metadata.remove("edge_step");
                if job.node == "computer"
                    && !object.metadata.contains_key("frame_board")
                    && let AuthoredKind::Placement { prototype } = &object.kind
                    && let Some(board) =
                        construction::CATALOG["computers"][&self.ss14_names[prototype]].as_str()
                {
                    object.metadata.insert("frame_board".into(), board.into());
                }
                if job.node == "machine" {
                    if !object.metadata.contains_key("frame_board")
                        && let AuthoredKind::Placement { prototype } = &object.kind
                        && let Some(board) = construction::CATALOG["machines"]
                            [&self.ss14_names[prototype]]["board"]
                            .as_str()
                    {
                        self.populate_machine(&mut object, board);
                    }
                    object.metadata.remove("panel_open");
                }
                if job.graph == "Airlock"
                    && matches!(job.next.as_str(), "wired" | "glassElectronics")
                {
                    if let Some(id) = element_of_door(&object) {
                        ctx.request(WorldCommand::RemoveElement { id });
                    }
                    for field in [
                        "door_element",
                        "welded",
                        "panel_open",
                        "cut_wires",
                        "bolted",
                        "airlock_proto",
                        "wire_layout",
                        "wire_power_disabled",
                        "wire_power_pulsed",
                        "wire_power_until",
                        "wire_timing_until",
                        "wire_safety_until",
                        "wire_logging_until",
                        "close_modifier",
                        "safety",
                        "logging",
                        "ai_control",
                        "bolt_lights",
                        "electrified",
                    ] {
                        object.metadata.remove(field);
                    }
                }
                for callback in step["completed"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(edge["completed"].as_array().into_iter().flatten())
                {
                    match callback["__tag"].as_str() {
                        Some("SetAnchor") if callback["value"] == false => {
                            object.metadata.insert("loose_frame".into(), "true".into());
                        }
                        Some("EmptyAllContainers") => {
                            self.empty_frame(
                                ctx,
                                &mut object,
                                if callback["emptyAtUser"] == true {
                                    ctx.position_of(actor).unwrap()
                                } else {
                                    at
                                },
                                (callback["pickup"] == true).then_some(actor),
                            );
                        }
                        Some("AttemptElectrocute") => {
                            if let Some(element) = element_of_door(&object) {
                                self.attempt_airlock_electrocution(ctx, actor, element, &object);
                            }
                        }
                        Some("GivePrototype") | Some("SpawnPrototype") => {
                            if let Some(item) = callback["prototype"]
                                .as_str()
                                .and_then(|n| self.create_item(n))
                            {
                                self.charges.insert(
                                    item.id,
                                    callback["amount"].as_u64().unwrap_or(1) as u32,
                                );
                                let count = callback["amount"].as_u64().unwrap_or(1) as u32;
                                if callback["__tag"] != "GivePrototype"
                                    || !self.merge_into_hands(ctx, actor, &item, count)
                                {
                                    self.place(
                                        ctx,
                                        &item,
                                        if callback["__tag"] == "GivePrototype" {
                                            ctx.position_of(actor).unwrap_or(at)
                                        } else {
                                            at
                                        },
                                    );
                                }
                            }
                        }
                        Some("DeleteEntity") => {
                            self.gone.insert(object.id);
                            self.publish(ctx, actor);
                            return;
                        }
                        _ => {}
                    }
                }
                let next = &node(&job.graph, &job.next).unwrap()["entity"];
                let proto = next.as_str().or_else(|| {
                    if next["__tag"] == "BoardNodeEntity" {
                        let board = object.metadata.get("frame_board")?;
                        if job.graph == "Machine" {
                            construction::CATALOG["machineBoards"][board]["prototype"].as_str()
                        } else {
                            construction::CATALOG["computerBoards"][board].as_str()
                        }
                    } else {
                        None
                    }
                });
                if let Some(proto) = proto.filter(|_| element_of_door(&object).is_none()) {
                    object.kind = AuthoredKind::Placement {
                        prototype: prototypes::engine_id("ss14", proto).unwrap(),
                    };
                }
                if matches!(job.next.as_str(), "airlock" | "glassAirlock")
                    && element_of_door(&object).is_none()
                {
                    self.wall_serial += 1;
                    let id = ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial);
                    ctx.request(WorldCommand::PlaceElement(
                        astrocore_world::StructureElement {
                            id,
                            floor: job.floor,
                            kind: StructureKind::Opening(OpeningKind::Door),
                            placement: Placement::Edge(door_edge(job.cell, job.rotation)),
                            material: prototypes::engine_id("ss14", proto.unwrap()).unwrap(),
                            thickness: 0.1,
                        },
                    ));
                    object.metadata.insert(
                        "door_element".into(),
                        format!("{:x}", id.as_uuid().as_u128()),
                    );
                    object.kind = AuthoredKind::Zone;
                    object
                        .metadata
                        .insert("airlock_proto".into(), proto.unwrap().into());
                }
                object
                    .metadata
                    .insert("construction_graph".into(), job.graph.clone());
                object
                    .metadata
                    .insert("construction_node".into(), job.next.clone());
            }
        }
        let appearance = match (
            object
                .metadata
                .get("construction_graph")
                .map(String::as_str),
            object.metadata.get("construction_node").map(String::as_str),
        ) {
            (Some("Computer"), Some("boardUnsecured")) => Some("ss14:computer_frame_1"),
            (Some("Computer"), Some("missingWires")) => Some("ss14:computer_frame_2"),
            (Some("Computer"), Some("monitorMissing")) => Some("ss14:computer_frame_3"),
            (Some("Computer"), Some("monitorUnsecured")) => Some("ss14:computer_frame_4"),
            (Some("Machine"), Some("machineFrame"))
                if object.metadata.contains_key("frame_board") =>
            {
                Some("ss14:machine_frame_box_2")
            }
            _ => None,
        };
        if let Some(id) = appearance {
            object.metadata.insert("render_sprite".into(), id.into());
        } else {
            object.metadata.remove("render_sprite");
        }
        ctx.request(WorldCommand::PlaceAuthored(object));
        self.publish(ctx, actor);
    }
}

pub(super) fn door_edge(cell: GridCell, rotation: u8) -> astrocore_math::GridEdge {
    let (x, z) = [(0, 1), (1, 0), (0, -1), (-1, 0)][usize::from(rotation % 4)];
    astrocore_math::GridEdge::between(cell, GridCell::new(cell.x + x, cell.z + z)).unwrap()
}

fn element_of_door(o: &AuthoredObject) -> Option<ObjectId> {
    Some(ObjectId::from_u128(
        u128::from_str_radix(o.metadata.get("door_element")?, 16).ok()?,
    ))
}

impl State {
    pub(crate) fn door_assembly<'a>(
        &self,
        ctx: &'a HookContext<'_>,
        element: ObjectId,
    ) -> Option<&'a AuthoredObject> {
        if self.airlock_marker_revision == Some(ctx.world.revision().0) {
            return self
                .airlock_markers
                .get(&element)
                .and_then(|id| ctx.world.authored_object(*id));
        }
        ctx.world
            .authored()
            .find(|o| element_of_door(o) == Some(element))
    }

    pub(crate) fn door_secured(&self, ctx: &HookContext<'_>, id: ObjectId) -> bool {
        self.door_assembly(ctx, id).is_some_and(|o| {
            ["bolted", "welded"]
                .iter()
                .any(|k| o.metadata.get(*k).is_some_and(|v| v == "1"))
        })
    }

    pub(crate) fn door_wires_unpowered(&self, ctx: &HookContext<'_>, id: ObjectId) -> bool {
        self.door_assembly(ctx, id).is_some_and(|o| {
            if o.metadata
                .get("wire_power_disabled")
                .is_some_and(|v| v == "1")
            {
                return true;
            }
            let mask = cut_mask(o);
            self.airlock_wires(o)
                .iter()
                .enumerate()
                .filter(|(_, w)| w.action == "PowerWireAction")
                .all(|(i, _)| mask & (1 << i) != 0)
        })
    }

    pub(super) fn airlock_wires(&self, o: &AuthoredObject) -> &[AirlockWire] {
        let layout = o
            .metadata
            .get("wire_layout")
            .map(String::as_str)
            .unwrap_or("Airlock");
        self.wire_layouts
            .get(layout)
            .unwrap_or(&self.wire_layouts["Airlock"])
    }
}
