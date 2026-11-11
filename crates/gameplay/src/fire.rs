//! Огонь в мире: источники зажигания (`IgnitionSourceComponent`), пожар на клетках и горящие
//! персонажи. Сам газовый огонь считает `atmos` (стадия `Hotspots`), горение тела — `body`.

use prototypes::{Data, Resolved};
use serde::{Deserialize, Serialize};

/// Все клетки с огнём; видят все.
pub const FIRES_COMPONENT: &str = "build.fires";
pub const FIRES_MAX_BYTES: usize = 8 * 1024;
/// Горит ли персонаж; видят все (значок огня на спрайте).
pub const BURNING_COMPONENT: &str = "build.burning";
/// Не больше стольких клеток с огнём уходит клиентам.
pub const MAX_FIRES: usize = 120;

/// Клетка с огнём: центр в метрах и размер пламени (1 — искра, 2 — пламя, 3 — пожар).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FireView {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub state: u8,
}

pub fn encode_fires(fires: &[FireView]) -> Vec<u8> {
    serde_json::to_vec(fires).unwrap_or_default()
}

pub fn decode_fires(bytes: &[u8]) -> Option<Vec<FireView>> {
    if bytes.len() > FIRES_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Температура источника огня прототипа (`IgnitionSource: (temperature: …)`); 700 К по умолчанию.
pub fn ignition_temperature(r: &Resolved) -> Option<f32> {
    let c = r.component("IgnitionSource")?;
    Some(
        c.get("temperature")
            .and_then(Data::as_f64)
            .map_or(crate::body::IGNITION_SOURCE_TEMPERATURE, |v| v as f32),
    )
}
