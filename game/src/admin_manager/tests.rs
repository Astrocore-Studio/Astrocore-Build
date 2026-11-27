use std::collections::BTreeSet;

use astrocore_ecs::EventWorldExt;
use astrocore_net::{ConnId, NetUserId, Session};

use super::*;
use crate::admins::{AdminRankRecord, AdminRecord};
use crate::chat::{ChatChannel, ChatMessageSent};
use crate::testing::world_with;

#[derive(Resource, Default)]
struct Heard(Vec<(ChatChannel, String, Vec<Entity>)>);

fn world() -> World {
    let mut world = world_with("");
    world.init_resource::<Heard>();
    world.subscribe_broadcast::<ChatMessageSent>(|world, event| {
        let entry = (
            event.message.channel,
            event.message.message.clone(),
            event.recipients.clone(),
        );
        world.resource_mut::<Heard>().0.push(entry);
    });
    world
}

fn session(world: &mut World, n: u8) -> Entity {
    world
        .spawn(Session {
            conn: ConnId(u64::from(n)),
            user_id: NetUserId(uuid::Uuid::from_bytes([n; 16])),
            name: format!("player{n}"),
            attached: None,
            last_input: 0,
            sync: Default::default(),
        })
        .id()
}

#[test]
fn flags_have_ss14_names() {
    use flags::*;
    assert_eq!(named("ban"), Some(BAN));
    assert_eq!(named("HOST"), Some(HOST));
    assert_eq!(from_names(["ADMIN", "SPAWN", "nonsense"]), ADMIN | SPAWN);
    assert_eq!(names(ROUND | FUN), ["FUN", "ROUND"]);
    assert_ne!(everything() & HOST, 0);
    assert_eq!(everything().count_ones(), 23);
}

#[test]
fn players_get_only_any_commands_until_promoted() {
    let mut world = world();
    let player = session(&mut world, 1);
    let admin = session(&mut world, 2);
    assert!(can_command(&world, player, "say"));
    assert!(can_command(&world, player, "ghost"));
    assert!(!can_command(&world, player, "startround"));
    assert!(!can_command(&world, player, "unknown-command"));
    assert!(permission(&world, &ConsoleShell::server(), "startround"));
    promote_host(&mut world, admin);
    assert!(can_command(&world, admin, "startround"));
    assert!(can_command(&world, admin, "cvar"));
    assert!(is_admin(&world, admin));
    assert_eq!(active_admins(&world), [admin]);
    // Снял права — обычный игрок; вернул — снова администратор.
    assert!(deadmin(&mut world, admin));
    assert!(!can_command(&world, admin, "startround"));
    assert!(admin_data(&world, admin, true).is_some());
    assert!(can_command(&world, admin, "readmin"));
    assert!(readmin(&mut world, admin));
    assert!(can_command(&world, admin, "startround"));
    // Не администратор не может «снять права».
    assert!(!deadmin(&mut world, player));
}

#[test]
fn ranks_and_own_flags_combine() {
    let world = world();
    let mut record = AdminRecord {
        user_id: NetUserId(uuid::Uuid::from_bytes([1; 16])),
        title: None,
        deadminned: false,
        suspended: false,
        rank: Some(AdminRankRecord {
            id: 3,
            name: "Game Admin".into(),
            flags: BTreeSet::from(["ADMIN".to_owned(), "BAN".to_owned()]),
        }),
        flags: vec![("SPAWN".into(), false), ("BAN".into(), true)],
    };
    let (data, rank) = data_from_record(&world, &record).unwrap();
    assert_eq!(data.flags, flags::ADMIN | flags::SPAWN);
    assert_eq!(data.title.as_deref(), Some("Game Admin"));
    assert_eq!(rank, Some(3));
    assert!(data.active);
    record.title = Some("Head Admin".into());
    record.deadminned = true;
    let (data, _) = data_from_record(&world, &record).unwrap();
    assert_eq!(data.title.as_deref(), Some("Head Admin"));
    assert!(!data.active);
    record.suspended = true;
    assert!(data_from_record(&world, &record).is_none());
}

#[test]
fn admins_are_told_about_logins_and_deadmins() {
    let mut world = world();
    let first = session(&mut world, 1);
    promote_host(&mut world, first);
    let second = session(&mut world, 2);
    let data = AdminData {
        flags: flags::ADMIN | flags::ADMINCHAT,
        title: None,
        active: true,
        stealth: false,
    };
    login_admin(&mut world, second, data, None, false);
    let heard = std::mem::take(&mut world.resource_mut::<Heard>().0);
    let login = heard
        .iter()
        .find(|(_, text, _)| text == "admin-manager-admin-login-message")
        .unwrap();
    assert_eq!(login.0, ChatChannel::ADMIN);
    assert_eq!(login.2.len(), 2);
    deadmin(&mut world, second);
    let heard = std::mem::take(&mut world.resource_mut::<Heard>().0);
    assert!(
        heard
            .iter()
            .any(|(_, text, to)| text == "admin-manager-self-de-admin-message" && to == &[first])
    );
}

#[test]
fn every_command_has_a_permission() {
    let world = world();
    let console = world.resource::<Console>();
    let missing: Vec<&str> = console
        .iter()
        .map(|c| c.name)
        .filter(|name| !any_commands().contains(name) && command_flags(name).is_none())
        .collect();
    assert!(missing.is_empty(), "команды без прав: {missing:?}");
}
