use super::*;
use crate::admin_manager::AdminData;
use crate::admin_system::PlayerInfo;

fn player(n: u8, name: &str) -> PlayerInfo {
    PlayerInfo {
        username: name.into(),
        character_name: String::new(),
        identity_name: String::new(),
        starting_job: String::new(),
        antag: false,
        net_entity: None,
        user_id: NetUserId(uuid::Uuid::from_bytes([n; 16])),
        connected: true,
        active_this_round: false,
    }
}

fn admin_world(active: bool) -> GameWorld {
    let mut world = GameWorld::new();
    let me = player(1, "Admin");
    world.insert_resource(ClientAdmin {
        admin: Some(AdminData {
            flags: crate::admin_manager::flags::ADMIN,
            title: None,
            active,
            stealth: false,
        }),
        available_commands: vec!["tpto".into(), "aghost".into()],
        user_id: Some(me.user_id),
        version: 0,
    });
    let mut list = ClientPlayerList::default();
    list.players.insert(me.user_id, me);
    let other = player(2, "Urist");
    list.players.insert(other.user_id, other);
    world.insert_resource(list);
    world
}

#[test]
fn only_active_admins_open_the_menu() {
    let mut world = admin_world(false);
    toggle_menu(&mut world);
    assert!(world.get_resource::<AdminMenu>().is_none_or(|m| !m.open));
    let mut world = admin_world(true);
    toggle_menu(&mut world);
    assert!(world.resource::<AdminMenu>().open);
    assert!(can_command(&world, "aghost"));
    assert!(!can_command(&world, "kick"));
}

#[test]
fn player_actions_become_console_commands() {
    let mut world = admin_world(true);
    let target = player(2, "Urist");
    assert_eq!(
        player_action(&world, PlayerAction::TeleportTo, &target).as_deref(),
        Some("tpto Urist")
    );
    assert_eq!(
        player_action(&world, PlayerAction::TeleportHere, &target).as_deref(),
        Some("tpto Admin Urist")
    );
    assert_eq!(
        player_action(&world, PlayerAction::Kick, &target).as_deref(),
        Some("kick Urist")
    );
    // Ahelp открывает переписку с игроком.
    run_player_action(&mut world, PlayerAction::Ahelp, target.user_id);
    let window = world.resource::<AhelpWindow>();
    assert!(window.open);
    assert_eq!(window.selected, Some(target.user_id));
}

#[test]
fn log_times_are_clock_times() {
    assert_eq!(log_time(0), "00:00:00");
    assert_eq!(log_time((13 * 3600 + 5 * 60 + 9) * 1000 + 999), "13:05:09");
    assert_eq!(log_time(86_400_000 + 61_000), "00:01:01");
}
