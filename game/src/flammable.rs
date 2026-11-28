//! Горение сущностей: «стопки огня», поджог пламенем клетки, горячим предметом, снарядом
//! или ударом, огонь греет и ранит, перекидывается между горящими телами, гаснет без
//! кислорода и со временем; горящее поджигает газ вокруг. Образец — `FlammableComponent`,
//! `FlammableSystem`, `IgnitionSourceComponent`, `IgnitionSourceSystem`,
//! `IgniteOnCollideComponent`, `IgniteOnMeleeHitComponent`, `IgniteOnHeatDamageComponent`,
//! `FireProtectionComponent` и `GetFireProtectionEvent` SS14 @ 2f4a2aad.
//!
//! Значок «горит» и сопротивление огню нажатием на него (`ResistFireAlert`: две секунды огонь
//! гаснет в 15 раз быстрее). Нет пока: оглушения при сопротивлении (оглушений нет — с
//! безопасностью); тушения огнетушителем и водой (`ExtinguishOnInteract`, реакции
//! реагентов) — с жидкостями. Включённые горячие предметы (`ItemToggleHot`) — в `welder.rs`.

use std::collections::BTreeMap;
use std::time::Duration;

use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update};
use astrocore_map::{TileCoord, Transform, TransformWorldExt};
use astrocore_net::AppearanceWorldExt;
use astrocore_physics::{
    CollisionGroups, Fixture, Fixtures, Physics, PhysicsWorldExt, Shape, StartCollide, group_bits,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::atmos::gas::OXYGEN;
use crate::atmos::{TileFire, hotspot_expose_at};
use crate::combat::{Attacked, MeleeHit};
use crate::damage::{DamageChanged, DamageSpecifier, DamageableWorldExt, Rejuvenate};
use crate::entity_effects::EffectsWorldExt;
use crate::fixed_point::FixedPoint2;
use crate::interaction::InteractUsing;
use crate::temperature::{change_heat, worn_items};
use crate::throwing::Land;

/// Тепло горения одной «стопки» за секунду (`FirestackEnergy`), Дж.
const FIRESTACK_ENERGY: f32 = 37_500.0;
/// Обновление раз в секунду (`UpdateTime`).
const UPDATE_TIME: Duration = Duration::from_secs(1);

/// Горит ли сущность (`FireVisuals.OnFire`).
pub const ON_FIRE: &str = "enum.FireVisuals.OnFire";
/// Сколько «стопок» (`FireVisuals.FireStacks`).
pub const FIRE_STACKS: &str = "enum.FireVisuals.FireStacks";
/// Вид включённого предмета (`ToggleableVisuals.Enabled`): спички и свечи.
pub const TOGGLEABLE_ENABLED: &str = "enum.ToggleableVisuals.Enabled";

fn resist_time() -> f32 {
    2.0
}
fn max_stacks() -> f32 {
    10.0
}
fn min_stacks() -> f32 {
    -10.0
}
fn fixture_id() -> String {
    "flammable".into()
}
fn min_ignition() -> f32 {
    373.15
}
fn collision_shape() -> Shape {
    Shape::PhysShapeCircle {
        radius: 0.35,
        position: astrocore_math::Vec2::ZERO,
    }
}
fn yes() -> bool {
    true
}
fn stacks_on_ignite() -> f32 {
    2.0
}
fn fade() -> f32 {
    -0.1
}

/// Может гореть (`FlammableComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flammable {
    #[serde(default = "resist_time")]
    pub resist_time: f32,
    #[serde(default)]
    pub on_fire: bool,
    #[serde(default)]
    pub fire_stacks: f32,
    #[serde(default = "max_stacks")]
    pub maximum_fire_stacks: f32,
    #[serde(default = "min_stacks")]
    pub minimum_fire_stacks: f32,
    #[serde(rename = "flammableFixtureID", default = "fixture_id")]
    pub flammable_fixture_id: String,
    #[serde(default = "min_ignition")]
    pub min_ignition_temperature: f32,
    #[serde(default)]
    pub fire_spread: bool,
    #[serde(default)]
    pub can_resist_fire: bool,
    pub damage: DamageSpecifier,
    #[serde(default = "collision_shape")]
    pub flammable_collision_shape: Shape,
    #[serde(default)]
    pub always_combustible: bool,
    #[serde(default = "yes")]
    pub can_extinguish: bool,
    #[serde(default = "stacks_on_ignite")]
    pub firestacks_on_ignite: f32,
    #[serde(default = "fade")]
    pub firestack_fade: f32,
    #[serde(skip)]
    pub next_update: Duration,
    /// Когда кончится сопротивление (`ResistCompleteTime`).
    #[serde(skip)]
    pub resist_complete_time: Option<Duration>,
    #[serde(default = "fire_alert")]
    pub fire_alert: String,
}

fn fire_alert() -> String {
    "Fire".into()
}

impl Flammable {
    /// Сопротивляется огню (`Resisting`).
    pub fn resisting(&self) -> bool {
        self.resist_complete_time.is_some()
    }
}

/// Сопротивляться огню (`Resist`): если горит, ещё не сопротивляется и может действовать.
pub fn resist(world: &mut World, entity: Entity) {
    let Some(flammable) = world.get::<Flammable>(entity).cloned() else {
        return;
    };
    if !flammable.on_fire
        || flammable.resisting()
        || !crate::action_blocker::ActionBlockerWorldExt::can_interact(world, entity, None)
    {
        return;
    }
    let end = now(world) + Duration::from_secs_f32(flammable.resist_time.max(0.0));
    if let Some(mut flammable) = world.get_mut::<Flammable>(entity) {
        flammable.resist_complete_time = Some(end);
    }
    let text = crate::examine::loc(world, "flammable-component-resist-message", &[]);
    crate::popups::PopupWorldExt::popup_entity_for(
        world,
        &text,
        entity,
        entity,
        crate::popups::PopupType::Small,
    );
}

fn ignition_temperature() -> f32 {
    700.0
}

/// Поджигает газ вокруг, пока горит (`IgnitionSourceComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IgnitionSource {
    #[serde(default)]
    pub ignited: bool,
    #[serde(default = "ignition_temperature")]
    pub temperature: f32,
}

impl Default for IgnitionSource {
    fn default() -> Self {
        Self {
            ignited: false,
            temperature: ignition_temperature(),
        }
    }
}

fn one() -> i32 {
    1
}
fn ignition_fixture() -> String {
    "ignition".into()
}

/// Поджигает то, во что врезался (`IgniteOnCollideComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IgniteOnCollide {
    #[serde(default = "one")]
    pub count: i32,
    #[serde(default)]
    pub fire_stacks: f32,
    #[serde(default = "ignition_fixture")]
    pub fixture_id: String,
}

/// Поджигает тех, по кому ударили (`IgniteOnMeleeHitComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IgniteOnMeleeHit {
    #[serde(default)]
    pub fire_stacks: f32,
}

fn one_f() -> f32 {
    1.0
}
fn heat_threshold() -> FixedPoint2 {
    FixedPoint2::from_f32(15.0)
}

/// Загорается от сильного ожога (`IgniteOnHeatDamageComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IgniteOnHeatDamage {
    #[serde(default = "one_f")]
    pub fire_stacks: f32,
    #[serde(default = "heat_threshold")]
    pub threshold: FixedPoint2,
}

/// Одежда ослабляет урон от огня (`FireProtectionComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FireProtection {
    #[serde(default)]
    pub reduction: f32,
}

/// Горячий ли предмет (`IsHotEvent`). На предмете.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct IsHot {
    pub is_hot: bool,
}

/// Загорелось (`IgnitedEvent`) и погасло (`ExtinguishedEvent`). На сущности.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Ignited;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Extinguished;

/// Множитель урона от огня (`GetFireProtectionEvent`): сущность и надетое, кроме карманов.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GetFireProtection {
    pub multiplier: f32,
}

impl GetFireProtection {
    pub fn reduce(&mut self, by: f32) {
        self.multiplier = (self.multiplier - by).max(0.0);
    }
}

/// Пламя клеток за такт: сущность → наибольший перегрев (`_fireEvents`).
#[derive(Resource, Debug, Default)]
struct FireEvents(BTreeMap<Entity, f32>);

fn now(world: &World) -> Duration {
    world.resource::<GameTime>().cur_time()
}

/// Обновить вид (`UpdateAppearance`).
fn update_appearance(world: &mut World, entity: Entity) {
    let Some(flammable) = world.get::<Flammable>(entity) else {
        return;
    };
    let (on_fire, stacks) = (flammable.on_fire, flammable.fire_stacks);
    world.set_appearance(entity, ON_FIRE, on_fire);
    world.set_appearance(entity, FIRE_STACKS, stacks);
    world.set_appearance(entity, TOGGLEABLE_ENABLED, on_fire);
}

/// Поджечь источник (`SetIgnited`).
pub fn set_ignited(world: &mut World, entity: Entity, ignited: bool) {
    if let Some(mut source) = world.get_mut::<IgnitionSource>(entity)
        && source.ignited != ignited
    {
        source.ignited = ignited;
    }
}

/// Добавить «стопки» (`AdjustFireStacks`).
pub fn adjust_fire_stacks(world: &mut World, entity: Entity, relative: f32, ignite: bool) {
    let Some(stacks) = world.get::<Flammable>(entity).map(|f| f.fire_stacks) else {
        return;
    };
    set_fire_stacks(world, entity, stacks + relative, ignite);
}

/// Задать «стопки» (`SetFireStacks`): без них огонь гаснет.
pub fn set_fire_stacks(world: &mut World, entity: Entity, stacks: f32, ignite: bool) {
    let Some(mut flammable) = world.get_mut::<Flammable>(entity) else {
        return;
    };
    flammable.fire_stacks = stacks
        .max(flammable.minimum_fire_stacks)
        .min(flammable.maximum_fire_stacks);
    if flammable.fire_stacks <= 0.0 {
        try_extinguish(world, entity);
    } else {
        flammable.on_fire |= ignite;
        update_appearance(world, entity);
    }
}

/// Потушить (`TryExtinguish`).
pub fn try_extinguish(world: &mut World, entity: Entity) -> bool {
    let Some(mut flammable) = world.get_mut::<Flammable>(entity) else {
        return false;
    };
    if !flammable.on_fire || !flammable.can_extinguish {
        return false;
    }
    flammable.on_fire = false;
    flammable.fire_stacks = 0.0;
    set_ignited(world, entity, false);
    world.raise(entity, &mut Extinguished);
    update_appearance(world, entity);
    true
}

/// Поджечь (`Ignite`): горит, если есть «стопки».
pub fn ignite(world: &mut World, entity: Entity) {
    let Some(mut flammable) = world.get_mut::<Flammable>(entity) else {
        return;
    };
    if flammable.always_combustible {
        flammable.fire_stacks = flammable.firestacks_on_ignite.max(flammable.fire_stacks);
    }
    if flammable.fire_stacks > 0.0 && !flammable.on_fire {
        flammable.on_fire = true;
        world.raise(entity, &mut Ignited);
    }
    update_appearance(world, entity);
}

/// Горячий ли предмет (`IsHotEvent`).
pub fn is_hot(world: &mut World, entity: Entity) -> bool {
    let mut event = IsHot::default();
    world.raise(entity, &mut event);
    event.is_hot
}

/// Поджечь горячим предметом (`TryIgniteFromHotSource`).
fn try_ignite_from_hot_source(world: &mut World, entity: Entity, source: Entity) -> bool {
    if !is_hot(world, source) {
        return false;
    }
    ignite(world, entity);
    true
}

/// Массы двух тел для выравнивания огня (`OnCollide`); без тел — по 1.
fn masses(world: &World, a: Entity, b: Entity) -> (f32, f32) {
    match (world.get::<Physics>(a), world.get::<Physics>(b)) {
        (Some(pa), Some(pb)) if pa.mass > 0.0 && pb.mass > 0.0 => (pa.mass, pb.mass),
        _ => (1.0, 1.0),
    }
}

/// Горящие тела касаются: огонь выравнивается по массе (`OnCollide`).
fn on_collide(world: &mut World, entity: Entity, event: &mut StartCollide) {
    let other = event.other_entity;
    if other.index() < entity.index() {
        return;
    }
    let Some(flammable) = world.get::<Flammable>(entity).cloned() else {
        return;
    };
    if event.other_fixture != flammable.flammable_fixture_id
        && event.our_fixture != flammable.flammable_fixture_id
    {
        return;
    }
    if !flammable.fire_spread {
        return;
    }
    let Some(other_flammable) = world.get::<Flammable>(other).cloned() else {
        return;
    };
    if !other_flammable.fire_spread || (!flammable.on_fire && !other_flammable.on_fire) {
        return;
    }
    let (m1, m2) = masses(world, entity, other);
    let average = (flammable.fire_stacks * m1 + other_flammable.fire_stacks * m2) / 2.0;
    set_fire_stacks(world, entity, average / m1, true);
    set_fire_stacks(world, other, average / m2, true);
}

/// Нетвёрдая фикстура для касаний горящих тел (`OnMapInit`).
fn add_flammable_fixture(world: &mut World, entity: Entity) {
    let Some(flammable) = world.get::<Flammable>(entity).cloned() else {
        return;
    };
    if world.get::<Physics>(entity).is_none() {
        return;
    }
    let mask = group_bits("FullTileLayer").unwrap_or(0);
    let Some(mut fixtures) = world.get_mut::<Fixtures>(entity) else {
        return;
    };
    if fixtures
        .fixtures
        .contains_key(&flammable.flammable_fixture_id)
    {
        return;
    }
    fixtures.fixtures.insert(
        flammable.flammable_fixture_id,
        Fixture {
            shape: flammable.flammable_collision_shape,
            hard: false,
            layer: CollisionGroups::NONE,
            mask: CollisionGroups(mask),
            density: 0.0,
            ..Fixture::default()
        },
    );
    world.refresh_fixtures(entity);
}

/// Клетка под сущностью: грид и клетка (`GetGridOrMapTilePosition`).
fn tile_of(world: &World, entity: Entity) -> Option<(Entity, TileCoord)> {
    let transform = world.get::<Transform>(entity)?;
    let grid = transform.grid()?;
    let level = transform.level();
    let world_position = world.world_transform(entity)?.position;
    let local = world.world_transform(grid)?.to_local(world_position);
    Some((grid, TileCoord::containing(local, level)))
}

fn update(world: &mut World) {
    // Пламя клеток (`_fireEvents`): 100 К перегрева → 1 «стопка», 200 → 2, 400 → 3.
    let events = std::mem::take(&mut world.resource_mut::<FireEvents>().0);
    for (entity, delta) in events {
        let Some(stacks) = world.get::<Flammable>(entity).map(|f| f.fire_stacks) else {
            continue;
        };
        let modifier = ((delta / 100.0).log2() + 1.0).max(0.0);
        let difference = modifier - stacks;
        if difference > 0.0 {
            adjust_fire_stacks(world, entity, difference, false);
        }
        ignite(world, entity);
    }

    // Горящие источники поджигают газ (`IgnitionSourceSystem.Update`), каждый такт.
    let sources: Vec<(Entity, f32)> = world
        .query::<(Entity, &IgnitionSource)>()
        .iter(world)
        .filter(|(_, s)| s.ignited)
        .map(|(e, s)| (e, s.temperature))
        .collect();
    for (entity, temperature) in sources {
        if let Some((grid, coord)) = tile_of(world, entity) {
            hotspot_expose_at(world, grid, coord, temperature, 50.0, true);
        }
    }

    let time = now(world);
    let due: Vec<Entity> = world
        .query::<(Entity, &Flammable)>()
        .iter(world)
        .filter(|(_, f)| time >= f.next_update)
        .map(|(e, _)| e)
        .collect();
    for entity in due {
        let Some(mut flammable) = world.get_mut::<Flammable>(entity) else {
            continue;
        };
        flammable.next_update += UPDATE_TIME;
        if flammable.next_update < time {
            flammable.next_update = time + UPDATE_TIME;
        }
        // Сопротивление закончилось.
        if flammable.resist_complete_time.is_some_and(|end| time > end) {
            flammable.resist_complete_time = None;
        }
        // Мокрое понемногу сохнет.
        if flammable.fire_stacks < 0.0 {
            flammable.fire_stacks = (flammable.fire_stacks + 1.0).min(0.0);
        }
        let flammable = flammable.clone();
        if !flammable.on_fire {
            crate::alerts::clear_alert(world, entity, &flammable.fire_alert);
            continue;
        }
        crate::alerts::show_alert(world, entity, &flammable.fire_alert, None);
        if flammable.fire_stacks <= 0.0 {
            try_extinguish(world, entity);
            continue;
        }
        // Без кислорода огонь гаснет.
        let oxygen = crate::barotrauma::containing_mixture(world, entity).map(|m| m.moles[OXYGEN]);
        if oxygen.is_none_or(|o| o < 1.0) {
            try_extinguish(world, entity);
            continue;
        }
        if world.get::<IgnitionSource>(entity).is_none() {
            world.entity_mut(entity).insert(IgnitionSource::default());
        }
        set_ignited(world, entity, true);
        change_heat(
            world,
            entity,
            FIRESTACK_ENERGY * flammable.fire_stacks,
            false,
        );
        let mut protection = GetFireProtection { multiplier: 1.0 };
        world.raise(entity, &mut protection);
        for item in worn_items(world, entity) {
            world.raise(item, &mut protection);
        }
        let damage = flammable
            .damage
            .scale(flammable.fire_stacks * protection.multiplier);
        world.change_damage(entity, &damage, false, false, None);
        // Сопротивление гасит огонь в 15 раз быстрее.
        let fade = flammable.firestack_fade * if flammable.resisting() { 15.0 } else { 1.0 };
        adjust_fire_stacks(world, entity, fade, true);
    }
}

/// Горючие стопки (`Flammable`): на уже горящем — свой множитель, если задан.
#[derive(Debug)]
struct FlammableEffect {
    multiplier: f32,
    multiplier_on_existing: Option<f32>,
}

impl crate::entity_effects::EffectKind for FlammableEffect {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        let Some(stacks) = world.get::<Flammable>(target).map(|f| f.fire_stacks) else {
            return;
        };
        let multiplier = match self.multiplier_on_existing {
            Some(existing) if stacks != 0.0 => existing,
            _ => self.multiplier,
        };
        adjust_fire_stacks(world, target, scale * multiplier, false);
    }
}

/// Поджечь (`Ignite`).
#[derive(Debug)]
struct IgniteEffect;

impl crate::entity_effects::EffectKind for IgniteEffect {
    fn apply(&self, world: &mut World, target: Entity, _: f32, _: Option<Entity>) {
        if world.get::<Flammable>(target).is_some() {
            ignite(world, target);
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Flammable>();
    world.register_effect("Flammable", |node| {
        Some(std::sync::Arc::new(FlammableEffect {
            multiplier: crate::entity_effects::number(node, "multiplier").unwrap_or(0.05),
            multiplier_on_existing: crate::entity_effects::number(node, "multiplierOnExisting"),
        }))
    });
    world.register_effect("Ignite", |_| Some(std::sync::Arc::new(IgniteEffect)));
    world.register_data_component::<IgnitionSource>();
    world.register_data_component::<IgniteOnCollide>();
    world.register_data_component::<IgniteOnMeleeHit>();
    world.register_data_component::<IgniteOnHeatDamage>();
    world.register_data_component::<FireProtection>();
    if !side.is_server() {
        return;
    }
    world.init_resource::<FireEvents>();
    world.subscribe::<Flammable, MapInit>(|world, entity, _| {
        let time = now(world);
        if let Some(mut flammable) = world.get_mut::<Flammable>(entity) {
            flammable.next_update = time + UPDATE_TIME;
        }
        add_flammable_fixture(world, entity);
        update_appearance(world, entity);
    });
    world.subscribe::<Flammable, crate::alerts::AlertClicked>(|world, entity, event| {
        if event.handled || event.event != "ResistFireAlertEvent" {
            return;
        }
        resist(world, entity);
        event.handled = true;
    });
    world.subscribe::<Flammable, InteractUsing>(|world, entity, event| {
        if event.handled {
            return;
        }
        event.handled = try_ignite_from_hot_source(world, entity, event.used);
    });
    world.subscribe::<Flammable, StartCollide>(on_collide);
    world.subscribe::<Flammable, IsHot>(|world, entity, event| {
        event.is_hot = world.get::<Flammable>(entity).is_some_and(|f| f.on_fire);
    });
    world.subscribe::<IgnitionSource, IsHot>(|world, entity, event| {
        event.is_hot |= world
            .get::<IgnitionSource>(entity)
            .is_some_and(|s| s.ignited);
    });
    world.subscribe::<Flammable, TileFire>(|world, entity, event| {
        let Some(minimum) = world
            .get::<Flammable>(entity)
            .map(|f| f.min_ignition_temperature)
        else {
            return;
        };
        let delta = event.temperature - minimum;
        let events = &mut world.resource_mut::<FireEvents>().0;
        let current = events.get(&entity).copied().unwrap_or(0.0);
        if delta > current {
            events.insert(entity, delta);
        }
    });
    world.subscribe::<Flammable, Rejuvenate>(|world, entity, _| {
        try_extinguish(world, entity);
    });
    world.subscribe::<Flammable, Attacked>(|world, entity, event| {
        try_ignite_from_hot_source(world, entity, event.used);
    });
    world.subscribe::<FireProtection, GetFireProtection>(|world, entity, event| {
        if let Some(protection) = world.get::<FireProtection>(entity) {
            event.reduce(protection.reduction);
        }
    });
    world.subscribe::<IgniteOnMeleeHit, MeleeHit>(|world, weapon, event| {
        let Some(stacks) = world.get::<IgniteOnMeleeHit>(weapon).map(|c| c.fire_stacks) else {
            return;
        };
        for target in event.hit_entities.clone() {
            if world.get::<Flammable>(target).is_none() {
                continue;
            }
            adjust_fire_stacks(world, target, stacks, false);
            if stacks >= 0.0 {
                ignite(world, target);
            }
        }
    });
    world.subscribe::<IgniteOnCollide, Land>(|world, entity, _| {
        world.entity_mut(entity).remove::<IgniteOnCollide>();
    });
    world.subscribe::<IgniteOnCollide, StartCollide>(|world, entity, event| {
        let Some(component) = world.get::<IgniteOnCollide>(entity).cloned() else {
            return;
        };
        if component.count == 0 {
            return;
        }
        let hard = world
            .get::<Fixtures>(event.other_entity)
            .and_then(|f| f.fixtures.get(&event.other_fixture))
            .is_some_and(|f| f.hard);
        if !hard {
            return;
        }
        if event.our_fixture != component.fixture_id
            && event.our_fixture != crate::projectiles::PROJECTILE_FIXTURE
        {
            return;
        }
        let other = event.other_entity;
        let Some(mut flammable) = world.get_mut::<Flammable>(other) else {
            return;
        };
        flammable.fire_stacks += component.fire_stacks;
        ignite(world, other);
        let mut remove = false;
        if let Some(mut component) = world.get_mut::<IgniteOnCollide>(entity) {
            component.count -= 1;
            remove = component.count == 0;
        }
        if remove {
            world.entity_mut(entity).remove::<IgniteOnCollide>();
        }
    });
    world.subscribe::<IgniteOnHeatDamage, DamageChanged>(|world, entity, event| {
        let Some(component) = world.get::<IgniteOnHeatDamage>(entity).cloned() else {
            return;
        };
        let Some(heat) = event
            .delta
            .as_ref()
            .and_then(|d| d.types.get("Heat"))
            .copied()
        else {
            return;
        };
        if heat <= component.threshold {
            return;
        }
        let Some(mut flammable) = world.get_mut::<Flammable>(entity) else {
            return;
        };
        flammable.fire_stacks += component.fire_stacks;
        ignite(world, entity);
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
