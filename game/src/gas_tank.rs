//! Газовый баллон: предмет с запасом газа. Через маску подключается к дыханию (internals) и
//! отдаёт газ под заданным давлением; клапан выпускает газ наружу и толкает баллон; выше
//! предела давления открывается сам, а при большем перегрузе баллон теряет прочность и
//! взрывается. Окно баллона — давление и подключение. Образец — `GasTankComponent`,
//! `GasMaxPressureHolderComponent`, `SharedGasTankSystem`, `GasTankSystem`,
//! `GasMaxPressureSystem` и `AtmosphereSystem.FlowGas`/`RemoveVolumeAtPressure` SS14
//! @ 2f4a2aad.
//!
//! Нет пока: вида прочности (`MaxPressureVisuals`), анализатора газа, цены. Баллоны вне
//! грида (в открытом космосе) не обновляются: устройства атмосферы работают на гридах.

use std::collections::BTreeMap;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EntityTerminating, EventWorldExt, GameTime, MapInit, RegistryWorldExt};
use astrocore_map::{ContainerWorldExt, EntParentChanged, TransformWorldExt};
use astrocore_physics::PhysicsWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::actions::{ActionEvent, ActionsWorldExt, GetItemActions, ensure_action};
use crate::atmos::{GasMixture, gas};
use crate::atmos_devices::AtmosDeviceUpdate;
use crate::internals::Internals;
use crate::popups::{PopupType, PopupWorldExt};
use crate::verbs::{GetVerbs, Verb, VerbKind};

/// Газы смеси в данных: список чисел по порядку газов или словарь по именам.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MolesData {
    List(Vec<f32>),
    Named(BTreeMap<String, f32>),
}

/// Смесь в данных (`GasMixture`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AirData {
    #[serde(default)]
    pub volume: f32,
    #[serde(default = "room")]
    pub temperature: f32,
    #[serde(default)]
    pub moles: Option<MolesData>,
}

impl Default for AirData {
    fn default() -> Self {
        Self {
            volume: 0.0,
            temperature: room(),
            moles: None,
        }
    }
}

fn room() -> f32 {
    gas::T20C
}

impl AirData {
    /// Смесь по данным.
    pub fn mixture(&self) -> GasMixture {
        let mut mixture = GasMixture::new(self.volume).with_temperature(self.temperature);
        match &self.moles {
            Some(MolesData::List(list)) => {
                for (index, moles) in list.iter().enumerate().take(gas::GAS_COUNT) {
                    mixture.moles[index] = *moles;
                }
            }
            Some(MolesData::Named(map)) => {
                for (name, moles) in map {
                    if let Some(index) = gas::gas_index(name) {
                        mixture.moles[index] = *moles;
                    }
                }
            }
            None => {}
        }
        mixture
    }
}

/// Окно баллона (`SharedGasTankUiKey.Key`).
pub const GAS_TANK_UI_KEY: &str = "enum.SharedGasTankUiKey.Key";
/// Задержка подключения (`GasTankDelay`), с.
const TOGGLE_DELAY: f64 = 1.0;
/// Ниже — выпуск газа беззвучный и баллон не толкает (`MinimumSoundValvePressure`).
const MINIMUM_SOUND_VALVE_PRESSURE: f32 = 21.3;
/// Сечение клапана (`ReleaseArea`), м².
const RELEASE_AREA: f32 = 0.0005;
/// кПа → кг/м² (`Atmospherics.kPaToKg_m2`).
const KPA_TO_KG_M2: f32 = 0.009_806_65;

fn one_atmosphere() -> f32 {
    gas::ONE_ATMOSPHERE
}
fn min_release() -> f32 {
    gas::ONE_ATMOSPHERE / 10.0
}
fn max_release() -> f32 {
    3.0 * gas::ONE_ATMOSPHERE
}
fn safety() -> f32 {
    15.0 * gas::ONE_ATMOSPHERE
}
fn overpressure() -> f32 {
    20.0 * gas::ONE_ATMOSPHERE
}
fn integrity() -> f32 {
    3.0
}
fn with_volume(mut sound: SoundSpecifier, volume: f32) -> SoundSpecifier {
    match &mut sound {
        SoundSpecifier::Path { params, .. } | SoundSpecifier::Collection { params, .. } => {
            params.volume = volume;
        }
    }
    sound
}
fn valve_sound() -> SoundSpecifier {
    with_volume(SoundSpecifier::collection("valveSqueak"), -5.0)
}
fn spray_sound() -> Option<SoundSpecifier> {
    Some(with_volume(
        SoundSpecifier::path("/Audio/Effects/spray.ogg"),
        -5.0,
    ))
}
fn rupture_sound() -> Option<SoundSpecifier> {
    Some(SoundSpecifier::path("/Audio/Effects/spray.ogg"))
}
fn connect_sound() -> Option<SoundSpecifier> {
    Some(with_volume(
        SoundSpecifier::path("/Audio/Effects/internals.ogg"),
        5.0,
    ))
}
fn toggle_action() -> String {
    "ActionToggleInternals".into()
}

/// Газовый баллон (`GasTankComponent`, `GasMaxPressureHolderComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasTank {
    #[serde(default)]
    pub air: AirData,
    #[serde(default = "min_release")]
    pub min_release_pressure: f32,
    #[serde(default = "max_release")]
    pub max_release_pressure: f32,
    #[serde(default = "one_atmosphere")]
    pub release_pressure: f32,
    #[serde(default)]
    pub release_valve_open: bool,
    #[serde(default = "valve_sound")]
    pub valve_sound: SoundSpecifier,
    #[serde(default = "spray_sound")]
    pub release_sound: Option<SoundSpecifier>,
    #[serde(default = "rupture_sound")]
    pub rupture_sound: Option<SoundSpecifier>,
    #[serde(default = "safety")]
    pub safety_pressure: f32,
    #[serde(default = "overpressure")]
    pub overpressure: f32,
    #[serde(default)]
    pub safety_alert: Option<String>,
    #[serde(default = "integrity")]
    pub max_integrity: f32,
    #[serde(default = "integrity")]
    pub integrity: f32,
    #[serde(default = "connect_sound")]
    pub connect_sound: Option<SoundSpecifier>,
    #[serde(default)]
    pub disconnect_sound: Option<SoundSpecifier>,
    #[serde(default = "one_atmosphere")]
    pub tank_low_pressure: f32,
    #[serde(default = "toggle_action")]
    pub toggle_action: String,
    /// Смесь в баллоне; заполняется из `air` при появлении.
    #[serde(skip)]
    pub mixture: Option<GasMixture>,
    /// Кто дышит из баллона (`User`).
    #[serde(skip)]
    pub user: Option<Entity>,
    #[serde(skip)]
    pub check_user: bool,
    #[serde(skip)]
    pub toggle_action_entity: Option<Entity>,
    /// Когда кончится задержка подключения (`UseDelay` `gasTank`), с.
    #[serde(skip)]
    pub delay_until: f64,
}

impl GasTank {
    pub fn is_connected(&self) -> bool {
        self.user.is_some()
    }

    fn pressure(&self) -> f32 {
        self.mixture.as_ref().map_or(0.0, GasMixture::pressure)
    }

    /// Давление ниже порога (`IsLowPressure`).
    pub fn is_low_pressure(&self) -> bool {
        self.pressure() <= self.tank_low_pressure
    }
}

/// Состояние окна баллона (`GasTankBoundUserInterfaceState` и поля баллона, которые клиент
/// SS14 берёт из компонента).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasTankUiState {
    pub tank_pressure: f32,
    pub release_pressure: f32,
    pub min_release_pressure: f32,
    pub max_release_pressure: f32,
    pub connected: bool,
    pub can_connect: bool,
}

/// Задать давление выпуска (`GasTankSetPressureMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasTankSetPressure {
    pub pressure: f32,
}

/// Подключить или отключить дыхание (`GasTankToggleInternalsMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasTankToggleInternals;

/// Смесь баллона.
pub fn tank_air(world: &World, tank: Entity) -> Option<&GasMixture> {
    world.get::<GasTank>(tank)?.mixture.as_ref()
}

/// Изменить смесь баллона.
pub fn with_tank_air<R>(
    world: &mut World,
    tank: Entity,
    f: impl FnOnce(&mut GasMixture) -> R,
) -> Option<R> {
    let mut component = world.get_mut::<GasTank>(tank)?;
    component.mixture.as_mut().map(f)
}

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

/// Газ из баллона под давлением выпуска (`RemoveAirOutput`): объём вдоха при давлении
/// выпуска, объём смеси — объём вдоха.
pub fn remove_air_output(world: &mut World, tank: Entity, volume: f32) -> Option<GasMixture> {
    let release = world.get::<GasTank>(tank)?.release_pressure;
    with_tank_air(world, tank, |air| {
        let temperature = air.temperature();
        let moles = if temperature > 0.0 {
            release * volume / (gas::R * temperature)
        } else {
            0.0
        };
        let mut mixture = air.remove(moles);
        mixture.volume = volume;
        mixture
    })
}

fn give_back(world: &mut World, tank: Entity, gas: &GasMixture) {
    let data = crate::atmos::gas_data(world);
    with_tank_air(world, tank, |air| data.merge(air, gas));
}

/// Кто может дышать из баллона (`TryGetInternalsComp`): пользователь или тот, в чьём
/// контейнере баллон.
fn internals_owner(world: &World, tank: Entity, user: Option<Entity>) -> Option<Entity> {
    let user = user.or_else(|| world.get::<GasTank>(tank).and_then(|t| t.user));
    if let Some(user) = user
        && world.get::<Internals>(user).is_some()
    {
        return Some(user);
    }
    let holder = world.containing_container(tank)?.owner;
    world.get::<Internals>(holder).is_some().then_some(holder)
}

/// Можно ли подключить (`CanConnectToInternals`): есть маска и клапан закрыт.
pub fn can_connect_to_internals(world: &World, tank: Entity) -> bool {
    let Some(component) = world.get::<GasTank>(tank) else {
        return false;
    };
    internals_owner(world, tank, None)
        .and_then(|owner| world.get::<Internals>(owner))
        .is_some_and(|i| !i.breath_tools.is_empty())
        && !component.release_valve_open
}

/// Задержка подключения кончилась; если да — начать новую (`TryResetDelay`).
fn try_reset_delay(world: &mut World, tank: Entity) -> bool {
    let time = now(world);
    let Some(mut component) = world.get_mut::<GasTank>(tank) else {
        return false;
    };
    if time < component.delay_until {
        return false;
    }
    component.delay_until = time + TOGGLE_DELAY;
    true
}

fn set_action_toggled(world: &mut World, tank: Entity, toggled: bool, cooldown: Option<f64>) {
    let Some(action) = world
        .get::<GasTank>(tank)
        .and_then(|t| t.toggle_action_entity)
    else {
        return;
    };
    if world.get_entity(action).is_err() {
        return;
    }
    world.set_toggled(action, toggled);
    if let Some(seconds) = cooldown {
        world.set_action_cooldown(action, seconds);
    }
}

/// Подключить к дыханию (`ConnectToInternals`).
pub fn connect_to_internals(world: &mut World, tank: Entity, user: Option<Entity>) -> bool {
    let Some(component) = world.get::<GasTank>(tank) else {
        return false;
    };
    if component.is_connected() || !can_connect_to_internals(world, tank) {
        return false;
    }
    let Some(owner) = internals_owner(world, tank, None) else {
        return false;
    };
    if !try_reset_delay(world, tank) {
        return false;
    }
    if crate::internals::try_connect_tank(world, owner, tank)
        && let Some(mut component) = world.get_mut::<GasTank>(tank)
    {
        component.user = Some(owner);
    }
    let connected = world
        .get::<GasTank>(tank)
        .is_some_and(GasTank::is_connected);
    set_action_toggled(world, tank, connected, Some(1.0));
    if !connected {
        return false;
    }
    if let Some(sound) = world
        .get::<GasTank>(tank)
        .and_then(|t| t.connect_sound.clone())
    {
        world.play_entity(&sound, tank, user);
    }
    update_user_interface(world, tank);
    true
}

/// Отключить от дыхания (`DisconnectFromInternals`).
pub fn disconnect_from_internals(
    world: &mut World,
    tank: Entity,
    user: Option<Entity>,
    forced: bool,
) -> bool {
    let Some(previous) = world.get::<GasTank>(tank).and_then(|t| t.user) else {
        return false;
    };
    if !forced && !try_reset_delay(world, tank) {
        return false;
    }
    let owner = internals_owner(world, tank, Some(previous));
    if let Some(mut component) = world.get_mut::<GasTank>(tank) {
        component.user = None;
    }
    set_action_toggled(world, tank, false, (!forced).then_some(TOGGLE_DELAY));
    if let Some(owner) = owner {
        crate::internals::disconnect_tank(world, owner, forced);
    }
    if let Some(sound) = world
        .get::<GasTank>(tank)
        .and_then(|t| t.disconnect_sound.clone())
    {
        world.play_entity(&sound, tank, user);
    }
    update_user_interface(world, tank);
    true
}

/// Переключить подключение (`ToggleInternals` баллона).
pub fn toggle_internals(world: &mut World, tank: Entity, user: Option<Entity>) -> bool {
    if world
        .get::<GasTank>(tank)
        .is_some_and(GasTank::is_connected)
    {
        disconnect_from_internals(world, tank, user, false)
    } else {
        connect_to_internals(world, tank, user)
    }
}

/// Открыть или закрыть клапан (`ToggleValve`).
pub fn set_valve(world: &mut World, tank: Entity, open: bool, user: Option<Entity>) {
    let Some(mut component) = world.get_mut::<GasTank>(tank) else {
        return;
    };
    component.release_valve_open = open;
    let sound = component.valve_sound.clone();
    world.play_entity(&sound, tank, user);
}

/// Состояние окна (`UpdateUserInterface`).
pub fn update_user_interface(world: &mut World, tank: Entity) {
    let Some(component) = world.get::<GasTank>(tank) else {
        return;
    };
    let state = GasTankUiState {
        tank_pressure: component.pressure(),
        release_pressure: component.release_pressure,
        min_release_pressure: component.min_release_pressure,
        max_release_pressure: component.max_release_pressure,
        connected: component.is_connected(),
        can_connect: can_connect_to_internals(world, tank),
    };
    astrocore_ui::UiWorldExt::set_ui_state(world, tank, GAS_TANK_UI_KEY, &state);
}

/// Поток газа через отверстие (`FlowGas` → `GetFlowVolume` → `ReleaseGasAt`): объём
/// `dt × сечение × √(2 ΔP V / m)` при давлении смеси.
pub fn flow_gas(
    data: &crate::atmos::GasData,
    mixture: &mut GasMixture,
    delta_p: f32,
    dt: f32,
    area: f32,
) -> Option<GasMixture> {
    if delta_p <= 0.0 || dt <= 0.0 {
        return None;
    }
    let mass = data.mass(mixture);
    if mass <= 0.0 {
        return None;
    }
    let velocity = (2.0 * delta_p * mixture.volume / mass).sqrt();
    let volume = dt * area * velocity;
    let target = mixture.pressure();
    if target <= 0.0 {
        return None;
    }
    let temperature = mixture.temperature();
    if temperature <= 0.0 {
        return None;
    }
    Some(mixture.remove(target * volume / (gas::R * temperature)))
}

/// Избыточное давление на объём (`GetOverPressure`).
pub fn over_pressure(mixture: &GasMixture) -> f32 {
    mixture.pressure() * mixture.volume
}

/// Выпуск газа через открытый клапан (`ReleaseGas`): газ уходит наружу, баллон толкает.
fn release_gas(world: &mut World, data: &crate::atmos::GasData, tank: Entity, dt: f32) {
    let Some(component) = world.get::<GasTank>(tank).cloned() else {
        return;
    };
    let pressure = component.pressure();
    let environment = crate::barotrauma::containing_mixture(world, tank).map(|m| m.pressure());
    let mut delta_p = match environment {
        Some(outside) => pressure - outside,
        None => pressure,
    };
    if delta_p < component.safety_pressure {
        delta_p = component.release_pressure.min(delta_p);
    }
    let Some(removed) = with_tank_air(world, tank, |air| {
        flow_gas(data, air, delta_p, dt, RELEASE_AREA)
    })
    .flatten() else {
        return;
    };
    crate::barotrauma::with_containing_mixture(world, tank, true, |air| data.merge(air, &removed));
    if removed.pressure() < MINIMUM_SOUND_VALVE_PRESSURE {
        return;
    }
    if let Some(sound) = &component.release_sound {
        world.play_entity(sound, tank, None);
    }
    let strength = over_pressure(&removed) * KPA_TO_KG_M2;
    if strength <= 0.0 || world.containing_container(tank).is_some() {
        return;
    }
    // Толчок вбок от сопла: угол между −1 и −0,5 рад от поворота баллона.
    let rotation = world.world_transform(tank).map_or(0.0, |t| t.rotation.0);
    let roll = crate::random::random(world).next_f32();
    let angle = rotation - 1.0 + 0.5 * roll;
    world.apply_angular_impulse(tank, strength * angle);
    let direction = astrocore_math::Vec2::new(angle.cos(), angle.sin());
    crate::throwing::try_throw(world, tank, direction * strength, strength, None, false);
}

/// Прочность и предохранитель (`CheckStatus`). `false` — баллон взорвался.
fn check_status(world: &mut World, data: &crate::atmos::GasData, tank: Entity, dt: f32) -> bool {
    let Some(component) = world.get::<GasTank>(tank).cloned() else {
        return false;
    };
    let pressure = component.pressure();
    if pressure > component.overpressure * (component.integrity + 1.0) {
        // Разрыв: газ наружу, баллон разрушен, взрыв силой √(избыточного давления).
        if let Some(air) = component.mixture.clone() {
            crate::barotrauma::with_containing_mixture(world, tank, true, |outside| {
                data.merge(outside, &air)
            });
            let intensity = over_pressure(&air).sqrt();
            if let Some(sound) = &component.rupture_sound
                && let Some(at) = world.map_coordinates(tank)
            {
                world.play_at(sound, at, None);
            }
            crate::destructible::destroy_entity(world, tank);
            crate::explosion::trigger_explosive_with(world, tank, None, Some(intensity));
        }
        return false;
    }
    if pressure > component.overpressure {
        if let Some(mut component) = world.get_mut::<GasTank>(tank) {
            component.integrity -= dt;
        }
    } else if component.integrity < component.max_integrity
        && let Some(mut component) = world.get_mut::<GasTank>(tank)
    {
        component.integrity = (component.integrity + dt).min(component.max_integrity);
    }
    // Предохранитель: открыть клапан (`SafetyMeasures`).
    if pressure > component.safety_pressure && !component.release_valve_open {
        set_valve(world, tank, true, None);
        if let Some(alert) = &component.safety_alert {
            let text = crate::examine::loc(world, alert, &[]);
            world.popup_entity(&text, tank, PopupType::LargeCaution);
        }
    }
    true
}

/// Шаг баллона как устройства атмосферы (`DeviceUpdated`).
fn device_update(world: &mut World, tank: Entity, dt: f32) {
    let data = crate::atmos::gas_data(world);
    if !check_status(world, &data, tank, dt) {
        return;
    }
    let Some(component) = world.get::<GasTank>(tank).cloned() else {
        return;
    };
    if component.release_valve_open {
        disconnect_from_internals(world, tank, None, false);
        release_gas(world, &data, tank, dt);
    } else if component.check_user {
        if let Some(mut c) = world.get_mut::<GasTank>(tank) {
            c.check_user = false;
        }
        if world.parent(tank) != component.user {
            disconnect_from_internals(world, tank, None, true);
        }
    }
    let reactions = crate::atmos::gas_reactions(world);
    with_tank_air(world, tank, |air| {
        let mut exposures = Vec::new();
        crate::atmos::fire::react(&data, &reactions, air, &mut exposures);
    });
    // Пока баллоном дышат или клапан открыт, давление в окне меняется.
    if component.is_connected() || component.release_valve_open {
        update_user_interface(world, tank);
    }
}

/// Строки осмотра (`OnExamined`).
fn on_examined(world: &mut World, tank: Entity, event: &mut crate::examine::Examined) {
    let Some(component) = world.get::<GasTank>(tank).cloned() else {
        return;
    };
    if event.is_in_details_range {
        let pressure = component.pressure().round() as i64;
        let text = crate::examine::loc(
            world,
            "comp-gas-tank-examine",
            &[("pressure", astrocore_loc::LocArg::from(pressure))],
        );
        event.push_markup(text, 0);
    }
    if component.is_connected() {
        let text = crate::examine::loc(world, "comp-gas-tank-connected", &[]);
        event.push_markup(text, 0);
    }
    let key = if component.release_valve_open {
        "comp-gas-tank-examine-open-valve"
    } else {
        "comp-gas-tank-examine-closed-valve"
    };
    let text = crate::examine::loc(world, key, &[]);
    event.push_markup(text, 0);
}

/// Глагол клапана (`OnGetAlternativeVerb`).
fn valve_verb(world: &mut World, tank: Entity, event: &mut GetVerbs) {
    if event.kind != VerbKind::Alternative || !event.can_access || !event.can_interact {
        return;
    }
    let Some(component) = world.get::<GasTank>(tank) else {
        return;
    };
    let (open, connected) = (component.release_valve_open, component.is_connected());
    let key = if open {
        "comp-gas-tank-close-valve"
    } else {
        "comp-gas-tank-open-valve"
    };
    let text = crate::examine::loc(world, key, &[]);
    let user = event.user;
    let mut verb = Verb::new(VerbKind::Alternative, text, move |world| {
        set_valve(world, tank, !open, Some(user));
    });
    verb.disabled = connected;
    event.verbs.push(verb);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<GasTank>();
    astrocore_ui::UiWorldExt::register_ui_message::<GasTankSetPressure>(world);
    astrocore_ui::UiWorldExt::register_ui_message::<GasTankToggleInternals>(world);
    if !side.is_server() {
        return;
    }
    world.insert_resource(crate::respiration::TankBreathing {
        remove_volume: remove_air_output,
        give_back,
    });
    world.subscribe::<GasTank, MapInit>(|world, tank, _| {
        if let Some(mut component) = world.get_mut::<GasTank>(tank)
            && component.mixture.is_none()
        {
            component.mixture = Some(component.air.mixture());
        }
        update_user_interface(world, tank);
    });
    world.subscribe::<GasTank, AtmosDeviceUpdate>(|world, tank, event| {
        device_update(world, tank, event.dt);
    });
    world.subscribe::<GasTank, EntParentChanged>(|world, tank, _| {
        if let Some(mut component) = world.get_mut::<GasTank>(tank) {
            component.check_user = true;
        }
    });
    world.subscribe::<GasTank, EntityTerminating>(|world, tank, _| {
        disconnect_from_internals(world, tank, None, true);
    });
    world.subscribe::<GasTank, crate::examine::Examined>(on_examined);
    world.subscribe::<GasTank, GetVerbs>(valve_verb);
    world.subscribe::<GasTank, GetItemActions>(|world, tank, event| {
        if world.get::<Internals>(event.user).is_none() {
            return;
        }
        let Some(component) = world.get::<GasTank>(tank) else {
            return;
        };
        let (existing, prototype) = (
            component.toggle_action_entity,
            component.toggle_action.clone(),
        );
        if let Some(action) = ensure_action(world, tank, existing, &prototype) {
            if let Some(mut component) = world.get_mut::<GasTank>(tank) {
                component.toggle_action_entity = Some(action);
            }
            let connected = world
                .get::<GasTank>(tank)
                .is_some_and(GasTank::is_connected);
            world.set_toggled(action, connected);
            event.actions.push(action);
        }
    });
    world.subscribe::<GasTank, ActionEvent>(|world, tank, event| {
        if event.handled || event.kind != "ToggleActionEvent" {
            return;
        }
        toggle_internals(world, tank, Some(event.performer));
        event.handled = true;
    });
    world.subscribe::<GasTank, astrocore_ui::BoundUiMessage<GasTankSetPressure>>(
        |world, tank, event| {
            if let Some(mut component) = world.get_mut::<GasTank>(tank) {
                component.release_pressure = event
                    .message
                    .pressure
                    .clamp(0.0, component.max_release_pressure);
            }
            update_user_interface(world, tank);
        },
    );
    world.subscribe::<GasTank, astrocore_ui::BoundUiMessage<GasTankToggleInternals>>(
        |world, tank, event| {
            toggle_internals(world, tank, Some(event.actor));
        },
    );
}

#[cfg(test)]
mod tests;
