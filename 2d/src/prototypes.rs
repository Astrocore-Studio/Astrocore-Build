use std::collections::BTreeMap;
use std::fmt;

use astrocore_core::inheritance::parent_order;
use ron::extensions::Extensions;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::Kind;
use crate::content::Prototype;

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
}

struct Properties {
    name: Option<String>,
    kind: Option<Kind>,
    color: Option<(u8, u8, u8)>,
}

pub(crate) fn parse_catalog(source: &str) -> Result<BTreeMap<String, Prototype>, String> {
    let Catalog(definitions) = ron::Options::default()
        .with_default_extension(Extensions::IMPLICIT_SOME)
        .from_str(source)
        .map_err(|error| format!("prototypes.ron: {error}"))?;
    let ordered = parent_order(&definitions, |definition| &definition.parents)
        .map_err(|error| format!("prototypes.ron: {error}"))?;
    let mut resolved = BTreeMap::<&str, Properties>::new();
    let mut concrete = BTreeMap::new();
    for id in ordered {
        let definition = &definitions[id];
        let mut properties = Properties {
            name: definition.name.clone(),
            kind: definition.kind,
            color: definition.color,
        };
        for parent in &definition.parents {
            let inherited = &resolved[parent.as_str()];
            properties.name = properties.name.or_else(|| inherited.name.clone());
            properties.kind = properties.kind.or(inherited.kind);
            properties.color = properties.color.or(inherited.color);
        }
        if !definition.is_abstract {
            let (Some(name), Some(kind), Some(color)) =
                (properties.name.clone(), properties.kind, properties.color)
            else {
                return Err(format!(
                    "prototypes.ron: прототип {id:?} требует name, kind и color"
                ));
            };
            concrete.insert(id.to_owned(), Prototype { name, kind, color });
        }
        resolved.insert(id, properties);
    }
    Ok(concrete)
}

struct Catalog(BTreeMap<String, Definition>);

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
