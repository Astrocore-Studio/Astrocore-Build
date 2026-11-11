use std::collections::BTreeMap;
use std::sync::Arc;

use astrocore_api::{GameModule, HookContext, HookEvent};
use astrocore_core::{NetworkEntityId, ObjectId};
use astrocore_math::glam::Vec3;
use astrocore_world::{AuthoredKind, AuthoredObject, FloorId, WorldCommand};
use prototypes::PrototypeRegistry;

use crate::hands::{Hands, HeldItem};
use crate::wire::{
    CMD_DROP, CMD_PICKUP, CMD_PICKUP_ANY, CMD_SELECT_HAND, CMD_SWAP_HANDS, HANDS_COMPONENT,
    encode_hands, parse_drop, parse_object_id,
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

/// Игровой модуль рук и предметов; логика следует Content.Shared/Hands SS14.
/// Предметы на земле — авторские объекты мира (`AuthoredKind::Placement`), поэтому они
/// реплицируются, сохраняются и валидируются движком; предметы в руках живут в состоянии модуля
/// и реплицируются как `build.hands`.
pub struct GameplayModule {
    registry: Arc<PrototypeRegistry>,
    names: BTreeMap<astrocore_core::PrototypeId, String>,
    range_m: f32,
    hands: BTreeMap<NetworkEntityId, Hands>,
}

impl GameplayModule {
    /// `Err`, если два имени прототипов дают один идентификатор движка.
    pub fn new(registry: Arc<PrototypeRegistry>) -> Result<Self, prototypes::ProtoError> {
        let names = registry.engine_index(NAMESPACE)?;
        Ok(Self {
            registry,
            names,
            range_m: INTERACTION_RANGE_M,
            hands: BTreeMap::new(),
        })
    }

    #[must_use]
    pub fn with_range(mut self, range_m: f32) -> Self {
        self.range_m = range_m;
        self
    }

    pub fn hands_of(&self, actor: NetworkEntityId) -> Option<&Hands> {
        self.hands.get(&actor)
    }

    /// Предмет можно брать в руки, если его прототип создаваем и имеет компонент `Item`.
    fn is_item(&self, proto: &astrocore_core::PrototypeId) -> bool {
        self.names
            .get(proto)
            .and_then(|name| self.registry.spawnable(name).ok())
            .is_some_and(|r| r.component("Item").is_some())
    }

    fn publish(&self, ctx: &mut HookContext<'_>, actor: NetworkEntityId) {
        if let Some(h) = self.hands.get(&actor) {
            ctx.set_component(actor, HANDS_COMPONENT, encode_hands(h));
        }
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
        if !self.is_item(prototype) {
            return;
        }
        let chest = pos + Vec3::Y * CHEST_M;
        let center = (obj.min() + obj.max()) * 0.5;
        // В зоне досягаемости и без преград между грудью и предметом.
        if (center - chest).length() > self.range_m || !ctx.is_unobstructed(chest, center) {
            return;
        }
        let item = HeldItem {
            id,
            prototype: prototype.clone(),
        };
        let hands = self.hands.entry(actor).or_insert_with(Hands::humanoid);
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
        // Если упёрлись в преграду — отступаем от неё; иначе предмет ложится в точку цели.
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
        let (Some(feet), Some(hands)) = (ctx.position_of(actor), self.hands.get(&actor)) else {
            return;
        };
        let Some(item) = hands.get(hand).cloned() else {
            return;
        };
        let at = self.drop_point(ctx, feet, target);
        if Self::place(ctx, &item, at) {
            if let Some(h) = self.hands.get_mut(&actor) {
                let _ = h.drop_hand(hand);
            }
            self.publish(ctx, actor);
        }
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
                    if let [hand] = data[..] {
                        let hands = self.hands.entry(*actor).or_insert_with(Hands::humanoid);
                        if hands.select(usize::from(hand)).is_ok() {
                            self.publish(ctx, *actor);
                        }
                    }
                }
                CMD_SWAP_HANDS => {
                    let reverse = data.first() == Some(&1);
                    let hands = self.hands.entry(*actor).or_insert_with(Hands::humanoid);
                    hands.swap(reverse);
                    self.publish(ctx, *actor);
                }
                _ => {}
            },
            HookEvent::PlayerLeft { actor, pos } => {
                // Всё, что игрок держал, остаётся лежать там, где он стоял (DropAll).
                if let Some(mut hands) = self.hands.remove(actor) {
                    for item in hands.take_all() {
                        Self::place(ctx, &item, *pos);
                    }
                }
            }
            _ => {}
        }
    }
}
