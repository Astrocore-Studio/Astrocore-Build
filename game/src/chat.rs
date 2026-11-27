//! Чат на сервере: речь, шёпот и эмоции в радиусе голоса, LOOC, мёртвые, OOC, чат
//! администраторов, сообщения и объявления сервера, объявления станции; префиксы каналов
//! (`;`, `:к`, `.к`, `>`), глаголы речи по окончанию фразы, звуки речи и эмоций голоса,
//! замена смайликов эмоциями, заглавная буква, ограничение длины и частоты. Образец —
//! `SharedChatSystem`, `SharedChatSystem.Emote`, `ChatSystem` (`PrivateAPI`, `Utility`,
//! `Announcements`), `ChatManager` (с `RateLimit`), `ChatSanitizationManager`,
//! `ReplacementAccentSystem.ApplyReplacements`, `SpeechSoundSystem`, `VocalSystem`,
//! `MsgChatMessage`, `ChatChannel`, команды `say`, `whisper`, `me`, `ooc`, `looc`, `asay`,
//! `dsay` SS14 @ 2f4a2aad.
//!
//! Радио (сообщения с каналом) доставляет `radio`, слушающие устройства — там же. Нет пока: акцентов, личности под маской
//! (`Identity`), ссылок на имена в чате, крика по действию эмоций, повторов, связи с Discord,
//! уведомлений чата.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_console::{
    CVarDef, CVarFlags, ConfigWorldExt, ConsoleCommand, ConsoleShell, ConsoleWorldExt,
};
use astrocore_data::DataNode;
use astrocore_ecs::{EventWorldExt, GameTime, RegistryWorldExt};
use astrocore_loc::{EntityNameWorldExt, LocArg, escape_text};
use astrocore_map::{Transform, TransformWorldExt};
use astrocore_net::{
    NetEntity, NetEventWorldExt, NetRegistryWorldExt, NetTarget, NetUserId, Session,
};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::examine::loc;
use crate::mobs::{MobState, MobStateWorldExt};
use crate::popups::{PopupType, PopupWorldExt};

// Настройки.

pub const CHAT_MAX_MESSAGE_LENGTH: CVarDef<i32> = CVarDef::new(
    "chat.max_message_length",
    1000,
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Наибольшая длина сообщения чата.",
);
pub const CHAT_RATE_LIMIT_PERIOD: CVarDef<f32> = CVarDef::new(
    "chat.rate_limit_period",
    2.0,
    CVarFlags::SERVERONLY,
    "Окно подсчёта сообщений игрока, секунды.",
);
pub const CHAT_RATE_LIMIT_COUNT: CVarDef<i32> = CVarDef::new(
    "chat.rate_limit_count",
    10,
    CVarFlags::SERVERONLY,
    "Сколько сообщений игрок может отправить за окно.",
);
pub const CHAT_SANITIZER_ENABLED: CVarDef<bool> = CVarDef::new(
    "chat.chat_sanitizer_enabled",
    true,
    CVarFlags::SERVERONLY,
    "Заменять смайлики эмоциями.",
);
pub const IC_PUNCTUATION: CVarDef<bool> = CVarDef::new(
    "ic.punctuation",
    false,
    CVarFlags::SERVER,
    "Ставить точку в конце фразы.",
);
pub const OOC_ENABLED: CVarDef<bool> = CVarDef::new(
    "ooc.enabled",
    true,
    CVarFlags::NOTIFY.union(CVarFlags::REPLICATED),
    "OOC включён.",
);
pub const OOC_ENABLED_ADMIN: CVarDef<bool> = CVarDef::new(
    "ooc.enabled_admin",
    true,
    CVarFlags::NOTIFY,
    "OOC для администраторов.",
);
pub const OOC_ENABLE_DURING_ROUND: CVarDef<bool> = CVarDef::new(
    "ooc.enable_during_round",
    false,
    CVarFlags::NOTIFY
        .union(CVarFlags::REPLICATED)
        .union(CVarFlags::SERVER),
    "OOC и во время раунда.",
);
pub const LOOC_ENABLED: CVarDef<bool> = CVarDef::new(
    "looc.enabled",
    true,
    CVarFlags::NOTIFY.union(CVarFlags::REPLICATED),
    "LOOC включён.",
);
pub const LOOC_ENABLED_DEAD: CVarDef<bool> = CVarDef::new(
    "looc.enabled_dead",
    false,
    CVarFlags::NOTIFY.union(CVarFlags::REPLICATED),
    "LOOC для мёртвых.",
);
pub const LOOC_ENABLED_CRIT: CVarDef<bool> = CVarDef::new(
    "looc.enabled_crit",
    false,
    CVarFlags::NOTIFY.union(CVarFlags::REPLICATED),
    "LOOC в критическом состоянии.",
);
pub const DEAD_CHAT_ENABLED: CVarDef<bool> = CVarDef::new(
    "dead_chat.enabled",
    true,
    CVarFlags::NOTIFY.union(CVarFlags::REPLICATED),
    "Чат мёртвых.",
);

// Префиксы и расстояния (`SharedChatSystem`).

pub const RADIO_COMMON_PREFIX: char = ';';
pub const RADIO_CHANNEL_PREFIX: char = ':';
pub const RADIO_CHANNEL_ALT_PREFIX: char = '.';
pub const LOCAL_PREFIX: char = '>';
pub const CONSOLE_PREFIX: char = '/';
pub const DEAD_PREFIX: char = '\\';
pub const LOOC_PREFIX: char = '(';
pub const OOC_PREFIX: char = '[';
pub const EMOTES_PREFIX: char = '@';
pub const EMOTES_ALT_PREFIX: char = '*';
pub const ADMIN_PREFIX: char = ']';
pub const WHISPER_PREFIX: char = ',';
pub const DEFAULT_CHANNEL_KEY: char = 'h';
/// Слышно голос (`VoiceRange`).
pub const VOICE_RANGE: f32 = 10.0;
/// Шёпот разборчив (`WhisperClearRange`).
pub const WHISPER_CLEAR_RANGE: f32 = 2.0;
/// Шёпот слышен вообще (`WhisperMuffledRange`).
pub const WHISPER_MUFFLED_RANGE: f32 = 5.0;
pub const COMMON_CHANNEL: &str = "Common";
pub const DEFAULT_SPEECH_VERB: &str = "Default";
/// Звук объявлений (`DefaultAnnouncementSound`).
pub const ANNOUNCEMENT_SOUND: &str = "/Audio/Announcements/announce.ogg";

// Каналы и сообщения.

/// Канал чата (`ChatChannel`): битовые флаги, как в SS14.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChatChannel(pub u16);

impl ChatChannel {
    pub const NONE: Self = Self(0);
    pub const LOCAL: Self = Self(1 << 0);
    pub const WHISPER: Self = Self(1 << 1);
    pub const SERVER: Self = Self(1 << 2);
    pub const DAMAGE: Self = Self(1 << 3);
    pub const RADIO: Self = Self(1 << 4);
    pub const LOOC: Self = Self(1 << 5);
    pub const OOC: Self = Self(1 << 6);
    pub const VISUAL: Self = Self(1 << 7);
    pub const NOTIFICATIONS: Self = Self(1 << 8);
    pub const EMOTES: Self = Self(1 << 9);
    pub const DEAD: Self = Self(1 << 10);
    pub const ADMIN: Self = Self(1 << 11);
    pub const ADMIN_ALERT: Self = Self(1 << 12);
    pub const ADMIN_CHAT: Self = Self(1 << 13);
    pub const UNSPECIFIED: Self = Self(1 << 14);
    /// Каналы персонажа (`IC`).
    pub const IC: Self = Self(
        Self::LOCAL.0
            | Self::WHISPER.0
            | Self::RADIO.0
            | Self::DEAD.0
            | Self::EMOTES.0
            | Self::DAMAGE.0
            | Self::VISUAL.0
            | Self::NOTIFICATIONS.0,
    );
    pub const ADMIN_RELATED: Self = Self(Self::ADMIN.0 | Self::ADMIN_ALERT.0 | Self::ADMIN_CHAT.0);

    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

/// Сообщение чата (`ChatMessage`, `MsgChatMessage`). Сервер → клиент.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub channel: ChatChannel,
    pub message: String,
    /// Сообщение с разметкой: имя, глагол, кавычки.
    pub wrapped_message: String,
    pub sender: Option<NetEntity>,
    /// Номер автора для удаления его сообщений (`SenderKey`).
    pub sender_key: Option<i32>,
    /// Только облачко, без строки в чате.
    pub hide_chat: bool,
    /// Цвет `#RRGGBB`, если не цвет канала.
    pub color_override: Option<String>,
    pub audio_path: Option<String>,
    pub audio_volume: f32,
}

/// Как далеко идёт сообщение (`ChatTransmitRange`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ChatTransmitRange {
    #[default]
    Normal,
    GhostRangeLimit,
    HideChat,
    NoGhosts,
}

/// Речь персонажа (`InGameICChatType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InGameIcChatType {
    Speak,
    Emote,
    Whisper,
}

/// Внеигровое в мире (`InGameOOCChatType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InGameOocChatType {
    Looc,
    Dead,
}

/// OOC сервера (`OOCChatType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OocChatType {
    Ooc,
    Admin,
}

// Компоненты.

/// Речь (`SpeechComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Speech {
    pub enabled: bool,
    pub speech_sounds: Option<String>,
    pub speech_verb: String,
    pub allowed_emotes: Vec<String>,
    /// Окончание фразы (ключ перевода) → глагол речи.
    pub suffix_speech_verbs: BTreeMap<String, String>,
    pub sound_cooldown_time: f32,
    #[serde(skip)]
    pub last_time_sound_played: Option<Duration>,
}

impl Default for Speech {
    fn default() -> Self {
        Self {
            enabled: true,
            speech_sounds: None,
            speech_verb: DEFAULT_SPEECH_VERB.into(),
            allowed_emotes: Vec::new(),
            suffix_speech_verbs: [
                (
                    "chat-speech-verb-suffix-exclamation-strong",
                    "DefaultExclamationStrong",
                ),
                ("chat-speech-verb-suffix-exclamation", "DefaultExclamation"),
                ("chat-speech-verb-suffix-question", "DefaultQuestion"),
                ("chat-speech-verb-suffix-stutter", "DefaultStutter"),
                ("chat-speech-verb-suffix-mumble", "DefaultMumble"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
            sound_cooldown_time: 0.5,
            last_time_sound_played: None,
        }
    }
}

/// Голос для звуков эмоций (`VocalComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Vocal {
    pub emote_sounds: Option<String>,
    pub scream_id: String,
    pub wilhelm: Option<SoundSpecifier>,
    pub wilhelm_probability: f32,
}

impl Default for Vocal {
    fn default() -> Self {
        Self {
            emote_sounds: None,
            scream_id: "Scream".into(),
            wilhelm: None,
            wilhelm_probability: 0.0002,
        }
    }
}

/// Призрак (`GhostComponent`): говорит в чат мёртвых. Поля переносятся в 6.5.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ghost {}

/// Слышит всю речь на карте (`GhostHearingComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GhostHearing {}

// События.

/// Сущность сказала фразу (`EntitySpokeEvent`); `channel` — радиоканал.
#[derive(Debug, Clone)]
pub struct EntitySpoke {
    pub source: Entity,
    pub message: String,
    pub channel: Option<String>,
    pub obfuscated_message: Option<String>,
}

/// Изменение речи акцентами (`TransformSpeechEvent`).
#[derive(Debug, Clone)]
pub struct TransformSpeech {
    pub sender: Entity,
    pub message: String,
}

/// Имя говорящего (`TransformSpeakerNameEvent`).
#[derive(Debug, Clone)]
pub struct TransformSpeakerName {
    pub sender: Entity,
    pub voice_name: String,
    pub speech_verb: Option<String>,
}

/// Можно ли говорить или изображать (`SpeakAttemptEvent`, `EmoteAttemptEvent`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SpeakAttempt {
    pub cancelled: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EmoteAttempt {
    pub cancelled: bool,
}

/// Эмоция выполнена (`EmoteEvent`).
#[derive(Debug, Clone)]
pub struct EmoteEvent {
    pub emote: String,
    pub handled: bool,
}

/// Перед эмоцией (`BeforeEmoteEvent`): можно отменить, назвав мешающее.
#[derive(Debug, Clone)]
pub struct BeforeEmote {
    pub emote: String,
    pub cancelled: bool,
    pub blocker: Option<Entity>,
}

/// Получатели речи можно дополнить (`ExpandICChatRecipientsEvent`). Широковещательно.
#[derive(Debug, Clone)]
pub struct ExpandIcChatRecipients {
    pub source: Entity,
    pub voice_range: f32,
    pub recipients: BTreeMap<Entity, RecipientData>,
}

/// Получатель речи (`ICChatRecipientData`): −1 — наблюдатель вне радиуса.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecipientData {
    pub range: f32,
    pub observer: bool,
    pub hide_chat_override: Option<bool>,
}

// Прототипы.

/// Глагол речи (`SpeechVerbPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechVerb {
    pub id: String,
    pub speech_verb_strings: Vec<String>,
    pub font_id: String,
    pub font_size: i32,
    pub priority: i32,
    pub bold: bool,
}

fn text(node: &DataNode, key: &str) -> Option<String> {
    node.get(key)?.as_str().map(str::to_owned)
}

fn strings(node: Option<&DataNode>) -> Vec<String> {
    node.and_then(DataNode::as_seq)
        .map(|s| {
            s.iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Глагол речи по прототипу `speechVerb`.
pub fn speech_verb(prototypes: &Prototypes, id: &str) -> Option<SpeechVerb> {
    let node = prototypes.resolved_node("speechVerb", id)?;
    Some(SpeechVerb {
        id: id.to_owned(),
        speech_verb_strings: strings(node.get("speechVerbStrings")),
        font_id: text(node, "fontId").unwrap_or_else(|| "Default".into()),
        font_size: text(node, "fontSize")
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(12),
        priority: text(node, "priority")
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(0),
        bold: text(node, "bold").is_some_and(|t| t == "true"),
    })
}

/// Эмоция (`EmotePrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct Emote {
    pub id: String,
    pub name: String,
    pub vocal: bool,
    pub available: bool,
    pub chat_messages: Vec<String>,
    pub chat_triggers: Vec<String>,
    pub whitelist: Option<crate::whitelist::EntityWhitelist>,
    pub blacklist: Option<crate::whitelist::EntityWhitelist>,
}

/// Эмоция по прототипу `emote`.
pub fn emote(prototypes: &Prototypes, id: &str) -> Option<Emote> {
    let node = prototypes.resolved_node("emote", id)?;
    let category = text(node, "category").unwrap_or_default();
    Some(Emote {
        id: id.to_owned(),
        name: text(node, "name").unwrap_or_else(|| id.to_owned()),
        vocal: category.split(',').any(|c| c.trim() == "Vocal"),
        available: text(node, "available").is_none_or(|t| t == "true"),
        chat_messages: strings(node.get("chatMessages")),
        chat_triggers: strings(node.get("chatTriggers")),
        whitelist: node
            .get("whitelist")
            .and_then(|n| astrocore_data::from_node(n).ok()),
        blacklist: node
            .get("blacklist")
            .and_then(|n| astrocore_data::from_node(n).ok()),
    })
}

/// Слова, вызывающие эмоции (`CacheEmotes`): слово в нижнем регистре → эмоция.
pub fn emote_words(prototypes: &Prototypes) -> BTreeMap<String, String> {
    let mut words = BTreeMap::new();
    let mut ids: Vec<&str> = prototypes.ids("emote").collect();
    ids.sort_unstable();
    for id in ids {
        if let Some(emote) = emote(prototypes, id) {
            for word in emote.chat_triggers {
                words
                    .entry(word.to_lowercase())
                    .or_insert_with(|| id.to_owned());
            }
        }
    }
    words
}

// Очистка текста.

/// Заглавная первая буква (`SanitizeMessageCapital`).
pub fn capitalize(message: &str) -> String {
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Отдельное «i» — заглавное (`SanitizeMessageCapitalizeTheWordI`).
pub fn capitalize_the_word_i(message: &str) -> String {
    let chars: Vec<char> = message.chars().collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let alone = c == 'i'
                && !chars.get(i + 1).is_some_and(|n| n.is_alphabetic())
                && !(i > 0 && chars[i - 1].is_alphabetic());
            if alone { 'I' } else { c }
        })
        .collect()
}

/// Точка в конце, если фраза кончается буквой (`SanitizeMessagePeriod`).
pub fn add_period(message: &str) -> String {
    if message.chars().last().is_some_and(char::is_alphabetic) {
        format!("{message}.")
    } else {
        message.to_owned()
    }
}

/// Слова по набору замен с сохранением регистра (`ReplacementAccentSystem.ApplyReplacements`).
pub fn apply_word_replacements(message: &str, replacements: &[(String, String)]) -> String {
    let mut result = message.to_owned();
    for (word, replacement) in replacements {
        if word.is_empty() {
            continue;
        }
        let lower_word = word.to_lowercase();
        let mut out = String::new();
        let chars: Vec<char> = result.chars().collect();
        let word_len = lower_word.chars().count();
        let mut i = 0;
        while i < chars.len() {
            let end = i + word_len;
            let candidate: String = chars[i..end.min(chars.len())].iter().collect();
            let boundary_before = i == 0 || !chars[i - 1].is_alphanumeric();
            let boundary_after = end >= chars.len() || !chars[end].is_alphanumeric();
            if end <= chars.len()
                && boundary_before
                && boundary_after
                && candidate.to_lowercase() == lower_word
            {
                let fixed = if candidate
                    .chars()
                    .all(|c| !c.is_alphabetic() || c.is_uppercase())
                    && candidate.chars().any(char::is_alphabetic)
                    && word_len > 1
                {
                    replacement.to_uppercase()
                } else if candidate.chars().next().is_some_and(char::is_uppercase) {
                    capitalize(replacement)
                } else {
                    replacement.clone()
                };
                out.push_str(&fixed);
                i = end;
            } else {
                out.push(chars[i]);
                i += 1;
            }
        }
        result = out;
    }
    result
}

/// Смайлики → ключ эмоции (`ChatSanitizationManager.ShorthandToEmote`).
pub const SHORTHANDS: &[(&str, &str)] = &[
    (":)", "chatsan-smiles"),
    (":]", "chatsan-smiles"),
    ("=)", "chatsan-smiles"),
    ("=]", "chatsan-smiles"),
    ("(:", "chatsan-smiles"),
    ("[:", "chatsan-smiles"),
    ("(=", "chatsan-smiles"),
    ("[=", "chatsan-smiles"),
    ("^^", "chatsan-smiles"),
    ("^-^", "chatsan-smiles"),
    (":(", "chatsan-frowns"),
    (":[", "chatsan-frowns"),
    ("=(", "chatsan-frowns"),
    ("=[", "chatsan-frowns"),
    ("):", "chatsan-frowns"),
    (")=", "chatsan-frowns"),
    ("]:", "chatsan-frowns"),
    ("]=", "chatsan-frowns"),
    (":D", "chatsan-smiles-widely"),
    ("D:", "chatsan-frowns-deeply"),
    (":O", "chatsan-surprised"),
    (":3", "chatsan-smiles"),
    (":S", "chatsan-uncertain"),
    (":>", "chatsan-grins"),
    (":<", "chatsan-pouts"),
    ("xD", "chatsan-laughs"),
    (":'(", "chatsan-cries"),
    (":'[", "chatsan-cries"),
    ("='(", "chatsan-cries"),
    ("='[", "chatsan-cries"),
    (")':", "chatsan-cries"),
    ("]':", "chatsan-cries"),
    (")'=", "chatsan-cries"),
    ("]'=", "chatsan-cries"),
    (";-;", "chatsan-cries"),
    (";_;", "chatsan-cries"),
    ("qwq", "chatsan-cries"),
    (":u", "chatsan-smiles-smugly"),
    (":v", "chatsan-smiles-smugly"),
    (">:i", "chatsan-annoyed"),
    (":i", "chatsan-sighs"),
    (":|", "chatsan-sighs"),
    (":p", "chatsan-stick-out-tongue"),
    (";p", "chatsan-stick-out-tongue"),
    (":b", "chatsan-stick-out-tongue"),
    ("0-0", "chatsan-wide-eyed"),
    ("o-o", "chatsan-wide-eyed"),
    ("o.o", "chatsan-wide-eyed"),
    ("._.", "chatsan-surprised"),
    (".-.", "chatsan-confused"),
    ("-_-", "chatsan-unimpressed"),
    ("smh", "chatsan-unimpressed"),
    (":?", "chatsan-shrugs"),
    ("o/", "chatsan-waves"),
    ("^^/", "chatsan-waves"),
    (":/", "chatsan-uncertain"),
    (":\\", "chatsan-uncertain"),
    ("lmao", "chatsan-laughs"),
    ("lmfao", "chatsan-laughs"),
    ("lol", "chatsan-laughs"),
    ("lel", "chatsan-laughs"),
    ("kek", "chatsan-laughs"),
    ("rofl", "chatsan-laughs"),
    ("o7", "chatsan-salutes"),
    (";_;7", "chatsan-tearfully-salutes"),
    (";)", "chatsan-winks"),
    (";]", "chatsan-winks"),
    ("(;", "chatsan-winks"),
    ("[;", "chatsan-winks"),
    (":')", "chatsan-tearfully-smiles"),
    (":']", "chatsan-tearfully-smiles"),
    ("=')", "chatsan-tearfully-smiles"),
    ("=']", "chatsan-tearfully-smiles"),
    ("(':", "chatsan-tearfully-smiles"),
    ("[':", "chatsan-tearfully-smiles"),
    ("('=", "chatsan-tearfully-smiles"),
    ("['=", "chatsan-tearfully-smiles"),
];

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() || matches!(c, '«' | '»' | '“' | '”' | '…' | '—' | '–' | '‘' | '’')
}

/// Вхождения смайлика как отдельного слова (`\sX(?=\p{P}|\s|$)` или `^X(?:\p{P}|(?=\s|$))`):
/// начало, конец удаляемого куска и начало самого смайлика.
fn shorthand_matches(message: &str, shorthand: &str) -> Vec<(usize, usize, usize)> {
    let lower: Vec<char> = message.to_lowercase().chars().collect();
    let chars: Vec<char> = message.chars().collect();
    if lower.len() != chars.len() {
        return Vec::new();
    }
    let target: Vec<char> = shorthand.to_lowercase().chars().collect();
    let mut found = Vec::new();
    let n = target.len();
    for start in 0..=chars.len().saturating_sub(n) {
        if start + n > chars.len() || lower[start..start + n] != target[..] {
            continue;
        }
        let after = chars.get(start + n).copied();
        if start == 0 {
            match after {
                Some(c) if is_punctuation(c) => found.push((0, n + 1, 0)),
                None => found.push((0, n, 0)),
                Some(c) if c.is_whitespace() => found.push((0, n, 0)),
                _ => {}
            }
        } else if chars[start - 1].is_whitespace()
            && after.is_none_or(|c| is_punctuation(c) || c.is_whitespace())
        {
            found.push((start - 1, start + n, start));
        }
    }
    found
}

/// Смайлики убираются, последний по месту становится эмоцией (`TrySanitizeEmoteShorthands`).
/// Возвращает очищенный текст и ключ эмоции.
pub fn sanitize_emote_shorthands(message: &str) -> (String, Option<&'static str>) {
    let mut last: Option<(usize, &'static str)> = None;
    let mut result = message.to_owned();
    for (shorthand, key) in SHORTHANDS {
        let original = shorthand_matches(message, shorthand);
        let Some(&(_, _, at)) = original.last() else {
            continue;
        };
        if last.is_none_or(|(index, _)| at > index) {
            last = Some((at, key));
        }
        // Убрать из текущего текста (справа налево — индексы не съезжают).
        let chars: Vec<char> = result.chars().collect();
        let mut keep = vec![true; chars.len()];
        for (from, to, _) in shorthand_matches(&result, shorthand) {
            for flag in &mut keep[from..to.min(chars.len())] {
                *flag = false;
            }
        }
        result = chars
            .iter()
            .zip(keep)
            .filter(|(_, k)| *k)
            .map(|(c, _)| *c)
            .collect();
    }
    (result.trim().to_owned(), last.map(|(_, key)| key))
}

/// Читаемость шёпота издали: часть букв — `~` (`ObfuscateMessageReadability`).
pub fn obfuscate(world: &mut World, message: &str, chance: f32) -> String {
    let mut random = crate::random::random(world);
    message
        .chars()
        .map(|c| {
            if c.is_whitespace() || !random.prob(1.0 - chance) {
                c
            } else {
                '~'
            }
        })
        .collect()
}

// Менеджер чата.

/// Авторы сообщений и учёт частоты (`ChatManager._players`, `PlayerRateLimitManager`).
#[derive(Resource, Debug, Default)]
pub struct ChatUsers {
    keys: BTreeMap<NetUserId, i32>,
    recent: BTreeMap<NetUserId, Vec<Duration>>,
}

impl ChatUsers {
    /// Номер автора (`EnsurePlayer`).
    pub fn key(&mut self, user: NetUserId) -> i32 {
        let next = self.keys.len() as i32;
        *self.keys.entry(user).or_insert(next)
    }
}

fn now(world: &World) -> Duration {
    world
        .get_resource::<GameTime>()
        .map(GameTime::cur_time)
        .unwrap_or_default()
}

fn session_user(world: &World, session: Entity) -> Option<NetUserId> {
    world.get::<Session>(session).map(|s| s.user_id)
}

fn sessions(world: &mut World) -> Vec<Entity> {
    let mut list: Vec<Entity> = world
        .query_filtered::<Entity, bevy_ecs::query::With<Session>>()
        .iter(world)
        .collect();
    list.sort();
    list
}

fn is_admin(world: &World, session: Entity) -> bool {
    crate::map_commands::is_admin(world, session)
}

fn admins(world: &mut World) -> Vec<Entity> {
    sessions(world)
        .into_iter()
        .filter(|s| is_admin(world, *s))
        .collect()
}

/// Частота сообщений игрока (`HandleRateLimit`): за окно — не больше заданного.
pub fn rate_limit_allows(world: &mut World, session: Entity) -> bool {
    let Some(user) = session_user(world, session) else {
        return true;
    };
    let time = now(world);
    let period = Duration::from_secs_f32(world.cvar(&CHAT_RATE_LIMIT_PERIOD).max(0.0));
    let count = world.cvar(&CHAT_RATE_LIMIT_COUNT).max(1) as usize;
    let allowed = {
        let mut users = world.get_resource_or_insert_with(ChatUsers::default);
        let recent = users.recent.entry(user).or_default();
        recent.retain(|t| time.saturating_sub(*t) < period);
        if recent.len() >= count {
            false
        } else {
            recent.push(time);
            true
        }
    };
    if !allowed {
        let text = loc(world, "chat-manager-rate-limited", &[]);
        dispatch_server_message(world, session, &text);
    }
    allowed
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn chat_message(
    world: &mut World,
    channel: ChatChannel,
    message: &str,
    wrapped: &str,
    source: Option<Entity>,
    hide_chat: bool,
    color: Option<String>,
    author: Option<NetUserId>,
) -> ChatMessage {
    let sender_key = author.map(|a| world.get_resource_or_insert_with(ChatUsers::default).key(a));
    ChatMessage {
        channel,
        message: message.to_owned(),
        wrapped_message: wrapped.to_owned(),
        sender: source.and_then(|s| world.get::<NetEntity>(s).copied()),
        sender_key,
        hide_chat,
        color_override: color,
        audio_path: None,
        audio_volume: 0.0,
    }
}

/// Сообщение одному игроку (`ChatMessageToOne`).
#[allow(clippy::too_many_arguments)]
pub fn chat_message_to_one(
    world: &mut World,
    channel: ChatChannel,
    message: &str,
    wrapped: &str,
    source: Option<Entity>,
    hide_chat: bool,
    session: Entity,
    author: Option<NetUserId>,
) {
    let msg = chat_message(
        world, channel, message, wrapped, source, hide_chat, None, author,
    );
    world.send_net_event(NetTarget::Session(session), &msg);
    world.raise_broadcast(&mut ChatMessageSent {
        message: msg,
        recipients: vec![session],
    });
}

/// Готовое сообщение игрокам (`ServerSendMessage`).
pub fn deliver(world: &mut World, message: ChatMessage, sessions: &[Entity]) {
    for session in sessions {
        world.send_net_event(NetTarget::Session(*session), &message);
    }
    world.raise_broadcast(&mut ChatMessageSent {
        message,
        recipients: sessions.to_vec(),
    });
}

/// Сообщение многим (`ChatMessageToMany`).
#[allow(clippy::too_many_arguments)]
pub fn chat_message_to_many(
    world: &mut World,
    channel: ChatChannel,
    message: &str,
    wrapped: &str,
    source: Option<Entity>,
    hide_chat: bool,
    recipients: &[Entity],
    color: Option<String>,
    author: Option<NetUserId>,
) {
    let msg = chat_message(
        world, channel, message, wrapped, source, hide_chat, color, author,
    );
    for session in recipients {
        world.send_net_event(NetTarget::Session(*session), &msg);
    }
    world.raise_broadcast(&mut ChatMessageSent {
        message: msg,
        recipients: recipients.to_vec(),
    });
}

/// Сообщение всем (`ChatMessageToAll`).
#[allow(clippy::too_many_arguments)]
pub fn chat_message_to_all(
    world: &mut World,
    channel: ChatChannel,
    message: &str,
    wrapped: &str,
    source: Option<Entity>,
    hide_chat: bool,
    color: Option<String>,
    author: Option<NetUserId>,
) {
    let recipients = sessions(world);
    chat_message_to_many(
        world,
        channel,
        message,
        wrapped,
        source,
        hide_chat,
        &recipients,
        color,
        author,
    );
}

/// Сообщение ушло получателям. Широковещательно на сервере — для проверок и повторов.
#[derive(Debug, Clone)]
pub struct ChatMessageSent {
    pub message: ChatMessage,
    pub recipients: Vec<Entity>,
}

/// Объявление сервера всем (`DispatchServerAnnouncement`).
pub fn dispatch_server_announcement(world: &mut World, message: &str) {
    let wrapped = loc(
        world,
        "chat-manager-server-wrap-message",
        &[("message", LocArg::from(escape_text(message)))],
    );
    chat_message_to_all(
        world,
        ChatChannel::SERVER,
        message,
        &wrapped,
        None,
        false,
        None,
        None,
    );
    add_admin_log(
        world,
        LogType::Chat,
        LogImpact::Low,
        &format!("Server announcement from SYSTEM: {message}"),
        &[],
        &[],
    );
}

/// Сообщение сервера одному игроку (`DispatchServerMessage`).
pub fn dispatch_server_message(world: &mut World, session: Entity, message: &str) {
    let wrapped = loc(
        world,
        "chat-manager-server-wrap-message",
        &[("message", LocArg::from(escape_text(message)))],
    );
    chat_message_to_one(
        world,
        ChatChannel::SERVER,
        message,
        &wrapped,
        None,
        false,
        session,
        None,
    );
}

/// Объявление администраторам (`SendAdminAnnouncement`).
pub fn send_admin_announcement(world: &mut World, message: &str) {
    let channel_name = loc(world, "chat-manager-admin-channel-name", &[]);
    let wrapped = loc(
        world,
        "chat-manager-send-admin-announcement-wrap-message",
        &[
            ("adminChannelName", LocArg::from(channel_name)),
            ("message", LocArg::from(escape_text(message))),
        ],
    );
    let recipients = admins(world);
    chat_message_to_many(
        world,
        ChatChannel::ADMIN,
        message,
        &wrapped,
        None,
        false,
        &recipients,
        None,
        None,
    );
}

/// Сообщение длиннее предела (`MessageCharacterLimit`): игрок узнаёт об этом.
fn over_limit(world: &mut World, session: Option<Entity>, message: &str) -> bool {
    let Some(session) = session else {
        return false;
    };
    let limit = world.cvar(&CHAT_MAX_MESSAGE_LENGTH).max(0) as usize;
    if message.chars().count() <= limit {
        return false;
    }
    let text = loc(
        world,
        "chat-manager-max-message-length-exceeded-message",
        &[("limit", LocArg::from(limit))],
    );
    dispatch_server_message(world, session, &text);
    true
}

/// OOC или чат администраторов (`TrySendOOCMessage`).
pub fn try_send_ooc_message(world: &mut World, session: Entity, message: &str, kind: OocChatType) {
    if !rate_limit_allows(world, session) || over_limit(world, Some(session), message) {
        return;
    }
    let Some(user) = session_user(world, session) else {
        return;
    };
    let name = world
        .get::<Session>(session)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    match kind {
        OocChatType::Ooc => {
            let enabled = if is_admin(world, session) {
                world.cvar(&OOC_ENABLED_ADMIN)
            } else {
                world.cvar(&OOC_ENABLED)
            };
            if !enabled {
                return;
            }
            let wrapped = loc(
                world,
                "chat-manager-send-ooc-wrap-message",
                &[
                    ("playerName", LocArg::from(name.as_str())),
                    ("message", LocArg::from(escape_text(message))),
                ],
            );
            chat_message_to_all(
                world,
                ChatChannel::OOC,
                message,
                &wrapped,
                None,
                false,
                None,
                Some(user),
            );
            add_admin_log(
                world,
                LogType::Chat,
                LogImpact::Low,
                &format!("OOC from {name}: {message}"),
                &[],
                &[user],
            );
        }
        OocChatType::Admin => {
            if !is_admin(world, session) {
                add_admin_log(
                    world,
                    LogType::Chat,
                    LogImpact::Extreme,
                    &format!("{name} attempted to send admin message but was not admin"),
                    &[],
                    &[user],
                );
                return;
            }
            let channel_name = loc(world, "chat-manager-admin-channel-name", &[]);
            let wrapped = loc(
                world,
                "chat-manager-send-admin-chat-wrap-message",
                &[
                    ("adminChannelName", LocArg::from(channel_name)),
                    ("playerName", LocArg::from(escape_text(&name))),
                    ("message", LocArg::from(escape_text(message))),
                ],
            );
            let recipients = admins(world);
            chat_message_to_many(
                world,
                ChatChannel::ADMIN_CHAT,
                message,
                &wrapped,
                None,
                false,
                &recipients,
                None,
                Some(user),
            );
            add_admin_log(
                world,
                LogType::Chat,
                LogImpact::Low,
                &format!("Admin chat from {name}: {message}"),
                &[],
                &[user],
            );
        }
    }
}

// Речь в мире.

/// Можно ли говорить (`ActionBlockerSystem.CanSpeak`): речь включена, моб в сознании.
pub fn can_speak(world: &mut World, source: Entity) -> bool {
    if world.get::<Speech>(source).is_some_and(|s| !s.enabled) {
        return false;
    }
    let mut attempt = SpeakAttempt::default();
    world.raise(source, &mut attempt);
    !attempt.cancelled && !incapacitated(world, source)
}

/// Можно ли изображать (`CanEmote`).
pub fn can_emote(world: &mut World, source: Entity) -> bool {
    let mut attempt = EmoteAttempt::default();
    world.raise(source, &mut attempt);
    !attempt.cancelled && !incapacitated(world, source)
}

/// Критическое состояние или смерть (`MobStateSystem.CheckAct`).
fn incapacitated(world: &World, entity: Entity) -> bool {
    matches!(
        world.mob_state(entity),
        Some(MobState::Critical | MobState::Dead)
    )
}

/// Получатели речи (`GetRecipients`): игроки на той же карте в радиусе; слышащие всё —
/// и вне радиуса.
pub fn recipients(
    world: &mut World,
    source: Entity,
    range: f32,
) -> BTreeMap<Entity, RecipientData> {
    let mut found = BTreeMap::new();
    let Some(from) = world.map_coordinates(source) else {
        return found;
    };
    for session in sessions(world) {
        let Some(listener) = world.get::<Session>(session).and_then(|s| s.attached) else {
            continue;
        };
        let Some(to) = world.map_coordinates(listener) else {
            continue;
        };
        if to.map != from.map {
            continue;
        }
        let observer = world.get::<GhostHearing>(listener).is_some();
        let distance = from.position.distance(to.position);
        if distance < range {
            found.insert(
                session,
                RecipientData {
                    range: distance,
                    observer,
                    hide_chat_override: None,
                },
            );
        } else if observer {
            found.insert(
                session,
                RecipientData {
                    range: -1.0,
                    observer: true,
                    hide_chat_override: None,
                },
            );
        }
    }
    let mut event = ExpandIcChatRecipients {
        source,
        voice_range: range,
        recipients: found,
    };
    world.raise_broadcast(&mut event);
    event.recipients
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeCheck {
    Disallowed,
    HideChat,
    Full,
}

/// Видит ли получатель сообщение в чате (`MessageRangeCheck`).
fn range_check(
    world: &World,
    session: Entity,
    data: RecipientData,
    range: ChatTransmitRange,
) -> RangeCheck {
    let initial = match range {
        ChatTransmitRange::Normal => RangeCheck::Full,
        ChatTransmitRange::GhostRangeLimit => {
            if data.observer && data.range < 0.0 && !is_admin(world, session) {
                RangeCheck::HideChat
            } else {
                RangeCheck::Full
            }
        }
        ChatTransmitRange::HideChat => RangeCheck::HideChat,
        ChatTransmitRange::NoGhosts => {
            if data.observer && !is_admin(world, session) {
                RangeCheck::Disallowed
            } else {
                RangeCheck::Full
            }
        }
    };
    match (data.hide_chat_override, initial) {
        (Some(true), RangeCheck::Full) => RangeCheck::HideChat,
        (Some(false), RangeCheck::HideChat) => RangeCheck::Full,
        _ => initial,
    }
}

/// Сообщение всем в радиусе голоса (`SendInVoiceRange`).
fn send_in_voice_range(
    world: &mut World,
    channel: ChatChannel,
    message: &str,
    wrapped: &str,
    source: Entity,
    range: ChatTransmitRange,
    author: Option<NetUserId>,
) {
    for (session, data) in recipients(world, source, VOICE_RANGE) {
        let check = range_check(world, session, data, range);
        if check == RangeCheck::Disallowed {
            continue;
        }
        chat_message_to_one(
            world,
            channel,
            message,
            wrapped,
            Some(source),
            check == RangeCheck::HideChat,
            session,
            author,
        );
    }
}

/// Речь после акцентов (`TransformSpeech`).
pub fn transform_speech(world: &mut World, sender: Entity, message: &str) -> String {
    let mut event = TransformSpeech {
        sender,
        message: message.to_owned(),
    };
    world.raise(sender, &mut event);
    event.message
}

/// Имя говорящего после подмен (`TransformSpeakerNameEvent`).
pub fn speaker_name(world: &mut World, source: Entity) -> TransformSpeakerName {
    let mut event = TransformSpeakerName {
        sender: source,
        voice_name: world.entity_name(source),
        speech_verb: None,
    };
    world.raise(source, &mut event);
    event
}

/// Глагол речи по окончанию фразы (`GetSpeechVerb`).
pub fn get_speech_verb(world: &World, source: Entity, message: &str) -> Option<SpeechVerb> {
    let prototypes = world.get_resource::<Prototypes>()?;
    let Some(speech) = world.get::<Speech>(source) else {
        return speech_verb(prototypes, DEFAULT_SPEECH_VERB);
    };
    let mut current: Option<SpeechVerb> = None;
    for (suffix, id) in &speech.suffix_speech_verbs {
        let Some(verb) = speech_verb(prototypes, id) else {
            continue;
        };
        let ending = loc(world, suffix, &[]);
        if message.ends_with(&ending) && verb.priority >= current.as_ref().map_or(0, |c| c.priority)
        {
            current = Some(verb);
        }
    }
    current.or_else(|| speech_verb(prototypes, &speech.speech_verb))
}

pub(crate) fn pick<'a>(world: &mut World, items: &'a [String]) -> Option<&'a String> {
    if items.is_empty() {
        return None;
    }
    let index = crate::random::random(world).range(0, items.len() as i32) as usize;
    items.get(index)
}

/// Сказать вслух (`SendEntitySpeak`).
fn send_entity_speak(
    world: &mut World,
    source: Entity,
    original: &str,
    range: ChatTransmitRange,
    name_override: Option<&str>,
    hide_log: bool,
    ignore_blocker: bool,
) {
    if !ignore_blocker && !can_speak(world, source) {
        return;
    }
    let message = transform_speech(world, source, original);
    if message.is_empty() {
        return;
    }
    let mut verb = get_speech_verb(world, source, &message);
    let name = match name_override {
        Some(name) => name.to_owned(),
        None => {
            let event = speaker_name(world, source);
            if let Some(id) = &event.speech_verb
                && let Some(v) = world
                    .get_resource::<Prototypes>()
                    .and_then(|p| speech_verb(p, id))
            {
                verb = Some(v);
            }
            event.voice_name
        }
    };
    let verb = verb.unwrap_or(SpeechVerb {
        id: DEFAULT_SPEECH_VERB.into(),
        speech_verb_strings: vec!["chat-speech-verb-default".into()],
        font_id: "Default".into(),
        font_size: 12,
        priority: 0,
        bold: false,
    });
    let verb_text = pick(world, &verb.speech_verb_strings)
        .cloned()
        .map(|key| loc(world, &key, &[]))
        .unwrap_or_default();
    let key = if verb.bold {
        "chat-manager-entity-say-bold-wrap-message"
    } else {
        "chat-manager-entity-say-wrap-message"
    };
    let wrapped = loc(
        world,
        key,
        &[
            ("entityName", LocArg::from(escape_text(&name))),
            ("verb", LocArg::from(verb_text)),
            ("fontType", LocArg::from(verb.font_id.as_str())),
            ("fontSize", LocArg::from(verb.font_size)),
            ("message", LocArg::from(escape_text(&message))),
        ],
    );
    send_in_voice_range(
        world,
        ChatChannel::LOCAL,
        &message,
        &wrapped,
        source,
        range,
        None,
    );
    world.raise_and_broadcast(
        source,
        &mut EntitySpoke {
            source,
            message: message.clone(),
            channel: None,
            obfuscated_message: None,
        },
    );
    play_speech_sound(world, source, &message);
    if !hide_log && world.get::<astrocore_net::Actor>(source).is_some() {
        let real = world.entity_name(source);
        let who = if name == real {
            String::new()
        } else {
            format!(" as {name}")
        };
        let text = if original == message {
            format!("Say from {real}{who}: {original}.")
        } else {
            format!("Say from {real}{who}, original: {original}, transformed: {message}.")
        };
        add_admin_log(world, LogType::Chat, LogImpact::Low, &text, &[], &[]);
    }
}

/// Шёпот (`SendEntityWhisper`): рядом — разборчиво, дальше — часть букв, вне видимости —
/// «кто-то». С каналом — шёпот в радио.
#[allow(clippy::too_many_arguments)]
fn send_entity_whisper(
    world: &mut World,
    source: Entity,
    original: &str,
    range: ChatTransmitRange,
    channel: Option<String>,
    name_override: Option<&str>,
    hide_log: bool,
    ignore_blocker: bool,
) {
    if !ignore_blocker && !can_speak(world, source) {
        return;
    }
    let stripped = crate::examine::strip_markup(original);
    let message = transform_speech(world, source, &stripped);
    if message.is_empty() {
        return;
    }
    let obfuscated = obfuscate(world, &message, 0.2);
    let identity = escape_text(
        name_override
            .map_or_else(|| world.entity_name(source), str::to_owned)
            .as_str(),
    );
    let name = escape_text(&match name_override {
        Some(name) => name.to_owned(),
        None => speaker_name(world, source).voice_name,
    });
    let wrapped = loc(
        world,
        "chat-manager-entity-whisper-wrap-message",
        &[
            ("entityName", LocArg::from(name.as_str())),
            ("message", LocArg::from(escape_text(&message))),
        ],
    );
    let wrapped_obfuscated = loc(
        world,
        "chat-manager-entity-whisper-wrap-message",
        &[
            ("entityName", LocArg::from(identity.as_str())),
            ("message", LocArg::from(escape_text(&obfuscated))),
        ],
    );
    let wrapped_unknown = loc(
        world,
        "chat-manager-entity-whisper-unknown-wrap-message",
        &[("message", LocArg::from(escape_text(&obfuscated)))],
    );
    for (session, data) in recipients(world, source, WHISPER_MUFFLED_RANGE) {
        let Some(listener) = world.get::<Session>(session).and_then(|s| s.attached) else {
            continue;
        };
        if range_check(world, session, data, range) != RangeCheck::Full {
            continue;
        }
        if data.range <= WHISPER_CLEAR_RANGE || data.observer {
            chat_message_to_one(
                world,
                ChatChannel::WHISPER,
                &message,
                &wrapped,
                Some(source),
                false,
                session,
                None,
            );
        } else if crate::examine::entities_in_range_unoccluded(
            world,
            source,
            listener,
            WHISPER_MUFFLED_RANGE,
        ) {
            chat_message_to_one(
                world,
                ChatChannel::WHISPER,
                &obfuscated,
                &wrapped_obfuscated,
                Some(source),
                false,
                session,
                None,
            );
        } else {
            chat_message_to_one(
                world,
                ChatChannel::WHISPER,
                &obfuscated,
                &wrapped_unknown,
                Some(source),
                false,
                session,
                None,
            );
        }
    }
    world.raise_and_broadcast(
        source,
        &mut EntitySpoke {
            source,
            message: message.clone(),
            channel,
            obfuscated_message: Some(obfuscated),
        },
    );
    if !hide_log {
        let real = world.entity_name(source);
        add_admin_log(
            world,
            LogType::Chat,
            LogImpact::Low,
            &format!("Whisper from {real}: {original}."),
            &[],
            &[],
        );
    }
}

/// Действие (`SendEntityEmote`): «Урист машет рукой» или «The crowbar rattles».
#[allow(clippy::too_many_arguments)]
fn send_entity_emote(
    world: &mut World,
    source: Entity,
    action: &str,
    range: ChatTransmitRange,
    name_override: Option<&str>,
    hide_log: bool,
    check_emote: bool,
    ignore_blocker: bool,
    author: Option<NetUserId>,
) {
    if !ignore_blocker && !can_emote(world, source) {
        return;
    }
    let entity = world.entity_arg(source);
    let name = escape_text(name_override.unwrap_or(&entity.name));
    let wrapped = loc(
        world,
        "chat-manager-entity-me-wrap-message",
        &[
            ("entityName", LocArg::from(name.as_str())),
            ("entity", LocArg::from(entity)),
            (
                "message",
                LocArg::from(crate::examine::strip_markup(action)),
            ),
        ],
    );
    if check_emote && !try_emote_chat_input(world, source, action) {
        return;
    }
    send_in_voice_range(
        world,
        ChatChannel::EMOTES,
        action,
        &wrapped,
        source,
        range,
        author,
    );
    if !hide_log {
        let real = world.entity_name(source);
        add_admin_log(
            world,
            LogType::Chat,
            LogImpact::Low,
            &format!("Emote from {real}: {action}"),
            &[],
            &[],
        );
    }
}

/// LOOC (`SendLOOC`).
fn send_looc(world: &mut World, source: Entity, session: Entity, message: &str, hide_chat: bool) {
    let enabled = if is_admin(world, session) {
        true
    } else {
        world.cvar(&LOOC_ENABLED)
    };
    if !enabled {
        return;
    }
    if !world.cvar(&LOOC_ENABLED_CRIT) && world.mob_state(source) == Some(MobState::Critical) {
        return;
    }
    let name = escape_text(&world.entity_name(source));
    let wrapped = loc(
        world,
        "chat-manager-entity-looc-wrap-message",
        &[
            ("entityName", LocArg::from(name.as_str())),
            ("message", LocArg::from(escape_text(message))),
        ],
    );
    let author = session_user(world, session);
    let range = if hide_chat {
        ChatTransmitRange::HideChat
    } else {
        ChatTransmitRange::Normal
    };
    send_in_voice_range(
        world,
        ChatChannel::LOOC,
        message,
        &wrapped,
        source,
        range,
        author,
    );
    add_admin_log(
        world,
        LogType::Chat,
        LogImpact::Low,
        &format!("LOOC from {name}: {message}"),
        &[],
        &[],
    );
}

/// Кто читает чат мёртвых (`GetDeadChatClients`): призраки и администраторы.
fn dead_chat_clients(world: &mut World) -> Vec<Entity> {
    sessions(world)
        .into_iter()
        .filter(|s| {
            is_admin(world, *s)
                || world
                    .get::<Session>(*s)
                    .and_then(|session| session.attached)
                    .is_some_and(|e| world.get::<Ghost>(e).is_some())
        })
        .collect()
}

/// Чат мёртвых (`SendDeadChat`).
fn send_dead_chat(
    world: &mut World,
    source: Entity,
    session: Entity,
    message: &str,
    hide_chat: bool,
) {
    let admin = is_admin(world, session);
    if !admin && !world.cvar(&DEAD_CHAT_ENABLED) {
        return;
    }
    let wrapped = if admin {
        let user = world
            .get::<Session>(session)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let channel = loc(world, "chat-manager-admin-channel-name", &[]);
        loc(
            world,
            "chat-manager-send-admin-dead-chat-wrap-message",
            &[
                ("adminChannelName", LocArg::from(channel)),
                ("userName", LocArg::from(escape_text(&user))),
                ("message", LocArg::from(escape_text(message))),
            ],
        )
    } else {
        let player = escape_text(&world.entity_name(source));
        let channel = loc(world, "chat-manager-dead-channel-name", &[]);
        loc(
            world,
            "chat-manager-send-dead-chat-wrap-message",
            &[
                ("deadChannelName", LocArg::from(channel)),
                ("playerName", LocArg::from(player)),
                ("message", LocArg::from(escape_text(message))),
            ],
        )
    };
    let clients = dead_chat_clients(world);
    let author = session_user(world, session);
    chat_message_to_many(
        world,
        ChatChannel::DEAD,
        message,
        &wrapped,
        Some(source),
        hide_chat,
        &clients,
        None,
        author,
    );
    add_admin_log(
        world,
        LogType::Chat,
        LogImpact::Low,
        &format!("Dead chat: {message}"),
        &[],
        &[],
    );
}

/// Радиоканал по клавише (`_keyCodes`).
pub fn radio_channels_by_key(prototypes: &Prototypes) -> BTreeMap<char, String> {
    let mut ids: Vec<&str> = prototypes.ids("radioChannel").collect();
    ids.sort_unstable();
    ids.into_iter()
        .filter_map(|id| {
            let node = prototypes.resolved_node("radioChannel", id)?;
            let key = text(node, "keycode")?.chars().next()?.to_ascii_lowercase();
            Some((key, id.to_owned()))
        })
        .collect()
}

/// Канал по умолчанию сущности (`GetDefaultRadioChannelEvent`).
#[derive(Debug, Clone, Default)]
pub struct GetDefaultRadioChannel {
    pub channel: Option<String>,
}

/// Префикс канала (`TryProcessRadioMessage`): `;` — общий, `:к` и `.к` — канал по клавише,
/// `:h` — канал по умолчанию. `Some((текст, канал))`, если это сообщение в радио.
pub fn try_process_radio_message(
    world: &mut World,
    source: Entity,
    input: &str,
    quiet: bool,
) -> Option<(String, Option<String>)> {
    let input = input.trim();
    let mut chars = input.chars();
    let first = chars.next()?;
    if first == RADIO_COMMON_PREFIX {
        return Some((
            capitalize(chars.as_str().trim_start()),
            Some(COMMON_CHANNEL.into()),
        ));
    }
    if first != RADIO_CHANNEL_PREFIX && first != RADIO_CHANNEL_ALT_PREFIX {
        return None;
    }
    let rest = chars.as_str();
    let Some(key) = rest.chars().next().filter(|c| !c.is_whitespace()) else {
        if !quiet {
            let text = loc(world, "chat-manager-no-radio-key", &[]);
            world.popup_entity_for(&text, source, source, PopupType::Small);
        }
        return Some((capitalize(rest.trim_start()), None));
    };
    let key = key.to_ascii_lowercase();
    let message = capitalize(rest[key.len_utf8()..].trim_start());
    if key == DEFAULT_CHANNEL_KEY {
        let mut event = GetDefaultRadioChannel::default();
        world.raise(source, &mut event);
        return Some((message, event.channel));
    }
    let channel = world
        .get_resource::<Prototypes>()
        .and_then(|p| radio_channels_by_key(p).get(&key).cloned());
    if channel.is_none() && !quiet {
        let text = loc(
            world,
            "chat-manager-no-such-channel",
            &[("key", LocArg::from(key.to_string()))],
        );
        world.popup_entity_for(&text, source, source, PopupType::Small);
    }
    Some((message, channel))
}

/// Префикс канала в начале текста (`GetRadioKeycodePrefix`).
fn radio_keycode_prefix(world: &World, input: &str) -> (String, String) {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= 2
        || !(chars[0] == RADIO_CHANNEL_PREFIX || chars[0] == RADIO_CHANNEL_ALT_PREFIX)
    {
        return (String::new(), input.to_owned());
    }
    let known = world
        .get_resource::<Prototypes>()
        .is_some_and(|p| radio_channels_by_key(p).contains_key(&chars[1].to_ascii_lowercase()));
    if !known {
        return (String::new(), input.to_owned());
    }
    (chars[..2].iter().collect(), chars[2..].iter().collect())
}

/// Слова для замены из прототипа `accent` (ключи и значения — переводы).
fn chat_sanitize_words(world: &World) -> Vec<(String, String)> {
    let Some(node) = world
        .get_resource::<Prototypes>()
        .and_then(|p| p.resolved_node("accent", "chatsanitize"))
        .and_then(|n| n.get("wordReplacements"))
        .and_then(DataNode::as_map)
    else {
        return Vec::new();
    };
    node.iter()
        .filter_map(|(k, v)| Some((loc(world, k, &[]), loc(world, v.as_str()?, &[]))))
        .collect()
}

/// Очистка речи (`SanitizeInGameICMessage`): замены слов, смайлики → эмоция, заглавная,
/// «I», точка. Возвращает текст и эмоцию из смайлика.
pub fn sanitize_in_game_ic(
    world: &mut World,
    source: Entity,
    message: &str,
    capital: bool,
    punctuate: bool,
) -> (String, Option<String>) {
    let words = chat_sanitize_words(world);
    let replaced = apply_word_replacements(message.trim(), &words);
    let (prefix, body) = radio_keycode_prefix(world, &replaced);
    let (body, emote) = if world.cvar(&CHAT_SANITIZER_ENABLED) {
        let (clean, key) = sanitize_emote_shorthands(&body);
        let emote = key.map(|key| {
            let entity = world.entity_arg(source);
            loc(world, key, &[("ent", LocArg::from(entity))])
        });
        (clean, emote)
    } else {
        (body, None)
    };
    let mut body = body;
    if capital {
        body = capitalize(&body);
    }
    body = capitalize_the_word_i(&body);
    if punctuate {
        body = add_period(&body);
    }
    (format!("{prefix}{body}"), emote)
}

/// Речь персонажа (`TrySendInGameICMessage`): призраки — в чат мёртвых; префикс `>` —
/// только голосом; префикс канала — шёпот в радио.
#[allow(clippy::too_many_arguments)]
pub fn try_send_in_game_ic_message(
    world: &mut World,
    source: Entity,
    message: &str,
    desired: InGameIcChatType,
    range: ChatTransmitRange,
    hide_log: bool,
    session: Option<Entity>,
    name_override: Option<&str>,
    check_radio_prefix: bool,
    ignore_blocker: bool,
) {
    if world.get::<Ghost>(source).is_some() {
        try_send_in_game_ooc_message(
            world,
            source,
            message,
            InGameOocChatType::Dead,
            range == ChatTransmitRange::HideChat,
            session,
        );
        return;
    }
    if let Some(session) = session {
        if !rate_limit_allows(world, session) {
            return;
        }
        if world.get::<Session>(session).and_then(|s| s.attached) != Some(source) {
            return;
        }
    }
    if over_limit(world, session, message) {
        return;
    }
    let mut message = message.to_owned();
    let mut check_radio_prefix = check_radio_prefix;
    if desired == InGameIcChatType::Speak && message.starts_with(LOCAL_PREFIX) {
        check_radio_prefix = false;
        message = message[LOCAL_PREFIX.len_utf8()..].to_owned();
    }
    let punctuate = world.cvar(&IC_PUNCTUATION);
    let (message, emote_str) = sanitize_in_game_ic(
        world,
        source,
        &message,
        desired != InGameIcChatType::Emote,
        punctuate,
    );
    if session.is_some()
        && let Some(emote) = emote_str.filter(|e| *e != message)
    {
        send_entity_emote(
            world,
            source,
            &emote,
            range,
            name_override,
            false,
            true,
            ignore_blocker,
            None,
        );
    }
    if message.is_empty() {
        return;
    }
    if check_radio_prefix
        && let Some((text, channel)) = try_process_radio_message(world, source, &message, false)
    {
        send_entity_whisper(
            world,
            source,
            &text,
            range,
            channel,
            name_override,
            hide_log,
            ignore_blocker,
        );
        return;
    }
    match desired {
        InGameIcChatType::Speak => {
            send_entity_speak(
                world,
                source,
                &message,
                range,
                name_override,
                hide_log,
                ignore_blocker,
            );
        }
        InGameIcChatType::Whisper => {
            send_entity_whisper(
                world,
                source,
                &message,
                range,
                None,
                name_override,
                hide_log,
                ignore_blocker,
            );
        }
        InGameIcChatType::Emote => {
            send_entity_emote(
                world,
                source,
                &message,
                range,
                name_override,
                hide_log,
                true,
                ignore_blocker,
                None,
            );
        }
    }
}

/// LOOC или чат мёртвых (`TrySendInGameOOCMessage`): мёртвым LOOC — чат мёртвых.
pub fn try_send_in_game_ooc_message(
    world: &mut World,
    source: Entity,
    message: &str,
    kind: InGameOocChatType,
    hide_chat: bool,
    session: Option<Entity>,
) {
    let Some(session) = session else {
        return;
    };
    if over_limit(world, Some(session), message) || !rate_limit_allows(world, session) {
        return;
    }
    if world.get::<Session>(session).and_then(|s| s.attached) != Some(source) {
        return;
    }
    let message = escape_text(message.trim());
    let dead =
        world.get::<Ghost>(source).is_some() || world.mob_state(source) == Some(MobState::Dead);
    let kind = if is_admin(world, session) || world.cvar(&LOOC_ENABLED_DEAD) || !dead {
        kind
    } else {
        InGameOocChatType::Dead
    };
    if !world.cvar(&LOOC_ENABLED_CRIT) && world.mob_state(source) == Some(MobState::Critical) {
        return;
    }
    match kind {
        InGameOocChatType::Dead => send_dead_chat(world, source, session, &message, hide_chat),
        InGameOocChatType::Looc => send_looc(world, source, session, &message, hide_chat),
    }
}

// Эмоции.

/// Можно ли этой сущности эмоцию (`AllowedToUseEmote`).
fn allowed_to_use_emote(world: &World, source: Entity, emote: &Emote) -> bool {
    if world
        .get::<Speech>(source)
        .is_some_and(|s| s.allowed_emotes.contains(&emote.id))
    {
        return true;
    }
    if emote
        .whitelist
        .as_ref()
        .is_some_and(|w| !w.is_valid(world, source))
        || emote
            .blacklist
            .as_ref()
            .is_some_and(|b| b.is_valid(world, source))
    {
        return false;
    }
    emote.available
}

/// Эмоция без строки чата (`TryInvokeEmoteEvent`): можно отменить; звуки — голосу.
fn try_invoke_emote(world: &mut World, source: Entity, emote: &Emote) -> bool {
    let mut before = BeforeEmote {
        emote: emote.id.clone(),
        cancelled: false,
        blocker: None,
    };
    world.raise(source, &mut before);
    if before.cancelled {
        let name = loc(world, &emote.name, &[]).to_lowercase();
        let text = match before.blocker {
            Some(blocker) => {
                let blocker = world.entity_arg(blocker);
                loc(
                    world,
                    "chat-system-emote-cancelled-blocked",
                    &[
                        ("emote", LocArg::from(name)),
                        ("blocker", LocArg::from(blocker)),
                    ],
                )
            }
            None => loc(
                world,
                "chat-system-emote-cancelled-generic",
                &[("emote", LocArg::from(name))],
            ),
        };
        world.popup_entity_for(&text, source, source, PopupType::Small);
        return false;
    }
    let mut event = EmoteEvent {
        emote: emote.id.clone(),
        handled: false,
    };
    world.raise(source, &mut event);
    if !event.handled && emote.vocal {
        play_emote_sound(world, source, &emote.id);
    }
    true
}

/// Слово действия, совпавшее с эмоцией, вызывает её (`TryEmoteChatInput`).
fn try_emote_chat_input(world: &mut World, source: Entity, text: &str) -> bool {
    let trimmed = text.to_lowercase().trim_matches(is_punctuation).to_owned();
    let Some(prototypes) = world.get_resource::<Prototypes>() else {
        return true;
    };
    let Some(emote) = emote_words(prototypes)
        .get(&trimmed)
        .and_then(|id| emote(prototypes, id))
    else {
        return true;
    };
    if !allowed_to_use_emote(world, source, &emote) {
        return true;
    }
    try_invoke_emote(world, source, &emote)
}

/// Эмоция со строкой чата (`TryEmoteWithChat`).
pub fn try_emote_with_chat(
    world: &mut World,
    source: Entity,
    emote_id: &str,
    range: ChatTransmitRange,
) -> bool {
    let Some(emote) = world
        .get_resource::<Prototypes>()
        .and_then(|p| emote(p, emote_id))
    else {
        return false;
    };
    if !allowed_to_use_emote(world, source, &emote) || !can_emote(world, source) {
        return false;
    }
    if !try_invoke_emote(world, source, &emote) {
        return false;
    }
    if let Some(key) = pick(world, &emote.chat_messages).cloned() {
        let entity = world.entity_arg(source);
        let action = loc(world, &key, &[("entity", LocArg::from(entity))]);
        send_entity_emote(
            world, source, &action, range, None, false, false, false, None,
        );
    }
    true
}

/// Звук эмоции голосом сущности (`VocalSystem.OnEmote`, `TryPlayEmoteSound`).
fn play_emote_sound(world: &mut World, source: Entity, emote: &str) {
    let Some(voice) = world
        .get::<Vocal>(source)
        .and_then(|v| v.emote_sounds.clone())
    else {
        return;
    };
    let Some(node) = world
        .get_resource::<Prototypes>()
        .and_then(|p| p.resolved_node("emoteSounds", &voice))
        .cloned()
    else {
        return;
    };
    let sound = node
        .get("sounds")
        .and_then(|s| s.get(emote))
        .or_else(|| node.get("fallbackSound"))
        .and_then(|n| astrocore_data::from_node::<SoundSpecifier>(n).ok());
    if let Some(sound) = sound {
        world.play_entity(&sound, source, None);
    }
}

/// Звук речи (`SpeechSoundSystem.OnEntitySpoke`): вопрос, восклицание, крик заглавными.
fn play_speech_sound(world: &mut World, source: Entity, message: &str) {
    let time = now(world);
    let Some(speech) = world.get::<Speech>(source).cloned() else {
        return;
    };
    let Some(id) = speech.speech_sounds.clone() else {
        return;
    };
    let cooldown = Duration::from_secs_f32(speech.sound_cooldown_time.max(0.0));
    if speech
        .last_time_sound_played
        .is_some_and(|last| time.saturating_sub(last) < cooldown)
    {
        return;
    }
    let Some(node) = world
        .get_resource::<Prototypes>()
        .and_then(|p| p.resolved_node("speechSounds", &id))
        .cloned()
    else {
        return;
    };
    let upper = message.chars().filter(|c| c.is_uppercase()).count();
    let mut key = match message.chars().last() {
        Some('?') => "askSound",
        Some('!') => "exclaimSound",
        _ => "saySound",
    };
    if upper > message.chars().count() / 2 {
        key = "exclaimSound";
    }
    let Some(sound) = node
        .get(key)
        .and_then(|n| astrocore_data::from_node::<SoundSpecifier>(n).ok())
    else {
        return;
    };
    if let Some(mut speech) = world.get_mut::<Speech>(source) {
        speech.last_time_sound_played = Some(time);
    }
    world.play_entity(&sound, source, None);
}

// Объявления.

fn wrap_announcement(
    world: &World,
    sender: &str,
    message: &str,
    signature: Option<&str>,
) -> String {
    let escaped = escape_text(message);
    match signature.filter(|s| !s.trim().is_empty()) {
        None => loc(
            world,
            "chat-manager-sender-announcement-wrap-message",
            &[
                ("sender", LocArg::from(sender)),
                ("message", LocArg::from(escaped)),
            ],
        ),
        Some(signature) => loc(
            world,
            "chat-manager-sender-announcement-wrap-message-signed",
            &[
                ("sender", LocArg::from(sender)),
                ("message", LocArg::from(escaped)),
                ("signature", LocArg::from(escape_text(signature))),
            ],
        ),
    }
}

fn announcement_sound(world: &mut World, recipients: &[Entity]) {
    let sound = SoundSpecifier::Path {
        path: ANNOUNCEMENT_SOUND.into(),
        params: Default::default(),
    };
    for session in recipients {
        if let Some(listener) = world.get::<Session>(*session).and_then(|s| s.attached) {
            world.play_entity(&sound, listener, None);
        }
    }
}

/// Объявление всем (`DispatchGlobalAnnouncement`).
pub fn dispatch_global_announcement(
    world: &mut World,
    message: &str,
    sender: Option<&str>,
    play_sound: bool,
    color: Option<String>,
) {
    let sender = sender.map_or_else(
        || loc(world, "chat-manager-sender-announcement", &[]),
        str::to_owned,
    );
    let wrapped = wrap_announcement(world, &sender, message, None);
    chat_message_to_all(
        world,
        ChatChannel::RADIO,
        message,
        &wrapped,
        None,
        false,
        color,
        None,
    );
    if play_sound {
        let all = sessions(world);
        announcement_sound(world, &all);
    }
    add_admin_log(
        world,
        LogType::Chat,
        LogImpact::Low,
        &format!("Global station announcement from {sender}: {message}"),
        &[],
        &[],
    );
}

/// Объявление станции (`DispatchStationAnnouncement`): тем, кто на её гридах.
pub fn dispatch_station_announcement(
    world: &mut World,
    source: Entity,
    message: &str,
    sender: Option<&str>,
    play_sound: bool,
    color: Option<String>,
) {
    let Some(station) = crate::stations::owning_station(world, source) else {
        return;
    };
    let grids: BTreeSet<Entity> = world
        .get::<crate::stations::StationData>(station)
        .map(|d| d.grids.iter().copied().collect())
        .unwrap_or_default();
    let sender = sender.map_or_else(
        || loc(world, "chat-manager-sender-announcement", &[]),
        str::to_owned,
    );
    let wrapped = wrap_announcement(world, &sender, message, None);
    let recipients: Vec<Entity> = sessions(world)
        .into_iter()
        .filter(|s| {
            world
                .get::<Session>(*s)
                .and_then(|session| session.attached)
                .and_then(|e| world.get::<Transform>(e))
                .and_then(Transform::grid)
                .is_some_and(|g| grids.contains(&g))
        })
        .collect();
    chat_message_to_many(
        world,
        ChatChannel::RADIO,
        message,
        &wrapped,
        Some(source),
        false,
        &recipients,
        color,
        None,
    );
    if play_sound {
        announcement_sound(world, &recipients);
    }
    add_admin_log(
        world,
        LogType::Chat,
        LogImpact::Low,
        &format!("Station Announcement from {sender}: {message}"),
        &[],
        &[],
    );
}

// Команды.

fn joined(args: &[String]) -> String {
    args.join(" ").trim().to_owned()
}

fn attached(world: &World, shell: &ConsoleShell) -> Option<(Entity, Entity)> {
    let session = shell.player?;
    let entity = world.get::<Session>(session)?.attached?;
    Some((session, entity))
}

fn ic_command(
    world: &mut World,
    shell: &mut ConsoleShell,
    args: &[String],
    kind: InGameIcChatType,
) {
    let Some((session, entity)) = attached(world, shell) else {
        shell.write_error("У вас нет сущности.");
        return;
    };
    let message = joined(args);
    if message.is_empty() {
        return;
    }
    try_send_in_game_ic_message(
        world,
        entity,
        &message,
        kind,
        ChatTransmitRange::Normal,
        false,
        Some(session),
        None,
        true,
        false,
    );
}

/// Команды, доступные любому игроку.
pub const CHAT_COMMANDS: [&str; 6] = ["say", "whisper", "me", "ooc", "looc", "dsay"];

fn command(
    name: &'static str,
    description: &'static str,
    help: &'static str,
    execute: astrocore_console::CommandFn,
) -> ConsoleCommand {
    ConsoleCommand {
        name,
        description,
        help,
        execute,
        complete: None,
    }
}

fn commands() -> Vec<ConsoleCommand> {
    vec![
        command(
            "say",
            "Сказать вслух.",
            "say <текст>",
            |world, shell, args| ic_command(world, shell, args, InGameIcChatType::Speak),
        ),
        command(
            "whisper",
            "Прошептать.",
            "whisper <текст>",
            |world, shell, args| ic_command(world, shell, args, InGameIcChatType::Whisper),
        ),
        command(
            "me",
            "Действие персонажа.",
            "me <действие>",
            |world, shell, args| ic_command(world, shell, args, InGameIcChatType::Emote),
        ),
        command(
            "ooc",
            "Сообщение вне игры всем.",
            "ooc <текст>",
            |world, shell, args| {
                let Some(session) = shell.player else {
                    shell.write_error("Команду выполняет игрок.");
                    return;
                };
                let message = joined(args);
                if !message.is_empty() {
                    try_send_ooc_message(world, session, &message, OocChatType::Ooc);
                }
            },
        ),
        command(
            "looc",
            "Сообщение вне игры рядом.",
            "looc <текст>",
            |world, shell, args| {
                let Some((session, entity)) = attached(world, shell) else {
                    shell.write_error("У вас нет сущности.");
                    return;
                };
                let message = joined(args);
                if !message.is_empty() {
                    try_send_in_game_ooc_message(
                        world,
                        entity,
                        &message,
                        InGameOocChatType::Looc,
                        false,
                        Some(session),
                    );
                }
            },
        ),
        command(
            "dsay",
            "Сообщение в чат мёртвых.",
            "dsay <текст>",
            |world, shell, args| {
                let Some((session, entity)) = attached(world, shell) else {
                    shell.write_error("У вас нет сущности.");
                    return;
                };
                let message = joined(args);
                if !message.is_empty() {
                    try_send_in_game_ooc_message(
                        world,
                        entity,
                        &message,
                        InGameOocChatType::Dead,
                        false,
                        Some(session),
                    );
                }
            },
        ),
        command(
            "asay",
            "Чат администраторов.",
            "asay <текст>",
            |world, shell, args| {
                let Some(session) = shell.player else {
                    shell.write_error("Команду выполняет игрок.");
                    return;
                };
                let message = joined(args);
                if !message.is_empty() {
                    try_send_ooc_message(world, session, &message, OocChatType::Admin);
                }
            },
        ),
    ]
}

/// Сообщения чата на клиенте: по порядку прихода.
#[derive(Resource, Debug, Clone, Default)]
pub struct ClientChat {
    pub messages: Vec<ChatMessage>,
    pub version: u64,
}

/// Сколько сообщений хранит клиент.
pub const CLIENT_CHAT_HISTORY: usize = 500;

pub fn install(world: &mut World, side: Side) {
    world.register_cvar(&CHAT_MAX_MESSAGE_LENGTH);
    world.register_cvar(&CHAT_RATE_LIMIT_PERIOD);
    world.register_cvar(&CHAT_RATE_LIMIT_COUNT);
    world.register_cvar(&CHAT_SANITIZER_ENABLED);
    world.register_cvar(&IC_PUNCTUATION);
    world.register_cvar(&OOC_ENABLED);
    world.register_cvar(&OOC_ENABLED_ADMIN);
    world.register_cvar(&OOC_ENABLE_DURING_ROUND);
    world.register_cvar(&LOOC_ENABLED);
    world.register_cvar(&LOOC_ENABLED_DEAD);
    world.register_cvar(&LOOC_ENABLED_CRIT);
    world.register_cvar(&DEAD_CHAT_ENABLED);
    world.register_data_component::<Speech>();
    world.register_net_serde::<Speech>();
    world.register_data_component::<Vocal>();
    world.register_data_component::<Ghost>();
    world.register_net_serde::<Ghost>();
    world.register_data_component::<GhostHearing>();
    world.register_net_event::<ChatMessage>();
    if !side.is_server() {
        world.init_resource::<ClientChat>();
        world.subscribe_broadcast::<astrocore_net::NetEvent<ChatMessage>>(|world, event| {
            let message = event.event.clone();
            #[cfg(feature = "client")]
            crate::chat_ui::on_message(world, &message);
            let mut chat = world.resource_mut::<ClientChat>();
            chat.messages.push(message);
            if chat.messages.len() > CLIENT_CHAT_HISTORY {
                let extra = chat.messages.len() - CLIENT_CHAT_HISTORY;
                chat.messages.drain(..extra);
            }
            chat.version += 1;
        });
        return;
    }
    world.init_resource::<ChatUsers>();
    // OOC в раунде выключается, в лобби и после — включается (`OnGameChange`).
    world.subscribe_broadcast::<crate::game_ticker::GameRunLevelChanged>(|world, event| {
        if world.cvar(&OOC_ENABLE_DURING_ROUND) {
            return;
        }
        let enabled = event.new != crate::game_ticker::GameRunLevel::InRound;
        if world.cvar(&OOC_ENABLED) != enabled {
            world.set_cvar(&OOC_ENABLED, enabled).ok();
        }
    });
    // Включение и выключение каналов объявляется (`OnOocEnabledChanged` и соседи).
    world.subscribe_broadcast::<astrocore_console::CVarChanged>(|world, event| {
        let enabled = matches!(event.value, astrocore_console::CVarValue::Bool(true));
        let key = match (event.name.as_str(), enabled) {
            ("ooc.enabled", true) => "chat-manager-ooc-chat-enabled-message",
            ("ooc.enabled", false) => "chat-manager-ooc-chat-disabled-message",
            ("looc.enabled", true) => "chat-manager-looc-chat-enabled-message",
            ("looc.enabled", false) => "chat-manager-looc-chat-disabled-message",
            ("looc.enabled_dead", true) => "chat-manager-dead-looc-chat-enabled-message",
            ("looc.enabled_dead", false) => "chat-manager-dead-looc-chat-disabled-message",
            ("looc.enabled_crit", true) => "chat-manager-crit-looc-chat-enabled-message",
            ("looc.enabled_crit", false) => "chat-manager-crit-looc-chat-disabled-message",
            ("dead_chat.enabled", true) => "chat-manager-dead-chat-enabled-message",
            ("dead_chat.enabled", false) => "chat-manager-dead-chat-disabled-message",
            _ => return,
        };
        let text = loc(world, key, &[]);
        dispatch_server_announcement(world, &text);
    });
    for command in commands() {
        world.register_command(command);
    }
}

/// Голос эмоций по голосу персонажа (`VocalSystem.LoadSounds`).
pub fn load_vocal_sounds(world: &mut World, entity: Entity, voice: &str) {
    let known = world
        .get_resource::<Prototypes>()
        .is_some_and(|p| p.resolved_node("emoteSounds", voice).is_some());
    if !known {
        return;
    }
    if let Some(mut vocal) = world.get_mut::<Vocal>(entity) {
        vocal.emote_sounds = Some(voice.to_owned());
    }
}

#[cfg(test)]
mod tests;
