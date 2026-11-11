//! Атмосфера Build: перенос механики из SS14 (газы, LINDA, группы возбуждения).

pub mod gas;
pub mod grid;
pub mod reactions;

pub use gas::*;
pub use grid::{
    ATMOS_TICK_RATE, Cell, CellKind, Edge, FLOOR_STRIDE, Fill, Fire, Grid, Hotspot, Layout, edge,
    floor_cell,
};
