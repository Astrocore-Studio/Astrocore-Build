use astrocore_console::ConfigWorldExt;
use astrocore_net::ConsoleLog;

use super::*;

fn world() -> GameWorld {
    let mut world = GameWorld::new();
    world.register_cvar(&CHAT_MAX_MESSAGE_LENGTH);
    world.init_resource::<ClientChat>();
    world.init_resource::<ClientNet>();
    world
}

fn with_body(world: &mut GameWorld, ghost: bool) -> GameEntity {
    let body = world.spawn_empty().id();
    if ghost {
        world.entity_mut(body).insert(Ghost::default());
    }
    world.resource_mut::<ClientNet>().attached = Some(body);
    body
}

fn sent(world: &GameWorld) -> Vec<String> {
    world
        .get_resource::<ConsoleLog>()
        .map(|log| log.lines.iter().map(|l| l.text.clone()).collect())
        .unwrap_or_default()
}

fn type_and_send(world: &mut GameWorld, text: &str, channel: SelectChannel) -> Result<(), String> {
    let mut input = world.get_resource_or_insert_with(ChatInput::default);
    input.text = text.to_owned();
    input.selected = channel;
    submit(world)
}

#[test]
fn channels_depend_on_body_and_ghost() {
    let mut world = world();
    let lobby = can_send_channels(&world);
    assert!(lobby.contains(&SelectChannel::Ooc));
    assert!(!lobby.contains(&SelectChannel::Local));
    with_body(&mut world, false);
    let alive = can_send_channels(&world);
    assert!(alive.contains(&SelectChannel::Radio));
    assert!(!alive.contains(&SelectChannel::Dead));
    with_body(&mut world, true);
    let ghost = can_send_channels(&world);
    assert!(ghost.contains(&SelectChannel::Dead));
    assert!(!ghost.contains(&SelectChannel::Whisper));
}

#[test]
fn prefixes_pick_channels_like_ss14() {
    let mut world = world();
    with_body(&mut world, false);
    let allowed = can_send_channels(&world);
    let split = |text: &str| split_input(&world, text, &allowed);
    assert_eq!(
        split("[ hi all"),
        (Some(SelectChannel::Ooc), "hi all".into())
    );
    assert_eq!(split("(brb"), (Some(SelectChannel::Looc), "brb".into()));
    assert_eq!(
        split("@waves"),
        (Some(SelectChannel::Emotes), "waves".into())
    );
    assert_eq!(
        split("*waves"),
        (Some(SelectChannel::Emotes), "waves".into())
    );
    assert_eq!(
        split(",psst"),
        (Some(SelectChannel::Whisper), "psst".into())
    );
    // Радио и «рядом» идут в `say` с префиксом.
    assert_eq!(
        split("; help"),
        (Some(SelectChannel::Radio), "; help".into())
    );
    assert_eq!(
        split(":h help"),
        (Some(SelectChannel::Radio), ":h help".into())
    );
    assert_eq!(split(">hey"), (Some(SelectChannel::Local), ">hey".into()));
    // Неизвестный канал или недоступный — без префикса.
    assert_eq!(split(":z what"), (None, ":z what".into()));
    assert_eq!(split("\\boo"), (None, "\\boo".into()));
    assert_eq!(split("hello"), (None, "hello".into()));
    // Призраку «рядом» недоступно: строка уходит в выбранный канал как есть.
    with_body(&mut world, true);
    let allowed = can_send_channels(&world);
    assert_eq!(split_input(&world, ">boo", &allowed), (None, ">boo".into()));
}

#[test]
fn messages_become_console_commands() {
    let mut world = world();
    with_body(&mut world, false);
    type_and_send(&mut world, "hello \"crew\"", SelectChannel::Local).unwrap();
    type_and_send(&mut world, "all hands", SelectChannel::Radio).unwrap();
    type_and_send(&mut world, "[ gg", SelectChannel::Local).unwrap();
    type_and_send(&mut world, "waves", SelectChannel::Emotes).unwrap();
    type_and_send(&mut world, "   ", SelectChannel::Local).unwrap();
    assert_eq!(
        sent(&world),
        [
            "> say \"hello \\\"crew\\\"\"",
            "> say \";all hands\"",
            "> ooc \"gg\"",
            "> me \"waves\"",
        ]
    );
    // Призрак говорит в чат мёртвых через `say`; живой не может писать в мёртвых.
    assert_eq!(
        command_for("boo", SelectChannel::Dead, true).as_deref(),
        Some("say \"boo\"")
    );
    assert_eq!(
        command_for("boo", SelectChannel::Dead, false).as_deref(),
        Some("dsay \"boo\"")
    );
    assert_eq!(
        command_for("lsmap", SelectChannel::Console, false).as_deref(),
        Some("lsmap")
    );
}

#[test]
fn long_messages_are_refused_with_a_warning() {
    let mut world = world();
    with_body(&mut world, false);
    world.set_cvar(&CHAT_MAX_MESSAGE_LENGTH, 5).unwrap();
    let warning = type_and_send(&mut world, "far too long", SelectChannel::Local).unwrap_err();
    assert_eq!(warning, "chat-manager-max-message-length");
    assert!(sent(&world).is_empty());
    assert!(world.resource::<ChatInput>().text.is_empty());
}

#[test]
fn tab_cycles_through_allowed_channels() {
    let mut world = world();
    with_body(&mut world, false);
    world.init_resource::<ChatInput>();
    cycle_channel(&mut world, true);
    assert_eq!(
        world.resource::<ChatInput>().selected,
        SelectChannel::Whisper
    );
    cycle_channel(&mut world, false);
    cycle_channel(&mut world, false);
    // Перед «рядом» — консоль (админ и мёртвые недоступны).
    assert_eq!(
        world.resource::<ChatInput>().selected,
        SelectChannel::Console
    );
}

#[test]
fn markup_turns_into_colored_spans() {
    let base = GameColor::WHITE;
    let spans = markup_spans(
        "[color=#2cdb2c]\\[Common\\] [bold]Urist[/bold] says, [font=Default size=12]“hi”[/font][/color] done",
        base,
    );
    let green = GameColor::from_hex("#2cdb2c").unwrap();
    let text: String = spans.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(text, "[Common] Urist says, “hi” done");
    assert_eq!(spans[0].color, green);
    assert!(spans.iter().any(|s| s.bold && s.text == "Urist"));
    assert_eq!(spans.last().unwrap().color, base);
    // Цвет по имени и незакрытая скобка.
    let named = markup_spans("[color=orange]x[/color] [oops", base);
    assert_eq!(named[0].color, GameColor::parse("orange").unwrap());
    assert_eq!(
        named.iter().map(|s| s.text.as_str()).collect::<String>(),
        "x [oops"
    );
    assert_eq!(
        string_inside_tag(
            "[BubbleHeader]Bob[/BubbleHeader] says [BubbleContent]Hi[/BubbleContent]",
            "BubbleContent"
        ),
        "Hi"
    );
    assert_eq!(string_inside_tag("plain", "BubbleContent"), "");
}

fn speech(text: &str, channel: ChatChannel) -> ChatMessage {
    ChatMessage {
        channel,
        message: text.into(),
        wrapped_message: format!(
            "[BubbleHeader]Bob[/BubbleHeader] says, [BubbleContent]{text}[/BubbleContent]"
        ),
        sender: None,
        sender_key: None,
        hide_chat: false,
        color_override: None,
        audio_path: None,
        audio_volume: 0.0,
    }
}

#[test]
fn bubbles_queue_stack_and_expire() {
    let mut world = GameWorld::new();
    let speaker = world.spawn_empty().id();
    let mut bubbles = SpeechBubbles::default();
    for i in 0..6 {
        bubbles.enqueue(
            speaker,
            speech(&format!("line {i}"), ChatChannel::LOCAL),
            SpeechType::Say,
        );
    }
    // Первое появляется сразу, следующие — с задержкой по длине.
    bubbles.update(0.0, |_| true);
    assert_eq!(bubbles.active.len(), 1);
    assert_eq!(bubbles.active[0].content, "line 0");
    assert_eq!(bubbles.active[0].header, "Bob");
    bubbles.update(0.1, |_| true);
    assert_eq!(bubbles.active.len(), 1);
    for _ in 0..5 {
        bubbles.update(0.3, |_| true);
    }
    // Больше четырёх: лишнее старое облачко быстро тает и уходит.
    assert_eq!(bubbles.active.len(), SPEECH_BUBBLE_CAP);
    assert_eq!(bubbles.active[0].content, "line 2");
    // Всё уходит по времени; ушедшая сущность — сразу.
    bubbles.update(BUBBLE_LIFETIME, |_| true);
    assert!(bubbles.active.is_empty());
    bubbles.enqueue(
        speaker,
        speech("again", ChatChannel::LOCAL),
        SpeechType::Say,
    );
    bubbles.update(1.0, |_| true);
    assert_eq!(bubbles.active.len(), 1);
    bubbles.update(0.0, |_| false);
    assert!(bubbles.active.is_empty());
}
