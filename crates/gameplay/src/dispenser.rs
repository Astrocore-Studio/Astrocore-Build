//! Раздатчики реагентов (`ReagentDispenserSystem`, `SharedReagentDispenser`, снимок SS14 `910154a7`).
//!
//! В раздатчике стоят банки (`Jug…`) с реагентами и слот мензурки. Окно: выбрать количество
//! (1, 5, 10, 15, 20, 25, 30, 40, 50, 60, 100, 120; по умолчанию 10), нажать на банку — столько единиц переливается
//! в мензурку, пока в ней есть место (в мензурке идут реакции); «Очистить» опустошает мензурку, «Вынуть»
//! возвращает её в руку. Нужен ток. Не перенесено: звук щелчка, метки банок, перезарядка банками из рук.

use serde::{Deserialize, Serialize};

pub const DISPENSER_COMPONENT: &str = "build.dispenser";
pub const DISPENSER_MAX_BYTES: usize = 8192;
/// `ReagentDispenserDispenseAmount`.
pub const AMOUNTS: [f32; 9] = [1.0, 5.0, 10.0, 15.0, 20.0, 30.0, 40.0, 60.0, 120.0];
pub const DEFAULT_AMOUNT: f32 = 10.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispenserView {
    pub machine: astrocore_core::ObjectId,
    pub beaker: Option<BeakerInfo>,
    pub jugs: Vec<JugInfo>,
    pub amount: f32,
    pub amounts: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeakerInfo {
    pub name: String,
    pub volume: f32,
    pub max: f32,
    pub reagents: Vec<(String, f32)>,
    /// Температура раствора, К.
    #[serde(default)]
    pub temperature: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JugInfo {
    pub index: u8,
    pub label: String,
    pub quantity: f32,
    pub color: String,
}

pub fn encode_dispenser(v: Option<&DispenserView>) -> Vec<u8> {
    v.map(|v| serde_json::to_vec(v).unwrap_or_default())
        .unwrap_or_default()
}

pub fn decode_dispenser(bytes: &[u8]) -> Option<DispenserView> {
    if bytes.is_empty() || bytes.len() > DISPENSER_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_round_trips() {
        let v = DispenserView {
            machine: astrocore_core::ObjectId::from_u128(3),
            beaker: Some(BeakerInfo {
                name: "beaker".into(),
                volume: 10.0,
                max: 50.0,
                reagents: vec![("Aluminium".into(), 10.0)],
                temperature: 293.15,
            }),
            jugs: vec![JugInfo {
                index: 0,
                label: "aluminium".into(),
                quantity: 240.0,
                color: "#a1a1a1".into(),
            }],
            amount: 10.0,
            amounts: AMOUNTS.to_vec(),
        };
        assert_eq!(decode_dispenser(&encode_dispenser(Some(&v))), Some(v));
        assert_eq!(decode_dispenser(&[]), None);
    }
}
