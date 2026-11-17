//! Токен игрока: случайная строка из 32 шестнадцатеричных знаков, которую клиент хранит у себя и
//! называет серверу (`user.hello`). По нему сервер находит сохранённые профили персонажей; в базе
//! лежит только его хеш. Потерял файл — потерял персонажей, как потерянный пароль.
//!
//! Где лежит: `ASTRO_USER_TOKEN` (значение прямо, для проверок и нескольких игроков на одном
//! компьютере), иначе файл `astrocore-build/user_token` в каталоге настроек пользователя
//! (`XDG_CONFIG_HOME`, `~/.config` или `%APPDATA%`).

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;

/// Случайный токен: 128 бит из двух независимых случайно заполненных хешеров ОС.
fn generate() -> String {
    let word = |salt: u64| {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(salt);
        h.finish()
    };
    format!("{:016x}{:016x}", word(1), word(2))
}

fn token_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("astrocore-build").join("user_token"))
}

/// Токен игрока; создаётся и сохраняется при первом запуске. Если сохранить негде, токен живёт
/// до конца запуска.
pub fn load_or_create() -> String {
    if let Some(t) = std::env::var("ASTRO_USER_TOKEN")
        .ok()
        .filter(|t| gameplay::character::token_ok(t))
    {
        return t;
    }
    let path = token_path();
    if let Some(p) = &path
        && let Ok(text) = std::fs::read_to_string(p)
        && gameplay::character::token_ok(text.trim())
    {
        return text.trim().to_string();
    }
    let token = generate();
    if let Some(p) = &path {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, &token);
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_valid_and_different() {
        let (a, b) = (generate(), generate());
        assert!(gameplay::character::token_ok(&a));
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
