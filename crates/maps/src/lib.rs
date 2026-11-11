//! Карта как данные: этажи, элементы конструкции, предметы на земле и точка появления.
//! Хранится в RON; игра строит из неё мир движка, импортёр SS14 пишет в тот же формат.

use std::path::{Path, PathBuf};

use astrocore_world::{FloorDef, StructureElement, World, WorldCommand, WorldConfig, WorldError};
use serde::{Deserialize, Serialize};

/// Предмет на земле: имя прототипа Build (`Crowbar`) и точка на полу, м.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapItem {
    pub prototype: String,
    pub x: f32,
    pub z: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapFile {
    pub name: String,
    /// Точка появления игроков (ноги), м.
    pub spawn: [f32; 3],
    pub floors: Vec<FloorDef>,
    /// Стены, проёмы, полы и лестницы; применяются в порядке списка.
    pub elements: Vec<StructureElement>,
    #[serde(default)]
    pub items: Vec<MapItem>,
}

#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error("мир: {0}")]
    World(#[from] WorldError),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("карта: {0}")]
    Ron(String),
}

impl MapFile {
    pub fn parse(text: &str) -> Result<Self, MapError> {
        ron::from_str(text).map_err(|e| MapError::Ron(e.to_string()))
    }

    pub fn load(path: &Path) -> Result<Self, MapError> {
        let text =
            std::fs::read_to_string(path).map_err(|e| MapError::Io(path.to_path_buf(), e))?;
        Self::parse(&text)
    }

    pub fn to_ron(&self) -> Result<String, MapError> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default().depth_limit(3))
            .map_err(|e| MapError::Ron(e.to_string()))
    }

    pub fn save(&self, path: &Path) -> Result<(), MapError> {
        std::fs::write(path, self.to_ron()?).map_err(|e| MapError::Io(path.to_path_buf(), e))
    }

    /// Мир движка из этажей и элементов карты (без предметов: их ставит игра).
    pub fn build_world(&self, world_id: &str) -> Result<World, MapError> {
        let mut world = World::new(world_id, WorldConfig::default())?;
        for f in &self.floors {
            world.apply(WorldCommand::DefineFloor(f.clone()))?;
        }
        for e in &self.elements {
            world.apply(WorldCommand::PlaceElement(e.clone()))?;
        }
        Ok(world)
    }

    /// Снимок структуры мира как карты (элементы по возрастанию идентификатора).
    pub fn from_world(name: &str, spawn: [f32; 3], world: &World, items: Vec<MapItem>) -> Self {
        let mut elements: Vec<StructureElement> = world.elements().cloned().collect();
        elements.sort_by_key(|e| e.id);
        let mut floors: Vec<FloorDef> = world.floors().cloned().collect();
        floors.sort_by_key(|f| f.id.0);
        Self {
            name: name.into(),
            spawn,
            floors,
            elements,
            items,
        }
    }
}
