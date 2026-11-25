//! Ordinary steel walls from the SS14 Girder construction graph on Saltern.
use super::*;
use astrocore_math::{GridCell, GridEdge};
use astrocore_world::{SlotKey, StructureElement};

pub(super) use crate::crafting::CATALOG;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WelderSpec {
    pub fuel: String,
    pub capacity: f32,
    pub initial: f32,
    pub lit_cost: f32,
    pub burn_rate: f32,
    pub temperature: f32,
    pub speed: f32,
    pub refillable: bool,
}
pub(super) static WELDERS: LazyLock<BTreeMap<String, WelderSpec>> = LazyLock::new(|| {
    serde_json::from_value(CATALOG["welders"].clone()).expect("welder definitions")
});

pub(crate) fn edges(cell: (i32, i32)) -> impl Iterator<Item = GridEdge> {
    let here = GridCell::new(cell.0, cell.1);
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .filter_map(move |(dx, dz)| {
            GridEdge::between(here, GridCell::new(cell.0 + dx, cell.1 + dz))
        })
}

impl State {
    pub(super) fn girder_at(&self, ctx: &HookContext<'_>, cell: (i32, i32)) -> Option<ObjectId> {
        self.girders.get(&cell).copied().or_else(|| {
            ctx.world.authored().find_map(|o| {
                let AuthoredKind::Placement { prototype } = &o.kind else {
                    return None;
                };
                let at = (o.min() + o.max()) * 0.5;
                (!self.gone.contains(&o.id)
                    && o.floor == FloorId(0)
                    && matches!(prototype.as_str(), "ss14:girder" | "ss14:reinforced_girder")
                    && (at.x.floor() as i32, at.z.floor() as i32) == cell)
                    .then_some(o.id)
            })
        })
    }

    pub(super) fn begin_construction(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        point: Vec3,
    ) -> bool {
        self.begin_construction_layer(ctx, actor, hand, point, None)
    }

    pub(super) fn begin_construction_layer(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        point: Vec3,
        layer: Option<u8>,
    ) -> bool {
        let Some(item) = self.actors.get(&actor).and_then(|a| a.hands.get(hand)) else {
            return false;
        };
        let item = item.clone();
        if !point.is_finite()
            || ctx
                .position_of(actor)
                .is_none_or(|feet| crate::module::reach_distance(feet, point) > self.range_m)
        {
            return true;
        }
        let cell = (point.x.floor() as i32, point.z.floor() as i32);
        let girder = self.girder_at(ctx, cell);
        let reinforced = girder.is_some() && self.material_count(&item, "Plasteel").is_some();
        if !reinforced
            && self
                .material_count(&item, &material_step(false, false).material)
                .is_none()
        {
            let walls = self.take_walls(ctx);
            self.walls = Some(walls);
            let Some(operation) = self.tool_operation(ctx, cell, &item, layer) else {
                return false;
            };
            self.begin(
                ctx,
                actor,
                Action::WorkStructure {
                    cell,
                    hand,
                    tool: item.id,
                    operation,
                },
            );
            return true;
        }
        let action = Action::Construct {
            cell,
            hand,
            stack: item.id,
            girder,
            reinforced,
        };
        self.begin(ctx, actor, action);
        true
    }

    pub(super) fn can_construct(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        cell: (i32, i32),
        hand: usize,
        stack: ObjectId,
        frame: (Option<ObjectId>, bool),
    ) -> bool {
        let (girder, reinforced) = frame;
        if !reinforced && girder.is_some_and(|id| self.is_reinforced_girder(ctx, id)) {
            return false;
        }
        let Some(a) = self.actors.get(&actor) else {
            return false;
        };
        let Some(item) = a.hands.get(hand) else {
            return false;
        };
        let Some(count) =
            self.material_count(item, &material_step(girder.is_some(), reinforced).material)
        else {
            return false;
        };
        let Some(feet) = ctx.position_of(actor) else {
            return false;
        };
        let center = Vec3::new(cell.0 as f32 + 0.5, feet.y + CHEST_M, cell.1 as f32 + 0.5);
        let floor = FloorId(0);
        if a.ghost
            || !a.health.state().can_act()
            || a.hands.active() != hand
            || item.id != stack
            || count < material_step(girder.is_some(), reinforced).amount
            || floor_at(ctx, feet.y) != Some(floor)
            || crate::module::reach_distance(feet, center) > self.range_m
            || !ctx.is_unobstructed(feet + Vec3::Y * CHEST_M, center)
            || girder.is_some_and(|id| self.loose_girders.contains(&id))
            || self.girder_at(ctx, cell) != girder
            || self.walls.as_ref().is_some_and(|w| w.contains_key(&cell))
            || ctx
                .world
                .element_at(floor, SlotKey::FloorSlab(GridCell::new(cell.0, cell.1)))
                .is_none()
        {
            return false;
        }
        if ctx.players().filter_map(|p| ctx.position_of(p)).any(|p| {
            floor_at(ctx, p.y) == Some(floor)
                && (p.x - center.x).abs() < 0.8
                && (p.z - center.z).abs() < 0.8
        }) {
            return false;
        }
        if ctx.world.authored().any(|o| {
            o.floor == floor
                && matches!(o.kind, AuthoredKind::Placement { .. })
                && Some(o.id) != girder
                && !self.gone.contains(&o.id)
                && !matches!(&o.kind, AuthoredKind::Placement { prototype } if self.info(prototype).is_some())
                && {
                    let p = (o.min() + o.max()) * 0.5;
                    (p.x.floor() as i32, p.z.floor() as i32) == cell
                }
        }) {
            return false;
        }
        !ctx.world.elements().any(|e| {
            e.floor == floor
                && matches!(e.placement,Placement::Free(p) if (p.cell.x,p.cell.z)==cell)
        }) && edges(cell).all(|edge| {
            ctx.world
                .edge_element(floor, edge)
                .is_none_or(|e| matches!(e.kind, StructureKind::Wall))
        })
    }

    pub(super) fn construct(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        cell: (i32, i32),
        hand: usize,
        stack: ObjectId,
        frame: (Option<ObjectId>, bool),
    ) {
        let (girder, reinforced) = frame;
        if !self.can_construct(ctx, actor, cell, hand, stack, frame) {
            return;
        }
        let floor = FloorId(0);
        if let Some(id) = girder.filter(|id| !reinforced || self.is_reinforced_girder(ctx, *id)) {
            ctx.request(WorldCommand::RemoveAuthored { id });
            self.girders.remove(&cell);
            self.gone.insert(id);
            let proto = if reinforced {
                "WallReinforced"
            } else {
                "WallSolid"
            };
            self.build_barrier(ctx, floor, cell, proto, wall_material(reinforced));
        } else {
            if let Some(id) = girder {
                ctx.request(WorldCommand::RemoveAuthored { id });
                self.gone.insert(id);
            }
            self.wall_serial += 1;
            let id = ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial);
            let at = Vec3::new(
                cell.0 as f32 + 0.5,
                ctx.position_of(actor).unwrap().y,
                cell.1 as f32 + 0.5,
            );
            self.place(
                ctx,
                &HeldItem {
                    id,
                    prototype: girder_prototype(reinforced),
                },
                at,
            );
            self.girders.insert(cell, id);
        }
        let count = self
            .material_count(
                self.actors[&actor].hands.get(hand).unwrap(),
                &material_step(girder.is_some(), reinforced).material,
            )
            .unwrap();
        let remaining = self.charges.entry(stack).or_insert(count);
        *remaining -= material_step(girder.is_some(), reinforced).amount;
        if *remaining == 0 {
            self.charges.remove(&stack);
            let _ = self.actor(actor).hands.drop_hand(hand);
        }
        self.publish(ctx, actor);
    }
}

fn wall_material(reinforced: bool) -> PrototypeId {
    PrototypeId::new(if reinforced {
        "ss14:wall_reinforced"
    } else {
        "ss14:wall_solid"
    })
    .unwrap()
}

fn girder_prototype(reinforced: bool) -> PrototypeId {
    PrototypeId::new(if reinforced {
        "ss14:reinforced_girder"
    } else {
        "ss14:girder"
    })
    .unwrap()
}

impl State {
    pub(crate) fn build_barrier(
        &mut self,
        ctx: &mut HookContext<'_>,
        floor: FloorId,
        cell: (i32, i32),
        proto: &str,
        material: PrototypeId,
    ) {
        let slab = ctx
            .world
            .element_at(floor, SlotKey::FloorSlab(GridCell::new(cell.0, cell.1)));
        // Уровень могли определить этим же тиком (маппинг): тогда его основание — по номеру.
        let base = ctx
            .world
            .floor(floor)
            .map_or(floor.0 as f32 * maps::LEVEL_HEIGHT_M, |f| f.base_y)
            + slab.map_or(maps::FLOOR_TOP_M, |s| s.thickness);
        for level in 0..2 {
            self.wall_serial += 1;
            ctx.request(WorldCommand::PlaceElement(StructureElement {
                id: ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial),
                kind: StructureKind::Freeform,
                placement: Placement::Free(astrocore_world::FreePlacement {
                    cell: GridCell::new(cell.0, cell.1),
                    x_mm: 500,
                    z_mm: 500,
                    y_mm: (base * 1000.0).round() as i32 + level * 1000,
                    yaw_cdeg: 0,
                    // Куб 1 × 1 × 1 м: высота ящика — две полувысоты.
                    half_mm: [500, 500, 500],
                }),
                floor,
                material: material.clone(),
                thickness: 0.1,
            }));
        }
        for edge in edges(cell)
            .filter(|edge| ctx.world.edge_element(floor, *edge).is_none())
            .collect::<Vec<_>>()
        {
            self.wall_serial += 1;
            ctx.request(WorldCommand::PlaceElement(StructureElement {
                id: ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial),
                kind: StructureKind::Wall,
                placement: Placement::Edge(edge),
                floor,
                material: material.clone(),
                thickness: 0.2,
            }));
        }
        let mut walls = self.take_walls(ctx);
        walls.insert(cell, barrier_health(proto));
        self.walls = Some(walls);
    }
}

pub(crate) fn barrier_health(proto: &str) -> crate::explosion::Wall {
    let spec = &CATALOG["barriers"][proto];
    crate::explosion::Wall {
        prototype: Some(proto.into()),
        hp: spec["hp"].as_f64().expect("barrier hp") as f32,
        coefficients: serde_json::from_value(spec["coefficients"].clone()).unwrap(),
        flat: serde_json::from_value(spec["flat"].clone()).unwrap(),
        damage: Default::default(),
    }
}

impl State {
    pub(super) fn is_reinforced_girder(&self, ctx: &HookContext<'_>, id: ObjectId) -> bool {
        ctx.world.authored_object(id).is_some_and(|o| matches!(&o.kind,AuthoredKind::Placement {prototype} if prototype.as_str()=="ss14:reinforced_girder"))
    }
}

use crate::doafter::StructureWork;
use std::sync::LazyLock;

// Read the four transitions supported by the ordinary wall from the exported graph.
#[derive(serde::Deserialize)]
pub(super) struct Step {
    #[serde(default)]
    completed: Vec<serde_json::Value>,
    #[serde(rename = "doAfter")]
    pub delay: f32,
    #[serde(default)]
    pub amount: u32,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    material: String,
}
struct Transition {
    step: Step,
    payout: u32,
}
static STEPS: LazyLock<[Transition; 6]> = LazyLock::new(|| {
    let graph = CATALOG["graphs"]["Girder"]["graph"]
        .as_array()
        .expect("Girder nodes");
    [
        ("start", "girder"),
        ("girder", "wall"),
        ("wall", "girder"),
        ("girder", "start"),
        ("girder", "reinforcedGirder"),
        ("reinforcedGirder", "girder"),
    ]
    .map(|(from, to)| {
        let node = graph
            .iter()
            .find(|n| n["node"] == from)
            .expect("source node");
        let edge = node["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["to"] == to)
            .expect("edge");
        let step = serde_json::from_value(edge["steps"][0].clone()).expect("construction step");
        let payout = edge["completed"]
            .as_array()
            .and_then(|actions| actions.iter().find(|a| a["prototype"] == "SheetSteel1"))
            .and_then(|a| a["amount"].as_u64())
            .unwrap_or(0) as u32;
        Transition { step, payout }
    })
});
pub(super) fn material_step(on_frame: bool, reinforced: bool) -> &'static Step {
    &STEPS[if reinforced { 4 } else { usize::from(on_frame) }].step
}
static REINFORCED_STEPS: LazyLock<Vec<Step>> = LazyLock::new(|| {
    let graph = CATALOG["graphs"]["Girder"]["graph"].as_array().unwrap();
    let node = graph
        .iter()
        .find(|n| n["node"] == "reinforcedWall")
        .unwrap();
    serde_json::from_value(node["edges"][0]["steps"].clone()).unwrap()
});
pub(super) fn work_delay(op: StructureWork) -> f32 {
    match op {
        StructureWork::PipeAnchor(_, _, delay) | StructureWork::CutCable(_, delay) => delay,
        StructureWork::PipeDismantle(_, _, delay) => delay,
        StructureWork::Repair(_, delay, _) => delay,
        StructureWork::StripWall(_) => STEPS[2].step.delay,
        StructureWork::StripReinforced(_, i) => REINFORCED_STEPS[i].delay,
        StructureWork::StripGirder(_) => STEPS[5].step.delay,
        StructureWork::Dismantle(_) => STEPS[3].step.delay,
        StructureWork::ToggleAnchor(..) => 1.0,
    }
}

impl State {
    fn tool_operation(
        &self,
        ctx: &HookContext<'_>,
        cell: (i32, i32),
        item: &HeldItem,
        layer: Option<u8>,
    ) -> Option<StructureWork> {
        let qualities = &self.info(&item.prototype)?.qualities;
        if let Some(operation) = self.pipe_work(ctx, cell, item, layer) {
            return Some(operation);
        }
        if let Some(id) = self.girder_at(ctx, cell) {
            let loose = self.loose_girders.contains(&id);
            let reinforced = self.is_reinforced_girder(ctx, id);
            if qualities.contains("Anchoring") {
                return Some(StructureWork::ToggleAnchor(id, loose));
            }
            if reinforced && !loose && qualities.contains(&STEPS[5].step.tool) {
                return Some(StructureWork::StripGirder(id));
            }
            if !reinforced && loose && qualities.contains(&STEPS[3].step.tool) {
                return Some(StructureWork::Dismantle(id));
            }
            return None;
        }
        let wall = self.walls.as_ref()?.get(&cell)?;
        let prototype = wall.prototype.as_deref()?;
        let edge = edges(cell)
            .find_map(|e| ctx.world.edge_element(FloorId(0), e))?
            .id;
        let repair = &CATALOG["repairables"][prototype];
        let (quality, fuel, operation) = if repair.is_object() && wall.total_damage() > 0. {
            let fuel = repair["fuelCost"].as_f64().unwrap_or(5.) as f32;
            let delay = repair["doAfterDelay"].as_f64().unwrap_or(1.) as f32;
            (
                repair["qualityNeeded"].as_str().unwrap_or("Welding"),
                fuel,
                StructureWork::Repair(edge, delay, fuel),
            )
        } else if matches!(prototype, "WallReinforced" | "WallReinforcedChitin") {
            let i = self.reinforced_work.get(&cell).copied().unwrap_or(0);
            (
                REINFORCED_STEPS[i].tool.as_str(),
                10.,
                StructureWork::StripReinforced(edge, i),
            )
        } else if matches!(prototype, "WallSolid" | "WallSolidChitin") {
            (
                STEPS[2].step.tool.as_str(),
                10.,
                StructureWork::StripWall(edge),
            )
        } else {
            return None;
        };
        if !qualities.contains(quality) {
            return None;
        }
        if let Some(spec) = self.welding_tools.get(&item.prototype) {
            let solution = self
                .solutions
                .get(&item.id)
                .or_else(|| self.solution_templates.get(&item.prototype))?;
            if !self.lit.contains(&item.id) || solution.amount(&spec.fuel) < fuel {
                return None;
            }
        }
        Some(operation)
    }
    pub(super) fn work_valid(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        cell: (i32, i32),
        hand: usize,
        tool: ObjectId,
        op: StructureWork,
    ) -> bool {
        let Some(a) = self.actors.get(&actor) else {
            return false;
        };
        let Some(item) = a.hands.get(hand) else {
            return false;
        };
        let Some(feet) = ctx.position_of(actor) else {
            return false;
        };
        if !a.health.state().can_act()
            || a.ghost
            || a.hands.active() != hand
            || tool != item.id
            || floor_at(ctx, feet.y) != Some(FloorId(0))
            || !match (
                self.tool_operation(
                    ctx,
                    cell,
                    item,
                    match op {
                        StructureWork::PipeAnchor(id, ..)
                        | StructureWork::PipeDismantle(id, ..) => {
                            ctx.world.authored_object(id).map(crate::piping::pipe_layer)
                        }
                        _ => None,
                    },
                ),
                op,
            ) {
                (
                    Some(StructureWork::PipeAnchor(a, anchored, _)),
                    StructureWork::PipeAnchor(b, expected, _),
                ) => a == b && anchored == expected,
                (found, expected) => found == Some(expected),
            }
        {
            return false;
        }
        let chest = feet + Vec3::Y * CHEST_M;
        let center = Vec3::new(cell.0 as f32 + 0.5, chest.y, cell.1 as f32 + 0.5);
        let target = if matches!(
            op,
            StructureWork::StripWall(_)
                | StructureWork::StripReinforced(..)
                | StructureWork::Repair(..)
        ) {
            // Stop just outside the near face of the selected wall.
            let d = chest - center;
            center + d * (0.61 / d.x.abs().max(d.z.abs()).max(0.61))
        } else {
            center
        };
        crate::module::reach_distance(chest - Vec3::Y * crate::module::CHEST_M, target)
            <= self.range_m
            && ctx.is_unobstructed(chest, target)
    }

    pub(super) fn finish_work(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        cell: (i32, i32),
        hand: usize,
        tool: ObjectId,
        op: StructureWork,
    ) {
        if !self.work_valid(ctx, actor, cell, hand, tool, op) {
            return;
        }
        let feet = ctx.position_of(actor).unwrap();
        let at = Vec3::new(cell.0 as f32 + 0.5, feet.y, cell.1 as f32 + 0.5);
        let mut payout = "SheetSteel1";
        if let Some(used) = self.used_item(actor) {
            self.tool_sound(&used.item.prototype, "use", feet);
        }
        let (count, give) = match op {
            StructureWork::CutCable(id, _) => {
                self.cut_cable(ctx, actor, id, at);
                return;
            }
            StructureWork::PipeAnchor(id, loose, _) => {
                self.toggle_pipe(ctx, id, loose);
                return;
            }
            StructureWork::PipeDismantle(id, index, _) => {
                let mut pipe = ctx.world.authored_object(id).unwrap().clone();
                let edge = self.pipe_recipe(&pipe).unwrap().reverse;
                if let Some(spec) = self
                    .welding_tools
                    .get(&self.actors[&actor].hands.get(hand).unwrap().prototype)
                    .copied()
                {
                    let item = self.actors[&actor].hands.get(hand).unwrap().clone();
                    self.solution_mut(&item).unwrap().remove(&spec.fuel, 10.);
                }
                if index + 1 < edge["steps"].as_array().unwrap().len() {
                    pipe.metadata
                        .insert("pipe_step".into(), (index + 1).to_string());
                    ctx.request(WorldCommand::RemoveAuthored { id });
                    ctx.request(WorldCommand::PlaceAuthored(pipe));
                    return;
                }
                let drop = edge["completed"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|step| step["__tag"] == "SpawnPrototype")
                    .unwrap();
                payout = drop["prototype"].as_str().unwrap();
                self.gone.insert(id);
                ctx.request(WorldCommand::RemoveAuthored { id });
                (drop["amount"].as_u64().unwrap_or(1) as u32, false)
            }
            StructureWork::Repair(_, _, fuel) => {
                let item = self.actors[&actor].hands.get(hand).unwrap().clone();
                if let Some(spec) = self.welding_tools.get(&item.prototype).copied() {
                    self.solution_mut(&item).unwrap().remove(&spec.fuel, fuel);
                }
                let wall = self.walls.as_mut().unwrap().get_mut(&cell).unwrap();
                let spec = &CATALOG["repairables"][wall.prototype.as_deref().unwrap()];
                wall.repair(spec);
                if wall.total_damage() > 0. && spec["autoDoAfter"] != false {
                    self.begin(
                        ctx,
                        actor,
                        Action::WorkStructure {
                            cell,
                            hand,
                            tool,
                            operation: op,
                        },
                    );
                    return;
                }
                self.popup(actor, "Ремонт завершён");
                self.publish(ctx, actor);
                return;
            }
            StructureWork::ToggleAnchor(id, loose) => {
                if loose {
                    self.loose_girders.remove(&id);
                } else {
                    self.loose_girders.insert(id);
                }
                self.popup(
                    actor,
                    if loose {
                        "Каркас закреплён"
                    } else {
                        "Каркас откреплён"
                    },
                );
                return;
            }
            StructureWork::StripGirder(id) => {
                ctx.request(WorldCommand::RemoveAuthored { id });
                self.gone.insert(id);
                self.wall_serial += 1;
                let frame = HeldItem {
                    id: ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial),
                    prototype: girder_prototype(false),
                };
                self.place(ctx, &frame, at);
                self.girders.insert(cell, frame.id);
                payout = "SheetPlasteel1";
                (2, false)
            }
            StructureWork::StripWall(_) | StructureWork::StripReinforced(..) => {
                let item = self.actors[&actor].hands.get(hand).unwrap().clone();
                if let Some(spec) = self.welding_tools.get(&item.prototype).copied() {
                    self.solution_mut(&item).unwrap().remove(&spec.fuel, 10.);
                }
                let reinforced = matches!(op, StructureWork::StripReinforced(..));
                if let StructureWork::StripReinforced(_, i) = op {
                    if i + 1 < REINFORCED_STEPS.len() {
                        self.reinforced_work.insert(cell, i + 1);
                        if let Some(stage) = REINFORCED_STEPS[i]
                            .completed
                            .iter()
                            .find(|a| a["__tag"] == "VisualizerDataInt")
                            .and_then(|a| a["data"].as_u64())
                        {
                            let material =
                                PrototypeId::new(format!("ss14:wall_reinforced_stage{stage}"))
                                    .unwrap();
                            let changes:Vec<_>=ctx.world.elements().filter(|e|e.floor==FloorId(0) && matches!(e.placement,Placement::Free(p) if (p.cell.x,p.cell.z)==cell)).cloned().collect();
                            for mut element in changes {
                                element.material = material.clone();
                                ctx.request(WorldCommand::RemoveElement { id: element.id });
                                ctx.request(WorldCommand::PlaceElement(element));
                            }
                        }
                        self.publish(ctx, actor);
                        return;
                    }
                    self.reinforced_work.remove(&cell);
                    payout = "SheetPlasteel1";
                }
                let mut walls = self.take_walls(ctx);
                walls.remove(&cell);
                self.open_wall_geometry(ctx, FloorId(0), cell, &walls);
                self.walls = Some(walls);
                self.wall_serial += 1;
                let id = ObjectId::from_u128(0xB1A5_7000_0000 + self.wall_serial);
                self.place(
                    ctx,
                    &HeldItem {
                        id,
                        prototype: girder_prototype(reinforced),
                    },
                    at,
                );
                self.girders.insert(cell, id);
                (STEPS[2].payout, !reinforced)
            }
            StructureWork::Dismantle(id) => {
                ctx.request(WorldCommand::RemoveAuthored { id });
                self.girders.remove(&cell);
                self.loose_girders.remove(&id);
                self.gone.insert(id);
                (STEPS[3].payout, false)
            }
        };
        if let Some(stack) = self.create_item(payout) {
            self.charges.insert(stack.id, count);
            if !give || self.actor(actor).hands.pickup_any(stack.clone()).is_err() {
                self.place(ctx, &stack, if give { feet } else { at });
            }
        }
        self.publish(ctx, actor);
    }
}
