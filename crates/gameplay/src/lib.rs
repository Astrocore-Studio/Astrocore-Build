mod doafter;
pub mod handlers;
mod hands;
mod health;
pub mod interact;
pub mod inventory;
pub mod items;
pub mod module;
pub mod storage;
pub mod wire;

pub use doafter::{BREAK_DISTANCE_M, DOAFTER_COMPONENT, DoAfterView, PRY_TIME_S, decode_doafter};
pub use hands::{Hands, HandsError, HeldItem};
pub use health::{
    CRITICAL_AT, DEAD_AT, FIST_DAMAGE, GROUPS, HEALTH_COMPONENT, Health, HealthView,
    MOBSTATE_COMPONENT, MobState, PARTS, decode_health,
};
pub use interact::{InteractionHandler, Target, Used};
pub use inventory::{Inventory, InventoryError};
pub use items::{ItemInfo, ItemSize};
pub use module::State;
pub use module::{GameplayModule, INTERACTION_RANGE_M, NAMESPACE};
pub use storage::{Storage, StorageError, StorageSpec};
pub use wire::{
    BagView, CMD_ATTACK, CMD_CANCEL, CMD_DROP, CMD_EQUIP, CMD_HEAL, CMD_INTERACT, CMD_MOVE_HAND,
    CMD_MOVE_SLOT, CMD_PICKUP, CMD_PICKUP_ANY, CMD_SELECT_HAND, CMD_STORE, CMD_SWAP_HANDS,
    CMD_TAKE, CMD_TAKE_TO, CMD_UNEQUIP, CMD_UNEQUIP_TO, CMD_USE, HANDS_COMPONENT,
    INVENTORY_COMPONENT, InventoryView, WORN_COMPONENT, Worn, attack_data, attack_part_data,
    decode_hands, decode_inventory, decode_worn, drop_at_data, drop_data, encode_worn, equip_data,
    heal_data, interact_data, move_hand_data, move_slot_data, parse_drop, parse_object_id,
    pickup_data, register_replication, store_data, take_data, take_to_data, unequip_data,
    unequip_to_data, use_data,
};

#[cfg(test)]
mod tests;
