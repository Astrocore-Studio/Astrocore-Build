//! Подсказки и звуки, которые сервер присылает игрокам (`PopupSystem`, `AudioSystem.PlayPvs`).
//!
//! За тик набирается очередь: подсказка адресована одному игроку, звук — точке мира и слышен всем,
//! кто рядом (до 14 м). В конце тика каждый получает пачку `FxBatch` с новым номером; клиент
//! показывает подсказки у себя на экране и играет звуки по пути внутри `Audio` (`Machines/machine_vend.ogg`).

use serde::{Deserialize, Serialize};

pub const FX_COMPONENT: &str = "build.fx";
pub const FX_MAX_BYTES: usize = 4096;
/// Слышимость звука, м (как у клиента).
pub const HEARING_M: f32 = 14.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FxSound {
    pub path: String,
    pub at: [f32; 3],
    #[serde(default)]
    pub volume_db: f32,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FxBatch {
    pub seq: u32,
    pub popups: Vec<String>,
    pub sounds: Vec<FxSound>,
}

pub fn encode_fx(b: &FxBatch) -> Vec<u8> {
    serde_json::to_vec(b).unwrap_or_default()
}

pub fn decode_fx(bytes: &[u8]) -> Option<FxBatch> {
    if bytes.is_empty() || bytes.len() > FX_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_round_trips() {
        let b = FxBatch {
            seq: 3,
            popups: vec!["Отказано".into()],
            sounds: vec![FxSound {
                path: "Machines/buzz-sigh.ogg".into(),
                at: [1.0, 0.0, 2.0],
                volume_db: 0.0,
            }],
        };
        assert_eq!(decode_fx(&encode_fx(&b)), Some(b));
    }
}
