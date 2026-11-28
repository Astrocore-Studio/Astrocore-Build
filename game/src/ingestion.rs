//! Еда, питьё и таблетки: съесть в руке или накормить другого щелчком, с задержкой
//! (кормление прерывается движением); раствор съедобного (`food`, `drink`…)
//! уходит частями в желудок с наибольшим свободным объёмом, способный это переварить, с
//! реакцией тела на проглоченное; с повтором, пока есть что есть; съеденное целиком
//! удаляется и оставляет мусор. Рот закрывают маски и шлемы, полные хранилища и слоты мешают
//! есть, некоторые блюда требуют столовых приборов, которые иногда ломаются. Таблетку
//! глотают целиком. Вкус — по вкусам блюда и реагентов. Образец — `IngestionSystem` (с
//! `API`, `Blockers`, `Utensils`), `EdibleComponent`, `EdiblePrototype`, `PillComponent`,
//! `StomachComponent`, `StomachSystem`, `IngestionBlockerComponent`, `UtensilComponent`,
//! `FlavorProfileSystem`, `FlavorProfileComponent`, `FlavorPrototype` SS14 @ 2f4a2aad.
//!
//! Как в SS14: вкусы реагентов добавляются к вкусам блюда навсегда; `TryNewMaximum` задаёт
//! наименьшее; `forceFeedDelay` не используется — кормить другого так же быстро. Нет пока: сытости и жажды (они в 7.11), закрытых ёмкостей (`Openable`),
//! неснимаемых предметов (`Unremoveable`), складных и опускаемых масок, следов ДНК, луж.

use std::collections::HashMap;
use std::sync::Arc;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_console::{CVarDef, CVarFlags, ConfigWorldExt};
use astrocore_ecs::{EventWorldExt, LifecycleWorldExt, MapInit, RegistryWorldExt};
use astrocore_loc::{EntityNameWorldExt, LocArg};
use astrocore_map::{ContainerWorldExt, TransformWorldExt};
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Deserializer, Serialize};

use crate::Side;
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::body::BodyWorldExt;
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterWorldExt, DuplicateConditions};
use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::hands::HandsWorldExt;
use crate::interaction::{AfterInteract, InteractionWorldExt, UseInHand};
use crate::inventory::{Inventory, InventoryWorldExt, SlotFlags};
use crate::mobs::MobStateWorldExt;
use crate::popups::{PopupType, PopupWorldExt};
use crate::reactive::{ReactionMethod, do_entity_reaction};
use crate::solution::Solution;
use crate::solution_container::{self as containers, SolutionChanged};
use crate::verbs::{GetVerbs, Verb, VerbKind};
use crate::whitelist::EntityWhitelist;

/// Дальше этого не накормить (`MaxFeedDistance`), м.
pub const MAX_FEED_DISTANCE: f32 = 1.0;
/// Раствор желудка (`StomachSystem.DefaultSolutionName`).
pub const STOMACH_SOLUTION: &str = "stomach";
/// Ключ вида остатка еды (`FoodVisuals.Visual`).
pub const FOOD_VISUAL: &str = "enum.FoodVisuals.Visual";
/// Сколько вкусов называть (`flavor.limit`).
pub const FLAVOR_LIMIT: CVarDef<i32> = CVarDef::new(
    "flavor.limit",
    10,
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Сколько вкусов называть при еде.",
);

/// Столовые приборы (`UtensilType`), набор флагов.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
pub struct UtensilType(pub u8);

impl UtensilType {
    pub const NONE: Self = Self(0);
    pub const FORK: Self = Self(1);
    pub const SPOON: Self = Self(1 << 1);
    pub const KNIFE: Self = Self(1 << 2);

    fn named(name: &str) -> Option<Self> {
        match name.trim() {
            "None" => Some(Self::NONE),
            "Fork" => Some(Self::FORK),
            "Spoon" => Some(Self::SPOON),
            "Knife" => Some(Self::KNIFE),
            _ => None,
        }
    }

    pub fn has_any(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Имена флагов через запятую, как `Enum.ToString` с `[Flags]`.
    pub fn names(self) -> String {
        if self == Self::NONE {
            return "None".into();
        }
        [
            (Self::FORK, "Fork"),
            (Self::SPOON, "Spoon"),
            (Self::KNIFE, "Knife"),
        ]
        .into_iter()
        .filter(|(flag, _)| self.has_any(*flag))
        .map(|(_, name)| name)
        .collect::<Vec<_>>()
        .join(", ")
    }
}

impl<'de> Deserialize<'de> for UtensilType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            One(String),
            Many(Vec<String>),
        }
        let names = match Raw::deserialize(deserializer)? {
            Raw::One(text) => text.split([',', '|']).map(str::to_owned).collect(),
            Raw::Many(list) => list,
        };
        let mut value = Self::NONE;
        for name in names {
            let flag = Self::named(&name)
                .ok_or_else(|| serde::de::Error::custom(format!("нет прибора {name}")))?;
            value.0 |= flag.0;
        }
        Ok(value)
    }
}

fn food_solution() -> String {
    "food".into()
}
fn yes() -> bool {
    true
}
fn five() -> Option<FixedPoint2> {
    Some(FixedPoint2::new(5))
}
fn fork() -> UtensilType {
    UtensilType::FORK
}
fn one() -> f32 {
    1.0
}
fn three() -> f32 {
    3.0
}
fn food_type() -> String {
    "Food".into()
}

/// Съедобное (`EdibleComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edible {
    #[serde(default = "food_solution")]
    pub solution: String,
    #[serde(default = "yes")]
    pub destroy_on_empty: bool,
    #[serde(default)]
    pub trash: Vec<String>,
    /// `None` — всё сразу.
    #[serde(default = "five")]
    pub transfer_amount: Option<FixedPoint2>,
    #[serde(default = "fork")]
    pub utensil: UtensilType,
    #[serde(default)]
    pub utensil_required: bool,
    #[serde(default)]
    pub requires_special_digestion: bool,
    /// С.
    #[serde(default = "one")]
    pub delay: f32,
    #[serde(default = "three")]
    pub force_feed_delay: f32,
    #[serde(default = "yes")]
    pub require_dead: bool,
    #[serde(default)]
    pub use_sound: Option<SoundSpecifier>,
    #[serde(default = "food_type")]
    pub edible: String,
}

/// Таблетка: глотается целиком (`PillComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pill {
    #[serde(default)]
    pub pill_type: u32,
}

fn exclusive() -> bool {
    true
}

/// Желудок (`StomachComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stomach {
    #[serde(default)]
    pub special_digestible: Option<EntityWhitelist>,
    #[serde(default = "exclusive")]
    pub is_special_digestible_exclusive: bool,
}

/// Закрывает рот (`IngestionBlockerComponent`): маски, шлемы.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestionBlocker {
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn snap() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Items/snap.ogg")
}

/// Столовый прибор (`UtensilComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Utensil {
    #[serde(default)]
    pub types: UtensilType,
    #[serde(default)]
    pub break_chance: f32,
    #[serde(default = "snap")]
    pub break_sound: SoundSpecifier,
}

fn ignored_reagents() -> Vec<String> {
    ["Nutriment", "Vitamin", "Protein"]
        .map(str::to_owned)
        .to_vec()
}

/// Вкусы блюда (`FlavorProfileComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlavorProfile {
    #[serde(default)]
    pub flavors: Vec<String>,
    #[serde(default = "ignored_reagents")]
    pub ignore_reagents: Vec<String>,
}

/// Вид съедобного (`EdiblePrototype`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EdibleType {
    #[serde(default = "eating")]
    pub use_sound: SoundSpecifier,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub other_message: String,
    #[serde(default)]
    pub verb: String,
    #[serde(default)]
    pub noun: String,
    #[serde(default)]
    pub verb_name: String,
}

fn eating() -> SoundSpecifier {
    SoundSpecifier::collection("eating")
}

/// Вкус (`FlavorPrototype`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flavor {
    /// `Base` раньше `Complex`.
    #[serde(default)]
    pub flavor_type: FlavorType,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FlavorType {
    #[default]
    Base,
    Complex,
}

/// Виды съедобного, вкусы и вкусы реагентов. Ресурс `World`.
#[derive(Resource, Debug, Default)]
pub struct IngestionData {
    edibles: HashMap<String, Option<Arc<EdibleType>>>,
    flavors: HashMap<String, Option<Arc<Flavor>>>,
    /// Реагент → (вкус, наименьшее количество для вкуса).
    reagent_flavors: HashMap<String, (Option<String>, FixedPoint2)>,
}

fn data(world: &mut World) -> bevy_ecs::world::Mut<'_, IngestionData> {
    world.get_resource_or_insert_with(IngestionData::default)
}

fn proto<T: serde::de::DeserializeOwned>(world: &World, kind: &str, id: &str) -> Option<Arc<T>> {
    let node = world
        .get_resource::<Prototypes>()?
        .resolved_node(kind, id)?;
    astrocore_data::from_node(node).ok().map(Arc::new)
}

/// Вид съедобного (`ProtoMan.Index<EdiblePrototype>`).
pub fn edible_type(world: &mut World, id: &str) -> Option<Arc<EdibleType>> {
    if let Some(found) = world
        .get_resource::<IngestionData>()
        .and_then(|d| d.edibles.get(id).cloned())
    {
        return found;
    }
    let parsed = proto::<EdibleType>(world, "edible", id);
    data(world).edibles.insert(id.to_owned(), parsed.clone());
    parsed
}

fn flavor(world: &mut World, id: &str) -> Option<Arc<Flavor>> {
    if let Some(found) = world
        .get_resource::<IngestionData>()
        .and_then(|d| d.flavors.get(id).cloned())
    {
        return found;
    }
    let parsed = proto::<Flavor>(world, "flavor", id);
    data(world).flavors.insert(id.to_owned(), parsed.clone());
    parsed
}

fn reagent_flavor(world: &mut World, reagent: &str) -> (Option<String>, FixedPoint2) {
    if let Some(found) = world
        .get_resource::<IngestionData>()
        .and_then(|d| d.reagent_flavors.get(reagent).cloned())
    {
        return found;
    }
    let node = world
        .get_resource::<Prototypes>()
        .and_then(|p| p.resolved_node("reagent", reagent).cloned());
    let parsed = match node {
        Some(node) => (
            crate::entity_effects::text(&node, "flavor"),
            crate::entity_effects::field(&node, "flavorMinimum")
                .unwrap_or(FixedPoint2::from_cents(10)),
        ),
        None => (None, FixedPoint2::from_cents(10)),
    };
    data(world)
        .reagent_flavors
        .insert(reagent.to_owned(), parsed.clone());
    parsed
}

/// Можно ли вообще съесть (`IngestibleEvent`). На съедобном.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Ingestible {
    pub cancelled: bool,
}

/// Попытка съесть (`AttemptIngestEvent`): `ingest` — всерьёз, иначе только проверка. На
/// том, кто ест.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttemptIngest {
    pub user: Entity,
    pub ingested: Entity,
    pub ingest: bool,
    pub handled: bool,
}

/// Открыт ли рот (`IngestionAttemptEvent`): на том, кто ест, и на его одежде в слотах.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IngestionAttempt {
    pub target_slots: SlotFlags,
    pub cancelled: bool,
    pub blocker: Option<Entity>,
}

/// Чем и сколько есть (`EdibleEvent`). На съедобном.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdibleQuery {
    pub user: Entity,
    pub solution: Option<Entity>,
    /// С.
    pub time: Option<f32>,
    pub cancelled: bool,
}

/// Переваривается ли (`IsDigestibleEvent`). На съедобном.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct IsDigestible {
    pub digestible: bool,
    pub special_digestion: bool,
    pub universal: bool,
}

impl IsDigestible {
    pub fn add_digestible(&mut self, special: bool) {
        self.special_digestion = special;
        self.digestible = true;
    }

    pub fn universal_digestion(&mut self) {
        self.universal = true;
        self.digestible = true;
    }
}

/// Сколько отдать (`BeforeIngestedEvent`). На съедобном и на том, кто ест.
#[derive(Debug, Clone, PartialEq)]
pub struct BeforeIngested {
    pub min: FixedPoint2,
    pub max: FixedPoint2,
    pub solution: Option<Solution>,
    pub transfer: FixedPoint2,
    pub cancelled: bool,
    /// Раствор возвращается обратно (стопки).
    pub refresh: bool,
}

impl BeforeIngested {
    pub fn try_new_minimum(&mut self, min: FixedPoint2) -> bool {
        if min > self.max {
            return false;
        }
        self.min = min;
        true
    }

    /// Как в SS14: задаёт наименьшее, а не наибольшее.
    pub fn try_new_maximum(&mut self, max: FixedPoint2) -> bool {
        if max < self.min {
            return false;
        }
        self.min = max;
        true
    }
}

/// Ест (`IngestingEvent`). На том, кто ест.
#[derive(Debug, Clone, PartialEq)]
pub struct Ingesting {
    pub food: Entity,
    pub split: Solution,
    pub force_fed: bool,
}

/// Съедено (`IngestedEvent`). На съедобном.
#[derive(Debug, Clone, PartialEq)]
pub struct Ingested {
    pub user: Entity,
    pub target: Entity,
    pub split: Solution,
    pub force_fed: bool,
    pub satiated: bool,
    pub destroy: bool,
    pub repeat: bool,
    pub handled: bool,
}

/// Съедено целиком (`FullyEatenEvent`). На съедобном.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FullyEaten {
    pub user: Entity,
}

/// Нужные приборы (`GetUtensilsEvent`). На съедобном.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GetUtensils {
    pub types: UtensilType,
    pub required_types: UtensilType,
}

impl GetUtensils {
    pub fn add_required_types(&mut self, types: UtensilType) {
        self.required_types.0 |= types.0;
        self.types.0 |= types.0;
    }
}

/// Изменить вкусы (`FlavorProfileModificationEvent`). Широковещательно, на блюде и на
/// том, кто ест.
#[derive(Debug, Clone, PartialEq)]
pub struct FlavorProfileModification {
    pub user: Entity,
    pub flavors: Vec<String>,
}

/// Действие «есть» (`EatingDoAfterEvent`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EatingDoAfter;

fn name(world: &World, entity: Entity) -> LocArg {
    world.entity_arg(entity).into()
}

fn popup_for(world: &mut World, key: &str, args: &[(&str, LocArg)], on: Entity, user: Entity) {
    let text = loc(world, key, args);
    world.popup_entity_for(&text, on, user, PopupType::Small);
}

fn solution_volume(world: &World, soln: Entity) -> FixedPoint2 {
    containers::solution(world, soln).map_or(FixedPoint2::ZERO, |s| s.volume)
}

fn pretty_solution(solution: &Solution) -> String {
    solution
        .contents
        .iter()
        .map(|(id, quantity)| format!("{}: {quantity}", id.prototype))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Глагол вида съедобного (`GetEdibleVerb`).
pub fn edible_verb(world: &mut World, entity: Entity) -> String {
    let Some(kind) = world.get::<Edible>(entity).map(|e| e.edible.clone()) else {
        return loc(world, "edible-verb-edible", &[]);
    };
    let key = edible_type(world, &kind)
        .map(|t| t.verb.clone())
        .unwrap_or_default();
    loc(world, &key, &[])
}

/// Раствор съедобного (`EdibleVolume`).
pub fn edible_volume(world: &mut World, entity: Entity) -> FixedPoint2 {
    let Some(name) = world.get::<Edible>(entity).map(|e| e.solution.clone()) else {
        return FixedPoint2::ZERO;
    };
    containers::try_get_solution(world, entity, &name)
        .map_or(FixedPoint2::ZERO, |s| solution_volume(world, s))
}

pub fn is_empty(world: &mut World, entity: Entity) -> bool {
    edible_volume(world, entity) == FixedPoint2::ZERO
}

/// Проверить или начать (`AttemptIngest`).
fn attempt_ingest(
    world: &mut World,
    user: Entity,
    target: Entity,
    ingested: Entity,
    ingest: bool,
) -> bool {
    let mut blocked = Ingestible::default();
    world.raise(ingested, &mut blocked);
    if blocked.cancelled {
        return false;
    }
    let mut attempt = AttemptIngest {
        user,
        ingested,
        ingest,
        handled: false,
    };
    world.raise(target, &mut attempt);
    attempt.handled
}

/// Съесть самому (`TryIngest`).
pub fn try_ingest(world: &mut World, user: Entity, target: Entity, ingested: Entity) -> bool {
    attempt_ingest(world, user, target, ingested, true)
}

/// Может ли съесть (`CanIngest`).
pub fn can_ingest(world: &mut World, user: Entity, ingested: Entity) -> bool {
    attempt_ingest(world, user, user, ingested, false)
}

/// Открыт ли рот цели и достаёт ли до неё (`HasMouthAvailable`).
pub fn has_mouth_available(
    world: &mut World,
    user: Entity,
    target: Entity,
    flags: SlotFlags,
) -> bool {
    let in_range = match (world.map_coordinates(user), world.map_coordinates(target)) {
        (Some(a), Some(b)) => {
            a.map == b.map && a.z == b.z && a.position.distance(b.position) <= MAX_FEED_DISTANCE
        }
        _ => false,
    };
    if !in_range {
        let text = loc(
            world,
            "interaction-system-user-interaction-cannot-reach",
            &[],
        );
        world.popup_entity_for(&text, user, user, PopupType::Small);
        return false;
    }
    let mut attempt = IngestionAttempt {
        target_slots: flags,
        cancelled: false,
        blocker: None,
    };
    world.raise(target, &mut attempt);
    // Ретрансляция одежде в слотах (`InventoryRelayedEvent`).
    let slots: Vec<(String, SlotFlags)> = world
        .get::<Inventory>(target)
        .map(|inventory| {
            inventory
                .slots
                .iter()
                .map(|slot| (slot.name.clone(), slot.slot_flags))
                .collect()
        })
        .unwrap_or_default();
    for (slot, slot_flags) in slots {
        if !slot_flags.intersects(flags) {
            continue;
        }
        if let Some(item) = world.slot_entity(target, &slot) {
            on_blocker_relayed(world, item, &mut attempt);
        }
    }
    if !attempt.cancelled {
        return true;
    }
    if let Some(blocker) = attempt.blocker {
        popup_for(
            world,
            "ingestion-remove-mask",
            &[("entity", name(world, blocker))],
            target,
            user,
        );
    }
    false
}

fn on_blocker_relayed(world: &mut World, item: Entity, attempt: &mut IngestionAttempt) {
    if attempt.cancelled
        || !world
            .get::<IngestionBlocker>(item)
            .is_some_and(|b| b.enabled)
    {
        return;
    }
    attempt.cancelled = true;
    attempt.blocker = Some(item);
}

/// Доступен ли раствор и сколько есть (`CanAccessSolution`).
fn can_access_solution(
    world: &mut World,
    ingested: Entity,
    user: Entity,
) -> Option<(Entity, Option<f32>)> {
    let mut query = EdibleQuery {
        user,
        solution: None,
        time: None,
        cancelled: false,
    };
    world.raise(ingested, &mut query);
    let Some(solution) = query.solution else {
        popup_for(
            world,
            "ingestion-try-use-is-empty",
            &[("entity", name(world, ingested))],
            ingested,
            user,
        );
        return None;
    };
    (!query.cancelled).then_some((solution, query.time))
}

/// Можно ли накормить (`CanConsume`).
pub fn can_consume(
    world: &mut World,
    user: Entity,
    target: Entity,
    ingested: Entity,
) -> Option<(Entity, Option<f32>)> {
    if !has_mouth_available(
        world,
        user,
        target,
        SlotFlags(SlotFlags::HEAD.0 | SlotFlags::MASK.0),
    ) {
        return None;
    }
    can_access_solution(world, ingested, user)
}

fn on_edible_query(world: &mut World, entity: Entity, query: &mut EdibleQuery) {
    if query.cancelled || query.solution.is_some() {
        return;
    }
    let Some(edible) = world.get::<Edible>(entity).cloned() else {
        return;
    };
    if edible.utensil_required && !has_required_utensils(world, query.user, edible.utensil) {
        query.cancelled = true;
        return;
    }
    let solution = containers::try_get_solution(world, entity, &edible.solution);
    let empty = is_empty(world, entity);
    match solution {
        Some(soln) if !(empty && !edible.destroy_on_empty) => {
            query.solution = Some(soln);
            // Время складывается.
            query.time = Some(query.time.unwrap_or(0.0) + edible.delay);
        }
        _ => {
            query.solution = solution;
            query.cancelled = true;
            popup_for(
                world,
                "ingestion-try-use-is-empty",
                &[("entity", name(world, entity))],
                entity,
                query.user,
            );
        }
    }
}

fn used_storage(world: &mut World, entity: Entity, query: &mut EdibleQuery) {
    let verb = edible_verb(world, entity);
    popup_for(
        world,
        "edible-has-used-storage",
        &[("food", name(world, entity)), ("verb", verb.into())],
        query.user,
        query.user,
    );
    query.cancelled = true;
}

/// Переваривает ли хоть один желудок (`IsDigestibleBy` по списку); `popup` — сказать,
/// что не переваривается.
fn is_digestible_by_any(world: &mut World, food: Entity, stomachs: &[Entity]) -> (bool, bool) {
    let mut event = IsDigestible::default();
    world.raise(food, &mut event);
    if !event.digestible {
        return (false, false);
    }
    if event.universal {
        return (true, false);
    }
    for stomach in stomachs {
        let Some(component) = world.get::<Stomach>(*stomach) else {
            continue;
        };
        let pass = component
            .special_digestible
            .as_ref()
            .is_some_and(|w| w.is_valid(world, food));
        if event.special_digestion {
            if pass {
                return (true, false);
            }
        } else if component.special_digestible.is_none()
            || !component.is_special_digestible_exclusive
            || pass
        {
            return (true, false);
        }
    }
    (false, true)
}

/// Переваривает ли этот желудок (`IsDigestibleBy`).
fn is_digestible_by(world: &mut World, food: Entity, stomach: Entity) -> bool {
    let mut event = IsDigestible::default();
    world.raise(food, &mut event);
    if !event.digestible {
        return false;
    }
    if event.universal {
        return true;
    }
    let Some(component) = world.get::<Stomach>(stomach) else {
        return false;
    };
    let pass = component
        .special_digestible
        .as_ref()
        .is_some_and(|w| w.is_valid(world, food));
    if event.special_digestion {
        return pass;
    }
    component.special_digestible.is_none() || !component.is_special_digestible_exclusive || pass
}

fn stomachs_of(world: &World, body: Entity) -> Vec<Entity> {
    world
        .organs(body)
        .into_iter()
        .filter(|o| world.get::<Stomach>(*o).is_some())
        .collect()
}

/// Начать есть (`OnTryIngest`): на теле с желудками.
fn on_try_ingest(world: &mut World, body: Entity, event: &mut AttemptIngest) {
    let food = event.ingested;
    let user = event.user;
    let force_fed = user != body;
    let stomachs = stomachs_of(world, body);
    if stomachs.is_empty() {
        return;
    }
    let (digestible, popup) = is_digestible_by_any(world, food, &stomachs);
    if !digestible {
        if !event.ingest || !popup {
            return;
        }
        if force_fed {
            popup_for(
                world,
                "ingestion-cant-digest-other",
                &[("target", name(world, body)), ("entity", name(world, food))],
                body,
                user,
            );
        } else {
            popup_for(
                world,
                "ingestion-cant-digest",
                &[("entity", name(world, food))],
                body,
                body,
            );
        }
        return;
    }
    if !event.ingest {
        event.handled = true;
        return;
    }
    let Some((solution, time)) = can_consume(world, user, body, food) else {
        return;
    };
    let args = edible_do_after_args(world, user, body, food, time.unwrap_or(0.0));
    if world.try_start_do_after(args, EatingDoAfter) == crate::do_after::DoAfterStart::Refused {
        return;
    }
    event.handled = true;
    let contents = containers::solution(world, solution)
        .map(pretty_solution)
        .unwrap_or_default();
    let (user_name, body_name, food_name) = (
        world.entity_name(user),
        world.entity_name(body),
        world.entity_name(food),
    );
    if force_fed {
        let verb = edible_verb(world, food);
        popup_for(
            world,
            "edible-force-feed",
            &[("user", name(world, user)), ("verb", verb.into())],
            user,
            body,
        );
        add_admin_log(
            world,
            LogType::ForceFeed,
            LogImpact::Medium,
            &format!("{user_name} is forcing {body_name} to eat {food_name} {contents}"),
            &[],
            &[],
        );
    } else {
        add_admin_log(
            world,
            LogType::Ingestion,
            LogImpact::Low,
            &format!("{body_name} is eating {food_name} {contents}"),
            &[],
            &[],
        );
    }
}

/// Параметры действия «есть» (`GetEdibleDoAfterArgs`).
fn edible_do_after_args(
    world: &World,
    user: Entity,
    target: Entity,
    food: Entity,
    delay: f32,
) -> DoAfterArgs {
    let force_feed = user != target;
    let mut args = DoAfterArgs::new(user, f64::from(delay), Some(food), None, Some(target));
    args.break_on_hand_change = false;
    args.break_on_move = force_feed;
    args.break_on_damage = true;
    args.movement_threshold = 0.01;
    args.distance_threshold = Some(MAX_FEED_DISTANCE);
    args.duplicate_condition = DuplicateConditions {
        same_target: false,
        same_tool: false,
        same_event: true,
    };
    args.need_hand = force_feed || world.hand_holding(user, food).is_some();
    args
}

fn cannot_ingest_more(
    world: &mut World,
    body: Entity,
    food: Entity,
    user: Entity,
    force_fed: bool,
) {
    let verb = edible_verb(world, food);
    popup_for(
        world,
        "ingestion-you-cannot-ingest-any-more",
        &[("verb", verb.clone().into())],
        body,
        body,
    );
    if force_fed {
        popup_for(
            world,
            "ingestion-other-cannot-ingest-any-more",
            &[("target", name(world, body)), ("verb", verb.into())],
            food,
            user,
        );
    }
}

/// Кусок съеден (`OnEatingDoAfter`).
fn on_eating_do_after(world: &mut World, body: Entity, event: &mut DoAfterDone<EatingDoAfter>) {
    if event.cancelled || event.handled {
        return;
    }
    let Some(food) = event.target else {
        return;
    };
    let user = event.user;
    let mut blocked = Ingestible::default();
    world.raise(food, &mut blocked);
    if blocked.cancelled {
        return;
    }
    let Some((solution, _)) = can_consume(world, user, body, food) else {
        return;
    };
    let stomachs = stomachs_of(world, body);
    if stomachs.is_empty() {
        return;
    }
    let force_fed = user != body;
    let mut highest = FixedPoint2::ZERO;
    let mut chosen = None;
    for stomach in stomachs {
        let Some(soln) = containers::try_get_solution(world, stomach, STOMACH_SOLUTION) else {
            continue;
        };
        let available =
            containers::solution(world, soln).map_or(FixedPoint2::ZERO, |s| s.available_volume());
        if available <= highest || !is_digestible_by(world, food, stomach) {
            continue;
        }
        chosen = Some(stomach);
        highest = available;
    }
    let Some(stomach) = chosen else {
        cannot_ingest_more(world, body, food, user, force_fed);
        return;
    };
    let mut before = BeforeIngested {
        min: FixedPoint2::ZERO,
        max: highest,
        solution: containers::solution(world, solution).cloned(),
        transfer: FixedPoint2::ZERO,
        cancelled: false,
        refresh: false,
    };
    world.raise(food, &mut before);
    world.raise(body, &mut before);
    if before.cancelled || before.min > before.max {
        cannot_ingest_more(world, body, food, user, force_fed);
        return;
    }
    let transfer = before.transfer.clamp(before.min, before.max);
    let split = containers::split_solution(world, solution, transfer);
    if before.refresh {
        containers::try_add_solution(world, solution, &split);
    }
    let mut ingesting = Ingesting {
        food,
        split: split.clone(),
        force_fed,
    };
    world.raise(body, &mut ingesting);
    do_entity_reaction(world, body, &split, ReactionMethod::Ingestion);
    let full = before.transfer >= before.max;
    let mut after = Ingested {
        user,
        target: body,
        split: split.clone(),
        force_fed,
        satiated: full,
        destroy: false,
        repeat: false,
        handled: false,
    };
    world.raise(food, &mut after);
    try_transfer_to_stomach(world, stomach, &split);
    if !after.destroy {
        if full {
            return;
        }
        event.repeat = after.repeat;
        return;
    }
    let mut attempt = crate::destructible::DestructionAttempt::default();
    world.raise(food, &mut attempt);
    if attempt.cancelled {
        return;
    }
    world.raise(food, &mut FullyEaten { user });
    world.raise(food, &mut crate::destructible::Destroyed);
    world.queue_delete(food);
    event.repeat = false;
}

/// В желудок, только целиком (`StomachSystem.TryTransferSolution`).
pub fn try_transfer_to_stomach(world: &mut World, stomach: Entity, solution: &Solution) -> bool {
    let Some(soln) = containers::try_get_solution(world, stomach, STOMACH_SOLUTION) else {
        return false;
    };
    if !containers::solution(world, soln).is_some_and(|s| s.can_add_solution(solution)) {
        return false;
    }
    containers::try_add_solution(world, soln, solution);
    true
}

/// Съедено: звук, вкус, сообщения, приборы (`OnEdibleIngested`).
fn on_edible_ingested(world: &mut World, food: Entity, event: &mut Ingested) {
    if event.handled {
        return;
    }
    event.handled = true;
    let Some(edible) = world.get::<Edible>(food).cloned() else {
        return;
    };
    let kind = edible_type(world, &edible.edible);
    let sound = edible
        .use_sound
        .clone()
        .or_else(|| kind.as_ref().map(|k| k.use_sound.clone()));
    if let Some(sound) = sound {
        world.play_entity(&sound, event.target, None);
    }
    let flavors = localized_flavors_message(world, food, event.target, Some(&event.split));
    let verb = kind
        .as_ref()
        .map(|k| loc(world, &k.verb, &[]))
        .unwrap_or_default();
    let satiated = if event.satiated { "true" } else { "false" };
    let (user, target) = (event.user, event.target);
    if event.force_fed {
        let text = loc(
            world,
            "edible-force-feed-success",
            &[
                ("user", name(world, user)),
                ("verb", verb.clone().into()),
                ("flavors", flavors.into()),
                ("satiated", satiated.into()),
            ],
        );
        // Как в SS14: сообщение кормлёному — на самом блюде.
        world.popup_entity_for(&text, food, food, PopupType::Small);
        popup_for(
            world,
            "edible-force-feed-success-user",
            &[("target", name(world, target)), ("verb", verb.into())],
            user,
            user,
        );
        let message = format!(
            "{} forced {} to eat {}",
            world.entity_name(food),
            world.entity_name(user),
            world.entity_name(food)
        );
        add_admin_log(
            world,
            LogType::ForceFeed,
            LogImpact::Medium,
            &message,
            &[],
            &[],
        );
    } else if let Some(kind) = &kind {
        let text = loc(
            world,
            &kind.message,
            &[
                ("food", name(world, food)),
                ("flavors", flavors.into()),
                ("satiated", satiated.into()),
            ],
        );
        let other = loc(world, &kind.other_message, &[]);
        world.popup_entity_split(&text, &other, user, user, PopupType::Small);
        let message = format!(
            "{} ate {}",
            world.entity_name(user),
            world.entity_name(food)
        );
        add_admin_log(
            world,
            LogType::Ingestion,
            LogImpact::Low,
            &message,
            &[],
            &[],
        );
    }
    if let Some(utensils) = try_get_utensils_for(world, user, food) {
        for utensil in utensils {
            try_break(world, utensil, user);
        }
    }
    if !is_empty(world, food) {
        event.repeat = !event.force_fed;
        return;
    }
    event.destroy = edible.destroy_on_empty;
}

/// Мусор после еды; в руке — в ту же руку (`SpawnTrash`).
pub fn spawn_trash(world: &mut World, food: Entity, user: Option<Entity>) {
    let Some(trash) = world.get::<Edible>(food).map(|e| e.trash.clone()) else {
        return;
    };
    let pickup = user.is_some_and(|user| {
        world
            .hand_holding(user, food)
            .is_some_and(|hand| world.try_drop(user, &hand, None, true))
    });
    for prototype in trash {
        let Some(spawned) = crate::entity_effects::spawn_next_to(world, &prototype, food) else {
            continue;
        };
        if pickup && let Some(user) = user {
            world.try_pickup_any_hand(user, spawned, true);
        }
    }
}

/// Вкусы словами (`GetLocalizedFlavorsMessage`).
pub fn localized_flavors_message(
    world: &mut World,
    food: Entity,
    user: Entity,
    solution: Option<&Solution>,
) -> String {
    let limit = world.cvar(&FLAVOR_LIMIT).max(0) as usize;
    let (mut flavors, ignore) = match world.get::<FlavorProfile>(food) {
        Some(profile) => (
            profile.flavors.clone(),
            Some(profile.ignore_reagents.clone()),
        ),
        None => (Vec::new(), None),
    };
    if let Some(solution) = solution {
        let wanted = limit.saturating_sub(flavors.len());
        for flavor in flavors_from_reagents(world, solution, wanted, ignore.as_deref()) {
            if !flavors.contains(&flavor) {
                flavors.push(flavor);
            }
        }
    }
    // Как в SS14: множество вкусов блюда пополняется навсегда.
    if let Some(mut profile) = world.get_mut::<FlavorProfile>(food) {
        profile.flavors = flavors.clone();
    }
    let mut event = FlavorProfileModification { user, flavors };
    world.raise_broadcast(&mut event);
    world.raise(food, &mut event);
    world.raise(user, &mut event);
    if event.flavors.is_empty() {
        return loc(world, "flavor-profile-unknown", &[]);
    }
    flavors_to_message(world, &event.flavors)
}

fn flavors_from_reagents(
    world: &mut World,
    solution: &Solution,
    wanted: usize,
    ignore: Option<&[String]>,
) -> Vec<String> {
    let mut flavors: Vec<String> = Vec::new();
    for (reagent, quantity) in solution.reagent_prototypes() {
        if ignore.is_some_and(|list| list.contains(&reagent)) {
            continue;
        }
        if flavors.len() == wanted {
            break;
        }
        let (flavor, minimum) = reagent_flavor(world, &reagent);
        if quantity < minimum {
            continue;
        }
        if let Some(flavor) = flavor
            && !flavors.contains(&flavor)
        {
            flavors.push(flavor);
        }
    }
    flavors
}

fn flavors_to_message(world: &mut World, ids: &[String]) -> String {
    let mut flavors: Vec<Arc<Flavor>> = ids.iter().filter_map(|id| flavor(world, id)).collect();
    flavors.sort_by_key(|f| f.flavor_type);
    if flavors.len() == 1 && !flavors[0].description.is_empty() {
        let text = loc(world, &flavors[0].description, &[]);
        return loc(world, "flavor-profile", &[("flavor", text.into())]);
    }
    if flavors.len() > 1 {
        let last = loc(world, &flavors[flavors.len() - 1].description, &[]);
        let all = flavors[..flavors.len() - 1]
            .iter()
            .map(|f| loc(world, &f.description, &[]))
            .collect::<Vec<_>>()
            .join(", ");
        return loc(
            world,
            "flavor-profile-multiple",
            &[("flavors", all.into()), ("lastFlavor", last.into())],
        );
    }
    loc(world, "flavor-profile-unknown", &[])
}

/// Приборы в руках для блюда (`TryGetUtensils`): `None` — нужных нет.
fn try_get_utensils_for(world: &mut World, user: Entity, food: Entity) -> Option<Vec<Entity>> {
    let mut event = GetUtensils::default();
    world.raise(food, &mut event);
    try_get_utensils(world, user, event.types, event.required_types)
}

fn try_get_utensils(
    world: &mut World,
    user: Entity,
    types: UtensilType,
    required: UtensilType,
) -> Option<Vec<Entity>> {
    let mut utensils = Vec::new();
    if types == UtensilType::NONE || world.get::<crate::hands::Hands>(user).is_none() {
        return Some(utensils);
    }
    let mut used = UtensilType::NONE;
    for item in world.held_items(user) {
        let Some(utensil) = world.get::<Utensil>(item) else {
            continue;
        };
        if !utensil.types.has_any(types) || used.0 & utensil.types.0 == utensil.types.0 {
            continue;
        }
        used.0 |= utensil.types.0;
        utensils.push(item);
    }
    if required == UtensilType::NONE || used.0 & required.0 == required.0 {
        return Some(utensils);
    }
    let missing = UtensilType(required.0 ^ used.0);
    popup_for(
        world,
        "ingestion-you-need-to-hold-utensil",
        &[("utensil", missing.names().into())],
        user,
        user,
    );
    None
}

/// Есть ли нужные приборы (`HasRequiredUtensils`).
pub fn has_required_utensils(world: &mut World, user: Entity, types: UtensilType) -> bool {
    try_get_utensils(world, user, types, types).is_some()
}

/// Прибор ломается с шансом (`TryBreak`).
fn try_break(world: &mut World, utensil: Entity, user: Entity) {
    let Some(component) = world.get::<Utensil>(utensil).cloned() else {
        return;
    };
    if !crate::random::random(world).prob(component.break_chance) {
        return;
    }
    world.play_entity(&component.break_sound, user, None);
    world.queue_delete(utensil);
}

/// Есть прибором (`TryUseUtensil`).
fn try_use_utensil(world: &mut World, user: Entity, target: Entity, utensil: Entity) -> bool {
    let mut event = GetUtensils::default();
    world.raise(target, &mut event);
    let types = world
        .get::<Utensil>(utensil)
        .map_or(UtensilType::NONE, |u| u.types);
    if event.types != UtensilType::NONE && !event.types.has_any(types) {
        let verb = edible_verb(world, target);
        popup_for(
            world,
            "ingestion-try-use-wrong-utensil",
            &[
                ("verb", verb.into()),
                ("food", name(world, target)),
                ("utensil", name(world, utensil)),
            ],
            user,
            user,
        );
        return true;
    }
    if !world.in_range_unobstructed(user, target, crate::interaction::INTERACTION_RANGE) {
        return true;
    }
    try_ingest(world, user, user, target)
}

/// Глагол «съесть» для чужого блюда (`AddEdibleVerbs`, `TryGetIngestionVerb`).
fn edible_verbs(world: &mut World, food: Entity, event: &mut GetVerbs) {
    let user = event.user;
    if event.kind != VerbKind::Alternative
        || food == user
        || !event.can_interact
        || !event.can_access
    {
        return;
    }
    if !can_ingest(world, user, food) {
        return;
    }
    let Some(kind) = world
        .get::<Edible>(food)
        .map(|e| e.edible.clone())
        .and_then(|id| edible_type(world, &id))
    else {
        return;
    };
    let mut verb = Verb::new(
        VerbKind::Alternative,
        loc(world, &kind.verb_name, &[]),
        move |world| {
            try_ingest(world, user, user, food);
        },
    );
    verb.priority = 2;
    event.verbs.push(verb);
}

fn update_appearance(world: &mut World, food: Entity) {
    if world.get::<astrocore_net::Appearance>(food).is_none() {
        return;
    }
    let volume = edible_volume(world, food).float();
    world.set_appearance(food, FOOD_VISUAL, volume);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Edible>();
    world.register_data_component::<Pill>();
    world.register_net_serde::<Pill>();
    world.register_data_component::<Stomach>();
    world.register_data_component::<IngestionBlocker>();
    world.register_data_component::<Utensil>();
    world.register_data_component::<FlavorProfile>();
    world.register_cvar(&FLAVOR_LIMIT);
    if !side.is_server() {
        return;
    }
    world.subscribe::<Edible, MapInit>(|world, food, _| {
        if let Some(name) = world.get::<Edible>(food).map(|e| e.solution.clone()) {
            containers::ensure_solution(world, food, &name);
        }
        update_appearance(world, food);
    });
    world.subscribe::<Edible, SolutionChanged>(|world, food, _| update_appearance(world, food));
    world.subscribe::<Edible, UseInHand>(|world, food, event| {
        if event.handled {
            return;
        }
        event.handled = try_ingest(world, event.user, event.user, food);
    });
    world.subscribe::<Edible, AfterInteract>(|world, food, event| {
        if event.handled || !event.can_reach {
            return;
        }
        let Some(target) = event.target else {
            return;
        };
        event.handled = try_ingest(world, event.user, target, food);
    });
    world.subscribe::<Edible, BeforeIngested>(|world, food, event| {
        if event.cancelled {
            return;
        }
        let Some(volume) = event.solution.as_ref().map(|s| s.volume) else {
            return;
        };
        let amount = world.get::<Edible>(food).and_then(|e| e.transfer_amount);
        event.transfer = amount.unwrap_or(volume);
    });
    world.subscribe::<Edible, Ingested>(on_edible_ingested);
    world.subscribe::<Edible, FullyEaten>(|world, food, event| {
        spawn_trash(world, food, Some(event.user));
    });
    world.subscribe::<Edible, GetVerbs>(edible_verbs);
    world.subscribe::<Edible, EdibleQuery>(on_edible_query);
    world.subscribe::<Edible, IsDigestible>(|world, food, event| {
        let require_dead = world.get::<Edible>(food).is_some_and(|e| e.require_dead);
        if require_dead && world.is_mob_alive(food) {
            return;
        }
        let special = world
            .get::<Edible>(food)
            .is_some_and(|e| e.requires_special_digestion);
        event.add_digestible(special);
    });
    world.subscribe::<Edible, GetUtensils>(|world, food, event| {
        let Some(edible) = world.get::<Edible>(food) else {
            return;
        };
        if edible.utensil == UtensilType::NONE {
            return;
        }
        if edible.utensil_required {
            event.add_required_types(edible.utensil);
        } else {
            event.types.0 |= edible.utensil.0;
        }
    });
    world.subscribe::<crate::storage::Storage, EdibleQuery>(|world, entity, query| {
        if query.cancelled {
            return;
        }
        let full = world
            .container(entity, crate::storage::STORAGE_CONTAINER)
            .is_some_and(|c| !c.entities().is_empty());
        if full {
            used_storage(world, entity, query);
        }
    });
    world.subscribe::<crate::item_slots::ItemSlots, EdibleQuery>(|world, entity, query| {
        if query.cancelled {
            return;
        }
        let slots: Vec<String> = world
            .get::<crate::item_slots::ItemSlots>(entity)
            .map(|s| s.slots.keys().cloned().collect())
            .unwrap_or_default();
        if slots
            .iter()
            .any(|slot| crate::item_slots::slot_item(world, entity, slot).is_some())
        {
            used_storage(world, entity, query);
        }
    });
    world.subscribe::<containers::DrainableSolution, IsDigestible>(|_, _, event| {
        event.universal_digestion();
    });
    // Таблетку — только целиком.
    world.subscribe::<Pill, BeforeIngested>(|_, _, event| {
        if event.cancelled {
            return;
        }
        let Some(volume) = event.solution.as_ref().map(|s| s.volume) else {
            return;
        };
        if !event.try_new_minimum(volume) {
            event.cancelled = true;
        }
    });
    world.subscribe::<IngestionBlocker, IngestionAttempt>(|world, entity, event| {
        if !event.cancelled
            && world
                .get::<IngestionBlocker>(entity)
                .is_some_and(|b| b.enabled)
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<crate::body::Body, AttemptIngest>(on_try_ingest);
    world.subscribe::<crate::body::Body, DoAfterDone<EatingDoAfter>>(on_eating_do_after);
    world.subscribe::<Utensil, AfterInteract>(|world, utensil, event| {
        if event.handled || !event.can_reach {
            return;
        }
        let Some(target) = event.target else {
            return;
        };
        event.handled = try_use_utensil(world, event.user, target, utensil);
    });
}

#[cfg(test)]
mod tests;
