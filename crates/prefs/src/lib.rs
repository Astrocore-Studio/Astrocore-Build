//! Настройки игроков в SQLite (`PreferencesManager` и `Content.Server.Database` SS14).
//!
//! База хранит профили персонажей по слотам: таблицы `preference` (выбранный слот игрока) и
//! `profile` (слот, имя, вид и весь профиль JSON с версией схемы). Игрока определяет токен, который
//! клиент хранит у себя, а в базе лежит только его SHA-256: утечка файла базы токены не раскрывает.
//! Версия схемы — `PRAGMA user_version`; миграции идут по порядку и применяются при открытии.
//!
//! Не перенесено: аккаунты и авторизация, предпочтения антагонистов и черт (отдельные таблицы
//! SS14), резервные копии.

use std::path::Path;

use gameplay::character::{Profile, ProfileStore, Slots};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

/// Версия схемы, которую понимает этот код.
pub const SCHEMA_VERSION: i64 = 1;
/// Версия формата профиля внутри столбца `data`.
pub const PROFILE_FORMAT: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum PrefsError {
    #[error("база: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("формат профиля: {0}")]
    Json(#[from] serde_json::Error),
    #[error("база новее программы: схема {0}, поддерживается {1}")]
    TooNew(i64, i64),
}

/// Хранилище на SQLite.
pub struct PrefsDb {
    conn: Connection,
}

/// Ключ игрока в базе: SHA-256 токена в шестнадцатеричном виде.
pub fn user_key(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub use gameplay::character::token_ok;

fn migrate(conn: &Connection) -> Result<(), PrefsError> {
    let have: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if have > SCHEMA_VERSION {
        return Err(PrefsError::TooNew(have, SCHEMA_VERSION));
    }
    if have < 1 {
        conn.execute_batch(
            "CREATE TABLE preference (
                 user TEXT PRIMARY KEY NOT NULL,
                 selected_slot INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE profile (
                 user TEXT NOT NULL,
                 slot INTEGER NOT NULL,
                 char_name TEXT NOT NULL,
                 species TEXT NOT NULL,
                 format INTEGER NOT NULL,
                 data TEXT NOT NULL,
                 PRIMARY KEY (user, slot)
             );
             PRAGMA user_version = 1;",
        )?;
    }
    Ok(())
}

impl PrefsDb {
    /// Открыть (или создать) базу по пути и привести схему к текущей.
    pub fn open(path: &Path) -> Result<Self, PrefsError> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(dir);
        }
        Self::from_connection(Connection::open(path)?)
    }

    /// База в памяти: для тестов.
    pub fn in_memory() -> Result<Self, PrefsError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, PrefsError> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .or_else(|_| conn.execute_batch("PRAGMA foreign_keys = ON;"))?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    pub fn schema_version(&self) -> Result<i64, PrefsError> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    /// Слоты игрока; `None`, если его в базе нет. Строки, которые не читаются (повреждены или
    /// другого формата), пропускаются, а не роняют загрузку.
    pub fn load_slots(&self, token: &str) -> Result<Option<Slots>, PrefsError> {
        let user = user_key(token);
        let selected: Option<i64> = self
            .conn
            .query_row(
                "SELECT selected_slot FROM preference WHERE user = ?1",
                params![user],
                |r| r.get(0),
            )
            .optional()?;
        let Some(selected) = selected else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare("SELECT slot, format, data FROM profile WHERE user = ?1 ORDER BY slot")?;
        let rows = stmt.query_map(params![user], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut profiles: Vec<Profile> = Vec::new();
        let mut chosen = 0usize;
        for row in rows {
            let (slot, format, data) = row?;
            if format != PROFILE_FORMAT {
                continue;
            }
            let Ok(p) = serde_json::from_str::<Profile>(&data) else {
                continue;
            };
            if slot == selected {
                chosen = profiles.len();
            }
            profiles.push(p);
        }
        if profiles.is_empty() {
            return Ok(None);
        }
        Ok(Some(Slots {
            selected: chosen.min(profiles.len() - 1),
            profiles,
        }))
    }

    /// Записать все слоты игрока (одной транзакцией): прежние строки заменяются.
    pub fn save_slots(&mut self, token: &str, slots: &Slots) -> Result<(), PrefsError> {
        let user = user_key(token);
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM profile WHERE user = ?1", params![user])?;
        for (i, p) in slots.profiles.iter().enumerate() {
            tx.execute(
                "INSERT INTO profile (user, slot, char_name, species, format, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    user,
                    i as i64,
                    p.name,
                    p.species,
                    PROFILE_FORMAT,
                    serde_json::to_string(p)?
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO preference (user, selected_slot) VALUES (?1, ?2)
             ON CONFLICT(user) DO UPDATE SET selected_slot = excluded.selected_slot",
            params![user, slots.selected as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Удалить все данные игрока.
    pub fn delete_user(&mut self, token: &str) -> Result<(), PrefsError> {
        let user = user_key(token);
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM profile WHERE user = ?1", params![user])?;
        tx.execute("DELETE FROM preference WHERE user = ?1", params![user])?;
        tx.commit()?;
        Ok(())
    }
}

impl ProfileStore for PrefsDb {
    fn load(&mut self, token: &str) -> Option<Slots> {
        match self.load_slots(token) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("настройки: не прочитать профили: {e}");
                None
            }
        }
    }

    fn save(&mut self, token: &str, slots: &Slots) {
        if let Err(e) = self.save_slots(token, slots) {
            eprintln!("настройки: не записать профили: {e}");
        }
    }
}

#[cfg(test)]
mod tests;
