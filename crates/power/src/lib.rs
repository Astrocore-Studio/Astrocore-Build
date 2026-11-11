//! Электричество (`Content.Server/Power`): сети кабелей и решатель `BatteryRampPegSolver` (Pow3r).
//!
//! Сеть — набор источников (`Supply`), потребителей (`Load`) и аккумуляторов (`Battery`), которые
//! заряжаются от одной сети и разряжаются в другую (АПЦ: средняя сеть → сеть АПЦ, подстанция:
//! высокая → средняя, СМЭС: высокая → высокая). Решатель идёт от «нижних» сетей к верхним, чтобы
//! верхние видели спрос аккумуляторов нижних.

pub mod cables;
pub mod solver;

pub use cables::{CableGroup, Cables};
pub use solver::{Battery, Load, Network, PowerState, Supply};
