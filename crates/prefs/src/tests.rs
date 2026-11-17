use gameplay::character::{Profile, Rules, Slots};

use super::*;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const OTHER: &str = "fedcba9876543210fedcba9876543210";

fn slots(rules: &Rules) -> Slots {
    let mut s = Slots::new(rules);
    let mut second = Profile::random(5, rules);
    second.name = "Анна Иванова".into();
    assert!(s.set(1, &second, rules));
    s.selected = 1;
    s
}

#[test]
fn slots_survive_a_round_trip_and_users_are_separate() {
    let rules = Rules::default();
    let mut db = PrefsDb::in_memory().unwrap();
    assert_eq!(db.load_slots(TOKEN).unwrap(), None, "новый игрок");
    let mine = slots(&rules);
    db.save_slots(TOKEN, &mine).unwrap();
    assert_eq!(db.load_slots(TOKEN).unwrap(), Some(mine.clone()));
    assert_eq!(db.load_slots(OTHER).unwrap(), None, "чужие слоты не видны");
    // Повторная запись заменяет, а не дублирует.
    let mut fewer = mine;
    fewer.delete(0);
    db.save_slots(TOKEN, &fewer).unwrap();
    assert_eq!(db.load_slots(TOKEN).unwrap(), Some(fewer));
    db.delete_user(TOKEN).unwrap();
    assert_eq!(db.load_slots(TOKEN).unwrap(), None);
}

#[test]
fn only_a_hash_of_the_token_is_stored() {
    let rules = Rules::default();
    let mut db = PrefsDb::in_memory().unwrap();
    db.save_slots(TOKEN, &Slots::new(&rules)).unwrap();
    let stored: String = db
        .conn
        .query_row("SELECT user FROM preference", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored.len(), 64);
    assert!(!stored.contains(TOKEN), "сырого токена в базе нет");
    assert_eq!(stored, user_key(TOKEN));
}

#[test]
fn tokens_are_validated() {
    assert!(token_ok(TOKEN));
    assert!(!token_ok("short"));
    assert!(!token_ok(&"g".repeat(32)), "не шестнадцатеричные знаки");
    assert!(!token_ok(&"a".repeat(65)));
    assert!(!token_ok("'; DROP TABLE profile;--aaaaaaaaaaaa"));
}

#[test]
fn a_file_database_is_created_migrated_and_reopened() {
    let dir = std::env::temp_dir().join(format!("prefs_test_{}", std::process::id()));
    let path = dir.join("nested").join("prefs.sqlite3");
    let rules = Rules::default();
    {
        let mut db = PrefsDb::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        db.save_slots(TOKEN, &slots(&rules)).unwrap();
    }
    let db = PrefsDb::open(&path).unwrap();
    assert_eq!(db.load_slots(TOKEN).unwrap().unwrap().profiles.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_database_from_the_future_is_refused() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
        .unwrap();
    assert!(matches!(
        PrefsDb::from_connection(conn),
        Err(PrefsError::TooNew(_, _))
    ));
}

#[test]
fn broken_rows_are_skipped_instead_of_failing_the_load() {
    let rules = Rules::default();
    let mut db = PrefsDb::in_memory().unwrap();
    db.save_slots(TOKEN, &slots(&rules)).unwrap();
    let user = user_key(TOKEN);
    db.conn
        .execute(
            "UPDATE profile SET data = 'not json' WHERE user = ?1 AND slot = 0",
            params![user],
        )
        .unwrap();
    db.conn
        .execute(
            "UPDATE profile SET format = 99 WHERE user = ?1 AND slot = 1",
            params![user],
        )
        .unwrap();
    assert_eq!(db.load_slots(TOKEN).unwrap(), None, "читаемых строк нет");
    db.save_slots(TOKEN, &Slots::new(&rules)).unwrap();
    assert!(
        db.load_slots(TOKEN).unwrap().is_some(),
        "после записи снова читается"
    );
}

#[test]
fn the_store_trait_loads_and_saves() {
    let rules = Rules::default();
    let mut db = PrefsDb::in_memory().unwrap();
    let store: &mut dyn ProfileStore = &mut db;
    assert!(store.load(TOKEN).is_none());
    store.save(TOKEN, &slots(&rules));
    assert_eq!(store.load(TOKEN).unwrap().profiles.len(), 2);
}
