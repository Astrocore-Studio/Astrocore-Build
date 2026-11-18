//! SS14 RepairableSystem, SharedToolSystem and the damage state of repairable objects.
use super::*;
use crate::doafter::RepairJob;
use serde_json::Value;

#[derive(Clone)]
pub(super) struct ObjectDamage {
    prototype: PrototypeId,
    damage: DamageSpec,
    reached: BTreeSet<usize>,
    once: BTreeSet<usize>,
    broken: bool,
}

fn cents(value: f32) -> i64 {
    let scaled = value * 100.;
    (scaled + scaled.signum() * 0.00001).trunc() as i64
}

/// DamageValue takes precedence. HealEvenly rounds up to FixedPoint2.Epsilon (0.01).
pub(crate) fn repair_damage(damage: &mut DamageSpec, spec: &Value) {
    if let Some(value) = spec["damageValue"].as_f64() {
        let mut left = (-cents(value as f32)).max(0);
        let mut amounts: Vec<_> = damage
            .iter_mut()
            .filter(|(_, value)| **value > 0.)
            .collect();
        amounts.sort_by_key(|(name, _)| {
            crate::health::KINDS
                .iter()
                .position(|kind| *kind == name.as_str())
                .unwrap_or(usize::MAX)
        });
        while left > 0 && !amounts.is_empty() {
            let max_heal = (left + amounts.len() as i64 - 1) / amounts.len() as i64;
            for (_, value) in amounts.iter_mut().rev() {
                let old = (**value * 100.).round() as i64;
                let heal = old.min(max_heal).min(left);
                **value = (old - heal) as f32 / 100.;
                left -= heal;
                if left == 0 {
                    break;
                }
            }
            amounts.retain(|(_, value)| **value > 0.);
        }
    } else if let Some(types) = spec["damage"]["types"].as_object() {
        for (kind, delta) in types {
            if let Some(delta) = delta.as_f64() {
                let old = damage.get(kind).copied().unwrap_or(0.);
                damage.insert(
                    kind.clone(),
                    ((old * 100.).round() as i64 + cents(delta as f32)).max(0) as f32 / 100.,
                );
            }
        }
    } else {
        damage.clear();
    }
    damage.retain(|_, value| *value > 0.);
}

impl State {
    fn object_damage_spec(&self, prototype: &PrototypeId) -> &'static Value {
        let name = self
            .ss14_names
            .get(prototype)
            .map(String::as_str)
            .unwrap_or("");
        &construction::CATALOG["damageables"][name]
    }

    fn repair_target(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        target: ObjectId,
    ) -> Option<(PrototypeId, Vec3)> {
        let person = self.actors.get(&actor)?;
        if person.ghost || !person.health.state().can_act() || person.stun_until > self.clock {
            return None;
        }
        let feet = ctx.position_of(actor)?;
        if let Some(item) = person
            .hands
            .held()
            .chain(person.inventory.worn())
            .find(|item| item.id == target)
        {
            return Some((item.prototype.clone(), feet));
        }
        let object = ctx.world.authored_object(target)?;
        let AuthoredKind::Placement { prototype } = &object.kind else {
            return None;
        };
        if self.gone.contains(&target) || !self.reachable(ctx, feet, object) {
            return None;
        }
        Some((prototype.clone(), (object.min() + object.max()) * 0.5))
    }

    fn read_object_damage(
        &self,
        ctx: &HookContext<'_>,
        target: ObjectId,
        prototype: &PrototypeId,
    ) -> ObjectDamage {
        let spec = self.object_damage_spec(prototype);
        let object = ctx.world.authored_object(target);
        if (object.is_none() || self.pending_object_damage.contains(&target))
            && let Some(cached) = self
                .object_damage
                .get(&target)
                .filter(|cached| cached.prototype == *prototype)
        {
            return cached.clone();
        }
        let metadata = object.map(|object| &object.metadata);
        ObjectDamage {
            prototype: prototype.clone(),
            damage: metadata
                .and_then(|m| m.get("damage"))
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| {
                    serde_json::from_value(spec["initial"].clone()).unwrap_or_default()
                }),
            reached: metadata
                .and_then(|m| m.get("damage_reached"))
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default(),
            once: metadata
                .and_then(|m| m.get("damage_once"))
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default(),
            broken: metadata
                .and_then(|m| m.get("broken"))
                .is_some_and(|s| s == "1"),
        }
    }

    pub(super) fn remember_object_damage(
        &mut self,
        ctx: &HookContext<'_>,
        id: ObjectId,
        prototype: &PrototypeId,
    ) {
        if self.object_damage_spec(prototype).is_object() {
            let damage = self.read_object_damage(ctx, id, prototype);
            self.object_damage.insert(id, damage);
        }
    }

    pub(super) fn place_damage_metadata(&self, item: &HeldItem, object: &mut AuthoredObject) {
        if let Some(damage) = self
            .object_damage
            .get(&item.id)
            .filter(|damage| damage.prototype == item.prototype)
        {
            Self::damage_metadata(damage, object);
        }
    }

    fn damage_metadata(damage: &ObjectDamage, object: &mut AuthoredObject) {
        object.metadata.insert(
            "damage".into(),
            serde_json::to_string(&damage.damage).unwrap(),
        );
        object.metadata.insert(
            "damage_reached".into(),
            serde_json::to_string(&damage.reached).unwrap(),
        );
        object.metadata.insert(
            "damage_once".into(),
            serde_json::to_string(&damage.once).unwrap(),
        );
        object
            .metadata
            .insert("broken".into(), u8::from(damage.broken).to_string());
        let broken_sprite = format!("{}_broken", damage.prototype);
        if damage.broken {
            object
                .metadata
                .insert("render_sprite".into(), broken_sprite);
        } else if object.metadata.get("render_sprite") == Some(&broken_sprite) {
            object.metadata.remove("render_sprite");
        }
    }

    fn write_object_damage(
        &mut self,
        ctx: &mut HookContext<'_>,
        id: ObjectId,
        damage: ObjectDamage,
    ) {
        if let Some(mut object) = ctx.world.authored_object(id).cloned() {
            Self::damage_metadata(&damage, &mut object);
            ctx.request(WorldCommand::RemoveAuthored { id });
            ctx.request(WorldCommand::PlaceAuthored(object));
        }
        self.object_damage.insert(id, damage);
        self.pending_object_damage.insert(id);
    }

    pub(super) fn begin_object_repair(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        target: ObjectId,
    ) -> bool {
        let Some((prototype, _)) = self.repair_target(ctx, actor, target) else {
            return false;
        };
        if !self.object_damage_spec(&prototype).is_object() {
            return false;
        }
        let damage = self.read_object_damage(ctx, target, &prototype);
        if damage.damage.values().sum::<f32>() <= 0. {
            return false;
        }
        let name = &self.ss14_names[&prototype];
        let spec = &construction::CATALOG["repairables"][name];
        let Some(used) = self.used_item(actor) else {
            return false;
        };
        let tool = self
            .ss14_names
            .get(&used.item.prototype)
            .map(String::as_str)
            .unwrap_or("");
        let speed = construction::CATALOG["toolSpeeds"][tool]
            .as_f64()
            .unwrap_or(1.) as f32;
        let job = RepairJob {
            target,
            prototype,
            hand: used.hand,
            tool: used.item.id,
            fuel: spec["fuelCost"].as_f64().unwrap_or(5.) as f32,
            delay: spec["doAfterDelay"].as_f64().unwrap_or(1.) as f32 / speed,
        };
        if !self.can_repair_object(ctx, actor, &job) {
            self.repair_tool_feedback(actor, &job);
            return false;
        }
        self.object_damage.insert(target, damage);
        self.begin(ctx, actor, Action::Repair(job));
        true
    }

    pub(super) fn can_repair_object(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        job: &RepairJob,
    ) -> bool {
        if self
            .repair_target(ctx, actor, job.target)
            .is_none_or(|(prototype, _)| prototype != job.prototype)
        {
            return false;
        }
        let Some(used) = self
            .used_item(actor)
            .filter(|used| used.hand == job.hand && used.item.id == job.tool)
        else {
            return false;
        };
        let spec = &construction::CATALOG["repairables"][&self.ss14_names[&job.prototype]];
        if !self.info(&used.item.prototype).is_some_and(|info| {
            info.qualities
                .contains(spec["qualityNeeded"].as_str().unwrap_or("Welding"))
        }) {
            return false;
        }
        self.welding_tools
            .get(&used.item.prototype)
            .is_none_or(|welder| {
                self.lit.contains(&used.item.id)
                    && self
                        .solutions
                        .get(&used.item.id)
                        .or_else(|| self.solution_templates.get(&used.item.prototype))
                        .is_some_and(|solution| solution.amount(&welder.fuel) >= job.fuel)
            })
    }

    /// SharedToolSystem.Welder checks both conditions on start and every DoAfter attempt.
    pub(super) fn repair_tool_feedback(&mut self, actor: NetworkEntityId, job: &RepairJob) {
        let Some(used) = self
            .used_item(actor)
            .filter(|used| used.hand == job.hand && used.item.id == job.tool)
        else {
            return;
        };
        let Some(welder) = self.welding_tools.get(&used.item.prototype) else {
            return;
        };
        let unlit = !self.lit.contains(&job.tool);
        let low_fuel = self
            .solutions
            .get(&job.tool)
            .or_else(|| self.solution_templates.get(&used.item.prototype))
            .map_or(0., |solution| solution.amount(&welder.fuel))
            < job.fuel;
        let name = self
            .display_names
            .get(&used.item.prototype)
            .map(String::as_str)
            .unwrap_or("welder")
            .to_owned();
        if unlit {
            self.popup(actor, format!("The {name} is turned off!"));
        }
        if low_fuel {
            self.popup(
                actor,
                format!("The {name} does not have enough fuel for that!"),
            );
        }
    }

    pub(super) fn finish_object_repair(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        job: RepairJob,
    ) {
        if !self.can_repair_object(ctx, actor, &job) {
            self.repair_tool_feedback(actor, &job);
            return;
        }
        let tool = self.actors[&actor].hands.get(job.hand).unwrap().clone();
        if let Some(welder) = self.welding_tools.get(&tool.prototype).copied() {
            self.solution_mut(&tool)
                .unwrap()
                .remove(&welder.fuel, job.fuel);
        }
        self.tool_sound(&tool.prototype, "use", ctx.position_of(actor).unwrap());
        let mut damage = self.read_object_damage(ctx, job.target, &job.prototype);
        if damage.damage.values().sum::<f32>() <= 0. {
            self.publish(ctx, actor);
            return;
        }
        let spec = &construction::CATALOG["repairables"][&self.ss14_names[&job.prototype]];
        let before = damage.damage.values().sum::<f32>();
        repair_damage(&mut damage.damage, spec);
        let after = damage.damage.values().sum::<f32>();
        if after < before {
            damage.broken = false;
        }
        let destroyed = self.object_damage_thresholds(ctx, job.target, &mut damage);
        self.write_object_damage(ctx, job.target, damage);
        self.publish(ctx, actor);
        if destroyed {
            self.destroy_repairable(ctx, job.target, &job.prototype);
            return;
        }
        ctx.log(format!(
            "{actor:?} repaired {} by {}",
            job.target,
            before - after
        ));
        if spec["autoDoAfter"].as_bool().unwrap_or(true) && after > 0. {
            self.begin(ctx, actor, Action::Repair(job));
        } else {
            let target = self
                .display_names
                .get(&job.prototype)
                .map(String::as_str)
                .unwrap_or("object");
            let tool = self
                .display_names
                .get(&tool.prototype)
                .map(String::as_str)
                .unwrap_or("tool");
            self.popup(
                actor,
                format!("You finish repairing the {target} with the {tool}"),
            );
        }
    }

    pub(super) fn attack_repairable(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) {
        let Some((prototype, _)) = self.repair_target(ctx, actor, id) else {
            return;
        };
        if !self.object_damage_spec(&prototype).is_object() {
            return;
        }
        let Some(person) = self.actors.get(&actor) else {
            return;
        };
        if self.clock < person.next_attack
            || person
                .body
                .as_ref()
                .is_some_and(|body| body.has_status("Pacified"))
        {
            return;
        }
        let weapon = person
            .hands
            .get(person.hands.active())
            .and_then(|item| self.melee.get(&item.prototype))
            .cloned()
            .unwrap_or_else(|| Melee {
                damage: [("Blunt".into(), FIST_DAMAGE)].into(),
                rate: DEFAULT_ATTACK_RATE,
                stamina_factor: 0.5,
            });
        self.actor(actor).next_attack = self.clock + 1. / weapon.rate;
        let mut state = self.read_object_damage(ctx, id, &prototype);
        let spec = self.object_damage_spec(&prototype);
        for (kind, raw) in weapon.damage {
            let coefficient = spec["coefficients"][&kind].as_f64().unwrap_or(1.) as f32;
            let flat = spec["flat"][&kind].as_f64().unwrap_or(0.) as f32;
            let amount = (cents(raw * coefficient) - cents(flat)).max(0);
            let old = (state.damage.get(&kind).copied().unwrap_or(0.) * 100.).round() as i64;
            state.damage.insert(kind, (old + amount) as f32 / 100.);
        }
        let destroyed = self.object_damage_thresholds(ctx, id, &mut state);
        self.write_object_damage(ctx, id, state);
        if destroyed {
            self.destroy_repairable(ctx, id, &prototype);
        }
    }

    pub(crate) fn repairable_broken(&self, ctx: &HookContext<'_>, id: ObjectId) -> bool {
        ctx.world.authored_object(id).is_some_and(|object| {
            object
                .metadata
                .get("broken")
                .is_some_and(|value| value == "1")
        })
    }

    fn object_damage_thresholds(
        &mut self,
        ctx: &mut HookContext<'_>,
        id: ObjectId,
        damage: &mut ObjectDamage,
    ) -> bool {
        let total = damage.damage.values().sum::<f32>();
        let mut destroyed = false;
        let Some(object) = ctx.world.authored_object(id) else {
            return false;
        };
        let middle = (object.min() + object.max()) * 0.5;
        let at = Vec3::new(middle.x, object.min().y, middle.z);
        for (index, threshold) in self.object_damage_spec(&damage.prototype)["thresholds"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if threshold["trigger"]["__tag"] != "DamageTrigger" {
                continue;
            }
            let reached = total
                >= threshold["trigger"]["damage"]
                    .as_f64()
                    .unwrap_or(f64::INFINITY) as f32;
            let old = damage.reached.contains(&index);
            if reached {
                damage.reached.insert(index);
            } else {
                damage.reached.remove(&index);
            }
            if !reached || old || damage.once.contains(&index) {
                continue;
            }
            if threshold["triggersOnce"] == true {
                damage.once.insert(index);
            }
            for behavior in threshold["behaviors"].as_array().into_iter().flatten() {
                match behavior["__tag"].as_str() {
                    Some("DoActsBehavior") => {
                        for act in behavior["acts"].as_array().into_iter().flatten() {
                            if act == "Breakage" {
                                damage.broken = true;
                            }
                            if act == "Destruction" {
                                destroyed = true;
                            }
                        }
                    }
                    Some("PlaySoundBehavior") => self.engineering_sound(&behavior["sound"], at),
                    Some("SpawnEntitiesBehavior") => {
                        for (name, amount) in behavior["spawn"].as_object().into_iter().flatten() {
                            let min = amount["min"].as_u64().unwrap_or(1) as u32;
                            let max = amount["max"].as_u64().unwrap_or(u64::from(min)) as u32;
                            let count = if min >= max {
                                min as f32
                            } else {
                                min as f32 + self.spawn_rng.next_f32() * (max - min + 1) as f32
                            };
                            if count == 0. {
                                continue;
                            }
                            let Some(item) = self.create_item(name) else {
                                ctx.log(format!("Missing destruction drop: {name}"));
                                continue;
                            };
                            let stack = self.stack_count(&item).is_some();
                            let instances = if stack { 1 } else { count.ceil() as u32 };
                            if stack {
                                self.charges.insert(item.id, count as u32);
                            }
                            let offset = behavior["offset"].as_f64().unwrap_or(0.5) as f32;
                            for index in 0..instances {
                                let dropped = if index == 0 {
                                    item.clone()
                                } else {
                                    self.create_item(name).unwrap()
                                };
                                let point = at
                                    + Vec3::new(
                                        (self.spawn_rng.next_f32() * 2. - 1.) * offset,
                                        0.,
                                        (self.spawn_rng.next_f32() * 2. - 1.) * offset,
                                    );
                                self.place(ctx, &dropped, point);
                            }
                        }
                    }
                    Some("EjectVendorItems") => self.eject_damaged_vendor(ctx, id, behavior, at),
                    _ => ctx.log(format!(
                        "Unsupported destruction behavior for {}: {}",
                        damage.prototype, behavior["__tag"]
                    )),
                }
            }
            if destroyed {
                break;
            }
        }
        destroyed
    }

    /// `EjectVendorItems`: автомат при поломке выбрасывает часть запаса. Запас — у системы
    /// автоматов, поэтому здесь только событие для неё.
    fn eject_damaged_vendor(
        &mut self,
        _ctx: &mut HookContext<'_>,
        id: ObjectId,
        spec: &Value,
        at: Vec3,
    ) {
        self.events
            .push(crate::systems::GameEvent::EjectVendorItems {
                object: id,
                percent: spec["percent"].as_f64().unwrap_or(0.25) as f32,
                max: spec["max"].as_f64().unwrap_or(3.) as f32,
                at,
            });
    }

    fn destroy_repairable(
        &mut self,
        ctx: &mut HookContext<'_>,
        id: ObjectId,
        prototype: &PrototypeId,
    ) {
        if let Some(object) = ctx.world.authored_object(id) {
            let at = (object.min() + object.max()) * 0.5;
            let elements: Vec<_> = ctx.world.elements().filter(|element| element.floor == object.floor && element.material == *prototype
                && matches!(element.placement, Placement::Free(p) if p.cell.x == at.x.floor() as i32 && p.cell.z == at.z.floor() as i32)).map(|element| element.id).collect();
            for id in elements {
                ctx.request(WorldCommand::RemoveElement { id });
            }
        }
        self.gone.insert(id);
        ctx.request(WorldCommand::RemoveAuthored { id });
    }
}
