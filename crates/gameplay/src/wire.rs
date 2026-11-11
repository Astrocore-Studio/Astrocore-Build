use astrocore_api::{
    Interest, ReplicatedKind, ReplicationChannel, ReplicationError, ReplicationRegistry,
};
use astrocore_core::ObjectId;

use crate::hands::Hands;
use crate::inventory::Inventory;
use crate::storage::Stored;
use serde::{Deserialize, Serialize};

/// Команды клиента.
/// Клик по предмету: только в активную руку (как `TryPickup` в SS14).
pub const CMD_PICKUP: &str = "item.pickup";
/// Глагол «поднять»: в первую свободную руку, начиная с активной (`TryPickupAnyHand`).
pub const CMD_PICKUP_ANY: &str = "item.pickup_any";
/// Смена активной руки по кругу; данные — `[1]` для обратного направления.
pub const CMD_SWAP_HANDS: &str = "hands.swap";
pub const CMD_DROP: &str = "item.drop";
pub const CMD_SELECT_HAND: &str = "hands.select";
/// Переложить предмет между руками (в пустую или с обменом): `[откуда, куда]`.
pub const CMD_MOVE_HAND: &str = "hands.move";

/// Надеть предмет из руки в слот: `[рука, имя слота utf-8]`.
pub const CMD_EQUIP: &str = "inv.equip";
/// Снять предмет из слота: `[имя слота utf-8]`; предмет идёт в свободную руку, иначе на пол.
pub const CMD_UNEQUIP: &str = "inv.unequip";
/// Положить предмет из руки в сумку: `[рука, id сумки 16 байт]`.
pub const CMD_STORE: &str = "storage.insert";
/// Достать предмет из сумки в свободную руку: `[id сумки 16 байт, id предмета 16 байт]`.
pub const CMD_TAKE: &str = "storage.take";
/// Снять предмет в указанную руку: `[рука, имя слота]`; рука занята — в первую свободную.
pub const CMD_UNEQUIP_TO: &str = "inv.unequip_to";
/// Достать предмет из сумки в указанную руку: `[рука, id сумки, id предмета]`.
pub const CMD_TAKE_TO: &str = "storage.take_to";
/// Переложить надетое из слота в слот: `[слот откуда, 0, слот куда]`.
pub const CMD_MOVE_SLOT: &str = "inv.move";
/// Отменить идущее действие с таймером.
pub const CMD_CANCEL: &str = "doafter.cancel";
/// Призраку: возродиться в точке появления, когда истекло время ожидания.
pub const CMD_RESPAWN: &str = "ghost.respawn";
/// Ударить персонажа в пределах досягаемости: `[сетевой id цели, 4 байта LE]`. Оружие — предмет в
/// активной руке (`MeleeWeapon`), без него кулак.
pub const CMD_ATTACK: &str = "combat.attack";
/// Применить лечащий предмет из руки на персонажа: `[рука, сетевой id цели, 4 байта LE]`.
pub const CMD_HEAL: &str = "combat.heal";
/// Накормить или напоить другого персонажа предметом из руки: `[рука, сетевой id цели, 4 байта LE]`.
pub const CMD_FEED: &str = "feed.other";

/// Вылить содержимое предмета из руки на пол под собой: `[рука]`.
pub const CMD_SPILL: &str = "item.spill";

/// Применить предмет из руки к точке на полу (швабра по луже, по ведру): `[рука, x, y, z]`.
pub const CMD_USE_GROUND: &str = "use.ground";

/// Сбить огонь на себе (перекат): без данных. Тот же эффект у щелчка по значку огня.
pub const CMD_RESIST: &str = "fire.resist";

/// Взаимодействие с целью в мире (предмет на земле или дверь): `[id цели 16 байт]`.
/// Что именно произойдёт, решает цепочка обработчиков: пустая рука — `InteractHand`, предмет в
/// активной руке — `InteractUsing` цели, затем `AfterInteract` предмета (как в SS14).
pub const CMD_INTERACT: &str = "interact";
/// Использовать предмет в руке (`UseInHand`): `[рука]`.
pub const CMD_USE: &str = "use";

/// Состояние инвентаря и содержимое сумок игрока; видит только сам игрок.
pub const INVENTORY_COMPONENT: &str = "build.inventory";
pub const INVENTORY_MAX_BYTES: usize = 16 * 1024;

/// Реплицируемое состояние рук игрока.
pub const HANDS_COMPONENT: &str = "build.hands";

/// Что надето на игроке, как его видят все: пары (слот, прототип). Нужно для внешнего вида.
pub const WORN_COMPONENT: &str = "build.worn";
pub const WORN_MAX_BYTES: usize = 2048;

/// Надетое для рассылки: (имя слота, идентификатор прототипа).
pub type Worn = Vec<(String, String)>;

pub fn encode_worn(w: &Worn) -> Vec<u8> {
    serde_json::to_vec(w).unwrap_or_default()
}

pub fn decode_worn(bytes: &[u8]) -> Option<Worn> {
    if bytes.len() > WORN_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Предел размера состояния рук в сообщении (двенадцать рук с длинными идентификаторами).
pub const HANDS_MAX_BYTES: usize = 4096;

pub fn pickup_data(item: ObjectId) -> Vec<u8> {
    item.as_uuid().as_u128().to_be_bytes().to_vec()
}

/// Выбросить предмет из руки под ноги.
pub fn drop_data(hand: u8) -> Vec<u8> {
    vec![hand]
}

/// Выбросить предмет из руки в точку мира (дальность и препятствия учитывает сервер).
pub fn drop_at_data(hand: u8, target: [f32; 3]) -> Vec<u8> {
    let mut v = vec![hand];
    for c in target {
        v.extend_from_slice(&c.to_le_bytes());
    }
    v
}

/// Разобрать команду выброса: рука и необязательная цель; нечисловые координаты отвергаются.
pub fn parse_drop(data: &[u8]) -> Option<(u8, Option<[f32; 3]>)> {
    match data {
        [hand] => Some((*hand, None)),
        [hand, rest @ ..] if rest.len() == 12 => {
            let mut t = [0.0f32; 3];
            for (slot, c) in t.iter_mut().zip(rest.as_chunks::<4>().0) {
                *slot = f32::from_le_bytes(*c);
            }
            t.iter().all(|v| v.is_finite()).then_some((*hand, Some(t)))
        }
        _ => None,
    }
}

pub fn parse_object_id(data: &[u8]) -> Option<ObjectId> {
    let bytes: [u8; 16] = data.try_into().ok()?;
    Some(ObjectId::from_u128(u128::from_be_bytes(bytes)))
}

pub fn encode_hands(h: &Hands) -> Vec<u8> {
    serde_json::to_vec(h).unwrap_or_default()
}

/// Разобрать состояние рук, полученное от сервера (данные недоверенные).
pub fn decode_hands(bytes: &[u8]) -> Option<Hands> {
    if bytes.len() > HANDS_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Зарегистрировать виды данных билда на сервере.
pub fn register_replication(reg: &mut ReplicationRegistry) -> Result<(), ReplicationError> {
    reg.register(ReplicatedKind {
        name: HANDS_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Everyone,
        max_bytes: HANDS_MAX_BYTES,
    })?;
    reg.register(ReplicatedKind {
        name: INVENTORY_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Owner,
        max_bytes: INVENTORY_MAX_BYTES,
    })?;
    reg.register(ReplicatedKind {
        name: crate::doafter::DOAFTER_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Everyone,
        max_bytes: crate::doafter::DOAFTER_MAX_BYTES,
    })?;
    reg.register(ReplicatedKind {
        name: crate::health::HEALTH_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Owner,
        max_bytes: crate::health::HEALTH_MAX_BYTES,
    })?;
    reg.register(ReplicatedKind {
        name: crate::electric::LIGHTS_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Everyone,
        max_bytes: crate::electric::LIGHTS_MAX_BYTES,
    })?;
    reg.register(ReplicatedKind {
        name: crate::puddles::PUDDLES_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Everyone,
        max_bytes: crate::puddles::PUDDLES_MAX_BYTES,
    })?;
    reg.register(ReplicatedKind {
        name: crate::fire::FIRES_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Everyone,
        max_bytes: crate::fire::FIRES_MAX_BYTES,
    })?;
    reg.register(ReplicatedKind {
        name: crate::fire::BURNING_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Everyone,
        max_bytes: 4,
    })?;
    reg.register(ReplicatedKind {
        name: crate::health::MOBSTATE_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Everyone,
        max_bytes: 4,
    })?;
    reg.register(ReplicatedKind {
        name: crate::health::GHOST_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Everyone,
        max_bytes: 4,
    })?;
    reg.register(ReplicatedKind {
        name: crate::body::VITALS_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Owner,
        max_bytes: crate::body::VITALS_MAX_BYTES,
    })?;
    reg.register(ReplicatedKind {
        name: WORN_COMPONENT.into(),
        channel: ReplicationChannel::ReliableOrdered,
        interest: Interest::Everyone,
        max_bytes: WORN_MAX_BYTES,
    })
}

/// Содержимое одной сумки в состоянии инвентаря.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BagView {
    pub id: ObjectId,
    /// Клетки сетки сумки (может быть не прямоугольником).
    pub grid: Vec<(i32, i32)>,
    pub items: Vec<Stored>,
}

/// Состояние инвентаря игрока: слоты и сумки, которые он держит или носит.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventoryView {
    pub inventory: Inventory,
    pub bags: Vec<BagView>,
    /// Остаток применений у начатых стопок лечения (предмет, сколько осталось).
    #[serde(default)]
    pub charges: Vec<(ObjectId, u32)>,
    /// Растворы предметов у персонажа: (предмет, объём, предел, закрыт ли).
    #[serde(default)]
    pub fills: Vec<(ObjectId, f32, f32, bool)>,
}

pub fn encode_inventory(v: &InventoryView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_inventory(bytes: &[u8]) -> Option<InventoryView> {
    if bytes.len() > INVENTORY_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

fn id_bytes(id: ObjectId) -> [u8; 16] {
    id.as_uuid().as_u128().to_be_bytes()
}

pub fn move_hand_data(from: u8, to: u8) -> Vec<u8> {
    vec![from, to]
}

pub fn interact_data(target: ObjectId) -> Vec<u8> {
    id_bytes(target).to_vec()
}

pub fn use_data(hand: u8) -> Vec<u8> {
    vec![hand]
}

pub fn equip_data(hand: u8, slot: &str) -> Vec<u8> {
    let mut v = vec![hand];
    v.extend_from_slice(slot.as_bytes());
    v
}

pub fn unequip_data(slot: &str) -> Vec<u8> {
    slot.as_bytes().to_vec()
}

pub fn store_data(hand: u8, bag: ObjectId) -> Vec<u8> {
    let mut v = vec![hand];
    v.extend_from_slice(&id_bytes(bag));
    v
}

pub fn take_data(bag: ObjectId, item: ObjectId) -> Vec<u8> {
    let mut v = id_bytes(bag).to_vec();
    v.extend_from_slice(&id_bytes(item));
    v
}

pub fn unequip_to_data(hand: u8, slot: &str) -> Vec<u8> {
    equip_data(hand, slot)
}

pub fn take_to_data(hand: u8, bag: ObjectId, item: ObjectId) -> Vec<u8> {
    let mut v = vec![hand];
    v.extend(take_data(bag, item));
    v
}

pub fn attack_data(target: astrocore_core::NetworkEntityId) -> Vec<u8> {
    target.0.to_le_bytes().to_vec()
}

pub fn heal_data(hand: u8, target: astrocore_core::NetworkEntityId) -> Vec<u8> {
    let mut v = vec![hand];
    v.extend_from_slice(&target.0.to_le_bytes());
    v
}

pub fn parse_attack(data: &[u8]) -> Option<astrocore_core::NetworkEntityId> {
    let b: [u8; 4] = data.try_into().ok()?;
    Some(astrocore_core::NetworkEntityId(u32::from_le_bytes(b)))
}

/// Удар по части тела: `attack_data` плюс байт — индекс в `health::PARTS`.
pub fn attack_part_data(target: astrocore_core::NetworkEntityId, part: u8) -> Vec<u8> {
    let mut v = attack_data(target);
    v.push(part);
    v
}

/// Цель и необязательная часть тела; неизвестный индекс части отвергает команду.
pub fn parse_attack_part(
    data: &[u8],
) -> Option<(astrocore_core::NetworkEntityId, Option<&'static str>)> {
    match data.len() {
        4 => Some((parse_attack(data)?, None)),
        5 => {
            let part = crate::health::PARTS.get(usize::from(data[4]))?;
            Some((parse_attack(&data[..4])?, Some(*part)))
        }
        _ => None,
    }
}

pub fn feed_data(hand: u8, target: astrocore_core::NetworkEntityId) -> Vec<u8> {
    heal_data(hand, target)
}

pub fn parse_heal(data: &[u8]) -> Option<(u8, astrocore_core::NetworkEntityId)> {
    let (hand, rest) = data.split_first()?;
    Some((*hand, parse_attack(rest)?))
}

pub fn move_slot_data(from: &str, to: &str) -> Vec<u8> {
    let mut v = from.as_bytes().to_vec();
    v.push(0);
    v.extend_from_slice(to.as_bytes());
    v
}

pub fn parse_take_to(data: &[u8]) -> Option<(u8, ObjectId, ObjectId)> {
    let (hand, rest) = data.split_first()?;
    let (bag, item) = parse_take(rest)?;
    Some((*hand, bag, item))
}

pub fn parse_move_slot(data: &[u8]) -> Option<(&str, &str)> {
    let at = data.iter().position(|b| *b == 0)?;
    Some((slot_name(&data[..at])?, slot_name(&data[at + 1..])?))
}

/// Имя слота из команды: короткая строка из букв и цифр.
fn slot_name(bytes: &[u8]) -> Option<&str> {
    let s = std::str::from_utf8(bytes).ok()?;
    (!s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(s)
}

pub fn parse_equip(data: &[u8]) -> Option<(u8, &str)> {
    let (hand, rest) = data.split_first()?;
    Some((*hand, slot_name(rest)?))
}

pub fn parse_unequip(data: &[u8]) -> Option<&str> {
    slot_name(data)
}

pub fn parse_store(data: &[u8]) -> Option<(u8, ObjectId)> {
    let (hand, rest) = data.split_first()?;
    Some((*hand, parse_object_id(rest)?))
}

pub fn parse_take(data: &[u8]) -> Option<(ObjectId, ObjectId)> {
    if data.len() != 32 {
        return None;
    }
    Some((parse_object_id(&data[..16])?, parse_object_id(&data[16..])?))
}
