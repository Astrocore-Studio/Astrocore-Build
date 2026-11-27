//! Урон: типы и группы из прототипов, сопротивления, накопленный урон сущности и его
//! изменение. Образец — `Content.Shared/Damage` SS14 @ 2f4a2aad (`DamageSpecifier`,
//! `DamageModifierSet`, `DamageableComponent`, `InjurableComponent`, `DamageableSystem`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use astrocore_ecs::{ComponentInit, EventWorldExt, RegistryWorldExt};
use astrocore_net::NetRegistryWorldExt;
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::do_after::{DoAfterComponent, DoAfterWorldExt};
use crate::fixed_point::FixedPoint2;

/// Урон по типам (`DamageSpecifier`). В данных — `types: { Blunt: 5 }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DamageSpecifier {
    #[serde(default)]
    pub types: BTreeMap<String, FixedPoint2>,
}

impl DamageSpecifier {
    pub fn new(types: impl IntoIterator<Item = (impl Into<String>, FixedPoint2)>) -> Self {
        Self {
            types: types.into_iter().map(|(t, v)| (t.into(), v)).collect(),
        }
    }

    /// Поровну по типам группы с остатком в последних (`DamageSpecifier(group, value)`).
    pub fn from_group(types: &[String], value: FixedPoint2) -> Self {
        let mut damage = Self::default();
        let mut remaining_types = types.len() as i32;
        let mut remaining = value;
        for damage_type in types {
            let part = remaining / FixedPoint2::new(remaining_types);
            damage.types.insert(damage_type.clone(), part);
            remaining -= part;
            remaining_types -= 1;
        }
        damage
    }

    pub fn total(&self) -> FixedPoint2 {
        self.types.values().fold(FixedPoint2::ZERO, |a, b| a + *b)
    }

    pub fn any_positive(&self) -> bool {
        self.types.values().any(|v| *v > FixedPoint2::ZERO)
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    pub fn trim_zeros(&mut self) {
        self.types.retain(|_, v| *v != FixedPoint2::ZERO);
    }

    pub fn clamp_min(&mut self, min: FixedPoint2) {
        for value in self.types.values_mut() {
            *value = (*value).max(min);
        }
    }

    pub fn positive(&self) -> Self {
        Self {
            types: self
                .types
                .iter()
                .filter(|(_, v)| **v > FixedPoint2::ZERO)
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        }
    }

    pub fn negative(&self) -> Self {
        Self {
            types: self
                .types
                .iter()
                .filter(|(_, v)| **v < FixedPoint2::ZERO)
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        }
    }

    /// Умножение на дробное (`operator *(DamageSpecifier, float)`).
    pub fn scale(&self, factor: f32) -> Self {
        Self {
            types: self
                .types
                .iter()
                .map(|(k, v)| (k.clone(), v.scale(factor)))
                .collect(),
        }
    }

    /// Сумма урона типов группы, если хоть один тип группы есть (`TryGetDamageInGroup`).
    pub fn in_group(&self, group: &[String]) -> Option<FixedPoint2> {
        let mut found = false;
        let mut total = FixedPoint2::ZERO;
        for damage_type in group {
            if let Some(value) = self.types.get(damage_type) {
                total += *value;
                found = true;
            }
        }
        found.then_some(total)
    }

    /// Урон после сопротивлений (`ApplyModifierSet`): лечение не меняется, плоское снижение
    /// не уводит в минус, коэффициент применяется после него.
    pub fn apply_modifier_set(&self, set: &DamageModifierSet) -> Self {
        let mut result = Self::default();
        for (key, value) in &self.types {
            if *value == FixedPoint2::ZERO {
                continue;
            }
            if *value < FixedPoint2::ZERO {
                result.types.insert(key.clone(), *value);
                continue;
            }
            let mut amount = value.float();
            if let Some(reduction) = set.flat_reductions.get(key) {
                amount = (amount - reduction).max(0.0);
            }
            if let Some(coefficient) = set.coefficients.get(key) {
                amount *= coefficient;
            }
            if amount != 0.0 {
                result
                    .types
                    .insert(key.clone(), FixedPoint2::from_f32(amount));
            }
        }
        result
    }
}

impl std::ops::Neg for &DamageSpecifier {
    type Output = DamageSpecifier;
    fn neg(self) -> DamageSpecifier {
        DamageSpecifier {
            types: self.types.iter().map(|(k, v)| (k.clone(), -*v)).collect(),
        }
    }
}

impl std::ops::Add for &DamageSpecifier {
    type Output = DamageSpecifier;
    fn add(self, other: &DamageSpecifier) -> DamageSpecifier {
        let mut result = self.clone();
        for (key, value) in &other.types {
            *result.types.entry(key.clone()).or_default() += *value;
        }
        result
    }
}

impl std::ops::Sub for &DamageSpecifier {
    type Output = DamageSpecifier;
    fn sub(self, other: &DamageSpecifier) -> DamageSpecifier {
        let mut result = self.clone();
        for (key, value) in &other.types {
            *result.types.entry(key.clone()).or_default() -= *value;
        }
        result
    }
}

/// Сопротивления (`DamageModifierSet`). В данных — прототип `damageModifierSet`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DamageModifierSet {
    #[serde(default)]
    pub coefficients: BTreeMap<String, f32>,
    #[serde(default)]
    pub flat_reductions: BTreeMap<String, f32>,
}

/// Прототипы урона, собранные один раз после загрузки: группы, контейнеры и наборы
/// сопротивлений. Ресурс `World`.
#[derive(Resource, Debug, Default)]
pub struct DamageCatalog {
    /// Группы по порядку идентификаторов с их типами.
    pub groups: Vec<(String, Vec<String>)>,
    /// Типы, которые принимает контейнер (`_supportedTypesByContainer`).
    pub containers: HashMap<String, HashSet<String>>,
    pub modifier_sets: HashMap<String, DamageModifierSet>,
}

impl DamageCatalog {
    pub fn build(prototypes: &Prototypes) -> Self {
        let list = |node: Option<&astrocore_data::DataNode>| -> Vec<String> {
            node.and_then(|n| astrocore_data::from_node::<Vec<String>>(n).ok())
                .unwrap_or_default()
        };
        let groups: Vec<(String, Vec<String>)> = prototypes
            .ids("damageGroup")
            .filter_map(|id| {
                let node = prototypes.resolved_node("damageGroup", id)?;
                Some((id.to_owned(), list(node.get("damageTypes"))))
            })
            .collect();
        let containers = prototypes
            .ids("damageContainer")
            .filter_map(|id| {
                let node = prototypes.resolved_node("damageContainer", id)?;
                let mut types: HashSet<String> =
                    list(node.get("supportedTypes")).into_iter().collect();
                for group in list(node.get("supportedGroups")) {
                    if let Some((_, members)) = groups.iter().find(|(g, _)| *g == group) {
                        types.extend(members.iter().cloned());
                    }
                }
                Some((id.to_owned(), types))
            })
            .collect();
        let modifier_sets = prototypes
            .ids("damageModifierSet")
            .filter_map(|id| {
                let node = prototypes.resolved_node("damageModifierSet", id)?;
                Some((id.to_owned(), astrocore_data::from_node(node).ok()?))
            })
            .collect();
        Self {
            groups,
            containers,
            modifier_sets,
        }
    }

    pub fn group(&self, id: &str) -> Option<&[String]> {
        self.groups
            .iter()
            .find(|(g, _)| g == id)
            .map(|(_, types)| types.as_slice())
    }

    /// Урон по группам (`GetDamagePerGroup`).
    pub fn per_group(&self, damage: &DamageSpecifier) -> BTreeMap<String, FixedPoint2> {
        self.groups
            .iter()
            .filter_map(|(id, types)| Some((id.clone(), damage.in_group(types)?)))
            .collect()
    }

    /// Принимает ли контейнер тип; без контейнера — любой (`SupportsType`).
    pub fn supports(&self, container: Option<&str>, damage_type: &str) -> bool {
        match container {
            None => true,
            Some(container) => self
                .containers
                .get(container)
                .is_some_and(|types| types.contains(damage_type)),
        }
    }
}

/// Каталог урона мира; собирается при первом обращении.
pub fn catalog(world: &mut World) -> Arc<DamageCatalog> {
    if let Some(catalog) = world.get_resource::<SharedCatalog>() {
        return catalog.0.clone();
    }
    let built = Arc::new(
        world
            .get_resource::<Prototypes>()
            .map(DamageCatalog::build)
            .unwrap_or_default(),
    );
    world.insert_resource(SharedCatalog(built.clone()));
    built
}

/// Каталог урона, если он уже собран; иначе — собранный заново из прототипов (без записи в
/// мир).
pub fn catalog_of(world: &World) -> Option<Arc<DamageCatalog>> {
    if let Some(catalog) = world.get_resource::<SharedCatalog>() {
        return Some(catalog.0.clone());
    }
    world
        .get_resource::<Prototypes>()
        .map(|p| Arc::new(DamageCatalog::build(p)))
}

#[derive(Resource)]
struct SharedCatalog(Arc<DamageCatalog>);

fn radiation() -> Vec<String> {
    vec!["Radiation".into()]
}

/// Накопленный урон. В данных — компонент `Damageable`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Damageable {
    #[serde(default)]
    pub damage_modifier_set: Option<String>,
    #[serde(default)]
    pub damage: DamageSpecifier,
    /// Урон по группам (`DamagePerGroup`), пересчитывается при изменении.
    #[serde(default)]
    pub damage_per_group: BTreeMap<String, FixedPoint2>,
    #[serde(default)]
    pub total_damage: FixedPoint2,
    #[serde(default = "radiation", rename = "radiationDamageTypes")]
    pub radiation_damage_types: Vec<String>,
    #[serde(default)]
    pub displacement: Option<String>,
}

impl Default for Damageable {
    fn default() -> Self {
        Self {
            damage_modifier_set: None,
            damage: DamageSpecifier::default(),
            damage_per_group: BTreeMap::new(),
            total_damage: FixedPoint2::ZERO,
            radiation_damage_types: radiation(),
            displacement: None,
        }
    }
}

fn pain_groups() -> Vec<String> {
    vec!["Brute".into(), "Burn".into()]
}

fn health_icons() -> BTreeMap<String, String> {
    [
        ("Alive", "HealthIconFine"),
        ("Critical", "HealthIconCritical"),
        ("Dead", "HealthIconDead"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

fn rotting_icon() -> String {
    "HealthIconRotting".into()
}

/// Сущность может получать урон заданных типов. В данных — компонент `Injurable`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Injurable {
    #[serde(default)]
    pub damage_container: Option<String>,
    #[serde(default = "pain_groups")]
    pub pain_damage_groups: Vec<String>,
    #[serde(default = "health_icons")]
    pub health_icons: BTreeMap<String, String>,
    #[serde(default = "rotting_icon")]
    pub rotting_icon: String,
}

impl Default for Injurable {
    fn default() -> Self {
        Self {
            damage_container: None,
            pain_damage_groups: pain_groups(),
            health_icons: health_icons(),
            rotting_icon: rotting_icon(),
        }
    }
}

/// Урон сейчас изменится; можно отменить (`BeforeDamageChangedEvent`). На цели.
#[derive(Debug, Clone)]
pub struct BeforeDamageChanged {
    pub damage: DamageSpecifier,
    pub origin: Option<Entity>,
    pub cancelled: bool,
}

/// Урон после сопротивлений сущности; системы брони меняют `damage` (`DamageModifyEvent`).
#[derive(Debug, Clone)]
pub struct DamageModify {
    pub original_damage: DamageSpecifier,
    pub damage: DamageSpecifier,
    pub origin: Option<Entity>,
}

/// Урон нанесён (`DamageDealtEvent`): компонент модели урона записывает его.
#[derive(Debug, Clone)]
pub struct DamageDealt {
    pub damage: DamageSpecifier,
    pub origin: Option<Entity>,
    pub interrupts_do_afters: bool,
}

/// Накопленный урон изменился (`DamageChangedEvent`). На цели.
#[derive(Debug, Clone)]
pub struct DamageChanged {
    pub delta: Option<DamageSpecifier>,
    pub damage_increased: bool,
    pub interrupts_do_afters: bool,
    pub origin: Option<Entity>,
}

/// Полное исцеление (`RejuvenateEvent`). На цели.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rejuvenate;

/// Урон на `World`.
pub trait DamageableWorldExt {
    /// Изменить урон (`ChangeDamage`); возвращает урон после сопротивлений.
    fn change_damage(
        &mut self,
        target: Entity,
        damage: &DamageSpecifier,
        ignore_resistances: bool,
        interrupts_do_afters: bool,
        origin: Option<Entity>,
    ) -> DamageSpecifier;
    /// Задать урон целиком (`SetDamage`).
    fn set_damage(&mut self, target: Entity, damage: &DamageSpecifier);
    /// Всем типам — одно значение (`SetAllDamage`).
    fn set_all_damage(&mut self, target: Entity, value: FixedPoint2);
    fn clear_all_damage(&mut self, target: Entity);
    /// Лечить поровну по типам (`HealEvenly`); `amount` отрицательный.
    fn heal_evenly(
        &mut self,
        target: Entity,
        amount: FixedPoint2,
        group: Option<&str>,
        origin: Option<Entity>,
    ) -> DamageSpecifier;
    /// Лечить пропорционально урону (`HealDistributed`).
    fn heal_distributed(
        &mut self,
        target: Entity,
        amount: FixedPoint2,
        group: Option<&str>,
        origin: Option<Entity>,
    ) -> DamageSpecifier;
    fn total_damage(&self, target: Entity) -> FixedPoint2;
    /// Положительный урон, всего или группы (`GetPositiveDamage`).
    fn positive_damage(&mut self, target: Entity, group: Option<&str>) -> DamageSpecifier;
    fn rejuvenate(&mut self, target: Entity);
}

/// Пересчёт и событие после изменения (`OnEntityDamageChanged`).
fn damage_changed(
    world: &mut World,
    target: Entity,
    delta: Option<DamageSpecifier>,
    interrupts: bool,
    origin: Option<Entity>,
) {
    let catalog = catalog(world);
    let Some(mut damageable) = world.get_mut::<Damageable>(target) else {
        return;
    };
    damageable.damage_per_group = catalog.per_group(&damageable.damage);
    damageable.total_damage = damageable.damage.total();
    let increased = delta.as_ref().is_some_and(DamageSpecifier::any_positive);
    let mut event = DamageChanged {
        delta,
        damage_increased: increased,
        interrupts_do_afters: interrupts && increased,
        origin,
    };
    world.raise(target, &mut event);
}

impl DamageableWorldExt for World {
    fn change_damage(
        &mut self,
        target: Entity,
        damage: &DamageSpecifier,
        ignore_resistances: bool,
        interrupts_do_afters: bool,
        origin: Option<Entity>,
    ) -> DamageSpecifier {
        let Some(modifier_set) = self
            .get::<Damageable>(target)
            .map(|d| d.damage_modifier_set.clone())
        else {
            return DamageSpecifier::default();
        };
        if damage.is_empty() {
            return DamageSpecifier::default();
        }
        let mut before = BeforeDamageChanged {
            damage: damage.clone(),
            origin,
            cancelled: false,
        };
        self.raise(target, &mut before);
        if before.cancelled {
            return DamageSpecifier::default();
        }
        let mut damage = damage.clone();
        if !ignore_resistances {
            let catalog = catalog(self);
            if let Some(set) = modifier_set
                .as_deref()
                .and_then(|id| catalog.modifier_sets.get(id))
            {
                damage = damage.apply_modifier_set(set);
            }
            let mut modify = DamageModify {
                original_damage: damage.clone(),
                damage,
                origin,
            };
            self.raise(target, &mut modify);
            damage = modify.damage;
            if damage.is_empty() {
                return DamageSpecifier::default();
            }
        }
        let mut dealt = DamageDealt {
            damage: damage.clone(),
            origin,
            interrupts_do_afters,
        };
        self.raise(target, &mut dealt);
        damage
    }

    fn set_damage(&mut self, target: Entity, damage: &DamageSpecifier) {
        let Some(mut damageable) = self.get_mut::<Damageable>(target) else {
            return;
        };
        damageable
            .damage
            .types
            .retain(|key, _| damage.types.contains_key(key));
        for (key, value) in &damage.types {
            damageable.damage.types.insert(key.clone(), *value);
        }
        damage_changed(self, target, None, true, None);
    }

    fn set_all_damage(&mut self, target: Entity, value: FixedPoint2) {
        if value < FixedPoint2::ZERO {
            return;
        }
        let Some(mut damageable) = self.get_mut::<Damageable>(target) else {
            return;
        };
        for amount in damageable.damage.types.values_mut() {
            *amount = value;
        }
        damage_changed(self, target, Some(DamageSpecifier::default()), true, None);
    }

    fn clear_all_damage(&mut self, target: Entity) {
        self.set_all_damage(target, FixedPoint2::ZERO);
    }

    fn heal_evenly(
        &mut self,
        target: Entity,
        amount: FixedPoint2,
        group: Option<&str>,
        origin: Option<Entity>,
    ) -> DamageSpecifier {
        if self.get::<Damageable>(target).is_none() || amount >= FixedPoint2::ZERO {
            return DamageSpecifier::default();
        }
        let damage = self.positive_damage(target, group);
        if damage.total() <= -amount {
            return self.change_damage(target, &-&damage, true, false, origin);
        }
        let mut change: BTreeMap<String, FixedPoint2> = damage
            .types
            .keys()
            .map(|k| (k.clone(), FixedPoint2::ZERO))
            .collect();
        let mut remaining = -amount;
        let mut keys: Vec<String> = damage.types.keys().cloned().collect();
        while remaining > FixedPoint2::ZERO {
            let count = keys.len() as i32;
            let max_heal = if count == 1 {
                remaining
            } else {
                (remaining + FixedPoint2::EPSILON * (count - 1)) / FixedPoint2::new(count)
            };
            let mut index = keys.len();
            while index > 0 {
                index -= 1;
                let key = keys[index].clone();
                let mut heal = damage.types[&key] + change[&key];
                if heal > max_heal {
                    heal = max_heal;
                } else {
                    keys.remove(index);
                }
                if heal >= remaining {
                    if let Some(value) = change.get_mut(&key) {
                        *value -= remaining;
                    }
                    remaining = FixedPoint2::ZERO;
                    break;
                }
                remaining -= heal;
                if let Some(value) = change.get_mut(&key) {
                    *value -= heal;
                }
            }
            if keys.is_empty() {
                break;
            }
        }
        self.change_damage(
            target,
            &DamageSpecifier { types: change },
            true,
            false,
            origin,
        )
    }

    fn heal_distributed(
        &mut self,
        target: Entity,
        amount: FixedPoint2,
        group: Option<&str>,
        origin: Option<Entity>,
    ) -> DamageSpecifier {
        if self.get::<Damageable>(target).is_none() || amount >= FixedPoint2::ZERO {
            return DamageSpecifier::default();
        }
        let damage = self.positive_damage(target, group);
        if damage.total() <= -amount {
            return self.change_damage(target, &-&damage, true, false, origin);
        }
        let total = damage.total();
        let change = DamageSpecifier {
            types: damage
                .types
                .iter()
                .map(|(k, v)| (k.clone(), *v / total * amount))
                .collect(),
        };
        self.change_damage(target, &change, true, false, origin)
    }

    fn total_damage(&self, target: Entity) -> FixedPoint2 {
        self.get::<Damageable>(target)
            .map_or(FixedPoint2::ZERO, |d| d.total_damage)
    }

    fn positive_damage(&mut self, target: Entity, group: Option<&str>) -> DamageSpecifier {
        let Some(damage) = self.get::<Damageable>(target).map(|d| d.damage.clone()) else {
            return DamageSpecifier::default();
        };
        match group {
            None => damage.positive(),
            Some(group) => {
                let catalog = catalog(self);
                let Some(types) = catalog.group(group) else {
                    return DamageSpecifier::default();
                };
                DamageSpecifier {
                    types: types
                        .iter()
                        .filter_map(|t| Some((t.clone(), *damage.types.get(t)?)))
                        .filter(|(_, v)| *v > FixedPoint2::ZERO)
                        .collect(),
                }
            }
        }
    }

    fn rejuvenate(&mut self, target: Entity) {
        self.raise(target, &mut Rejuvenate);
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<Damageable>();
    world.register_data_component::<Injurable>();
    world.register_net_serde::<Damageable>();
    world.register_net_serde::<Injurable>();
    world.subscribe::<Damageable, ComponentInit>(|world, entity, _| {
        let catalog = catalog(world);
        if let Some(mut damageable) = world.get_mut::<Damageable>(entity) {
            damageable.damage_per_group = catalog.per_group(&damageable.damage);
            damageable.total_damage = damageable.damage.total();
        }
    });
    if !side.is_server() {
        return;
    }
    // Нанесённый урон записывают только поддерживаемые типы, урон не уходит ниже нуля
    // (`OnDamageDealt`).
    world.subscribe::<Injurable, DamageDealt>(|world, entity, event| {
        let catalog = catalog(world);
        let container = world
            .get::<Injurable>(entity)
            .and_then(|i| i.damage_container.clone());
        let Some(mut damageable) = world.get_mut::<Damageable>(entity) else {
            return;
        };
        let mut done = DamageSpecifier::default();
        for (damage_type, value) in &event.damage.types {
            if !catalog.supports(container.as_deref(), damage_type) {
                continue;
            }
            let old = damageable
                .damage
                .types
                .get(damage_type)
                .copied()
                .unwrap_or_default();
            let new = (old + *value).max(FixedPoint2::ZERO);
            if new == old {
                continue;
            }
            damageable.damage.types.insert(damage_type.clone(), new);
            done.types.insert(damage_type.clone(), new - old);
        }
        if !done.is_empty() {
            damage_changed(
                world,
                entity,
                Some(done),
                event.interrupts_do_afters,
                event.origin,
            );
        }
    });
    world.subscribe::<Damageable, Rejuvenate>(|world, entity, _| {
        crate::mobs::set_allow_revives(world, entity, true);
        world.clear_all_damage(entity);
        crate::mobs::set_allow_revives(world, entity, false);
    });
    // Урон прерывает действия с задержкой (`SharedDoAfterSystem.OnDamage`).
    world.subscribe::<DoAfterComponent, DamageChanged>(|world, entity, event| {
        if !event.interrupts_do_afters || !event.damage_increased {
            return;
        }
        if let Some(delta) = &event.delta {
            world.cancel_do_afters_on_damage(entity, delta.total().float());
        }
    });
}

#[cfg(test)]
pub(crate) mod tests;
