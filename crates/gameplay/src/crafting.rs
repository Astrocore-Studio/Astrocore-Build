//! Initial item construction with material steps and SetStackCount.
use std::sync::LazyLock;

pub const CMD_CRAFT: &str = "construction.item";
pub const CMD_PLACE: &str = "construction.place";
pub const CMD_RCD_MODE: &str = "rcd.mode";
pub const CMD_RCD_USE: &str = "rcd.use";
pub fn rcd_mode_info(id: &str) -> &'static serde_json::Value {
    &CATALOG["rcdModes"][id]
}

pub fn pipe_appearance(id: &str) -> Option<(&'static str, &'static str)> {
    let spec = &CATALOG["pipes"][id];
    Some((spec["sprite"].as_str()?, spec["state"].as_str()?))
}

pub fn rcd_modes(device: &str) -> impl Iterator<Item = &'static str> {
    CATALOG["rcdDevices"]
        .as_object()
        .into_iter()
        .flatten()
        .find(|(key, _)| prototypes::engine_id("ss14", key).is_some_and(|id| id.as_str() == device))
        .and_then(|(_, s)| s["modes"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
}

pub fn barriers() -> impl Iterator<Item = &'static str> {
    CATALOG["recipes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| matches!(r["graph"].as_str(), Some("Grille" | "Window")) && r["hide"] != true)
        .filter_map(|r| r["id"].as_str())
}

pub fn barrier_edge(proto: &str, dismantle: bool) -> Option<&'static serde_json::Value> {
    let base = girder_base(proto);
    let graph = if base.is_some() {
        "Girder"
    } else if proto == "Grille" || proto == "GrilleBroken" {
        "Grille"
    } else {
        "Window"
    };
    let nodes = CATALOG["graphs"][graph]["graph"].as_array()?;
    let target = nodes.iter().find(|n| n["entity"] == proto)?;
    let start = nodes
        .iter()
        .find(|n| n["node"] == base.unwrap_or("start"))?;
    let (from, to) = if dismantle {
        (target, start)
    } else {
        (start, target)
    };
    from["edges"]
        .as_array()?
        .iter()
        .find(|e| e["to"] == to["node"])
}

pub fn girder_base(proto: &str) -> Option<&'static str> {
    match proto {
        "WallUranium" | "WallSilver" | "WallBrass" | "WallPlasma" | "WallGold" | "WallMeat" => {
            Some("girder")
        }
        "WallShuttle" => Some("reinforcedGirder"),
        _ => None,
    }
}

pub fn girder_walls() -> impl Iterator<Item = (&'static str, &'static str)> {
    let nodes = CATALOG["graphs"]["Girder"]["graph"].as_array().unwrap();
    CATALOG["recipes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["graph"] == "Girder" && r["hide"] != true)
        .filter_map(|r| {
            let proto = nodes.iter().find(|n| n["node"] == r["targetNode"])?["entity"].as_str()?;
            girder_base(proto)?;
            Some((r["id"].as_str()?, proto))
        })
}

pub fn is_subfloor(material: &str) -> bool {
    material
        .strip_prefix("ss14:tile_")
        .is_some_and(|key| CATALOG["tiles"][key]["subfloor"] == true)
}

impl PipeRecipe {
    pub fn rotation_for(&self, directions: &str) -> Option<u8> {
        let dirs = crate::piping::parse_dirs(directions);
        (0..4).find(|&quarter| crate::piping::rotate_dirs(self.dirs, quarter) == dirs)
    }
}

pub fn placement_data(recipe: &str, rotation: u8, point: [f32; 3]) -> Vec<u8> {
    let mut data = crate::drop_at_data(rotation, point);
    data.extend_from_slice(recipe.as_bytes());
    data
}

pub fn placement_variant(recipe: &str, layer: u8) -> &str {
    if layer == 0 {
        return recipe;
    }
    pipes()
        .iter()
        .find(|r| {
            r.layer == layer
                && r.id.strip_suffix(if layer == 1 { "Alt1" } else { "Alt2" }) == Some(recipe)
        })
        .map_or(recipe, |r| r.id.as_str())
}

pub struct PipeRecipe {
    pub device: bool,
    pub reverse: &'static serde_json::Value,
    pub id: String,
    pub material: String,
    pub amount: u32,
    pub delay: f32,
    pub dirs: u8,
    pub volume: f32,
    pub layer: u8,
}

pub fn pipes() -> &'static [PipeRecipe] {
    static PIPES: LazyLock<Vec<PipeRecipe>> = LazyLock::new(|| {
        CATALOG["recipes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| {
                let graph = CATALOG["graphs"][r["graph"].as_str()?]["graph"].as_array()?;
                let start = graph.iter().find(|n| n["node"] == "start")?;
                let node = graph.iter().find(|n| n["node"] == r["targetNode"])?;
                let id = node["entity"].as_str()?;
                let data = &CATALOG["pipes"][id];
                let ports = data["ports"].as_str().or_else(|| {
                    Some(match data["direction"].as_str()? {
                        "South" => "S",
                        "Longitudinal" => "NS",
                        "SWBend" => "SW",
                        "TSouth" => "SEW",
                        "Fourway" => "NSEW",
                        _ => return None,
                    })
                })?;
                let dirs = crate::piping::parse_dirs(ports);
                let step = &start["edges"]
                    .as_array()?
                    .iter()
                    .find(|e| e["to"] == r["targetNode"])?["steps"][0];
                Some(PipeRecipe {
                    device: data["device"] == true,
                    reverse: node["edges"]
                        .as_array()?
                        .iter()
                        .find(|edge| edge["to"] == "start")?,
                    id: r["id"].as_str()?.into(),
                    material: step["material"].as_str()?.into(),
                    amount: step["amount"].as_u64()? as u32,
                    delay: step["doAfter"].as_f64()? as f32,
                    dirs,
                    volume: data["volume"]
                        .as_f64()
                        .unwrap_or(atmos::pipes::DEFAULT_PIPE_VOLUME as f64)
                        as f32,
                    layer: data["layer"].as_u64()? as u8,
                })
            })
            .collect()
    });
    &PIPES
}
pub(crate) static CATALOG: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../content/engineering-graphs.json"))
        .expect("generated engineering catalog")
});

pub struct Recipe {
    pub id: String,
    pub result: String,
    pub count: u32,
    pub materials: Vec<(String, u32)>,
}

pub fn recipes() -> &'static [Recipe] {
    static RECIPES: LazyLock<Vec<Recipe>> = LazyLock::new(|| {
        CATALOG["recipes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| {
                if r["objectType"] != "Item" || r.get("conditions").is_some() {
                    return None;
                }
                let graph = &CATALOG["graphs"][r["graph"].as_str()?];
                let nodes = graph["graph"].as_array()?;
                let start = nodes.iter().find(|n| n["node"] == graph["start"])?;
                let target = nodes.iter().find(|n| n["node"] == r["targetNode"])?;
                let edge = start["edges"]
                    .as_array()?
                    .iter()
                    .find(|e| e["to"] == target["node"])?;
                if edge.get("conditions").is_some() || target.get("actions").is_some() {
                    return None;
                }
                let completed = edge["completed"].as_array()?;
                if completed.len() != 1 || completed[0]["__tag"] != "SetStackCount" {
                    return None;
                }
                let materials: Vec<_> = edge["steps"]
                    .as_array()?
                    .iter()
                    .map(|s| {
                        if s.get("doAfter").is_some() || s.get("completed").is_some() {
                            return None;
                        }
                        Some((
                            s["material"].as_str()?.to_string(),
                            u32::try_from(s["amount"].as_u64()?).ok()?,
                        ))
                    })
                    .collect::<Option<_>>()?;
                if materials.is_empty() {
                    return None;
                }
                Some(Recipe {
                    id: r["id"].as_str()?.into(),
                    result: target["entity"].as_str()?.into(),
                    count: u32::try_from(completed[0]["amount"].as_u64()?).ok()?,
                    materials,
                })
            })
            .collect()
    });
    &RECIPES
}

impl Recipe {
    pub fn sprite_id(&self) -> String {
        prototypes::engine_id("ss14", &self.result)
            .expect("source entity id")
            .to_string()
    }
}
