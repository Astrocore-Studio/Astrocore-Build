//! Действия с таймером (`DoAfter` в SS14): надевание и снятие с задержкой, вскрытие двери
//! монтировкой. Пока идёт отсчёт, персонаж не должен сходить с места (`BreakOnMove`) и обязан
//! сохранять условия действия; иначе оно прерывается без последствий. Одновременно у персонажа
//! одно такое действие. Начало и конец рассылаются всем (`build.doafter`), чтобы клиент рисовал
//! полоску.

use astrocore_core::ObjectId;
use astrocore_math::glam::Vec3;
use serde::{Deserialize, Serialize};

/// Состояние действия с таймером у персонажа; пустое значение — действия нет.
pub const DOAFTER_COMPONENT: &str = "build.doafter";
pub const DOAFTER_MAX_BYTES: usize = 256;

/// На сколько можно сместиться, не прерывая действие, м (порог `BreakOnMove` в SS14).
pub const BREAK_DISTANCE_M: f32 = 0.3;
/// Время вскрытия двери монтировкой, с (`DoorComponent.PryTime` в SS14).
pub const PRY_TIME_S: f32 = 1.5;

/// Что делает действие по завершении.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    Equip { hand: usize, slot: String },
    Unequip { slot: String, hand: Option<usize> },
    Pry { element: ObjectId, hand: usize },
}

impl Action {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Equip { .. } => "equip",
            Self::Unequip { .. } => "unequip",
            Self::Pry { .. } => "pry",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DoAfter {
    pub(crate) action: Action,
    pub(crate) delay: f32,
    pub(crate) elapsed: f32,
    pub(crate) start: Vec3,
}

/// Для клиента: что за действие и сколько оно длится.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoAfterView {
    pub kind: String,
    pub delay_s: f32,
}

pub fn encode_doafter(v: Option<&DoAfterView>) -> Vec<u8> {
    v.map(|v| serde_json::to_vec(v).unwrap_or_default())
        .unwrap_or_default()
}

/// Разобрать состояние; `None` и для пустого значения, и для мусора.
pub fn decode_doafter(bytes: &[u8]) -> Option<DoAfterView> {
    if bytes.is_empty() || bytes.len() > DOAFTER_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_round_trips_and_empty_means_no_action() {
        let v = DoAfterView {
            kind: "pry".into(),
            delay_s: 1.5,
        };
        assert_eq!(decode_doafter(&encode_doafter(Some(&v))), Some(v));
        assert_eq!(decode_doafter(&encode_doafter(None)), None);
        assert_eq!(decode_doafter(b"\xff garbage"), None);
    }
}
