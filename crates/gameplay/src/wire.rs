use astrocore_api::{
    Interest, ReplicatedKind, ReplicationChannel, ReplicationError, ReplicationRegistry,
};
use astrocore_core::ObjectId;

use crate::hands::Hands;

/// Команды клиента.
pub const CMD_PICKUP: &str = "item.pickup";
pub const CMD_DROP: &str = "item.drop";
pub const CMD_SELECT_HAND: &str = "hands.select";

/// Реплицируемое состояние рук игрока.
pub const HANDS_COMPONENT: &str = "build.hands";

/// Предел размера состояния рук в сообщении (двенадцать рук с длинными идентификаторами).
pub const HANDS_MAX_BYTES: usize = 4096;

pub fn pickup_data(item: ObjectId) -> Vec<u8> {
    item.as_uuid().as_u128().to_be_bytes().to_vec()
}

pub fn drop_data(hand: u8) -> Vec<u8> {
    vec![hand]
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
