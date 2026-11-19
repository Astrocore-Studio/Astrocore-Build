//! Конец раунда и перезапуск (`GameTicker.EndRound`, `RoundEndSummaryWindow` SS14).
//!
//! Раунд заканчивается, когда весь экипаж (игроки с должностью) погиб или вышло время раунда
//! (`--round-secs`). Всем игрокам приходит сводка: причина, длительность, кто выжил. Игра идёт
//! дальше, пока не истечёт время до перезапуска (`RestartTimer`, 20 с); после этого модуль
//! подаёт сигнал, и сервер начинает новый раунд с нуля, а клиенты входят в него заново.
//!
//! Не перенесено: итоги по целям, имена и достижения, шаттл эвакуации и объявления.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use astrocore_core::NetworkEntityId;
use serde::{Deserialize, Serialize};

pub const ROUNDEND_COMPONENT: &str = "build.roundend";
pub const ROUNDEND_MAX_BYTES: usize = 4096;
/// Сколько секунд показывается сводка до нового раунда.
pub const RESTART_DELAY_S: f32 = 20.0;

/// Сигнал модуля серверу: раунд закончен, пора начинать новый.
#[derive(Debug, Clone, Default)]
pub struct RoundSignal(Arc<AtomicBool>);

impl RoundSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request_restart(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Был ли запрос; сбрасывает его.
    pub fn take(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }
}

/// Как закончился раунд.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Весь экипаж погиб.
    CrewDead,
    /// Вышло время раунда.
    TimeLimit,
}

impl EndReason {
    pub fn text(self) -> &'static str {
        match self {
            Self::CrewDead => "Весь экипаж погиб",
            Self::TimeLimit => "Время раунда вышло",
        }
    }
}

/// Состояние конца раунда внутри модуля.
#[derive(Debug, Default)]
pub(crate) struct RoundState {
    pub signal: Option<RoundSignal>,
    pub limit_s: Option<f32>,
    /// Секунд с начала раунда (после старта лобби).
    pub clock: f32,
    /// Игроки, бравшие должность, и их последняя должность.
    pub crew: BTreeMap<NetworkEntityId, String>,
    /// Причина, часы раунда в момент конца и итоги экипажа.
    pub ended: Option<(EndReason, f32, Vec<CrewEntry>)>,
    pub sent: BTreeMap<NetworkEntityId, Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewEntry {
    pub job: String,
    pub alive: bool,
}

/// Для клиента: сводка конца раунда.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundEndView {
    pub reason: String,
    /// Длительность раунда, с.
    pub duration: u32,
    /// Секунд до нового раунда (округлено вверх).
    pub restart_in: u32,
    pub round: u32,
    pub crew: Vec<CrewEntry>,
}

pub fn encode_end(v: &RoundEndView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_end(bytes: &[u8]) -> Option<RoundEndView> {
    if bytes.is_empty() || bytes.len() > ROUNDEND_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signal_fires_once() {
        let s = RoundSignal::new();
        assert!(!s.take());
        s.clone().request_restart();
        assert!(s.take());
        assert!(!s.take());
    }

    #[test]
    fn the_summary_survives_the_wire() {
        let v = RoundEndView {
            reason: EndReason::CrewDead.text().into(),
            duration: 725,
            restart_in: 20,
            round: 3,
            crew: vec![CrewEntry {
                job: "Chemist".into(),
                alive: false,
            }],
        };
        assert_eq!(decode_end(&encode_end(&v)), Some(v));
    }
}
