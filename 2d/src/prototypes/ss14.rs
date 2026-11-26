use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::path::Path;

use ::prototypes::{Data, PrototypeRegistry, Resolved};
use astrocore_grid2d::session::TICK_HZ;
use ron::ser::PrettyConfig;
use serde::Deserialize;

use crate::content::Prototype;
use crate::{Damage, Kind};

const DEFAULT_HEALING_SECONDS: f64 = 3.0;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    id: String,
    source: String,
    color: (u8, u8, u8),
    #[serde(default)]
    name: Option<String>,
}

pub fn convert_ss14_catalog(yaml_directory: &Path, selection: &str) -> Result<String, String> {
    let selection: Vec<Selection> =
        ron::from_str(selection).map_err(|error| format!("список импорта: {error}"))?;
    let mut registry = PrototypeRegistry::new();
    registry
        .load_dir(yaml_directory)
        .map_err(|error| error.to_string())?;
    let mut catalog = BTreeMap::new();
    for entry in selection {
        let target = match catalog.entry(entry.id) {
            Entry::Vacant(target) => target,
            Entry::Occupied(target) => {
                return Err(format!(
                    "список импорта: прототип {:?} объявлен дважды",
                    target.key()
                ));
            }
        };
        let source = registry
            .spawnable(&entry.source)
            .map_err(|error| error.to_string())?;
        let kind = medical_stack(&source)
            .map_err(|error| format!("прототип SS14 {:?}: {error}", source.id))?;
        let name = entry.name.or(source.name).ok_or_else(|| {
            format!(
                "прототип SS14 {:?}: требуется name в списке импорта",
                source.id
            )
        })?;
        target.insert(Prototype {
            access_reader: None,
            name,
            kind,
            color: entry.color,
        });
    }
    ron::ser::to_string_pretty(&catalog, PrettyConfig::default()).map_err(|error| error.to_string())
}

fn medical_stack(source: &Resolved) -> Result<Kind, String> {
    let healing = source
        .component("Healing")
        .ok_or("нет компонента Healing")?;
    let types = healing.get("damage").and_then(|damage| damage.get("types"));
    let restores = Damage {
        blunt: restoring_points(types, "Blunt")?,
        asphyxiation: restoring_points(types, "Asphyxiation")?,
    };
    if restores.total() == 0 {
        return Err("нет лечения Blunt или Asphyxiation в Healing.damage.types".into());
    }
    let seconds = number(
        healing.get("delay"),
        DEFAULT_HEALING_SECONDS,
        "Healing.delay",
    )?;
    if seconds <= 0.0 || seconds > 15.0 {
        return Err("Healing.delay должен быть больше нуля и до 15 секунд".into());
    }
    let uses = number(
        source
            .component("Stack")
            .and_then(|stack| stack.get("count")),
        1.0,
        "Stack.count",
    )?;
    if !(1.0..=f64::from(u8::MAX)).contains(&uses) || uses.fract() != 0.0 {
        return Err("Stack.count должен быть целым числом от 1 до 255".into());
    }
    Ok(Kind::MedicalStack {
        restores,
        use_ticks: (seconds * f64::from(TICK_HZ)).ceil() as u16,
        uses: uses as u8,
    })
}

fn restoring_points(types: Option<&Data>, kind: &str) -> Result<u16, String> {
    let field = format!("Healing.damage.types.{kind}");
    let amount = -number(types.and_then(|types| types.get(kind)), 0.0, &field)?;
    if !(0.0..=f64::from(u16::MAX)).contains(&amount) || amount.fract() != 0.0 {
        return Err(format!(
            "{field} должен задавать целое неположительное значение"
        ));
    }
    Ok(amount as u16)
}

fn number(value: Option<&Data>, default: f64, field: &str) -> Result<f64, String> {
    match value {
        Some(data) => data
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("{field} должен быть конечным числом")),
        None => Ok(default),
    }
}
