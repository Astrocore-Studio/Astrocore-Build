use astrocore_ecs::EventWorldExt;
use astrocore_net::{
    ConnId, NetEventWorldExt, NetTarget, PlayerConnected, PlayerDisconnected, ServerMessage,
    ServerNet, Session,
};

use super::*;
use crate::mind::tests::{PROTOTYPES, scene};

/// Номер события ahelp в протоколе: отправка пробного сообщения служебной сессии.
fn event_id(world: &mut World) -> u16 {
    let probe = world
        .spawn(Session {
            conn: ConnId(999),
            user_id: SYSTEM_USER,
            name: String::new(),
            attached: None,
            last_input: 0,
            sync: Default::default(),
        })
        .id();
    world.send_net_event(NetTarget::Session(probe), &message(SYSTEM_USER, ""));
    world.despawn(probe);
    let outgoing = std::mem::take(&mut world.resource_mut::<ServerNet>().outgoing);
    let packet = outgoing.iter().find(|o| o.conn == ConnId(999)).unwrap();
    match postcard::from_bytes::<ServerMessage>(&packet.bytes) {
        Ok(ServerMessage::Event { id, .. }) => id,
        _ => unreachable!(),
    }
}

/// Сообщения ahelp, ушедшие с последнего вызова: разбор отправленных пакетов.
fn drain(world: &mut World) -> Vec<(ConnId, BwoinkTextMessage)> {
    let outgoing = std::mem::take(&mut world.resource_mut::<ServerNet>().outgoing);
    let id = event_id(world);
    outgoing
        .iter()
        .filter_map(|o| match postcard::from_bytes::<ServerMessage>(&o.bytes) {
            Ok(ServerMessage::Event { id: got, bytes }) if got == id => {
                Some((o.conn, postcard::from_bytes(&bytes).unwrap()))
            }
            _ => None,
        })
        .collect()
}

fn received(world: &mut World, session: Entity) -> Vec<BwoinkTextMessage> {
    let conn = self::conn(world, session);
    drain(world)
        .into_iter()
        .filter(|(c, _)| *c == conn)
        .map(|(_, m)| m)
        .collect()
}

fn conn(world: &World, session: Entity) -> ConnId {
    world.get::<Session>(session).unwrap().conn
}

fn message(channel: NetUserId, text: &str) -> BwoinkTextMessage {
    BwoinkTextMessage {
        user_id: channel,
        true_sender: SYSTEM_USER,
        text: text.into(),
        play_sound: true,
        admin_only: false,
        channel_name: String::new(),
    }
}

#[test]
fn player_messages_reach_admins_and_the_player() {
    let mut scene = scene(PROTOTYPES);
    let (admin, admin_user) = scene.session(1);
    crate::admin_manager::promote_host(&mut scene.world, admin);
    let (player, user) = scene.session(2);
    let (other, _) = scene.session(3);
    scene.world.resource_mut::<ServerNet>().outgoing.clear();

    receive_message(
        &mut scene.world,
        player,
        &message(user, "help [bold]me[/bold]"),
    );
    let to_admin = received(&mut scene.world, admin);
    let sent: Vec<&BwoinkTextMessage> = to_admin.iter().filter(|m| m.user_id == user).collect();
    assert_eq!(sent.len(), 1);
    // Отправитель — сервер подставляет настоящего; разметка игрока экранирована.
    assert_eq!(sent[0].true_sender, user);
    assert!(
        sent[0].text.ends_with("player2: help \\[bold]me\\[/bold]"),
        "{}",
        sent[0].text
    );
    assert_eq!(sent[0].channel_name, "player2");
    assert!(sent[0].play_sound);

    // Ответ администратора — игроку, имя администратора красным; посторонний не видит.
    receive_message(&mut scene.world, admin, &message(user, "on my way"));
    let sent = drain(&mut scene.world);
    let to_player: Vec<_> = sent
        .iter()
        .filter(|(c, _)| *c == conn(&scene.world, player))
        .collect();
    assert_eq!(to_player.len(), 1);
    assert_eq!(to_player[0].1.true_sender, admin_user);
    assert!(
        to_player[0]
            .1
            .text
            .ends_with("[color=red]player1[/color]: on my way"),
        "{}",
        to_player[0].1.text
    );
    assert!(!sent.iter().any(|(c, _)| *c == conn(&scene.world, other)));
}

#[test]
fn players_cannot_write_to_other_channels_or_admins_only() {
    let mut scene = scene(PROTOTYPES);
    let (admin, _) = scene.session(1);
    crate::admin_manager::promote_host(&mut scene.world, admin);
    let (player, user) = scene.session(2);
    let (_, other_user) = scene.session(3);
    scene.world.resource_mut::<ServerNet>().outgoing.clear();
    receive_message(&mut scene.world, player, &message(other_user, "spoof"));
    let mut hidden = message(user, "secret");
    hidden.admin_only = true;
    receive_message(&mut scene.world, player, &hidden);
    assert!(received(&mut scene.world, admin).is_empty());

    // Администратор пишет «только администраторам»: игрок не получает, звука нет.
    receive_message(&mut scene.world, admin, &hidden);
    let to_admin = received(&mut scene.world, admin);
    assert_eq!(to_admin.len(), 1);
    assert!(to_admin[0].admin_only);
    assert!(!to_admin[0].play_sound);
    assert!(to_admin[0].text.starts_with("bwoink-message-admin-only"));
    receive_message(&mut scene.world, admin, &hidden);
    assert!(received(&mut scene.world, player).is_empty());
}

#[test]
fn nobody_to_answer_and_rate_limit_are_reported() {
    let mut scene = scene(PROTOTYPES);
    let (player, user) = scene.session(2);
    scene.world.resource_mut::<ServerNet>().outgoing.clear();
    receive_message(&mut scene.world, player, &message(user, "anyone?"));
    let got = received(&mut scene.world, player);
    assert!(got.iter().any(|m| m.true_sender == SYSTEM_USER
        && m.text == "bwoink-system-starmute-message-no-other-users"));
    // Ещё девять — проходят, одиннадцатое за две секунды — отказ.
    for _ in 0..9 {
        receive_message(&mut scene.world, player, &message(user, "spam"));
    }
    received(&mut scene.world, player);
    receive_message(&mut scene.world, player, &message(user, "spam"));
    let got = received(&mut scene.world, player);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].text, "bwoink-system-rate-limited");
}

#[test]
fn admins_hear_when_a_recent_correspondent_leaves() {
    let mut scene = scene(PROTOTYPES);
    let (admin, _) = scene.session(1);
    crate::admin_manager::promote_host(&mut scene.world, admin);
    let (player, user) = scene.session(2);
    let (quiet, _) = scene.session(3);
    receive_message(&mut scene.world, player, &message(user, "hi"));
    scene.world.resource_mut::<ServerNet>().outgoing.clear();
    scene
        .world
        .raise_broadcast(&mut PlayerDisconnected { session: player });
    scene
        .world
        .raise_broadcast(&mut PlayerDisconnected { session: quiet });
    let got = received(&mut scene.world, admin);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].user_id, user);
    assert_eq!(
        got[0].text,
        "[color=#FFFF00]player2 bwoink-system-player-disconnecting[/color]"
    );
    scene
        .world
        .raise_broadcast(&mut PlayerConnected { session: player });
    let got = received(&mut scene.world, admin);
    assert!(
        got.iter()
            .any(|m| m.text.contains("bwoink-system-player-reconnecting"))
    );
}

#[test]
fn names_are_coloured_by_rights() {
    let helper = AdminData {
        flags: flags::ADMINHELP,
        title: None,
        active: true,
        stealth: false,
    };
    let admin = AdminData {
        flags: flags::ADMINHELP | flags::BAN,
        ..helper.clone()
    };
    assert_eq!(format_name(None, "", "Urist"), "Urist");
    assert_eq!(
        format_name(Some(&helper), "", "Urist"),
        "[color=purple]Urist[/color]"
    );
    assert_eq!(
        format_name(Some(&admin), "", "Urist"),
        "[color=red]Urist[/color]"
    );
}

#[test]
fn the_client_keeps_channels_and_unread_state() {
    let mut world = World::new();
    let me = NetUserId(uuid::Uuid::from_bytes([1; 16]));
    world.insert_resource(crate::admin_manager::ClientAdmin {
        user_id: Some(me),
        ..Default::default()
    });
    let mut own = message(me, " me: hi");
    own.true_sender = me;
    client_receive(&mut world, &own);
    assert!(!world.resource::<ClientBwoink>().unread);
    let mut reply = message(me, " admin: hello");
    reply.true_sender = NetUserId(uuid::Uuid::from_bytes([9; 16]));
    reply.play_sound = false;
    client_receive(&mut world, &reply);
    let state = world.resource::<ClientBwoink>();
    assert!(state.unread);
    assert_eq!(state.channels[&me].1, [" me: hi", " admin: hello"]);
}
