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

/// Система криокапсул: капсулы карты, окна, впрыск из буфера, укладывание пациента (действие с
/// задержкой), выход по кнопке или «сопротивлением» самого пациента.
#[derive(Default)]
pub struct CryoSystem {
    specs: std::collections::BTreeMap<astrocore_core::PrototypeId, CryoSpec>,
    pods: std::collections::BTreeMap<ObjectId, CryoPod>,
    open: std::collections::BTreeMap<NetworkEntityId, ObjectId>,
    sent: std::collections::BTreeMap<NetworkEntityId, Vec<u8>>,
}

const SYSTEM: &str = "cryo";

impl CryoSystem {
    /// Описания капсул из прототипов с компонентом `CryoPod`.
    pub(crate) fn from_registry(
        registry: &prototypes::PrototypeRegistry,
    ) -> Result<Self, prototypes::ProtoError> {
        let mut specs = std::collections::BTreeMap::new();
        for (id, name) in registry.engine_index(crate::module::NAMESPACE)? {
            if let Ok(r) = registry.spawnable(&name)
                && let Some(c) = CryoSpec::parse(&r)
            {
                specs.insert(id, c);
            }
        }
        Ok(Self {
            specs,
            ..Self::default()
        })
    }
}

impl crate::systems::GameSystem for CryoSystem {
    fn id(&self) -> &'static str {
        SYSTEM
    }

    fn commands(&self) -> &'static [&'static str] {
        use crate::wire::*;
        &[
            CMD_CRYO_PUT,
            CMD_CRYO_EJECT,
            CMD_CRYO_BEAKER_EJECT,
            CMD_CRYO_INSERT,
            CMD_CRYO_INJECT,
            CMD_CRYO_CLEAR,
            CMD_CRYO_CLOSE,
        ]
    }

    fn command(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        kind: &str,
        data: &[u8],
    ) {
        use crate::wire::*;
        if kind == CMD_CRYO_CLOSE {
            self.open.remove(&actor);
            return;
        }
        // Как и прочие команды, окно капсулы слушается только того, кто может действовать.
        if !st.can_act_now(actor) {
            return;
        }
        let Some(id) = self.open.get(&actor).copied() else {
            return;
        };
        match kind {
            CMD_CRYO_PUT => {
                if let Ok(b) = <[u8; 4]>::try_from(data) {
                    let target = NetworkEntityId(u32::from_le_bytes(b));
                    let delay = self.specs.values().next().map_or(2.0, |s| s.entry_delay);
                    st.begin(
                        ctx,
                        actor,
                        crate::doafter::Action::System(crate::doafter::SystemJob {
                            system: SYSTEM,
                            kind: "cryo",
                            delay,
                            breaks_on_move: true,
                            object: Some(id),
                            target: Some(target),
                            hand: None,
                        }),
                    );
                }
            }
            CMD_CRYO_EJECT => self.cryo_eject_patient(st, ctx, id),
            CMD_CRYO_BEAKER_EJECT => self.cryo_eject_beaker(st, ctx, actor, id),
            CMD_CRYO_INSERT => {
                if let [hand] = data[..] {
                    self.cryo_insert(st, ctx, actor, id, usize::from(hand));
                }
            }
            CMD_CRYO_INJECT => {
                if let [i] = data[..]
                    && let Some(a) = INJECT_AMOUNTS.get(usize::from(i))
                {
                    self.cryo_inject(st, id, *a);
                }
            }
            CMD_CRYO_CLEAR => self.cryo_clear(id),
            _ => {}
        }
    }

    fn before_command(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        kind: &str,
        _data: &[u8],
    ) {
        // Пациент выходит из капсулы «сопротивлением» (`TryEjectBody`).
        if kind == crate::wire::CMD_RESIST
            && let Some(pod) = st.actors.get(&actor).and_then(|a| a.cryo)
        {
            self.cryo_eject_patient(st, ctx, pod);
        }
    }

    fn finish_job(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        job: &crate::doafter::SystemJob,
    ) {
        let (Some(pod), Some(target)) = (job.object, job.target) else {
            return;
        };
        // Рядом ли ещё капсула и с тем, кто кладёт, и с тем, кого кладут.
        let near = |who: NetworkEntityId| {
            Self::cryo_pos(ctx, pod).is_some_and(|p| {
                ctx.position_of(who)
                    .is_some_and(|f| (p - f).length() <= REACH_M)
            })
        };
        if near(actor) && near(target) {
            self.cryo_put(st, ctx, pod, target);
        }
    }

    fn interact_hand(
        &mut self,
        st: &mut State,
        _ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        target: &crate::interact::Target,
    ) -> bool {
        let crate::interact::Target::Item { id, .. } = target else {
            return false;
        };
        self.open_cryo(st, user, *id)
    }

    fn interact_using(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        used: &crate::interact::Used,
        target: &crate::interact::Target,
    ) -> bool {
        let crate::interact::Target::Item { id, .. } = target else {
            return false;
        };
        self.pods.contains_key(id) && self.cryo_insert(st, ctx, user, *id, used.hand)
    }

    fn tick(&mut self, st: &mut State, ctx: &mut HookContext<'_>, _dt: f32) {
        self.tick_cryo(st, ctx);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl CryoSystem {
    pub(crate) fn sync_cryo(&mut self, st: &mut State, ctx: &HookContext<'_>) {
        let mut seen = std::collections::BTreeSet::new();
        for o in ctx.world.authored() {
            if let AuthoredKind::Placement { prototype } = &o.kind
                && self.specs.contains_key(prototype)
            {
                seen.insert(o.id);
                self.pods.entry(o.id).or_default();
            }
        }
        let gone: Vec<ObjectId> = self
            .pods
            .keys()
            .filter(|i| !seen.contains(i))
            .copied()
            .collect();
        for id in gone {
            if let Some(pod) = self.pods.remove(&id)
                && let Some(p) = pod.patient
            {
                st.actor(p).cryo = None;
            }
        }
    }

    pub(crate) fn cryo_pos(ctx: &HookContext<'_>, id: ObjectId) -> Option<Vec3> {
        let o = ctx.world.authored_object(id)?;
        Some((o.min() + o.max()) * 0.5)
    }

    /// Ключ воздушного узла капсулы в сети труб.
    pub(crate) fn cryo_key(st: &mut State, id: ObjectId) -> u64 {
        st.pipe_keys.key_of(id)
    }

    /// Пустая рука по капсуле открывает окно.
    pub(crate) fn open_cryo(
        &mut self,
        st: &mut State,
        user: NetworkEntityId,
        id: ObjectId,
    ) -> bool {
        if !self.pods.contains_key(&id) {
            return false;
        }
        if st.actors.get(&user).is_some_and(|a| a.cryo == Some(id)) {
            // Пациент не открывает собственное окно (`OnActivateUIAttempt`).
            return true;
        }
        self.open.insert(user, id);
        true
    }

    /// Мензурка из руки в слот (`beakerSlot`: `FitsInDispenser`).
    pub(crate) fn cryo_insert(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        id: ObjectId,
        hand: usize,
    ) -> bool {
        let Some(item) = st
            .actors
            .get(&user)
            .and_then(|a| a.hands.get(hand))
            .cloned()
        else {
            return false;
        };
        if !st.fits_in_dispenser.contains(&item.prototype) {
            return false;
        }
        let Some(pod) = self.pods.get_mut(&id) else {
            return false;
        };
        if pod.beaker.is_some() {
            return false;
        }
        pod.beaker = Some(item);
        let _ = st.actor(user).hands.drop_hand(hand);
        st.publish(ctx, user);
        true
    }

    pub(crate) fn cryo_eject_beaker(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        user: NetworkEntityId,
        id: ObjectId,
    ) {
        let Some(b) = self.pods.get_mut(&id).and_then(|p| p.beaker.take()) else {
            return;
        };
        if st.actor(user).hands.pickup_any(b.clone()).is_err()
            && let Some(feet) = ctx.position_of(user)
        {
            st.place(ctx, &b, feet);
        }
        st.publish(ctx, user);
    }

    /// `TryInject`: из мензурки в буфер, пока в буфере есть место и в капсуле есть пациент.
    pub(crate) fn cryo_inject(&mut self, st: &mut State, id: ObjectId, amount: f32) {
        let Some(pod) = self.pods.get(&id) else {
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
        let Some(src) = st.solution_mut(&beaker) else {
            return;
        };
        let part = src.split(amount.min(room));
        if let Some(pod) = self.pods.get_mut(&id) {
            pod.buffer.merge(&part);
        }
    }

    pub(crate) fn cryo_clear(&mut self, id: ObjectId) {
        if let Some(pod) = self.pods.get_mut(&id) {
            pod.buffer.contents.clear();
        }
    }

    /// `InsertBody`: пациент встаёт в капсулу и замирает.
    pub(crate) fn cryo_put(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        id: ObjectId,
        target: NetworkEntityId,
    ) {
        let Some(at) = Self::cryo_pos(ctx, id) else {
            return;
        };
        let free = self.pods.get(&id).is_some_and(|p| p.patient.is_none());
        let alone = st
            .actors
            .get(&target)
            .is_some_and(|a| a.cryo.is_none() && !a.ghost);
        if !free || !alone {
            return;
        }
        let next = st.clock + self.cryo_specs_for(ctx, id).transfer_time;
        if let Some(pod) = self.pods.get_mut(&id) {
            pod.patient = Some(target);
            pod.next_inject = next;
        }
        st.actor(target).cryo = Some(id);
        ctx.teleport(
            target,
            Vec3::new(at.x, ctx.position_of(target).map_or(at.y, |p| p.y), at.z),
        );
        st.cancel(ctx, target);
        st.after_health_change(ctx, target);
    }

    fn cryo_specs_for(&self, ctx: &HookContext<'_>, id: ObjectId) -> CryoSpec {
        ctx.world
            .authored_object(id)
            .and_then(|o| match &o.kind {
                AuthoredKind::Placement { prototype } => self.specs.get(prototype).cloned(),
                _ => None,
            })
            .unwrap_or(CryoSpec {
                transfer_time: 2.0,
                transfer_amount: 1.0,
                entry_delay: 2.0,
            })
    }

    /// `EjectBody`: пациент выходит из капсулы.
    pub(crate) fn cryo_eject_patient(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        id: ObjectId,
    ) {
        let Some(p) = self.pods.get_mut(&id).and_then(|c| c.patient.take()) else {
            return;
        };
        st.actor(p).cryo = None;
        st.after_health_change(ctx, p);
    }

    /// Окно капсулы для `p`.
    pub(crate) fn cryo_view(
        &mut self,
        st: &mut State,
        ctx: &HookContext<'_>,
        p: NetworkEntityId,
    ) -> Option<CryoView> {
        let id = self.open.get(&p).copied()?;
        let pos = Self::cryo_pos(ctx, id)?;
        let feet = ctx.position_of(p)?;
        if (pos - feet).length() > st.range_m + 1.0 {
            return None;
        }
        let pod = self.pods.get(&id)?.clone();
        let key = Self::cryo_key(st, id);
        let air = st.atmos.pipes().cryo_air(key).cloned();
        let patient = pod.patient.and_then(|n| {
            let a = st.actors.get(&n)?;
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
                st.actors
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
            powered: st.machine_powered(id),
            patient,
            air_temperature_k: air.as_ref().map_or(0.0, |a| a.temperature()),
            air_pressure_kpa: air.as_ref().map_or(0.0, |a| a.pressure()),
            gases,
            beaker: pod.beaker.as_ref().map(|b| st.container_info(b)),
            buffer: st.named(&pod.buffer.contents),
            buffer_volume: pod.buffer.volume(),
            nearby,
            amounts: INJECT_AMOUNTS.to_vec(),
        })
    }

    /// Шаг: впрыск из буфера и рассылка окон.
    pub(crate) fn tick_cryo(&mut self, st: &mut State, ctx: &mut HookContext<'_>) {
        if self.specs.is_empty() {
            return;
        }
        self.sync_cryo(st, ctx);
        let ids: Vec<ObjectId> = self.pods.keys().copied().collect();
        for id in ids {
            let powered = st.machine_powered(id);
            let spec = self.cryo_specs_for(ctx, id);
            let clock = st.clock;
            let Some(pod) = self.pods.get_mut(&id) else {
                continue;
            };
            let Some(patient) = pod.patient else { continue };
            if !powered || clock < pod.next_inject {
                continue;
            }
            pod.next_inject = clock + spec.transfer_time;
            let part = pod.buffer.split(spec.transfer_amount);
            if part.volume() > 0.0
                && let Some(body) = st.actor(patient).body.as_mut()
            {
                for (r, q) in &part.contents {
                    body.chemicals.add(r, *q);
                }
            }
        }
        for p in ctx.players().collect::<Vec<_>>() {
            let view = self.cryo_view(st, ctx, p);
            if view.is_none() {
                self.open.remove(&p);
            }
            let bytes = encode_cryo(view.as_ref());
            if self.sent.get(&p) != Some(&bytes) {
                ctx.set_component(p, CRYO_COMPONENT, bytes.clone());
                self.sent.insert(p, bytes);
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
