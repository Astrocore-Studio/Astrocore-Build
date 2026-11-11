//! Атмосфера Build: перенос механики из SS14 (газы, LINDA, группы возбуждения).

pub mod gas;
pub mod grid;

pub use gas::*;
pub use grid::{
    ATMOS_TICK_RATE, Cell, CellKind, Edge, FLOOR_STRIDE, Grid, Layout, edge, floor_cell,
};
