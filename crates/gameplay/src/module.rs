use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use astrocore_api::{GameModule, HookContext, HookEvent};
use astrocore_core::{NetworkEntityId, ObjectId, PrototypeId};
use astrocore_math::glam::Vec3;
use astrocore_runtime::modules::DoorAction;
use astrocore_world::{
    AuthoredKind, AuthoredObject, FloorId, OpeningKind, Placement, StructureKind, WorldCommand,
};
use prototypes::PrototypeRegistry;

use crate::body::{Body, Environment, VITALS_COMPONENT, Vitals, encode_vitals};
use crate::doafter::{
    Action, BREAK_DISTANCE_M, DOAFTER_COMPONENT, DoAfter, DoAfterView, PRY_TIME_S, encode_doafter,
};
use crate::edibles::{EdibleSpec, openable_closed, solution_template};
use crate::hands::{Hands, HeldItem};
use crate::health::{
    CORPSE_PROTOTYPE, DEFAULT_ATTACK_RATE, DamageSpec, FIST_DAMAGE, GHOST_COMPONENT,
    HEALTH_COMPONENT, Healing, Health, MELEE_RANGE_M, MOBSTATE_COMPONENT, Melee, MobState,
    RESPAWN_DELAY_S, encode_health,
};
use crate::interact::{InteractionHandler, Target, Used};
use crate::inventory::Inventory;
use crate::items::{ItemInfo, item_info};
use crate::reagents::Solution;
use crate::storage::Storage;
use crate::wire::{
    BagView, CMD_ATTACK, CMD_CANCEL, CMD_DROP, CMD_EQUIP, CMD_FEED, CMD_HEAL, CMD_INTERACT,
    CMD_MOVE_HAND, CMD_MOVE_SLOT, CMD_PICKUP, CMD_PICKUP_ANY, CMD_RESPAWN, CMD_SELECT_HAND,
    CMD_STORE, CMD_SWAP_HANDS, CMD_TAKE, CMD_TAKE_TO, CMD_UNEQUIP, CMD_UNEQUIP_TO, CMD_USE,
    HANDS_COMPONENT, INVENTORY_COMPONENT, InventoryView, encode_hands, encode_inventory,
    parse_attack_part, parse_drop, parse_equip, parse_heal, parse_move_slot, parse_object_id,
    parse_store, parse_take, parse_take_to, parse_unequip,
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
    /// Заморожен ли персонаж (чтобы замечать перемены).
    pub(crate) frozen: bool,
    /// Когда в следующий раз разослать здоровье из-за кровотечения или удушья, с.
    pub(crate) next_health_publish: f32,
    /// Здоровье изменилось сидящими внутри процессами, а разослать ещё не успели.
    pub(crate) health_dirty: bool,
    /// Умерший летает призраком; тело осталось лежать на месте смерти.
    pub(crate) ghost: bool,
    /// Когда призрак может возродиться, с (по часам модуля), и где умер.
    pub(crate) respawn_at: f32,
    pub(crate) death_pos: Option<Vec3>,
    /// Физиология: дыхание, желудок, сытость, баротравма.
    pub(crate) body: Option<Body>,
    /// Последнее разосланное состояние тела (чтобы не слать повторы).
    pub(crate) last_vitals: Option<Vitals>,
}

impl Actor {
    fn new(seed: u32) -> Self {
        Self {
            hands: Hands::humanoid(),
            inventory: Inventory::human(),
            doafter: None,
            health: Health::default(),
            mob_state: MobState::Alive,
            next_attack: 0.0,
            frozen: false,
            next_health_publish: 0.0,
            health_dirty: false,
            ghost: false,
            respawn_at: 0.0,
            death_pos: None,
            body: Some(Body::new(seed)),
            last_vitals: None,
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
    /// Сколько применений осталось у начатых стопок лечения (ключ — предмет).
    pub(crate) charges: BTreeMap<ObjectId, u32>,
    /// Куда возрождаются; без точки возрождение недоступно.
    pub(crate) spawn: Option<Vec3>,
    /// Сколько тел оставлено (для идентификаторов).
    pub(crate) corpses: u32,
    /// Атмосфера по этажам.
    pub(crate) atmos: BTreeMap<i32, atmos::Grid>,
    /// Какой множитель скорости уже отправлен движку.
    pub(crate) speed_sent: BTreeMap<NetworkEntityId, f32>,
    pub(crate) edibles: BTreeMap<PrototypeId, EdibleSpec>,
    pub(crate) solution_templates: BTreeMap<PrototypeId, Solution>,
    /// Прототипы, которые открываются и закрываются (бутылки).
    pub(crate) openable: BTreeMap<PrototypeId, bool>,
    /// Растворы предметов (создаются при первом обращении).
    pub(crate) solutions: BTreeMap<ObjectId, Solution>,
    /// Предметы, которые уже открыты.
    pub(crate) opened: BTreeSet<ObjectId>,
}

impl State {
    /// `Err`, если два имени прототипов дают один идентификатор движка.
    pub(crate) fn new(registry: Arc<PrototypeRegistry>) -> Result<Self, prototypes::ProtoError> {
        let names = registry.engine_index(NAMESPACE)?;
        let mut infos = BTreeMap::new();
        let mut delays = BTreeMap::new();
        let mut melee = BTreeMap::new();
        let mut healing = BTreeMap::new();
        let mut edibles = BTreeMap::new();
        let mut solution_templates = BTreeMap::new();
        let mut openable = BTreeMap::new();
        for (id, name) in names {
            let resolved = registry.spawnable(&name).ok();
            if let Some(info) = resolved.as_ref().and_then(item_info) {
                infos.insert(id.clone(), info);
            }
            if let Some(r) = resolved.as_ref() {
                if let Some(e) = EdibleSpec::from_resolved(r) {
                    edibles.insert(id.clone(), e);
                }
                if let Some(s) = solution_template(r) {
                    solution_templates.insert(id.clone(), s);
                }
                if let Some(closed) = openable_closed(r) {
                    openable.insert(id.clone(), closed);
                }
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
                        bleed_modifier: c
                            .get("bloodlossModifier")
                            .and_then(prototypes::Data::as_f64)
                            .map_or(0.0, |v| v as f32),
                        // Размер стопки — `Stack.count`, как у лечащих предметов SS14.
                        uses: resolved
                            .as_ref()
                            .and_then(|r| r.component("Stack"))
                            .and_then(|s| s.get("count"))
                            .and_then(prototypes::Data::as_int)
                            .map_or(1, |n| n.clamp(1, 1000) as u32),
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
            charges: BTreeMap::new(),
            spawn: None,
            corpses: 0,
            atmos: BTreeMap::new(),
            speed_sent: BTreeMap::new(),
            edibles,
            solution_templates,
            openable,
            solutions: BTreeMap::new(),
            opened: BTreeSet::new(),
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
        self.actors
            .entry(id)
            .or_insert_with(|| Actor::new(id.0.wrapping_mul(2_246_822_519) ^ 0x85eb_ca6b))
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
        let mut health_view = a.health.view();
        health_view.ghost = a.ghost;
        health_view.suffocating = a.body.as_ref().is_some_and(|b| b.low_oxygen);
        health_view.respawn_in = if a.ghost {
            (a.respawn_at - self.clock).max(0.0)
        } else {
            0.0
        };
        ctx.set_component(actor, HEALTH_COMPONENT, encode_health(&health_view));
        ctx.set_component(actor, MOBSTATE_COMPONENT, vec![a.health.state().to_byte()]);
        ctx.set_component(actor, GHOST_COMPONENT, vec![u8::from(a.ghost)]);
        if let Some(body) = a.body.as_ref() {
            ctx.set_component(actor, VITALS_COMPONENT, encode_vitals(&body.vitals()));
        }
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
            charges: self.charges.iter().map(|(id, n)| (*id, *n)).collect(),
            fills: a
                .hands
                .held()
                .chain(a.inventory.worn())
                .filter_map(|i| {
                    let t = self.solution_templates.get(&i.prototype)?;
                    let vol = self
                        .solutions
                        .get(&i.id)
                        .map_or_else(|| t.volume(), Solution::volume);
                    Some((i.id, vol, t.max_volume, self.is_closed(i)))
                })
                .collect(),
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
        part: Option<&str>,
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
        let t = self.actor(target);
        let dealt = t
            .health
            .apply_to(Some(part.unwrap_or("Torso")), &weapon.damage);
        if let Some(body) = t.body.as_mut() {
            body.on_damage(&dealt);
        }
        self.after_health_change(ctx, target);
    }

    /// Закрыт ли предмет (бутылка, пока её не открыли).
    pub(crate) fn is_closed(&self, item: &HeldItem) -> bool {
        self.openable.get(&item.prototype).copied().unwrap_or(false)
            && !self.opened.contains(&item.id)
    }

    /// Объём раствора предмета: созданный при первом обращении или шаблон прототипа.
    pub(crate) fn solution_volume(&self, item: &HeldItem) -> f32 {
        self.solutions.get(&item.id).map_or_else(
            || {
                self.solution_templates
                    .get(&item.prototype)
                    .map_or(0.0, Solution::volume)
            },
            Solution::volume,
        )
    }

    /// Раствор предмета, создавая его из шаблона при первом обращении.
    pub(crate) fn solution_mut(&mut self, item: &HeldItem) -> Option<&mut Solution> {
        if !self.solutions.contains_key(&item.id) {
            let t = self.solution_templates.get(&item.prototype)?.clone();
            self.solutions.insert(item.id, t);
        }
        self.solutions.get_mut(&item.id)
    }

    /// Открыть предмет (`OpenableSystem.TryOpen`): `false`, если он не открывается или уже открыт.
    pub(crate) fn open(&mut self, item: &HeldItem) -> bool {
        if !self.is_closed(item) {
            return false;
        }
        self.opened.insert(item.id);
        true
    }

    /// Один укус: часть раствора переходит в желудок, пустая еда исчезает, а напиток остаётся.
    /// Съеденное самому себе продолжается само, пока есть что есть и место в желудке.
    fn bite(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        id: ObjectId,
        target: Option<NetworkEntityId>,
    ) {
        let who = target.unwrap_or(actor);
        let Some(item) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .filter(|i| i.id == id)
            .cloned()
        else {
            return;
        };
        let Some(spec) = self.edibles.get(&item.prototype).cloned() else {
            return;
        };
        let room = self
            .actors
            .get(&who)
            .and_then(|a| a.body.as_ref())
            .map_or(0.0, |b| b.stomach.available());
        let Some(sol) = self.solution_mut(&item) else {
            return;
        };
        let amount = spec.transfer.min(sol.volume()).min(room);
        let split = sol.split(amount);
        let empty = sol.is_empty();
        if let Some(body) = self.actor(who).body.as_mut() {
            let rest = body.swallow(&split);
            debug_assert!(rest.is_empty());
        }
        if empty && spec.destroy_on_empty {
            self.solutions.remove(&item.id);
            self.opened.remove(&item.id);
            let _ = self.actor(actor).hands.drop_hand(hand);
        } else if !empty {
            // Продолжаем есть, пока не кончится или не отменят.
            // Сам себя кормят подряд, пока не кончится; чужого — по одному укусу за раз.
            if target.is_none() {
                self.begin(
                    ctx,
                    actor,
                    Action::Eat {
                        hand,
                        item: id,
                        target: None,
                    },
                );
            }
        }
        self.publish(ctx, actor);
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
        // Один пакет стопки расходуется за применение; последний уходит из руки.
        let id = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .map(|i| i.id);
        if let Some(id) = id {
            let left = self.charges.entry(id).or_insert(h.uses);
            *left = left.saturating_sub(1);
            if *left == 0 {
                self.charges.remove(&id);
                let _ = self.actor(actor).hands.drop_hand(hand);
            }
        }
        let t = self.actor(target);
        t.health.apply(&h.damage);
        if let Some(body) = t.body.as_mut() {
            body.modify_bleed(h.bleed_modifier);
        }
        self.after_health_change(ctx, target);
        self.publish(ctx, actor);
    }

    /// После любого изменения здоровья: рассылка и последствия. Тот, кто не может действовать,
    /// замораживается, бросает действие и роняет всё из рук; с двумя негодными ногами не ходят;
    /// негодная рука выпускает предмет и больше ничего не берёт.
    fn after_health_change(&mut self, ctx: &mut HookContext<'_>, who: NetworkEntityId) {
        let Some(a) = self.actors.get(&who) else {
            return;
        };
        let (now, before) = (a.health.state(), a.mob_state);
        let legs = a.health.part_disabled("LeftLeg") && a.health.part_disabled("RightLeg");
        let died = now == MobState::Dead && before != MobState::Dead;
        let freeze = (!now.can_act() || legs) && !a.ghost && !died;
        let feet = ctx.position_of(who);
        let mut to_floor: Vec<HeldItem> = Vec::new();
        let clock = self.clock;
        {
            let a = self.actor(who);
            a.mob_state = now;
            if died {
                // Смерть: тело остаётся лежать, игрок становится призраком.
                a.ghost = true;
                a.respawn_at = clock + RESPAWN_DELAY_S;
                a.death_pos = feet;
                ctx.set_ghost(who, true);
            }
            if freeze != a.frozen {
                a.frozen = freeze;
                ctx.freeze(who, freeze);
            }
            if !now.can_act() && before.can_act() {
                to_floor.extend(a.hands.take_all());
            }
            let names: Vec<String> = a.hands.iter().map(|(_, n, _)| n.to_string()).collect();
            for (i, name) in names.iter().enumerate() {
                let part = if name.starts_with('l') {
                    "LeftArm"
                } else {
                    "RightArm"
                };
                let off = a.health.part_disabled(part);
                if off != a.hands.is_disabled(i) {
                    a.hands.set_disabled(i, off);
                    if off && let Ok(item) = a.hands.drop_hand(i) {
                        to_floor.push(item);
                    }
                }
            }
        }
        if !now.can_act() {
            self.cancel(ctx, who);
        }
        if let Some(feet) = feet {
            for i in &to_floor {
                Self::place(ctx, i, feet);
            }
            if died {
                self.corpses += 1;
                let id = ObjectId::from_u128(0xC0F5_0000_0000 + u128::from(self.corpses));
                if let Some(floor) = floor_at(ctx, feet.y) {
                    let obj = AuthoredObject::new(
                        id,
                        AuthoredKind::Placement {
                            prototype: PrototypeId::new(CORPSE_PROTOTYPE)
                                .expect("встроенный идентификатор валиден"),
                        },
                        floor,
                        Vec3::new(feet.x - 0.4, feet.y, feet.z - 0.4),
                        Vec3::new(feet.x + 0.4, feet.y + 0.3, feet.z + 0.4),
                    );
                    ctx.request(WorldCommand::PlaceAuthored(obj));
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
            Action::Eat { hand, item, target } => {
                if target.is_some_and(|t| !self.in_reach(ctx, actor, t)) {
                    return false;
                }
                let held = a.hands.get(*hand).filter(|i| i.id == *item);
                let Some(held) = held else { return false };
                let Some(_) = self.edibles.get(&held.prototype) else {
                    return false;
                };
                let closed = self.is_closed(held);
                let has_food = self.solution_volume(held) > 1e-4;
                let room = a
                    .body
                    .as_ref()
                    .is_some_and(|b| b.stomach.available() > 1e-4);
                !closed && has_food && room
            }
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
            Action::Eat { hand, target, .. } => a
                .hands
                .get(*hand)
                .and_then(|i| self.edibles.get(&i.prototype))
                .map_or(0.0, |e| {
                    if target.is_some() {
                        e.force_delay
                    } else {
                        e.delay
                    }
                }),
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
            Action::Eat { hand, item, target } => self.bite(ctx, actor, hand, item, target),
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

    /// Возродиться: призрак по истечении ожидания получает новое тело в точке возрождения. Одежда
    /// старого тела остаётся лежать у места смерти.
    pub(crate) fn respawn(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        let (Some(spawn), Some(a)) = (self.spawn, self.actors.get(&actor)) else {
            return;
        };
        if !a.ghost || self.clock < a.respawn_at {
            return;
        }
        let at = a.death_pos.unwrap_or(spawn);
        let a = self.actor(actor);
        let worn = a.inventory.take_all();
        a.health = Health::default();
        a.body = Some(Body::new(actor.0.wrapping_mul(97) ^ 0x5bd1_e995));
        a.last_vitals = None;
        a.mob_state = MobState::Alive;
        a.ghost = false;
        a.frozen = false;
        a.doafter = None;
        a.next_attack = 0.0;
        a.death_pos = None;
        for h in 0..a.hands.len() {
            a.hands.set_disabled(h, false);
        }
        for i in &worn {
            Self::place(ctx, i, at);
        }
        ctx.set_ghost(actor, false);
        ctx.freeze(actor, false);
        ctx.teleport(actor, spawn);
        self.publish_doafter(ctx, actor);
        self.publish(ctx, actor);
    }

    /// Атмосфера и тело: обновить геометрию, продвинуть газ, дать каждому персонажу подышать,
    /// переварить съеденное и проверить давление. Кровотечение идёт тут же. Здоровье рассылается не
    /// чаще двух раз в секунду, но конец процесса — сразу, чтобы клиент не застрял на старом числе.
    fn tick_health(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        self.tick_atmosphere(ctx, dt);
        let ids: Vec<NetworkEntityId> = ctx.players().collect();
        for id in ids {
            let Some(pos) = ctx.position_of(id) else {
                continue;
            };
            let floor = floor_at(ctx, pos.y).map(|f| f.0);
            let cell = (pos.x.floor() as i32, pos.z.floor() as i32);
            let clock = self.clock;
            self.actor(id);
            let Some(mut body) = self.actors.get_mut(&id).and_then(|a| a.body.take()) else {
                continue;
            };
            let (mob, barotrauma) = {
                let a = &self.actors[&id];
                (
                    a.health.state(),
                    a.health.damage_of("Blunt") + a.health.damage_of("Heat"),
                )
            };
            let a_ghost = self.actors[&id].ghost;
            let outcome = {
                let grid = floor.and_then(|f| self.atmos.get_mut(&f));
                let mut env = ActorEnv { grid, cell };
                body.tick(dt, &mut env, mob, barotrauma)
            };
            let vitals = body.vitals();
            let speed = if a_ghost { 1.0 } else { vitals.speed };
            if (speed - self.speed_sent.get(&id).copied().unwrap_or(1.0)).abs() > 1e-4 {
                self.speed_sent.insert(id, speed);
                ctx.set_speed(id, speed);
            }
            let a = self.actor(id);
            let dealt = a.health.apply(&outcome.damage);
            body.on_damage(&dealt);
            let vitals = body.vitals();
            let vitals_changed = a
                .last_vitals
                .as_ref()
                .is_none_or(|v| vitals_differ(v, &vitals));
            a.body = Some(body);
            let changed = outcome.changed;
            a.health_dirty |= changed;
            let due = clock >= a.next_health_publish;
            if (a.health_dirty && (due || !changed))
                || a.health.state() != a.mob_state
                || vitals_changed
            {
                a.health_dirty = false;
                a.next_health_publish = clock + 0.5;
                a.last_vitals = Some(vitals);
                self.after_health_change(ctx, id);
            }
        }
    }

    /// Геометрия и ход газа: при любом изменении стен или дверей сетка перестраивается, воздух
    /// уцелевших клеток сохраняется.
    fn tick_atmosphere(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        let layouts = crate::world_atmos::layouts(ctx.world, &|id| ctx.door_open(id) == Some(true));
        self.atmos.retain(|f, _| layouts.contains_key(f));
        for (floor, layout) in layouts {
            let grid = self.atmos.entry(floor).or_default();
            if grid.layout() != &layout {
                grid.set_layout(layout);
            }
            grid.advance(dt);
        }
    }

    /// Шаг времени: отсчёт, прерывание движением и потерей условий, завершение.
    pub(crate) fn tick(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        self.clock += dt;
        self.tick_health(ctx, dt);
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
            if d.action.breaks_on_move() && pos.distance(d.start) > BREAK_DISTANCE_M {
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

/// Воздух под ногами персонажа: клетка сетки своего этажа. Вне сетки (в пустоте) дышать нечем.
struct ActorEnv<'a> {
    grid: Option<&'a mut atmos::Grid>,
    cell: (i32, i32),
}

impl Environment for ActorEnv<'_> {
    fn take_breath(&mut self, litres: f32) -> Option<atmos::GasMixture> {
        let cell = self.cell;
        self.grid.as_mut()?.remove_volume(cell, litres)
    }

    fn give_back(&mut self, gas: &atmos::GasMixture) {
        let cell = self.cell;
        if let Some(g) = self.grid.as_mut() {
            g.merge_into(cell, gas);
        }
    }

    fn pressure(&self) -> f32 {
        self.grid.as_ref().map_or(0.0, |g| g.pressure(self.cell))
    }
}

/// Достаточно ли изменилось состояние тела, чтобы слать его заново.
fn vitals_differ(a: &Vitals, b: &Vitals) -> bool {
    a.hunger.round() != b.hunger.round()
        || a.thirst.round() != b.thirst.round()
        || a.hunger_alert != b.hunger_alert
        || a.thirst_alert != b.thirst_alert
        || a.pressure_alert != b.pressure_alert
        || a.pressure_kpa.round() != b.pressure_kpa.round()
        || a.low_oxygen != b.low_oxygen
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

    /// Точка возрождения (ступни); без неё призраки не возрождаются.
    #[must_use]
    pub fn with_spawn(mut self, spawn: Vec3) -> Self {
        self.state.spawn = Some(spawn);
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
            HookEvent::Command { actor, kind, .. } if kind == CMD_RESPAWN => {
                st.respawn(ctx, *actor);
            }
            HookEvent::Command { actor, .. }
                if st
                    .actors
                    .get(actor)
                    .is_some_and(|a| !a.health.state().can_act()) => {}
            HookEvent::Command { actor, kind, data } => match kind.as_str() {
                CMD_ATTACK => {
                    if let Some((target, part)) = parse_attack_part(data) {
                        st.attack(ctx, *actor, target, part);
                    }
                }
                CMD_FEED => {
                    if let Some((hand, target)) = parse_heal(data)
                        && target != *actor
                    {
                        if let Some(it) = st
                            .actors
                            .get(actor)
                            .and_then(|a| a.hands.get(usize::from(hand)))
                            .cloned()
                        {
                            st.begin(
                                ctx,
                                *actor,
                                Action::Eat {
                                    hand: usize::from(hand),
                                    item: it.id,
                                    target: Some(target),
                                },
                            );
                        }
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
