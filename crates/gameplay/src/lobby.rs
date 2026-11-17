//! Лобби перед раундом (`LobbyState`/`LobbyGui` SS14).
//!
//! Новый игрок попадает в лобби: он остаётся на точке появления и ничего не может делать, кроме
//! готовности. Таймер до старта идёт сам; когда он дошёл до нуля, раунд начинается — готовые
//! игроки выходят на станцию (выбирают должность), остальные остаются в лобби и могут
//! присоединиться позже кнопкой «Присоединиться» (`LateJoin`).
//!
//! В лобби, как в `LobbyGui`: список игроков с именами и готовностью, кнопка «Наблюдать» (призрак
//! без должности), поздний вход с выбором должности (`LateJoin`), превью персонажа, чат.
//!
//! Не перенесено: имя учётной записи (в списке — имя персонажа), фон лобби и «совет дня»,
//! голосование за режим, таймеры паузы.

use std::collections::{BTreeMap, BTreeSet};

use astrocore_core::NetworkEntityId;
use serde::{Deserialize, Serialize};

pub const LOBBY_COMPONENT: &str = "build.lobby";
pub const LOBBY_MAX_BYTES: usize = 8192;
/// Игроков в списке лобби не больше (список идёт одним компонентом).
pub const LOBBY_MAX_PLAYERS: usize = 64;
/// Команды лобби: стать наблюдателем; присоединиться к идущему раунду на должности (id в байтах).
pub const CMD_LOBBY_OBSERVE: &str = "lobby.observe";
pub const CMD_LOBBY_LATEJOIN: &str = "lobby.latejoin";

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

/// Что делает игрок (`PlayerStatus`): в лобби готов или нет, играет, наблюдает.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerStatus {
    NotReady,
    Ready,
    Playing,
    Observing,
}

/// Строка списка игроков.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyPlayer {
    pub name: String,
    pub status: PlayerStatus,
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
    /// Все игроки сервера с именами персонажей и статусом.
    #[serde(default)]
    pub players: Vec<LobbyPlayer>,
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
            players: vec![
                LobbyPlayer {
                    name: "Иван Иванов".into(),
                    status: PlayerStatus::Ready,
                },
                LobbyPlayer {
                    name: "Анна".into(),
                    status: PlayerStatus::Observing,
                },
            ],
        };
        assert_eq!(decode_lobby(&encode_lobby(&v)), Some(v));
        assert_eq!(decode_lobby(&[]), None);
    }
}
