use super::*;
use std::sync::LazyLock;

#[derive(serde::Deserialize)]
struct Stack {
    kind: String,
    initial: u32,
    capacity: Option<u32>,
    spawn: String,
}

static STACKS: LazyLock<BTreeMap<String, Stack>> = LazyLock::new(|| {
    serde_json::from_value(construction::CATALOG["stacks"].clone()).expect("material stacks")
});
const SPLIT_AMOUNTS: [u32; 6] = [1, 5, 10, 20, 30, 50];

#[derive(Clone, Copy)]
enum Location {
    Hand(usize),
    Bag(ObjectId),
    Ground,
}

impl State {
    /// `SharedStackSystem.TryMergeToHands`, used by construction's GivePrototype.
    pub(super) fn merge_into_hands(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        item: &HeldItem,
        mut count: u32,
    ) -> bool {
        if let Some(spec) = self.stack_spec(&item.prototype) {
            let held: Vec<_> = self.actor(actor).hands.held().cloned().collect();
            for other in held {
                if let Some(current) = self.material_count(&other, &spec.kind) {
                    let capacity = self
                        .stack_spec(&other.prototype)
                        .and_then(|s| s.capacity)
                        .unwrap_or(u32::MAX);
                    let moved = count.min(capacity.saturating_sub(current));
                    self.charges.insert(other.id, current + moved);
                    count -= moved;
                    if count == 0 {
                        self.charges.remove(&item.id);
                        self.publish(ctx, actor);
                        return true;
                    }
                }
            }
            self.charges.insert(item.id, count);
        }
        let picked = self.actor(actor).hands.pickup_any(item.clone()).is_ok();
        self.publish(ctx, actor);
        picked
    }

    pub(super) fn consume_stack(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        used: &Used,
        count: u32,
        amount: u32,
    ) {
        if count == amount {
            let _ = self.actor(actor).hands.drop_hand(used.hand);
            self.charges.remove(&used.item.id);
        } else {
            self.charges.insert(used.item.id, count - amount);
        }
        self.publish(ctx, actor);
    }
    pub(super) fn craft_material(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        id: &[u8],
    ) {
        let Some(recipe) = crate::crafting::recipes()
            .iter()
            .find(|r| r.id.as_bytes() == id)
        else {
            return;
        };
        let Some(person) = self.actors.get(&actor) else {
            return;
        };
        if person.ghost
            || !person.health.state().can_act()
            || !self.proto_by_name.contains_key(&recipe.result)
        {
            return;
        }
        let Some(feet) = ctx.position_of(actor) else {
            return;
        };
        let mut candidates = Vec::new();
        for hand in 0..person.hands.len() {
            if let Some(i) = person.hands.get(hand) {
                candidates.push((i.clone(), Location::Hand(hand)));
            }
        }
        for bag in person.hands.held().chain(person.inventory.worn()) {
            if let Some(storage) = self.storages.get(&bag.id) {
                candidates.extend(
                    storage
                        .items()
                        .iter()
                        .map(|s| (s.item.clone(), Location::Bag(bag.id))),
                );
            }
        }
        for object in ctx.world.authored() {
            let AuthoredKind::Placement { prototype } = &object.kind else {
                continue;
            };
            if self.stack_spec(prototype).is_some()
                && self.reachable(ctx, feet, object)
                && !self.gone.contains(&object.id)
            {
                candidates.push((
                    HeldItem {
                        id: object.id,
                        prototype: prototype.clone(),
                    },
                    Location::Ground,
                ));
            }
        }
        let mut needed = BTreeMap::new();
        for (kind, count) in &recipe.materials {
            *needed.entry(kind.as_str()).or_insert(0u32) += count;
        }
        let mut deductions = Vec::new();
        for (item, location) in candidates {
            let Some(spec) = self.stack_spec(&item.prototype) else {
                continue;
            };
            let Some(needed) = needed.get_mut(spec.kind.as_str()) else {
                continue;
            };
            if *needed == 0 {
                continue;
            }
            let count = match location {
                Location::Ground => self.accessible_stack(ctx, actor, item.id).map(|s| s.1),
                _ => self.stack_count(&item),
            }
            .unwrap_or(0);
            let n = count.min(*needed);
            if n > 0 {
                deductions.push((item.id, location, count - n));
                *needed -= n;
            }
        }
        if needed.values().any(|&n| n > 0) {
            return;
        }
        let Some(output) = self.create_item(&recipe.result) else {
            return;
        };
        for (id, location, left) in deductions {
            if left > 0 {
                self.charges.insert(id, left);
                continue;
            }
            self.charges.remove(&id);
            match location {
                Location::Bag(bag) => {
                    let _ = self.storages.get_mut(&bag).unwrap().remove(id);
                }
                Location::Hand(hand) => {
                    let _ = self.actor(actor).hands.drop_hand(hand);
                }
                Location::Ground => {
                    self.gone.insert(id);
                    ctx.request(WorldCommand::RemoveAuthored { id });
                }
            }
        }
        let mut count = recipe.count;
        if let Some(spec) = self.stack_spec(&output.prototype) {
            let hands = self.actors[&actor].hands.clone();
            for item in hands.held() {
                if let Some(v) = self.material_count(item, &spec.kind) {
                    let max = self
                        .stack_spec(&item.prototype)
                        .and_then(|s| s.capacity)
                        .unwrap_or(u32::MAX);
                    let add = count.min(max.saturating_sub(v));
                    self.charges.insert(item.id, v + add);
                    count -= add;
                    if count == 0 {
                        break;
                    }
                }
            }
        }
        if count > 0 {
            self.charges.insert(output.id, count);
            if self.actor(actor).hands.pickup_any(output.clone()).is_err() {
                self.place(ctx, &output, feet);
            }
        }
        self.publish(ctx, actor);
    }
    fn stack_spec(&self, prototype: &PrototypeId) -> Option<&'static Stack> {
        STACKS.get(self.ss14_names.get(prototype)?)
    }

    pub(super) fn stack_count(&self, item: &HeldItem) -> Option<u32> {
        let spec = self.stack_spec(&item.prototype)?;
        Some(self.charges.get(&item.id).copied().unwrap_or(spec.initial))
    }

    pub(super) fn material_count(&self, item: &HeldItem, kind: &str) -> Option<u32> {
        (self.stack_spec(&item.prototype)?.kind == kind)
            .then(|| self.stack_count(item))
            .flatten()
    }

    fn accessible_stack(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) -> Option<(&'static Stack, u32)> {
        let person = self.actors.get(&actor)?;
        let object = ctx.world.authored_object(id)?;
        if person.ghost
            || !person.health.state().can_act()
            || self.gone.contains(&id)
            || !self.reachable(ctx, ctx.position_of(actor)?, object)
        {
            return None;
        }
        let AuthoredKind::Placement { prototype } = &object.kind else {
            return None;
        };
        let spec = self.stack_spec(prototype)?;
        let count = self
            .charges
            .get(&id)
            .copied()
            .or_else(|| object.metadata.get("stack_count")?.parse().ok())
            .unwrap_or(spec.initial);
        Some((spec, count))
    }

    pub(super) fn stack_verbs(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
    ) -> Vec<crate::verbs::Verb> {
        let Some((_, count)) = self.accessible_stack(ctx, actor, id) else {
            return Vec::new();
        };
        let mut verbs: Vec<_> = SPLIT_AMOUNTS
            .into_iter()
            .filter(|&n| n <= count)
            .map(|n| crate::verbs::Verb {
                id: format!("split_{n}"),
                label: format!("Отделить {n}"),
                disabled: None,
            })
            .collect();
        verbs.push(crate::verbs::Verb {
            id: "split_half".into(),
            label: "Разделить пополам".into(),
            disabled: None,
        });
        verbs
    }

    pub(super) fn move_stack(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        source: ObjectId,
        split: Option<&str>,
    ) -> bool {
        let Some((spec, count)) = self.accessible_stack(ctx, actor, source) else {
            return false;
        };
        let amount = match split {
            Some("half") => count.div_ceil(2),
            Some(s) => match s.parse::<u32>() {
                Ok(n) if SPLIT_AMOUNTS.contains(&n) && n <= count => n,
                _ => return false,
            },
            None => count,
        };
        if amount == 0 {
            return false;
        }
        let held = self.used_item(actor).map(|u| u.item);
        let moved = if let Some((target, current)) = held.as_ref().and_then(|i| {
            self.material_count(i, &spec.kind)
                .map(|current| (i, current))
        }) {
            let capacity = self
                .stack_spec(&target.prototype)
                .and_then(|s| s.capacity)
                .unwrap_or(u32::MAX);
            let n = amount.min(capacity.saturating_sub(current));
            if n == 0 && split.is_none() {
                return true;
            }
            if n > 0 {
                self.charges.insert(target.id, current + n);
                n
            } else {
                0
            }
        } else {
            0
        };
        let moved = if moved > 0 {
            moved
        } else {
            if split.is_none() {
                return false;
            }
            if amount == count {
                return self.pickup(ctx, actor, source, true);
            }
            let Some(item) = self.create_item(&spec.spawn) else {
                return false;
            };
            if self.actor(actor).hands.pickup_any(item.clone()).is_err()
                && !self.place(ctx, &item, ctx.position_of(actor).unwrap())
            {
                return false;
            }
            self.charges.insert(item.id, amount);
            amount
        };
        if moved == count {
            self.charges.remove(&source);
            self.gone.insert(source);
            ctx.request(WorldCommand::RemoveAuthored { id: source });
        } else {
            self.charges.insert(source, count - moved);
        }
        self.publish(ctx, actor);
        true
    }
}
