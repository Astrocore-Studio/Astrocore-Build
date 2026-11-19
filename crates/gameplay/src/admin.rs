//! Админ-права и консоль (по `AdminFlags`, `AdminManager` и `ConsoleHost` SS14).
//!
//! Права — данные: ранг (`AdminRank`: название и флаги) лежит в прототипах
//! (`content/prototypes/admin.ron`), а кто какой ранг имеет — в списке админов сервера: SHA-256
//! токена игрока (тот же токен, что называет клиент командой `user.hello`) и название ранга.
//! Токена в списке нет — игрок не админ. Флаги называются как в SS14 (`Admin`, `Spawn`, `Mapping`,
//! `Debug`, `Round`, …); команда консоли объявляет, какой флаг ей нужен, и без него сервер её не
//! выполняет. Консоль доступна всем: неадмин видит только команды без флага.
//!
//! Клиент получает два компонента: `build.admin` (ранг, флаги и команды, доступные игроку, — для
//! подсказок и дополнения) и `build.console` (последние строки ответов сервера на его команды).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Команда клиента: одна строка консоли (UTF-8).
pub const CMD_CONSOLE: &str = "console.exec";
pub const ADMIN_COMPONENT: &str = "build.admin";
pub const CONSOLE_COMPONENT: &str = "build.console";
/// Предел размера реплицируемого компонента (у движка 16 КиБ).
pub const COMPONENT_MAX_BYTES: usize = 15_000;
/// Предел длины строки консоли, символы.
pub const MAX_LINE: usize = 400;
/// Сколько строк ответов хранится у игрока.
pub const KEEP_LINES: usize = 60;
/// Не чаще одной команды за столько секунд от игрока.
pub const MIN_GAP_S: f32 = 0.05;

/// Длина имени карты для `savemap`.
pub const MAP_NAME_MAX: usize = 64;

/// Имя карты становится именем файла: только латиница, цифры, `_` и `-` (без путей и точек).
pub fn valid_map_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAP_NAME_MAX
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Флаги прав (`AdminFlags` SS14); ранг может содержать только эти названия.
pub const FLAGS: &[&str] = &[
    "Admin",
    "Spawn",
    "Fun",
    "Mapping",
    "Debug",
    "Round",
    "Moderator",
    "Ban",
    "Permissions",
    "Server",
];

/// Ранг: название для показа и набор флагов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rank {
    pub id: String,
    pub title: String,
    pub flags: BTreeSet<String>,
}

/// Список админов сервера: SHA-256 токена (шестнадцатеричный) → идентификатор ранга; `everyone` —
/// ранг для всех без исключения (только для локальной игры, флаг сервера `--admin-all`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminRoster {
    pub by_hash: BTreeMap<String, String>,
    pub everyone: Option<String>,
}

/// SHA-256 токена, шестнадцатеричный.
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

impl AdminRoster {
    /// Разобрать файл списка: RON-список пар `("sha256 токена", "ранг")`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let pairs: Vec<(String, String)> = ron::from_str(text).map_err(|e| e.to_string())?;
        let mut by_hash = BTreeMap::new();
        for (hash, rank) in pairs {
            let hash = hash.trim().to_ascii_lowercase();
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!(
                    "«{hash}»: нужен SHA-256 в 64 шестнадцатеричных знака"
                ));
            }
            by_hash.insert(hash, rank);
        }
        Ok(Self {
            by_hash,
            everyone: None,
        })
    }

    /// Ранг игрока по его токену.
    fn rank_id(&self, token: Option<&str>) -> Option<&str> {
        token
            .and_then(|t| self.by_hash.get(&hash_token(t)))
            .or(self.everyone.as_ref())
            .map(String::as_str)
    }
}

/// Ранги из прототипов и список админов.
#[derive(Debug, Clone, Default)]
pub struct Admins {
    pub ranks: BTreeMap<String, Rank>,
    pub roster: AdminRoster,
}

impl Admins {
    /// Ранги из прототипов с компонентом `AdminRank`: `title` и `flags`.
    pub fn from_registry(registry: &prototypes::PrototypeRegistry) -> Result<Self, String> {
        let mut ranks = BTreeMap::new();
        for name in registry.ids().map(str::to_string).collect::<Vec<_>>() {
            let Ok(r) = registry.spawnable(&name) else {
                continue;
            };
            let Some(c) = r.component("AdminRank") else {
                continue;
            };
            let flags: BTreeSet<String> = c
                .get("flags")
                .and_then(prototypes::Data::as_list)
                .unwrap_or_default()
                .iter()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect();
            if let Some(bad) = flags.iter().find(|f| !FLAGS.contains(&f.as_str())) {
                return Err(format!("ранг {name}: неизвестный флаг «{bad}»"));
            }
            let title = c
                .get("title")
                .and_then(prototypes::Data::as_str)
                .unwrap_or(&name)
                .to_string();
            ranks.insert(
                name.clone(),
                Rank {
                    id: name,
                    title,
                    flags,
                },
            );
        }
        Ok(Self {
            ranks,
            roster: AdminRoster::default(),
        })
    }

    /// Ранг игрока по его токену; нет токена или он не в списке — `None`.
    pub fn rank_of(&self, token: Option<&str>) -> Option<&Rank> {
        self.ranks.get(self.roster.rank_id(token)?)
    }

    /// Есть ли у игрока флаг; `None` — флага не требуется, разрешено всем.
    pub fn allows(&self, token: Option<&str>, flag: Option<&str>) -> bool {
        match flag {
            None => true,
            Some(f) => self.rank_of(token).is_some_and(|r| r.flags.contains(f)),
        }
    }
}

/// Описание команды консоли.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub name: &'static str,
    pub usage: &'static str,
    pub help: &'static str,
    /// Нужный флаг; `None` — команда для всех.
    pub flag: Option<&'static str>,
}

/// Команды консоли (имена как в SS14, где такая команда есть).
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "help",
        usage: "help [команда]",
        help: "Список команд или описание одной",
        flag: None,
    },
    CommandSpec {
        name: "whoami",
        usage: "whoami",
        help: "Ваш ранг и флаги",
        flag: None,
    },
    CommandSpec {
        name: "list",
        usage: "list",
        help: "Игроки: номер, должность, состояние, имя",
        flag: None,
    },
    CommandSpec {
        name: "aghost",
        usage: "aghost",
        help: "Стать админ-призраком или вернуться в тело",
        flag: Some("Admin"),
    },
    CommandSpec {
        name: "tp",
        usage: "tp <x> <z> | tp <номер игрока>",
        help: "Переместиться в точку или к игроку",
        flag: Some("Admin"),
    },
    CommandSpec {
        name: "spawn",
        usage: "spawn <прототип> [число]",
        help: "Создать предмет у ног",
        flag: Some("Spawn"),
    },
    CommandSpec {
        name: "savemap",
        usage: "savemap <имя>",
        help: "Сохранить карту: стены, полы, предметы на земле и зоны (файл <имя>.ron)",
        flag: Some("Mapping"),
    },
    CommandSpec {
        name: "loadmap",
        usage: "loadmap <имя>",
        help: "Новый раунд на сохранённой карте <имя>.ron (сразу, без сводки)",
        flag: Some("Mapping"),
    },
    CommandSpec {
        name: "mapping",
        usage: "mapping",
        help: "Новый раунд на пустой карте для маппинга (сразу, без сводки)",
        flag: Some("Mapping"),
    },
    CommandSpec {
        name: "endround",
        usage: "endround",
        help: "Закончить раунд и начать следующий",
        flag: Some("Round"),
    },
];

/// Что клиент знает о правах: для подсказок и дополнения. Сервер всё равно проверяет сам.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminView {
    pub rank: Option<String>,
    pub flags: Vec<String>,
    pub commands: Vec<CommandInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInfo {
    pub name: String,
    pub usage: String,
    pub help: String,
}

/// Строка ответа сервера консоли.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleLine {
    pub id: u32,
    pub error: bool,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleView {
    pub lines: Vec<ConsoleLine>,
}

pub fn encode_admin(v: &AdminView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_admin(bytes: &[u8]) -> Option<AdminView> {
    if bytes.is_empty() || bytes.len() > COMPONENT_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

pub fn encode_console(v: &ConsoleView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_console(bytes: &[u8]) -> Option<ConsoleView> {
    if bytes.is_empty() || bytes.len() > COMPONENT_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Что игрок видит в консоли: ранг и доступные ему команды.
pub fn view_for(admins: &Admins, token: Option<&str>) -> AdminView {
    let rank = admins.rank_of(token);
    AdminView {
        rank: rank.map(|r| r.title.clone()),
        flags: rank
            .map(|r| r.flags.iter().cloned().collect())
            .unwrap_or_default(),
        commands: COMMANDS
            .iter()
            .filter(|c| admins.allows(token, c.flag))
            .map(|c| CommandInfo {
                name: c.name.into(),
                usage: c.usage.into(),
                help: c.help.into(),
            })
            .collect(),
    }
}

/// Строка консоли: без управляющих символов, обрезана по длине, пустая — `None`.
pub fn clean(line: &str) -> Option<String> {
    let t: String = line
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LINE)
        .collect();
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Разбить строку на слова; слова в двойных кавычках могут содержать пробелы.
pub fn tokenize(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Описание команды по имени.
pub fn spec(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.name == name)
}

/// Состояние консоли на сервере.
#[derive(Debug, Default)]
pub(crate) struct AdminState {
    pub next_id: u32,
    pub log: BTreeMap<astrocore_core::NetworkEntityId, std::collections::VecDeque<ConsoleLine>>,
    pub dirty: BTreeSet<astrocore_core::NetworkEntityId>,
    /// Какой вид прав последний раз отправлен игроку.
    pub sent: BTreeMap<astrocore_core::NetworkEntityId, Vec<u8>>,
    /// Когда игрок вводил команду в последний раз (часы модуля).
    pub last: BTreeMap<astrocore_core::NetworkEntityId, f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admins() -> Admins {
        let mut registry = prototypes::PrototypeRegistry::new();
        registry
            .load_ron_str(
                r#"[
                    (id: "Host", components: {"AdminRank": (title: "Хозяин", flags: ["Admin", "Spawn", "Round"])}),
                    (id: "Helper", components: {"AdminRank": (title: "Помощник", flags: ["Spawn"])}),
                ]"#,
                "admin",
            )
            .unwrap();
        registry.validate().unwrap();
        let mut a = Admins::from_registry(&registry).unwrap();
        a.roster = AdminRoster::parse(&format!(
            r#"[("{}", "Host"), ("{}", "Helper")]"#,
            hash_token("host-token"),
            hash_token("helper-token")
        ))
        .unwrap();
        a
    }

    #[test]
    fn ranks_come_from_prototypes_and_players_from_the_roster() {
        let a = admins();
        assert_eq!(a.rank_of(Some("host-token")).unwrap().title, "Хозяин");
        assert!(a.allows(Some("host-token"), Some("Round")));
        assert!(a.allows(Some("helper-token"), Some("Spawn")));
        assert!(
            !a.allows(Some("helper-token"), Some("Round")),
            "флаг не выдан"
        );
        assert!(!a.allows(Some("stranger"), Some("Spawn")), "нет в списке");
        assert!(!a.allows(None, Some("Admin")), "не назвался");
        assert!(a.allows(None, None), "команда без флага — всем");
    }

    #[test]
    fn everyone_rank_makes_every_player_an_admin() {
        let mut a = admins();
        a.roster.everyone = Some("Host".into());
        assert!(a.allows(Some("anyone"), Some("Spawn")));
        assert!(a.allows(None, Some("Admin")));
    }

    #[test]
    fn an_unknown_flag_in_a_rank_is_refused() {
        let mut registry = prototypes::PrototypeRegistry::new();
        registry
            .load_ron_str(
                r#"[(id: "Bad", components: {"AdminRank": (title: "x", flags: ["Everything"])})]"#,
                "admin",
            )
            .unwrap();
        registry.validate().unwrap();
        assert!(Admins::from_registry(&registry).is_err());
    }

    #[test]
    fn the_roster_needs_real_sha256_hashes() {
        assert!(AdminRoster::parse(r#"[("abc", "Host")]"#).is_err());
        assert!(AdminRoster::parse("[]").unwrap().by_hash.is_empty());
        assert_eq!(hash_token("a").len(), 64);
    }

    #[test]
    fn a_player_sees_only_the_commands_their_rank_allows() {
        let a = admins();
        let names = |t: Option<&str>| -> Vec<String> {
            view_for(&a, t)
                .commands
                .into_iter()
                .map(|c| c.name)
                .collect()
        };
        let guest = names(None);
        assert!(guest.contains(&"help".into()) && !guest.contains(&"spawn".into()));
        let helper = names(Some("helper-token"));
        assert!(helper.contains(&"spawn".into()) && !helper.contains(&"endround".into()));
        assert!(names(Some("host-token")).contains(&"endround".into()));
        assert_eq!(
            view_for(&a, Some("host-token")).rank.as_deref(),
            Some("Хозяин")
        );
    }

    #[test]
    fn lines_split_into_words_with_quotes() {
        assert_eq!(tokenize("spawn  Crowbar 3"), ["spawn", "Crowbar", "3"]);
        assert_eq!(tokenize(r#"say "two words" x"#), ["say", "two words", "x"]);
        assert!(tokenize("   ").is_empty());
        assert_eq!(clean("  ab\u{7}c  ").as_deref(), Some("abc"));
        assert!(clean(" \t").is_none());
    }
}
