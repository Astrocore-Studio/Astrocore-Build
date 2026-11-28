//! Хранилища с сеткой: рюкзаки, сумки, ящики для инструментов. Предмет занимает клетки по
//! своей форме и может лежать повёрнутым. Образец — `Content.Shared/Storage`
//! SS14 @ 2f4a2aad (`StorageComponent`, `SharedStorageSystem`: `CanInsert`,
//! `TryGetAvailableGridSpace`, `ItemFitsInGridLocation`, `GetAdjustedItemShape`).

use std::collections::BTreeMap;
use std::fmt;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, LifecycleWorldExt, RegistryWorldExt};
use astrocore_map::{ContainerKind, ContainerWorldExt, EntGotRemovedFromContainer, Transform};
use astrocore_math::{Angle, IVec2, Vec2};
use astrocore_net::{
    NetComponent, NetEntities, NetEntity, NetError, NetRegistryWorldExt, WriteContext,
};
use astrocore_prototypes::Prototypes;
use astrocore_ui::{BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::action_blocker::ActionBlockerWorldExt;
use crate::examine::loc;
use crate::hands::{Hands, HandsWorldExt};
use crate::interaction::{AccessViaContainer, ActivateInWorld, InteractUsing, InteractionWorldExt};
use crate::inventory::InventoryWorldExt;
use crate::item::{Item, size_weight};
use crate::popups::{PopupType, PopupWorldExt};
use crate::verbs::{GetVerbs, Verb, VerbKind};
use crate::whitelist::EntityWhitelist;
use astrocore_input::{BoundKeyState, InputWorldExt};

/// Контейнер хранилища (`StorageComponent.ContainerId`).
pub const STORAGE_CONTAINER: &str = "storagebase";
/// Ключ окна хранилища (`StorageUiKey.Key`).
pub const STORAGE_UI_KEY: &str = "enum.StorageUiKey.Key";
/// Функция ввода: открыть рюкзак.
pub const OPEN_BACKPACK: &str = "OpenBackpack";
/// Функция ввода: открыть пояс.
pub const OPEN_BELT: &str = "OpenBelt";
/// Наибольший размер по умолчанию (`_defaultStorageMaxItemSize`).
pub const DEFAULT_MAX_ITEM_SIZE: &str = "Normal";

/// Прямоугольник клеток включительно (`Box2i`). В данных — `"l,b,r,t"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellBox {
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
    pub top: i32,
}

impl CellBox {
    pub const fn new(left: i32, bottom: i32, right: i32, top: i32) -> Self {
        Self {
            left,
            bottom,
            right,
            top,
        }
    }

    pub fn contains(&self, cell: IVec2) -> bool {
        cell.x >= self.left && cell.x <= self.right && cell.y >= self.bottom && cell.y <= self.top
    }

    pub fn cells(&self) -> impl Iterator<Item = IVec2> + '_ {
        (self.bottom..=self.top)
            .flat_map(move |y| (self.left..=self.right).map(move |x| IVec2::new(x, y)))
    }

    pub fn width(&self) -> i32 {
        self.right - self.left + 1
    }

    pub fn height(&self) -> i32 {
        self.top - self.bottom + 1
    }
}

impl Serialize for CellBox {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.collect_str(&format_args!(
                "{},{},{},{}",
                self.left, self.bottom, self.right, self.top
            ))
        } else {
            (self.left, self.bottom, self.right, self.top).serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for CellBox {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BoxVisitor;

        impl Visitor<'_> for BoxVisitor {
            type Value = CellBox;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("клетки «лево,низ,право,верх»")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<CellBox, E> {
                let numbers: Vec<i32> = text
                    .split(',')
                    .map(|part| part.trim().parse())
                    .collect::<Result<_, _>>()
                    .map_err(|_| E::custom(format!("не прямоугольник клеток: `{text}`")))?;
                match numbers.as_slice() {
                    [l, b, r, t] => Ok(CellBox::new(*l, *b, *r, *t)),
                    _ => Err(E::custom(format!("не прямоугольник клеток: `{text}`"))),
                }
            }
        }

        if !deserializer.is_human_readable() {
            let (l, b, r, t) = <(i32, i32, i32, i32)>::deserialize(deserializer)?;
            return Ok(CellBox::new(l, b, r, t));
        }
        deserializer.deserialize_str(BoxVisitor)
    }
}

/// Ограничивающий прямоугольник набора (`GetBoundingBox`).
pub fn bounding(boxes: &[CellBox]) -> Option<CellBox> {
    let first = *boxes.first()?;
    Some(boxes.iter().fold(first, |acc, b| CellBox {
        left: acc.left.min(b.left),
        bottom: acc.bottom.min(b.bottom),
        right: acc.right.max(b.right),
        top: acc.top.max(b.top),
    }))
}

/// Место предмета в сетке (`ItemStorageLocation`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ItemStorageLocation {
    pub rotation: Angle,
    pub position: IVec2,
}

impl ItemStorageLocation {
    pub fn new(rotation: Angle, position: IVec2) -> Self {
        Self { rotation, position }
    }
}

fn yes() -> bool {
    true
}

fn rustle() -> Option<SoundSpecifier> {
    Some(SoundSpecifier::collection("storageRustle"))
}

/// Хранилище. В данных — компонент `Storage`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Storage {
    #[serde(default)]
    pub grid: Vec<CellBox>,
    #[serde(default)]
    pub max_item_size: Option<String>,
    /// Кликом этим хранилищем по предмету его можно убрать внутрь.
    #[serde(default)]
    pub quick_insert: bool,
    /// Кликом предметом по хранилищу его можно убрать внутрь.
    #[serde(default = "yes")]
    pub click_insert: bool,
    #[serde(default = "yes")]
    pub open_on_activate: bool,
    #[serde(default)]
    pub whitelist: Option<EntityWhitelist>,
    #[serde(default)]
    pub blacklist: Option<EntityWhitelist>,
    #[serde(default = "rustle")]
    pub storage_insert_sound: Option<SoundSpecifier>,
    #[serde(default)]
    pub storage_remove_sound: Option<SoundSpecifier>,
    #[serde(default = "rustle")]
    pub storage_open_sound: Option<SoundSpecifier>,
    #[serde(default)]
    pub storage_close_sound: Option<SoundSpecifier>,
    #[serde(default = "yes")]
    pub show_verb: bool,
    /// Где лежат предметы.
    #[serde(skip)]
    pub stored_items: BTreeMap<Entity, ItemStorageLocation>,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            grid: Vec::new(),
            max_item_size: None,
            quick_insert: false,
            click_insert: true,
            open_on_activate: true,
            whitelist: None,
            blacklist: None,
            storage_insert_sound: rustle(),
            storage_remove_sound: None,
            storage_open_sound: rustle(),
            storage_close_sound: None,
            show_verb: true,
            stored_items: BTreeMap::new(),
        }
    }
}

/// Состояние хранилища в сети: сетка, размер и места предметов.
#[derive(Serialize, Deserialize)]
struct StorageState {
    grid: Vec<CellBox>,
    max_item_size: Option<String>,
    stored: Vec<(NetEntity, ItemStorageLocation)>,
}

impl NetComponent for Storage {
    const NAME: &'static str = "Storage";
    // Места ссылаются на предметы, которые могут прийти в том же состоянии позже.
    const DEFERRED: bool = true;

    fn write(&self, world: &World, _: Entity, _: &WriteContext, out: &mut Vec<u8>) {
        let state = StorageState {
            grid: self.grid.clone(),
            max_item_size: self.max_item_size.clone(),
            stored: self
                .stored_items
                .iter()
                .filter_map(|(item, location)| Some((*world.get::<NetEntity>(*item)?, *location)))
                .collect(),
        };
        if let Ok(bytes) = postcard::to_extend(&state, std::mem::take(out)) {
            *out = bytes;
        }
    }

    fn apply(world: &mut World, entity: Entity, bytes: &[u8]) -> Result<(), NetError> {
        let state: StorageState = postcard::from_bytes(bytes)?;
        let stored: BTreeMap<Entity, ItemStorageLocation> = state
            .stored
            .into_iter()
            .filter_map(|(net, location)| {
                Some((world.get_resource::<NetEntities>()?.entity(net)?, location))
            })
            .collect();
        match world.get_mut::<Storage>(entity) {
            Some(mut storage) => {
                storage.grid = state.grid;
                storage.max_item_size = state.max_item_size;
                storage.stored_items = stored;
            }
            None => {
                let storage = Storage {
                    grid: state.grid,
                    max_item_size: state.max_item_size,
                    stored_items: stored,
                    ..Storage::default()
                };
                world.add_comp(entity, storage);
            }
        }
        Ok(())
    }
}

/// Форма предмета: своя или форма его размера (`GetItemShape`).
/// Сколько клеток занимают прямоугольники вместе, без повторов (`StorageHelpers.GetArea`).
pub fn shape_area(boxes: &[CellBox]) -> i32 {
    let cells: std::collections::BTreeSet<(i32, i32)> = boxes
        .iter()
        .flat_map(|b| b.cells().map(|c| (c.x, c.y)).collect::<Vec<_>>())
        .collect();
    cells.len() as i32
}

/// Сколько клеток занимают предметы хранилища (`GetCumulativeItemAreas`).
pub fn cumulative_item_areas(world: &World, storage: Entity) -> i32 {
    world
        .container(storage, STORAGE_CONTAINER)
        .map(|c| c.entities().to_vec())
        .unwrap_or_default()
        .into_iter()
        .filter(|item| world.get::<crate::item::Item>(*item).is_some())
        .map(|item| shape_area(&item_shape(world, item)))
        .sum()
}

pub fn item_shape(world: &World, item: Entity) -> Vec<CellBox> {
    let Some(component) = world.get::<Item>(item) else {
        return Vec::new();
    };
    if let Some(shape) = &component.shape {
        return shape.clone();
    }
    world
        .get_resource::<Prototypes>()
        .and_then(|prototypes| prototypes.resolved_node("itemSize", &component.size))
        .and_then(|node| node.get("defaultShape"))
        .and_then(|shape| astrocore_data::from_node::<Vec<CellBox>>(shape).ok())
        .unwrap_or_else(|| vec![CellBox::new(0, 0, 0, 0)])
}

/// Форма, повёрнутая вокруг своего центра и поставленная в клетку (`GetAdjustedItemShape`).
pub fn adjusted_shape(shape: &[CellBox], rotation: Angle, position: IVec2) -> Vec<CellBox> {
    let Some(bounds) = bounding(shape) else {
        return Vec::new();
    };
    let center = Vec2::new(
        (bounds.left + bounds.right) as f32 / 2.0,
        (bounds.bottom + bounds.top) as f32 / 2.0,
    );
    let turn = |point: Vec2| rotation.rotate(point - center) + center;
    let transform_box = |b: &CellBox| {
        let corners = [
            turn(Vec2::new(b.left as f32, b.bottom as f32)),
            turn(Vec2::new(b.right as f32, b.bottom as f32)),
            turn(Vec2::new(b.left as f32, b.top as f32)),
            turn(Vec2::new(b.right as f32, b.top as f32)),
        ];
        let min = corners.iter().fold(corners[0], |a, c| a.min(*c));
        let max = corners.iter().fold(corners[0], |a, c| a.max(*c));
        (min, max)
    };
    let (bounds_min, _) = transform_box(&bounds);
    let drift = Vec2::new(bounds.left as f32, bounds.bottom as f32) - bounds_min;
    shape
        .iter()
        .map(|b| {
            let (min, max) = transform_box(b);
            let (min, max) = (min + drift, max + drift);
            // Округление гасит погрешность поворота на четверть оборота.
            let floor = |value: f32| (value + 1e-4).floor() as i32;
            CellBox::new(
                floor(min.x) + position.x,
                floor(min.y) + position.y,
                floor(max.x) + position.x,
                floor(max.y) + position.y,
            )
        })
        .collect()
}

/// Размеры по весу (`ItemSizePrototype` по возрастанию).
fn sizes_by_weight(world: &World) -> Vec<(String, i32)> {
    let Some(prototypes) = world.get_resource::<Prototypes>() else {
        return Vec::new();
    };
    let mut sizes: Vec<(String, i32)> = prototypes
        .ids("itemSize")
        .map(|id| (id.to_owned(), size_weight(world, id)))
        .collect();
    sizes.sort_by_key(|(_, weight)| *weight);
    sizes
}

/// Наибольший размер предмета, который влезает (`GetMaxItemSize`): заданный, иначе на
/// ступень меньше размера самого хранилища.
pub fn max_item_size(world: &World, storage: Entity) -> String {
    let Some(component) = world.get::<Storage>(storage) else {
        return DEFAULT_MAX_ITEM_SIZE.to_owned();
    };
    if let Some(size) = &component.max_item_size {
        return size.clone();
    }
    let Some(own) = world.get::<Item>(storage) else {
        return DEFAULT_MAX_ITEM_SIZE.to_owned();
    };
    let sizes = sizes_by_weight(world);
    let own_weight = size_weight(world, &own.size);
    sizes
        .iter()
        .rev()
        .find(|(_, weight)| *weight < own_weight)
        .or_else(|| sizes.first())
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| DEFAULT_MAX_ITEM_SIZE.to_owned())
}

/// Занятые клетки, кроме клеток предмета `ignore`.
fn occupied(
    world: &World,
    storage: &Storage,
    ignore: Option<Entity>,
) -> std::collections::HashSet<IVec2> {
    let mut cells = std::collections::HashSet::new();
    for (item, location) in &storage.stored_items {
        if Some(*item) == ignore {
            continue;
        }
        for b in adjusted_shape(
            &item_shape(world, *item),
            location.rotation,
            location.position,
        ) {
            cells.extend(b.cells());
        }
    }
    cells
}

/// Помещается ли предмет в это место (`ItemFitsInGridLocation`).
pub fn item_fits(
    world: &World,
    storage: Entity,
    item: Entity,
    location: ItemStorageLocation,
) -> bool {
    let Some(component) = world.get::<Storage>(storage) else {
        return false;
    };
    let Some(bounds) = bounding(&component.grid) else {
        return false;
    };
    if !bounds.contains(location.position) {
        return false;
    }
    let taken = occupied(world, component, Some(item));
    adjusted_shape(
        &item_shape(world, item),
        location.rotation,
        location.position,
    )
    .iter()
    .flat_map(CellBox::cells)
    .all(|cell| component.grid.iter().any(|g| g.contains(cell)) && !taken.contains(&cell))
}

/// Свободное место для предмета (`TryGetAvailableGridSpace`): снизу вверх, слева направо,
/// сначала без поворота.
pub fn available_space(
    world: &World,
    storage: Entity,
    item: Entity,
) -> Option<ItemStorageLocation> {
    let component = world.get::<Storage>(storage)?;
    let bounds = bounding(&component.grid)?;
    let shape = item_shape(world, item);
    let angles: Vec<Angle> = match shape.as_slice() {
        [only] if only.width() == only.height() => vec![Angle::ZERO],
        [_] => vec![Angle::ZERO, Angle::degrees(90.0)],
        _ => (0..4)
            .map(|quarter| Angle::degrees(90.0 * quarter as f32))
            .collect(),
    };
    for y in bounds.bottom..=bounds.top {
        for x in bounds.left..=bounds.right {
            for angle in &angles {
                let location = ItemStorageLocation::new(*angle, IVec2::new(x, y));
                if item_fits(world, storage, item, location) {
                    return Some(location);
                }
            }
        }
    }
    None
}

/// Хранилища на `World`.
pub trait StorageWorldExt {
    /// Можно ли положить (`CanInsert`). `Err` — ключ сообщения о причине.
    fn can_insert_storage(
        &mut self,
        storage: Entity,
        item: Entity,
        ignore_location: bool,
    ) -> Result<(), Option<String>>;
    /// Положить в свободное место (`Insert`).
    fn storage_insert(
        &mut self,
        storage: Entity,
        item: Entity,
        user: Option<Entity>,
        play_sound: bool,
    ) -> bool;
    /// Положить в заданное место (`InsertAt`).
    fn storage_insert_at(
        &mut self,
        storage: Entity,
        item: Entity,
        location: ItemStorageLocation,
        user: Option<Entity>,
    ) -> bool;
    /// Положить предмет из активной руки (`PlayerInsertHeldEntity`).
    fn player_insert_held(&mut self, storage: Entity, user: Entity) -> bool;
    /// Переложить внутри (`TrySetItemStorageLocation`).
    fn set_item_location(
        &mut self,
        storage: Entity,
        item: Entity,
        location: ItemStorageLocation,
    ) -> bool;
    /// Открыть окно хранилища (`OpenStorageUI`).
    fn open_storage_ui(&mut self, storage: Entity, user: Entity, silent: bool);
}

impl StorageWorldExt for World {
    fn can_insert_storage(
        &mut self,
        storage: Entity,
        item: Entity,
        ignore_location: bool,
    ) -> Result<(), Option<String>> {
        let Some(component) = self.get::<Storage>(storage).cloned() else {
            return Err(None);
        };
        let Some(size) = self.get::<Item>(item).map(|item| item.size.clone()) else {
            return Err(None);
        };
        if self.get::<Transform>(item).is_some_and(|t| t.anchored()) {
            return Err(Some("comp-storage-anchored-failure".into()));
        }
        if component
            .whitelist
            .as_ref()
            .is_some_and(|list| !list.is_valid(self, item))
            || component
                .blacklist
                .as_ref()
                .is_some_and(|list| list.is_valid(self, item))
        {
            return Err(Some("comp-storage-invalid-container".into()));
        }
        let max = max_item_size(self, storage);
        let max_weight = size_weight(self, &max);
        if size_weight(self, &size) > max_weight {
            return Err(Some("comp-storage-too-big".into()));
        }
        if self.get::<Storage>(item).is_some()
            && size_weight(self, &max_item_size(self, item)) >= max_weight
        {
            return Err(Some("comp-storage-too-big".into()));
        }
        if !ignore_location
            && !component.stored_items.contains_key(&item)
            && available_space(self, storage, item).is_none()
        {
            return Err(Some("comp-storage-insufficient-capacity".into()));
        }
        self.can_insert(item, storage, STORAGE_CONTAINER)
            .map_err(|_| None)
    }

    fn storage_insert(
        &mut self,
        storage: Entity,
        item: Entity,
        user: Option<Entity>,
        play_sound: bool,
    ) -> bool {
        let location = match self
            .get::<Storage>(storage)
            .and_then(|s| s.stored_items.get(&item).copied())
        {
            Some(location) => location,
            None => match available_space(self, storage, item) {
                Some(location) => location,
                None => return false,
            },
        };
        insert_at(self, storage, item, location, user, play_sound)
    }

    fn storage_insert_at(
        &mut self,
        storage: Entity,
        item: Entity,
        location: ItemStorageLocation,
        user: Option<Entity>,
    ) -> bool {
        if !item_fits(self, storage, item, location) {
            return false;
        }
        insert_at(self, storage, item, location, user, true)
    }

    fn player_insert_held(&mut self, storage: Entity, user: Entity) -> bool {
        let Some(held) = self.active_item(user) else {
            return false;
        };
        if let Err(reason) = self.can_insert_storage(storage, held, false) {
            let text = loc(
                self,
                reason.as_deref().unwrap_or("comp-storage-cant-insert"),
                &[],
            );
            self.popup_entity_for(&text, storage, user, PopupType::Small);
            return false;
        }
        if !self.can_drop(user) {
            let name = astrocore_loc::EntityNameWorldExt::entity_arg(self, held);
            let text = loc(
                self,
                "comp-storage-cant-drop",
                &[("entity", name.clone().into())],
            );
            self.popup_entity_for(&text, storage, user, PopupType::Small);
            return false;
        }
        if !self.in_range_unobstructed(user, storage, crate::interaction::INTERACTION_RANGE)
            && self
                .containing_container(storage)
                .is_none_or(|c| c.owner != user)
        {
            return false;
        }
        if !self.storage_insert(storage, held, Some(user), true) {
            let text = loc(self, "comp-storage-cant-insert", &[]);
            self.popup_entity_for(&text, storage, user, PopupType::Small);
            return false;
        }
        true
    }

    fn set_item_location(
        &mut self,
        storage: Entity,
        item: Entity,
        location: ItemStorageLocation,
    ) -> bool {
        let inside = self
            .container(storage, STORAGE_CONTAINER)
            .is_some_and(|container| container.contains(item));
        if !inside || !item_fits(self, storage, item, location) {
            return false;
        }
        if let Some(mut component) = self.get_mut::<Storage>(storage) {
            component.stored_items.insert(item, location);
        }
        true
    }

    fn open_storage_ui(&mut self, storage: Entity, user: Entity, silent: bool) {
        if self.get::<Storage>(storage).is_none() || !self.can_interact(user, Some(storage)) {
            return;
        }
        if self.open_ui(storage, STORAGE_UI_KEY, user)
            && !silent
            && let Some(sound) = self
                .get::<Storage>(storage)
                .and_then(|s| s.storage_open_sound.clone())
        {
            self.play_entity(&sound, storage, None);
        }
    }
}

fn insert_at(
    world: &mut World,
    storage: Entity,
    item: Entity,
    location: ItemStorageLocation,
    user: Option<Entity>,
    play_sound: bool,
) -> bool {
    let Some(mut component) = world.get_mut::<Storage>(storage) else {
        return false;
    };
    let previous = component.stored_items.insert(item, location);
    // Из руки или другого контейнера предмет сначала вынимается.
    let current = world.containing_container(item);
    let already = current
        .as_ref()
        .is_some_and(|c| c.owner == storage && &*c.id == STORAGE_CONTAINER);
    let moved = already || current.is_none() || world.remove_from_container(item, None).is_ok();
    if !moved
        || (!already
            && world
                .insert_into_container(item, storage, STORAGE_CONTAINER)
                .is_err())
    {
        if let Some(mut component) = world.get_mut::<Storage>(storage) {
            match previous {
                Some(previous) => component.stored_items.insert(item, previous),
                None => component.stored_items.remove(&item),
            };
        }
        return false;
    }
    let silent =
        user.is_some_and(|user| crate::tag::TagWorldExt::has_tag(world, user, "SilentStorageUser"));
    if play_sound
        && !silent
        && let Some(sound) = world
            .get::<Storage>(storage)
            .and_then(|s| s.storage_insert_sound.clone())
    {
        world.play_entity(&sound, storage, None);
    }
    true
}

/// Сообщения окна хранилища.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StorageMessage {
    /// Клик по предмету в окне (`StorageInteractWithItemEvent`).
    InteractWithItem(NetEntity),
    /// Переложить (`StorageSetItemLocationEvent`).
    SetItemLocation(NetEntity, ItemStorageLocation),
    /// Положить предмет из руки в место (`StorageInsertItemIntoLocationEvent`).
    InsertItemIntoLocation(ItemStorageLocation),
}

fn handle_message(world: &mut World, storage: Entity, event: &mut BoundUiMessage<StorageMessage>) {
    let user = event.actor;
    if world.get::<Hands>(user).is_none() || !world.can_interact(user, Some(storage)) {
        return;
    }
    let entity_of =
        |world: &World, net: NetEntity| world.get_resource::<NetEntities>()?.entity(net);
    let inside = |world: &World, item: Entity| {
        world
            .container(storage, STORAGE_CONTAINER)
            .is_some_and(|container| container.contains(item))
    };
    match event.message.clone() {
        StorageMessage::InteractWithItem(net) => {
            let Some(item) = entity_of(world, net).filter(|item| inside(world, *item)) else {
                return;
            };
            match world.active_item(user) {
                None => {
                    world.try_pickup_any_hand(user, item, true);
                }
                Some(held) => {
                    if let Some(at) = astrocore_map::TransformWorldExt::map_coordinates(world, item)
                    {
                        world.interact_using(user, held, item, at);
                    }
                }
            }
        }
        StorageMessage::SetItemLocation(net, location) => {
            if let Some(item) = entity_of(world, net).filter(|item| inside(world, *item)) {
                world.set_item_location(storage, item, location);
            }
        }
        StorageMessage::InsertItemIntoLocation(location) => {
            let Some(held) = world.active_item(user) else {
                return;
            };
            if world.can_insert_storage(storage, held, true).is_ok() && world.can_drop(user) {
                world.storage_insert_at(storage, held, location, Some(user));
            }
        }
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<Storage>();
    world.register_net_component::<Storage>();
    astrocore_ui::UiWorldExt::register_ui_message::<StorageMessage>(world);
    world.subscribe::<Storage, astrocore_ecs::ComponentInit>(|world, storage, _| {
        world.ensure_container(storage, STORAGE_CONTAINER, ContainerKind::List);
    });
    if !side.is_server() {
        return;
    }
    // Попавшему в контейнер напрямую (начинка, слоты) — место в сетке; не влез — наружу
    // (`OnEntInserted`).
    world.subscribe::<Storage, astrocore_map::EntInsertedIntoContainer>(|world, storage, event| {
        if &*event.container != STORAGE_CONTAINER {
            return;
        }
        let item = event.entity;
        if world
            .get::<Storage>(storage)
            .is_some_and(|s| s.stored_items.contains_key(&item))
        {
            return;
        }
        match available_space(world, storage, item) {
            Some(location) => {
                if let Some(mut component) = world.get_mut::<Storage>(storage) {
                    component.stored_items.insert(item, location);
                }
            }
            None => {
                let _ = world.remove_from_container(item, None);
            }
        }
    });
    // Вынутый предмет освобождает место.
    world.subscribe::<Item, EntGotRemovedFromContainer>(|world, item, event| {
        if &*event.container != STORAGE_CONTAINER {
            return;
        }
        let removed = world
            .get_mut::<Storage>(event.owner)
            .is_some_and(|mut storage| storage.stored_items.remove(&item).is_some());
        if removed
            && let Some(sound) = world
                .get::<Storage>(event.owner)
                .and_then(|s| s.storage_remove_sound.clone())
        {
            world.play_entity(&sound, event.owner, None);
        }
    });
    world.subscribe::<Storage, InteractUsing>(|world, storage, event| {
        if event.handled
            || !world
                .get::<Storage>(storage)
                .is_some_and(|s| s.click_insert)
        {
            return;
        }
        if !world.can_interact(event.user, Some(storage)) {
            return;
        }
        world.player_insert_held(storage, event.user);
        // Обработано, даже если не влезло: провода не должны раскладываться (как в SS14).
        event.handled = true;
    });
    world.subscribe::<Storage, ActivateInWorld>(|world, storage, event| {
        if event.handled
            || !event.complex
            || !world
                .get::<Storage>(storage)
                .is_some_and(|s| s.open_on_activate)
        {
            return;
        }
        if world.ui_open_for(storage, STORAGE_UI_KEY, event.user) {
            world.close_ui(storage, STORAGE_UI_KEY, event.user);
        } else {
            world.open_storage_ui(storage, event.user, false);
        }
        event.handled = true;
    });
    world.subscribe::<Storage, GetVerbs>(|world, storage, event| {
        let shown = world.get::<Storage>(storage).is_some_and(|s| s.show_verb);
        if event.kind != VerbKind::Activation || !shown || !event.can_access || !event.can_interact
        {
            return;
        }
        let user = event.user;
        let open = world.ui_open_for(storage, STORAGE_UI_KEY, user);
        let (key, icon) = if open {
            (
                "comp-storage-verb-close-storage",
                "/Textures/Interface/VerbIcons/close.svg.192dpi.png",
            )
        } else {
            (
                "comp-storage-verb-open-storage",
                "/Textures/Interface/VerbIcons/open.svg.192dpi.png",
            )
        };
        let text = loc(world, key, &[]);
        let mut verb = Verb::new(VerbKind::Activation, text, move |world| {
            if open {
                world.close_ui(storage, STORAGE_UI_KEY, user);
            } else {
                world.open_storage_ui(storage, user, false);
            }
        });
        verb.icon = Some(icon.to_owned());
        event.verbs.push(verb);
    });
    // Клавиши открытия рюкзака и пояса (`HandleToggleSlotUI`).
    for (function, slot) in [(OPEN_BACKPACK, "back"), (OPEN_BELT, "belt")] {
        world.bind_input(function, move |world, args| {
            if args.state != BoundKeyState::Down {
                return false;
            }
            let Some(user) = args.player else {
                return false;
            };
            let Some(storage) = world.slot_entity(user, slot) else {
                return false;
            };
            if !world.can_interact(user, Some(storage)) {
                return false;
            }
            if world.ui_open_for(storage, STORAGE_UI_KEY, user) {
                world.close_ui(storage, STORAGE_UI_KEY, user);
            } else {
                world.open_storage_ui(storage, user, false);
            }
            false
        });
    }
    // К содержимому открытого хранилища можно дотянуться (`CanAccessViaStorage`).
    world.subscribe::<Storage, AccessViaContainer>(|world, storage, event| {
        if event.container == STORAGE_CONTAINER
            && world.ui_open_for(storage, STORAGE_UI_KEY, event.user)
        {
            event.accessible = true;
        }
    });
    world.subscribe::<Storage, BoundUiMessage<StorageMessage>>(handle_message);
}

#[cfg(test)]
mod tests;
