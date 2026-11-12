//! Анализатор здоровья (`HealthAnalyzerSystem`, снимок SS14 `910154a7`).
//!
//! Предмет в руке на человеке (`T` — на другого, `R` — на себя): 0,8 с неподвижности, затем окно
//! показывает состояние, температуру, кровь, кровотечение, суммарный и покомпонентный урон по
//! группам; данные обновляются раз в секунду, пока цель в пределах `MaxScanRange` (2,5 м), иначе
//! сканирование приостановлено. Не перенесено: подробности вида, возраста, звук, режим «неизлечим».

use serde::{Deserialize, Serialize};

pub const SCAN_COMPONENT: &str = "build.scan";
pub const SCAN_MAX_BYTES: usize = 4096;
/// `HealthAnalyzerComponent.ScanDelay` и `MaxScanRange`.
pub const SCAN_DELAY_S: f32 = 0.8;
pub const SCAN_RANGE_M: f32 = 2.5;

/// Группа урона: название, сумма и виды с уроном.
pub type DamageGroup = (String, f32, Vec<(String, f32)>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanView {
    /// Цель в пределах дальности, данные свежие.
    pub active: bool,
    pub target: String,
    pub state: String,
    pub temperature_k: f32,
    /// Доля крови от нормы (1,0 — норма).
    pub blood: f32,
    pub bleeding: bool,
    pub total_damage: f32,
    /// Группы урона: название, сумма и виды с уроном.
    pub groups: Vec<DamageGroup>,
}

pub fn encode_scan(v: Option<&ScanView>) -> Vec<u8> {
    v.map(|v| serde_json::to_vec(v).unwrap_or_default())
        .unwrap_or_default()
}

pub fn decode_scan(bytes: &[u8]) -> Option<ScanView> {
    if bytes.is_empty() || bytes.len() > SCAN_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_round_trips() {
        let v = ScanView {
            active: true,
            target: "Игрок".into(),
            state: "Alive".into(),
            temperature_k: 310.15,
            blood: 1.0,
            bleeding: false,
            total_damage: 12.5,
            groups: vec![("Brute".into(), 12.5, vec![("Blunt".into(), 12.5)])],
        };
        assert_eq!(decode_scan(&encode_scan(Some(&v))), Some(v));
    }
}
