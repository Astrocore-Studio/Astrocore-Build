pub mod hands;
pub mod inventory;
pub mod items;
pub mod module;
pub mod storage;
pub mod wire;

pub use hands::{Hands, HandsError, HeldItem};
pub use inventory::{Inventory, InventoryError};
pub use items::{ItemInfo, ItemSize};
pub use module::{GameplayModule, INTERACTION_RANGE_M, NAMESPACE};
pub use storage::{Storage, StorageError, StorageSpec};
pub use wire::{
    BagView, CMD_DROP, CMD_EQUIP, CMD_PICKUP, CMD_PICKUP_ANY, CMD_SELECT_HAND, CMD_STORE,
    CMD_SWAP_HANDS, CMD_TAKE, CMD_UNEQUIP, HANDS_COMPONENT, INVENTORY_COMPONENT, InventoryView,
    decode_hands, decode_inventory, drop_at_data, drop_data, equip_data, parse_drop,
    parse_object_id, pickup_data, register_replication, store_data, take_data, unequip_data,
};

#[cfg(test)]
mod tests;
