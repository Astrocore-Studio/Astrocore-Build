//! Химлаборатория: смесители, плитка-нагреватель и ХимМастер (снимок SS14 `910154a7`).
//!
//! * `ReactionMixer` (`ReactionMixerSystem`): ложка (`Stir`), бумажная центрифуга (`Centrifuge`, в руке), шейкер
//!   (`Shake`), библия (`Holy`) — смешивают раствор за `timeToMix`, и тогда идут реакции с `requiredMixerCategories`.
//! * `SolutionContainerMixer` (`SharedSolutionContainerMixerSystem`): центрифуга и электролизёр — слот для
//!   мензурки, запуск на `mixDuration`, затем реакции с их видом смешивания; пока идёт работа, мензурку не вынуть.
//! * `SolutionHeater` (`SolutionHeaterSystem`): плитка греет всё с раствором на ней на `heatPerSecond` Дж/с.
//! * `ChemMaster` (`ChemMasterSystem`): буфер, перенос и удаление реагентов, таблетки и бутылки.

use std::collections::{BTreeMap, BTreeSet};

use astrocore_api::HookContext;
use astrocore_core::{NetworkEntityId, ObjectId, PrototypeId};
use astrocore_math::glam::Vec3;
use astrocore_world::AuthoredKind;
use prototypes::{Data, Resolved};
use serde::{Deserialize, Serialize};

use crate::hands::HeldItem;
use crate::module::State;
use crate::reagents::Solution;

pub const LAB_COMPONENT: &str = "build.lab";
pub const LAB_MAX_BYTES: usize = 16384;
/// `ChemMasterReagentAmount`: кнопки 1…120 и «всё».
pub const MASTER_AMOUNTS: [f32; 10] = [
    1.0,
    5.0,
    10.0,
    15.0,
    20.0,
    30.0,
    40.0,
    60.0,
    120.0,
    f32::MAX,
];
/// Раз в сколько секунд плитка отдаёт тепло.
const HEAT_STEP_S: f32 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MixerKind {
    /// Смешивание по применению на раствор.
    Machine,
    /// Смешивание по «использовать в руке», раствор у самого предмета.
    Handheld,
    /// Смешивает собственная система (шейкер).
    None,
}

#[derive(Debug, Clone)]
pub(crate) struct MixerSpec {
    pub types: Vec<String>,
    pub kind: MixerKind,
    pub time: f32,
}

/// Параметры из прототипов: смесители, нагреватели, машины.
#[derive(Debug, Default)]
pub(crate) struct LabSpecs {
    pub mixers: BTreeMap<PrototypeId, MixerSpec>,
    pub mixable: BTreeSet<PrototypeId>,
    /// `Shakeable`: время встряхивания, с.
    pub shakeable: BTreeMap<PrototypeId, f32>,
    pub heaters: BTreeMap<PrototypeId, f32>,
    /// `SolutionContainerMixer.mixDuration`.
    pub mixer_machines: BTreeMap<PrototypeId, f32>,
    /// `ChemMaster.pillDosageLimit`.
    pub masters: BTreeMap<PrototypeId, u32>,
}

impl LabSpecs {
    pub fn load(&mut self, id: &PrototypeId, r: &Resolved) {
        let f = |c: &Data, k: &str, d: f32| c.get(k).and_then(Data::as_f64).map_or(d, |v| v as f32);
        if let Some(c) = r.component("ReactionMixer") {
            let types = c
                .get("types")
                .and_then(Data::as_list)
                .map(|l| {
                    l.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let kind = match c.get("mixerType").and_then(Data::as_str) {
                Some("Handheld") => MixerKind::Handheld,
                Some("None") => MixerKind::None,
                _ => MixerKind::Machine,
            };
            self.mixers.insert(
                id.clone(),
                MixerSpec {
                    types,
                    kind,
                    time: f(c, "time", 0.0),
                },
            );
        }
        if r.component("MixableSolution").is_some() {
            self.mixable.insert(id.clone());
        }
        if let Some(c) = r.component("Shakeable") {
            self.shakeable.insert(id.clone(), f(c, "duration", 1.0));
        }
        if let Some(c) = r.component("SolutionHeater") {
            self.heaters.insert(id.clone(), f(c, "heatPerSecond", 0.0));
        }
        if let Some(c) = r.component("SolutionContainerMixer") {
            self.mixer_machines
                .insert(id.clone(), f(c, "mixDuration", 10.0));
        }
        if let Some(c) = r.component("ChemMaster") {
            self.masters
                .insert(id.clone(), f(c, "pillDosageLimit", 20.0) as u32);
        }
    }
}

/// Настольная центрифуга или электролизёр в мире.
#[derive(Debug, Clone, Default)]
pub(crate) struct MixerMachine {
    pub slot: Option<HeldItem>,
    /// Сколько работать ещё, с.
    pub mixing: Option<f32>,
}

/// ХимМастер в мире (`ChemMasterComponent`).
#[derive(Debug, Clone)]
pub(crate) struct Master {
    pub buffer: Solution,
    pub input: Option<HeldItem>,
    pub output: Option<HeldItem>,
    pub discard: bool,
    pub external: bool,
    /// `pillDosageLimit` прототипа.
    pub limit: u32,
}

impl Default for Master {
    fn default() -> Self {
        Self {
            // Буфер неограничен (`Solution` без `maxVol` у ХимМастера).
            buffer: Solution::new(1.0e9),
            input: None,
            output: None,
            discard: false,
            external: false,
            limit: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub name: String,
    pub volume: f32,
    pub max: f32,
    pub temperature: f32,
    /// Реагенты: идентификатор, название, количество.
    pub reagents: Vec<(String, String, f32)>,
    /// Предметы внутри (таблетки в банке): название и доза.
    pub items: Vec<(String, f32)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabView {
    /// `"mixer"` — центрифуга и электролизёр, `"master"` — ХимМастер.
    pub kind: String,
    pub machine: ObjectId,
    pub title: String,
    pub input: Option<ContainerInfo>,
    pub output: Option<ContainerInfo>,
    /// Осталось работать, с; `None`, если машина простаивает.
    pub mixing: Option<f32>,
    pub duration: f32,
    pub buffer: Vec<(String, String, f32)>,
    pub buffer_volume: f32,
    pub discard: bool,
    pub dosage_limit: u32,
    pub external: bool,
}

pub fn encode_lab(v: Option<&LabView>) -> Vec<u8> {
    v.map(|v| serde_json::to_vec(v).unwrap_or_default())
        .unwrap_or_default()
}

pub fn decode_lab(bytes: &[u8]) -> Option<LabView> {
    if bytes.is_empty() || bytes.len() > LAB_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Запрос «налить или выбросить реагент»: `from_buffer`, номер кнопки количества, реагент.
pub fn parse_reagent(data: &[u8]) -> Option<(bool, usize, String)> {
    let (&b, rest) = data.split_first()?;
    let (&a, name) = rest.split_first()?;
    let name = std::str::from_utf8(name).ok()?;
    (usize::from(a) < MASTER_AMOUNTS.len() && name.len() <= 64)
        .then(|| (b == 1, usize::from(a), name.to_string()))
}

/// Запрос «сделать таблетки или бутылку»: доза, число, ярлык.
pub fn parse_make(data: &[u8]) -> Option<(u32, u32, String)> {
    let [d, n, label @ ..] = data else {
        return None;
    };
    let label = std::str::from_utf8(label).ok()?;
    (label.chars().count() <= 50).then(|| (u32::from(*d), u32::from(*n), label.to_string()))
}

impl State {
    /// Заведение машин по объектам мира (как у раздатчиков).
    pub(crate) fn sync_lab(&mut self, ctx: &HookContext<'_>) {
        let mut mixers = BTreeSet::new();
        let mut masters = BTreeSet::new();
        for o in ctx.world.authored() {
            if let AuthoredKind::Placement { prototype } = &o.kind {
                if self.lab.mixer_machines.contains_key(prototype) {
                    mixers.insert(o.id);
                    self.mixer_machines.entry(o.id).or_default();
                }
                if let Some(limit) = self.lab.masters.get(prototype) {
                    masters.insert(o.id);
                    self.masters.entry(o.id).or_insert_with(|| Master {
                        limit: *limit,
                        ..Master::default()
                    });
                }
            }
        }
        self.mixer_machines.retain(|id, _| mixers.contains(id));
        self.masters.retain(|id, _| masters.contains(id));
    }

    pub(crate) fn machine_powered(&self, id: ObjectId) -> bool {
        !self.power_required || self.powered_map().get(&id).copied().unwrap_or(false)
    }

    fn solution_of(&self, item: &HeldItem) -> Option<&Solution> {
        self.solutions
            .get(&item.id)
            .or_else(|| self.solution_templates.get(&item.prototype))
    }

    fn container_info(&self, item: &HeldItem) -> ContainerInfo {
        let s = self.solution_of(item);
        let mut items = Vec::new();
        if let Some(st) = self.storages.get(&item.id) {
            for stored in st.items() {
                let dose = self.solution_of(&stored.item).map_or(0.0, Solution::volume);
                let name = self
                    .labels
                    .get(&stored.item.id)
                    .cloned()
                    .unwrap_or_else(|| stored.item.prototype.to_string());
                items.push((name, dose));
            }
        }
        ContainerInfo {
            name: self
                .labels
                .get(&item.id)
                .cloned()
                .unwrap_or_else(|| item.prototype.to_string()),
            volume: s.map_or(0.0, Solution::volume),
            max: s.map_or(0.0, |s| s.max_volume),
            temperature: s.map_or(0.0, |s| s.temperature),
            reagents: s.map_or_else(Vec::new, |s| self.named(&s.contents)),
            items,
        }
    }

    fn named(&self, contents: &BTreeMap<String, f32>) -> Vec<(String, String, f32)> {
        contents
            .iter()
            .map(|(k, v)| {
                let name = self
                    .chem
                    .reagents
                    .get(k)
                    .map_or_else(|| k.clone(), |r| r.name.clone());
                (k.clone(), name, *v)
            })
            .collect()
    }

    /// Окно машины для игрока `p`, если он рядом и машина есть.
    pub(crate) fn lab_view(&self, ctx: &HookContext<'_>, p: NetworkEntityId) -> Option<LabView> {
        let id = self.lab_open.get(&p).copied()?;
        let o = ctx.world.authored_object(id)?;
        let feet = ctx.position_of(p)?;
        if (((o.min() + o.max()) * 0.5) - feet).length() > self.range_m + 1.0 {
            return None;
        }
        let AuthoredKind::Placement { prototype } = &o.kind else {
            return None;
        };
        let title = self
            .titles
            .get(prototype)
            .cloned()
            .unwrap_or_else(|| prototype.to_string());
        if let Some(m) = self.mixer_machines.get(&id) {
            return Some(LabView {
                kind: "mixer".into(),
                machine: id,
                title,
                input: m.slot.as_ref().map(|i| self.container_info(i)),
                output: None,
                mixing: m.mixing,
                duration: self
                    .lab
                    .mixer_machines
                    .get(prototype)
                    .copied()
                    .unwrap_or(0.0),
                buffer: Vec::new(),
                buffer_volume: 0.0,
                discard: false,
                dosage_limit: 0,
                external: false,
            });
        }
        let m = self.masters.get(&id)?;
        Some(LabView {
            kind: "master".into(),
            machine: id,
            title,
            input: m.input.as_ref().map(|i| self.container_info(i)),
            output: m.output.as_ref().map(|i| self.container_info(i)),
            mixing: None,
            duration: 0.0,
            buffer: self.named(&m.buffer.contents),
            buffer_volume: m.buffer.volume(),
            discard: m.discard,
            dosage_limit: self.lab.masters.get(prototype).copied().unwrap_or(0),
            external: m.external,
        })
    }

    /// Рассылка окон игрокам (по изменению).
    pub(crate) fn tick_lab_views(&mut self, ctx: &mut HookContext<'_>) {
        for p in ctx.players().collect::<Vec<_>>() {
            let view = self.lab_view(ctx, p);
            if view.is_none() {
                self.lab_open.remove(&p);
            }
            let bytes = encode_lab(view.as_ref());
            if self.lab_sent.get(&p) != Some(&bytes) {
                ctx.set_component(p, LAB_COMPONENT, bytes.clone());
                self.lab_sent.insert(p, bytes);
            }
        }
    }

    /// Окно открывает пустая рука при наличии тока.
    pub(crate) fn open_lab(&mut self, user: NetworkEntityId, id: ObjectId) -> bool {
        if !self.mixer_machines.contains_key(&id) && !self.masters.contains_key(&id) {
            return false;
        }
        if self.machine_powered(id) {
            self.lab_open.insert(user, id);
        } else {
            self.popup(user, "Нет питания");
        }
        true
    }

    /// Мензурка из руки в слот машины (центрифуга — `mixer`, ХимМастер — `beakerSlot`/`outputSlot`).
    pub(crate) fn lab_insert(
        &mut self,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        id: ObjectId,
        hand: usize,
    ) -> bool {
        let Some(item) = self
            .actors
            .get(&user)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return false;
        };
        let tags = self
            .info(&item.prototype)
            .map(|i| i.tags.clone())
            .unwrap_or_default();
        let fits = self.fits_in_dispenser.contains(&item.prototype);
        let is_output = tags.contains("Bottle") || tags.contains("PillCanister");
        let taken = if let Some(m) = self.mixer_machines.get_mut(&id) {
            if fits && m.slot.is_none() && m.mixing.is_none() {
                m.slot = Some(item.clone());
                true
            } else {
                false
            }
        } else if let Some(m) = self.masters.get_mut(&id) {
            if is_output && m.output.is_none() {
                m.output = Some(item.clone());
                true
            } else if fits && !tags.contains("Bottle") && m.input.is_none() {
                m.input = Some(item.clone());
                true
            } else {
                false
            }
        } else {
            false
        };
        if taken {
            let _ = self.actor(user).hands.drop_hand(hand);
            self.publish(ctx, user);
        } else {
            self.popup(user, "Сюда это не поставить");
        }
        taken
    }

    /// Вынуть: `0` — вход (мензурка), `1` — выход (бутылка или банка).
    pub(crate) fn lab_eject(
        &mut self,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        id: ObjectId,
        slot: u8,
    ) {
        let item = if let Some(m) = self.mixer_machines.get_mut(&id) {
            if m.mixing.is_some() {
                self.popup(user, "Идёт работа");
                return;
            }
            m.slot.take()
        } else if let Some(m) = self.masters.get_mut(&id) {
            if slot == 1 {
                m.output.take()
            } else {
                m.input.take()
            }
        } else {
            None
        };
        if let Some(item) = item {
            if self.actor(user).hands.pickup_any(item.clone()).is_err()
                && let Some(feet) = ctx.position_of(user)
            {
                State::place(ctx, &item, feet);
            }
            self.publish(ctx, user);
        }
    }

    /// Пуск центрифуги или электролизёра (`TryStartMix`).
    pub(crate) fn lab_start(&mut self, ctx: &HookContext<'_>, user: NetworkEntityId, id: ObjectId) {
        let Some(duration) = ctx.world.authored_object(id).and_then(|o| match &o.kind {
            AuthoredKind::Placement { prototype } => {
                self.lab.mixer_machines.get(prototype).copied()
            }
            _ => None,
        }) else {
            return;
        };
        let powered = self.machine_powered(id);
        let Some(m) = self.mixer_machines.get_mut(&id) else {
            return;
        };
        if m.mixing.is_some() {
            return;
        }
        if !powered {
            self.popup(user, "Нет питания");
            return;
        }
        if m.slot.is_none() {
            self.popup(user, "Нечего смешивать");
            return;
        }
        m.mixing = Some(duration);
        if let Some(at) = ctx.position_of(user) {
            self.sound("Machines/buzz_loop.ogg", at);
        }
    }

    /// Ход времени: машины работают, плитки греют.
    pub(crate) fn tick_lab(&mut self, ctx: &mut HookContext<'_>, dt: f32) {
        if self.lab.mixer_machines.is_empty()
            && self.lab.masters.is_empty()
            && self.lab.heaters.is_empty()
        {
            return;
        }
        self.sync_lab(ctx);
        let ids: Vec<ObjectId> = self.mixer_machines.keys().copied().collect();
        for id in ids {
            let Some(m) = self.mixer_machines.get_mut(&id) else {
                continue;
            };
            let Some(left) = m.mixing else { continue };
            let left = left - dt;
            if left > 0.0 {
                m.mixing = Some(left);
                continue;
            }
            m.mixing = None;
            let slot = m.slot.clone();
            let types: Vec<String> = ctx
                .world
                .authored_object(id)
                .and_then(|o| match &o.kind {
                    AuthoredKind::Placement { prototype } => {
                        self.lab.mixers.get(prototype).map(|s| s.types.clone())
                    }
                    _ => None,
                })
                .unwrap_or_default();
            if let Some(item) = slot {
                self.react_item(&item, Some(&types));
            }
        }
        self.heat_timer += dt;
        if self.heat_timer >= HEAT_STEP_S {
            let step = std::mem::take(&mut self.heat_timer);
            self.tick_heaters(ctx, step);
        }
        self.tick_lab_views(ctx);
    }

    /// Реакции в растворе предмета (со смесителем или без).
    pub(crate) fn react_item(&mut self, item: &HeldItem, mixer: Option<&[String]>) -> bool {
        let chem = self.chem.clone();
        self.solution_mut(item)
            .is_some_and(|s| chem.react_with(s, mixer))
    }

    /// Плитки: всё с раствором, лежащее сверху, получает `heatPerSecond · dt` Дж.
    fn tick_heaters(&mut self, ctx: &HookContext<'_>, dt: f32) {
        if self.lab.heaters.is_empty() {
            return;
        }
        let mut plates: Vec<(ObjectId, Vec3, Vec3, f32)> = Vec::new();
        let mut items: Vec<(HeldItem, Vec3)> = Vec::new();
        for o in ctx.world.authored() {
            let AuthoredKind::Placement { prototype } = &o.kind else {
                continue;
            };
            if let Some(w) = self.lab.heaters.get(prototype) {
                plates.push((o.id, o.min(), o.max(), *w));
            } else if self.fits_in_dispenser.contains(prototype) {
                items.push((
                    HeldItem {
                        id: o.id,
                        prototype: prototype.clone(),
                    },
                    (o.min() + o.max()) * 0.5,
                ));
            }
        }
        for (plate, min, max, watts) in plates {
            if !self.machine_powered(plate) {
                continue;
            }
            for (item, at) in &items {
                let over = at.x >= min.x - 0.1
                    && at.x <= max.x + 0.1
                    && at.z >= min.z - 0.1
                    && at.z <= max.z + 0.1
                    && at.y >= min.y - 0.1
                    && at.y <= max.y + 1.0;
                if !over {
                    continue;
                }
                let chem = self.chem.clone();
                if let Some(s) = self.solution_mut(item) {
                    s.add_thermal_energy(watts * dt);
                    chem.react(s);
                }
            }
        }
    }

    // --- Смесители в руках (`ReactionMixerSystem`) ---

    /// Можно ли начать или завершить смешивание.
    pub(crate) fn mix_can(
        &self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        target: Option<ObjectId>,
        shake: bool,
    ) -> bool {
        let Some(item) = self.actors.get(&actor).and_then(|a| a.hands.get(hand)) else {
            return false;
        };
        if shake {
            return self.lab.shakeable.contains_key(&item.prototype);
        }
        if !self.lab.mixers.contains_key(&item.prototype) {
            return false;
        }
        let Some(id) = target else {
            return self.solution_volume(item) > 0.0;
        };
        let Some(o) = ctx.world.authored_object(id) else {
            return false;
        };
        let AuthoredKind::Placement { prototype } = &o.kind else {
            return false;
        };
        let held = HeldItem {
            id,
            prototype: prototype.clone(),
        };
        let near = ctx
            .position_of(actor)
            .is_some_and(|f| (((o.min() + o.max()) * 0.5) - f).length() <= self.range_m + 0.5);
        near && self.lab.mixable.contains(prototype) && self.solution_volume(&held) > 0.0
    }

    pub(crate) fn mix_delay(&self, actor: NetworkEntityId, hand: usize, shake: bool) -> f32 {
        let Some(item) = self.actors.get(&actor).and_then(|a| a.hands.get(hand)) else {
            return 0.0;
        };
        if shake {
            return self
                .lab
                .shakeable
                .get(&item.prototype)
                .copied()
                .unwrap_or(1.0);
        }
        self.lab.mixers.get(&item.prototype).map_or(0.0, |m| m.time)
    }

    /// Завершение: в растворе идут реакции, требующие умения смесителя.
    pub(crate) fn mix_run(
        &mut self,
        ctx: &HookContext<'_>,
        actor: NetworkEntityId,
        hand: usize,
        target: Option<ObjectId>,
        shake: bool,
    ) {
        let Some(item) = self
            .actors
            .get(&actor)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return;
        };
        let types: Vec<String> = self
            .lab
            .mixers
            .get(&item.prototype)
            .map(|m| m.types.clone())
            .unwrap_or_default();
        let subject = match target {
            Some(id) => {
                let proto = ctx.world.authored_object(id).and_then(|o| match &o.kind {
                    AuthoredKind::Placement { prototype } => Some(prototype.clone()),
                    _ => None,
                });
                match proto {
                    Some(prototype) => HeldItem { id, prototype },
                    None => return,
                }
            }
            None => item.clone(),
        };
        let reacted = self.react_item(&subject, Some(&types));
        // Звук встряхивания (`soda_shake.ogg`) под CC-BY-NC, в сборку не берётся.
        if !shake && let Some(at) = ctx.position_of(actor) {
            self.sound("Items/Medical/paper_centrifuge.ogg", at);
        }
        self.popup(
            actor,
            if reacted {
                "Смесь прореагировала"
            } else {
                "Смешано"
            },
        );
    }

    // --- ХимМастер ---

    fn master_input_solution(&mut self, id: ObjectId) -> Option<&mut Solution> {
        let item = self.masters.get(&id)?.input.clone()?;
        self.solution_mut(&item)
    }

    /// `TransferReagents` / `DiscardReagents`.
    pub(crate) fn master_reagent(
        &mut self,
        id: ObjectId,
        from_buffer: bool,
        amount_idx: usize,
        reagent: &str,
    ) {
        let amount = MASTER_AMOUNTS[amount_idx];
        let Some(m) = self.masters.get(&id) else {
            return;
        };
        let (discard, has_input) = (m.discard, m.input.is_some());
        if !has_input {
            return;
        }
        if discard {
            if from_buffer {
                if let Some(m) = self.masters.get_mut(&id) {
                    m.buffer
                        .remove(reagent, amount.min(m.buffer.amount(reagent)));
                }
            } else if let Some(s) = self.master_input_solution(id) {
                let have = s.amount(reagent);
                s.remove(reagent, amount.min(have));
            }
            return;
        }
        if from_buffer {
            let room = self
                .master_input_solution(id)
                .map_or(0.0, |s| s.available());
            let Some(m) = self.masters.get_mut(&id) else {
                return;
            };
            let take = amount.min(m.buffer.amount(reagent)).min(room);
            m.buffer.remove(reagent, take);
            let chem = self.chem.clone();
            if let Some(s) = self.master_input_solution(id) {
                s.add(reagent, take);
                chem.react(s);
            }
        } else {
            let Some(s) = self.master_input_solution(id) else {
                return;
            };
            let take = amount.min(s.amount(reagent));
            s.remove(reagent, take);
            if let Some(m) = self.masters.get_mut(&id) {
                m.buffer.add(reagent, take);
            }
        }
    }

    /// `WithdrawFromSource`: забрать `needed` из буфера или из мензурки.
    fn master_withdraw(
        &mut self,
        user: NetworkEntityId,
        id: ObjectId,
        needed: f32,
    ) -> Option<Solution> {
        let external = self.masters.get(&id)?.external;
        if external {
            if self.masters.get(&id)?.input.is_none() {
                self.popup(user, "Нет мензурки");
                return None;
            }
            let s = self.master_input_solution(id)?;
            if s.volume() <= 0.0 {
                self.popup(user, "Мензурка пуста");
                return None;
            }
            if needed > s.volume() {
                self.popup(user, "В мензурке не хватает");
                return None;
            }
            Some(s.split(needed))
        } else {
            let m = self.masters.get_mut(&id)?;
            if m.buffer.volume() <= 0.0 {
                self.popup(user, "Буфер пуст");
                return None;
            }
            if needed > m.buffer.volume() {
                self.popup(user, "В буфере не хватает");
                return None;
            }
            Some(m.buffer.split(needed))
        }
    }

    /// `OnCreatePillsMessage`: таблетки в банку на выходе.
    pub(crate) fn master_pills(
        &mut self,
        user: NetworkEntityId,
        id: ObjectId,
        dosage: u32,
        number: u32,
        label: &str,
    ) {
        let Some(m) = self.masters.get(&id) else {
            return;
        };
        let Some(canister) = m.output.clone() else {
            return;
        };
        let limit = m.limit;
        let Some(spec) = self
            .info(&canister.prototype)
            .and_then(|i| i.storage.clone())
        else {
            return;
        };
        let used: usize = self
            .storages
            .get(&canister.id)
            .map_or(0, |s| s.items().iter().map(|x| x.cells().len()).sum());
        if number == 0 || spec.cells.len().saturating_sub(used) < number as usize {
            return;
        }
        if dosage == 0 || dosage > limit {
            return;
        }
        let Some(mut taken) = self.master_withdraw(user, id, (dosage * number) as f32) else {
            return;
        };
        if !label.is_empty() {
            self.labels.insert(canister.id, label.to_string());
        }
        for _ in 0..number {
            let Some(pill) = self.create_item("Pill") else {
                break;
            };
            let part = taken.split(dosage as f32);
            let mut s = Solution::new(dosage as f32);
            s.can_react = false;
            s.temperature = part.temperature;
            for (k, v) in &part.contents {
                s.add(k, *v);
            }
            self.solutions.insert(pill.id, s);
            if !label.is_empty() {
                self.labels.insert(pill.id, label.to_string());
            }
            if let Some(info) = self.info(&pill.prototype).cloned() {
                let _ = self
                    .storages
                    .entry(canister.id)
                    .or_default()
                    .insert(&spec, pill, &info);
            }
        }
        self.popup(user, format!("Таблеток: {number}"));
    }

    /// `OnOutputToBottleMessage`.
    pub(crate) fn master_bottle(
        &mut self,
        user: NetworkEntityId,
        id: ObjectId,
        dosage: u32,
        label: &str,
    ) {
        let Some(bottle) = self.masters.get(&id).and_then(|m| m.output.clone()) else {
            return;
        };
        let Some(room) = self.solution_mut(&bottle).map(|s| s.available()) else {
            return;
        };
        if dosage == 0 || dosage as f32 > room {
            return;
        }
        let Some(taken) = self.master_withdraw(user, id, dosage as f32) else {
            return;
        };
        if !label.is_empty() {
            self.labels.insert(bottle.id, label.to_string());
        }
        let chem = self.chem.clone();
        if let Some(s) = self.solution_mut(&bottle) {
            s.merge(&taken);
            chem.react(s);
        }
        self.popup(user, format!("Налито в бутылку: {dosage} ед."));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_requests_parse_strictly() {
        assert_eq!(
            parse_reagent(&[1, 2, b'A', b'b']),
            Some((true, 2, "Ab".to_string()))
        );
        assert_eq!(parse_reagent(&[1, 99, b'A']), None);
        assert_eq!(parse_make(&[20, 3, b'x']), Some((20, 3, "x".to_string())));
        assert_eq!(parse_make(&[1]), None);
    }
}
