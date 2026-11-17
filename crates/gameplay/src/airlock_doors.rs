use super::airlock_wires::{flag, number};
use super::*;
use astrocore_physics::{CharacterShape, CharacterState, aabb_overlap};

#[derive(Clone)]
pub(super) struct Motion {
    opening: bool,
    started: f32,
    pass_at: f32,
    finish_at: f32,
    animation: f32,
    applied: bool,
    auto_close: bool,
    crushed: bool,
}

impl State {
    pub(super) fn sync_airlock_index(&mut self, ctx: &HookContext<'_>) {
        let revision = ctx.world.revision().0;
        if self.airlock_marker_revision == Some(revision) {
            return;
        }
        self.airlock_markers = ctx
            .world
            .authored()
            .filter_map(|object| {
                let element =
                    u128::from_str_radix(object.metadata.get("door_element")?, 16).ok()?;
                Some((ObjectId::from_u128(element), object.id))
            })
            .collect();
        self.airlock_marker_revision = Some(revision);
    }

    fn prying_spec(&self, tool: &PrototypeId) -> &'static serde_json::Value {
        let name = self.ss14_names.get(tool).map(String::as_str).unwrap_or("");
        &construction::CATALOG["prying"][name]
    }

    pub(crate) fn can_pry_door(
        &self,
        ctx: &HookContext<'_>,
        element: ObjectId,
        tool: &PrototypeId,
    ) -> bool {
        let prying = self.prying_spec(tool);
        if prying.is_null() || prying["enabled"].as_bool() == Some(false) {
            return false;
        }
        let Some(object) = self.door_assembly(ctx, element) else {
            return ctx.door_open(element).is_some();
        };
        let spec = self.airlock_spec(object);
        !flag(object, "welded", false)
            && spec["Door"]["canPry"].as_bool() != Some(false)
            && (!flag(object, "bolted", false) || prying["force"].as_bool() == Some(true))
            && (!self.airlock_powered(ctx, element) || prying["pryPowered"].as_bool() == Some(true))
    }

    pub(super) fn pry_delay(
        &self,
        ctx: &HookContext<'_>,
        element: ObjectId,
        tool: &PrototypeId,
    ) -> f32 {
        let mut delay = crate::doafter::PRY_TIME_S;
        if let Some(object) = self.door_assembly(ctx, element) {
            let spec = self.airlock_spec(object);
            delay = spec["Door"]["pryTime"].as_f64().unwrap_or(1.5) as f32;
            if self.airlock_powered(ctx, element) {
                delay *= spec["Airlock"]["poweredPryModifier"].as_f64().unwrap_or(9.) as f32;
            }
            if flag(object, "bolted", false) {
                delay *= spec["Airlock"]["boltedPryModifier"].as_f64().unwrap_or(3.) as f32;
            }
        }
        delay
            / self.prying_spec(tool)["speedModifier"]
                .as_f64()
                .unwrap_or(1.) as f32
    }

    pub(super) fn finish_pry(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        element: ObjectId,
        hand: usize,
    ) {
        if let Some(tool) = self.actors.get(&actor).and_then(|a| a.hands.get(hand)) {
            let spec = self.prying_spec(&tool.prototype);
            if let Some(at) = ctx.position_of(actor) {
                self.engineering_sound(&spec["useSound"], at);
            }
        }
        if let Some(object) = self.door_assembly(ctx, element).cloned() {
            if !self.airlock_moving(element) {
                self.start_airlock_motion(
                    element,
                    &object,
                    ctx.door_open(element) != Some(true),
                    true,
                );
            }
        } else {
            ctx.request_door(element, DoorAction::Toggle, true);
        }
    }

    pub(crate) fn airlock_wants_open(&self, ctx: &HookContext<'_>, element: ObjectId) -> bool {
        self.airlock_motions
            .get(&element)
            .map_or_else(|| ctx.door_open(element) == Some(true), |m| m.opening)
    }

    pub(super) fn airlock_moving(&self, element: ObjectId) -> bool {
        self.airlock_motions.contains_key(&element)
    }

    pub(crate) fn open_airlock(
        &mut self,
        element: ObjectId,
        object: &AuthoredObject,
        auto_close: bool,
    ) {
        if auto_close {
            self.airlock_held_open.remove(&element);
        } else {
            self.airlock_held_open.insert(element);
        }
        self.start_airlock_motion(element, object, true, auto_close);
    }

    fn start_airlock_motion(
        &mut self,
        element: ObjectId,
        object: &AuthoredObject,
        opening: bool,
        auto_close: bool,
    ) {
        if self
            .airlock_motions
            .get(&element)
            .is_some_and(|m| m.opening == opening)
        {
            return;
        }
        let door = &self.airlock_spec(object)["Door"];
        let (one, two, animation) = if opening {
            ("openTimeOne", "openTimeTwo", "openingAnimationTime")
        } else {
            ("closeTimeOne", "closeTimeTwo", "closingAnimationTime")
        };
        let first = door[one].as_f64().unwrap_or(0.4) as f32;
        let total = first + door[two].as_f64().unwrap_or(0.2) as f32;
        self.airlock_close_at.remove(&element);
        self.airlock_motions.insert(
            element,
            Motion {
                opening,
                started: self.clock,
                pass_at: self.clock + first,
                finish_at: self.clock + total,
                animation: door[animation].as_f64().unwrap_or(0.6) as f32,
                applied: false,
                auto_close,
                crushed: false,
            },
        );
        let sound = if opening { "openSound" } else { "closeSound" };
        let at = (object.min() + object.max()) * 0.5;
        let mut sound = door[sound].clone();
        if sound.is_object() {
            sound["volume"] = (sound["volume"].as_f64().unwrap_or(0.) - 5.).into();
            self.engineering_sound(&sound, at);
        }
    }

    fn airlock_occupants(&self, ctx: &HookContext<'_>, element: ObjectId) -> Vec<NetworkEntityId> {
        let Some(e) = ctx.world.element(element) else {
            return Vec::new();
        };
        let Placement::Edge(edge) = e.placement else {
            return Vec::new();
        };
        let Some(floor) = ctx.world.floor(e.floor) else {
            return Vec::new();
        };
        let (a, b) = edge.endpoints(ctx.world.config().grid_step_m, floor.base_y);
        let across = Vec3::Y.cross((b - a).normalize_or_zero()).abs() * e.thickness * 0.5;
        let bounds = astrocore_math::Aabb {
            min: a.min(b) - across,
            max: a.max(b) + across + Vec3::Y * floor.height,
        };
        let shape = CharacterShape::default();
        ctx.players()
            .filter(|id| {
                self.actors.get(id).is_some_and(|actor| !actor.ghost)
                    && ctx.position_of(*id).is_some_and(|at| {
                        aabb_overlap(&bounds, &CharacterState::at(at).aabb(&shape))
                    })
            })
            .collect()
    }

    pub(crate) fn close_airlock(
        &mut self,
        ctx: &mut HookContext<'_>,
        element: ObjectId,
        object: &AuthoredObject,
    ) -> bool {
        if flag(object, "safety", true) && !self.airlock_occupants(ctx, element).is_empty() {
            return false;
        }
        self.start_airlock_motion(element, object, false, false);
        true
    }

    fn schedule_airlock_close(&mut self, object: &AuthoredObject, element: ObjectId) {
        if self.airlock_held_open.contains(&element)
            || self.airlock_spec(object)["Airlock"]["autoClose"].as_bool() == Some(false)
        {
            return;
        }
        let delay = self.airlock_spec(object)["Airlock"]["autoCloseDelay"]
            .as_f64()
            .unwrap_or(5.) as f32;
        let modifier = number(object, "close_modifier", 1.);
        if modifier > 0. {
            self.airlock_close_at
                .insert(element, self.clock + delay * modifier);
        }
    }

    pub(super) fn tick_airlock_doors(&mut self, ctx: &mut HookContext<'_>) {
        let doors: Vec<_> = ctx
            .world
            .authored()
            .filter_map(|object| {
                let element =
                    u128::from_str_radix(object.metadata.get("door_element")?, 16).ok()?;
                Some((ObjectId::from_u128(element), object.clone()))
            })
            .collect();
        let existing: BTreeSet<_> = doors.iter().map(|(element, _)| *element).collect();
        self.airlock_availability
            .retain(|element, _| existing.contains(element));
        self.airlock_held_open
            .retain(|element| existing.contains(element));
        self.airlock_close_at
            .retain(|element, _| existing.contains(element));
        for (element, object) in doors {
            let powered = self.airlock_powered(ctx, element);
            let bolted = self.door_secured(ctx, element);
            let previous = self.airlock_availability.insert(element, (powered, bolted));
            if ctx.door_open(element) != Some(true) || self.airlock_moving(element) {
                continue;
            }
            if !powered {
                self.airlock_close_at.remove(&element);
            } else if !bolted
                && previous.is_some_and(|(was_powered, was_bolted)| !was_powered || was_bolted)
            {
                self.schedule_airlock_close(&object, element);
            }
        }
        let motions: Vec<_> = self
            .airlock_motions
            .iter()
            .map(|(element, motion)| (*element, motion.clone()))
            .collect();
        for (element, mut motion) in motions {
            let Some(object) = self.door_assembly(ctx, element).cloned() else {
                self.airlock_motions.remove(&element);
                continue;
            };
            if !motion.applied && self.clock >= motion.pass_at {
                if !motion.opening {
                    let people = self.airlock_occupants(ctx, element);
                    if self.door_secured(ctx, element)
                        || (flag(&object, "safety", true) && !people.is_empty())
                    {
                        self.airlock_motions.remove(&element);
                        let retry = self.airlock_spec(&object)["Door"]["openTimeTwo"]
                            .as_f64()
                            .unwrap_or(0.2) as f32;
                        self.airlock_close_at.insert(element, self.clock + retry);
                        continue;
                    }
                    let spec = &self.airlock_spec(&object)["Door"];
                    if !people.is_empty() && spec["canCrush"].as_bool().unwrap_or(true) {
                        let damage: DamageSpec =
                            serde_json::from_value(spec["crushDamage"]["types"].clone())
                                .unwrap_or_default();
                        let hold = spec["doorStunTime"].as_f64().unwrap_or(2.) as f32;
                        let until =
                            self.clock + hold + spec["openTimeOne"].as_f64().unwrap_or(0.4) as f32;
                        for id in people {
                            let actor = self.actor(id);
                            actor.health.apply(&damage);
                            actor.stun_until = actor.stun_until.max(until);
                            self.after_health_change(ctx, id);
                        }
                        motion.crushed = true;
                        motion.finish_at = self.clock + hold;
                    }
                }
                ctx.request_door(
                    element,
                    if motion.opening {
                        DoorAction::Open
                    } else {
                        DoorAction::Close
                    },
                    true,
                );
                motion.applied = true;
            }
            if self.clock >= motion.finish_at {
                self.airlock_motions.remove(&element);
                if motion.crushed {
                    self.start_airlock_motion(element, &object, true, true);
                } else if motion.opening
                    && motion.auto_close
                    && self.airlock_powered(ctx, element)
                    && !self.door_secured(ctx, element)
                {
                    self.schedule_airlock_close(&object, element);
                } else if !motion.opening {
                    self.airlock_held_open.remove(&element);
                }
            } else {
                self.airlock_motions.insert(element, motion);
            }
        }
        let due: Vec<_> = self
            .airlock_close_at
            .iter()
            .filter(|(_, time)| **time <= self.clock)
            .map(|(id, _)| *id)
            .collect();
        for element in due {
            let Some(object) = self.door_assembly(ctx, element).cloned() else {
                self.airlock_close_at.remove(&element);
                continue;
            };
            if ctx.door_open(element) != Some(true) {
                self.airlock_close_at.remove(&element);
                continue;
            }
            if !self.airlock_powered(ctx, element) {
                self.airlock_close_at.remove(&element);
            } else if self.door_secured(ctx, element) || !self.close_airlock(ctx, element, &object)
            {
                self.airlock_close_at.insert(element, self.clock + 1.);
            }
        }
        let view: Vec<_> = self
            .airlock_motions
            .iter()
            .map(|(element, motion)| crate::wires::Motion {
                element: *element,
                opening: motion.opening,
                elapsed: self.clock - motion.started,
                duration: motion.animation,
            })
            .collect();
        let bytes = crate::wires::encode_motions(&view);
        let changed = self.airlock_motion_sent != bytes;
        let players: BTreeSet<_> = ctx.players().collect();
        for id in &players {
            if changed || !self.airlock_motion_to.contains(id) {
                ctx.set_component(*id, crate::wires::MOTION_COMPONENT, bytes.clone());
            }
        }
        self.airlock_motion_sent = bytes;
        self.airlock_motion_to = players;
    }
}
