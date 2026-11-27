//! Радио: каналы, ключи шифрования, гарнитуры, встроенные приёмники и передатчики (боргов и
//! ИИ), ручные рации, динамики и интеркомы, серверы связи; слушающие устройства рядом с
//! говорящим. Образец — `RadioChannelPrototype`, `RadioSystem`, `SharedRadioSystem`,
//! `HeadsetSystem`, `SharedHeadsetSystem`, `EncryptionKeySystem`, `SharedRadioDeviceSystem`,
//! компоненты `Content.Shared/Radio/Components`, события `RadioEvent`,
//! `EncryptionChannelsChangedEvent`, `GetDefaultRadioChannelEvent`, а также `ListeningSystem`,
//! `ActiveListenerComponent` и `ListenEvent` (`Content.Shared/Speech`) SS14 @ 2f4a2aad.
//!
//! Нет пока: окна интеркома (выбор канала и переключатели), глушилок, ЭМИ, оповещения о
//! неработающих устройствах (`NotifyOnNonFunctioning`).

use std::collections::{BTreeSet, HashSet};

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{ComponentInit, EventWorldExt, GameWorldExt, RegistryWorldExt, Update};
use astrocore_loc::{EntityNameWorldExt, LocArg, escape_text};
use astrocore_map::{
    ContainerKind, ContainerWorldExt, EntInsertedIntoContainer, EntRemovedFromContainer,
    TransformWorldExt,
};
use astrocore_net::{Actor, AppearanceWorldExt};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::chat::{
    COMMON_CHANNEL, ChatChannel, ChatMessage, ChatTransmitRange, DEFAULT_CHANNEL_KEY, EntitySpoke,
    GetDefaultRadioChannel, InGameIcChatType, RADIO_CHANNEL_PREFIX, RADIO_COMMON_PREFIX,
    VOICE_RANGE, WHISPER_CLEAR_RANGE,
};
use crate::do_after::DoAfterDone;
use crate::examine::{Examined, loc};
use crate::hands::HandsWorldExt;
use crate::interaction::{ActivateInWorld, InteractUsing, InteractionWorldExt};
use crate::inventory::{Equipped, Inventory, InventoryWorldExt, SlotFlags, Unequipped};
use crate::popups::{PopupType, PopupWorldExt};
use crate::power::{PowerChanged, is_powered};
use crate::tools::{ToolWorldExt, register_tool_event};

/// Контейнер ключей шифрования (`EncryptionKeyHolderComponent.KeyContainerName`).
pub const KEY_CONTAINER: &str = "key_slots";

/// Цвет канала по умолчанию (`Color.Lime`).
const DEFAULT_COLOR: &str = "#00FF00";

// Прототип.

/// Радиоканал (`RadioChannelPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct RadioChannel {
    pub id: String,
    /// Ключ перевода названия.
    pub name: String,
    pub key_code: Option<char>,
    /// Цвет строкой `#rrggbb`.
    pub color: String,
    /// Частота как в данных (`145.9`).
    pub frequency: String,
    /// Работает между станциями без сервера связи.
    pub long_range: bool,
}

/// Канал по id.
pub fn radio_channel(prototypes: &Prototypes, id: &str) -> Option<RadioChannel> {
    let node = prototypes.resolved_node("radioChannel", id)?;
    let text = |key: &str| node.get(key).and_then(|n| n.as_str()).map(str::to_owned);
    Some(RadioChannel {
        id: id.to_owned(),
        name: text("name").unwrap_or_default(),
        key_code: text("keycode").and_then(|k| k.chars().next()),
        color: text("color").unwrap_or_else(|| DEFAULT_COLOR.into()),
        frequency: text("frequency").unwrap_or_else(|| "1".into()),
        long_range: text("longRange").is_some_and(|t| t == "true"),
    })
}

/// Переведённое название канала (`LocalizedName`).
pub fn channel_name(world: &World, channel: &RadioChannel) -> String {
    loc(world, &channel.name, &[])
}

// Компоненты.

/// Ключ шифрования (`EncryptionKeyComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EncryptionKey {
    pub channels: BTreeSet<String>,
    pub default_channel: Option<String>,
}

/// Держатель ключей: гарнитура, интерком, сервер связи (`EncryptionKeyHolderComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EncryptionKeyHolder {
    pub keys_unlocked: bool,
    pub keys_extraction_method: String,
    pub key_slots: usize,
    pub key_extraction_sound: Option<SoundSpecifier>,
    pub key_insertion_sound: Option<SoundSpecifier>,
    /// Каналы всех ключей.
    #[serde(skip)]
    pub channels: BTreeSet<String>,
    /// Канал префикса `:h`.
    #[serde(skip)]
    pub default_channel: Option<String>,
}

impl Default for EncryptionKeyHolder {
    fn default() -> Self {
        Self {
            keys_unlocked: true,
            keys_extraction_method: "Screwing".into(),
            key_slots: 2,
            key_extraction_sound: None,
            key_insertion_sound: None,
            channels: BTreeSet::new(),
            default_channel: None,
        }
    }
}

/// Гарнитура: передаёт сообщения радио носящему (`HeadsetComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Headset {
    pub enabled: bool,
    pub is_equipped: bool,
    pub required_slot: SlotFlags,
}

impl Default for Headset {
    fn default() -> Self {
        Self {
            enabled: true,
            is_equipped: false,
            required_slot: SlotFlags(1 << 3),
        }
    }
}

/// Носит гарнитуру (`WearingHeadsetComponent`).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct WearingHeadset {
    pub headset: Entity,
}

/// Принимает сообщения радио по каналам (`ActiveRadioComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ActiveRadio {
    pub channels: BTreeSet<String>,
    pub receive_all_channels: bool,
    /// Слышит все карты.
    pub global_receive: bool,
}

/// Сообщения радио сразу в чат владельца (`IntrinsicRadioReceiverComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IntrinsicRadioReceiver {}

/// Речь сразу в радио без гарнитуры (`IntrinsicRadioTransmitterComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IntrinsicRadioTransmitter {
    pub channels: BTreeSet<String>,
}

impl Default for IntrinsicRadioTransmitter {
    fn default() -> Self {
        Self {
            channels: [COMMON_CHANNEL.to_owned()].into(),
        }
    }
}

/// Сервер связи: без него на карте гарнитуры не работают (`TelecomServerComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TelecomServer {}

/// Передатчику сервер связи не нужен: рации, интеркомы (`TelecomExemptComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TelecomExempt {}

/// Микрофон рации или интеркома (`RadioMicrophoneComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RadioMicrophone {
    pub broadcast_channel: String,
    pub listen_range: f32,
    pub enabled: bool,
    pub power_required: bool,
    pub toggle_on_interact: bool,
    pub unobstructed_required: bool,
}

impl Default for RadioMicrophone {
    fn default() -> Self {
        Self {
            broadcast_channel: COMMON_CHANNEL.into(),
            listen_range: 4.0,
            enabled: false,
            power_required: false,
            toggle_on_interact: true,
            unobstructed_required: false,
        }
    }
}

/// Динамик рации или интеркома (`RadioSpeakerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RadioSpeaker {
    pub toggle_on_interact: bool,
    pub channels: BTreeSet<String>,
    pub enabled: bool,
}

impl Default for RadioSpeaker {
    fn default() -> Self {
        Self {
            toggle_on_interact: true,
            channels: [COMMON_CHANNEL.to_owned()].into(),
            enabled: false,
        }
    }
}

/// Интерком (`IntercomComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Intercom {
    pub requires_power: bool,
    pub speaker_enabled: bool,
    pub microphone_enabled: bool,
    pub current_channel: Option<String>,
    pub supported_channels: Vec<String>,
}

impl Default for Intercom {
    fn default() -> Self {
        Self {
            requires_power: true,
            speaker_enabled: false,
            microphone_enabled: false,
            current_channel: None,
            supported_channels: Vec::new(),
        }
    }
}

/// Слышит речь рядом (`ActiveListenerComponent`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActiveListener {
    pub range: f32,
}

impl Default for ActiveListener {
    fn default() -> Self {
        Self { range: VOICE_RANGE }
    }
}

// События.

/// Сообщение радио принято (`RadioReceiveEvent`). Поднимается на приёмнике.
#[derive(Debug, Clone)]
pub struct RadioReceive {
    pub message: String,
    pub message_source: Entity,
    pub channel: String,
    pub radio_source: Entity,
    pub chat: ChatMessage,
}

/// Гарнитура приняла сообщение: поднимается на носящем (`HeadsetRadioReceiveRelayEvent`).
#[derive(Debug, Clone)]
pub struct HeadsetRadioReceiveRelay {
    pub relayed: RadioReceive,
}

/// Можно ли принять (`RadioReceiveAttemptEvent`): на приёмнике и широковещательно.
#[derive(Debug, Clone)]
pub struct RadioReceiveAttempt {
    pub channel: String,
    pub radio_source: Entity,
    pub radio_receiver: Entity,
    pub cancelled: bool,
}

/// Можно ли передать (`RadioSendAttemptEvent`): широковещательно и на передатчике.
#[derive(Debug, Clone)]
pub struct RadioSendAttempt {
    pub channel: String,
    pub radio_source: Entity,
    pub cancelled: bool,
}

/// Каналы держателя ключей изменились (`EncryptionChannelsChangedEvent`).
#[derive(Debug, Clone)]
pub struct EncryptionChannelsChanged {
    pub channels: BTreeSet<String>,
    pub default_channel: Option<String>,
}

/// Ключи вынуты отвёрткой (`EncryptionRemovalFinishedEvent`).
#[derive(Debug, Clone, PartialEq)]
pub struct EncryptionRemovalFinished;

/// Слушающее устройство услышало речь (`ListenEvent`).
#[derive(Debug, Clone)]
pub struct Listen {
    pub message: String,
    pub source: Entity,
}

/// Может ли устройство слышать (`ListenAttemptEvent`).
#[derive(Debug, Clone)]
pub struct ListenAttempt {
    pub source: Entity,
    pub cancelled: bool,
}

/// Сообщения в пути: защита от петли «динамик — микрофон».
#[derive(Resource, Debug, Default)]
struct RadioInFlight(HashSet<String>);

/// Недавно переданное микрофонами за такт: одна фраза не уходит дважды через несколько раций.
#[derive(Resource, Debug, Default)]
struct RecentlySent(HashSet<(String, Entity, String)>);

// Передача.

/// Сообщение в радиоканал всем приёмникам (`RadioSystem.SendRadioMessage`).
pub fn send_radio_message(
    world: &mut World,
    message_source: Entity,
    message: &str,
    channel_id: &str,
    radio_source: Entity,
    escape_markup: bool,
) {
    let Some(channel) = world
        .get_resource::<Prototypes>()
        .and_then(|p| radio_channel(p, channel_id))
    else {
        return;
    };
    if !world
        .get_resource_or_insert_with(RadioInFlight::default)
        .0
        .insert(message.to_owned())
    {
        return;
    }
    let speaker = crate::chat::speaker_name(world, message_source);
    let name = escape_text(&speaker.voice_name);
    let verb = speaker
        .speech_verb
        .as_deref()
        .and_then(|id| crate::chat::speech_verb(world.get_resource::<Prototypes>()?, id))
        .or_else(|| crate::chat::get_speech_verb(world, message_source, message));
    let (font, size, bold, verbs) = match &verb {
        Some(v) => (
            v.font_id.clone(),
            v.font_size,
            v.bold,
            v.speech_verb_strings.clone(),
        ),
        None => ("Default".into(), 12, false, Vec::new()),
    };
    let verb_text = crate::chat::pick(world, &verbs)
        .map(|key| loc(world, key, &[]))
        .unwrap_or_default();
    let content = if escape_markup {
        escape_text(message)
    } else {
        message.to_owned()
    };
    let localized = channel_name(world, &channel);
    let wrapped = loc(
        world,
        if bold {
            "chat-radio-message-wrap-bold"
        } else {
            "chat-radio-message-wrap"
        },
        &[
            ("color", LocArg::from(channel.color.clone())),
            ("fontType", LocArg::from(font)),
            ("fontSize", LocArg::from(size)),
            ("verb", LocArg::from(verb_text)),
            ("channel", LocArg::from(format!("\\[{localized}\\]"))),
            ("name", LocArg::from(name.clone())),
            ("message", LocArg::from(content)),
        ],
    );
    let chat = crate::chat::chat_message(
        world,
        ChatChannel::RADIO,
        message,
        &wrapped,
        None,
        false,
        None,
        None,
    );
    let mut attempt = RadioSendAttempt {
        channel: channel.id.clone(),
        radio_source,
        cancelled: false,
    };
    world.raise_broadcast(&mut attempt);
    world.raise(radio_source, &mut attempt);
    let can_send = !attempt.cancelled;

    let source_map = world.map_coordinates(radio_source).map(|c| c.map);
    let has_server = source_map.is_some_and(|map| has_active_server(world, map, &channel.id));
    let exempt = world.get::<TelecomExempt>(radio_source).is_some();
    let receivers: Vec<(Entity, ActiveRadio)> = world
        .query::<(Entity, &ActiveRadio)>()
        .iter(world)
        .map(|(e, r)| (e, r.clone()))
        .collect();
    for (receiver, radio) in receivers {
        if !can_send {
            break;
        }
        if !radio.receive_all_channels
            && (!radio.channels.contains(&channel.id)
                || world
                    .get::<Intercom>(receiver)
                    .is_some_and(|i| !i.supported_channels.contains(&channel.id)))
        {
            continue;
        }
        let map = world.map_coordinates(receiver).map(|c| c.map);
        if !channel.long_range && map != source_map && !radio.global_receive {
            continue;
        }
        if !channel.long_range && !exempt && !has_server {
            continue;
        }
        let mut attempt = RadioReceiveAttempt {
            channel: channel.id.clone(),
            radio_source,
            radio_receiver: receiver,
            cancelled: false,
        };
        world.raise_broadcast(&mut attempt);
        world.raise(receiver, &mut attempt);
        if attempt.cancelled {
            continue;
        }
        world.raise(
            receiver,
            &mut RadioReceive {
                message: message.to_owned(),
                message_source,
                channel: channel.id.clone(),
                radio_source,
                chat: chat.clone(),
            },
        );
    }
    let real = world.entity_name(message_source);
    let text = if speaker.voice_name != real {
        format!(
            "Radio message from {real} as {} on {localized}: {message}",
            speaker.voice_name
        )
    } else {
        format!("Radio message from {real} on {localized}: {message}")
    };
    add_admin_log(world, LogType::Chat, LogImpact::Low, &text, &[], &[]);
    if let Some(mut in_flight) = world.get_resource_mut::<RadioInFlight>() {
        in_flight.0.remove(message);
    }
}

/// Есть ли на карте включённый сервер связи с ключом канала (`HasActiveServer`).
fn has_active_server(world: &mut World, map: astrocore_map::MapId, channel: &str) -> bool {
    let servers: Vec<Entity> = world
        .query_filtered::<Entity, bevy_ecs::query::With<TelecomServer>>()
        .iter(world)
        .collect();
    servers.into_iter().any(|server| {
        world
            .get::<EncryptionKeyHolder>(server)
            .is_some_and(|keys| keys.channels.contains(channel))
            && world
                .get::<crate::power::ApcPowerReceiver>(server)
                .is_some()
            && is_powered(world, server)
            && world.map_coordinates(server).is_some_and(|c| c.map == map)
    })
}

/// Сообщение в чат игрока, управляющего сущностью.
fn send_to_actor(world: &mut World, entity: Entity, chat: &ChatMessage) {
    if let Some(session) = world.get::<Actor>(entity).map(|a| a.session) {
        crate::chat::deliver(world, chat.clone(), &[session]);
    }
}

// Ключи шифрования.

/// Пересчитать каналы держателя по ключам (`UpdateChannels`).
pub fn update_channels(world: &mut World, holder: Entity) {
    if world.get::<EncryptionKeyHolder>(holder).is_none() {
        return;
    }
    let keys: Vec<Entity> = world
        .container(holder, KEY_CONTAINER)
        .map(|c| c.entities().to_vec())
        .unwrap_or_default();
    let mut channels = BTreeSet::new();
    let mut default_channel = None;
    for key in keys {
        if let Some(key) = world.get::<EncryptionKey>(key) {
            channels.extend(key.channels.iter().cloned());
            if default_channel.is_none() {
                default_channel.clone_from(&key.default_channel);
            }
        }
    }
    if let Some(mut component) = world.get_mut::<EncryptionKeyHolder>(holder) {
        component.channels.clone_from(&channels);
        component.default_channel.clone_from(&default_channel);
    }
    world.raise(
        holder,
        &mut EncryptionChannelsChanged {
            channels,
            default_channel,
        },
    );
}

fn popup(world: &mut World, key: &str, entity: Entity, user: Entity) {
    let text = loc(world, key, &[]);
    world.popup_entity_for(&text, entity, user, PopupType::Small);
}

fn sound_or(sound: Option<SoundSpecifier>, path: &str) -> SoundSpecifier {
    sound.unwrap_or_else(|| SoundSpecifier::path(path))
}

/// Вставить ключ (`TryInsertKey`).
fn try_insert_key(world: &mut World, holder: Entity, user: Entity, key: Entity) {
    let Some(component) = world.get::<EncryptionKeyHolder>(holder).cloned() else {
        return;
    };
    if !component.keys_unlocked {
        popup(world, "encryption-keys-are-locked", holder, user);
        return;
    }
    if world
        .get::<crate::wires::WiresPanel>(holder)
        .is_some_and(|p| !p.open)
    {
        popup(world, "encryption-keys-panel-locked", holder, user);
        return;
    }
    let count = world
        .container(holder, KEY_CONTAINER)
        .map_or(0, |c| c.len());
    if component.key_slots <= count {
        popup(world, "encryption-key-slots-already-full", holder, user);
        return;
    }
    if world
        .insert_into_container(key, holder, KEY_CONTAINER)
        .is_ok()
    {
        popup(world, "encryption-key-successfully-installed", holder, user);
        let sound = sound_or(
            component.key_insertion_sound,
            "/Audio/Items/pistol_magin.ogg",
        );
        world.play_entity(&sound, holder, None);
    }
}

/// Начать вынимать ключи инструментом (`TryRemoveKey`).
fn try_remove_key(world: &mut World, holder: Entity, user: Entity, tool: Entity) {
    let Some(component) = world.get::<EncryptionKeyHolder>(holder).cloned() else {
        return;
    };
    if !component.keys_unlocked {
        popup(world, "encryption-keys-are-locked", holder, user);
        return;
    }
    if !crate::wires::is_panel_open(world, holder, None) {
        popup(world, "encryption-keys-panel-locked", holder, user);
        return;
    }
    if world
        .container(holder, KEY_CONTAINER)
        .is_none_or(|c| c.is_empty())
    {
        popup(world, "encryption-keys-no-keys", holder, user);
        return;
    }
    world.use_tool(
        tool,
        user,
        Some(holder),
        1.0,
        &[component.keys_extraction_method.as_str()],
        EncryptionRemovalFinished,
        0.0,
    );
}

/// Ключи вынуты (`OnKeyRemoval`).
fn on_key_removal(
    world: &mut World,
    holder: Entity,
    event: &mut DoAfterDone<EncryptionRemovalFinished>,
) {
    if event.cancelled {
        return;
    }
    let user = event.user;
    let removed = world.empty_container(holder, KEY_CONTAINER);
    for key in removed {
        world.pickup_or_drop(Some(user), key);
    }
    popup(world, "encryption-keys-all-extracted", holder, user);
    let sound = world
        .get::<EncryptionKeyHolder>(holder)
        .and_then(|h| h.key_extraction_sound.clone());
    let sound = sound_or(sound, "/Audio/Items/pistol_magout.ogg");
    world.play_entity(&sound, holder, None);
    event.handled = true;
}

/// Строки осмотра о каналах (`AddChannelsExamine`).
pub fn channels_examine(
    world: &World,
    channels: &BTreeSet<String>,
    default_channel: Option<&str>,
    examined: Entity,
    pattern: &str,
) -> Vec<String> {
    let Some(prototypes) = world.get_resource::<Prototypes>() else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for id in channels {
        let Some(proto) = radio_channel(prototypes, id) else {
            continue;
        };
        let key = if id == COMMON_CHANNEL {
            RADIO_COMMON_PREFIX.to_string()
        } else {
            format!("{RADIO_CHANNEL_PREFIX}{}", proto.key_code.unwrap_or('\0'))
        };
        lines.push(loc(
            world,
            pattern,
            &[
                ("color", LocArg::from(proto.color.clone())),
                ("key", LocArg::from(key)),
                ("id", LocArg::from(channel_name(world, &proto))),
                ("freq", LocArg::from(proto.frequency.clone())),
            ],
        ));
    }
    if let Some(proto) = default_channel.and_then(|id| radio_channel(prototypes, id)) {
        let name = channel_name(world, &proto);
        if world.get::<Headset>(examined).is_some() {
            lines.push(loc(
                world,
                "examine-headset-default-channel",
                &[
                    (
                        "prefix",
                        LocArg::from(format!("{RADIO_CHANNEL_PREFIX}{DEFAULT_CHANNEL_KEY}")),
                    ),
                    ("channel", LocArg::from(name.clone())),
                    ("color", LocArg::from(proto.color.clone())),
                ],
            ));
        }
        if world.get::<EncryptionKey>(examined).is_some() {
            lines.push(loc(
                world,
                "examine-encryption-default-channel",
                &[
                    ("channel", LocArg::from(name)),
                    ("color", LocArg::from(proto.color.clone())),
                ],
            ));
        }
    }
    lines
}

// Гарнитуры.

/// Каналы приёма гарнитуры по ключам (`UpdateRadioChannels`).
fn update_headset_channels(world: &mut World, headset: Entity) {
    if !world.get::<Headset>(headset).is_some_and(|h| h.enabled) {
        return;
    }
    let Some(channels) = world
        .get::<EncryptionKeyHolder>(headset)
        .map(|k| k.channels.clone())
    else {
        return;
    };
    if channels.is_empty() {
        world.entity_mut(headset).remove::<ActiveRadio>();
    } else {
        let mut radio = world
            .get::<ActiveRadio>(headset)
            .cloned()
            .unwrap_or_default();
        radio.channels = channels;
        world.entity_mut(headset).insert(radio);
    }
}

/// Включить или выключить гарнитуру (`SetEnabled`).
pub fn set_headset_enabled(world: &mut World, headset: Entity, value: bool) {
    let Some(mut component) = world.get_mut::<Headset>(headset) else {
        return;
    };
    if component.enabled == value {
        return;
    }
    component.enabled = value;
    let equipped = component.is_equipped;
    let wearer = world.parent(headset);
    if !value {
        world.entity_mut(headset).remove::<ActiveRadio>();
        if equipped && let Some(wearer) = wearer {
            world.entity_mut(wearer).remove::<WearingHeadset>();
        }
    } else if equipped && let Some(wearer) = wearer {
        world.entity_mut(wearer).insert(WearingHeadset { headset });
        update_headset_channels(world, headset);
    }
}

/// Канал по умолчанию от гарнитуры в слотах кроме карманов (`OnGetDefault` с ретрансляцией).
fn default_channel_from_inventory(
    world: &mut World,
    wearer: Entity,
    event: &mut GetDefaultRadioChannel,
) {
    let Some(slots) = world.get::<Inventory>(wearer).map(|i| i.slots.clone()) else {
        return;
    };
    for slot in slots {
        if slot.slot_flags.intersects(SlotFlags::POCKET) {
            continue;
        }
        let Some(item) = world.slot_entity(wearer, &slot.name) else {
            continue;
        };
        if !world
            .get::<Headset>(item)
            .is_some_and(|h| h.enabled && h.is_equipped)
        {
            continue;
        }
        if event.channel.is_none() {
            event.channel = world
                .get::<EncryptionKeyHolder>(item)
                .and_then(|k| k.default_channel.clone());
        }
    }
}

// Рации, динамики, интеркомы.

fn radio_state_popup(world: &mut World, user: Entity, enabled: bool) {
    let state = loc(
        world,
        if enabled {
            "handheld-radio-component-on-state"
        } else {
            "handheld-radio-component-off-state"
        },
        &[],
    );
    let text = loc(
        world,
        "handheld-radio-component-on-use",
        &[("radioState", LocArg::from(state))],
    );
    world.popup_entity_for(&text, user, user, PopupType::Small);
}

/// Включить микрофон (`SetMicrophoneEnabled`).
pub fn set_microphone_enabled(
    world: &mut World,
    entity: Entity,
    user: Option<Entity>,
    enabled: bool,
    quiet: bool,
) {
    let Some(mic) = world.get::<RadioMicrophone>(entity).cloned() else {
        return;
    };
    if mic.power_required && !is_powered(world, entity) {
        return;
    }
    if let Some(mut mic) = world.get_mut::<RadioMicrophone>(entity) {
        mic.enabled = enabled;
    }
    if !quiet && let Some(user) = user {
        radio_state_popup(world, user, enabled);
    }
    world.set_appearance(entity, "enum.RadioDeviceVisuals.Broadcasting", enabled);
    if enabled {
        world.entity_mut(entity).insert(ActiveListener {
            range: mic.listen_range,
        });
    } else {
        world.entity_mut(entity).remove::<ActiveListener>();
    }
}

/// Включить динамик (`SetSpeakerEnabled`).
pub fn set_speaker_enabled(
    world: &mut World,
    entity: Entity,
    user: Option<Entity>,
    enabled: bool,
    quiet: bool,
) {
    let Some(mut speaker) = world.get_mut::<RadioSpeaker>(entity) else {
        return;
    };
    speaker.enabled = enabled;
    let channels = speaker.channels.clone();
    if !quiet && let Some(user) = user {
        radio_state_popup(world, user, enabled);
    }
    world.set_appearance(entity, "enum.RadioDeviceVisuals.Speaker", enabled);
    if enabled {
        let mut radio = world
            .get::<ActiveRadio>(entity)
            .cloned()
            .unwrap_or_default();
        radio.channels.extend(channels);
        world.entity_mut(entity).insert(radio);
    } else {
        world.entity_mut(entity).remove::<ActiveRadio>();
    }
}

/// Канал интеркома (`SetIntercomChannel`).
pub fn set_intercom_channel(world: &mut World, entity: Entity, channel: Option<String>) {
    let Some(mut intercom) = world.get_mut::<Intercom>(entity) else {
        return;
    };
    intercom.current_channel.clone_from(&channel);
    if world.get::<RadioMicrophone>(entity).is_some() {
        match &channel {
            None => {
                set_microphone_enabled(world, entity, None, false, false);
                if let Some(mut intercom) = world.get_mut::<Intercom>(entity) {
                    intercom.microphone_enabled = false;
                }
            }
            Some(id) => {
                if let Some(mut mic) = world.get_mut::<RadioMicrophone>(entity) {
                    mic.broadcast_channel.clone_from(id);
                }
            }
        }
    }
    if world.get::<RadioSpeaker>(entity).is_some() {
        match &channel {
            None => {
                set_speaker_enabled(world, entity, None, false, false);
                if let Some(mut intercom) = world.get_mut::<Intercom>(entity) {
                    intercom.speaker_enabled = false;
                }
            }
            Some(id) => {
                if let Some(mut speaker) = world.get_mut::<RadioSpeaker>(entity) {
                    speaker.channels = [id.clone()].into();
                }
            }
        }
    }
}

/// Речь рядом — слушающим устройствам (`ListeningSystem.PingListeners`).
pub fn ping_listeners(world: &mut World, source: Entity, message: &str, obfuscated: Option<&str>) {
    let Some(origin) = world.map_coordinates(source) else {
        return;
    };
    let listeners: Vec<(Entity, f32)> = world
        .query::<(Entity, &ActiveListener)>()
        .iter(world)
        .map(|(e, l)| (e, l.range))
        .collect();
    for (listener, range) in listeners {
        let Some(at) = world.map_coordinates(listener) else {
            continue;
        };
        if at.map != origin.map {
            continue;
        }
        let distance = origin.position.distance_squared(at.position);
        if distance > range * range {
            continue;
        }
        let mut attempt = ListenAttempt {
            source,
            cancelled: false,
        };
        world.raise(listener, &mut attempt);
        if attempt.cancelled {
            continue;
        }
        // Как в SS14: квадрат расстояния сравнивается с радиусом разборчивого шёпота.
        let text = match obfuscated {
            Some(obfuscated) if distance > WHISPER_CLEAR_RANGE => obfuscated,
            _ => message,
        };
        world.raise(
            listener,
            &mut Listen {
                message: text.to_owned(),
                source,
            },
        );
    }
}

fn clear_recently_sent(world: &mut World) {
    if let Some(mut recent) = world.get_resource_mut::<RecentlySent>() {
        recent.0.clear();
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<EncryptionKey>();
    world.register_data_component::<EncryptionKeyHolder>();
    world.register_data_component::<Headset>();
    world.register_data_component::<ActiveRadio>();
    world.register_data_component::<IntrinsicRadioReceiver>();
    world.register_data_component::<IntrinsicRadioTransmitter>();
    world.register_data_component::<TelecomServer>();
    world.register_data_component::<TelecomExempt>();
    world.register_data_component::<RadioMicrophone>();
    world.register_data_component::<RadioSpeaker>();
    world.register_data_component::<Intercom>();
    world.register_data_component::<ActiveListener>();
    if !side.is_server() {
        return;
    }
    world.init_resource::<RadioInFlight>();
    world.init_resource::<RecentlySent>();
    register_tool_event::<EncryptionRemovalFinished>(world);

    // Встроенные приёмник и передатчик (`OnIntrinsicReceive`, `OnIntrinsicSpeak`).
    world.subscribe::<IntrinsicRadioTransmitter, EntitySpoke>(|world, entity, event| {
        let Some(channel) = event.channel.clone() else {
            return;
        };
        if world
            .get::<IntrinsicRadioTransmitter>(entity)
            .is_some_and(|t| t.channels.contains(&channel))
        {
            let message = event.message.clone();
            send_radio_message(world, entity, &message, &channel, entity, true);
            event.channel = None;
        }
    });
    world.subscribe::<IntrinsicRadioReceiver, RadioReceive>(|world, entity, event| {
        let chat = event.chat.clone();
        send_to_actor(world, entity, &chat);
    });

    // Ключи шифрования.
    world.subscribe::<EncryptionKeyHolder, ComponentInit>(|world, holder, _| {
        world.ensure_container(holder, KEY_CONTAINER, ContainerKind::List);
        update_channels(world, holder);
    });
    world.subscribe::<EncryptionKeyHolder, EntInsertedIntoContainer>(|world, holder, event| {
        if &*event.container == KEY_CONTAINER {
            update_channels(world, holder);
        }
    });
    world.subscribe::<EncryptionKeyHolder, EntRemovedFromContainer>(|world, holder, event| {
        if &*event.container == KEY_CONTAINER {
            update_channels(world, holder);
        }
    });
    world.subscribe::<EncryptionKeyHolder, InteractUsing>(|world, holder, event| {
        if event.handled {
            return;
        }
        if world.get::<EncryptionKey>(event.used).is_some() {
            event.handled = true;
            try_insert_key(world, holder, event.user, event.used);
            return;
        }
        let method = world
            .get::<EncryptionKeyHolder>(holder)
            .map(|h| h.keys_extraction_method.clone())
            .unwrap_or_default();
        let has_keys = world
            .container(holder, KEY_CONTAINER)
            .is_some_and(|c| !c.is_empty());
        // Без ключей отвёртка не перехватывается: разборка не блокируется.
        if world.has_quality(event.used, &method) && has_keys {
            event.handled = true;
            try_remove_key(world, holder, event.user, event.used);
        }
    });
    world.subscribe::<EncryptionKeyHolder, DoAfterDone<EncryptionRemovalFinished>>(on_key_removal);
    world.subscribe::<EncryptionKeyHolder, Examined>(|world, holder, event| {
        if !event.is_in_details_range {
            return;
        }
        if world
            .container(holder, KEY_CONTAINER)
            .is_none_or(|c| c.is_empty())
        {
            event.push_markup(loc(world, "encryption-keys-no-keys", &[]), 0);
            return;
        }
        let Some(keys) = world.get::<EncryptionKeyHolder>(holder).cloned() else {
            return;
        };
        if keys.channels.is_empty() {
            return;
        }
        event.push_markup(loc(world, "examine-encryption-channels-prefix", &[]), 0);
        for line in channels_examine(
            world,
            &keys.channels,
            keys.default_channel.as_deref(),
            holder,
            "examine-encryption-channel",
        ) {
            event.push_markup(line, 0);
        }
    });
    world.subscribe::<EncryptionKey, Examined>(|world, key, event| {
        if !event.is_in_details_range {
            return;
        }
        let Some(component) = world.get::<EncryptionKey>(key).cloned() else {
            return;
        };
        if component.channels.is_empty() {
            return;
        }
        event.push_markup(loc(world, "examine-encryption-channels-prefix", &[]), 0);
        for line in channels_examine(
            world,
            &component.channels,
            component.default_channel.as_deref(),
            key,
            "examine-encryption-channel",
        ) {
            event.push_markup(line, 0);
        }
    });

    // Гарнитуры.
    world.subscribe::<Headset, EncryptionChannelsChanged>(|world, headset, _| {
        update_headset_channels(world, headset);
    });
    world.subscribe::<Headset, Equipped>(|world, headset, event| {
        if event.item != headset {
            return;
        }
        let Some(mut component) = world.get_mut::<Headset>(headset) else {
            return;
        };
        component.is_equipped = event.slot_flags.has(component.required_slot);
        if component.is_equipped && component.enabled {
            world
                .entity_mut(event.wearer)
                .insert(WearingHeadset { headset });
            update_headset_channels(world, headset);
        }
    });
    world.subscribe::<Headset, Unequipped>(|world, headset, event| {
        if event.item != headset {
            return;
        }
        if let Some(mut component) = world.get_mut::<Headset>(headset) {
            component.is_equipped = false;
        }
        world.entity_mut(headset).remove::<ActiveRadio>();
        world.entity_mut(event.wearer).remove::<WearingHeadset>();
    });
    world.subscribe::<WearingHeadset, EntitySpoke>(|world, wearer, event| {
        let Some(channel) = event.channel.clone() else {
            return;
        };
        let Some(headset) = world.get::<WearingHeadset>(wearer).map(|w| w.headset) else {
            return;
        };
        if world
            .get::<EncryptionKeyHolder>(headset)
            .is_some_and(|k| k.channels.contains(&channel))
        {
            let message = event.message.clone();
            send_radio_message(world, wearer, &message, &channel, headset, true);
            event.channel = None;
        }
    });
    world.subscribe::<Headset, RadioReceive>(|world, headset, event| {
        let Some(parent) = world.parent(headset) else {
            return;
        };
        world.raise(
            parent,
            &mut HeadsetRadioReceiveRelay {
                relayed: event.clone(),
            },
        );
        let chat = event.chat.clone();
        send_to_actor(world, parent, &chat);
    });
    world.subscribe::<Inventory, GetDefaultRadioChannel>(default_channel_from_inventory);

    // Микрофоны и динамики.
    world.subscribe::<RadioMicrophone, ComponentInit>(|world, entity, _| {
        let Some(mic) = world.get::<RadioMicrophone>(entity).cloned() else {
            return;
        };
        if mic.enabled {
            world.entity_mut(entity).insert(ActiveListener {
                range: mic.listen_range,
            });
        } else {
            world.entity_mut(entity).remove::<ActiveListener>();
        }
    });
    world.subscribe::<RadioSpeaker, ComponentInit>(|world, entity, _| {
        let Some(speaker) = world.get::<RadioSpeaker>(entity).cloned() else {
            return;
        };
        if speaker.enabled {
            let mut radio = world
                .get::<ActiveRadio>(entity)
                .cloned()
                .unwrap_or_default();
            radio.channels.extend(speaker.channels);
            world.entity_mut(entity).insert(radio);
        } else {
            world.entity_mut(entity).remove::<ActiveRadio>();
        }
    });
    world.subscribe::<RadioMicrophone, ActivateInWorld>(|world, entity, event| {
        if !event.complex
            || !world
                .get::<RadioMicrophone>(entity)
                .is_some_and(|m| m.toggle_on_interact)
        {
            return;
        }
        let enabled = world
            .get::<RadioMicrophone>(entity)
            .is_some_and(|m| m.enabled);
        set_microphone_enabled(world, entity, Some(event.user), !enabled, event.handled);
        event.handled = true;
    });
    world.subscribe::<RadioSpeaker, ActivateInWorld>(|world, entity, event| {
        if !event.complex
            || !world
                .get::<RadioSpeaker>(entity)
                .is_some_and(|s| s.toggle_on_interact)
        {
            return;
        }
        let enabled = world.get::<RadioSpeaker>(entity).is_some_and(|s| s.enabled);
        set_speaker_enabled(world, entity, Some(event.user), !enabled, event.handled);
        event.handled = true;
    });
    world.subscribe::<RadioMicrophone, PowerChanged>(|world, entity, event| {
        if !event.powered {
            set_microphone_enabled(world, entity, None, false, true);
        }
    });
    world.subscribe::<RadioMicrophone, Examined>(|world, entity, event| {
        if !event.is_in_details_range {
            return;
        }
        let Some(channel) = world
            .get::<RadioMicrophone>(entity)
            .map(|m| m.broadcast_channel.clone())
            .and_then(|id| radio_channel(world.get_resource::<Prototypes>()?, &id))
        else {
            return;
        };
        let line = loc(
            world,
            "radio-microphone-component-examine",
            &[
                ("color", LocArg::from(channel.color.clone())),
                ("channel", LocArg::from(channel_name(world, &channel))),
                ("frequency", LocArg::from(channel.frequency.clone())),
            ],
        );
        event.push_markup(line, 0);
    });
    world.subscribe::<RadioMicrophone, Listen>(|world, entity, event| {
        if world.get::<RadioSpeaker>(event.source).is_some() {
            return;
        }
        let Some(channel) = world
            .get::<RadioMicrophone>(entity)
            .map(|m| m.broadcast_channel.clone())
        else {
            return;
        };
        let key = (event.message.clone(), event.source, channel.clone());
        if world.resource_mut::<RecentlySent>().0.insert(key) {
            let message = event.message.clone();
            send_radio_message(world, event.source, &message, &channel, entity, true);
        }
    });
    world.subscribe::<RadioMicrophone, ListenAttempt>(|world, entity, event| {
        let Some(mic) = world.get::<RadioMicrophone>(entity).cloned() else {
            return;
        };
        if (mic.power_required && !is_powered(world, entity))
            || (mic.unobstructed_required
                && !world.in_range_unobstructed(event.source, entity, 0.0))
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<RadioSpeaker, RadioReceive>(|world, entity, event| {
        if entity == event.radio_source {
            return;
        }
        let original = crate::chat::speaker_name(world, event.message_source).voice_name;
        let speaker = world.entity_name(entity);
        let name = loc(
            world,
            "speech-name-relay",
            &[
                ("speaker", LocArg::from(speaker)),
                ("originalName", LocArg::from(original)),
            ],
        );
        let message = event.message.clone();
        crate::chat::try_send_in_game_ic_message(
            world,
            entity,
            &message,
            InGameIcChatType::Whisper,
            ChatTransmitRange::GhostRangeLimit,
            false,
            None,
            Some(&name),
            false,
            false,
        );
    });
    world.subscribe::<Intercom, EncryptionChannelsChanged>(|world, entity, event| {
        let supported: Vec<String> = event.channels.iter().cloned().collect();
        let current = world
            .get::<Intercom>(entity)
            .and_then(|i| i.current_channel.clone());
        let channel = match current {
            Some(current) if supported.contains(&current) => Some(current),
            _ => event.default_channel.clone(),
        };
        if let Some(mut intercom) = world.get_mut::<Intercom>(entity) {
            intercom.supported_channels = supported;
        }
        set_intercom_channel(world, entity, channel);
    });

    // Слушающие устройства.
    world.subscribe_broadcast::<EntitySpoke>(|world, event| {
        let (source, message, obfuscated) = (
            event.source,
            event.message.clone(),
            event.obfuscated_message.clone(),
        );
        ping_listeners(world, source, &message, obfuscated.as_deref());
    });
    world.add_game_systems(Update, clear_recently_sent);
}

#[cfg(test)]
mod tests;
