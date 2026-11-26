//! Сглаживание спрайтов соседних сущностей: стены, окна и столы срастаются в одно целое.
//! Образец — `Content.Client/IconSmoothing` SS14 @ 2f4a2aad: компонент `IconSmooth` и правила
//! `CornerSpriteSmoothing`, `CardinalCornerSpriteSmoothing`, `DiagonalSpriteSmoothing`,
//! `AltDiagonalSpriteSmoothing`, `DirectionSpriteSmoothing`, `EdgeSpriteSmoothing`.
//!
//! Сущность с `IconSmooth`, привязанная к клетке, смотрит на восемь соседних клеток. Если
//! на соседней клетке есть привязанная сущность, чей ключ входит в маску правила, сторона
//! считается соединённой, и слои спрайта получают подходящие состояния набора RSI.

use std::collections::HashSet;

use astrocore_client::{Sprite, SpriteLayer};
use astrocore_ecs::{ComponentInit, EventWorldExt, GameWorldExt, PostUpdate, RegistryWorldExt};
use astrocore_map::{AnchorStateChanged, MapGrid, MoveEvent, TileCoord, Transform};
use astrocore_math::{Angle, Vec2};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

/// Восемь направлений в порядке `Direction` Robust: юг, юго-восток, восток, северо-восток,
/// север, северо-запад, запад, юго-запад.
const DIRECTIONS: [(i32, i32); 8] = [
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
];

const SOUTH: u8 = 1 << 0;
const SOUTH_EAST: u8 = 1 << 1;
const EAST: u8 = 1 << 2;

fn corner_key() -> String {
    "corner".to_owned()
}

fn direction_key() -> String {
    "direction".to_owned()
}

fn edge_key() -> String {
    "edge".to_owned()
}

/// Правило из четырёх угловых слоёв (`CornerSpriteSmoothing`) и его родственники с одним
/// слоем (`DiagonalSpriteSmoothing`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CornerRule {
    /// Начало имени состояния: к нему добавляется число.
    pub base: String,
    /// Ключи сущностей, с которыми правило соединяет.
    pub mask: HashSet<String>,
    #[serde(default = "corner_key")]
    pub layer_key: String,
    /// Место слоёв в спрайте; без него слои ложатся сверху.
    #[serde(default)]
    pub index: Option<usize>,
    #[serde(default)]
    pub shader: Option<String>,
}

/// Угловое правило со второй маской (`CardinalCornerSpriteSmoothing`,
/// `AltDiagonalSpriteSmoothing`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AltCornerRule {
    pub base: String,
    pub mask: HashSet<String>,
    pub alt_mask: HashSet<String>,
    #[serde(default = "corner_key")]
    pub layer_key: String,
    #[serde(default)]
    pub index: Option<usize>,
    #[serde(default)]
    pub shader: Option<String>,
}

/// Один слой, состояние которого — число из четырёх сторон (`DirectionSpriteSmoothing`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectionRule {
    pub base: String,
    pub mask: HashSet<String>,
    #[serde(default = "direction_key")]
    pub layer_key: String,
    #[serde(default)]
    pub index: Option<usize>,
    #[serde(default)]
    pub shader: Option<String>,
}

/// Кромки на соседних клетках: видны там, где соседа нет (`EdgeSpriteSmoothing`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeRule {
    pub base: String,
    pub mask: HashSet<String>,
    #[serde(default = "edge_key")]
    pub layer_key: String,
    #[serde(default)]
    pub index: Option<usize>,
    #[serde(default)]
    pub shader: Option<String>,
}

/// Правило сглаживания одного набора слоёв. Имена вариантов — типы SS14 (`!type:…`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SmoothState {
    CornerSpriteSmoothing(CornerRule),
    CardinalCornerSpriteSmoothing(AltCornerRule),
    DiagonalSpriteSmoothing(CornerRule),
    AltDiagonalSpriteSmoothing(AltCornerRule),
    DirectionSpriteSmoothing(DirectionRule),
    EdgeSpriteSmoothing(EdgeRule),
}

fn yes() -> bool {
    true
}

/// Сглаживание спрайта с соседями. В данных — компонент `IconSmooth`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IconSmooth {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Ключ сущности: правила соседей ищут его в своих масках.
    pub key: String,
    #[serde(default)]
    pub states: Vec<SmoothState>,
}

/// Сущности, спрайт которых пора пересчитать.
#[derive(Resource, Debug, Default)]
struct SmoothQueue {
    entities: Vec<Entity>,
    queued: HashSet<Entity>,
}

impl SmoothQueue {
    fn push(&mut self, entity: Entity) {
        if self.queued.insert(entity) {
            self.entities.push(entity);
        }
    }
}

/// Углы правила: имя угла, направление набора RSI с картинкой этого угла и три стороны,
/// из которых складывается номер состояния (против часовой стрелки, диагональ, по часовой).
const CORNERS: [(&str, u8, [usize; 3]); 4] = [
    ("SouthEast", 0, [0, 1, 2]),
    ("NorthEast", 2, [2, 3, 4]),
    ("NorthWest", 1, [4, 5, 6]),
    ("SouthWest", 3, [6, 7, 0]),
];

/// Кромки правила `EdgeSpriteSmoothing`: имя и номер направления.
const EDGES: [(&str, usize); 4] = [("south", 0), ("east", 2), ("north", 4), ("west", 6)];

/// Добавляет слой с ключом, если его ещё нет.
fn add_layer(sprite: &mut Sprite, index: Option<usize>, layer: SpriteLayer) {
    if sprite.layers.iter().any(|l| l.map == layer.map) {
        return;
    }
    // Спрайт из одного состояния хранит его вне списка слоёв: переносим в список, иначе
    // новые слои его заменят.
    if sprite.layers.is_empty()
        && let Some(state) = sprite.state.clone()
    {
        sprite.layers.push(SpriteLayer {
            state: Some(state),
            ..SpriteLayer::default()
        });
    }
    let at = index.map_or(sprite.layers.len(), |i| i.min(sprite.layers.len()));
    sprite.layers.insert(at, layer);
}

/// Создаёт слои правил в спрайте.
fn init_layers(world: &mut World, entity: Entity) {
    let Some(smooth) = world.get::<IconSmooth>(entity).cloned() else {
        return;
    };
    let Some(mut sprite) = world.get_mut::<Sprite>(entity) else {
        return;
    };
    let layer = |key: String, state: String, shader: &Option<String>| SpriteLayer {
        state: Some(state),
        map: vec![key],
        shader: shader.clone(),
        ..SpriteLayer::default()
    };
    for state in &smooth.states {
        match state {
            SmoothState::CornerSpriteSmoothing(CornerRule {
                base,
                layer_key,
                index,
                shader,
                ..
            })
            | SmoothState::CardinalCornerSpriteSmoothing(AltCornerRule {
                base,
                layer_key,
                index,
                shader,
                ..
            }) => {
                for (corner, direction, _) in CORNERS {
                    add_layer(
                        &mut sprite,
                        *index,
                        SpriteLayer {
                            direction: Some(direction),
                            ..layer(format!("{layer_key}{corner}"), format!("{base}0"), shader)
                        },
                    );
                }
            }
            SmoothState::DiagonalSpriteSmoothing(CornerRule {
                base,
                layer_key,
                index,
                shader,
                ..
            })
            | SmoothState::AltDiagonalSpriteSmoothing(AltCornerRule {
                base,
                layer_key,
                index,
                shader,
                ..
            })
            | SmoothState::DirectionSpriteSmoothing(DirectionRule {
                base,
                layer_key,
                index,
                shader,
                ..
            }) => add_layer(
                &mut sprite,
                *index,
                layer(layer_key.clone(), format!("{base}0"), shader),
            ),
            SmoothState::EdgeSpriteSmoothing(rule) => {
                for (suffix, direction) in EDGES {
                    let (x, y) = DIRECTIONS[direction];
                    add_layer(
                        &mut sprite,
                        rule.index,
                        SpriteLayer {
                            offset: Vec2::new(x as f32, y as f32),
                            visible: false,
                            ..layer(
                                format!("{}{suffix}", rule.layer_key),
                                format!("{}{suffix}", rule.base),
                                &rule.shader,
                            )
                        },
                    );
                }
            }
        }
    }
}

/// Поворот сущности как сдвиг номеров направлений (`AngleToOffset`): соседи правил с одним
/// слоем считаются в осях самой сущности, потому что её спрайт поворачивается вместе с ней.
fn angle_offset(rotation: Angle) -> usize {
    let turn = std::f32::consts::TAU;
    let angle = (-rotation.0).rem_euclid(turn);
    ((angle + turn / 8.0) / (turn / 4.0)).floor() as usize * 2 % 8
}

/// Стороны, на которых есть сосед с ключом из маски: бит `i` — направление `i`.
fn matches(adjacent: &[Vec<&str>; 8], mask: &HashSet<String>) -> u8 {
    let mut bits = 0;
    for (i, keys) in adjacent.iter().enumerate() {
        if keys.iter().any(|key| mask.contains(*key)) {
            bits |= 1 << i;
        }
    }
    bits
}

/// Три бита угла: сторона против часовой стрелки, диагональ, сторона по часовой.
fn corner_bits(bits: u8, sides: [usize; 3]) -> u8 {
    let bit = |side: usize| (bits >> side) & 1;
    bit(sides[0]) | bit(sides[1]) << 1 | bit(sides[2]) << 2
}

/// Номер состояния угла с двумя масками (`CardinalCornerSpriteSmoothing.GetCornerState`).
fn cardinal_corner_state(directions: u8, alt: u8) -> u8 {
    // Из трёх битов угла остаются две стороны: биты 0 и 1.
    let cardinal = |bits: u8| (bits & 1) | ((bits >> 2) & 1) << 1;
    let cardinals = cardinal(directions);
    let both = cardinals | cardinal(alt);
    if both < 3 {
        return if both != cardinals {
            both + 2
        } else {
            cardinals
        };
    }
    if both == cardinals {
        return if directions & 2 != 0 {
            6
        } else if alt & 2 != 0 {
            7
        } else {
            5
        };
    }
    if directions & 2 != 0 || alt & 2 != 0 {
        cardinals + 11
    } else {
        cardinals + 8
    }
}

/// Пересчитывает состояния слоёв сущности по соседям.
fn update_sprite(world: &mut World, entity: Entity) {
    let Some(smooth) = world.get::<IconSmooth>(entity) else {
        return;
    };
    let transform = world.get::<Transform>(entity);
    let placed = transform
        .filter(|t| t.anchored() && smooth.enabled)
        .and_then(|t| {
            let grid = world.get::<MapGrid>(t.grid()?)?;
            Some((
                grid,
                TileCoord::containing(t.local_position(), t.level()),
                t.local_rotation(),
            ))
        });

    // Ключи соседей по сторонам света и в осях сущности.
    let mut world_keys: [Vec<&str>; 8] = Default::default();
    let mut local_keys: [Vec<&str>; 8] = Default::default();
    if let Some((grid, tile, rotation)) = placed {
        let offset = angle_offset(rotation);
        for (i, (x, y)) in DIRECTIONS.into_iter().enumerate() {
            let neighbour = TileCoord::new(tile.x + x, tile.y + y, tile.z);
            for other in grid.anchored_at(neighbour) {
                if let Some(other) = world.get::<IconSmooth>(*other)
                    && other.enabled
                {
                    world_keys[i].push(&other.key);
                    local_keys[(i + offset) % 8].push(&other.key);
                }
            }
        }
    }

    // Новые состояния и видимость слоёв по ключам.
    let mut states: Vec<(String, Option<String>, Option<bool>)> = Vec::new();
    for state in &smooth.states {
        match state {
            SmoothState::CornerSpriteSmoothing(rule) => {
                let bits = matches(&world_keys, &rule.mask);
                for (corner, _, sides) in CORNERS {
                    states.push((
                        format!("{}{corner}", rule.layer_key),
                        Some(format!("{}{}", rule.base, corner_bits(bits, sides))),
                        None,
                    ));
                }
            }
            SmoothState::CardinalCornerSpriteSmoothing(rule) => {
                let bits = matches(&world_keys, &rule.mask);
                let alt = matches(&world_keys, &rule.alt_mask);
                for (corner, _, sides) in CORNERS {
                    let number =
                        cardinal_corner_state(corner_bits(bits, sides), corner_bits(alt, sides));
                    states.push((
                        format!("{}{corner}", rule.layer_key),
                        Some(format!("{}{number}", rule.base)),
                        None,
                    ));
                }
            }
            SmoothState::DiagonalSpriteSmoothing(rule) => {
                let all = SOUTH | SOUTH_EAST | EAST;
                let joined = matches(&local_keys, &rule.mask) & all == all;
                states.push((
                    rule.layer_key.clone(),
                    Some(format!("{}{}", rule.base, u8::from(joined))),
                    None,
                ));
            }
            SmoothState::AltDiagonalSpriteSmoothing(rule) => {
                let all = SOUTH | SOUTH_EAST | EAST;
                let number = if matches(&local_keys, &rule.mask) & all == all {
                    1
                } else if matches(&local_keys, &rule.alt_mask) & all == all {
                    2
                } else {
                    0
                };
                states.push((
                    rule.layer_key.clone(),
                    Some(format!("{}{number}", rule.base)),
                    None,
                ));
            }
            SmoothState::DirectionSpriteSmoothing(rule) => {
                let bits = matches(&local_keys, &rule.mask);
                // Четыре стороны света: биты юга, востока, севера и запада подряд.
                let number = (0..4).fold(0, |n, i| n | ((bits >> (2 * i)) & 1) << i);
                states.push((
                    rule.layer_key.clone(),
                    Some(format!("{}{number}", rule.base)),
                    None,
                ));
            }
            SmoothState::EdgeSpriteSmoothing(rule) => {
                let bits = matches(&local_keys, &rule.mask);
                for (suffix, direction) in EDGES {
                    states.push((
                        format!("{}{suffix}", rule.layer_key),
                        None,
                        Some(bits & (1 << direction) == 0),
                    ));
                }
            }
        }
    }

    let Some(sprite) = world.get::<Sprite>(entity) else {
        return;
    };
    let differs = |layer: &SpriteLayer, state: &Option<String>, visible: Option<bool>| {
        state
            .as_ref()
            .is_some_and(|s| layer.state.as_ref() != Some(s))
            || visible.is_some_and(|v| layer.visible != v)
    };
    let find = |sprite: &Sprite, key: &str| {
        sprite
            .layers
            .iter()
            .position(|layer| layer.map.iter().any(|k| k == key))
    };
    // Спрайт помечается изменённым, только если состояние слоя правда другое: клиент
    // перестраивает картинку сущности по отметке изменения.
    let changes: Vec<(usize, Option<String>, Option<bool>)> = states
        .into_iter()
        .filter_map(|(key, state, visible)| {
            let index = find(sprite, &key)?;
            differs(&sprite.layers[index], &state, visible).then_some((index, state, visible))
        })
        .collect();
    if changes.is_empty() {
        return;
    }
    let Some(mut sprite) = world.get_mut::<Sprite>(entity) else {
        return;
    };
    for (index, state, visible) in changes {
        let layer = &mut sprite.layers[index];
        if let Some(state) = state {
            layer.state = Some(state);
        }
        if let Some(visible) = visible {
            layer.visible = visible;
        }
    }
}

/// Ставит в очередь сущность и сглаживаемых соседей её клетки.
fn queue_around(world: &mut World, entity: Entity, grid: Entity, tile: TileCoord) {
    let mut found = vec![entity];
    if let Some(grid) = world.get::<MapGrid>(grid) {
        for (x, y) in DIRECTIONS {
            let neighbour = TileCoord::new(tile.x + x, tile.y + y, tile.z);
            found.extend(
                grid.anchored_at(neighbour)
                    .iter()
                    .filter(|other| world.get::<IconSmooth>(**other).is_some()),
            );
        }
    }
    let mut queue = world.resource_mut::<SmoothQueue>();
    for entity in found {
        queue.push(entity);
    }
}

fn process_queue(world: &mut World) {
    loop {
        let entities = {
            let mut queue = world.resource_mut::<SmoothQueue>();
            queue.queued.clear();
            std::mem::take(&mut queue.entities)
        };
        if entities.is_empty() {
            break;
        }
        for entity in entities {
            update_sprite(world, entity);
        }
    }
}

/// Подключает сглаживание к миру клиента.
pub fn install(world: &mut World) {
    world.register_data_component::<IconSmooth>();
    world.init_resource::<SmoothQueue>();
    world.subscribe::<IconSmooth, ComponentInit>(|world, entity, _| {
        init_layers(world, entity);
        world.resource_mut::<SmoothQueue>().push(entity);
    });
    world.subscribe::<IconSmooth, AnchorStateChanged>(|world, entity, event| {
        queue_around(world, entity, event.grid, event.tile);
    });
    // Поворот меняет оси, в которых правила с одним слоем считают соседей.
    world.subscribe::<IconSmooth, MoveEvent>(|world, entity, event| {
        if event.old_rotation != event.new_rotation {
            world.resource_mut::<SmoothQueue>().push(entity);
        }
    });
    world.add_game_systems(PostUpdate, process_queue);
}

#[cfg(test)]
mod tests {
    use astrocore_data::{from_node, parse_yaml};

    use super::*;

    #[test]
    fn rules_read_ss14_data() {
        let smooth: IconSmooth = from_node(
            &parse_yaml(
                "key: walls\nstates:\n- !type:CornerSpriteSmoothing\n  mask: [walls, windows]\n  base: solid\n- !type:CardinalCornerSpriteSmoothing\n  mask: [walls]\n  altMask: [doors]\n  base: frame\n  layerKey: frame\n  index: 0\n",
            )
            .unwrap(),
        )
        .unwrap();
        assert!(smooth.enabled);
        assert_eq!(smooth.states.len(), 2);
        let SmoothState::CornerSpriteSmoothing(rule) = &smooth.states[0] else {
            panic!("первое правило — угловое");
        };
        assert_eq!(rule.layer_key, "corner");
        assert!(rule.mask.contains("windows"));
        let SmoothState::CardinalCornerSpriteSmoothing(rule) = &smooth.states[1] else {
            panic!("второе правило — угловое с двумя масками");
        };
        assert_eq!(rule.index, Some(0));
        assert!(rule.alt_mask.contains("doors"));
    }

    #[test]
    fn corner_numbers_follow_ss14() {
        // Сосед на юге и востоке без диагонали: юго-восточный угол — состояние 5.
        let bits = SOUTH | EAST;
        assert_eq!(corner_bits(bits, CORNERS[0].2), 5);
        // Для северо-восточного угла восток — сторона против часовой стрелки.
        assert_eq!(corner_bits(bits, CORNERS[1].2), 1);
        // Для юго-западного юг — сторона по часовой стрелке.
        assert_eq!(corner_bits(bits, CORNERS[3].2), 4);
        assert_eq!(corner_bits(0xFF, CORNERS[2].2), 7);
    }

    #[test]
    fn cardinal_corner_states_follow_ss14() {
        // Нет соседей.
        assert_eq!(cardinal_corner_state(0, 0), 0);
        // Одна сторона своей маски.
        assert_eq!(cardinal_corner_state(1, 0), 1);
        // Одна сторона только второй маски.
        assert_eq!(cardinal_corner_state(0, 1), 3);
        // Обе стороны своей маски: без диагонали, с диагональю своей и второй маски.
        assert_eq!(cardinal_corner_state(5, 0), 5);
        assert_eq!(cardinal_corner_state(7, 0), 6);
        assert_eq!(cardinal_corner_state(5, 2), 7);
        // Стороны разных масок.
        assert_eq!(cardinal_corner_state(1, 4), 9);
        assert_eq!(cardinal_corner_state(1, 6), 12);
    }

    #[test]
    fn rotation_shifts_directions() {
        use std::f32::consts::FRAC_PI_2;
        assert_eq!(angle_offset(Angle(0.0)), 0);
        assert_eq!(angle_offset(Angle(FRAC_PI_2)), 6);
        assert_eq!(angle_offset(Angle(-FRAC_PI_2)), 2);
        assert_eq!(angle_offset(Angle(2.0 * FRAC_PI_2)), 4);
    }
}
