mod data;
mod ss14;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub use data::Data;
pub use ss14::{Ss14Report, parse_ss14_yaml};

/// Предел размера файла прототипов: файлы с диска и из сети недоверенные.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Предел глубины наследования.
pub const MAX_DEPTH: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: файл больше {MAX_FILE_BYTES} байт")]
    TooLarge(PathBuf),
    #[error("{0}: {1}")]
    Parse(String, String),
    #[error("прототип {0:?} объявлен дважды")]
    Duplicate(String),
    #[error("прототип {0:?}: неизвестный родитель {1:?}")]
    UnknownParent(String, String),
    #[error("цикл наследования через {0:?}")]
    Cycle(String),
    #[error("наследование {0:?} глубже {MAX_DEPTH}")]
    TooDeep(String),
    #[error("прототип {0:?} не найден")]
    Unknown(String),
    #[error("прототип {0:?} абстрактный: создавать его нельзя")]
    Abstract(String),
    #[error("неверный идентификатор {0:?}")]
    BadId(String),
    #[error("прототипы {0:?} и {1:?} дают один идентификатор движка {2:?}")]
    IdCollision(String, String, String),
}

/// Прототип сущности в том виде, как он записан в файле.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PrototypeDef {
    pub id: String,
    /// Родители слева направо: поздние перекрывают ранние, потомок — всех.
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default, rename = "abstract")]
    pub is_abstract: bool,
    #[serde(default)]
    pub name: Option<String>,
    /// Компоненты по имени; значение — данные компонента.
    #[serde(default)]
    pub components: BTreeMap<String, Data>,
}

/// Прототип после разрешения наследования.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub id: String,
    pub name: Option<String>,
    pub components: BTreeMap<String, Data>,
}

impl Resolved {
    pub fn component(&self, name: &str) -> Option<&Data> {
        self.components.get(name)
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
}

/// Идентификатор прототипа для движка: `Crowbar` → `ss14:crowbar`, `CrowbarRed` → `ss14:crowbar_red`.
/// Идентификаторы движка — нижний регистр, `пространство:имя`; имена SS14 — CamelCase.
pub fn engine_id(namespace: &str, name: &str) -> Option<astrocore_core::PrototypeId> {
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            if prev.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit()) {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
        prev = Some(c);
    }
    astrocore_core::PrototypeId::new(format!("{namespace}:{out}")).ok()
}

#[derive(Debug, Default)]
pub struct PrototypeRegistry {
    defs: BTreeMap<String, PrototypeDef>,
}

impl PrototypeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.defs.keys().map(String::as_str)
    }

    pub fn insert(&mut self, def: PrototypeDef) -> Result<(), ProtoError> {
        if !valid_id(&def.id) {
            return Err(ProtoError::BadId(def.id));
        }
        if self.defs.contains_key(&def.id) {
            return Err(ProtoError::Duplicate(def.id));
        }
        self.defs.insert(def.id.clone(), def);
        Ok(())
    }

    /// Разобрать текст RON: список прототипов `[(id: "...", ...), ...]`.
    pub fn load_ron_str(&mut self, source: &str, origin: &str) -> Result<usize, ProtoError> {
        let defs: Vec<PrototypeDef> = ron::from_str(source)
            .map_err(|e| ProtoError::Parse(origin.to_string(), e.to_string()))?;
        let n = defs.len();
        for d in defs {
            self.insert(d)?;
        }
        Ok(n)
    }

    /// Разобрать YAML прототипов SS14 (только `type: entity`); отчёт говорит, что пропущено.
    pub fn load_ss14_yaml_str(
        &mut self,
        source: &str,
        origin: &str,
    ) -> Result<Ss14Report, ProtoError> {
        let (defs, report) =
            parse_ss14_yaml(source).map_err(|e| ProtoError::Parse(origin.to_string(), e))?;
        for d in defs {
            self.insert(d)?;
        }
        Ok(report)
    }

    /// Загрузить каталог: `*.ron` и `*.yml`/`*.yaml`, по возрастанию путей (порядок
    /// детерминирован). Возвращает число прототипов.
    pub fn load_dir(&mut self, dir: &Path) -> Result<usize, ProtoError> {
        let mut files = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let rd = std::fs::read_dir(&d).map_err(|e| ProtoError::Io(d.clone(), e))?;
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    files.push(p);
                }
            }
        }
        files.sort();
        let mut total = 0;
        for path in files {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "ron" | "yml" | "yaml") {
                continue;
            }
            let len = std::fs::metadata(&path)
                .map_err(|e| ProtoError::Io(path.clone(), e))?
                .len();
            if len > MAX_FILE_BYTES {
                return Err(ProtoError::TooLarge(path));
            }
            let text =
                std::fs::read_to_string(&path).map_err(|e| ProtoError::Io(path.clone(), e))?;
            let origin = path.display().to_string();
            total += if ext == "ron" {
                self.load_ron_str(&text, &origin)?
            } else {
                self.load_ss14_yaml_str(&text, &origin)?.entities
            };
        }
        Ok(total)
    }

    /// Соответствие «идентификатор движка → имя в реестре» для всех прототипов. Коллизия двух
    /// имён в один идентификатор — ошибка (иначе прототип подменился бы молча).
    pub fn engine_index(
        &self,
        namespace: &str,
    ) -> Result<BTreeMap<astrocore_core::PrototypeId, String>, ProtoError> {
        let mut index: BTreeMap<astrocore_core::PrototypeId, String> = BTreeMap::new();
        for name in self.defs.keys() {
            let id = engine_id(namespace, name).ok_or_else(|| ProtoError::BadId(name.clone()))?;
            if let Some(other) = index.insert(id.clone(), name.clone()) {
                return Err(ProtoError::IdCollision(other, name.clone(), id.to_string()));
            }
        }
        Ok(index)
    }

    /// Проверить все ссылки и цепочки наследования (вызывать после загрузки).
    pub fn validate(&self) -> Result<(), ProtoError> {
        for id in self.defs.keys() {
            self.resolve_inner(id, &mut Vec::new())?;
        }
        Ok(())
    }

    /// Разрешённый прототип. Абстрактные разрешаются тоже (для проверки), но `spawnable` их отвергает.
    pub fn resolve(&self, id: &str) -> Result<Resolved, ProtoError> {
        self.resolve_inner(id, &mut Vec::new())
    }

    /// Разрешённый прототип для создания сущности: абстрактные запрещены.
    pub fn spawnable(&self, id: &str) -> Result<Resolved, ProtoError> {
        let def = self
            .defs
            .get(id)
            .ok_or_else(|| ProtoError::Unknown(id.to_string()))?;
        if def.is_abstract {
            return Err(ProtoError::Abstract(id.to_string()));
        }
        self.resolve(id)
    }

    fn resolve_inner(&self, id: &str, path: &mut Vec<String>) -> Result<Resolved, ProtoError> {
        let def = self
            .defs
            .get(id)
            .ok_or_else(|| ProtoError::Unknown(id.to_string()))?;
        if path.iter().any(|p| p == id) {
            return Err(ProtoError::Cycle(id.to_string()));
        }
        if path.len() >= MAX_DEPTH {
            return Err(ProtoError::TooDeep(id.to_string()));
        }
        path.push(id.to_string());
        let mut components: BTreeMap<String, Data> = BTreeMap::new();
        let mut name = None;
        let mut seen = BTreeSet::new();
        for parent in &def.parents {
            if !seen.insert(parent.as_str()) {
                continue;
            }
            if !self.defs.contains_key(parent) {
                return Err(ProtoError::UnknownParent(id.to_string(), parent.clone()));
            }
            let p = self.resolve_inner(parent, path)?;
            name = p.name.or(name);
            for (k, v) in p.components {
                match components.remove(&k) {
                    Some(old) => components.insert(k, old.merged_with(v)),
                    None => components.insert(k, v),
                };
            }
        }
        path.pop();
        for (k, v) in &def.components {
            match components.remove(k) {
                Some(old) => components.insert(k.clone(), old.merged_with(v.clone())),
                None => components.insert(k.clone(), v.clone()),
            };
        }
        Ok(Resolved {
            id: id.to_string(),
            name: def.name.clone().or(name),
            components,
        })
    }
}

#[cfg(test)]
mod tests;
