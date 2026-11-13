//! Лобби перед раундом (`LobbyState`/`LobbyGui` SS14).
//!
//! Новый игрок попадает в лобби: он остаётся на точке появления и ничего не может делать, кроме
//! готовности. Таймер до старта идёт сам; когда он дошёл до нуля, раунд начинается — готовые
//! игроки выходят на станцию (выбирают должность), остальные остаются в лобби и могут
//! присоединиться позже кнопкой «Присоединиться» (`LateJoin`).
//!
//! Не перенесено: чат лобби, выбор персонажа, список игроков по именам, наблюдатели.

use std::collections::{BTreeMap, BTreeSet};

use astrocore_core::NetworkEntityId;
use serde::{Deserialize, Serialize};

pub const LOBBY_COMPONENT: &str = "build.lobby";
pub const LOBBY_MAX_BYTES: usize = 1024;

/// Состояние лобби на сервере.
#[derive(Debug, Clone)]
pub struct Lobby {
    pub started: bool,
    /// Секунд до старта раунда.
    pub left: f32,
    pub round: u32,
    /// Игроки, которых лобби уже видело (новичок — тот, кого здесь нет).
    pub known: BTreeSet<NetworkEntityId>,
    pub in_lobby: BTreeSet<NetworkEntityId>,
    pub ready: BTreeSet<NetworkEntityId>,
    pub(crate) sent: BTreeMap<NetworkEntityId, Vec<u8>>,
}

impl Lobby {
    pub fn new(secs: f32, round: u32) -> Self {
        Self {
            started: secs <= 0.0,
            left: secs,
            round,
            known: BTreeSet::new(),
            in_lobby: BTreeSet::new(),
            ready: BTreeSet::new(),
            sent: BTreeMap::new(),
        }
    }
}

/// Для клиента: состояние лобби этого игрока.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LobbyView {
    /// Игрок сейчас в лобби (а не на станции).
    pub in_lobby: bool,
    /// Раунд уже идёт: вместо «Готов» кнопка «Присоединиться».
    pub started: bool,
    /// Секунд до старта (округлено вверх).
    pub left: u32,
    pub ready: bool,
    pub ready_count: u32,
    pub total: u32,
    pub round: u32,
    pub station: String,
}

pub fn encode_lobby(v: &LobbyView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_lobby(bytes: &[u8]) -> Option<LobbyView> {
    if bytes.is_empty() || bytes.len() > LOBBY_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_survives_the_wire() {
        let v = LobbyView {
            in_lobby: true,
            started: false,
            left: 42,
            ready: true,
            ready_count: 1,
            total: 3,
            round: 1,
            station: "Saltern".into(),
        };
        assert_eq!(decode_lobby(&encode_lobby(&v)), Some(v));
        assert_eq!(decode_lobby(&[]), None);
    }
}
