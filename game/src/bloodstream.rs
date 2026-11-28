//! Кровоток: кровь — раствор `bloodstream` на теле по эталонному составу (с ДНК в данных
//! реагентов), вытекшая кровь — раствор `bloodstreamTemporary`, метаболиты — `metabolites`.
//! Кровотечение от ран, потеря крови и её урон, восстановление крови, значок
//! кровотечения, сообщения осмотра здоровья, прижигание ран ожогом, очистка крови от
//! лекарств и ядов, смена состава крови. Раз в 3 секунды живое тело восстанавливает
//! кровь, течёт кровью (кровотечение слабеет) и при крови ниже порога получает урон
//! `Bloodloss`, выше — лечит его; мёртвое только течёт. Реагенты крови не перерабатываются
//! метаболизмом, в крови не идут реакции с появлением сущностей, дымом и пеной. Образец —
//! `BloodstreamComponent`, `BloodstreamSystem`, эффекты `ModifyBleed`, `ModifyBloodLevel`,
//! `CleanBloodstream` SS14 @ 2f4a2aad.
//!
//! Как в SS14, восстановление крови меняет раствор без проведения изменений (без событий
//! и реакций). Луж пока нет: вытекшая кровь сверх порога лужи пропадает. Нет ДНК тела
//! (`DnaComponent`: в данных крови — «неизвестная ДНК») и головокружения от потери крови
//! (`StatusEffectBloodloss`).

use std::sync::Arc;
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
use crate::metabolism::MetabolismExclusion;
use crate::mobs::MobStateWorldExt;
use crate::popups::{PopupType, PopupWorldExt};
use crate::reactions::ReactionAttempt;
use crate::solution::{ReagentData, ReagentId, Solution};
use crate::solution_container::{self as containers, SolutionComponent};

/// Имена растворов кровотока по умолчанию.
pub const BLOOD_SOLUTION: &str = "bloodstream";
pub const BLOOD_TEMPORARY_SOLUTION: &str = "bloodstreamTemporary";
pub const METABOLITES_SOLUTION: &str = "metabolites";

fn reference_blood() -> Solution {
    Solution::from_reagents([(ReagentId::new("Blood"), FixedPoint2::new(600))])
}
fn blood_solution_name() -> String {
    BLOOD_SOLUTION.into()
}
fn temporary_solution_name() -> String {
    BLOOD_TEMPORARY_SOLUTION.into()
}
fn metabolites_solution_name() -> String {
    METABOLITES_SOLUTION.into()
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
    /// Эталонный состав крови.
    #[serde(default = "reference_blood")]
    pub blood_reference_solution: Solution,
    /// Данные реагентов крови этого тела (ДНК), при первом обращении.
    #[serde(skip)]
    pub blood_data: Option<Vec<ReagentData>>,
    #[serde(default = "blood_solution_name")]
    pub blood_solution_name: String,
    #[serde(default = "temporary_solution_name")]
    pub blood_temporary_solution_name: String,
    #[serde(default = "metabolites_solution_name")]
    pub metabolites_solution_name: String,
    #[serde(default = "bleeding_alert")]
    pub bleeding_alert: String,
}

impl Bloodstream {
    /// Интервал с учётом обмена веществ (`AdjustedUpdateInterval`).
    pub fn adjusted_interval(&self) -> Duration {
        Duration::from_secs_f32(self.update_interval * self.update_interval_multiplier)
    }
}

/// Раствор тела по имени без событий доступа (`ResolveSolution`).
pub fn named_solution(world: &World, entity: Entity, name: &str) -> Option<Entity> {
    containers::enumerate_solutions(world, entity)
        .into_iter()
        .find(|(id, _)| id == name)
        .map(|(_, soln)| soln)
}

/// Раствор крови тела.
pub fn blood_solution(world: &World, entity: Entity) -> Option<Entity> {
    let name = &world.get::<Bloodstream>(entity)?.blood_solution_name;
    named_solution(world, entity, name)
}

/// Всего крови в растворе крови (с лекарствами и ядами).
pub fn blood_volume(world: &World, entity: Entity) -> FixedPoint2 {
    blood_solution(world, entity)
        .and_then(|soln| containers::solution(world, soln))
        .map_or(FixedPoint2::ZERO, |s| s.volume)
}

/// Доля пригодной крови, от 0 до `max_volume_modifier` (`GetBloodLevel`): меньшая по
/// реагентам эталона; без кровотока или раствора — 0.
pub fn blood_level(world: &World, entity: Entity) -> f32 {
    let Some(bloodstream) = world.get::<Bloodstream>(entity) else {
        return 0.0;
    };
    let Some(solution) = blood_solution(world, entity).and_then(|s| containers::solution(world, s))
    else {
        return 0.0;
    };
    let reference = &bloodstream.blood_reference_solution;
    if reference.volume == FixedPoint2::ZERO {
        return 0.0;
    }
    let mut level = FixedPoint2::from_f32(bloodstream.max_volume_modifier);
    for (id, quantity) in &reference.contents {
        level = level.min(solution.prototype_quantity(&id.prototype) / *quantity);
    }
    level.float()
}

/// Подвести кровь к эталону × `reference_factor`, не больше доли `amount` по каждому
/// реагенту (`TryRegulateBloodLevel`). Раствор меняется без проведения изменений.
pub fn regulate_blood_level_to(
    world: &mut World,
    entity: Entity,
    amount: FixedPoint2,
    reference_factor: f32,
) -> bool {
    let Some(bloodstream) = world.get::<Bloodstream>(entity).cloned() else {
        return false;
    };
    let Some(soln) = blood_solution(world, entity) else {
        return false;
    };
    if amount == FixedPoint2::ZERO {
        return false;
    }
    if !bloodstream.blood_increase_enabled && amount > FixedPoint2::ZERO {
        return false;
    }
    let factor = reference_factor.clamp(0.0, bloodstream.max_volume_modifier);
    let reference = &bloodstream.blood_reference_solution;
    let ratio = amount.float() / reference.volume.float();
    let Some(mut component) = world.get_mut::<SolutionComponent>(soln) else {
        return false;
    };
    let solution = &mut component.solution;
    for (id, quantity) in &reference.contents {
        let error = quantity.scale(factor) - solution.prototype_quantity(&id.prototype);
        let adjusted = quantity.scale(ratio);
        if error > FixedPoint2::ZERO {
            solution.add_reagent(id.clone(), error.min(adjusted));
        } else if error < FixedPoint2::ZERO {
            solution.remove_reagent(id, (-error).min(adjusted), false);
        }
    }
    true
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

/// Поднять или опустить кровь на `amount`, не выше нормы (`TryModifyBloodLevel`).
pub fn modify_blood_level(world: &mut World, entity: Entity, amount: FixedPoint2) -> bool {
    if amount < FixedPoint2::ZERO {
        regulate_blood_level_to(world, entity, -amount, 0.0)
    } else {
        regulate_blood_level_to(world, entity, amount, 1.0)
    }
}

/// Подвести кровь к норме (`TryRegulateBloodLevel`).
pub fn regulate_blood_level(world: &mut World, entity: Entity, amount: FixedPoint2) -> bool {
    regulate_blood_level_to(world, entity, amount, 1.0)
}

/// Выпустить кровь (`TryBleedOut`): часть раствора крови по составу уходит в вытекшую;
/// больше порога лужи — разлив (луж пока нет: пропадает).
pub fn bleed_out(world: &mut World, entity: Entity, amount: FixedPoint2) -> bool {
    let Some(bloodstream) = world.get::<Bloodstream>(entity).cloned() else {
        return false;
    };
    let Some(soln) = blood_solution(world, entity) else {
        return false;
    };
    if amount <= FixedPoint2::ZERO {
        return false;
    }
    let leaked = containers::split_solution(world, soln, amount);
    let Some(temporary) = named_solution(world, entity, &bloodstream.blood_temporary_solution_name)
    else {
        return true;
    };
    let catalog = crate::solution::reagents(world);
    if let Some(mut component) = world.get_mut::<SolutionComponent>(temporary) {
        let solution = &mut component.solution;
        solution.add_solution(&leaked, &catalog);
        if solution.volume > bloodstream.bleed_puddle_threshold {
            // Лужи — с жидкостями (фаза 7.4).
            solution.remove_all();
        }
    }
    containers::update_chemicals(world, temporary, true);
    true
}

/// Добавить раствор в кровь (`TryAddToBloodstream`).
pub fn try_add_to_bloodstream(world: &mut World, entity: Entity, solution: &Solution) -> bool {
    if world.get::<Bloodstream>(entity).is_none() {
        return false;
    }
    let Some(soln) = blood_solution(world, entity) else {
        return false;
    };
    containers::try_add_solution(world, soln, solution)
}

/// Убрать из крови понемногу всё, кроме реагентов крови и исключённого
/// (`FlushChemicals`); `None` — ничего не убрано.
pub fn flush_chemicals(
    world: &mut World,
    entity: Entity,
    quantity: FixedPoint2,
    excluded: Option<&str>,
) -> Option<Solution> {
    let reference = world
        .get::<Bloodstream>(entity)?
        .blood_reference_solution
        .clone();
    let soln = blood_solution(world, entity)?;
    let contents = containers::solution(world, soln)?.contents.clone();
    let mut flushed = Solution::default();
    for (id, _) in contents.into_iter().rev() {
        if reference.contains_prototype(&id.prototype) || Some(id.prototype.as_str()) == excluded {
            continue;
        }
        let removed = containers::remove_reagent(world, soln, &id, quantity);
        flushed.add_reagent(id, removed);
    }
    (flushed.volume != FixedPoint2::ZERO).then_some(flushed)
}

/// Сменить состав крови на ходу (`ChangeBloodReagents`): кровь старого состава
/// заменяется тем же объёмом нового.
pub fn change_blood_reagents(world: &mut World, entity: Entity, reagents: &Solution) {
    let Some(old) = world
        .get::<Bloodstream>(entity)
        .map(|b| b.blood_reference_solution.clone())
    else {
        return;
    };
    let Some(soln) = blood_solution(world, entity) else {
        if let Some(mut bloodstream) = world.get_mut::<Bloodstream>(entity) {
            bloodstream.blood_reference_solution = reagents.clone();
        }
        return;
    };
    let mut current = FixedPoint2::ZERO;
    if let Some(mut component) = world.get_mut::<SolutionComponent>(soln) {
        for (id, _) in &old.contents {
            let volume = component.solution.volume;
            current += component.solution.remove_reagent(id, volume, true);
        }
    }
    let mut reference = reagents.clone();
    reference.scale_to(old.max_volume);
    if let Some(mut bloodstream) = world.get_mut::<Bloodstream>(entity) {
        bloodstream.blood_reference_solution = reference.clone();
    }
    if current == FixedPoint2::ZERO {
        return;
    }
    let mut solution = reference;
    solution.scale((current / solution.volume).float());
    containers::add_solution(world, soln, &solution);
}

/// Данные реагентов крови тела (`GetEntityBloodData`): ДНК; без ДНК тела —
/// «неизвестная».
pub fn entity_blood_data(world: &mut World, entity: Entity) -> Vec<ReagentData> {
    if let Some(data) = world
        .get::<Bloodstream>(entity)
        .and_then(|b| b.blood_data.clone())
    {
        return data;
    }
    let data = vec![ReagentData::Dna {
        dna: loc(world, "forensics-dna-unknown", &[]),
    }];
    if let Some(mut bloodstream) = world.get_mut::<Bloodstream>(entity) {
        bloodstream.blood_data = Some(data.clone());
    }
    data
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
    if blood_solution(world, entity).is_none() {
        return;
    }
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
    let level = blood_level(world, entity);
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

/// Растворы кровотока и кровь по эталону (`OnMapInit`).
fn on_map_init(world: &mut World, entity: Entity) {
    let time = now(world);
    let Some(mut bloodstream) = world.get_mut::<Bloodstream>(entity) else {
        return;
    };
    bloodstream.next_update = time + bloodstream.adjusted_interval();
    let bloodstream = bloodstream.clone();
    let Some((blood, _)) =
        containers::ensure_solution(world, entity, &bloodstream.blood_solution_name)
    else {
        return;
    };
    let temporary =
        containers::ensure_solution(world, entity, &bloodstream.blood_temporary_solution_name);
    let metabolites =
        containers::ensure_solution(world, entity, &bloodstream.metabolites_solution_name);
    let max = bloodstream
        .blood_reference_solution
        .volume
        .scale(bloodstream.max_volume_modifier);
    let set_max = |world: &mut World, soln: Entity, max: FixedPoint2| {
        if let Some(mut component) = world.get_mut::<SolutionComponent>(soln) {
            component.solution.max_volume = max;
        }
    };
    set_max(world, blood, max);
    if let Some((soln, _)) = metabolites {
        set_max(world, soln, max);
    }
    if let Some((soln, _)) = temporary {
        // С запасом, в том числе для химии.
        set_max(world, soln, bloodstream.bleed_puddle_threshold * 4);
    }
    let data = entity_blood_data(world, entity);
    let mut reference = bloodstream.blood_reference_solution.clone();
    reference.set_reagent_data(data);
    if let Some(mut component) = world.get_mut::<Bloodstream>(entity) {
        component.blood_reference_solution = reference.clone();
    }
    let catalog = crate::solution::reagents(world);
    if let Some(mut component) = world.get_mut::<SolutionComponent>(blood) {
        let mut fill = reference.clone();
        fill.scale_to(reference.volume - component.solution.volume);
        component.solution.add_solution(&fill, &catalog);
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
    if blood_level(world, entity) < bloodstream.bloodloss_threshold {
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
    let Some(bloodstream) = world.get::<Bloodstream>(entity).cloned() else {
        return;
    };
    modify_bleed_amount(world, entity, -bloodstream.bleed_amount);
    if let Some(soln) = blood_solution(world, entity) {
        containers::remove_all_solution(world, soln);
        regulate_blood_level(world, entity, bloodstream.blood_reference_solution.volume);
    }
}

/// В крови и вытекшей крови не идут реакции с появлением сущностей, дымом и пеной
/// (`OnReactionAttempt`).
fn on_reaction_attempt(world: &mut World, soln: Entity, event: &mut ReactionAttempt) {
    if event.cancelled {
        return;
    }
    let owner = containers::solution_owner(world, soln);
    let Some(bloodstream) = world.get::<Bloodstream>(owner) else {
        return;
    };
    let Some(id) = world.get::<SolutionComponent>(soln).map(|s| s.id.clone()) else {
        return;
    };
    if id != bloodstream.blood_solution_name && id != bloodstream.blood_temporary_solution_name {
        return;
    }
    let reactions = crate::reactions::reactions(world);
    if let Some(reaction) = reactions.get(&event.reaction)
        && reaction
            .effect_kinds
            .iter()
            .any(|kind| kind == "SpawnEntity" || kind == "AreaReactionEffect")
    {
        event.cancelled = true;
    }
}

/// Кровотечение сильнее или слабее (`ModifyBleed`).
#[derive(Debug)]
struct ModifyBleed {
    amount: f32,
}

impl crate::entity_effects::EffectKind for ModifyBleed {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        modify_bleed_amount(world, target, self.amount * scale);
    }
}

/// Больше или меньше крови (`ModifyBloodLevel`).
#[derive(Debug)]
struct ModifyBloodLevel {
    amount: FixedPoint2,
}

impl crate::entity_effects::EffectKind for ModifyBloodLevel {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        modify_blood_level(world, target, self.amount.scale(scale));
    }
}

/// Очистить кровь (`CleanBloodstream`).
#[derive(Debug)]
struct CleanBloodstream {
    cleanse_rate: FixedPoint2,
    excluded: Option<String>,
}

impl crate::entity_effects::EffectKind for CleanBloodstream {
    fn apply(&self, world: &mut World, target: Entity, scale: f32, _: Option<Entity>) {
        if world.get::<Bloodstream>(target).is_some() {
            flush_chemicals(
                world,
                target,
                self.cleanse_rate.scale(scale),
                self.excluded.as_deref(),
            );
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    use crate::entity_effects::{EffectsWorldExt, field, number, text};
    world.register_data_component::<Bloodstream>();
    world.register_effect("ModifyBleed", |node| {
        Some(Arc::new(ModifyBleed {
            amount: number(node, "amount").unwrap_or(-1.0),
        }))
    });
    world.register_effect("ModifyBloodLevel", |node| {
        Some(Arc::new(ModifyBloodLevel {
            amount: field(node, "amount").unwrap_or(FixedPoint2::new(1)),
        }))
    });
    world.register_effect("CleanBloodstream", |node| {
        Some(Arc::new(CleanBloodstream {
            cleanse_rate: field(node, "cleanseRate").unwrap_or(FixedPoint2::new(3)),
            excluded: text(node, "excluded"),
        }))
    });
    if !side.is_server() {
        return;
    }
    world.subscribe::<Bloodstream, MetabolismExclusion>(|world, entity, event| {
        if let Some(bloodstream) = world.get::<Bloodstream>(entity) {
            event.reagents.extend(
                bloodstream
                    .blood_reference_solution
                    .contents
                    .iter()
                    .map(|(id, _)| id.clone()),
            );
        }
    });
    world.subscribe::<SolutionComponent, ReactionAttempt>(on_reaction_attempt);
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
