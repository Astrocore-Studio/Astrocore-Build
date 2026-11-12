//! Должности и стартовое снаряжение (`StationJobsSystem`, `StationSpawningSystem`, снимок SS14
//! `910154a7`).
//!
//! Должность — прототип с компонентом `Job`: название, число мест (`start` — на старте раунда,
//! `max` — предел, −1 без ограничения) и снаряжение: слоты инвентаря, предметы в руки и в
//! хранилища надетых вещей. Игрок, появившийся в раунде, выбирает должность из свободных:
//! снаряжение создаётся и надевается принудительно (`EquipStartingGear`), заполняемые
//! контейнеры наполняются (`StorageFill`).
//!
//! Не перенесено: удостоверения и доступ (`PDA` лежит в слоте как обычный предмет), имя и
//! внешность персонажа, особые действия должности (`Special`), приоритеты и очередь на роли.

use std::collections::BTreeMap;

use prototypes::Data;
use serde::{Deserialize, Serialize};

pub const JOBS_COMPONENT: &str = "build.jobs";
pub const JOBS_MAX_BYTES: usize = 8192;

#[derive(Debug, Clone, PartialEq)]
pub struct JobSpec {
    pub id: String,
    pub title: String,
    pub max: i32,
    /// Порядок в списке: больше — выше (`JobUIComparer`).
    pub weight: i32,
    /// Уровни доступа удостоверения должности (`access` и группы).
    pub access: Vec<String>,
    pub equipment: BTreeMap<String, String>,
    pub inhand: Vec<String>,
    pub storage: BTreeMap<String, Vec<String>>,
}

impl JobSpec {
    pub fn parse(id: &str, d: &Data) -> Option<Self> {
        let strings = |k: &str| -> Vec<String> {
            d.get(k)
                .and_then(Data::as_list)
                .map(|l| {
                    l.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        let map = d.get("equipment").and_then(Data::as_map)?;
        Some(Self {
            id: id.strip_prefix("Job").unwrap_or(id).to_string(),
            title: d.get("title").and_then(Data::as_str)?.to_string(),
            max: d.get("max").and_then(Data::as_int).unwrap_or(-1) as i32,
            weight: d.get("weight").and_then(Data::as_int).unwrap_or(0) as i32,
            access: strings("access"),
            equipment: map
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect(),
            inhand: strings("inhand"),
            storage: d
                .get("storage")
                .and_then(Data::as_map)
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                v.as_list()
                                    .map(|l| {
                                        l.iter()
                                            .filter_map(|x| x.as_str().map(String::from))
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// Для клиента: список должностей и текущая.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobsView {
    pub jobs: Vec<JobEntry>,
    pub current: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobEntry {
    pub id: String,
    pub title: String,
    /// Сколько мест занято и сколько всего (−1 — без ограничения).
    pub taken: u32,
    pub max: i32,
}

pub fn encode_jobs(v: &JobsView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_jobs(bytes: &[u8]) -> Option<JobsView> {
    if bytes.is_empty() || bytes.len() > JOBS_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_is_read_from_prototype_data() {
        let d = Data::Map(BTreeMap::from([
            ("title".to_string(), Data::Str("Janitor".into())),
            ("max".to_string(), Data::Int(1)),
            (
                "equipment".to_string(),
                Data::Map(BTreeMap::from([(
                    "shoes".to_string(),
                    Data::Str("Galoshes".into()),
                )])),
            ),
            (
                "inhand".to_string(),
                Data::List(vec![Data::Str("Mop".into())]),
            ),
            (
                "storage".to_string(),
                Data::Map(BTreeMap::from([(
                    "back".to_string(),
                    Data::List(vec![Data::Str("Box".into())]),
                )])),
            ),
        ]));
        let j = JobSpec::parse("JobJanitor", &d).unwrap();
        assert_eq!(j.id, "Janitor");
        assert_eq!(j.max, 1);
        assert_eq!(j.equipment["shoes"], "Galoshes");
        assert_eq!(j.inhand, ["Mop"]);
        assert_eq!(j.storage["back"], ["Box"]);
    }

    #[test]
    fn the_wire_form_round_trips() {
        let v = JobsView {
            jobs: vec![JobEntry {
                id: "Janitor".into(),
                title: "Janitor".into(),
                taken: 0,
                max: 1,
            }],
            current: None,
        };
        assert_eq!(decode_jobs(&encode_jobs(&v)), Some(v));
    }
}
