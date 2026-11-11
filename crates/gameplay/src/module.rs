use std::collections::BTreeMap;
use std::sync::Arc;

use astrocore_api::{GameModule, HookContext, HookEvent};
use astrocore_core::{NetworkEntityId, ObjectId, PrototypeId};
use astrocore_math::glam::Vec3;
use astrocore_runtime::modules::DoorAction;
use astrocore_world::{
    AuthoredKind, AuthoredObject, FloorId, OpeningKind, Placement, StructureKind, WorldCommand,
};
use prototypes::PrototypeRegistry;

use crate::doafter::{
    Action, BREAK_DISTANCE_M, DOAFTER_COMPONENT, DoAfter, DoAfterView, PRY_TIME_S, encode_doafter,
};
use crate::hands::{Hands, HeldItem};
use crate::health::{
    DEFAULT_ATTACK_RATE, DamageSpec, FIST_DAMAGE, HEALTH_COMPONENT, Healing, Health, MELEE_RANGE_M,
    MOBSTATE_COMPONENT, Melee, MobState, encode_health,
};
use crate::interact::{InteractionHandler, Target, Used};
use crate::inventory::Inventory;
use crate::items::{ItemInfo, item_info};
use crate::storage::Storage;
use crate::wire::{
    BagView, CMD_ATTACK, CMD_CANCEL, CMD_DROP, CMD_EQUIP, CMD_HEAL, CMD_INTERACT, CMD_MOVE_HAND,
    CMD_MOVE_SLOT, CMD_PICKUP, CMD_PICKUP_ANY, CMD_SELECT_HAND, CMD_STORE, CMD_SWAP_HANDS,
    CMD_TAKE, CMD_TAKE_TO, CMD_UNEQUIP, CMD_UNEQUIP_TO, CMD_USE, HANDS_COMPONENT,
    INVENTORY_COMPONENT, InventoryView, encode_hands, encode_inventory, parse_attack, parse_drop,
    parse_equip, parse_heal, parse_move_slot, parse_object_id, parse_store, parse_take,
    parse_take_to, parse_unequip,
};

/// Дальность взаимодействия, м: `InteractionRange` в SS14 (1,5). Меряется от груди персонажа
/// до центра предмета; путь не должен перекрываться стенами и закрытыми дверями.
pub const INTERACTION_RANGE_M: f32 = 1.5;
/// Высота груди над ступнями, м.
const CHEST_M: f32 = 1.0;
/// Размер выброшенного предмета в мире, м.
const DROPPED_SIZE_M: f32 = 0.3;
/// На сколько выброшенный предмет отступает от препятствия, м.
const DROP_CLEARANCE_M: f32 = 0.1;

/// Пространство имён идентификаторов прототипов билда в движке.
pub const NAMESPACE: &str = "ss14";

/// Всё, что персонаж держит и носит.
pub(crate) struct Actor {
    pub(crate) hands: Hands,
    pub(crate) inventory: Inventory,
    pub(crate) doafter: Option<DoAfter>,
    pub(crate) health: Health,
    /// Состояние, которое клиентам уже разослано (чтобы замечать переходы).
    pub(crate) mob_state: MobState,
    /// Когда персонаж снова может ударить, с (по часам модуля).
    pub(crate) next_attack: f32,
}

impl Actor {
    fn new() -> Self {
        Self {
            hands: Hands::humanoid(),
            inventory: Inventory::human(),
            doafter: None,
            health: Health::default(),
            mob_state: MobState::Alive,
            next_attack: 0.0,
        }
    }
}

/// Игровой модуль рук, инвентаря и хранилищ; логика следует Content.Shared SS14.
///
/// Предметы на земле — авторские объекты мира (`AuthoredKind::Placement`): они реплицируются,
/// сохраняются и валидируются движком. Предметы в руках, на теле и в сумках живут в состоянии
/// модуля; руки видны всем (`build.hands`), инвентарь и содержимое сумок — только владельцу
/// (`build.inventory`).
pub struct State {
    pub(crate) infos: BTreeMap<PrototypeId, ItemInfo>,
    pub(crate) range_m: f32,
    pub(crate) actors: BTreeMap<NetworkEntityId, Actor>,
    /// Содержимое сумок по идентификатору сумки: переживает смену владельца и выброс сумки.
    pub(crate) storages: BTreeMap<ObjectId, Storage>,
    /// Задержки надевания и снятия одежды, с: (`equipDelay`, `unequipDelay`) из `Clothing`.
    pub(crate) delays: BTreeMap<PrototypeId, (f32, f32)>,
    pub(crate) melee: BTreeMap<PrototypeId, Melee>,
    pub(crate) healing: BTreeMap<PrototypeId, Healing>,
    /// Часы модуля, с: копятся по `Tick`.
    pub(crate) clock: f32,
}

impl State {
    /// `Err`, если два имени прототипов дают один идентификатор движка.
    pub(crate) fn new(registry: Arc<PrototypeRegistry>) -> Result<Self, prototypes::ProtoError> {
        let names = registry.engine_index(NAMESPACE)?;
        let mut infos = BTreeMap::new();
        let mut delays = BTreeMap::new();
        let mut melee = BTreeMap::new();
        let mut healing = BTreeMap::new();
        for (id, name) in names {
            let resolved = registry.spawnable(&name).ok();
            if let Some(info) = resolved.as_ref().and_then(item_info) {
                infos.insert(id.clone(), info);
            }
            let delay = |key: &str| {
                resolved
                    .as_ref()
                    .and_then(|r| r.component("Clothing"))
                    .and_then(|c| c.get(key))
                    .and_then(prototypes::Data::as_f64)
                    .map_or(0.0, |v| v.max(0.0) as f32)
            };
            if let Some(c) = resolved.as_ref().and_then(|r| r.component("MeleeWeapon")) {
                melee.insert(
                    id.clone(),
                    Melee {
                        damage: damage_spec(c.get("damage")),
                        rate: c
                            .get("attackRate")
                            .and_then(prototypes::Data::as_f64)
                            .map_or(DEFAULT_ATTACK_RATE, |v| v.max(0.1) as f32),
                    },
                );
            }
            if let Some(c) = resolved.as_ref().and_then(|r| r.component("Healing")) {
                healing.insert(
                    id.clone(),
                    Healing {
                        damage: damage_spec(c.get("damage")),
                        delay: c
                            .get("delay")
                            .and_then(prototypes::Data::as_f64)
                            .map_or(3.0, |v| v.max(0.0) as f32),
                    },
                );
            }
            let pair = (delay("equipDelay"), delay("unequipDelay"));
            if pair != (0.0, 0.0) {
                delays.insert(id, pair);
            }
        }
        Ok(Self {
            infos,
            range_m: INTERACTION_RANGE_M,
            actors: BTreeMap::new(),
            storages: BTreeMap::new(),
            delays,
            melee,
            healing,
            clock: 0.0,
        })
    }

    #[must_use]
    pub fn with_range(mut self, range_m: f32) -> Self {
        self.range_m = range_m;
        self
    }

    pub fn hands_of(&self, actor: NetworkEntityId) -> Option<&Hands> {
        self.actors.get(&actor).map(|a| &a.hands)
    }

    pub fn inventory_of(&self, actor: NetworkEntityId) -> Option<&Inventory> {
        self.actors.get(&actor).map(|a| &a.inventory)
    }

    pub fn storage_of(&self, bag: ObjectId) -> Option<&Storage> {
        self.storages.get(&bag)
    }

    /// Предмет в активной руке: рука и сам предмет.
    pub(crate) fn used_item(&self, actor: NetworkEntityId) -> Option<Used> {
        let a = self.actors.get(&actor)?;
        let hand = a.hands.active();
        a.hands.get(hand).map(|i| Used {
            hand,
            item: i.clone(),
        })
    }

    pub(crate) fn used_in(&self, actor: NetworkEntityId, hand: usize) -> Option<Used> {
        self.actors.get(&actor)?.hands.get(hand).map(|i| Used {
            hand,
            item: i.clone(),
        })
    }

    pub(crate) fn actor(&mut self, id: NetworkEntityId) -> &mut Actor {
        self.actors.entry(id).or_insert_with(Actor::new)
    }

    pub(crate) fn info(&self, proto: &PrototypeId) -> Option<&ItemInfo> {
        self.infos.get(proto)
    }

    /// Рассылка состояния: руки всем, инвентарь с содержимым сумок — владельцу.
    pub(crate) fn publish(&self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        let Some(a) = self.actors.get(&actor) else {
            return;
        };
        ctx.set_component(actor, HANDS_COMPONENT, encode_hands(&a.hands));
        ctx.set_component(actor, HEALTH_COMPONENT, encode_health(&a.health.view()));
        ctx.set_component(actor, MOBSTATE_COMPONENT, vec![a.health.state().to_byte()]);
        let worn: crate::wire::Worn = a
            .inventory
            .iter()
            .filter_map(|(d, i)| Some((d.name.clone(), i?.prototype.to_string())))
            .collect();
        ctx.set_component(
            actor,
            crate::wire::WORN_COMPONENT,
            crate::wire::encode_worn(&worn),
        );
        let bags = a
            .hands
            .held()
            .chain(a.inventory.worn())
            .filter(|i| self.info(&i.prototype).is_some_and(|x| x.storage.is_some()))
            .map(|i| BagView {
                id: i.id,
                grid: self
                    .info(&i.prototype)
                    .and_then(|x| x.storage.as_ref())
                    .map(|s| s.cells.iter().copied().collect())
                    .unwrap_or_default(),
                items: self
                    .storages
                    .get(&i.id)
                    .map(|s| s.items().to_vec())
                    .unwrap_or_default(),
            })
            .collect();
        let view = InventoryView {
            inventory: a.inventory.clone(),
            bags,
        };
        ctx.set_component(actor, INVENTORY_COMPONENT, encode_inventory(&view));
    }

    /// Предмет в зоне досягаемости: близко, без преград между грудью и центром объекта.
    pub(crate) fn reachable(
        &self,
        ctx: &HookContext<'_>,
        feet: Vec3,
        obj: &AuthoredObject,
    ) -> bool {
        let chest = feet + Vec3::Y * CHEST_M;
        let center = (obj.min() + obj.max()) * 0.5;
        (center - chest).length() <= self.range_m && ctx.is_unobstructed(chest, center)
    }

    /// Подбор. `any = false` — клик (только активная рука), `true` — глагол «поднять».
    pub(crate) fn pickup(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
        any: bool,
    ) -> bool {
        let (Some(pos), Some(obj)) = (ctx.position_of(actor), ctx.world.authored_object(id)) else {
            return false;
        };
        let AuthoredKind::Placement { prototype } = &obj.kind else {
            return false;
        };
        if self.info(prototype).is_none() || !self.reachable(ctx, pos, obj) {
            return false;
        }
        let item = HeldItem {
            id,
            prototype: prototype.clone(),
        };
        let hands = &mut self.actor(actor).hands;
        let taken = if any {
            hands.pickup_any(item)
        } else {
            hands.pickup_active(item)
        };
        if taken.is_err() {
            return false;
        }
        ctx.request(WorldCommand::RemoveAuthored { id });
        self.publish(ctx, actor);
        true
    }

    /// Положить предмет в мир в точке `at` (ступни предмета).
    pub(crate) fn place(ctx: &mut HookContext<'_>, item: &HeldItem, at: Vec3) -> bool {
        let Some(floor) = floor_at(ctx, at.y) else {
            return false;
        };
        let h = DROPPED_SIZE_M * 0.5;
        let obj = AuthoredObject::new(
            item.id,
            AuthoredKind::Placement {
                prototype: item.prototype.clone(),
            },
            floor,
            Vec3::new(at.x - h, at.y, at.z - h),
            Vec3::new(at.x + h, at.y + DROPPED_SIZE_M, at.z + h),
        );
        ctx.request(WorldCommand::PlaceAuthored(obj));
        true
    }

    /// Куда ляжет выброшенный предмет (`GetFinalDropCoordinates` в SS14): цель обрезается
    /// дальностью взаимодействия и укорачивается до ближайшей преграды; без цели — под ноги.
    pub(crate) fn drop_point(
        &self,
        ctx: &HookContext<'_>,
        feet: Vec3,
        target: Option<[f32; 3]>,
    ) -> Vec3 {
        let Some([tx, _, tz]) = target else {
            return feet;
        };
        let chest = feet + Vec3::Y * CHEST_M;
        let want = Vec3::new(tx, chest.y, tz) - chest;
        let len = want.length();
        if len < 1e-4 {
            return feet;
        }
        let dir = want / len;
        let reach = len.min(self.range_m);
        let free = ctx
            .unobstructed_distance(chest, chest + dir * reach)
            .min(reach);
        let dist = if free < reach {
            (free - DROP_CLEARANCE_M).max(0.0)
        } else {
            free
        };
        let p = chest + dir * dist;
        Vec3::new(p.x, feet.y, p.z)
    }

    pub(crate) fn drop_hand(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        target: Option<[f32; 3]>,
    ) {
        let (Some(feet), Some(a)) = (ctx.position_of(actor), self.actors.get(&actor)) else {
            return;
        };
        let Some(item) = a.hands.get(hand).cloned() else {
            return;
        };
        let at = self.drop_point(ctx, feet, target);
        if Self::place(ctx, &item, at) {
            let _ = self.actor(actor).hands.drop_hand(hand);
            self.publish(ctx, actor);
        }
    }

    /// Надеть предмет из руки в слот (`CanEquip` + `TryEquip` в SS14).
    pub(crate) fn equip(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        slot: &str,
    ) {
        let Some(a) = self.actors.get(&actor) else {
            return;
        };
        let Some(item) = a.hands.get(hand).cloned() else {
            return;
        };
        let Some(info) = self.info(&item.prototype).cloned() else {
            return;
        };
        if a.inventory.can_equip(slot, &info).is_err() {
            return;
        }
        let a = self.actor(actor);
        let _ = a.hands.drop_hand(hand);
        let _ = a.inventory.equip(slot, item, &info);
        self.publish(ctx, actor);
    }

    /// Снять предмет: в свободную руку (`PickupOrDrop`), иначе на пол; зависимые слоты
    /// выпадают на пол.
    pub(crate) fn unequip(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        slot: &str,
        hand: Option<usize>,
    ) {
        let (Some(feet), Some(a)) = (ctx.position_of(actor), self.actors.get(&actor)) else {
            return;
        };
        let Some(worn) = a.inventory.get(slot) else {
            return;
        };
        let needs_floor = a.hands.order().all(|h| a.hands.get(h).is_some())
            || a.inventory
                .iter()
                .any(|(d, i)| d.depends_on.as_deref() == Some(slot) && i.is_some());
        // Нужен пол, а этажа под ногами нет: ничего не меняем, чтобы предметы не пропали.
        if needs_floor && floor_at(ctx, feet.y).is_none() {
            return;
        }
        let _ = worn;
        let a = self.actor(actor);
        let Ok((item, dropped)) = a.inventory.unequip(slot) else {
            return;
        };
        let picked = match hand {
            Some(h) => a
                .hands
                .pickup_to(h, item.clone())
                .or_else(|_| a.hands.pickup_any(item.clone())),
            None => a.hands.pickup_any(item.clone()),
        };
        let to_floor = match picked {
            Ok(_) => dropped,
            Err(_) => std::iter::once(item).chain(dropped).collect(),
        };
        for i in &to_floor {
            Self::place(ctx, i, feet);
        }
        self.publish(ctx, actor);
    }

    /// Переложить надетое из слота в слот. Всё или ничего: если у снимаемого есть зависимые слоты с
    /// предметами (они бы выпали) или предмет не подходит к новому слоту, ничего не меняется.
    pub(crate) fn move_slot(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        from: &str,
        to: &str,
    ) {
        let Some(a) = self.actors.get(&actor) else {
            return;
        };
        let Some(item) = a.inventory.get(from).cloned() else {
            return;
        };
        let Some(info) = self.info(&item.prototype).cloned() else {
            return;
        };
        let mut next = a.inventory.clone();
        let Ok((item, dropped)) = next.unequip(from) else {
            return;
        };
        if !dropped.is_empty() || next.equip(to, item, &info).is_err() {
            return;
        }
        self.actor(actor).inventory = next;
        self.publish(ctx, actor);
    }

    /// Дотягивается ли `actor` до `target` (себя — всегда): расстояние между грудью и прямая видимость.
    fn in_reach(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        target: NetworkEntityId,
    ) -> bool {
        if actor == target {
            return true;
        }
        let (Some(a), Some(t)) = (ctx.position_of(actor), ctx.position_of(target)) else {
            return false;
        };
        let (a, t) = (a + Vec3::Y * CHEST_M, t + Vec3::Y * CHEST_M);
        (a - t).length() <= MELEE_RANGE_M && ctx.is_unobstructed(a, t)
    }

    /// Ударить: оружие из активной руки или кулак; темп ограничен `attackRate`.
    pub(crate) fn attack(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        target: NetworkEntityId,
    ) {
        if actor == target || ctx.position_of(target).is_none() {
            return;
        }
        // Состояние создаётся при первом обращении: стороны могли ещё ничего не делать.
        self.actor(target);
        self.actor(actor);
        let Some(a) = self.actors.get(&actor) else {
            return;
        };
        if self.clock < a.next_attack || !self.in_reach(ctx, actor, target) {
            return;
        }
        if self
            .actors
            .get(&target)
            .is_some_and(|t| t.health.state() == MobState::Dead)
        {
            return;
        }
        let weapon = a
            .hands
            .get(a.hands.active())
            .and_then(|i| self.melee.get(&i.prototype))
            .cloned()
            .unwrap_or_else(|| Melee {
                damage: DamageSpec::from([("Blunt".to_string(), FIST_DAMAGE)]),
                rate: DEFAULT_ATTACK_RATE,
            });
        let clock = self.clock;
        self.actor(actor).next_attack = clock + 1.0 / weapon.rate;
        self.actor(target).health.apply(&weapon.damage);
        self.after_health_change(ctx, target);
    }

    /// Лечение: применяет предмет к цели и расходует его.
    fn heal(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        target: NetworkEntityId,
    ) {
        let Some(h) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .and_then(|i| self.healing.get(&i.prototype))
            .cloned()
        else {
            return;
        };
        let _ = self.actor(actor).hands.drop_hand(hand);
        self.actor(target).health.apply(&h.damage);
        self.after_health_change(ctx, target);
        self.publish(ctx, actor);
    }

    /// После любого изменения здоровья: рассылка и переходы состояния. Тот, кто не может действовать,
    /// замораживается, бросает действие и роняет всё из рук; пришедший в себя — свободен.
    fn after_health_change(&mut self, ctx: &mut HookContext<'_>, who: NetworkEntityId) {
        let Some(a) = self.actors.get(&who) else {
            return;
        };
        let (now, before) = (a.health.state(), a.mob_state);
        if now != before {
            self.actor(who).mob_state = now;
            if now.can_act() {
                ctx.freeze(who, false);
            } else {
                ctx.freeze(who, true);
                self.cancel(ctx, who);
                if before.can_act() {
                    let feet = ctx.position_of(who);
                    let dropped = self.actor(who).hands.take_all();
                    if let Some(feet) = feet {
                        for i in &dropped {
                            Self::place(ctx, i, feet);
                        }
                    }
                }
            }
        }
        self.publish(ctx, who);
    }

    /// Начать действие: сразу выполнить, если задержки нет, иначе запустить таймер. Пока идёт
    /// другое действие, новое не принимается.
    pub(crate) fn begin(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        action: Action,
    ) {
        let Some(a) = self.actors.get(&actor) else {
            return;
        };
        if a.doafter.is_some() || !self.can_start(ctx, actor, &action) {
            return;
        }
        let delay = self.delay_of(actor, &action);
        if delay <= 0.0 {
            self.run(ctx, actor, action);
            return;
        }
        let Some(start) = ctx.position_of(actor) else {
            return;
        };
        self.actor(actor).doafter = Some(DoAfter {
            action,
            delay,
            elapsed: 0.0,
            start,
        });
        self.publish_doafter(ctx, actor);
    }

    /// Условия начала и завершения действия (проверяются в обоих случаях).
    fn can_start(&self, ctx: &HookContext<'_>, actor: NetworkEntityId, action: &Action) -> bool {
        let Some(a) = self.actors.get(&actor) else {
            return false;
        };
        match action {
            Action::Equip { hand, slot } => a.hands.get(*hand).is_some_and(|item| {
                self.info(&item.prototype)
                    .is_some_and(|info| a.inventory.can_equip(slot, info).is_ok())
            }),
            Action::Unequip { slot, .. } => a.inventory.get(slot).is_some(),
            Action::Heal { hand, target } => {
                let healable = ctx.position_of(*target).is_some()
                    && self
                        .actors
                        .get(target)
                        .is_none_or(|t| t.health.state() != MobState::Dead);
                let item_ok = a
                    .hands
                    .get(*hand)
                    .is_some_and(|i| self.healing.contains_key(&i.prototype));
                healable && item_ok && self.in_reach(ctx, actor, *target)
            }
            Action::Pry { element, hand } => {
                ctx.door_open(*element) == Some(false)
                    && a.hands.get(*hand).is_some_and(|i| {
                        self.info(&i.prototype)
                            .is_some_and(|x| x.qualities.contains("Prying"))
                    })
            }
        }
    }

    fn delay_of(&self, actor: NetworkEntityId, action: &Action) -> f32 {
        let Some(a) = self.actors.get(&actor) else {
            return 0.0;
        };
        match action {
            Action::Equip { hand, .. } => a
                .hands
                .get(*hand)
                .and_then(|i| self.delays.get(&i.prototype))
                .map_or(0.0, |d| d.0),
            Action::Unequip { slot, .. } => a
                .inventory
                .get(slot)
                .and_then(|i| self.delays.get(&i.prototype))
                .map_or(0.0, |d| d.1),
            Action::Heal { hand, .. } => a
                .hands
                .get(*hand)
                .and_then(|i| self.healing.get(&i.prototype))
                .map_or(0.0, |h| h.delay),
            Action::Pry { .. } => PRY_TIME_S,
        }
    }

    fn run(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId, action: Action) {
        match action {
            Action::Equip { hand, slot } => self.equip(ctx, actor, hand, &slot),
            Action::Unequip { slot, hand } => self.unequip(ctx, actor, &slot, hand),
            Action::Pry { element, .. } => ctx.request_door(element, DoorAction::Open, true),
            Action::Heal { hand, target } => self.heal(ctx, actor, hand, target),
        }
    }

    /// Отменить идущее действие.
    pub(crate) fn cancel(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        if self
            .actors
            .get_mut(&actor)
            .and_then(|a| a.doafter.take())
            .is_some()
        {
            self.publish_doafter(ctx, actor);
        }
    }

    fn publish_doafter(&self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        let view = self
            .actors
            .get(&actor)
            .and_then(|a| a.doafter.as_ref())
            .map(|d| DoAfterView {
                kind: d.action.kind().into(),
                delay_s: d.delay,
            });
        ctx.set_component(actor, DOAFTER_COMPONENT, encode_doafter(view.as_ref()));
    }

    /// Шаг времени: отсчёт, прерывание движением и потерей условий, завершение.
    pub(crate) fn tick(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        self.clock += dt;
        let waiting: Vec<NetworkEntityId> = self
            .actors
            .iter()
            .filter(|(_, a)| a.doafter.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in waiting {
            let Some(pos) = ctx.position_of(id) else {
                self.cancel(ctx, id);
                continue;
            };
            let Some(d) = self.actors.get_mut(&id).and_then(|a| a.doafter.as_mut()) else {
                continue;
            };
            if pos.distance(d.start) > BREAK_DISTANCE_M {
                self.cancel(ctx, id);
                continue;
            }
            d.elapsed += dt;
            if d.elapsed < d.delay {
                continue;
            }
            let action = d.action.clone();
            if let Some(a) = self.actors.get_mut(&id) {
                a.doafter = None;
            }
            self.publish_doafter(ctx, id);
            if self.can_start(ctx, id, &action) {
                self.run(ctx, id, action);
            }
        }
    }

    /// Прототип сумки, если актёр может до неё дотянуться: она в руке, надета или лежит рядом.
    pub(crate) fn bag_proto(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        bag: ObjectId,
    ) -> Option<PrototypeId> {
        let a = self.actors.get(&actor)?;
        if let Some(i) = a
            .hands
            .held()
            .chain(a.inventory.worn())
            .find(|i| i.id == bag)
        {
            return Some(i.prototype.clone());
        }
        let feet = ctx.position_of(actor)?;
        let obj = ctx.world.authored_object(bag)?;
        let AuthoredKind::Placement { prototype } = &obj.kind else {
            return None;
        };
        self.reachable(ctx, feet, obj).then(|| prototype.clone())
    }

    /// Положить предмет из руки в сумку.
    pub(crate) fn store(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        bag: ObjectId,
    ) -> bool {
        let Some(item) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return false;
        };
        if item.id == bag {
            return false;
        }
        let (Some(bag_proto), Some(info)) = (
            self.bag_proto(ctx, actor, bag),
            self.info(&item.prototype).cloned(),
        ) else {
            return false;
        };
        let Some(spec) = self.info(&bag_proto).and_then(|i| i.storage.clone()) else {
            return false;
        };
        let storage = self.storages.entry(bag).or_default();
        if storage.insert(&spec, item, &info).is_err() {
            return false;
        }
        let _ = self.actor(actor).hands.drop_hand(hand);
        self.publish(ctx, actor);
        true
    }

    /// Достать предмет из сумки в свободную руку.
    pub(crate) fn take(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        bag: ObjectId,
        id: ObjectId,
        hand: Option<usize>,
    ) -> bool {
        if self.bag_proto(ctx, actor, bag).is_none() {
            return false;
        }
        let Some(storage) = self.storages.get_mut(&bag) else {
            return false;
        };
        let Some(stored) = storage.items().iter().find(|s| s.item.id == id) else {
            return false;
        };
        let item = stored.item.clone();
        // Сначала место в руках: если его нет, предмет остаётся в сумке.
        let hands = &mut self.actor(actor).hands;
        let taken = match hand {
            Some(h) => hands
                .pickup_to(h, item.clone())
                .or_else(|_| hands.pickup_any(item)),
            None => hands.pickup_any(item),
        };
        if taken.is_err() {
            return false;
        }
        if let Some(s) = self.storages.get_mut(&bag) {
            let _ = s.remove(id);
        }
        self.publish(ctx, actor);
        true
    }
}

/// Урон из данных прототипа: карта «вид → число».
fn damage_spec(d: Option<&prototypes::Data>) -> DamageSpec {
    d.and_then(prototypes::Data::as_map)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32)))
                .collect()
        })
        .unwrap_or_default()
}

/// Этаж, в который попадает высота `y` (ступни стоят на нижней границе).
fn floor_at(ctx: &HookContext<'_>, y: f32) -> Option<FloorId> {
    ctx.world
        .floors()
        .find(|f| y >= f.base_y - 0.5 && y < f.top_y())
        .map(|f| f.id)
}

/// Модуль рук, инвентаря, хранилищ и взаимодействий; логика следует Content.Shared SS14.
///
/// Предметы на земле — авторские объекты мира (`AuthoredKind::Placement`): они реплицируются,
/// сохраняются и валидируются движком. Предметы в руках, на теле и в сумках живут в состоянии
/// модуля; руки видны всем (`build.hands`), инвентарь и содержимое сумок — только владельцу
/// (`build.inventory`). Взаимодействия идут через цепочку обработчиков (`InteractionHandler`):
/// форки добавляют свои, не трогая базовые.
pub struct GameplayModule {
    state: State,
    handlers: Vec<Box<dyn InteractionHandler>>,
}

impl GameplayModule {
    /// `Err`, если два имени прототипов дают один идентификатор движка.
    pub fn new(registry: Arc<PrototypeRegistry>) -> Result<Self, prototypes::ProtoError> {
        Ok(Self {
            state: State::new(registry)?,
            handlers: crate::handlers::default_handlers(),
        })
    }

    #[must_use]
    pub fn with_range(mut self, range_m: f32) -> Self {
        self.state.range_m = range_m;
        self
    }

    /// Добавить обработчик в конец цепочки (после базовых).
    #[must_use]
    pub fn with_handler(mut self, handler: Box<dyn InteractionHandler>) -> Self {
        self.handlers.push(handler);
        self
    }

    pub fn hands_of(&self, actor: NetworkEntityId) -> Option<&Hands> {
        self.state.hands_of(actor)
    }

    pub fn inventory_of(&self, actor: NetworkEntityId) -> Option<&Inventory> {
        self.state.inventory_of(actor)
    }

    pub fn storage_of(&self, bag: ObjectId) -> Option<&Storage> {
        self.state.storage_of(bag)
    }

    /// Цель взаимодействия по идентификатору: предмет на земле или дверь.
    fn resolve_target(&self, ctx: &HookContext<'_>, id: ObjectId) -> Option<Target> {
        if let Some(obj) = ctx.world.authored_object(id)
            && let AuthoredKind::Placement { prototype } = &obj.kind
        {
            return Some(Target::Item {
                id,
                prototype: prototype.clone(),
                at: (obj.min() + obj.max()) * 0.5,
            });
        }
        let e = ctx.world.element(id)?;
        let (StructureKind::Opening(OpeningKind::Door), Placement::Edge(edge)) =
            (e.kind, e.placement)
        else {
            return None;
        };
        let floor = ctx.world.floor(e.floor)?;
        let (a, b) = edge.endpoints(ctx.world.config().grid_step_m, floor.base_y);
        // Точка двери на высоте груди: от неё считаются дальность и видимость.
        Some(Target::Door {
            element: id,
            at: Vec3::new((a.x + b.x) * 0.5, floor.base_y + CHEST_M, (a.z + b.z) * 0.5),
        })
    }

    /// Взаимодействие с целью (`UserInteraction` в SS14): дальность и видимость, затем цепочка
    /// обработчиков — пустая рука или предмет в активной руке.
    fn interact(&mut self, ctx: &mut HookContext<'_>, user: NetworkEntityId, id: ObjectId) {
        let (Some(feet), Some(target)) = (ctx.position_of(user), self.resolve_target(ctx, id))
        else {
            return;
        };
        let chest = feet + Vec3::Y * CHEST_M;
        let at = target.at();
        // Дверь стоит в стене: луч упирается в саму её коробку, поэтому для неё проверяем
        // видимость до точки чуть перед проёмом.
        let visible = match target {
            Target::Door { .. } => {
                ctx.unobstructed_distance(chest, at) >= (at - chest).length() - 0.35
            }
            Target::Item { .. } => ctx.is_unobstructed(chest, at),
        };
        if (at - chest).length() > self.state.range_m || !visible {
            return;
        }
        let used = self.state.used_item(user);
        let mut handlers = std::mem::take(&mut self.handlers);
        match used {
            None => {
                for h in &mut handlers {
                    if h.interact_hand(&mut self.state, ctx, user, &target) {
                        break;
                    }
                }
            }
            Some(used) => {
                let handled = handlers
                    .iter_mut()
                    .any(|h| h.interact_using(&mut self.state, ctx, user, &used, &target));
                if !handled {
                    // Обработчики самого предмета: инструмент в руке «после» взаимодействия.
                    let _ = handlers
                        .iter_mut()
                        .any(|h| h.after_interact(&mut self.state, ctx, user, &used, &target));
                }
            }
        }
        self.handlers = handlers;
    }

    fn use_in_hand(&mut self, ctx: &mut HookContext<'_>, user: NetworkEntityId, hand: usize) {
        let Some(used) = self.state.used_in(user, hand) else {
            return;
        };
        let mut handlers = std::mem::take(&mut self.handlers);
        let _ = handlers
            .iter_mut()
            .any(|h| h.use_in_hand(&mut self.state, ctx, user, &used));
        self.handlers = handlers;
    }
}

impl GameModule for GameplayModule {
    fn id(&self) -> &str {
        "build.gameplay"
    }

    fn handle(&mut self, ctx: &mut HookContext<'_>, event: &HookEvent) {
        let st = &mut self.state;
        match event {
            HookEvent::Command { actor, .. }
                if st
                    .actors
                    .get(actor)
                    .is_some_and(|a| !a.health.state().can_act()) => {}
            HookEvent::Command { actor, kind, data } => match kind.as_str() {
                CMD_ATTACK => {
                    if let Some(target) = parse_attack(data) {
                        st.attack(ctx, *actor, target);
                    }
                }
                CMD_HEAL => {
                    if let Some((hand, target)) = parse_heal(data) {
                        st.begin(
                            ctx,
                            *actor,
                            Action::Heal {
                                hand: usize::from(hand),
                                target,
                            },
                        );
                    }
                }
                CMD_PICKUP | CMD_PICKUP_ANY => {
                    if let Some(id) = parse_object_id(data) {
                        st.pickup(ctx, *actor, id, kind == CMD_PICKUP_ANY);
                    }
                }
                CMD_DROP => {
                    if let Some((hand, target)) = parse_drop(data) {
                        st.drop_hand(ctx, *actor, usize::from(hand), target);
                    }
                }
                CMD_SELECT_HAND => {
                    if let [hand] = data[..]
                        && st.actor(*actor).hands.select(usize::from(hand)).is_ok()
                    {
                        st.publish(ctx, *actor);
                    }
                }
                CMD_MOVE_HAND => {
                    if let [from, to] = data[..]
                        && st
                            .actor(*actor)
                            .hands
                            .exchange(usize::from(from), usize::from(to))
                            .is_ok()
                    {
                        st.publish(ctx, *actor);
                    }
                }
                CMD_SWAP_HANDS => {
                    let reverse = data.first() == Some(&1);
                    st.actor(*actor).hands.swap(reverse);
                    st.publish(ctx, *actor);
                }
                CMD_EQUIP => {
                    if let Some((hand, slot)) = parse_equip(data) {
                        st.begin(
                            ctx,
                            *actor,
                            Action::Equip {
                                hand: usize::from(hand),
                                slot: slot.to_string(),
                            },
                        );
                    }
                }
                CMD_UNEQUIP => {
                    if let Some(slot) = parse_unequip(data) {
                        st.begin(
                            ctx,
                            *actor,
                            Action::Unequip {
                                slot: slot.to_string(),
                                hand: None,
                            },
                        );
                    }
                }
                CMD_CANCEL => st.cancel(ctx, *actor),
                CMD_UNEQUIP_TO => {
                    if let Some((hand, slot)) = parse_equip(data) {
                        st.begin(
                            ctx,
                            *actor,
                            Action::Unequip {
                                slot: slot.to_string(),
                                hand: Some(usize::from(hand)),
                            },
                        );
                    }
                }
                CMD_TAKE_TO => {
                    if let Some((hand, bag, item)) = parse_take_to(data) {
                        st.take(ctx, *actor, bag, item, Some(usize::from(hand)));
                    }
                }
                CMD_MOVE_SLOT => {
                    if let Some((from, to)) = parse_move_slot(data) {
                        st.move_slot(ctx, *actor, from, to);
                    }
                }
                CMD_STORE => {
                    if let Some((hand, bag)) = parse_store(data) {
                        st.store(ctx, *actor, usize::from(hand), bag);
                    }
                }
                CMD_TAKE => {
                    if let Some((bag, item)) = parse_take(data) {
                        st.take(ctx, *actor, bag, item, None);
                    }
                }
                CMD_INTERACT => {
                    if let Some(id) = parse_object_id(data) {
                        self.interact(ctx, *actor, id);
                    }
                }
                CMD_USE => {
                    if let [hand] = data[..] {
                        self.use_in_hand(ctx, *actor, usize::from(hand));
                    }
                }
                _ => {}
            },
            HookEvent::Tick { dt, .. } => st.tick(ctx, *dt),
            HookEvent::PlayerLeft { actor, pos } => {
                // Всё, что игрок держал и носил, остаётся лежать там, где он стоял (DropAll);
                // содержимое сумок остаётся в сумках.
                if let Some(mut a) = st.actors.remove(actor) {
                    for item in a.hands.take_all().into_iter().chain(a.inventory.take_all()) {
                        State::place(ctx, &item, *pos);
                    }
                }
            }
            _ => {}
        }
    }
}
