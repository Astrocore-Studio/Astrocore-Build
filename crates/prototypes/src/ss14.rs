use std::collections::BTreeMap;

use serde_yaml::Value as Y;

use crate::{Data, PrototypeDef};

/// Что адаптер взял из файла SS14 и что пропустил.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Ss14Report {
    /// Прототипы `type: entity`, добавленные в реестр.
    pub entities: usize,
    /// Прототипы других типов (`reagent`, `tag`, `recipe`…), которые пока не переносятся.
    pub skipped_other_types: usize,
}

fn to_data(v: &Y) -> Data {
    match v {
        Y::Null => Data::Unit,
        Y::Bool(b) => Data::Bool(*b),
        Y::Number(n) => n
            .as_i64()
            .map_or_else(|| Data::Float(n.as_f64().unwrap_or(0.0)), Data::Int),
        Y::String(s) => Data::Str(s.clone()),
        Y::Sequence(s) => Data::List(s.iter().map(to_data).collect()),
        Y::Mapping(m) => Data::Map(
            m.iter()
                .filter_map(|(k, v)| Some((key_string(k)?, to_data(v))))
                .collect(),
        ),
        // Теги SS14 (`!type:Имя`) — подсказка десериализатору C#: берём само значение.
        Y::Tagged(t) => to_data(&t.value),
    }
}

fn key_string(k: &Y) -> Option<String> {
    match k {
        Y::String(s) => Some(s.clone()),
        Y::Number(n) => Some(n.to_string()),
        Y::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn str_field<'a>(m: &'a serde_yaml::Mapping, key: &str) -> Option<&'a str> {
    m.get(key).and_then(Y::as_str)
}

/// Прочитать YAML прототипов SS14. Компонент `- type: Item\n  size: Small` становится
/// компонентом `Item` с данными `{size: "Small"}`; `parent` может быть строкой или списком.
pub fn parse_ss14_yaml(source: &str) -> Result<(Vec<PrototypeDef>, Ss14Report), String> {
    let root: Y = serde_yaml::from_str(source).map_err(|e| e.to_string())?;
    let items = match root {
        Y::Sequence(s) => s,
        Y::Null => Vec::new(),
        _ => return Err("ожидался список прототипов верхнего уровня".into()),
    };
    let mut defs = Vec::new();
    let mut report = Ss14Report::default();
    for item in items {
        let Y::Mapping(m) = item else {
            return Err("элемент списка должен быть картой".into());
        };
        if str_field(&m, "type") != Some("entity") {
            report.skipped_other_types += 1;
            continue;
        }
        let id = str_field(&m, "id").ok_or("у прототипа нет id")?.to_string();
        let parents = match m.get("parent") {
            Some(Y::String(s)) => vec![s.clone()],
            Some(Y::Sequence(s)) => s
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        let mut components = BTreeMap::new();
        if let Some(Y::Sequence(list)) = m.get("components") {
            for c in list {
                let Y::Mapping(cm) = c else { continue };
                let Some(kind) = str_field(cm, "type") else {
                    continue;
                };
                let fields: BTreeMap<String, Data> = cm
                    .iter()
                    .filter_map(|(k, v)| {
                        let key = key_string(k)?;
                        (key != "type").then(|| (key, to_data(v)))
                    })
                    .collect();
                components.insert(kind.to_string(), Data::Map(fields));
            }
        }
        defs.push(PrototypeDef {
            id,
            parents,
            is_abstract: m.get("abstract").and_then(Y::as_bool).unwrap_or(false),
            name: str_field(&m, "name").map(str::to_string),
            components,
        });
        report.entities += 1;
    }
    Ok((defs, report))
}
