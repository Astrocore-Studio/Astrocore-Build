//! Настройка переливаемого количества у ёмкостей и шприцев (`SolutionTransferComponent.TransferAmount`,
//! `InjectorComponent.CurrentTransferAmount`): сервер присылает владельцу, что можно выбрать у предмета в
//! активной руке, а выбор приходит командой.

use serde::{Deserialize, Serialize};

pub const CHEMHAND_COMPONENT: &str = "build.chemhand";
pub const CHEMHAND_MAX_BYTES: usize = 512;
/// `SolutionTransferSystem.DefaultTransferAmounts`.
pub const TRANSFER_AMOUNTS: [f32; 10] =
    [1.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 240.0, 480.0, 960.0];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChemHandView {
    pub hand: u8,
    /// Название режима шприца (`Inject`/`Draw`/`Dynamic`), у ёмкости пусто.
    pub mode: String,
    pub amount: f32,
    pub amounts: Vec<f32>,
}

pub fn encode_chemhand(v: Option<&ChemHandView>) -> Vec<u8> {
    v.map(|v| serde_json::to_vec(v).unwrap_or_default())
        .unwrap_or_default()
}

pub fn decode_chemhand(bytes: &[u8]) -> Option<ChemHandView> {
    if bytes.is_empty() || bytes.len() > CHEMHAND_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_round_trips() {
        let v = ChemHandView {
            hand: 0,
            mode: "Draw".into(),
            amount: 5.0,
            amounts: vec![5.0, 10.0],
        };
        assert_eq!(decode_chemhand(&encode_chemhand(Some(&v))), Some(v));
        assert_eq!(decode_chemhand(&[]), None);
    }
}
