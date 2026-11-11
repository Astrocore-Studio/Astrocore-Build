pub mod hands;
pub mod module;
pub mod wire;

pub use hands::{Hands, HandsError, HeldItem};
pub use module::{DEFAULT_REACH_M, GameplayModule, NAMESPACE};
pub use wire::{
    CMD_DROP, CMD_PICKUP, CMD_SELECT_HAND, HANDS_COMPONENT, decode_hands, drop_data,
    parse_object_id, pickup_data, register_replication,
};

#[cfg(test)]
mod tests;
