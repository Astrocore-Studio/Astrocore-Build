//! Криокапсула (`SharedCryoPodSystem`, `CryoPodSystem`, снимок SS14 `910154a7`).
//!
//! Пациент стоит внутри (замёрзший на месте), дышит воздухом капсулы (`CryoPodAir`, 2000 л, обмен с сетью
//! порта и реакции газов — `Device::CryoPod`), а теплообмен с этим воздухом ускорен в 15 раз
//! (`InsideCryoPod.ConductanceMod`). Мензурка в слоте: кнопки 1/5/10/20 переливают в буфер (60 ед.), а пока
//! есть ток, раз в `beakerTransferTime` (2 с) `beakerTransferAmount` (1 ед.) уходит в кровь пациента. Вход —
//! 2 с (`entryDelay`), выход — кнопкой или «сопротивлением» самого пациента (Z).

use astrocore_api::HookContext;
use astrocore_core::{NetworkEntityId, ObjectId};
use astrocore_math::glam::Vec3;
use astrocore_world::AuthoredKind;
use prototypes::{Data, Resolved};
use serde::{Deserialize, Serialize};

use crate::hands::HeldItem;
use crate::module::State;
use crate::reagents::Solution;

pub const CRYO_COMPONENT: &str = "build.cryo";
pub const CRYO_MAX_BYTES: usize = 16384;
/// `SolutionCryoBuffer` (`SolutionSmall`).
pub const BUFFER_VOLUME: f32 = 60.0;
/// Кнопки переливания из мензурки в буфер (`Inject1/5/10/20`).
pub const INJECT_AMOUNTS: [f32; 4] = [1.0, 5.0, 10.0, 20.0];
/// Насколько рядом с капсулой можно положить человека, м.
pub const REACH_M: f32 = 2.5;

#[derive(Debug, Clone)]
pub(crate) struct CryoSpec {
    pub transfer_time: f32,
    pub transfer_amount: f32,
    pub entry_delay: f32,
}

impl CryoSpec {
    pub fn parse(r: &Resolved) -> Option<Self> {
        let c = r.component("CryoPod")?;
        let f = |c: &Data, k: &str, d: f32| c.get(k).and_then(Data::as_f64).map_or(d, |v| v as f32);
        Some(Self {
            transfer_time: f(c, "beakerTransferTime", 2.0),
            transfer_amount: f(c, "beakerTransferAmount", 1.0),
            entry_delay: f(c, "entryDelay", 2.0),
        })
    }
}

/// Капсула в мире.
#[derive(Debug, Clone)]
pub(crate) struct CryoPod {
    pub patient: Option<NetworkEntityId>,
    pub beaker: Option<HeldItem>,
    pub buffer: Solution,
    pub next_inject: f32,
}

impl Default for CryoPod {
    fn default() -> Self {
        Self {
            patient: None,
            beaker: None,
            buffer: Solution::new(BUFFER_VOLUME),
            next_inject: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatientInfo {
    pub state: String,
    pub temperature_k: f32,
    pub blood: f32,
    pub total_damage: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CryoView {
    pub machine: ObjectId,
    pub powered: bool,
    pub patient: Option<PatientInfo>,
    pub air_temperature_k: f32,
    pub air_pressure_kpa: f32,
    /// Газы капсулы: название и доля в процентах.
    pub gases: Vec<(String, f32)>,
    pub beaker: Option<crate::chemlab::ContainerInfo>,
    pub buffer: Vec<(String, String, f32)>,
    pub buffer_volume: f32,
    /// Кого можно положить: сетевой номер и подпись.
    pub nearby: Vec<(u32, String)>,
    pub amounts: Vec<f32>,
}

pub fn encode_cryo(v: Option<&CryoView>) -> Vec<u8> {
    v.map(|v| serde_json::to_vec(v).unwrap_or_default())
        .unwrap_or_default()
}

pub fn decode_cryo(bytes: &[u8]) -> Option<CryoView> {
    if bytes.is_empty() || bytes.len() > CRYO_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Воздух капсулы как среда для тела пациента.
pub(crate) struct CryoEnv<'a> {
    pub air: &'a mut atmos::GasMixture,
}

impl crate::body::Environment for CryoEnv<'_> {
    fn take_breath(&mut self, litres: f32) -> Option<atmos::GasMixture> {
        Some(self.air.remove_volume(litres))
    }

    fn give_back(&mut self, gas: &atmos::GasMixture) {
        self.air.merge(gas);
    }

    fn pressure(&self) -> f32 {
        self.air.pressure()
    }

    fn oxygen_moles(&self) -> f32 {
        self.air.moles(atmos::Gas::Oxygen)
    }

    fn air_thermal(&self) -> Option<(f32, f32)> {
        (self.air.total_moles() > 0.0).then(|| {
            (
                self.air.temperature(),
                self.air.heat_capacity() * atmos::HEAT_SCALE,
            )
        })
    }

    fn set_air_temperature(&mut self, t: f32) {
        self.air.set_temperature(t);
    }

    fn heat_transfer_modifier(&self) -> f32 {
        15.0
    }
}

impl State {
    pub(crate) fn sync_cryo(&mut self, ctx: &HookContext<'_>) {
        let mut seen = std::collections::BTreeSet::new();
        for o in ctx.world.authored() {
            if let AuthoredKind::Placement { prototype } = &o.kind
                && self.cryo_specs.contains_key(prototype)
            {
                seen.insert(o.id);
                self.cryo.entry(o.id).or_default();
            }
        }
        let gone: Vec<ObjectId> = self
            .cryo
            .keys()
            .filter(|i| !seen.contains(i))
            .copied()
            .collect();
        for id in gone {
            if let Some(pod) = self.cryo.remove(&id)
                && let Some(p) = pod.patient
            {
                self.actor(p).cryo = None;
            }
        }
    }

    pub(crate) fn cryo_pos(&self, ctx: &HookContext<'_>, id: ObjectId) -> Option<Vec3> {
        let o = ctx.world.authored_object(id)?;
        Some((o.min() + o.max()) * 0.5)
    }

    /// Ключ воздушного узла капсулы в сети труб.
    pub(crate) fn cryo_key(&mut self, id: ObjectId) -> u64 {
        self.pipe_keys.key_of(id)
    }

    /// Пустая рука по капсуле открывает окно.
    pub(crate) fn open_cryo(&mut self, user: NetworkEntityId, id: ObjectId) -> bool {
        if !self.cryo.contains_key(&id) {
            return false;
        }
        if self.actors.get(&user).is_some_and(|a| a.cryo == Some(id)) {
            // Пациент не открывает собственное окно (`OnActivateUIAttempt`).
            return true;
        }
        self.cryo_open.insert(user, id);
        true
    }

    /// Мензурка из руки в слот (`beakerSlot`: `FitsInDispenser`).
    pub(crate) fn cryo_insert(
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
        if !self.fits_in_dispenser.contains(&item.prototype) {
            return false;
        }
        let Some(pod) = self.cryo.get_mut(&id) else {
            return false;
        };
        if pod.beaker.is_some() {
            return false;
        }
        pod.beaker = Some(item);
        let _ = self.actor(user).hands.drop_hand(hand);
        self.publish(ctx, user);
        true
    }

    pub(crate) fn cryo_eject_beaker(
        &mut self,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        id: ObjectId,
    ) {
        let Some(b) = self.cryo.get_mut(&id).and_then(|p| p.beaker.take()) else {
            return;
        };
        if self.actor(user).hands.pickup_any(b.clone()).is_err()
            && let Some(feet) = ctx.position_of(user)
        {
            State::place(ctx, &b, feet);
        }
        self.publish(ctx, user);
    }

    /// `TryInject`: из мензурки в буфер, пока в буфере есть место и в капсуле есть пациент.
    pub(crate) fn cryo_inject(&mut self, id: ObjectId, amount: f32) {
        let Some(pod) = self.cryo.get(&id) else {
            return;
        };
        if pod.patient.is_none() {
            return;
        }
        let (Some(beaker), room) = (pod.beaker.clone(), pod.buffer.available()) else {
            return;
        };
        if room <= 0.0 {
            return;
        }
        let Some(src) = self.solution_mut(&beaker) else {
            return;
        };
        let part = src.split(amount.min(room));
        if let Some(pod) = self.cryo.get_mut(&id) {
            pod.buffer.merge(&part);
        }
    }

    pub(crate) fn cryo_clear(&mut self, id: ObjectId) {
        if let Some(pod) = self.cryo.get_mut(&id) {
            pod.buffer.contents.clear();
        }
    }

    /// `InsertBody`: пациент встаёт в капсулу и замирает.
    pub(crate) fn cryo_put(
        &mut self,
        ctx: &mut HookContext<'_>,
        id: ObjectId,
        target: NetworkEntityId,
    ) {
        let Some(at) = self.cryo_pos(ctx, id) else {
            return;
        };
        let free = self.cryo.get(&id).is_some_and(|p| p.patient.is_none());
        let alone = self
            .actors
            .get(&target)
            .is_some_and(|a| a.cryo.is_none() && !a.ghost);
        if !free || !alone {
            return;
        }
        let next = self.clock + self.cryo_specs_for(ctx, id).transfer_time;
        if let Some(pod) = self.cryo.get_mut(&id) {
            pod.patient = Some(target);
            pod.next_inject = next;
        }
        self.actor(target).cryo = Some(id);
        ctx.teleport(
            target,
            Vec3::new(at.x, ctx.position_of(target).map_or(at.y, |p| p.y), at.z),
        );
        self.cancel(ctx, target);
        self.after_health_change(ctx, target);
    }

    fn cryo_specs_for(&self, ctx: &HookContext<'_>, id: ObjectId) -> CryoSpec {
        ctx.world
            .authored_object(id)
            .and_then(|o| match &o.kind {
                AuthoredKind::Placement { prototype } => self.cryo_specs.get(prototype).cloned(),
                _ => None,
            })
            .unwrap_or(CryoSpec {
                transfer_time: 2.0,
                transfer_amount: 1.0,
                entry_delay: 2.0,
            })
    }

    /// `EjectBody`: пациент выходит из капсулы.
    pub(crate) fn cryo_eject_patient(&mut self, ctx: &mut HookContext<'_>, id: ObjectId) {
        let Some(p) = self.cryo.get_mut(&id).and_then(|c| c.patient.take()) else {
            return;
        };
        self.actor(p).cryo = None;
        self.after_health_change(ctx, p);
    }

    /// Окно капсулы для `p`.
    pub(crate) fn cryo_view(
        &mut self,
        ctx: &HookContext<'_>,
        p: NetworkEntityId,
    ) -> Option<CryoView> {
        let id = self.cryo_open.get(&p).copied()?;
        let pos = self.cryo_pos(ctx, id)?;
        let feet = ctx.position_of(p)?;
        if (pos - feet).length() > self.range_m + 1.0 {
            return None;
        }
        let pod = self.cryo.get(&id)?.clone();
        let key = self.cryo_key(id);
        let air = self.atmos.pipes().cryo_air(key).cloned();
        let patient = pod.patient.and_then(|n| {
            let a = self.actors.get(&n)?;
            let v = a.body.as_ref()?.vitals();
            Some(PatientInfo {
                state: format!("{:?}", a.health.state()),
                temperature_k: v.temperature,
                blood: v.blood,
                total_damage: a.health.total(),
            })
        });
        let mut gases = Vec::new();
        if let Some(a) = &air {
            let total = a.total_moles();
            if total > 0.0 {
                use atmos::Gas::*;
                for g in [
                    Oxygen,
                    Nitrogen,
                    CarbonDioxide,
                    Plasma,
                    Tritium,
                    WaterVapor,
                    Ammonia,
                    NitrousOxide,
                    Frezon,
                ] {
                    let m = a.moles(g);
                    if m > 0.0 {
                        gases.push((format!("{g:?}"), m / total * 100.0));
                    }
                }
            }
        }
        let nearby = ctx
            .players()
            .filter(|n| {
                self.actors
                    .get(n)
                    .is_some_and(|a| a.cryo.is_none() && !a.ghost)
                    && ctx
                        .position_of(*n)
                        .is_some_and(|q| (q - pos).length() <= REACH_M)
            })
            .map(|n| {
                let label = if n == p {
                    "Я".to_string()
                } else {
                    format!("Человек {}", n.0)
                };
                (n.0, label)
            })
            .collect();
        Some(CryoView {
            machine: id,
            powered: self.machine_powered(id),
            patient,
            air_temperature_k: air.as_ref().map_or(0.0, |a| a.temperature()),
            air_pressure_kpa: air.as_ref().map_or(0.0, |a| a.pressure()),
            gases,
            beaker: pod.beaker.as_ref().map(|b| self.container_info(b)),
            buffer: self.named(&pod.buffer.contents),
            buffer_volume: pod.buffer.volume(),
            nearby,
            amounts: INJECT_AMOUNTS.to_vec(),
        })
    }

    /// Шаг: впрыск из буфера и рассылка окон.
    pub(crate) fn tick_cryo(&mut self, ctx: &mut HookContext<'_>) {
        if self.cryo_specs.is_empty() {
            return;
        }
        self.sync_cryo(ctx);
        let ids: Vec<ObjectId> = self.cryo.keys().copied().collect();
        for id in ids {
            let powered = self.machine_powered(id);
            let spec = self.cryo_specs_for(ctx, id);
            let clock = self.clock;
            let Some(pod) = self.cryo.get_mut(&id) else {
                continue;
            };
            let Some(patient) = pod.patient else { continue };
            if !powered || clock < pod.next_inject {
                continue;
            }
            pod.next_inject = clock + spec.transfer_time;
            let part = pod.buffer.split(spec.transfer_amount);
            if part.volume() > 0.0
                && let Some(body) = self.actor(patient).body.as_mut()
            {
                for (r, q) in &part.contents {
                    body.chemicals.add(r, *q);
                }
            }
        }
        for p in ctx.players().collect::<Vec<_>>() {
            let view = self.cryo_view(ctx, p);
            if view.is_none() {
                self.cryo_open.remove(&p);
            }
            let bytes = encode_cryo(view.as_ref());
            if self.cryo_sent.get(&p) != Some(&bytes) {
                ctx.set_component(p, CRYO_COMPONENT, bytes.clone());
                self.cryo_sent.insert(p, bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;
    use crate::health::MobState;

    fn pod_air(kelvin: f32) -> atmos::GasMixture {
        let mut a = atmos::GasMixture::new(2000.0);
        // Давление около 101 кПа: n = PV / RT.
        let moles = 101.325 * 2000.0 / (8.314_462 * kelvin);
        a.set_moles(atmos::Gas::Oxygen, moles * 0.21);
        a.set_moles(atmos::Gas::Nitrogen, moles * 0.79);
        a.set_temperature(kelvin);
        a
    }

    fn cool(multiplier_env: bool) -> f32 {
        let mut body = Body::new(1);
        let mut air = pod_air(150.0);
        for _ in 0..60 {
            // Холодный газ из труб каждую секунду возвращает воздуху исходную температуру.
            air.set_temperature(150.0);
            if multiplier_env {
                let mut env = CryoEnv { air: &mut air };
                body.tick(1.0, &mut env, MobState::Alive, 0.0);
            } else {
                struct Plain<'a>(&'a mut atmos::GasMixture);
                impl crate::body::Environment for Plain<'_> {
                    fn take_breath(&mut self, l: f32) -> Option<atmos::GasMixture> {
                        Some(self.0.remove_volume(l))
                    }
                    fn give_back(&mut self, g: &atmos::GasMixture) {
                        self.0.merge(g);
                    }
                    fn pressure(&self) -> f32 {
                        self.0.pressure()
                    }
                    fn air_thermal(&self) -> Option<(f32, f32)> {
                        Some((
                            self.0.temperature(),
                            self.0.heat_capacity() * atmos::HEAT_SCALE,
                        ))
                    }
                    fn set_air_temperature(&mut self, t: f32) {
                        self.0.set_temperature(t);
                    }
                }
                let mut env = Plain(&mut air);
                body.tick(1.0, &mut env, MobState::Alive, 0.0);
            }
        }
        body.vitals().temperature
    }

    #[test]
    fn the_pod_air_cools_a_patient_much_faster_than_ordinary_air() {
        let (pod, plain) = (cool(true), cool(false));
        assert!(
            pod < plain - 5.0,
            "капсула {pod} К, обычный воздух {plain} К"
        );
        assert!(pod < 310.0);
    }
}
