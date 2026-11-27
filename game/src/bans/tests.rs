use super::*;
use crate::database::Database;

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

#[test]
fn ip_ranges_match_subnets() {
    let net = IpRange::parse("10.1.0.0/16").unwrap();
    assert!(net.contains(ip("10.1.200.7")));
    assert!(!net.contains(ip("10.2.0.1")));
    // IPv4 внутри IPv6 сверяется как IPv4.
    assert!(net.contains(ip("::ffff:10.1.2.3")));
    assert!(!net.contains(ip("2001:db8::1")));
    let single = IpRange::parse("192.168.1.5").unwrap();
    assert_eq!(single.to_string(), "192.168.1.5/32");
    assert!(single.contains(ip("192.168.1.5")));
    assert!(!single.contains(ip("192.168.1.6")));
    assert!(IpRange::parse("0.0.0.0/0").unwrap().contains(ip("8.8.8.8")));
    let v6 = IpRange::parse("2001:db8::/32").unwrap();
    assert!(v6.contains(ip("2001:db8:1::5")));
    assert!(!v6.contains(ip("2001:db9::5")));
    assert_eq!(IpRange::parse("10.0.0.0/33"), None);
    assert_eq!(IpRange::parse("nonsense"), None);
}

#[test]
fn ban_matcher_follows_ss14_rules() {
    let user = NetUserId::random();
    let player = PlayerInfo {
        user_id: Some(user),
        address: Some(ip("10.1.2.3")),
        exempt_flags: exempt::NONE,
        is_new_player: false,
    };
    let mut by_user = BanDef::server("гриф", None);
    by_user.user_ids.push(user);
    assert!(ban_matches(&by_user, &player));
    let mut by_address = BanDef::server("гриф", None);
    by_address
        .addresses
        .push(IpRange::parse("10.1.0.0/16").unwrap());
    assert!(ban_matches(&by_address, &player));
    // Исключение по адресу снимает только бан по адресу.
    let ip_exempt = PlayerInfo {
        exempt_flags: exempt::IP,
        ..player
    };
    assert!(!ban_matches(&by_address, &ip_exempt));
    assert!(ban_matches(&by_user, &ip_exempt));
    // Бан диапазона из чёрного списка касается только новых игроков и снимается любым
    // исключением.
    let mut blacklisted = by_address.clone();
    blacklisted.exempt_flags = exempt::BLACKLISTED_RANGE;
    assert!(!ban_matches(&blacklisted, &player));
    let newcomer = PlayerInfo {
        is_new_player: true,
        ..player
    };
    assert!(ban_matches(&blacklisted, &newcomer));
    let datacenter_exempt = PlayerInfo {
        exempt_flags: exempt::DATACENTER,
        ..newcomer
    };
    assert!(!ban_matches(&blacklisted, &datacenter_exempt));
    // Чужой игрок с другого адреса не попадает.
    let stranger = PlayerInfo {
        user_id: Some(NetUserId::random()),
        address: Some(ip("8.8.8.8")),
        ..player
    };
    assert!(!ban_matches(&by_user, &stranger) && !ban_matches(&by_address, &stranger));
}

#[test]
fn bans_are_stored_found_expired_and_lifted() {
    let database = Database::in_memory().unwrap();
    let db = database.db().clone();
    database.block_on(async {
        let user = NetUserId::random();
        let admin = NetUserId::random();
        let server = db.server_id().await.unwrap();
        let round = db.add_new_round(server, &[]).await.unwrap();

        let mut permanent = BanDef::server("гриф", None);
        permanent.user_ids.push(user);
        permanent
            .addresses
            .push(IpRange::parse("10.1.0.0/16").unwrap());
        permanent.round_ids.push(round);
        permanent.banning_admin = Some(admin);
        permanent.severity = NoteSeverity::High;
        let permanent = db.add_ban(&permanent).await.unwrap();
        let id = permanent.id.unwrap();
        assert_eq!(db.get_ban(id).await.unwrap(), Some(permanent.clone()));

        let mut expired = BanDef::server("старое", Some(now_ms() - 1000));
        expired.user_ids.push(user);
        db.add_ban(&expired).await.unwrap();
        let mut role = BanDef::server("не врач", None);
        role.ban_type = BanType::Role;
        role.user_ids.push(user);
        role.roles.push(("Job".into(), "MedicalDoctor".into()));
        db.add_ban(&role).await.unwrap();

        // Действующий бан на сервер находится и по идентификатору, и по адресу.
        let found = db
            .get_bans(Some(ip("10.1.9.9")), Some(user), false, BanType::Server)
            .await
            .unwrap();
        assert_eq!(found.iter().map(|b| b.id).collect::<Vec<_>>(), [Some(id)]);
        let by_address = db
            .get_bans(
                Some(ip("10.1.9.9")),
                Some(NetUserId::random()),
                false,
                BanType::Server,
            )
            .await
            .unwrap();
        assert_eq!(by_address.len(), 1);
        let roles = db
            .get_bans(None, Some(user), false, BanType::Role)
            .await
            .unwrap();
        assert_eq!(
            roles[0].roles,
            [("Job".to_owned(), "MedicalDoctor".to_owned())]
        );
        // Со снятыми и истёкшими — оба бана на сервер.
        let all = db
            .get_bans(None, Some(user), true, BanType::Server)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);

        // Исключение по адресу снимает бан по адресу у другого игрока.
        let other = NetUserId::random();
        db.update_ban_exemption(other, exempt::IP).await.unwrap();
        assert_eq!(db.get_ban_exemption(other).await.unwrap(), exempt::IP);
        assert!(
            db.get_bans(Some(ip("10.1.9.9")), Some(other), false, BanType::Server)
                .await
                .unwrap()
                .is_empty()
        );
        db.update_ban_exemption(other, exempt::NONE).await.unwrap();
        assert_eq!(db.get_ban_exemption(other).await.unwrap(), exempt::NONE);

        // Снятый бан больше не действует.
        db.add_unban(&UnbanDef {
            ban_id: id,
            unbanning_admin: Some(admin),
            unban_time: now_ms(),
        })
        .await
        .unwrap();
        assert!(
            db.get_bans(Some(ip("10.1.9.9")), Some(user), false, BanType::Server)
                .await
                .unwrap()
                .is_empty()
        );
        let lifted = db.get_ban(id).await.unwrap().unwrap();
        assert_eq!(
            lifted.unban.as_ref().map(|u| u.unbanning_admin),
            Some(Some(admin))
        );
        assert!(!lifted.is_active(now_ms()));
    });
}

#[test]
fn banned_player_is_refused_and_hit_is_logged() {
    use crate::connection::{Decision, Denial, on_connecting};
    let database = Database::in_memory().unwrap();
    let db = database.db().clone();
    database.block_on(async {
        let first = on_connecting(&db, "Griefer", Some(ip("10.1.2.3")), true)
            .await
            .unwrap();
        let Decision::Admit(user) = first else {
            panic!("впервые пускают");
        };
        let mut ban = BanDef::server("гриф", None);
        ban.user_ids.push(user);
        let ban = db.add_ban(&ban).await.unwrap();
        let second = on_connecting(&db, "Griefer", Some(ip("10.1.2.3")), true)
            .await
            .unwrap();
        assert!(matches!(&second, Decision::Deny(Denial::Ban(bans)) if bans[0].id == ban.id));
        assert_eq!(db.ban_hit_count(ban.id.unwrap()).await.unwrap(), 1);
        let logs = db.connection_logs(user).await.unwrap();
        assert_eq!(
            logs[0].denied,
            Some(crate::database::ConnectionDenyReason::Ban)
        );
        // Другой игрок с того же адреса проходит: бан только по идентификатору.
        let other = on_connecting(&db, "Friend", Some(ip("10.1.2.3")), true)
            .await
            .unwrap();
        assert!(matches!(other, Decision::Admit(_)));
    });
}

#[test]
fn ban_message_picks_expiry_line() {
    let mut world = crate::testing::world_with("");
    let mut ban = BanDef::server("гриф", Some(0));
    ban.ban_time = -60 * 60_000;
    let text = format_ban_message(&world, &ban);
    assert_eq!(text.lines().count(), 4);
    assert!(text.contains("ban-expires"));
    ban.expiration_time = None;
    assert!(format_ban_message(&world, &ban).contains("ban-banned-permanent"));
    world
        .set_cvar_text("infolinks.appeal", "https://example.org/appeal")
        .unwrap();
    assert!(format_ban_message(&world, &ban).contains("ban-banned-permanent-appeal"));
}

#[test]
fn utc_times_are_formatted() {
    assert_eq!(format_utc(0), "1970-01-01 00:00");
    assert_eq!(format_utc(1_700_000_000_000), "2023-11-14 22:13");
    assert_eq!(format_utc(951_782_400_000), "2000-02-29 00:00");
}
