use std::collections::BTreeMap;
use std::sync::Arc;

use astrocore_api::{GameModule, HookContext, HookEvent};
use astrocore_core::{NetworkEntityId, ObjectId};
use astrocore_math::glam::Vec3;
use astrocore_world::{AuthoredKind, AuthoredObject, FloorId, WorldCommand};
use prototypes::PrototypeRegistry;

use crate::hands::{Hands, HeldItem};
use crate::wire::{
    CMD_DROP, CMD_PICKUP, CMD_SELECT_HAND, HANDS_COMPONENT, encode_hands, parse_object_id,
};

/// Дальность взаимодействия по умолчанию, м (от груди персонажа до центра предмета).
pub const DEFAULT_REACH_M: f32 = 2.0;
/// Высота груди над ступнями, м.
const CHEST_M: f32 = 1.0;
/// Размер выброшенного предмета в мире, м.
const DROPPED_SIZE_M: f32 = 0.3;

/// Игровой модуль рук и предметов. Предметы на земле — авторские объекты мира
/// (`AuthoredKind::Placement`), поэтому они реплицируются, сохраняются и валидируются движком;
/// предметы в руках живут в состоянии модуля и реплицируются как `build.hands`.
pub struct GameplayModule {
    registry: Arc<PrototypeRegistry>,
    names: BTreeMap<astrocore_core::PrototypeId, String>,
    reach_m: f32,
    hands: BTreeMap<NetworkEntityId, Hands>,
}

/// Пространство имён идентификаторов прототипов билда в движке.
pub const NAMESPACE: &str = "ss14";

impl GameplayModule {
    /// `Err`, если два имени прототипов дают один идентификатор движка.
    pub fn new(registry: Arc<PrototypeRegistry>) -> Result<Self, prototypes::ProtoError> {
        let names = registry.engine_index(NAMESPACE)?;
        Ok(Self {
            registry,
            names,
            reach_m: DEFAULT_REACH_M,
            hands: BTreeMap::new(),
        })
    }

    #[must_use]
    pub fn with_reach(mut self, reach_m: f32) -> Self {
        self.reach_m = reach_m;
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

    fn pickup(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId, id: ObjectId) {
        let (Some(pos), Some(obj)) = (ctx.position_of(actor), ctx.world.authored_object(id)) else {
            return;
        };
        let AuthoredKind::Placement { prototype } = &obj.kind else {
            return;
        };
        if !self.is_item(prototype) {
            return;
        }
        let center = (obj.min() + obj.max()) * 0.5;
        if (center - (pos + Vec3::Y * CHEST_M)).length() > self.reach_m {
            return;
        }
        let item = HeldItem {
            id,
            prototype: prototype.clone(),
        };
        let hands = self.hands.entry(actor).or_insert_with(Hands::humanoid);
        if hands.pickup(item).is_err() {
            return;
        }
        ctx.request(WorldCommand::RemoveAuthored { id });
        self.publish(ctx, actor);
    }

    /// Положить предмет в мир у ног персонажа. Возвращает `false`, если места (этажа) нет.
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

    fn drop_hand(&mut self, ctx: &mut HookContext<'_>, actor: NetworkEntityId, hand: usize) {
        let (Some(pos), Some(hands)) = (ctx.position_of(actor), self.hands.get_mut(&actor)) else {
            return;
        };
        let Some(item) = hands.get(hand).cloned() else {
            return;
        };
        if Self::place(ctx, &item, pos) {
            let _ = hands.drop_hand(hand);
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
                CMD_PICKUP => {
                    if let Some(id) = parse_object_id(data) {
                        self.pickup(ctx, *actor, id);
                    }
                }
                CMD_DROP => {
                    if let [hand] = data[..] {
                        self.drop_hand(ctx, *actor, usize::from(hand));
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
                _ => {}
            },
            HookEvent::PlayerLeft { actor, pos } => {
                // Всё, что игрок держал, остаётся лежать там, где он стоял.
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
