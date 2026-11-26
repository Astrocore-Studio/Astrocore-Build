use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};

use astrocore_grid2d::{Cell, Grid, Tile};

use crate::content::{Content, LevelLink, MapData, Placement, Prototype, parse_grid};

const UNDO_LIMIT: usize = 32;

#[derive(Clone)]
pub enum Brush {
    Tile(Tile),
    Spawn,
    Cable,
    Link,
    Object(String),
}

pub struct Document {
    path: PathBuf,
    draft: MapData,
    prototypes: BTreeMap<String, Prototype>,
    grid: Grid,
    saved: MapData,
    undo: VecDeque<MapData>,
    redo: Vec<MapData>,
}

impl Document {
    pub fn open(directory: &Path) -> Result<Self, String> {
        let read = |path: &Path| {
            std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
        };
        let path = directory.join("station.ron");
        let prototypes: BTreeMap<String, Prototype> =
            ron::from_str(&read(&directory.join("prototypes.ron"))?)
                .map_err(|error| format!("prototypes.ron: {error}"))?;
        let draft: MapData = match path.try_exists().map_err(|error| error.to_string())? {
            true => {
                ron::from_str(&read(&path)?).map_err(|error| format!("station.ron: {error}"))?
            }
            false => empty_station(),
        };
        let grid = Content::from_data(draft.clone(), prototypes.clone())?
            .layout
            .grid;
        Ok(Self {
            path,
            grid,
            saved: draft.clone(),
            draft,
            prototypes,
            undo: VecDeque::new(),
            redo: Vec::new(),
        })
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn name(&self) -> &str {
        &self.draft.name
    }

    pub fn spawns(&self) -> &[Cell] {
        &self.draft.spawns
    }

    pub fn cables(&self) -> &[Cell] {
        &self.draft.cables
    }

    pub fn link_cells(&self) -> impl Iterator<Item = Cell> + '_ {
        self.draft
            .links
            .iter()
            .flat_map(|link| [link.lower, link.upper])
    }

    pub fn prototypes(&self) -> &BTreeMap<String, Prototype> {
        &self.prototypes
    }

    pub fn objects(&self) -> impl Iterator<Item = (Cell, &Prototype)> {
        self.draft.placements.iter().filter_map(|object| {
            self.prototypes
                .get(&object.prototype)
                .map(|prototype| (object.cell, prototype))
        })
    }

    pub fn dirty(&self) -> bool {
        self.draft != self.saved || !self.path.is_file()
    }

    pub fn paint(&mut self, cell: Cell, brush: &Brush, erase: bool) -> bool {
        if !self.grid.contains(cell) {
            return false;
        }
        let previous = self.draft.clone();
        match brush {
            Brush::Tile(tile) => {
                let tile = if erase { Tile::Void } else { *tile };
                let rows = if cell.z == 0 {
                    &mut self.draft.grid
                } else {
                    self.draft
                        .levels
                        .get_mut(&cell.z)
                        .expect("этаж находится в документе")
                };
                let row = rows.len() - 1 - cell.y as usize;
                let symbol = match tile {
                    Tile::Void => "~",
                    Tile::Wall => "#",
                    Tile::Floor => ".",
                };
                rows[row].replace_range(cell.x as usize..cell.x as usize + 1, symbol);
                self.grid.set_tile(cell, tile);
            }
            Brush::Spawn => mark(&mut self.draft.spawns, cell, erase),
            Brush::Cable => mark(&mut self.draft.cables, cell, erase),
            Brush::Link => {
                if erase {
                    self.draft.links.retain(|link| !link.contains(cell));
                }
            }
            Brush::Object(id) => {
                if erase {
                    self.draft.placements.retain(|object| object.cell != cell);
                } else if self.prototypes.contains_key(id)
                    && !self
                        .draft
                        .placements
                        .iter()
                        .any(|object| object.cell == cell && object.prototype == *id)
                {
                    self.draft.placements.push(Placement {
                        cell,
                        prototype: id.clone(),
                        starts_off: false,
                    });
                }
            }
        }
        self.record_edit(previous)
    }

    pub fn add_level(&mut self, z: i16) -> bool {
        if self.grid.size_at(z).is_some() {
            return false;
        }
        let previous = self.draft.clone();
        let (width, height) = self.grid.size();
        self.draft
            .levels
            .insert(z, vec!["~".repeat(usize::from(width)); usize::from(height)]);
        self.refresh_grid();
        self.record_edit(previous)
    }

    pub fn connect_levels(&mut self, first: Cell, second: Cell) -> bool {
        let (lower, upper) = if first.z < second.z {
            (first, second)
        } else {
            (second, first)
        };
        if lower.z.checked_add(1) != Some(upper.z)
            || [lower, upper]
                .into_iter()
                .any(|cell| self.grid.tile(cell) != Tile::Floor)
            || self
                .draft
                .links
                .iter()
                .any(|link| link.lower == lower || link.upper == upper)
        {
            return false;
        }
        let previous = self.draft.clone();
        self.draft.links.push(LevelLink { lower, upper });
        self.record_edit(previous)
    }

    fn record_edit(&mut self, previous: MapData) -> bool {
        if self.draft == previous {
            return false;
        }
        if self.undo.len() == UNDO_LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back(previous);
        self.redo.clear();
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop_back() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.draft, previous));
        self.refresh_grid();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo
            .push_back(std::mem::replace(&mut self.draft, next));
        self.refresh_grid();
        true
    }

    fn refresh_grid(&mut self) {
        self.grid = parse_grid(&self.draft).expect("черновик содержит корректные уровни");
    }

    pub fn save(&mut self) -> Result<(), String> {
        Content::from_data(self.draft.clone(), self.prototypes.clone())?;
        let text = ron::ser::to_string_pretty(&self.draft, ron::ser::PrettyConfig::default())
            .map_err(|error| error.to_string())?;
        let directory = self.path.parent().ok_or("у файла карты нет каталога")?;
        let mut pending =
            tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
        pending
            .write_all(text.as_bytes())
            .map_err(|error| error.to_string())?;
        pending
            .as_file()
            .sync_all()
            .map_err(|error| error.to_string())?;
        pending
            .persist(&self.path)
            .map_err(|error| error.to_string())?;
        self.saved = self.draft.clone();
        Ok(())
    }
}

fn mark(cells: &mut Vec<Cell>, cell: Cell, erase: bool) {
    if erase {
        cells.retain(|&existing| existing != cell);
    } else if !cells.contains(&cell) {
        cells.push(cell);
    }
}

fn empty_station() -> MapData {
    let mut grid = vec!["~".repeat(32); 16];
    grid[7].replace_range(16..17, ".");
    MapData {
        name: "Untitled Station".into(),
        grid,
        levels: BTreeMap::new(),
        links: Vec::new(),
        spawns: vec![Cell::new(16, 8)],
        placements: Vec::new(),
        roles: vec!["Operator".into(), "Engineer".into(), "Medic".into()],
        loadouts: BTreeMap::new(),
        recovery: None,
        cables: Vec::new(),
        round_seconds: 600,
    }
}
