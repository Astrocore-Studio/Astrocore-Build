#[cfg(feature = "prototype-import")]
mod ss14;

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use astrocore_core::inheritance::parent_order;
use include_dir::{Dir, include_dir};
use ron::extensions::Extensions;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::content::Prototype;
use crate::{AccessReader, Kind};

#[cfg(feature = "prototype-import")]
pub use ss14::convert_ss14_catalog;

static BUILTIN: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/content/prototypes");

#[derive(Deserialize)]
struct Definition {
    #[serde(default)]
    parents: Vec<String>,
    #[serde(default, rename = "abstract")]
    is_abstract: bool,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<Kind>,
    #[serde(default)]
    color: Option<(u8, u8, u8)>,
    #[serde(default)]
    access_reader: Option<AccessReader>,
}

struct Properties {
    name: Option<String>,
    kind: Option<Kind>,
    color: Option<(u8, u8, u8)>,
    access_reader: Option<AccessReader>,
}

pub(crate) fn load_catalog(
    directory: Option<&Path>,
) -> Result<BTreeMap<String, Prototype>, String> {
    let mut catalog = Catalog(BTreeMap::new());
    if let Some(directory) = directory {
        let mut files = Vec::new();
        let legacy = directory.join("prototypes.ron");
        if exists(&legacy)? {
            files.push(legacy);
        }
        let prototypes = directory.join("prototypes");
        if exists(&prototypes)? {
            collect_files(&prototypes, &mut files)?;
        }
        if files.is_empty() {
            return Err(format!(
                "{}: нет RON-файлов прототипов",
                directory.display()
            ));
        }
        files.sort();
        for path in files {
            let source = fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            catalog.add(&path.display().to_string(), &source)?;
        }
    } else {
        catalog.add_embedded(&BUILTIN)?;
    }
    resolve_catalog(&catalog)
}

fn exists(path: &Path) -> Result<bool, String> {
    path.try_exists()
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn is_ron(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "ron")
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if file_type.is_dir() {
            collect_files(&path, files)?;
        } else if file_type.is_file() && is_ron(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn resolve_catalog(Catalog(definitions): &Catalog) -> Result<BTreeMap<String, Prototype>, String> {
    let ordered = parent_order(definitions, |definition| &definition.parents)
        .map_err(|error| format!("каталог прототипов: {error}"))?;
    let mut resolved = BTreeMap::<&str, Properties>::new();
    let mut concrete = BTreeMap::new();
    for id in ordered {
        let definition = &definitions[id];
        let mut properties = Properties {
            name: definition.name.clone(),
            kind: definition.kind,
            color: definition.color,
            access_reader: definition.access_reader.clone(),
        };
        for parent in &definition.parents {
            let inherited = &resolved[parent.as_str()];
            properties.name = properties.name.or_else(|| inherited.name.clone());
            properties.kind = properties.kind.or(inherited.kind);
            properties.color = properties.color.or(inherited.color);
            properties.access_reader = properties
                .access_reader
                .or_else(|| inherited.access_reader.clone());
        }
        if !definition.is_abstract {
            let (Some(name), Some(kind), Some(color)) =
                (properties.name.clone(), properties.kind, properties.color)
            else {
                return Err(format!(
                    "каталог прототипов: прототип {id:?} требует name, kind и color"
                ));
            };
            concrete.insert(
                id.to_owned(),
                Prototype {
                    name,
                    kind,
                    color,
                    access_reader: properties.access_reader.clone(),
                },
            );
        }
        resolved.insert(id, properties);
    }
    Ok(concrete)
}

struct Catalog(BTreeMap<String, Definition>);

impl Catalog {
    fn add_embedded(&mut self, directory: &Dir<'_>) -> Result<(), String> {
        for file in directory.files().filter(|file| is_ron(file.path())) {
            let path = file.path().display().to_string();
            let source = file
                .contents_utf8()
                .ok_or_else(|| format!("{path}: ожидался UTF-8"))?;
            self.add(&path, source)?;
        }
        for child in directory.dirs() {
            self.add_embedded(child)?;
        }
        Ok(())
    }

    fn add(&mut self, path: &str, source: &str) -> Result<(), String> {
        let Catalog(definitions) = ron::Options::default()
            .with_default_extension(Extensions::IMPLICIT_SOME)
            .from_str(source)
            .map_err(|error| format!("{path}: {error}"))?;
        for (id, definition) in definitions {
            match self.0.entry(id) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(definition);
                }
                std::collections::btree_map::Entry::Occupied(entry) => {
                    return Err(format!(
                        "{path}: прототип {:?} объявлен дважды",
                        entry.key()
                    ));
                }
            }
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for Catalog {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CatalogVisitor;

        impl<'de> Visitor<'de> for CatalogVisitor {
            type Value = Catalog;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("каталог прототипов с уникальными идентификаторами")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Catalog, M::Error> {
                let mut definitions = BTreeMap::new();
                while let Some((id, definition)) = map.next_entry::<String, Definition>()? {
                    match definitions.entry(id) {
                        std::collections::btree_map::Entry::Vacant(entry) => {
                            entry.insert(definition);
                        }
                        std::collections::btree_map::Entry::Occupied(entry) => {
                            return Err(de::Error::custom(format!(
                                "прототип {:?} объявлен дважды",
                                entry.key()
                            )));
                        }
                    }
                }
                Ok(Catalog(definitions))
            }
        }

        deserializer.deserialize_map(CatalogVisitor)
    }
}
