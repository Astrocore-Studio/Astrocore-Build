use astrocore_core::ObjectId;
use serde::{Deserialize, Serialize};

pub const COMPONENT: &str = "build.wires";
pub const CMD_OPEN: &str = "wires.open";
pub const CMD_CLOSE: &str = "wires.close";
pub const CMD_ACTION: &str = "wires.action";
pub const MAX_BYTES: usize = 8192;
pub const MOTION_COMPONENT: &str = "build.airlock_motion";
pub const MOTION_MAX_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Motion {
    pub element: ObjectId,
    pub opening: bool,
    pub elapsed: f32,
    pub duration: f32,
}

pub fn decode_motions(bytes: &[u8]) -> Option<Vec<Motion>> {
    if bytes.len() > MOTION_MAX_BYTES || !bytes.len().is_multiple_of(25) {
        return None;
    }
    bytes
        .as_chunks::<25>()
        .0
        .iter()
        .map(|chunk| {
            let elapsed = f32::from_le_bytes(chunk[17..21].try_into().ok()?);
            let duration = f32::from_le_bytes(chunk[21..25].try_into().ok()?);
            if chunk[16] > 1
                || !elapsed.is_finite()
                || elapsed < 0.
                || !duration.is_finite()
                || duration <= 0.
            {
                return None;
            }
            Some(Motion {
                element: ObjectId::from_u128(u128::from_le_bytes(chunk[..16].try_into().ok()?)),
                opening: chunk[16] == 1,
                elapsed,
                duration,
            })
        })
        .collect()
}

pub(crate) fn encode_motions(motions: &[Motion]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(motions.len() * 25);
    for motion in motions {
        bytes.extend_from_slice(&motion.element.as_uuid().as_u128().to_le_bytes());
        bytes.push(u8::from(motion.opening));
        bytes.extend_from_slice(&motion.elapsed.to_le_bytes());
        bytes.extend_from_slice(&motion.duration.to_le_bytes());
    }
    bytes
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Wire {
    pub id: u8,
    pub letter: char,
    pub color: String,
    pub cut: bool,
    pub variant: u8,
    pub mirror: bool,
    pub flip: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum LightState {
    Off,
    On,
    BlinkingFast,
    BlinkingSlow,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Light {
    pub text: String,
    pub color: String,
    pub state: LightState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct View {
    pub element: ObjectId,
    pub name: String,
    pub serial: String,
    pub wires: Vec<Wire>,
    pub lights: Vec<Light>,
    pub can_cut: bool,
    pub can_pulse: bool,
}

pub fn decode(bytes: &[u8]) -> Option<View> {
    if bytes.len() > MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}
