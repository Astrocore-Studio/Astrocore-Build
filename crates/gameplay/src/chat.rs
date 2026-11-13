//! Чат (`ChatSystem`, `ChatManager` SS14): сказать, шептать, действие, OOC, чат мёртвых.
//!
//! Речь слышат игроки в радиусе голоса (`VoiceRange` 10 м); шёпот — ясно в 2 м
//! (`WhisperClearRange`), неразборчиво до 5 м (`WhisperMuffledRange`: часть букв заменяется на
//! `~`); действие (`/me`) — в радиусе голоса; OOC — все. Мёртвые и призраки говорят только друг
//! с другом, а слышат всех. Игроки в лобби пишут в OOC.
//!
//! Не перенесено: радио и каналы, LOOC, имена персонажей (вместо них название должности),
//! выделение речи цветом по должности, ограничение по прямой видимости.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use astrocore_core::NetworkEntityId;
use serde::{Deserialize, Serialize};

pub const CHAT_COMPONENT: &str = "build.chat";
pub const CHAT_MAX_BYTES: usize = 15_000;
/// Сколько строк хранится у игрока.
pub const KEEP_LINES: usize = 20;
/// Предел длины сообщения, символы.
pub const MAX_LEN: usize = 200;
pub const VOICE_RANGE_M: f32 = 10.0;
pub const WHISPER_CLEAR_M: f32 = 2.0;
pub const WHISPER_MUFFLED_M: f32 = 5.0;
/// Не чаще одного сообщения за столько секунд от игрока.
pub const MIN_GAP_S: f32 = 0.4;

/// Состояние чата на сервере.
#[derive(Debug, Default)]
pub(crate) struct ChatState {
    pub next_id: u32,
    /// Что слышал каждый игрок (уже с учётом расстояния).
    pub log: BTreeMap<NetworkEntityId, VecDeque<ChatLine>>,
    pub dirty: BTreeSet<NetworkEntityId>,
    /// Когда игрок писал в последний раз (часы модуля).
    pub last: BTreeMap<NetworkEntityId, f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    Say,
    Whisper,
    Emote,
    Ooc,
    Dead,
}

impl Channel {
    pub fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::Say,
            1 => Self::Whisper,
            2 => Self::Emote,
            3 => Self::Ooc,
            _ => return None,
        })
    }

    pub fn to_byte(self) -> u8 {
        match self {
            Self::Say => 0,
            Self::Whisper => 1,
            Self::Emote => 2,
            Self::Ooc | Self::Dead => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatLine {
    pub id: u32,
    pub channel: Channel,
    pub who: String,
    pub text: String,
}

/// Для клиента: последние строки, которые слышал игрок.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatView {
    pub lines: Vec<ChatLine>,
}

pub fn encode_chat(v: &ChatView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_chat(bytes: &[u8]) -> Option<ChatView> {
    if bytes.is_empty() || bytes.len() > CHAT_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Команда `chat`: первый байт — канал, дальше текст UTF-8.
pub fn chat_data(channel: Channel, text: &str) -> Vec<u8> {
    let mut v = vec![channel.to_byte()];
    v.extend_from_slice(text.as_bytes());
    v
}

/// Текст сообщения: без управляющих символов, обрезан по длине, пустое — `None`.
pub fn clean(text: &str) -> Option<String> {
    let t: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LEN)
        .collect();
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Неразборчивый шёпот: часть букв заменяется на `~` (`ObfuscateMessageReadability`).
pub fn muffle(id: u32, text: &str) -> String {
    text.chars()
        .enumerate()
        .map(|(i, c)| {
            let h = (id as usize).wrapping_mul(31).wrapping_add(i * 17) % 3;
            if c.is_whitespace() || h != 0 { c } else { '~' }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_cleaned_and_trimmed() {
        assert_eq!(clean("  привет\u{7} мир  ").as_deref(), Some("привет мир"));
        assert_eq!(clean("   \n"), None);
        assert_eq!(clean(&"а".repeat(500)).unwrap().chars().count(), MAX_LEN);
    }

    #[test]
    fn a_muffled_whisper_keeps_its_length_and_hides_letters() {
        let m = muffle(7, "секретное сообщение");
        assert_eq!(m.chars().count(), "секретное сообщение".chars().count());
        assert!(m.contains('~'));
        assert_eq!(m.chars().nth(9), Some(' '));
    }

    #[test]
    fn the_view_survives_the_wire() {
        let v = ChatView {
            lines: vec![ChatLine {
                id: 1,
                channel: Channel::Say,
                who: "Chemist".into(),
                text: "Привет".into(),
            }],
        };
        assert_eq!(decode_chat(&encode_chat(&v)), Some(v));
    }
}
