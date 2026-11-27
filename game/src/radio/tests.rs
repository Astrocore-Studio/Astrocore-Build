use astrocore_ecs::EventWorldExt;
use astrocore_map::ContainerWorldExt;
use astrocore_net::{Actor, ConnId, NetUserId, Session};
use bevy_ecs::resource::Resource;

use super::*;
use crate::chat::{ChatMessageSent, try_send_in_game_ic_message};
use crate::inventory::{EquipOptions, InventoryWorldExt};
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r##"
- type: inventoryTemplate
  id: human
  slots:
  - name: ears
    slotFlags: EARS
    uiWindowPos: 0,0
    strippingWindowPos: 0,0
    displayName: Ears
  - name: pocket1
    slotFlags: POCKET
    uiWindowPos: 1,0
    strippingWindowPos: 1,0
    displayName: Pocket
- type: speechVerb
  id: Default
  speechVerbStrings: [ chat-speech-verb-default ]
- type: radioChannel
  id: Common
  name: chat-radio-common
  keycode: ";"
  frequency: 145.9
  color: "#2cdb2c"
- type: radioChannel
  id: Engineering
  name: chat-radio-engineering
  keycode: 'e'
  frequency: 135.7
  color: "#ff733c"
- type: radioChannel
  id: Binary
  name: chat-radio-binary
  keycode: 'b'
  color: "#2ed2fd"
  longRange: true
- type: entity
  id: Person
  components:
  - type: Speech
  - type: Inventory
- type: entity
  id: KeyCommon
  components:
  - type: Item
  - type: EncryptionKey
    channels: [ Common ]
    defaultChannel: Common
- type: entity
  id: KeyEngineering
  components:
  - type: Item
  - type: EncryptionKey
    channels: [ Engineering ]
    defaultChannel: Engineering
- type: entity
  id: Headset
  components:
  - type: Item
  - type: Clothing
    slots: [ ears ]
  - type: ContainerContainer
    containers:
      key_slots: !type:Container
  - type: ContainerFill
    containers:
      key_slots:
      - KeyCommon
  - type: Headset
  - type: EncryptionKeyHolder
    keySlots: 2
- type: entity
  id: HeadsetEngineering
  parent: Headset
  components:
  - type: ContainerFill
    containers:
      key_slots:
      - KeyEngineering
      - KeyCommon
- type: entity
  id: Server
  components:
  - type: TelecomServer
  - type: ApcPowerReceiver
    needsPower: false
  - type: ContainerContainer
    containers:
      key_slots: !type:Container
  - type: ContainerFill
    containers:
      key_slots:
      - KeyCommon
      - KeyEngineering
  - type: EncryptionKeyHolder
- type: entity
  id: Borg
  components:
  - type: Speech
  - type: IntrinsicRadioTransmitter
    channels: [ Binary ]
  - type: IntrinsicRadioReceiver
  - type: ActiveRadio
    channels: [ Binary ]
- type: entity
  id: HandheldRadio
  components:
  - type: Item
  - type: Speech
  - type: RadioMicrophone
  - type: RadioSpeaker
  - type: TelecomExempt
- type: entity
  id: Screwdriver
  components:
  - type: Item
  - type: Tool
    qualities: [ Screwing ]
"##;

#[derive(Resource, Default)]
struct Heard(Vec<ChatMessageSent>);

struct Scene {
    world: World,
    grid: Entity,
}

fn scene() -> Scene {
    let mut world = world_with(PROTOTYPES);
    world.insert_resource(crate::random::GameRandom::seeded(4));
    world.init_resource::<Heard>();
    world.subscribe_broadcast::<ChatMessageSent>(|world, event| {
        let event = event.clone();
        world.resource_mut::<Heard>().0.push(event);
    });
    let grid = station(&mut world);
    Scene { world, grid }
}

impl Scene {
    fn player(&mut self, prototype: &str, x: i32, n: u8) -> (Entity, Entity) {
        self.player_on(prototype, self.grid, x, n)
    }

    fn player_on(&mut self, prototype: &str, grid: Entity, x: i32, n: u8) -> (Entity, Entity) {
        let body = spawn_at(&mut self.world, prototype, grid, x, 1);
        let session = self
            .world
            .spawn(Session {
                conn: ConnId(u64::from(n)),
                user_id: NetUserId(uuid::Uuid::from_bytes([n; 16])),
                name: format!("p{n}"),
                attached: Some(body),
                last_input: 0,
                sync: Default::default(),
            })
            .id();
        self.world.entity_mut(body).insert(Actor { session });
        (session, body)
    }

    fn wear(&mut self, body: Entity, prototype: &str, slot: &str) -> Entity {
        let item = spawn_at(&mut self.world, prototype, self.grid, 0, 0);
        let options = EquipOptions {
            silent: true,
            force: true,
            delay: false,
        };
        self.world.try_equip(body, body, item, slot, options);
        item
    }

    fn say(&mut self, session: Entity, body: Entity, text: &str) {
        try_send_in_game_ic_message(
            &mut self.world,
            body,
            text,
            InGameIcChatType::Speak,
            ChatTransmitRange::Normal,
            false,
            Some(session),
            None,
            true,
            false,
        );
    }

    /// Сообщения радио, пришедшие игроку.
    fn radio_to(&mut self, session: Entity) -> Vec<ChatMessage> {
        self.world
            .resource::<Heard>()
            .0
            .iter()
            .filter(|s| s.message.channel == ChatChannel::RADIO && s.recipients.contains(&session))
            .map(|s| s.message.clone())
            .collect()
    }

    fn clear(&mut self) {
        self.world.resource_mut::<Heard>().0.clear();
    }
}

#[test]
fn headset_channels_come_from_keys() {
    let mut scene = scene();
    let headset = spawn_at(&mut scene.world, "HeadsetEngineering", scene.grid, 0, 0);
    let keys = scene
        .world
        .get::<EncryptionKeyHolder>(headset)
        .unwrap()
        .clone();
    assert_eq!(
        keys.channels,
        ["Common".to_owned(), "Engineering".to_owned()].into()
    );
    // Канал по умолчанию — от первого ключа.
    assert_eq!(keys.default_channel.as_deref(), Some("Engineering"));
    // Как в SS14: приёмник есть и у лежащей гарнитуры, но передать сообщение некому.
    assert!(scene.world.get::<ActiveRadio>(headset).is_some());
}

#[test]
fn headset_talk_needs_a_telecom_server() {
    let mut scene = scene();
    let (speaker, speaker_body) = scene.player("Person", 1, 1);
    let (listener, listener_body) = scene.player("Person", 12, 2);
    let (deaf, _) = scene.player("Person", 13, 3);
    scene.wear(speaker_body, "Headset", "ears");
    scene.wear(listener_body, "Headset", "ears");
    assert!(scene.world.get::<WearingHeadset>(listener_body).is_some());
    // Без сервера связи гарнитуры молчат, а рядом слышен шёпот.
    scene.say(speaker, speaker_body, "; hello crew");
    assert!(scene.radio_to(listener).is_empty());
    let server = spawn_at(&mut scene.world, "Server", scene.grid, 8, 8);
    assert!(
        scene
            .world
            .get::<EncryptionKeyHolder>(server)
            .unwrap()
            .channels
            .contains("Common")
    );
    scene.say(speaker, speaker_body, "; hello crew");
    let heard = scene.radio_to(listener);
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0].message, "Hello crew");
    assert!(
        heard[0]
            .wrapped_message
            .starts_with("chat-radio-message-wrap")
    );
    assert_eq!(scene.radio_to(speaker).len(), 1);
    assert!(scene.radio_to(deaf).is_empty());
    // Канала нет в ключах — сообщение не уходит.
    scene.clear();
    scene.say(speaker, speaker_body, ":e engines");
    assert!(scene.radio_to(listener).is_empty());
}

#[test]
fn default_channel_and_unequip() {
    let mut scene = scene();
    spawn_at(&mut scene.world, "Server", scene.grid, 8, 8);
    let (engineer, engineer_body) = scene.player("Person", 1, 1);
    let (other, other_body) = scene.player("Person", 12, 2);
    let headset = scene.wear(engineer_body, "HeadsetEngineering", "ears");
    scene.wear(other_body, "HeadsetEngineering", "ears");
    let mut event = GetDefaultRadioChannel::default();
    scene.world.raise(engineer_body, &mut event);
    assert_eq!(event.channel.as_deref(), Some("Engineering"));
    scene.say(engineer, engineer_body, ":h pressure is fine");
    assert_eq!(scene.radio_to(other).len(), 1);
    // В кармане гарнитура канала по умолчанию не даёт и не передаёт.
    scene.world.try_unequip(
        engineer_body,
        engineer_body,
        "ears",
        EquipOptions {
            silent: true,
            force: true,
            delay: false,
        },
    );
    assert!(scene.world.get::<WearingHeadset>(engineer_body).is_none());
    assert!(scene.world.get::<ActiveRadio>(headset).is_none());
    scene.wear(engineer_body, "HeadsetEngineering", "pocket1");
    let mut event = GetDefaultRadioChannel::default();
    scene.world.raise(engineer_body, &mut event);
    assert_eq!(event.channel, None);
}

#[test]
fn keys_go_in_and_come_out_with_a_screwdriver() {
    let mut scene = scene();
    let (_, user) = scene.player("Person", 1, 1);
    let headset = spawn_at(&mut scene.world, "Headset", scene.grid, 2, 1);
    let key = spawn_at(&mut scene.world, "KeyEngineering", scene.grid, 2, 1);
    let click = scene.world.map_coordinates(headset).unwrap();
    let mut using = InteractUsing {
        user,
        used: key,
        target: headset,
        click,
        handled: false,
    };
    scene.world.raise(headset, &mut using);
    assert!(using.handled);
    let channels = scene
        .world
        .get::<EncryptionKeyHolder>(headset)
        .unwrap()
        .channels
        .clone();
    assert!(channels.contains("Engineering"));
    // Слоты заняты — третий ключ не входит.
    let extra = spawn_at(&mut scene.world, "KeyCommon", scene.grid, 2, 1);
    scene.world.raise(
        headset,
        &mut InteractUsing {
            used: extra,
            ..using
        },
    );
    assert_eq!(
        scene.world.container(headset, KEY_CONTAINER).unwrap().len(),
        2
    );
    // Отвёртка: все ключи вынуты, каналов нет.
    let tool = spawn_at(&mut scene.world, "Screwdriver", scene.grid, 2, 1);
    scene.world.raise(
        headset,
        &mut DoAfterDone {
            event: EncryptionRemovalFinished,
            user,
            target: Some(headset),
            used: Some(tool),
            cancelled: false,
            handled: false,
            repeat: false,
            delay: 0.0,
        },
    );
    assert_eq!(
        scene.world.container(headset, KEY_CONTAINER).unwrap().len(),
        0
    );
    assert!(
        scene
            .world
            .get::<EncryptionKeyHolder>(headset)
            .unwrap()
            .channels
            .is_empty()
    );
}

#[test]
fn intrinsic_radio_reaches_other_maps_on_long_range_channels() {
    let mut scene = scene();
    let (borg, borg_body) = scene.player("Borg", 1, 1);
    let far_grid = station(&mut scene.world);
    let (other, _) = scene.player_on("Borg", far_grid, 1, 2);
    scene.say(borg, borg_body, ":b beep");
    let heard = scene.radio_to(other);
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0].message, "Beep");
}

#[test]
fn handheld_radio_relays_speech_nearby() {
    let mut scene = scene();
    let (speaker, speaker_body) = scene.player("Person", 1, 1);
    let (listener, _) = scene.player("Person", 11, 2);
    let near = spawn_at(&mut scene.world, "HandheldRadio", scene.grid, 2, 1);
    let far = spawn_at(&mut scene.world, "HandheldRadio", scene.grid, 12, 1);
    set_microphone_enabled(&mut scene.world, near, None, true, true);
    set_speaker_enabled(&mut scene.world, far, None, true, true);
    assert!(scene.world.get::<ActiveListener>(near).is_some());
    assert!(scene.world.get::<ActiveRadio>(far).is_some());
    scene.say(speaker, speaker_body, "anyone copy");
    // Дальняя рация шепчет услышанное; рядом с ней — второй игрок.
    let sent = scene.world.resource::<Heard>().0.clone();
    let relayed: Vec<&ChatMessageSent> = sent
        .iter()
        .filter(|s| s.message.channel == ChatChannel::WHISPER && s.recipients.contains(&listener))
        .collect();
    assert_eq!(relayed.len(), 1);
    assert_eq!(relayed[0].message.message, "Anyone copy");
    // Выключение «использовать»: микрофон перестаёт слушать.
    let mut activate = ActivateInWorld {
        user: speaker_body,
        target: near,
        complex: true,
        handled: false,
    };
    scene.world.raise(near, &mut activate);
    assert!(scene.world.get::<ActiveListener>(near).is_none());
}
