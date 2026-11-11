use astrocore_api::{
    Interest, ReplicatedKind, ReplicationChannel, ReplicationError, ReplicationRegistry,
};
use astrocore_core::ObjectId;

use crate::hands::Hands;

/// Команды клиента.
/// Клик по предмету: только в активную руку (как `TryPickup` в SS14).
pub const CMD_PICKUP: &str = "item.pickup";
/// Глагол «поднять»: в первую свободную руку, начиная с активной (`TryPickupAnyHand`).
pub const CMD_PICKUP_ANY: &str = "item.pickup_any";
/// Смена активной руки по кругу; данные — `[1]` для обратного направления.
pub const CMD_SWAP_HANDS: &str = "hands.swap";
pub const CMD_DROP: &str = "item.drop";
pub const CMD_SELECT_HAND: &str = "hands.select";

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
    })
}
