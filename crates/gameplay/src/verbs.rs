//! Глаголы и осмотр (`SharedVerbSystem`, `ExamineSystemShared`, `HealthExaminableSystem` SS14).
//!
//! Правая кнопка по объекту или игроку открывает меню глаголов: сервер по `GetVerbsEvent`
//! собирает список доступного (осмотр, взять, …), клиент показывает его и присылает выбранный.
//! Осмотр показывает имя и описание прототипа; у людей — ещё осмотр здоровья по порогам
//! `HealthExaminable` (в радиусе 3 м, `ExamineDetailsRange`). Осмотр доступен до 16 м
//! (`ExamineRange`) без преград; умирающий видит ближе (`CritExamineRange` 1,3 м,
//! `DeadExamineRange` 0,75 м).
//!
//! Не перенесено: рассуждения систем о состоянии предмета (заряд, замок, содержимое),
//! размытие дальнего осмотра, разметка цветом, идентичность (маски скрывают имя).

use astrocore_core::{NetworkEntityId, ObjectId};
use serde::{Deserialize, Serialize};

pub const VERBS_COMPONENT: &str = "build.verbs";
pub const VERBS_MAX_BYTES: usize = 2048;
pub const EXAMINE_COMPONENT: &str = "build.examine";
pub const EXAMINE_MAX_BYTES: usize = 4096;

/// Дальность осмотра, подробностей (здоровье) и осмотра лежащим и умирающим, м.
pub const EXAMINE_RANGE_M: f32 = 16.0;
pub const EXAMINE_DETAILS_RANGE_M: f32 = 3.0;
pub const CRIT_EXAMINE_RANGE_M: f32 = 1.3;
pub const DEAD_EXAMINE_RANGE_M: f32 = 0.75;

/// Цель глагола: предмет или объект мира либо игрок.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerbTarget {
    Object(ObjectId),
    Player(NetworkEntityId),
}

/// Данные команд `verb.list` и `verb.do`: тип цели, цель и (для `verb.do`) идентификатор глагола.
pub fn target_data(target: VerbTarget, verb: Option<&str>) -> Vec<u8> {
    let mut v = match target {
        VerbTarget::Object(id) => {
            let mut v = vec![0];
            v.extend_from_slice(&id.as_uuid().as_u128().to_be_bytes());
            v
        }
        VerbTarget::Player(p) => {
            let mut v = vec![1];
            v.extend_from_slice(&p.0.to_le_bytes());
            v
        }
    };
    if let Some(verb) = verb {
        v.extend_from_slice(verb.as_bytes());
    }
    v
}

/// Разбор `target_data`: цель и остаток (идентификатор глагола).
pub fn parse_target(data: &[u8]) -> Option<(VerbTarget, &str)> {
    match data.split_first()? {
        (0, rest) if rest.len() >= 16 => {
            let (id, tail) = rest.split_at(16);
            let id = u128::from_be_bytes(id.try_into().ok()?);
            Some((
                VerbTarget::Object(ObjectId::from_u128(id)),
                std::str::from_utf8(tail).ok()?,
            ))
        }
        (1, rest) if rest.len() >= 4 => {
            let (id, tail) = rest.split_at(4);
            let id = u32::from_le_bytes(id.try_into().ok()?);
            Some((
                VerbTarget::Player(NetworkEntityId(id)),
                std::str::from_utf8(tail).ok()?,
            ))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verb {
    pub id: String,
    pub label: String,
    /// Почему глагол недоступен (в SS14 `Disabled` + `Message`); `None` — доступен.
    #[serde(default)]
    pub disabled: Option<String>,
}

/// Для клиента: глаголы для цели; `seq` растёт с каждым ответом, чтобы тот же ответ дошёл снова.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerbsView {
    pub seq: u32,
    pub target: Vec<u8>,
    pub verbs: Vec<Verb>,
}

/// Для клиента: результат осмотра.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExamineView {
    pub seq: u32,
    pub name: String,
    pub lines: Vec<String>,
}

pub fn encode_verbs(v: &VerbsView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_verbs(bytes: &[u8]) -> Option<VerbsView> {
    if bytes.is_empty() || bytes.len() > VERBS_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

pub fn encode_examine(v: &ExamineView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_examine(bytes: &[u8]) -> Option<ExamineView> {
    if bytes.is_empty() || bytes.len() > EXAMINE_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Предел осмотра по состоянию того, кто смотрит (`GetExaminerRange`).
pub fn examine_range(can_act: bool, dead: bool) -> f32 {
    if dead {
        DEAD_EXAMINE_RANGE_M
    } else if !can_act {
        CRIT_EXAMINE_RANGE_M
    } else {
        EXAMINE_RANGE_M
    }
}

/// Строка осмотра здоровья для урона данного вида: самый большой порог, который урон превысил.
/// Пороги и тексты — `health-examinable-carbon.ftl` SS14 (с местоимением «они»).
pub fn health_line(kind: &str, damage: f32) -> Option<&'static str> {
    // (порог, текст) по возрастанию; берётся последний порог ниже урона.
    let table: &[(f32, &str)] = match kind {
        "Slash" => &[
            (8.0, "They have a superficial cut."),
            (15.0, "They have some small cuts."),
            (30.0, "They have a few sizeable gashes."),
            (50.0, "They have deep gashes across their body."),
            (
                75.0,
                "They have jagged, deep lacerations running across their body.",
            ),
            (100.0, "Their body looks bloody and mauled."),
            (200.0, "Their body is completely torn to shreds!"),
        ],
        "Blunt" => &[
            (8.0, "They have a mild bruise."),
            (15.0, "They have some bruises."),
            (30.0, "They are beaten up."),
            (50.0, "They are badly beaten up."),
            (75.0, "They have major blunt trauma across their body."),
            (100.0, "Their body is disfigured and extensively beaten."),
            (200.0, "Their body is absolutely smashed to a pulp!"),
        ],
        "Piercing" => &[
            (8.0, "They have a minor puncture wound."),
            (15.0, "They have some puncture wounds."),
            (30.0, "They have multiple deep puncture wounds."),
            (50.0, "They have major deep punctures across their body."),
            (
                75.0,
                "They have extensive, deep ragged tears across their body.",
            ),
            (
                100.0,
                "Their body is completely covered in massive, gaping holes.",
            ),
            (200.0, "Their body looks torn apart!"),
        ],
        "Asphyxiation" => &[
            (30.0, "Their lips are turning blue."),
            (75.0, "Their face is turning blue."),
        ],
        "Heat" => &[
            (8.0, "They have a superficial burn."),
            (15.0, "They have a few first-degree burns."),
            (30.0, "They have some second-degree burns."),
            (50.0, "They are covered in second-degree burns."),
            (75.0, "Their body is covered in severe third-degree burns."),
            (
                100.0,
                "Their body is extensively covered in fourth-degree burns.",
            ),
        ],
        _ => return None,
    };
    table
        .iter()
        .rev()
        .find(|(t, _)| damage > *t)
        .map(|(_, s)| *s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_survives_the_wire() {
        let o = VerbTarget::Object(ObjectId::from_u128(0xabc));
        assert_eq!(
            parse_target(&target_data(o, Some("examine"))),
            Some((o, "examine"))
        );
        let p = VerbTarget::Player(NetworkEntityId(7));
        assert_eq!(parse_target(&target_data(p, None)), Some((p, "")));
        assert_eq!(parse_target(&[2, 1, 2]), None);
        assert_eq!(parse_target(&[]), None);
    }

    #[test]
    fn health_lines_follow_the_thresholds() {
        assert_eq!(health_line("Blunt", 5.0), None);
        assert_eq!(health_line("Blunt", 10.0), Some("They have a mild bruise."));
        assert_eq!(health_line("Blunt", 40.0), Some("They are beaten up."));
        assert_eq!(
            health_line("Blunt", 500.0),
            Some("Their body is absolutely smashed to a pulp!")
        );
        assert_eq!(health_line("Unknown", 50.0), None);
    }

    #[test]
    fn the_dying_see_closer() {
        assert_eq!(examine_range(true, false), 16.0);
        assert_eq!(examine_range(false, false), 1.3);
        assert_eq!(examine_range(false, true), 0.75);
    }

    #[test]
    fn views_survive_the_wire() {
        let v = VerbsView {
            seq: 1,
            target: vec![0, 1],
            verbs: vec![Verb {
                id: "examine".into(),
                label: "Examine".into(),
                disabled: None,
            }],
        };
        assert_eq!(decode_verbs(&encode_verbs(&v)), Some(v));
        let e = ExamineView {
            seq: 2,
            name: "crowbar".into(),
            lines: vec!["A tool.".into()],
        };
        assert_eq!(decode_examine(&encode_examine(&e)), Some(e));
    }
}
