//! Чат на клиенте: окно сообщений с разметкой SS14, строка ввода с выбором канала,
//! префиксами (`;`, `:к`, `,`, `@`, `[`, `(`, `\`, `/`) и клавишами каналов, отправка
//! командами `say`, `whisper`, `me`, `ooc`, `looc`, `dsay`, `asay`; облачка речи над
//! говорящими с очередью и затуханием. Образец — `ChatUIController` (`SplitInputContents`,
//! `SendMessage`, `ProcessChatMessage`, очередь облачков), `ChatManager.SendMessage`,
//! `ChatBox`, `ChannelSelectorButton`, `ChatChannelExtensions.TextColor`, `SpeechBubble`
//! клиента SS14 @ 2f4a2aad.
//!
//! Нет пока: фильтров каналов и счётчиков непрочитанного, админ-чата и чата мёртвых для
//! администраторов (клиент не знает прав, 6.6), подсветки слов и кодовых слов, цветных имён,
//! предпросмотра набора, индикатора набора, скрытия облачков за стенами, жирного и курсива
//! (у интерфейса пока один шрифт).

use std::collections::{HashMap, VecDeque};

use astrocore_client::{Game, KeyboardFocus, UiStyle};
use astrocore_console::ConfigWorldExt;
use astrocore_loc::LocArg;
use astrocore_map::TransformWorldExt;
use astrocore_math::Color as GameColor;
use astrocore_net::{ClientNet, NetEntities, RemoteConsoleWorldExt};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy_ecs::entity::Entity as GameEntity;
use bevy_ecs::world::World as GameWorld;

use crate::chat::{
    ADMIN_PREFIX, CHAT_MAX_MESSAGE_LENGTH, CONSOLE_PREFIX, ChatChannel, ChatMessage, ClientChat,
    DEAD_PREFIX, EMOTES_ALT_PREFIX, EMOTES_PREFIX, Ghost, LOCAL_PREFIX, LOOC_PREFIX, OOC_PREFIX,
    RADIO_CHANNEL_ALT_PREFIX, RADIO_CHANNEL_PREFIX, RADIO_COMMON_PREFIX, WHISPER_PREFIX,
};
use crate::examine::loc;

// ---------------------------------------------------------------- каналы ввода

/// Канал строки ввода (`ChatSelectChannel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SelectChannel {
    #[default]
    Local,
    Whisper,
    Radio,
    Looc,
    Ooc,
    Emotes,
    Dead,
    Admin,
    Console,
}

impl SelectChannel {
    /// Порядок перебора клавишей Tab: по значению флага в SS14.
    pub const ORDER: [SelectChannel; 9] = [
        SelectChannel::Local,
        SelectChannel::Whisper,
        SelectChannel::Radio,
        SelectChannel::Looc,
        SelectChannel::Ooc,
        SelectChannel::Emotes,
        SelectChannel::Dead,
        SelectChannel::Admin,
        SelectChannel::Console,
    ];

    /// Канал по префиксу (`PrefixToChannel`).
    pub fn from_prefix(prefix: char) -> Option<Self> {
        Some(match prefix {
            LOCAL_PREFIX => Self::Local,
            WHISPER_PREFIX => Self::Whisper,
            CONSOLE_PREFIX => Self::Console,
            LOOC_PREFIX => Self::Looc,
            OOC_PREFIX => Self::Ooc,
            EMOTES_PREFIX | EMOTES_ALT_PREFIX => Self::Emotes,
            ADMIN_PREFIX => Self::Admin,
            RADIO_COMMON_PREFIX => Self::Radio,
            DEAD_PREFIX => Self::Dead,
            _ => return None,
        })
    }

    /// Название для кнопки канала (`hud-chatbox-select-channel-*`).
    pub fn loc_key(self) -> &'static str {
        match self {
            Self::Local => "hud-chatbox-select-channel-Local",
            Self::Whisper => "hud-chatbox-select-channel-Whisper",
            Self::Radio => "hud-chatbox-select-channel-Radio",
            Self::Looc => "hud-chatbox-select-channel-LOOC",
            Self::Ooc => "hud-chatbox-select-channel-OOC",
            Self::Emotes => "hud-chatbox-select-channel-Emotes",
            Self::Dead => "hud-chatbox-select-channel-Dead",
            Self::Admin => "hud-chatbox-select-channel-Admin",
            Self::Console => "hud-chatbox-select-channel-Console",
        }
    }

    /// Цвет кнопки канала (`ChannelSelectorButton.ChannelSelectColor`).
    pub fn color(self) -> GameColor {
        let name = match self {
            Self::Radio => "limegreen",
            Self::Looc => "mediumturquoise",
            Self::Ooc => "lightskyblue",
            Self::Dead => "mediumpurple",
            Self::Admin => "hotpink",
            _ => "darkgray",
        };
        GameColor::parse(name).unwrap_or(GameColor::WHITE)
    }
}

/// Цвет строки канала (`ChatChannelExtensions.TextColor`).
pub fn channel_color(channel: ChatChannel) -> GameColor {
    let name = match channel {
        ChatChannel::SERVER => "orange",
        ChatChannel::RADIO => "limegreen",
        ChatChannel::LOOC => "mediumturquoise",
        ChatChannel::OOC => "lightskyblue",
        ChatChannel::DEAD => "mediumpurple",
        ChatChannel::ADMIN | ChatChannel::ADMIN_ALERT => "red",
        ChatChannel::ADMIN_CHAT => "hotpink",
        ChatChannel::WHISPER => "darkgray",
        _ => "lightgray",
    };
    GameColor::parse(name).unwrap_or(GameColor::WHITE)
}

/// Куда игрок может писать (`UpdateChannelPermissions`): консоль, OOC и LOOC всегда, речь —
/// с телом, чат мёртвых — призраку.
pub fn can_send_channels(world: &GameWorld) -> Vec<SelectChannel> {
    let mut channels = vec![
        SelectChannel::Console,
        SelectChannel::Ooc,
        SelectChannel::Looc,
    ];
    let attached = world.get_resource::<ClientNet>().and_then(|n| n.attached);
    if let Some(entity) = attached {
        if world.get::<Ghost>(entity).is_some() {
            channels.push(SelectChannel::Dead);
        } else {
            channels.extend([
                SelectChannel::Local,
                SelectChannel::Whisper,
                SelectChannel::Radio,
                SelectChannel::Emotes,
            ]);
        }
    }
    channels
}

fn is_ghost(world: &GameWorld) -> bool {
    world
        .get_resource::<ClientNet>()
        .and_then(|n| n.attached)
        .is_some_and(|e| world.get::<Ghost>(e).is_some())
}

/// Префикс радио в начале строки (`TryGetRadioChannel`): `;` или `:к`/`.к` известного канала.
pub fn radio_prefix_channel(world: &GameWorld, text: &str) -> Option<String> {
    let mut chars = text.chars();
    let first = chars.next()?;
    if first == RADIO_COMMON_PREFIX {
        return Some(crate::chat::COMMON_CHANNEL.into());
    }
    if first != RADIO_CHANNEL_PREFIX && first != RADIO_CHANNEL_ALT_PREFIX {
        return None;
    }
    let key = chars.next()?.to_ascii_lowercase();
    if key == crate::chat::DEFAULT_CHANNEL_KEY {
        return Some(String::new());
    }
    let prototypes = world.get_resource::<astrocore_prototypes::Prototypes>()?;
    crate::chat::radio_channels_by_key(prototypes)
        .get(&key)
        .cloned()
}

/// Канал и текст по префиксу строки (`SplitInputContents`). `None` — префикса нет.
pub fn split_input(
    world: &GameWorld,
    text: &str,
    allowed: &[SelectChannel],
) -> (Option<SelectChannel>, String) {
    let text = text.trim();
    let Some(first) = text.chars().next() else {
        return (None, String::new());
    };
    let channel = if radio_prefix_channel(world, text).is_some() {
        Some(SelectChannel::Radio)
    } else {
        SelectChannel::from_prefix(first)
    };
    let Some(mut channel) = channel.filter(|c| allowed.contains(c)) else {
        return (None, text.to_owned());
    };
    // Радио и «рядом» идут в `say` с префиксом.
    if channel == SelectChannel::Radio {
        return (Some(channel), text.to_owned());
    }
    if channel == SelectChannel::Local {
        if !is_ghost(world) {
            return (Some(channel), text.to_owned());
        }
        channel = SelectChannel::Dead;
    }
    let rest = text[first.len_utf8()..].trim_start().to_owned();
    (Some(channel), rest)
}

/// Кавычки и обратная косая в аргументе команды (`CommandParsing.Escape`).
pub fn escape_argument(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Команда для строки (`ChatManager.SendMessage`). `None` — писать некуда.
pub fn command_for(text: &str, channel: SelectChannel, ghost: bool) -> Option<String> {
    let quoted = format!("\"{}\"", escape_argument(text));
    Some(match channel {
        SelectChannel::Console => text.to_owned(),
        SelectChannel::Looc => format!("looc {quoted}"),
        SelectChannel::Ooc => format!("ooc {quoted}"),
        SelectChannel::Admin => format!("asay {quoted}"),
        SelectChannel::Emotes => format!("me {quoted}"),
        SelectChannel::Dead if ghost => format!("say {quoted}"),
        SelectChannel::Dead => format!("dsay {quoted}"),
        SelectChannel::Radio | SelectChannel::Local => format!("say {quoted}"),
        SelectChannel::Whisper => format!("whisper {quoted}"),
    })
}

/// Состояние строки ввода. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default)]
pub struct ChatInput {
    pub selected: SelectChannel,
    pub text: String,
    pub focused: bool,
    /// Сколько последних сообщений пропустить при листании назад.
    pub scroll: usize,
    pub version: u64,
}

/// Отправить набранное (`SendMessage`). Ошибка — строка предупреждения для окна.
pub fn submit(world: &mut GameWorld) -> Result<(), String> {
    let mut input = world.get_resource_or_insert_with(ChatInput::default);
    let text = std::mem::take(&mut input.text);
    let selected = input.selected;
    input.focused = false;
    input.scroll = 0;
    input.version += 1;
    if text.trim().is_empty() {
        return Ok(());
    }
    let allowed = can_send_channels(world);
    let (prefix, mut text) = split_input(world, &text, &allowed);
    let limit = usize::try_from(world.cvar(&CHAT_MAX_MESSAGE_LENGTH)).unwrap_or(1000);
    if text.chars().count() > limit {
        return Err(loc(
            world,
            "chat-manager-max-message-length",
            &[("maxMessageLength", LocArg::from(limit))],
        ));
    }
    let channel = match prefix {
        Some(channel) => channel,
        None if allowed.contains(&selected) => {
            // Радио без префикса идёт через `say` — префикс добавляется.
            if selected == SelectChannel::Radio {
                text = format!("{RADIO_COMMON_PREFIX}{text}");
            }
            selected
        }
        None => return Ok(()),
    };
    if let Some(command) = command_for(&text, channel, is_ghost(world)) {
        world.execute_remote_command(&command);
    }
    Ok(())
}

/// Следующий доступный канал (`CycleChatChannel`).
pub fn cycle_channel(world: &mut GameWorld, forward: bool) {
    let allowed = can_send_channels(world);
    let mut input = world.get_resource_or_insert_with(ChatInput::default);
    let order: Vec<SelectChannel> = SelectChannel::ORDER
        .into_iter()
        .filter(|c| allowed.contains(c))
        .collect();
    if order.is_empty() {
        return;
    }
    let index = order.iter().position(|c| *c == input.selected);
    let next = match (index, forward) {
        (None, _) => 0,
        (Some(i), true) => (i + 1) % order.len(),
        (Some(i), false) => (i + order.len() - 1) % order.len(),
    };
    input.selected = order[next];
    input.version += 1;
}

/// Предупреждение в окно чата (`ChatBox.AddLine`).
pub fn add_local_line(world: &mut GameWorld, text: &str, color: &str) {
    let mut chat = world.get_resource_or_insert_with(ClientChat::default);
    chat.messages.push(ChatMessage {
        channel: ChatChannel::NONE,
        message: text.to_owned(),
        wrapped_message: astrocore_loc::escape_text(text),
        sender: None,
        sender_key: None,
        hide_chat: false,
        color_override: Some(color.to_owned()),
        audio_path: None,
        audio_volume: 0.0,
    });
    chat.version += 1;
}

// ---------------------------------------------------------------- разметка

/// Кусок текста с цветом.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub color: GameColor,
    pub bold: bool,
    pub italic: bool,
}

/// Разбор разметки SS14 (`FormattedMessage.FromMarkup`): цвет, жирный, курсив; остальные
/// теги (`font`, `BubbleHeader`, `Name`, ссылки) только снимаются. `\[`, `\]` — скобки.
pub fn markup_spans(text: &str, base: GameColor) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut colors = vec![base];
    let (mut bold, mut italic) = (0u32, 0u32);
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    let flush = |spans: &mut Vec<Span>, current: &mut String, color: GameColor, b: u32, i: u32| {
        if current.is_empty() {
            return;
        }
        let text = std::mem::take(current);
        match spans.last_mut() {
            Some(last) if last.color == color && last.bold == (b > 0) && last.italic == (i > 0) => {
                last.text.push_str(&text);
            }
            _ => spans.push(Span {
                text,
                color,
                bold: b > 0,
                italic: i > 0,
            }),
        }
    };
    while let Some(c) = chars.next() {
        if c == '\\' && matches!(chars.peek(), Some('[' | ']' | '\\')) {
            current.push(chars.next().unwrap_or('\\'));
            continue;
        }
        if c != '[' {
            current.push(c);
            continue;
        }
        let mut tag = String::new();
        let mut closed = false;
        for t in chars.by_ref() {
            if t == ']' {
                closed = true;
                break;
            }
            tag.push(t);
        }
        if !closed {
            current.push('[');
            current.push_str(&tag);
            continue;
        }
        let color = *colors.last().unwrap_or(&base);
        flush(&mut spans, &mut current, color, bold, italic);
        let (closing, body) = match tag.strip_prefix('/') {
            Some(body) => (true, body),
            None => (false, tag.as_str()),
        };
        let name = body
            .split(|c: char| c == '=' || c.is_whitespace())
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match (name.as_str(), closing) {
            ("color", false) => {
                let value = body
                    .split_once('=')
                    .map(|(_, v)| v.trim().trim_matches('"'));
                let parsed = value.and_then(GameColor::parse).unwrap_or(color);
                colors.push(parsed);
            }
            ("color", true) => {
                if colors.len() > 1 {
                    colors.pop();
                }
            }
            ("bold", false) => bold += 1,
            ("bold", true) => bold = bold.saturating_sub(1),
            ("italic", false) => italic += 1,
            ("italic", true) => italic = italic.saturating_sub(1),
            _ => {}
        }
    }
    let color = *colors.last().unwrap_or(&base);
    flush(&mut spans, &mut current, color, bold, italic);
    spans
}

/// Текст между `[tag]` и `[/tag]` (`GetStringInsideTag`).
pub fn string_inside_tag(wrapped: &str, tag: &str) -> String {
    let open = format!("[{tag}]");
    let close = format!("[/{tag}]");
    match (wrapped.find(&open), wrapped.find(&close)) {
        (Some(start), Some(end)) if start + open.len() <= end => {
            wrapped[start + open.len()..end].to_owned()
        }
        _ => String::new(),
    }
}

/// Цвет строки: свой цвет сообщения или цвет канала.
pub fn line_color(message: &ChatMessage) -> GameColor {
    message
        .color_override
        .as_deref()
        .and_then(GameColor::parse)
        .unwrap_or_else(|| channel_color(message.channel))
}

// ---------------------------------------------------------------- облачка

/// Вид облачка (`SpeechBubble.SpeechType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechType {
    Emote,
    Say,
    Whisper,
    Looc,
}

/// Облачков над одной сущностью (`SpeechBubbleCap`).
pub const SPEECH_BUBBLE_CAP: usize = 4;
/// Время жизни облачка, секунды (`TotalTime`).
pub const BUBBLE_LIFETIME: f32 = 4.0;
/// Затухание, секунды (`FadeTime`).
pub const BUBBLE_FADE: f32 = 0.25;
const BUBBLE_DELAY_BASE: f32 = 0.2;
const BUBBLE_DELAY_FACTOR: f32 = 0.8 / 100.0;

/// Облачко над сущностью.
#[derive(Debug, Clone, PartialEq)]
pub struct Bubble {
    pub id: u64,
    pub entity: GameEntity,
    pub kind: SpeechType,
    /// Заголовок (имя) и текст в разметке.
    pub header: String,
    pub content: String,
    /// Сколько осталось жить, секунды.
    pub left: f32,
}

impl Bubble {
    /// Прозрачность: облачко тает в последние [`BUBBLE_FADE`] секунды.
    pub fn alpha(&self) -> f32 {
        (self.left / BUBBLE_FADE).clamp(0.0, 1.0)
    }
}

/// Облачки речи клиента: очередь по сущностям и показанные. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Default)]
pub struct SpeechBubbles {
    queued: HashMap<GameEntity, (f32, VecDeque<(ChatMessage, SpeechType)>)>,
    /// Показанные облачка, старые первыми.
    pub active: Vec<Bubble>,
    next_id: u64,
}

impl SpeechBubbles {
    /// В очередь (`EnqueueSpeechBubble`).
    pub fn enqueue(&mut self, entity: GameEntity, message: ChatMessage, kind: SpeechType) {
        self.queued
            .entry(entity)
            .or_insert((0.0, VecDeque::new()))
            .1
            .push_back((message, kind));
    }

    /// Время идёт (`UpdateQueuedSpeechBubbles`); `exists` — сущность ещё есть.
    pub fn update(&mut self, dt: f32, exists: impl Fn(GameEntity) -> bool) {
        let mut created = Vec::new();
        self.queued.retain(|entity, (left, queue)| {
            if !exists(*entity) {
                return false;
            }
            *left -= dt;
            if *left > 0.0 {
                return true;
            }
            let Some((message, kind)) = queue.pop_front() else {
                return false;
            };
            *left +=
                BUBBLE_DELAY_BASE + message.message.chars().count() as f32 * BUBBLE_DELAY_FACTOR;
            created.push((*entity, message, kind));
            true
        });
        for (entity, message, kind) in created {
            self.create(entity, &message, kind);
        }
        for bubble in &mut self.active {
            bubble.left -= dt;
        }
        self.active
            .retain(|bubble| bubble.left > 0.0 && exists(bubble.entity));
    }

    /// Новое облачко (`CreateSpeechBubble`): лишние над сущностью начинают таять.
    fn create(&mut self, entity: GameEntity, message: &ChatMessage, kind: SpeechType) {
        let (header, content) = match kind {
            SpeechType::Say | SpeechType::Whisper => (
                string_inside_tag(&message.wrapped_message, "BubbleHeader"),
                string_inside_tag(&message.wrapped_message, "BubbleContent"),
            ),
            SpeechType::Emote | SpeechType::Looc => {
                (String::new(), message.wrapped_message.clone())
            }
        };
        self.next_id += 1;
        self.active.push(Bubble {
            id: self.next_id,
            entity,
            kind,
            header,
            content,
            left: BUBBLE_LIFETIME,
        });
        let mine: Vec<usize> = self
            .active
            .iter()
            .enumerate()
            .filter(|(_, b)| b.entity == entity)
            .map(|(i, _)| i)
            .collect();
        if mine.len() > SPEECH_BUBBLE_CAP {
            let index = mine[mine.len() - SPEECH_BUBBLE_CAP - 1];
            let bubble = &mut self.active[index];
            bubble.left = bubble.left.min(BUBBLE_FADE);
        }
    }
}

/// Пришло сообщение (`ProcessChatMessage`): у речи рядом — облачко над говорящим.
pub fn on_message(world: &mut GameWorld, message: &ChatMessage) {
    let Some(sender) = message.sender else {
        return;
    };
    let kind = match message.channel {
        ChatChannel::LOCAL => SpeechType::Say,
        ChatChannel::WHISPER => SpeechType::Whisper,
        ChatChannel::EMOTES => SpeechType::Emote,
        ChatChannel::DEAD if is_ghost(world) => SpeechType::Say,
        _ => return,
    };
    let Some(entity) = world
        .get_resource::<NetEntities>()
        .and_then(|n| n.entity(sender))
    else {
        return;
    };
    world
        .get_resource_or_insert_with(SpeechBubbles::default)
        .enqueue(entity, message.clone(), kind);
}

// ---------------------------------------------------------------- окно

const PANEL: Color = Color::srgba(0.05, 0.06, 0.08, 0.78);
const INPUT: Color = Color::srgba(0.10, 0.11, 0.15, 0.95);
/// Сколько последних сообщений строится в окне.
const SHOWN_MESSAGES: usize = 60;

fn bevy_color(color: GameColor) -> Color {
    Color::srgba(color.r, color.g, color.b, color.a)
}

#[derive(Component)]
struct ChatPanel;

#[derive(Component)]
struct BubbleView(u64);

fn spans_text(commands: &mut Commands, style: &UiStyle, parent: Entity, spans: &[Span], size: f32) {
    let text = commands
        .spawn((
            ChildOf(parent),
            Text::default(),
            TextFont {
                font: style.font.clone().into(),
                font_size: size.into(),
                ..default()
            },
        ))
        .id();
    for span in spans {
        // Без жирного шрифта жирный текст чуть светлее.
        let color = if span.bold {
            span.color.with_alpha(1.0)
        } else {
            span.color
        };
        commands.spawn((
            ChildOf(text),
            TextSpan::new(span.text.clone()),
            TextFont {
                font: style.font.clone().into(),
                font_size: size.into(),
                ..default()
            },
            TextColor(bevy_color(color)),
        ));
    }
}

/// Клавиши чата: открытие по каналам, набор, отправка, перебор каналов, листание.
fn type_into(
    mut events: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut focus: ResMut<KeyboardFocus>,
    mut game: ResMut<Game>,
) {
    let world = &mut game.0;
    if world.get_resource::<ClientChat>().is_none() {
        return;
    }
    let was_focused = world.get_resource::<ChatInput>().is_some_and(|i| i.focused);
    // Другая строка ввода (консоль, поиск) держит клавиатуру.
    if focus.0 && !was_focused {
        events.clear();
        return;
    }
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let control = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    for event in events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }
        let focused = world.get_resource::<ChatInput>().is_some_and(|i| i.focused);
        if !focused {
            // Клавиши каналов (`FocusLocalChatWindow` и соседи).
            let channel = match (event.key_code, shift) {
                (KeyCode::KeyT, false) => Some(None),
                (KeyCode::Period, false) => Some(Some(SelectChannel::Local)),
                (KeyCode::Comma, false) => Some(Some(SelectChannel::Whisper)),
                (KeyCode::Semicolon, false) => Some(Some(SelectChannel::Radio)),
                (KeyCode::Quote, true) => Some(Some(SelectChannel::Emotes)),
                (KeyCode::Digit9, true) => Some(Some(SelectChannel::Looc)),
                (KeyCode::BracketLeft, false) => Some(Some(SelectChannel::Ooc)),
                (KeyCode::BracketRight, false) => Some(Some(SelectChannel::Admin)),
                (KeyCode::Backslash, false) => Some(Some(SelectChannel::Dead)),
                (KeyCode::Slash, false) => Some(Some(SelectChannel::Console)),
                _ => None,
            };
            let Some(channel) = channel else {
                continue;
            };
            let allowed = can_send_channels(world);
            let mut input = world.get_resource_or_insert_with(ChatInput::default);
            if let Some(channel) = channel.filter(|c| allowed.contains(c)) {
                input.selected = channel;
            }
            input.focused = true;
            input.version += 1;
            continue;
        }
        match &event.logical_key {
            Key::Escape => {
                let mut input = world.resource_mut::<ChatInput>();
                input.focused = false;
                input.version += 1;
            }
            Key::Enter => {
                if let Err(warning) = submit(world) {
                    add_local_line(world, &warning, "orange");
                }
            }
            Key::Tab => cycle_channel(world, !control),
            Key::Backspace => {
                let mut input = world.resource_mut::<ChatInput>();
                input.text.pop();
                input.version += 1;
            }
            Key::PageUp | Key::PageDown => {
                let mut input = world.resource_mut::<ChatInput>();
                input.scroll = if matches!(event.logical_key, Key::PageUp) {
                    input.scroll + 5
                } else {
                    input.scroll.saturating_sub(5)
                };
                input.version += 1;
            }
            Key::Space => {
                let mut input = world.resource_mut::<ChatInput>();
                input.text.push(' ');
                input.version += 1;
            }
            Key::Character(text) => {
                let mut input = world.resource_mut::<ChatInput>();
                input.text.extend(text.chars().filter(|c| !c.is_control()));
                input.version += 1;
            }
            _ => {}
        }
    }
    let focused = world.get_resource::<ChatInput>().is_some_and(|i| i.focused);
    if focused != was_focused {
        focus.0 = focused;
    }
}

/// Перерисовывает окно, когда меняются сообщения или строка ввода.
fn draw(
    mut commands: Commands,
    mut game: ResMut<Game>,
    style: Res<UiStyle>,
    panels: Query<Entity, With<ChatPanel>>,
    mut shown: Local<Option<(u64, u64, Vec<SelectChannel>)>>,
) {
    let world = &mut game.0;
    let Some(chat_version) = world.get_resource::<ClientChat>().map(|c| c.version) else {
        return;
    };
    let input = world
        .get_resource_or_insert_with(ChatInput::default)
        .clone();
    let allowed = can_send_channels(world);
    let now = (chat_version, input.version, allowed.clone());
    if shown.as_ref() == Some(&now) {
        return;
    }
    *shown = Some(now);
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                // Справа сверху, как в раскладке SS14 «Separated».
                right: Val::Px(12.0),
                top: Val::Px(12.0),
                width: Val::Px(440.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                padding: UiRect::all(Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            GlobalZIndex(60),
            ChatPanel,
        ))
        .id();
    let history = commands
        .spawn((
            ChildOf(panel),
            Node {
                height: Val::Px(240.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::FlexEnd,
                overflow: Overflow::clip(),
                row_gap: Val::Px(2.0),
                ..default()
            },
        ))
        .id();
    let messages: Vec<ChatMessage> = world
        .resource::<ClientChat>()
        .messages
        .iter()
        .filter(|m| !m.hide_chat)
        .cloned()
        .collect();
    let end = messages.len().saturating_sub(input.scroll);
    let start = end.saturating_sub(SHOWN_MESSAGES);
    for message in &messages[start..end] {
        let spans = markup_spans(&message.wrapped_message, line_color(message));
        spans_text(&mut commands, &style, history, &spans, 14.0);
    }
    // Строка ввода с кнопкой канала (`ChannelSelectorButton`).
    let channel = if input.text.trim().is_empty() {
        input.selected
    } else {
        split_input(world, &input.text, &allowed)
            .0
            .unwrap_or(input.selected)
    };
    let row = commands
        .spawn((
            ChildOf(panel),
            Node {
                column_gap: Val::Px(6.0),
                padding: UiRect::all(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(INPUT),
        ))
        .id();
    let name = loc(world, channel.loc_key(), &[]);
    spans_text(
        &mut commands,
        &style,
        row,
        &[Span {
            text: name,
            color: channel.color(),
            bold: false,
            italic: false,
        }],
        14.0,
    );
    let shown_text = if input.focused {
        format!("{}_", input.text)
    } else if input.text.is_empty() {
        loc(
            world,
            "hud-chatbox-info-talk",
            &[("talk-key", LocArg::from("T"))],
        )
    } else {
        input.text.clone()
    };
    let color = if input.focused || !input.text.is_empty() {
        GameColor::WHITE
    } else {
        GameColor::parse("gray").unwrap_or(GameColor::WHITE)
    };
    spans_text(
        &mut commands,
        &style,
        row,
        &[Span {
            text: shown_text,
            color,
            bold: false,
            italic: false,
        }],
        14.0,
    );
}

/// Облачки речи: время идёт, облачка ставятся над говорящими стопкой.
fn bubbles(
    mut commands: Commands,
    mut game: ResMut<Game>,
    time: Res<Time>,
    style: Res<UiStyle>,
    camera: Single<(&Camera, &GlobalTransform), With<astrocore_client::WorldCamera>>,
    mut views: Query<(Entity, &BubbleView, &mut Node, &ComputedNode)>,
) {
    let world = &mut game.0;
    let exists_world: &GameWorld = world;
    let alive: std::collections::HashSet<GameEntity> = {
        let bubbles = exists_world.get_resource::<SpeechBubbles>();
        let mut set = std::collections::HashSet::new();
        if let Some(bubbles) = bubbles {
            for entity in bubbles
                .queued
                .keys()
                .chain(bubbles.active.iter().map(|b| &b.entity))
            {
                if exists_world.get_entity(*entity).is_ok() {
                    set.insert(*entity);
                }
            }
        }
        set
    };
    let dt = time.delta_secs();
    let Some(mut state) = world.get_resource_mut::<SpeechBubbles>() else {
        return;
    };
    state.update(dt, |e| alive.contains(&e));
    let active = state.active.clone();
    let (camera, transform) = *camera;
    let mut existing: HashMap<u64, Entity> = HashMap::new();
    // Высота уже поставленных облачков над каждой сущностью (новые снизу).
    let mut stacked: HashMap<GameEntity, f32> = HashMap::new();
    for (view, bubble_view, _, _) in &views {
        if active.iter().any(|b| b.id == bubble_view.0) {
            existing.insert(bubble_view.0, view);
        } else {
            commands.entity(view).despawn();
        }
    }
    for bubble in active.iter().rev() {
        let Some(at) = world.world_transform(bubble.entity).map(|t| t.position) else {
            continue;
        };
        let Ok(screen) = camera.world_to_viewport(transform, Vec3::new(at.x, at.y + 0.5, 0.0))
        else {
            continue;
        };
        let offset = stacked.entry(bubble.entity).or_insert(0.0);
        if let Some(view) = existing.get(&bubble.id) {
            if let Ok((_, _, mut node, computed)) = views.get_mut(*view) {
                let size = computed.size() * computed.inverse_scale_factor();
                node.left = Val::Px(screen.x - size.x / 2.0);
                node.top = Val::Px(screen.y - size.y - *offset);
                *offset += size.y + 2.0;
            }
            continue;
        }
        let alpha = bubble.alpha();
        let background = match bubble.kind {
            SpeechType::Whisper => Color::srgba(0.12, 0.12, 0.14, 0.8 * alpha),
            SpeechType::Emote | SpeechType::Looc => Color::srgba(0.0, 0.0, 0.0, 0.0),
            SpeechType::Say => Color::srgba(0.07, 0.08, 0.10, 0.85 * alpha),
        };
        let view = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(screen.x),
                    top: Val::Px(screen.y - *offset),
                    max_width: Val::Px(256.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                    ..default()
                },
                BackgroundColor(background),
                GlobalZIndex(450),
                BubbleView(bubble.id),
            ))
            .id();
        let base = match bubble.kind {
            SpeechType::Looc => GameColor::from_hex("#48d1cc").unwrap_or(GameColor::WHITE),
            _ => GameColor::WHITE,
        };
        if !bubble.header.is_empty() {
            let header = markup_spans(&bubble.header, base.with_alpha(0.8));
            spans_text(&mut commands, &style, view, &header, 11.0);
        }
        let content = markup_spans(&bubble.content, base);
        spans_text(&mut commands, &style, view, &content, 13.0);
        *offset += 20.0;
    }
}

/// Окно чата и облачка речи.
pub struct ChatUiPlugin;

impl Plugin for ChatUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, type_into.after(bevy::input::InputSystems));
        app.add_systems(Update, (draw, bubbles));
    }
}

#[cfg(test)]
mod tests;
