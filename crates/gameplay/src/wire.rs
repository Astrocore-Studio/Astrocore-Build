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

/// Надеть предмет из руки в слот: `[рука, имя слота utf-8]`.
pub const CMD_EQUIP: &str = "inv.equip";
/// Снять предмет из слота: `[имя слота utf-8]`; предмет идёт в свободную руку, иначе на пол.
pub const CMD_UNEQUIP: &str = "inv.unequip";
/// Положить предмет из руки в сумку: `[рука, id сумки 16 байт]`.
pub const CMD_STORE: &str = "storage.insert";
/// Достать предмет из сумки в свободную руку: `[id сумки 16 байт, id предмета 16 байт]`.
pub const CMD_TAKE: &str = "storage.take";

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryView {
    pub inventory: Inventory,
    pub bags: Vec<BagView>,
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
