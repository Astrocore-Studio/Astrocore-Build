mod body;
pub mod chem;
mod conveyor;
pub mod vending;
pub use vending::{VENDING_COMPONENT, VendingItem, VendingView, decode_vending};
pub mod dispenser;
mod disposal;
pub mod fx;
pub use dispenser::{DISPENSER_COMPONENT, DispenserView, decode_dispenser};
pub use fx::{FX_COMPONENT, FxBatch, FxSound, decode_fx};
pub mod analyzer;
pub mod chemhand;
pub mod chemlab;
pub mod cryo;
pub use cryo::{CRYO_COMPONENT, CryoView, decode_cryo};
mod defib;
pub use analyzer::{SCAN_COMPONENT, ScanView, decode_scan};
pub use chemhand::{CHEMHAND_COMPONENT, ChemHandView, decode_chemhand};
pub use chemlab::{LAB_COMPONENT, LabView, decode_lab};
pub mod alerts;
mod doafter;
mod edibles;
pub mod electric;
pub mod fire;
pub mod handlers;
mod hands;
mod health;
pub mod idconsole;
pub mod power_monitor;
pub use idconsole::{
    IDCONSOLE_COMPONENT, IdConsoleView, TargetView, WriteRequest, decode_view, write_data,
};
pub mod jobs;
pub mod lobby;
pub mod round_end;
pub use jobs::{JOBS_COMPONENT, JobEntry, JobsView, decode_jobs};
pub use lobby::{LOBBY_COMPONENT, LobbyView, decode_lobby};
pub use round_end::{
    CrewEntry, EndReason, RESTART_DELAY_S, ROUNDEND_COMPONENT, RoundEndView, RoundSignal,
    decode_end,
};
pub mod interact;
pub mod inventory;
pub mod items;
pub mod module;
mod monitor;
mod mopping;
mod piping;
pub mod puddles;
mod reagents;
mod spawners;
pub mod storage;
pub mod wire;
mod world_atmos;

pub use body::{PressureAlert, VITALS_COMPONENT, Vitals, decode_vitals};
pub use doafter::{BREAK_DISTANCE_M, DOAFTER_COMPONENT, DoAfterView, PRY_TIME_S, decode_doafter};
pub use hands::{Hands, HandsError, HeldItem};
pub use health::{
    CRITICAL_AT, DEAD_AT, FIST_DAMAGE, GHOST_COMPONENT, GROUPS, HEALTH_COMPONENT, Health,
    HealthView, MOBSTATE_COMPONENT, MobState, PARTS, RESPAWN_DELAY_S, decode_health,
};
pub use interact::{InteractionHandler, Target, Used};
pub use inventory::{Inventory, InventoryError};
pub use items::{ItemInfo, ItemSize};
pub use module::State;
pub use module::{GameplayModule, INTERACTION_RANGE_M, NAMESPACE};
pub use storage::{Storage, StorageError, StorageSpec};
pub use wire::{
    BagView, CMD_ATTACK, CMD_CANCEL, CMD_CHEM_AMOUNT, CMD_CRYO_BEAKER_EJECT, CMD_CRYO_CLEAR,
    CMD_CRYO_CLOSE, CMD_CRYO_EJECT, CMD_CRYO_INJECT, CMD_CRYO_INSERT, CMD_CRYO_PUT,
    CMD_DISP_AMOUNT, CMD_DISP_CLEAR, CMD_DISP_CLOSE, CMD_DISP_DISPENSE, CMD_DISP_EJECT,
    CMD_DISP_INSERT, CMD_DROP, CMD_EQUIP, CMD_FEED, CMD_HEAL, CMD_ID_CLOSE, CMD_ID_EJECT,
    CMD_ID_INSERT, CMD_ID_WRITE, CMD_INTERACT, CMD_JOB, CMD_LAB_BOTTLE, CMD_LAB_CLOSE,
    CMD_LAB_EJECT, CMD_LAB_INSERT, CMD_LAB_MODE, CMD_LAB_PILLS, CMD_LAB_REAGENT, CMD_LAB_SOURCE,
    CMD_LAB_START, CMD_LOBBY_READY, CMD_LOCK, CMD_MOVE_HAND, CMD_MOVE_SLOT, CMD_PICKUP,
    CMD_PICKUP_ANY, CMD_RESIST, CMD_RESPAWN, CMD_SCAN_CLOSE, CMD_SELECT_HAND, CMD_SHAKE, CMD_SPILL,
    CMD_STORE, CMD_SWAP_HANDS, CMD_TAKE, CMD_TAKE_TO, CMD_UNEQUIP, CMD_UNEQUIP_TO, CMD_USE,
    CMD_USE_GROUND, CMD_VEND, CMD_VEND_CLOSE, HANDS_COMPONENT, INVENTORY_COMPONENT, InventoryView,
    WORN_COMPONENT, Worn, attack_data, attack_part_data, decode_hands, decode_inventory,
    decode_worn, drop_at_data, drop_data, encode_worn, equip_data, feed_data, heal_data,
    interact_data, move_hand_data, move_slot_data, parse_drop, parse_object_id, pickup_data,
    register_replication, store_data, take_data, take_to_data, unequip_data, unequip_to_data,
    use_data,
};

#[cfg(test)]
mod tests;
