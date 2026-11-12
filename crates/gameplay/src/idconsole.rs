//! Консоль выдачи ID (`IdCardConsoleComponent`, `IdCardConsoleSystem`, снимок SS14 `910154a7`).
//!
//! У консоли два слота: ID с правами (privileged) и целевая карта. Права на запись берутся с ID
//! в первом слоте: менять можно только уровни, которые есть у него самого и которые консоль
//! умеет выдавать; уровни, которых консоль не знает, на карте сохраняются. Имя и должность на
//! карте обрезаются до 32 и 30 символов (`ic.name_length`, `ic.id_job_length`).
//!
//! Отличие от SS14: слоты принимают и КПК (его карта) — отдельно вынуть карту из КПК в Build нельзя.

use std::collections::BTreeSet;

use astrocore_core::ObjectId;
use serde::{Deserialize, Serialize};

pub const IDCONSOLE_COMPONENT: &str = "build.idconsole";
pub const IDCONSOLE_MAX_BYTES: usize = 8192;
pub const MAX_NAME_LEN: usize = 32;
pub const MAX_JOB_LEN: usize = 30;

/// Что видит игрок у консоли (`IdCardConsoleBoundUserInterfaceState`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdConsoleView {
    pub console: ObjectId,
    pub privileged: Option<String>,
    pub authorized: bool,
    pub target: Option<TargetView>,
    /// Уровни, которые разрешено менять (есть у ID с правами).
    pub allowed: Vec<String>,
    /// Все уровни, которые умеет выдавать консоль.
    pub levels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetView {
    pub name: String,
    pub full_name: String,
    pub job_title: String,
    pub access: Vec<String>,
}

/// Запрос записи на карту (`WriteToTargetIdMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteRequest {
    pub name: String,
    pub title: String,
    pub access: Vec<String>,
}

pub fn encode_view(v: Option<&IdConsoleView>) -> Vec<u8> {
    v.map(|v| serde_json::to_vec(v).unwrap_or_default())
        .unwrap_or_default()
}

pub fn decode_view(bytes: &[u8]) -> Option<IdConsoleView> {
    if bytes.is_empty() || bytes.len() > IDCONSOLE_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

pub fn write_data(r: &WriteRequest) -> Vec<u8> {
    serde_json::to_vec(r).unwrap_or_default()
}

pub fn parse_write(bytes: &[u8]) -> Option<WriteRequest> {
    if bytes.len() > IDCONSOLE_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Обрезать до `max` символов.
pub fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Итог `TryWriteToTargetId` для уровней доступа: новый набор или `None`, если запись отклонена.
///
/// * неизвестный консоли уровень в запросе — отказ;
/// * набор не изменился — ничего не делать (`None`);
/// * каждый изменённый уровень, который знает консоль, должен быть у ID с правами;
/// * скрытые уровни старой карты (консоль о них не знает) возвращаются в набор.
pub fn rewrite_access(
    old: &BTreeSet<String>,
    requested: &[String],
    levels: &[String],
    privileged: &BTreeSet<String>,
) -> Option<BTreeSet<String>> {
    if !requested.iter().all(|t| levels.contains(t)) {
        return None;
    }
    let new: BTreeSet<String> = requested.iter().cloned().collect();
    if *old == new {
        return None;
    }
    let changed = old.symmetric_difference(&new);
    if !changed
        .filter(|t| levels.contains(*t))
        .all(|t| privileged.contains(t))
    {
        return None;
    }
    let mut out = new;
    out.extend(old.iter().filter(|t| !levels.contains(*t)).cloned());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn lv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_levels_the_privileged_card_holds_can_be_added_or_removed() {
        let levels = lv(&["Engineering", "Security", "Medical"]);
        let privileged = set(&["Engineering", "Medical"]);
        // Выдать Engineering: можно.
        assert_eq!(
            rewrite_access(&set(&[]), &lv(&["Engineering"]), &levels, &privileged),
            Some(set(&["Engineering"]))
        );
        // Выдать Security, которого у ID с правами нет: отказ.
        assert_eq!(
            rewrite_access(&set(&[]), &lv(&["Security"]), &levels, &privileged),
            None
        );
        // Забрать Security с карты без прав на него: отказ.
        assert_eq!(
            rewrite_access(&set(&["Security"]), &lv(&[]), &levels, &privileged),
            None
        );
        // Забрать Medical: можно.
        assert_eq!(
            rewrite_access(&set(&["Medical"]), &lv(&[]), &levels, &privileged),
            Some(set(&[]))
        );
    }

    #[test]
    fn unknown_levels_are_refused_and_hidden_ones_survive() {
        let levels = lv(&["Engineering"]);
        let privileged = set(&["Engineering", "Wizard"]);
        assert_eq!(
            rewrite_access(&set(&[]), &lv(&["Wizard"]), &levels, &privileged),
            None
        );
        // На карте был уровень, которого консоль не знает: после записи он остаётся.
        assert_eq!(
            rewrite_access(
                &set(&["Wizard"]),
                &lv(&["Engineering"]),
                &levels,
                &privileged
            ),
            Some(set(&["Engineering", "Wizard"]))
        );
        // Нет изменений — нет записи.
        assert_eq!(
            rewrite_access(
                &set(&["Engineering"]),
                &lv(&["Engineering"]),
                &levels,
                &privileged
            ),
            None
        );
    }

    #[test]
    fn names_are_clipped_by_characters() {
        assert_eq!(
            clip("Иванов Иван Иванович Иванов Иван Иванов", MAX_NAME_LEN)
                .chars()
                .count(),
            32
        );
        assert_eq!(clip("кратко", MAX_JOB_LEN), "кратко");
    }

    #[test]
    fn the_wire_form_round_trips() {
        let r = WriteRequest {
            name: "A".into(),
            title: "B".into(),
            access: lv(&["Service"]),
        };
        assert_eq!(parse_write(&write_data(&r)), Some(r));
    }
}
