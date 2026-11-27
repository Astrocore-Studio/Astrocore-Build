use astrocore_ecs::EventWorldExt;
use astrocore_net::ConnId;

use super::*;
use crate::testing::{spawn_at, station, world_with};

const PROTOTYPES: &str = r#"
- type: speechVerb
  id: Default
  speechVerbStrings: [ chat-speech-verb-default ]
- type: speechVerb
  id: DefaultQuestion
  speechVerbStrings: [ chat-speech-verb-question ]
- type: radioChannel
  id: Common
  keycode: ";"
- type: radioChannel
  id: Engineering
  keycode: 'e'
- type: emote
  id: Scream
  name: chat-emote-name-scream
  category: Vocal
  whitelist:
    components:
    - Vocal
  chatMessages: [ chat-emote-msg-scream ]
  chatTriggers: [ scream, screams ]
- type: emote
  id: Wave
  name: chat-emote-name-wave
  chatMessages: [ chat-emote-msg-wave ]
  chatTriggers: [ waves ]
- type: accent
  id: chatsanitize
  wordReplacements:
    idk: i don't know
- type: entity
  id: Talker
  components:
  - type: Speech
  - type: Vocal
- type: entity
  id: Mute
  components:
  - type: Speech
    enabled: false
- type: entity
  id: Observer
  components:
  - type: Ghost
  - type: GhostHearing
"#;

struct Scene {
    world: World,
    grid: Entity,
}

fn scene() -> Scene {
    let mut world = world_with(PROTOTYPES);
    world.insert_resource(crate::random::GameRandom::seeded(2));
    world.init_resource::<Heard>();
    world.subscribe_broadcast::<ChatMessageSent>(|world, event| {
        let event = event.clone();
        world.resource_mut::<Heard>().0.push(event);
    });
    let grid = station(&mut world);
    Scene { world, grid }
}

#[derive(Resource, Default)]
struct Heard(Vec<ChatMessageSent>);

impl Scene {
    /// Игрок в теле прототипа на клетке.
    fn player(&mut self, prototype: &str, x: i32, n: u8) -> (Entity, Entity) {
        let body = spawn_at(&mut self.world, prototype, self.grid, x, 1);
        if let Some(mut meta) = self.world.get_mut::<astrocore_ecs::MetaData>(body) {
            meta.name = Some(format!("P{n}"));
        }
        let session = self
            .world
            .spawn(Session {
                conn: ConnId(u64::from(n)),
                user_id: NetUserId(uuid::Uuid::from_bytes([n; 16])),
                name: format!("player{n}"),
                attached: Some(body),
                last_input: 0,
                sync: Default::default(),
            })
            .id();
        (session, body)
    }

    fn heard(&mut self) -> Vec<ChatMessageSent> {
        std::mem::take(&mut self.world.resource_mut::<Heard>().0)
    }

    fn say(&mut self, session: Entity, body: Entity, text: &str, kind: InGameIcChatType) {
        try_send_in_game_ic_message(
            &mut self.world,
            body,
            text,
            kind,
            ChatTransmitRange::Normal,
            false,
            Some(session),
            None,
            true,
            false,
        );
    }
}

fn to(sent: &[ChatMessageSent], session: Entity) -> Vec<ChatMessage> {
    sent.iter()
        .filter(|s| s.recipients.contains(&session))
        .map(|s| s.message.clone())
        .collect()
}

#[test]
fn speech_reaches_voice_range_with_verb_and_capital() {
    let mut scene = scene();
    let (speaker, body) = scene.player("Talker", 1, 1);
    let (near, _) = scene.player("Talker", 4, 2);
    let (far, _) = scene.player("Talker", 14, 3);
    let (ghost, _) = scene.player("Observer", 15, 4);
    scene.say(
        speaker,
        body,
        "where is the captain?",
        InGameIcChatType::Speak,
    );
    let sent = scene.heard();
    let heard = to(&sent, near);
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0].channel, ChatChannel::LOCAL);
    assert_eq!(heard[0].message, "Where is the captain?");
    // Без перевода — ключ обёртки; глагол — по окончанию (без перевода окончание — ключ).
    assert_eq!(
        heard[0].wrapped_message,
        "chat-manager-entity-say-wrap-message"
    );
    assert_eq!(
        get_speech_verb(&scene.world, body, "Hi").unwrap().id,
        "Default"
    );
    assert_eq!(
        get_speech_verb(&scene.world, body, "Where chat-speech-verb-suffix-question")
            .unwrap()
            .id,
        "DefaultQuestion"
    );
    assert!(to(&sent, far).is_empty());
    scene.say(speaker, body, "idk, i guess", InGameIcChatType::Speak);
    let replaced = scene.heard();
    assert_eq!(to(&replaced, near)[0].message, "I don't know, I guess");
    // Слышащий всё призрак получает и издалека.
    assert_eq!(to(&sent, ghost).len(), 1);
    assert_eq!(to(&sent, speaker).len(), 1);
}

#[test]
fn whispers_fade_with_distance() {
    let mut scene = scene();
    let (speaker, body) = scene.player("Talker", 1, 1);
    let (close, _) = scene.player("Talker", 2, 2);
    let (muffled, _) = scene.player("Talker", 5, 3);
    let (away, _) = scene.player("Talker", 9, 4);
    scene.say(
        speaker,
        body,
        "secret plans here",
        InGameIcChatType::Whisper,
    );
    let sent = scene.heard();
    assert_eq!(to(&sent, close)[0].message, "Secret plans here");
    let blurred = &to(&sent, muffled)[0].message;
    assert_ne!(blurred, "Secret plans here");
    assert_eq!(blurred.len(), "Secret plans here".len());
    assert!(blurred.contains('~'));
    assert!(to(&sent, away).is_empty());
}

#[test]
fn blocked_speakers_and_radio_prefixes() {
    let mut scene = scene();
    let (mute, mute_body) = scene.player("Mute", 1, 1);
    let (listener, _) = scene.player("Talker", 2, 2);
    scene.say(mute, mute_body, "hello", InGameIcChatType::Speak);
    assert!(scene.heard().is_empty());
    // Префикс радио — шёпот рядом и речь в канал.
    let (talker, body) = scene.player("Talker", 3, 3);
    scene.world.init_resource::<Spoken>();
    scene
        .world
        .subscribe::<Speech, EntitySpoke>(|world, _, event| {
            let channel = event.channel.clone();
            world.resource_mut::<Spoken>().0.push(channel);
        });
    scene.say(talker, body, ";all hands", InGameIcChatType::Speak);
    scene.say(talker, body, ":e engine is fine", InGameIcChatType::Speak);
    scene.say(talker, body, ">;not radio", InGameIcChatType::Speak);
    let spoken = scene.world.resource::<Spoken>().0.clone();
    assert_eq!(
        spoken,
        [Some("Common".into()), Some("Engineering".into()), None]
    );
    let sent = scene.heard();
    let heard = to(&sent, listener);
    assert_eq!(heard[0].channel, ChatChannel::WHISPER);
    assert_eq!(heard[0].message, "All hands");
    assert_eq!(heard[2].channel, ChatChannel::LOCAL);
    assert_eq!(heard[2].message, ";not radio");
}

#[derive(Resource, Default)]
struct Spoken(Vec<Option<String>>);

#[derive(Resource, Default)]
struct Emoted(Vec<String>);

#[test]
fn emotes_and_shorthands() {
    let mut scene = scene();
    let (session, body) = scene.player("Talker", 1, 1);
    let (_, mute) = scene.player("Mute", 3, 2);
    scene.world.init_resource::<Emoted>();
    scene
        .world
        .subscribe::<Speech, EmoteEvent>(|world, _, event| {
            let emote = event.emote.clone();
            world.resource_mut::<Emoted>().0.push(emote);
        });
    // Слово действия вызывает эмоцию; строка чата — канал эмоций.
    scene.say(session, body, "waves.", InGameIcChatType::Emote);
    let sent = scene.heard();
    let own = to(&sent, session);
    assert_eq!(own[0].channel, ChatChannel::EMOTES);
    assert_eq!(own[0].message, "waves.");
    // Смайлик — эмоция отдельно, фраза без него.
    scene.say(session, body, "nice work :)", InGameIcChatType::Speak);
    let sent = scene.heard();
    let own = to(&sent, session);
    assert_eq!(own[0].channel, ChatChannel::EMOTES);
    assert_eq!(own[0].message, "chatsan-smiles");
    assert_eq!(own[1].message, "Nice work");
    // Крик — только с голосом (белый список).
    assert!(try_emote_with_chat(
        &mut scene.world,
        body,
        "Scream",
        ChatTransmitRange::Normal
    ));
    assert!(!try_emote_with_chat(
        &mut scene.world,
        mute,
        "Scream",
        ChatTransmitRange::Normal
    ));
    assert_eq!(scene.world.resource::<Emoted>().0, ["Wave", "Scream"]);
}

#[test]
fn text_cleanup_matches_ss14() {
    assert_eq!(capitalize("ёлка"), "Ёлка");
    assert_eq!(
        capitalize_the_word_i("i think i'm fine, ii"),
        "I think I'm fine, ii"
    );
    assert_eq!(add_period("ok"), "ok.");
    assert_eq!(add_period("ok?"), "ok?");
    let words = [("idk".to_owned(), "i don't know".to_owned())];
    assert_eq!(
        apply_word_replacements("idk, IDK. Idk", &words),
        "i don't know, I DON'T KNOW. I don't know"
    );
    assert_eq!(apply_word_replacements("kidk", &words), "kidk");
    assert_eq!(
        sanitize_emote_shorthands("hello :D"),
        ("hello".into(), Some("chatsan-smiles-widely"))
    );
    assert_eq!(
        sanitize_emote_shorthands(":( why"),
        ("why".into(), Some("chatsan-frowns"))
    );
    assert_eq!(sanitize_emote_shorthands("a:)b"), ("a:)b".into(), None));
    // Последний по месту смайлик — эмоция; убираются все.
    assert_eq!(
        sanitize_emote_shorthands("lol ok :("),
        ("ok".into(), Some("chatsan-frowns"))
    );
}

#[test]
fn ooc_looc_dead_and_admin_channels() {
    let mut scene = scene();
    let (alive, alive_body) = scene.player("Talker", 1, 1);
    let (other, _) = scene.player("Talker", 2, 2);
    let (ghost, ghost_body) = scene.player("Observer", 3, 3);
    // OOC всем; в раунде выключается, после — включается.
    try_send_ooc_message(&mut scene.world, alive, "hi all", OocChatType::Ooc);
    let sent = scene.heard();
    assert_eq!(sent[0].message.channel, ChatChannel::OOC);
    assert_eq!(sent[0].recipients.len(), 3);
    scene
        .world
        .raise_broadcast(&mut crate::game_ticker::GameRunLevelChanged {
            old: crate::game_ticker::GameRunLevel::PreRoundLobby,
            new: crate::game_ticker::GameRunLevel::InRound,
        });
    assert!(!scene.world.cvar(&OOC_ENABLED));
    scene.heard();
    try_send_ooc_message(&mut scene.world, alive, "still?", OocChatType::Ooc);
    assert!(scene.heard().is_empty());
    // LOOC — рядом.
    try_send_in_game_ooc_message(
        &mut scene.world,
        alive_body,
        "brb",
        InGameOocChatType::Looc,
        false,
        Some(alive),
    );
    let sent = scene.heard();
    assert_eq!(sent[0].message.channel, ChatChannel::LOOC);
    assert!(to(&sent, other).len() == 1);
    // Призрак говорит — чат мёртвых, его видят только призраки (администраторов нет).
    scene.say(ghost, ghost_body, "boo", InGameIcChatType::Speak);
    let sent = scene.heard();
    assert_eq!(sent[0].message.channel, ChatChannel::DEAD);
    assert_eq!(sent[0].recipients, [ghost]);
    // Чат администраторов не-администратору недоступен.
    try_send_ooc_message(&mut scene.world, alive, "admin?", OocChatType::Admin);
    assert!(scene.heard().is_empty());
}

#[test]
fn rate_limit_and_length_limit() {
    let mut scene = scene();
    let (session, body) = scene.player("Talker", 1, 1);
    // Учёт частоты идёт по настоящему времени: короткий период, чтобы дождаться нового.
    scene.world.set_cvar(&CHAT_RATE_LIMIT_PERIOD, 0.3).unwrap();
    for i in 0..10 {
        scene.say(session, body, &format!("line {i}"), InGameIcChatType::Speak);
    }
    assert_eq!(scene.heard().len(), 10);
    scene.say(session, body, "one too many", InGameIcChatType::Speak);
    let sent = scene.heard();
    // Вместо речи — сообщение сервера о частоте и оповещение администраторов.
    // Как в SS14, сначала оповещение администраторов.
    let channels: Vec<ChatChannel> = sent.iter().map(|m| m.message.channel).collect();
    assert_eq!(channels, [ChatChannel::ADMIN_ALERT, ChatChannel::SERVER]);
    assert_eq!(sent[1].recipients, [session]);
    // Сообщение о частоте — раз за период.
    scene.say(session, body, "still too many", InGameIcChatType::Speak);
    assert!(scene.heard().is_empty());
    // Через период снова можно; длинное — нельзя.
    std::thread::sleep(std::time::Duration::from_millis(350));
    scene.world.set_cvar(&CHAT_MAX_MESSAGE_LENGTH, 5).unwrap();
    scene.say(session, body, "far too long", InGameIcChatType::Speak);
    let sent = scene.heard();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].message.channel, ChatChannel::SERVER);
}

#[test]
fn announcements_reach_station_crew() {
    let mut scene = scene();
    let (on_station, body) = scene.player("Talker", 1, 1);
    scene
        .world
        .entity_mut(scene.grid)
        .insert(crate::stations::BecomesStation { id: "S".into() });
    let config = crate::stations::StationConfig {
        station_proto: "Talker".into(),
        components: Vec::new(),
    };
    crate::stations::initialize_new_station(
        &mut scene.world,
        &config,
        &[scene.grid],
        Some("NT"),
        None,
    );
    let other_grid = station(&mut scene.world);
    let off = spawn_at(&mut scene.world, "Talker", other_grid, 1, 1);
    let off_session = scene
        .world
        .spawn(Session {
            conn: ConnId(9),
            user_id: NetUserId(uuid::Uuid::from_bytes([9; 16])),
            name: "away".into(),
            attached: Some(off),
            last_input: 0,
            sync: Default::default(),
        })
        .id();
    dispatch_station_announcement(&mut scene.world, body, "drill", None, false, None);
    let sent = scene.heard();
    assert_eq!(sent[0].recipients, [on_station]);
    assert_eq!(sent[0].message.channel, ChatChannel::RADIO);
    dispatch_global_announcement(&mut scene.world, "all", None, false, None);
    let sent = scene.heard();
    assert_eq!(sent[0].recipients.len(), 2);
    assert!(sent[0].recipients.contains(&off_session));
}
