use std::collections::BTreeMap;
use std::sync::Arc;

use astrocore_api::{GameModule, HookContext, HookEvent};
use astrocore_core::{NetworkEntityId, ObjectId, PrototypeId};
use astrocore_math::glam::Vec3;
use astrocore_world::{AuthoredKind, AuthoredObject, FloorId, WorldCommand};
use prototypes::PrototypeRegistry;

use crate::hands::{Hands, HeldItem};
use crate::inventory::Inventory;
use crate::items::{ItemInfo, item_info};
use crate::storage::Storage;
use crate::wire::{
    BagView, CMD_DROP, CMD_EQUIP, CMD_PICKUP, CMD_PICKUP_ANY, CMD_SELECT_HAND, CMD_STORE,
    CMD_SWAP_HANDS, CMD_TAKE, CMD_UNEQUIP, HANDS_COMPONENT, INVENTORY_COMPONENT, InventoryView,
    encode_hands, encode_inventory, parse_drop, parse_equip, parse_object_id, parse_store,
    parse_take, parse_unequip,
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
struct Actor {
    hands: Hands,
    inventory: Inventory,
}

impl Actor {
    fn new() -> Self {
        Self {
            hands: Hands::humanoid(),
            inventory: Inventory::human(),
        }
    }
}

/// Игровой модуль рук, инвентаря и хранилищ; логика следует Content.Shared SS14.
///
/// Предметы на земле — авторские объекты мира (`AuthoredKind::Placement`): они реплицируются,
/// сохраняются и валидируются движком. Предметы в руках, на теле и в сумках живут в состоянии
/// модуля; руки видны всем (`build.hands`), инвентарь и содержимое сумок — только владельцу
/// (`build.inventory`).
pub struct GameplayModule {
    infos: BTreeMap<PrototypeId, ItemInfo>,
    range_m: f32,
    actors: BTreeMap<NetworkEntityId, Actor>,
    /// Содержимое сумок по идентификатору сумки: переживает смену владельца и выброс сумки.
    storages: BTreeMap<ObjectId, Storage>,
}

impl GameplayModule {
    /// `Err`, если два имени прототипов дают один идентификатор движка.
    pub fn new(registry: Arc<PrototypeRegistry>) -> Result<Self, prototypes::ProtoError> {
        let names = registry.engine_index(NAMESPACE)?;
        let mut infos = BTreeMap::new();
        for (id, name) in names {
            if let Some(info) = registry.spawnable(&name).ok().as_ref().and_then(item_info) {
                infos.insert(id, info);
            }
        }
        Ok(Self {
            infos,
            range_m: INTERACTION_RANGE_M,
            actors: BTreeMap::new(),
            storages: BTreeMap::new(),
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

    fn actor(&mut self, id: NetworkEntityId) -> &mut Actor {
        self.actors.entry(id).or_insert_with(Actor::new)
    }

    fn info(&self, proto: &PrototypeId) -> Option<&ItemInfo> {
        self.infos.get(proto)
    }

    /// Рассылка состояния: руки всем, инвентарь с содержимым сумок — владельцу.
    fn publish(&self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        let Some(a) = self.actors.get(&actor) else {
            return;
        };
        ctx.set_component(actor, HANDS_COMPONENT, encode_hands(&a.hands));
        let bags = a
            .hands
            .held()
            .chain(a.inventory.worn())
            .filter(|i| self.info(&i.prototype).is_some_and(|x| x.storage.is_some()))
            .map(|i| BagView {
                id: i.id,
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
    fn reachable(&self, ctx: &HookContext<'_>, feet: Vec3, obj: &AuthoredObject) -> bool {
        let chest = feet + Vec3::Y * CHEST_M;
        let center = (obj.min() + obj.max()) * 0.5;
        (center - chest).length() <= self.range_m && ctx.is_unobstructed(chest, center)
    }

    /// Подбор. `any = false` — клик (только активная рука), `true` — глагол «поднять».
    fn pickup(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        id: ObjectId,
        any: bool,
    ) {
        let (Some(pos), Some(obj)) = (ctx.position_of(actor), ctx.world.authored_object(id)) else {
            return;
        };
        let AuthoredKind::Placement { prototype } = &obj.kind else {
            return;
        };
        if self.info(prototype).is_none() || !self.reachable(ctx, pos, obj) {
            return;
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
            return;
        }
        ctx.request(WorldCommand::RemoveAuthored { id });
        self.publish(ctx, actor);
    }

    /// Положить предмет в мир в точке `at` (ступни предмета).
    fn place(ctx: &mut HookContext<'_>, item: &HeldItem, at: Vec3) -> bool {
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
    fn drop_point(&self, ctx: &HookContext<'_>, feet: Vec3, target: Option<[f32; 3]>) -> Vec3 {
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

    fn drop_hand(
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
    fn equip(
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
    fn unequip(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId, slot: &str) {
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
        let to_floor = match a.hands.pickup_any(item.clone()) {
            Ok(_) => dropped,
            Err(_) => std::iter::once(item).chain(dropped).collect(),
        };
        for i in &to_floor {
            Self::place(ctx, i, feet);
        }
        self.publish(ctx, actor);
    }

    /// Прототип сумки, если актёр может до неё дотянуться: она в руке, надета или лежит рядом.
    fn bag_proto(
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
    fn store(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        bag: ObjectId,
    ) {
        let Some(item) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return;
        };
        if item.id == bag {
            return;
        }
        let (Some(bag_proto), Some(info)) = (
            self.bag_proto(ctx, actor, bag),
            self.info(&item.prototype).cloned(),
        ) else {
            return;
        };
        let Some(spec) = self.info(&bag_proto).and_then(|i| i.storage.clone()) else {
            return;
        };
        let storage = self.storages.entry(bag).or_default();
        if storage.insert(&spec, item, &info).is_err() {
            return;
        }
        let _ = self.actor(actor).hands.drop_hand(hand);
        self.publish(ctx, actor);
    }

    /// Достать предмет из сумки в свободную руку.
    fn take(
        &mut self,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        bag: ObjectId,
        id: ObjectId,
    ) {
        if self.bag_proto(ctx, actor, bag).is_none() {
            return;
        }
        let Some(storage) = self.storages.get_mut(&bag) else {
            return;
        };
        let Some(stored) = storage.items().iter().find(|s| s.item.id == id) else {
            return;
        };
        let item = stored.item.clone();
        // Сначала место в руках: если его нет, предмет остаётся в сумке.
        if self.actor(actor).hands.pickup_any(item).is_err() {
            return;
        }
        if let Some(s) = self.storages.get_mut(&bag) {
            let _ = s.remove(id);
        }
        self.publish(ctx, actor);
    }
}

/// Этаж, в который попадает высота `y` (ступни стоят на нижней границе).
fn floor_at(ctx: &HookContext<'_>, y: f32) -> Option<FloorId> {
    ctx.world
        .floors()
        .find(|f| y >= f.base_y - 0.5 && y < f.top_y())
        .map(|f| f.id)
}

impl GameModule for GameplayModule {
    fn id(&self) -> &str {
        "build.gameplay"
    }

    fn handle(&mut self, ctx: &mut HookContext<'_>, event: &HookEvent) {
        match event {
            HookEvent::Command { actor, kind, data } => match kind.as_str() {
                CMD_PICKUP | CMD_PICKUP_ANY => {
                    if let Some(id) = parse_object_id(data) {
                        self.pickup(ctx, *actor, id, kind == CMD_PICKUP_ANY);
                    }
                }
                CMD_DROP => {
                    if let Some((hand, target)) = parse_drop(data) {
                        self.drop_hand(ctx, *actor, usize::from(hand), target);
                    }
                }
                CMD_SELECT_HAND => {
                    if let [hand] = data[..]
                        && self.actor(*actor).hands.select(usize::from(hand)).is_ok()
                    {
                        self.publish(ctx, *actor);
                    }
                }
                CMD_SWAP_HANDS => {
                    let reverse = data.first() == Some(&1);
                    self.actor(*actor).hands.swap(reverse);
                    self.publish(ctx, *actor);
                }
                CMD_EQUIP => {
                    if let Some((hand, slot)) = parse_equip(data) {
                        self.equip(ctx, *actor, usize::from(hand), slot);
                    }
                }
                CMD_UNEQUIP => {
                    if let Some(slot) = parse_unequip(data) {
                        self.unequip(ctx, *actor, slot);
                    }
                }
                CMD_STORE => {
                    if let Some((hand, bag)) = parse_store(data) {
                        self.store(ctx, *actor, usize::from(hand), bag);
                    }
                }
                CMD_TAKE => {
                    if let Some((bag, item)) = parse_take(data) {
                        self.take(ctx, *actor, bag, item);
                    }
                }
                _ => {}
            },
            HookEvent::PlayerLeft { actor, pos } => {
                // Всё, что игрок держал и носил, остаётся лежать там, где он стоял (DropAll);
                // содержимое сумок остаётся в сумках.
                if let Some(mut a) = self.actors.remove(actor) {
                    for item in a.hands.take_all().into_iter().chain(a.inventory.take_all()) {
                        Self::place(ctx, &item, *pos);
                    }
                }
            }
            _ => {}
        }
    }
}
