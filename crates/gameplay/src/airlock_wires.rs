use super::computer_construction::{AirlockWire, cut_mask};
use super::*;

pub(super) fn flag(object: &AuthoredObject, key: &str, default: bool) -> bool {
    object
        .metadata
        .get(key)
        .map_or(default, |value| value == "1")
}

pub(super) fn number(object: &AuthoredObject, key: &str, default: f32) -> f32 {
    object
        .metadata
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

impl State {
    pub(super) fn tick_airlock_wires(&mut self, ctx: &mut HookContext<'_>) {
        let powered = self.powered_map();
        let doors: Vec<_> = ctx
            .world
            .authored()
            .filter(|o| o.metadata.contains_key("door_element"))
            .cloned()
            .collect();
        for mut object in doors {
            let Some(element) = object
                .metadata
                .get("door_element")
                .and_then(|value| u128::from_str_radix(value, 16).ok())
                .map(ObjectId::from_u128)
            else {
                continue;
            };
            let original = object.metadata.clone();
            let mask = cut_mask(&object);
            let power_mask = self
                .airlock_wires(&object)
                .iter()
                .enumerate()
                .filter(|(_, w)| w.action == "PowerWireAction")
                .fold(0u32, |bits, (i, _)| bits | 1 << i);
            if object.metadata.contains_key("wire_power_until")
                && number(&object, "wire_power_until", f32::INFINITY) <= self.clock
            {
                object.metadata.remove("wire_power_until");
                object.metadata.remove("wire_power_pulsed");
                object.metadata.remove("wire_power_disabled");
            }
            let on = (!self.power_required || powered.get(&element).copied().unwrap_or(false))
                && mask & power_mask != power_mask
                && !flag(&object, "wire_power_disabled", false);
            for (action, timer, field, value) in [
                (
                    "DoorTimingWireAction",
                    "wire_timing_until",
                    "close_modifier",
                    "1",
                ),
                ("DoorSafetyWireAction", "wire_safety_until", "safety", "1"),
                ("LogWireAction", "wire_logging_until", "logging", "1"),
            ] {
                if !object.metadata.contains_key(timer) {
                    continue;
                }
                if !on {
                    object.metadata.remove(timer);
                    continue;
                }
                if number(&object, timer, f32::INFINITY) <= self.clock {
                    let cut = self
                        .airlock_wires(&object)
                        .iter()
                        .enumerate()
                        .any(|(i, w)| w.action == action && mask & (1 << i) != 0);
                    if !cut {
                        object.metadata.insert(field.into(), value.into());
                    }
                    object.metadata.remove(timer);
                }
            }
            if mask & power_mask == 0
                || (mask & power_mask == power_mask && !flag(&object, "wire_power_pulsed", false))
            {
                object.metadata.remove("electrified");
            }
            if object.metadata != original {
                ctx.request(WorldCommand::RemoveAuthored { id: object.id });
                ctx.request(WorldCommand::PlaceAuthored(object.clone()));
            }
        }
        self.tick_airlock_doors(ctx);
        self.refresh_wires(ctx);
    }

    pub(crate) fn airlock_spec(&self, object: &AuthoredObject) -> &'static serde_json::Value {
        let proto = object
            .metadata
            .get("airlock_proto")
            .map(String::as_str)
            .unwrap_or("Airlock");
        &construction::CATALOG["airlockSettings"][proto]
    }

    pub(crate) fn airlock_powered(&self, ctx: &HookContext<'_>, element: ObjectId) -> bool {
        if self.door_assembly(ctx, element).is_none() {
            return true;
        }
        !self.door_wires_unpowered(ctx, element)
            && (!self.power_required
                || self
                    .power_receivers
                    .iter()
                    .find(|(id, _, _)| *id == element)
                    .is_some_and(|(_, index, want)| {
                        self.power
                            .loads
                            .get(*index)
                            .is_some_and(|load| load.receiving_power >= want * 0.999)
                    }))
    }

    fn wire_timeout(&self, wire: &AirlockWire) -> f32 {
        let field = if matches!(wire.action, "DoorTimingWireAction" | "DoorSafetyWireAction") {
            "timeout"
        } else {
            "pulseTimeout"
        };
        wire.spec
            .and_then(|spec| spec[field].as_f64())
            .unwrap_or(30.) as f32
    }

    pub(crate) fn shock_from_airlock(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        element: ObjectId,
    ) -> bool {
        let Some(object) = self.door_assembly(ctx, element).cloned() else {
            return false;
        };
        if !flag(&object, "electrified", false) {
            return false;
        }
        self.apply_airlock_shock(ctx, actor, element, &object)
    }

    fn apply_airlock_shock(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        element: ObjectId,
        object: &AuthoredObject,
    ) -> bool {
        if !self.airlock_powered(ctx, element) {
            return false;
        }
        let proto = object
            .metadata
            .get("airlock_proto")
            .map(String::as_str)
            .unwrap_or("Airlock");
        let spec = &construction::CATALOG["electrified"][proto];
        let equipment: f32 = self.actors[&actor]
            .inventory
            .iter()
            .filter(|(slot, _)| slot.flag != "pocket")
            .filter_map(|(_, item)| self.ss14_names.get(&item?.prototype))
            .filter_map(|name| construction::CATALOG["insulation"][name].as_f64())
            .map(|n| n as f32)
            .product();
        let tool = self
            .used_item(actor)
            .and_then(|used| self.ss14_names.get(&used.item.prototype))
            .and_then(|name| construction::CATALOG["insulation"][name].as_f64())
            .unwrap_or(1.) as f32;
        let coefficient = spec["siemensCoefficient"].as_f64().unwrap_or(1.) as f32;
        if equipment * tool * coefficient <= 0. {
            return false;
        }
        if self.spawn_rng.next_f32() >= spec["probability"].as_f64().unwrap_or(1.) as f32 {
            return false;
        }
        let base_damage = spec["shockDamage"].as_f64().unwrap_or(7.5) as i32;
        let damage = (base_damage as f32 * equipment * coefficient).trunc();
        if damage <= 0. {
            return false;
        }
        let duration = spec["shockTime"].as_f64().unwrap_or(5.) as f32;
        let until = self.clock + duration;
        let person = self.actor(actor);
        person.health.apply(&[("Shock".into(), damage)].into());
        if equipment * coefficient > 0.5 {
            person.stun_until = person.stun_until.max(until);
        }
        if let Some(body) = &mut person.body {
            body.modify_status(
                "Stutter",
                crate::chem::StatusOp::Update,
                Some(duration * 1.5),
                0.,
                1.,
            );
            body.modify_status(
                "Jitter",
                crate::chem::StatusOp::Update,
                Some(duration * 0.75),
                0.,
                1.,
            );
        }
        self.popup(actor, "Вас ударило током!");
        if spec["playSoundOnShock"].as_bool().unwrap_or(true) {
            let mut sound = spec["shockNoises"].clone();
            if sound.is_object() {
                sound["volume"] = (sound["volume"].as_f64().unwrap_or(0.)
                    + spec["shockVolume"].as_f64().unwrap_or(20.))
                .into();
                self.engineering_sound(&sound, ctx.position_of(actor).unwrap());
            }
        }
        self.after_health_change(ctx, actor);
        true
    }

    fn power_wire_shock(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        element: ObjectId,
        object: &mut AuthoredObject,
    ) -> bool {
        object.metadata.insert("electrified".into(), "1".into());
        // The source enables electrification before attempting the shock, including a previously safe door.
        self.apply_airlock_shock(ctx, actor, element, object)
    }

    pub(super) fn attempt_airlock_electrocution(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        element: ObjectId,
        object: &AuthoredObject,
    ) {
        self.apply_airlock_shock(ctx, actor, element, object);
    }

    fn wire_reach<'a>(
        &self,
        ctx: &'a HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) -> Option<&'a AuthoredObject> {
        let o = self.door_assembly(ctx, id)?;
        if let Some(current) = o.metadata.get("construction_node")
            && construction::CATALOG["graphs"]["Airlock"]["graph"]
                .as_array()?
                .iter()
                .find(|node| node["node"] == current.as_str())?["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|action| {
                    action["__tag"] == "SetWiresPanelSecurity" && action["wiresAccessible"] == false
                })
        {
            return None;
        }
        let feet = ctx.position_of(actor)?;
        let chest = feet + Vec3::Y * CHEST_M;
        let spot = (o.min() + o.max()) * 0.5;
        let who = self.actors.get(&actor)?;
        if who.ghost
            || !who.health.state().can_act()
            || o.metadata.get("panel_open")?.as_str() != "1"
            || floor_at(ctx, feet.y) != Some(o.floor)
            || chest.distance(spot) > self.range_m
            || ctx.unobstructed_distance(chest, spot) + 0.6 < chest.distance(spot)
        {
            return None;
        }
        Some(o)
    }

    fn wire_target<'a>(
        &self,
        ctx: &'a HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) -> Option<&'a AuthoredObject> {
        let used = self.used_item(actor)?;
        if !self
            .info(&used.item.prototype)?
            .qualities
            .iter()
            .any(|q| matches!(q.as_str(), "Cutting" | "Pulsing"))
        {
            return None;
        }
        self.wire_reach(ctx, actor, id)
    }

    pub(super) fn wires_accessible(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) -> bool {
        self.wire_reach(ctx, actor, id).is_some()
    }

    pub(super) fn open_wires(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) -> bool {
        if self.wire_reach(ctx, actor, id).is_none() {
            return false;
        }
        if !self.wire_serials.contains_key(&id) {
            let cyrillic = self.spawn_rng.next_f32() < 0.01;
            let mut serial = String::new();
            for _ in 0..4 {
                let (base, len) = if cyrillic { (0x0410, 32) } else { (0x41, 26) };
                serial.push(
                    char::from_u32(base + (self.spawn_rng.next_f32() * len as f32) as u32).unwrap(),
                );
            }
            serial.push('-');
            for _ in 0..4 {
                serial.push(char::from(b'0' + (self.spawn_rng.next_f32() * 10.) as u8));
            }
            self.wire_serials.insert(id, serial);
            let count = self
                .door_assembly(ctx, id)
                .map_or(0, |o| self.airlock_wires(o).len());
            let shapes = (0..count)
                .map(|_| {
                    let mirror = u8::from(self.spawn_rng.next_f32() < 0.5);
                    let flip = u8::from(self.spawn_rng.next_f32() < 0.5);
                    let variant = u8::from(self.spawn_rng.next_f32() >= 0.5);
                    mirror | flip << 1 | variant << 2
                })
                .collect();
            self.wire_shapes.insert(id, shapes);
        }
        self.wires_open.insert(actor, (id, Vec::new()));
        self.refresh_wires(ctx);
        true
    }

    pub(super) fn close_wires(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        self.wires_open.remove(&actor);
        ctx.set_component(actor, crate::wires::COMPONENT, Vec::new());
    }

    fn wire_view(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) -> Option<crate::wires::View> {
        use crate::wires::{Light, LightState, View, Wire};
        let object = self.wire_reach(ctx, actor, id)?;
        let mask = cut_mask(object);
        let mut lights = Vec::new();
        let mut power_seen = false;
        for wire in self.airlock_wires(object) {
            let (text, color, on) = match wire.action {
                "PowerWireAction" if !power_seen => {
                    power_seen = true;
                    let cuts = self
                        .airlock_wires(object)
                        .iter()
                        .enumerate()
                        .any(|(i, w)| w.action == "PowerWireAction" && mask & (1 << i) != 0);
                    let state = if cuts || flag(object, "wire_power_pulsed", false) {
                        LightState::BlinkingSlow
                    } else {
                        LightState::On
                    };
                    lights.push((
                        wire.original_position,
                        Light {
                            text: "Power".into(),
                            color: "Red".into(),
                            state,
                        },
                    ));
                    continue;
                }
                "DoorBoltWireAction" => ("Bolts", "Red", flag(object, "bolted", false)),
                "DoorBoltLightWireAction" => {
                    ("Bolt lights", "Lime", flag(object, "bolt_lights", true))
                }
                "DoorTimingWireAction" => {
                    let modifier = number(object, "close_modifier", 1.);
                    let state = if modifier == 0.01 {
                        LightState::Off
                    } else if modifier <= 0.5 {
                        LightState::BlinkingSlow
                    } else {
                        LightState::On
                    };
                    lights.push((
                        wire.original_position,
                        Light {
                            text: "Timing".into(),
                            color: "Orange".into(),
                            state,
                        },
                    ));
                    continue;
                }
                "DoorSafetyWireAction" => ("Safety", "Red", flag(object, "safety", true)),
                "LogWireAction" => ("Access logging", "Blue", flag(object, "logging", true)),
                "AiInteractWireAction" => (
                    "AI control",
                    "DeepSkyBlue",
                    flag(object, "ai_control", true),
                ),
                _ => continue,
            };
            lights.push((
                wire.original_position,
                Light {
                    text: text.into(),
                    color: color.into(),
                    state: if on { LightState::On } else { LightState::Off },
                },
            ));
        }
        lights.sort_by_key(|(position, _)| *position);
        let tool = self
            .used_item(actor)
            .and_then(|u| self.info(&u.item.prototype));
        Some(View {
            element: id,
            name: "Airlock Control".into(),
            serial: self.wire_serials.get(&id).cloned().unwrap_or_default(),
            wires: self
                .airlock_wires(object)
                .iter()
                .enumerate()
                .map(|(i, w)| Wire {
                    id: i as u8,
                    letter: w.letter,
                    color: w.color.into(),
                    cut: mask & (1 << i) != 0,
                    variant: self
                        .wire_shapes
                        .get(&id)
                        .and_then(|s| s.get(i))
                        .copied()
                        .unwrap_or(0)
                        >> 2,
                    mirror: self
                        .wire_shapes
                        .get(&id)
                        .and_then(|s| s.get(i))
                        .copied()
                        .unwrap_or(0)
                        & 1
                        != 0,
                    flip: self
                        .wire_shapes
                        .get(&id)
                        .and_then(|s| s.get(i))
                        .copied()
                        .unwrap_or(0)
                        & 2
                        != 0,
                })
                .collect(),
            lights: lights.into_iter().map(|(_, light)| light).collect(),
            can_cut: tool.is_some_and(|t| t.qualities.contains("Cutting")),
            can_pulse: tool.is_some_and(|t| t.qualities.contains("Pulsing")),
        })
    }

    fn refresh_wires(&mut self, ctx: &mut HookContext<'_>) {
        let open: Vec<_> = self
            .wires_open
            .iter()
            .map(|(actor, (element, _))| (*actor, *element))
            .collect();
        for (actor, element) in open {
            let Some(view) = self.wire_view(ctx, actor, element) else {
                self.close_wires(ctx, actor);
                continue;
            };
            let bytes = serde_json::to_vec(&view).unwrap();
            let (_, last) = self.wires_open.get_mut(&actor).unwrap();
            if *last != bytes {
                *last = bytes.clone();
                ctx.set_component(actor, crate::wires::COMPONENT, bytes);
            }
        }
    }

    pub(super) fn airlock_wire_verbs(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) -> Vec<crate::verbs::Verb> {
        let Some(o) = self.wire_target(ctx, actor, id) else {
            return vec![];
        };
        let mask = cut_mask(o);
        let pulse = self
            .used_item(actor)
            .and_then(|used| self.info(&used.item.prototype))
            .is_some_and(|info| info.qualities.contains("Pulsing"));
        self.airlock_wires(o)
            .iter()
            .enumerate()
            .map(|(i, wire)| {
                let action = if pulse {
                    "Pulse"
                } else if mask & (1 << i) == 0 {
                    "Cut"
                } else {
                    "Mend"
                };
                crate::verbs::Verb {
                    id: format!("door_wire_{action}_{i}"),
                    label: format!("{action} {} · {}", wire.letter, wire.color),
                    disabled: (pulse && mask & (1 << i) != 0).then(|| "Wire is cut".into()),
                }
            })
            .collect()
    }

    pub(super) fn use_airlock_wire(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
        verb: &str,
    ) {
        let Some((operation, index)) = verb
            .strip_prefix("door_wire_")
            .and_then(|rest| rest.rsplit_once('_'))
        else {
            return;
        };
        if !matches!(operation, "Cut" | "Mend" | "Pulse") {
            return;
        }
        let Ok(i) = index.parse::<u8>() else {
            return;
        };
        let Some(mut o) = self.wire_reach(ctx, actor, id).cloned() else {
            self.popup(actor, "You can't reach there!");
            return;
        };
        if usize::from(i) >= self.airlock_wires(&o).len() {
            return;
        }
        let pulse = operation == "Pulse";
        let cutting = operation == "Cut";
        let quality = if pulse { "Pulsing" } else { "Cutting" };
        if !self
            .used_item(actor)
            .and_then(|used| self.info(&used.item.prototype))
            .is_some_and(|info| info.qualities.contains(quality))
        {
            self.popup(
                actor,
                if pulse {
                    "You need to hold a multitool in your hand!"
                } else {
                    "You need to hold a wirecutter in your hand!"
                },
            );
            return;
        }
        let mut mask = cut_mask(&o);
        let cut = mask & (1 << i) != 0;
        let error = match operation {
            "Pulse" if cut => Some("You can't pulse a wire that's been cut!"),
            "Cut" if cut => Some("You can't cut a wire that's been cut!"),
            "Mend" if !cut => Some("You can't mend a wire that's been mended!"),
            _ => None,
        };
        if let Some(error) = error {
            self.popup(actor, error);
            return;
        }
        let powered = self.airlock_powered(ctx, id);
        let wire = &self.airlock_wires(&o)[usize::from(i)];
        let action = wire.action;
        let timeout = self.wire_timeout(wire);
        let mut changed = true;
        match action {
            "PowerWireAction" => {
                let shocked = self.power_wire_shock(ctx, actor, id, &mut o);
                if pulse {
                    if !flag(&o, "wire_power_pulsed", false) {
                        o.metadata.insert("wire_power_pulsed".into(), "1".into());
                        o.metadata.insert(
                            "wire_power_until".into(),
                            (self.clock + timeout).to_string(),
                        );
                        if !shocked {
                            o.metadata.insert("wire_power_disabled".into(), "1".into());
                        }
                    }
                } else if shocked {
                    changed = false;
                } else if !cutting {
                    o.metadata.remove("wire_power_until");
                    o.metadata.remove("wire_electrified_until");
                    if !flag(&o, "wire_power_pulsed", false) {
                        o.metadata.remove("wire_power_disabled");
                    }
                }
            }
            "DoorBoltWireAction" => {
                let down = flag(&o, "bolted", false);
                if pulse && (powered || !down) {
                    o.metadata
                        .insert("bolted".into(), u8::from(!down).to_string());
                } else if cutting && powered {
                    o.metadata.insert("bolted".into(), "1".into());
                }
            }
            "DoorBoltLightWireAction" => {
                let enabled = if pulse {
                    !flag(&o, "bolt_lights", true)
                } else {
                    !cutting
                };
                o.metadata
                    .insert("bolt_lights".into(), u8::from(enabled).to_string());
            }
            "DoorTimingWireAction" => {
                let modifier = if pulse {
                    0.5
                } else if cutting {
                    0.01
                } else {
                    1.
                };
                o.metadata
                    .insert("close_modifier".into(), modifier.to_string());
                if pulse {
                    o.metadata.insert(
                        "wire_timing_until".into(),
                        (self.clock + timeout).to_string(),
                    );
                } else if cutting {
                    o.metadata.remove("wire_timing_until");
                }
            }
            "DoorSafetyWireAction" | "LogWireAction" => {
                let (field, timer) = if action == "DoorSafetyWireAction" {
                    ("safety", "wire_safety_until")
                } else {
                    ("logging", "wire_logging_until")
                };
                o.metadata
                    .insert(field.into(), u8::from(!pulse && !cutting).to_string());
                if pulse {
                    o.metadata
                        .insert(timer.into(), (self.clock + timeout).to_string());
                } else if cutting {
                    o.metadata.remove(timer);
                }
            }
            "AiInteractWireAction" if !pulse => {
                o.metadata
                    .insert("ai_control".into(), u8::from(!cutting).to_string());
            }
            _ => {}
        }
        if !pulse && changed {
            mask ^= 1 << i;
        }
        o.metadata.insert("cut_wires".into(), mask.to_string());
        if let Some(used) = self.used_item(actor) {
            self.tool_sound(&used.item.prototype, "use", ctx.position_of(actor).unwrap());
        }
        ctx.request(WorldCommand::RemoveAuthored { id: o.id });
        ctx.request(WorldCommand::PlaceAuthored(o));
    }
}
