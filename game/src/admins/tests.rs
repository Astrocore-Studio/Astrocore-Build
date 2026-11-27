use super::*;
use crate::database::Database;

#[test]
fn admins_ranks_and_flags_round_trip() {
    let database = Database::in_memory().unwrap();
    let db = database.db().clone();
    database.block_on(async {
        let rank = db
            .add_admin_rank("Game Admin", &["ADMIN", "BAN", "SPAWN"])
            .await
            .unwrap();
        let rank_record = db.get_admin_rank(rank).await.unwrap().unwrap();
        assert_eq!(rank_record.name, "Game Admin");
        let user = NetUserId::random();
        db.update_player_record(user, "Admin", "127.0.0.1")
            .await
            .unwrap();
        let admin = AdminRecord {
            user_id: user,
            title: Some("Старший".into()),
            deadminned: false,
            suspended: false,
            rank: Some(rank_record.clone()),
            flags: vec![("FUN".into(), false), ("SPAWN".into(), true)],
        };
        db.set_admin(&admin).await.unwrap();
        let stored = db.get_admin_data_for(user).await.unwrap().unwrap();
        assert_eq!(stored, admin);
        // Права: ранг плюс свои, минус отнятые.
        assert_eq!(
            stored.effective_flags().into_iter().collect::<Vec<_>>(),
            ["ADMIN", "BAN", "FUN"]
        );
        // Замена флагов и отстранение.
        let changed = AdminRecord {
            suspended: true,
            flags: vec![("HOST".into(), false)],
            ..admin.clone()
        };
        db.set_admin(&changed).await.unwrap();
        assert_eq!(db.get_admin_data_for(user).await.unwrap(), Some(changed));
        let (admins, ranks) = db.get_all_admins_and_ranks().await.unwrap();
        assert_eq!(admins.len(), 1);
        assert_eq!(admins[0].1.as_deref(), Some("Admin"));
        assert_eq!(ranks, [rank_record]);
        // Удалённый ранг оставляет администратора без ранга.
        db.remove_admin_rank(rank).await.unwrap();
        assert_eq!(
            db.get_admin_data_for(user).await.unwrap().unwrap().rank,
            None
        );
        db.remove_admin(user).await.unwrap();
        assert_eq!(db.get_admin_data_for(user).await.unwrap(), None);
    });
}
