//! Кровоток: кровь по эталонному составу, кровотечение от ран, потеря крови и её урон,
//! восстановление крови, значок кровотечения, сообщения осмотра здоровья, прижигание ран
//! ожогом. Раз в 3 секунды живое тело восстанавливает кровь, течёт кровью (кровотечение
//! слабеет) и при крови ниже порога получает урон `Bloodloss`, выше — лечит его; мёртвое
//! только течёт. Образец — `BloodstreamComponent`, `BloodstreamSystem` SS14 @ 2f4a2aad.
//!
//! Кровь — список «реагент → количество», не раствор (ADR-0016): вытекшая кровь сверх
//! порога лужи пропадает, луж нет. Лекарств в крови, метаболитов, ДНК и головокружения от
//! потери крови (`StatusEffectBloodloss`) пока нет.

use std::collections::BTreeMap;
use std::time::Duration;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, MapInit, RegistryWorldExt, Update};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::damage::{DamageChanged, DamageSpecifier, DamageableWorldExt, Rejuvenate};
use crate::examine::loc;
use crate::fixed_point::FixedPoint2;
use crate::health_examinable::HealthBeingExamined;
use crate::mobs::MobStateWorldExt;
use crate::popups::{PopupType, PopupWorldExt};

/// Количество реагента в данных (`ReagentQuantity`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReagentQuantity {
    #[serde(rename = "ReagentId")]
    pub reagent_id: String,
    #[serde(rename = "Quantity")]
    pub quantity: FixedPoint2,
}

/// Состав крови в данных (`Solution` с `reagents`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReferenceSolution {
    pub reagents: Vec<ReagentQuantity>,
}

impl Default for ReferenceSolution {
    fn default() -> Self {
        Self {
            reagents: vec![ReagentQuantity {
                reagent_id: "Blood".into(),
                quantity: FixedPoint2::new(600),
            }],
        }
    }
}

impl ReferenceSolution {
    pub fn volume(&self) -> FixedPoint2 {
        self.reagents
            .iter()
            .fold(FixedPoint2::ZERO, |sum, r| sum + r.quantity)
    }
}

fn update_interval() -> f32 {
    3.0
}
fn one_f32() -> f32 {
    1.0
}
fn bleed_reduction() -> f32 {
    0.33
}
fn max_bleed() -> f32 {
    10.0
}
fn bloodloss_threshold() -> f32 {
    0.9
}
fn one_fixed() -> FixedPoint2 {
    FixedPoint2::new(1)
}
fn yes() -> bool {
    true
}
fn bleed_modifiers() -> String {
    "BloodlossHuman".into()
}
fn instant_blood_sound() -> SoundSpecifier {
    SoundSpecifier::collection("blood")
}
fn blood_healed_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Effects/lightburn.ogg")
}
fn healed_threshold() -> f32 {
    -0.1
}
fn max_volume_modifier() -> f32 {
    1.2
}
fn bleeding_alert() -> String {
    "Bleed".into()
}

/// Кровоток (`BloodstreamComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bloodstream {
    #[serde(skip)]
    pub next_update: Duration,
    /// Между обновлениями, с.
    #[serde(default = "update_interval")]
    pub update_interval: f32,
    #[serde(default = "one_f32")]
    pub update_interval_multiplier: f32,
    /// Кровотечение: столько крови уходит за обновление.
    #[serde(default)]
    pub bleed_amount: f32,
    #[serde(default = "bleed_reduction")]
    pub bleed_reduction_amount: f32,
    #[serde(default = "max_bleed")]
    pub max_bleed_amount: f32,
    /// Ниже этой доли крови — урон.
    #[serde(default = "bloodloss_threshold")]
    pub bloodloss_threshold: f32,
    #[serde(default)]
    pub bloodloss_damage: DamageSpecifier,
    #[serde(default)]
    pub bloodloss_heal_damage: DamageSpecifier,
    /// Восстановление крови за обновление.
    #[serde(default = "one_fixed")]
    pub blood_refresh_amount: FixedPoint2,
    /// Вытекшей крови больше этого — лужа.
    #[serde(default = "one_fixed")]
    pub bleed_puddle_threshold: FixedPoint2,
    #[serde(default = "yes")]
    pub blood_increase_enabled: bool,
    /// Набор коэффициентов урона → кровотечение.
    #[serde(default = "bleed_modifiers")]
    pub damage_bleed_modifiers: String,
    #[serde(default = "instant_blood_sound")]
    pub instant_blood_sound: SoundSpecifier,
    #[serde(default = "blood_healed_sound")]
    pub blood_healed_sound: SoundSpecifier,
    #[serde(default = "healed_threshold")]
    pub blood_healed_sound_threshold: f32,
    /// Крови не больше этой доли эталона.
    #[serde(default = "max_volume_modifier")]
    pub max_volume_modifier: f32,
    #[serde(default)]
    pub blood_reference_solution: ReferenceSolution,
    #[serde(default = "bleeding_alert")]
    pub bleeding_alert: String,
    /// Кровь (раствор `bloodstream`).
    #[serde(default)]
    pub blood: BTreeMap<String, FixedPoint2>,
    /// Вытекшая кровь (раствор `bloodstreamTemporary`).
    #[serde(default)]
    pub leaked: BTreeMap<String, FixedPoint2>,
}

impl Bloodstream {
    /// Интервал с учётом обмена веществ (`AdjustedUpdateInterval`).
    pub fn adjusted_interval(&self) -> Duration {
        Duration::from_secs_f32(self.update_interval * self.update_interval_multiplier)
    }

    /// Всего крови.
    pub fn blood_volume(&self) -> FixedPoint2 {
        self.blood
            .values()
            .fold(FixedPoint2::ZERO, |sum, quantity| sum + *quantity)
    }

    /// Доля пригодной крови, от 0 до `max_volume_modifier` (`GetBloodLevel`): меньшая по
    /// реагентам эталона.
    pub fn blood_level(&self) -> f32 {
        if self.blood_reference_solution.volume() == FixedPoint2::ZERO {
            return 0.0;
        }
        let mut level = FixedPoint2::from_f32(self.max_volume_modifier);
        for reference in &self.blood_reference_solution.reagents {
            let have = self
                .blood
                .get(&reference.reagent_id)
                .copied()
                .unwrap_or_default();
            level = level.min(have / reference.quantity);
        }
        level.float()
    }

    /// Подвести кровь к эталону × `reference_factor`, не больше `amount` за раз
    /// (`TryRegulateBloodLevel`).
    pub fn regulate(&mut self, amount: FixedPoint2, reference_factor: f32) -> bool {
        if amount == FixedPoint2::ZERO {
            return false;
        }
        if !self.blood_increase_enabled && amount > FixedPoint2::ZERO {
            return false;
        }
        let factor = reference_factor.clamp(0.0, self.max_volume_modifier);
        let volume = self.blood_reference_solution.volume().float();
        let ratio = amount.float() / volume;
        for reference in self.blood_reference_solution.reagents.clone() {
            let have = self
                .blood
                .get(&reference.reagent_id)
                .copied()
                .unwrap_or_default();
            let error = reference.quantity.scale(factor) - have;
            let adjusted = reference.quantity.scale(ratio);
            if error > FixedPoint2::ZERO {
                *self.blood.entry(reference.reagent_id).or_default() += error.min(adjusted);
            } else if error < FixedPoint2::ZERO {
                let removed = (-error).min(adjusted);
                remove_reagent(&mut self.blood, &reference.reagent_id, removed);
            }
        }
        true
    }

    /// Поднять или опустить кровь на `amount`, не выше нормы (`TryModifyBloodLevel`).
    pub fn modify_level(&mut self, amount: FixedPoint2) -> bool {
        if amount < FixedPoint2::ZERO {
            self.regulate(-amount, 0.0)
        } else {
            self.regulate(amount, 1.0)
        }
    }

    /// Выпустить кровь (`TryBleedOut`): часть крови поровну по составу уходит в вытекшую;
    /// больше порога лужи — вытекшая пропадает.
    pub fn bleed_out(&mut self, amount: FixedPoint2) -> bool {
        if amount <= FixedPoint2::ZERO {
            return false;
        }
        let leaked = split(&mut self.blood, amount);
        for (reagent, quantity) in leaked {
            *self.leaked.entry(reagent).or_default() += quantity;
        }
        let leaked_volume = self
            .leaked
            .values()
            .fold(FixedPoint2::ZERO, |sum, q| sum + *q);
        if leaked_volume > self.bleed_puddle_threshold {
            // Лужи — фаза 7.2 (ADR-0016).
            self.leaked.clear();
        }
        true
    }
}

fn remove_reagent(map: &mut BTreeMap<String, FixedPoint2>, reagent: &str, amount: FixedPoint2) {
    if let Some(have) = map.get_mut(reagent) {
        *have = (*have - amount).max(FixedPoint2::ZERO);
        if *have == FixedPoint2::ZERO {
            map.remove(reagent);
        }
    }
}

/// Отделить `amount` от раствора пропорционально составу (`SplitSolution`).
fn split(
    map: &mut BTreeMap<String, FixedPoint2>,
    amount: FixedPoint2,
) -> BTreeMap<String, FixedPoint2> {
    let volume = map.values().fold(FixedPoint2::ZERO, |sum, q| sum + *q);
    if volume == FixedPoint2::ZERO {
        return BTreeMap::new();
    }
    if amount >= volume {
        return std::mem::take(map);
    }
    let ratio = (amount / volume).float();
    let mut taken = BTreeMap::new();
    let mut left = amount;
    let keys: Vec<String> = map.keys().cloned().collect();
    let last = keys.len().saturating_sub(1);
    for (index, reagent) in keys.into_iter().enumerate() {
        let have = map[&reagent];
        // Остаток округления — последнему реагенту.
        let part = if index == last {
            left.min(have)
        } else {
            have.scale(ratio).min(left)
        };
        left -= part;
        remove_reagent(map, &reagent, part);
        if part > FixedPoint2::ZERO {
            taken.insert(reagent, part);
        }
    }
    taken
}

/// Изменить кровотечение (`BleedModifierEvent`): системы меняют потерю крови и
/// ослабление за обновление. На теле.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BleedModifier {
    pub bleed_amount: f32,
    pub bleed_reduction_amount: f32,
}

fn now(world: &World) -> Duration {
    world.resource::<GameTime>().cur_time()
}

/// Уровень крови (`GetBloodLevel`); без кровотока — 0.
pub fn blood_level(world: &World, entity: Entity) -> f32 {
    world
        .get::<Bloodstream>(entity)
        .map_or(0.0, Bloodstream::blood_level)
}

/// Изменить кровотечение (`TryModifyBleedAmount`): от 0 до наибольшего, значок `Bleed` с
/// уровнем — целой частью кровотечения.
pub fn modify_bleed_amount(world: &mut World, entity: Entity, amount: f32) -> bool {
    let Some(mut bloodstream) = world.get_mut::<Bloodstream>(entity) else {
        return false;
    };
    bloodstream.bleed_amount =
        (bloodstream.bleed_amount + amount).clamp(0.0, bloodstream.max_bleed_amount);
    let bleed = bloodstream.bleed_amount;
    let alert = bloodstream.bleeding_alert.clone();
    if bleed == 0.0 {
        crate::alerts::clear_alert(world, entity, &alert);
    } else {
        let severity = bleed.trunc().clamp(0.0, 10.0) as i16;
        crate::alerts::show_alert(world, entity, &alert, Some(severity));
    }
    true
}

/// Поднять или опустить кровь (`TryModifyBloodLevel`).
pub fn modify_blood_level(world: &mut World, entity: Entity, amount: FixedPoint2) -> bool {
    world
        .get_mut::<Bloodstream>(entity)
        .is_some_and(|mut b| b.modify_level(amount))
}

/// Подвести кровь к норме (`TryRegulateBloodLevel`).
pub fn regulate_blood_level(world: &mut World, entity: Entity, amount: FixedPoint2) -> bool {
    world
        .get_mut::<Bloodstream>(entity)
        .is_some_and(|mut b| b.regulate(amount, 1.0))
}

/// Выпустить кровь (`TryBleedOut`).
pub fn bleed_out(world: &mut World, entity: Entity, amount: FixedPoint2) -> bool {
    world
        .get_mut::<Bloodstream>(entity)
        .is_some_and(|mut b| b.bleed_out(amount))
}

/// Течь кровью за обновление (`TickBleed`).
pub fn tick_bleed(world: &mut World, entity: Entity) {
    let Some(bloodstream) = world.get::<Bloodstream>(entity) else {
        return;
    };
    if bloodstream.bleed_amount <= 0.0 {
        return;
    }
    let mut event = BleedModifier {
        bleed_amount: bloodstream.bleed_amount,
        bleed_reduction_amount: bloodstream.bleed_reduction_amount,
    };
    world.raise(entity, &mut event);
    bleed_out(world, entity, FixedPoint2::from_f32(event.bleed_amount));
    modify_bleed_amount(world, entity, -event.bleed_reduction_amount);
}

/// Обновить одно тело (`Update`).
fn update_body(world: &mut World, entity: Entity) {
    if world.is_mob_dead(entity) {
        tick_bleed(world, entity);
        return;
    }
    let Some(refresh) = world
        .get::<Bloodstream>(entity)
        .map(|b| b.blood_refresh_amount)
    else {
        return;
    };
    regulate_blood_level(world, entity, refresh);
    tick_bleed(world, entity);
    let Some(bloodstream) = world.get::<Bloodstream>(entity).cloned() else {
        return;
    };
    let level = bloodstream.blood_level();
    if level < bloodstream.bloodloss_threshold {
        // Чем меньше крови, тем сильнее урон.
        let damage = bloodstream.bloodloss_damage.scale(1.0 / (0.1 + level));
        world.change_damage(entity, &damage, false, false, None);
    } else {
        let heal = bloodstream.bloodloss_heal_damage.scale(level);
        world.change_damage(entity, &heal, true, false, None);
    }
}

fn update(world: &mut World) {
    let time = now(world);
    let due: Vec<Entity> = world
        .query::<(Entity, &Bloodstream)>()
        .iter(world)
        .filter(|(_, b)| time >= b.next_update)
        .map(|(e, _)| e)
        .collect();
    for entity in due {
        if let Some(mut bloodstream) = world.get_mut::<Bloodstream>(entity) {
            let interval = bloodstream.adjusted_interval();
            bloodstream.next_update += interval;
        }
        update_body(world, entity);
    }
}

/// Наполнить кровью по эталону (`OnMapInit`).
fn on_map_init(world: &mut World, entity: Entity) {
    let time = now(world);
    let Some(mut bloodstream) = world.get_mut::<Bloodstream>(entity) else {
        return;
    };
    bloodstream.next_update = time + bloodstream.adjusted_interval();
    let reference = bloodstream.blood_reference_solution.volume();
    let missing = reference - bloodstream.blood_volume();
    if missing <= FixedPoint2::ZERO || reference == FixedPoint2::ZERO {
        return;
    }
    let ratio = (missing / reference).float();
    for reagent in bloodstream.blood_reference_solution.reagents.clone() {
        *bloodstream.blood.entry(reagent.reagent_id).or_default() += reagent.quantity.scale(ratio);
    }
}

/// Раны кровоточат, сильный удар выпускает кровь, ожог прижигает (`OnDamageChanged`).
fn on_damage_changed(world: &mut World, entity: Entity, event: &DamageChanged) {
    let Some(delta) = event.delta.as_ref().filter(|_| event.damage_increased) else {
        return;
    };
    let Some(bloodstream) = world.get::<Bloodstream>(entity).cloned() else {
        return;
    };
    let catalog = crate::damage::catalog(world);
    let Some(modifiers) = catalog
        .modifier_sets
        .get(&bloodstream.damage_bleed_modifiers)
    else {
        return;
    };
    // Лечение в том же изменении кровотечения не вызывает.
    let bloodloss = delta.positive().apply_modifier_set(modifiers);
    if bloodloss.is_empty() {
        return;
    }
    let old_bleed = bloodstream.bleed_amount;
    let total = bloodloss.total();
    let total_float = total.float();
    modify_bleed_amount(world, entity, total_float);
    // Сильный удар: шанс — кровотечение удара / 25, выпускает его пятую часть.
    let chance = (total_float / 25.0).clamp(0.0, 1.0);
    if total_float > 0.0 && crate::random::random(world).prob(chance) {
        bleed_out(world, entity, total / FixedPoint2::new(5));
        world.play_entity(&bloodstream.instant_blood_sound, entity, None);
    } else if total_float <= bloodstream.blood_healed_sound_threshold && old_bleed > 0.0 {
        let text = loc(world, "bloodstream-component-wounds-cauterized", &[]);
        world.popup_entity_for(&text, entity, entity, PopupType::Medium);
        world.play_entity(&bloodstream.blood_healed_sound, entity, None);
    }
}

/// Строки осмотра здоровья по кровотечению и бледности (`OnHealthBeingExamined`).
pub fn health_examine_lines(world: &World, entity: Entity) -> Vec<String> {
    let Some(bloodstream) = world.get::<Bloodstream>(entity) else {
        return Vec::new();
    };
    let bleed = bloodstream.bleed_amount;
    let max = bloodstream.max_bleed_amount;
    let key = if bleed > max * 0.75 {
        Some("bloodstream-component-massive-bleeding")
    } else if bleed > max * 0.5 {
        Some("bloodstream-component-strong-bleeding")
    } else if bleed > max * 0.25 {
        Some("bloodstream-component-bleeding")
    } else if bleed > 0.0 {
        Some("bloodstream-component-slight-bleeding")
    } else {
        None
    };
    let target = || {
        use astrocore_loc::EntityNameWorldExt;
        world.entity_arg(entity)
    };
    let mut lines = Vec::new();
    if let Some(key) = key {
        lines.push(loc(world, key, &[("target", target().into())]));
    }
    if bloodstream.blood_level() < bloodstream.bloodloss_threshold {
        lines.push(loc(
            world,
            "bloodstream-component-looks-pale",
            &[("target", target().into())],
        ));
    }
    lines
}

/// Полное исцеление: кровотечение — ноль, кровь — норма (`OnRejuvenate`).
fn on_rejuvenate(world: &mut World, entity: Entity) {
    let Some(bleed) = world.get::<Bloodstream>(entity).map(|b| b.bleed_amount) else {
        return;
    };
    modify_bleed_amount(world, entity, -bleed);
    if let Some(mut bloodstream) = world.get_mut::<Bloodstream>(entity) {
        bloodstream.blood.clear();
        let volume = bloodstream.blood_reference_solution.volume();
        bloodstream.regulate(volume, 1.0);
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Bloodstream>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<Bloodstream, MapInit>(|world, entity, _| on_map_init(world, entity));
    world.subscribe::<Bloodstream, DamageChanged>(|world, entity, event| {
        on_damage_changed(world, entity, event);
    });
    world.subscribe::<Bloodstream, HealthBeingExamined>(|world, entity, event| {
        let lines = health_examine_lines(world, entity);
        event.lines.extend(lines);
    });
    world.subscribe::<Bloodstream, Rejuvenate>(|world, entity, _| on_rejuvenate(world, entity));
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
