//! Двери и шлюзы: открытие и закрытие в две фазы, отказ без доступа, автозакрытие,
//! болты, аварийный доступ, раздавливание, открытие толчком, взлом ломом. Образец —
//! `DoorComponent`, `DoorBoltComponent`, `AirlockComponent`, `SharedDoorSystem` (и
//! `.Bolts`), `SharedAirlockSystem`, серверные `DoorSystem` и `AirlockSystem`,
//! `PryingComponent`, `PryUnpoweredComponent`, `PryingSystem` SS14 @ 2f4a2aad.
//!
//! Закрытая дверь твёрдая и непрозрачная; на середине открытия она становится проходимой,
//! на середине закрытия — снова твёрдой (`Partial`). Оглушение раздавленных — вместе со
//! статус-эффектами; провода, электрификация и сигналы шлюза — в 5.3.

use std::collections::{BTreeSet, HashSet};

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{
    ComponentInit, EntityTerminating, EventWorldExt, GameTime, GameWorldExt, MapInit, Paused,
    RegistryWorldExt, Update,
};
use astrocore_light::Occluder;
use astrocore_map::{MapGrid, TileCoord, Transform};
use astrocore_math::{Box2, Vec2};
use astrocore_net::AppearanceWorldExt;
use astrocore_physics::{
    BodyType, CollisionGroups, Fixtures, Physics, PhysicsWorldExt, PreventCollide, StartCollide,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::access::{AccessReader, is_allowed};
use crate::airtight::set_airblocked;
use crate::content::groups;
use crate::damage::{DamageSpecifier, DamageableWorldExt};
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterStart, DoAfterWorldExt};
use crate::examine::loc;
use crate::interaction::{ActivateInWorld, InteractUsing};
use crate::popups::{PopupType, PopupWorldExt};
use crate::power::{PowerChanged, is_powered};
use crate::tag::TagWorldExt;
use crate::verbs::{GetVerbs, Verb, VerbKind};
use crate::weldable::{WeldableAttempt, is_welded};

/// Ключ внешнего вида: состояние двери (`DoorVisuals.State`).
pub const DOOR_STATE: &str = "enum.DoorVisuals.State";
/// Огни болтов (`DoorVisuals.BoltLights`).
pub const BOLT_LIGHTS: &str = "enum.DoorVisuals.BoltLights";
/// Огни аварийного доступа (`DoorVisuals.EmergencyLights`).
pub const EMERGENCY_LIGHTS: &str = "enum.DoorVisuals.EmergencyLights";
/// Огни закрытой двери (`DoorVisuals.ClosedLights`).
pub const CLOSED_LIGHTS: &str = "enum.DoorVisuals.ClosedLights";
/// Тег существ и предметов, открывающих двери толчком (`DoorBumpOpener`).
pub const DOOR_BUMP_TAG: &str = "DoorBumpOpener";

/// Состояние двери (`DoorState`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DoorState {
    #[default]
    Closed,
    Closing,
    Open,
    Opening,
    Denying,
    Emagging,
}

impl DoorState {
    /// Имя состояния, как в SS14: оно же значение внешнего вида.
    pub fn name(self) -> &'static str {
        match self {
            Self::Closed => "Closed",
            Self::Closing => "Closing",
            Self::Open => "Open",
            Self::Opening => "Opening",
            Self::Denying => "Denying",
            Self::Emagging => "Emagging",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "Closed" => Self::Closed,
            "Closing" => Self::Closing,
            "Open" => Self::Open,
            "Opening" => Self::Opening,
            "Denying" => Self::Denying,
            "Emagging" => Self::Emagging,
            _ => return None,
        })
    }
}

fn yes() -> bool {
    true
}

macro_rules! seconds {
    ($($name:ident = $value:expr;)*) => {
        $(fn $name() -> f64 { $value })*
    };
}

seconds! {
    close_time_one = 0.4;
    close_time_two = 0.2;
    open_time_one = 0.4;
    open_time_two = 0.2;
    deny_duration = 0.45;
    emag_duration = 0.8;
    door_stun_time = 2.0;
    pry_time = 1.5;
    animation_time = 0.8;
    emag_animation_time = 1.5;
    auto_close_delay = 5.0;
}

fn state_open() -> Option<String> {
    Some("open".into())
}
fn state_closed() -> Option<String> {
    Some("closed".into())
}
fn state_opening() -> Option<String> {
    Some("opening".into())
}
fn state_closing() -> Option<String> {
    Some("closing".into())
}
fn state_sparks() -> Option<String> {
    Some("sparks".into())
}
fn prying() -> String {
    "Prying".into()
}
fn sparks() -> SoundSpecifier {
    SoundSpecifier::collection("sparks")
}

/// Дверь (`DoorComponent`). Времена — в секундах.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Door {
    #[serde(default)]
    pub state: DoorState,
    #[serde(default = "close_time_one")]
    pub close_time_one: f64,
    #[serde(default = "close_time_two")]
    pub close_time_two: f64,
    #[serde(default = "open_time_one")]
    pub open_time_one: f64,
    #[serde(default = "open_time_two")]
    pub open_time_two: f64,
    #[serde(default = "deny_duration")]
    pub deny_duration: f64,
    #[serde(default = "emag_duration")]
    pub emag_duration: f64,
    /// Время следующей смены состояния, секунды игры (`NextStateChange`).
    #[serde(skip)]
    pub next_state_change: Option<f64>,
    #[serde(default)]
    pub partial: bool,
    #[serde(default)]
    pub open_sound: Option<SoundSpecifier>,
    #[serde(default)]
    pub close_sound: Option<SoundSpecifier>,
    #[serde(default)]
    pub deny_sound: Option<SoundSpecifier>,
    #[serde(default = "sparks")]
    pub spark_sound: SoundSpecifier,
    #[serde(default = "door_stun_time")]
    pub door_stun_time: f64,
    #[serde(default)]
    pub crush_damage: Option<DamageSpecifier>,
    #[serde(default = "yes")]
    pub can_crush: bool,
    #[serde(default = "yes")]
    pub perform_collision_check: bool,
    /// Кого дверь сейчас давит (`CurrentlyCrushing`).
    #[serde(skip)]
    pub currently_crushing: BTreeSet<Entity>,
    #[serde(default = "state_open")]
    pub open_sprite_state: Option<String>,
    #[serde(default = "state_closed")]
    pub closed_sprite_state: Option<String>,
    #[serde(default = "state_opening")]
    pub opening_sprite_state: Option<String>,
    #[serde(default = "state_closing")]
    pub closing_sprite_state: Option<String>,
    #[serde(default = "state_sparks")]
    pub emagging_sprite_state: Option<String>,
    #[serde(default = "animation_time")]
    pub opening_animation_time: f64,
    #[serde(default = "animation_time")]
    pub closing_animation_time: f64,
    #[serde(default = "emag_animation_time")]
    pub emagging_animation_time: f64,
    #[serde(default = "yes")]
    pub can_pry: bool,
    #[serde(default = "prying")]
    pub prying_quality: String,
    #[serde(default = "pry_time")]
    pub pry_time: f64,
    #[serde(default = "yes")]
    pub change_airtight: bool,
    #[serde(default = "yes")]
    pub occludes: bool,
    #[serde(default = "yes")]
    pub bump_open: bool,
    #[serde(default = "yes")]
    pub click_open: bool,
    #[serde(default)]
    pub check_fixture_collision: bool,
    #[serde(default)]
    pub allow_machine_layer: bool,
    /// Глубина отрисовки открытой и закрытой двери: имя или число.
    #[serde(default)]
    pub open_draw_depth: Option<DepthName>,
    #[serde(default)]
    pub closed_draw_depth: Option<DepthName>,
}

/// Глубина отрисовки в данных: имя константы `DrawDepth` или число.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DepthName {
    Number(i32),
    Name(String),
}

/// Болты (`DoorBoltComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoorBolt {
    #[serde(default = "bolt_up_sound")]
    pub bolt_up_sound: SoundSpecifier,
    #[serde(default = "bolt_down_sound")]
    pub bolt_down_sound: SoundSpecifier,
    #[serde(default)]
    pub bolts_down: bool,
    #[serde(default = "yes")]
    pub bolt_lights_enabled: bool,
    #[serde(default)]
    pub bolt_wire_cut: bool,
    #[serde(default)]
    pub powered: bool,
}

fn bolt_up_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/boltsup.ogg")
}
fn bolt_down_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/boltsdown.ogg")
}
fn emergency_on_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/airlock_emergencyon.ogg")
}
fn emergency_off_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/airlock_emergencyoff.ogg")
}
fn nine() -> f32 {
    9.0
}
fn three() -> f32 {
    3.0
}
fn one() -> f32 {
    1.0
}
fn pry_failed() -> String {
    "airlock-component-cannot-pry-is-powered-message".into()
}

/// Шлюз (`AirlockComponent`): питание, автозакрытие, безопасность, аварийный доступ.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Airlock {
    #[serde(default)]
    pub powered: bool,
    #[serde(default = "yes")]
    pub safety: bool,
    #[serde(default)]
    pub emergency_access: bool,
    #[serde(default = "emergency_on_sound")]
    pub emergency_on_sound: SoundSpecifier,
    #[serde(default = "emergency_off_sound")]
    pub emergency_off_sound: SoundSpecifier,
    #[serde(default = "nine")]
    pub powered_pry_modifier: f32,
    #[serde(default)]
    pub open_panel_visible: bool,
    #[serde(default)]
    pub keep_open_if_clicked: bool,
    #[serde(default = "yes")]
    pub auto_close: bool,
    #[serde(default = "auto_close_delay")]
    pub auto_close_delay: f64,
    #[serde(default = "one")]
    pub auto_close_delay_modifier: f32,
    #[serde(default = "pry_failed")]
    pub pry_failed_popup: String,
    #[serde(default)]
    pub open_unlit_visible: bool,
    #[serde(default = "yes")]
    pub emergency_access_layer: bool,
    #[serde(default = "yes")]
    pub animate_panel: bool,
    #[serde(default = "three")]
    pub bolted_pry_modifier: f32,
    /// Состояния спрайта огней и панели (клиент).
    #[serde(flatten)]
    pub sprite_states: AirlockSpriteStates,
}

/// Состояния спрайта шлюза для клиента.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AirlockSpriteStates {
    #[serde(default = "opening_unlit")]
    pub opening_sprite_state: Option<String>,
    #[serde(default = "panel_opening")]
    pub opening_panel_sprite_state: Option<String>,
    #[serde(default = "panel_open")]
    pub open_panel_sprite_state: Option<String>,
    #[serde(default = "closing_unlit")]
    pub closing_sprite_state: Option<String>,
    #[serde(default = "panel_closing")]
    pub closing_panel_sprite_state: Option<String>,
    #[serde(default = "open_unlit")]
    pub open_sprite_state: Option<String>,
    #[serde(default = "closed_unlit")]
    pub closed_sprite_state: Option<String>,
    #[serde(default = "deny_unlit")]
    pub deny_sprite_state: Option<String>,
    #[serde(default = "deny_animation_time")]
    pub deny_animation_time: f64,
}

fn opening_unlit() -> Option<String> {
    Some("opening_unlit".into())
}
fn panel_opening() -> Option<String> {
    Some("panel_opening".into())
}
fn panel_open() -> Option<String> {
    Some("panel_open".into())
}
fn closing_unlit() -> Option<String> {
    Some("closing_unlit".into())
}
fn panel_closing() -> Option<String> {
    Some("panel_closing".into())
}
fn open_unlit() -> Option<String> {
    Some("open_unlit".into())
}
fn closed_unlit() -> Option<String> {
    Some("closed_unlit".into())
}
fn deny_unlit() -> Option<String> {
    Some("deny_unlit".into())
}
fn deny_animation_time() -> f64 {
    0.3
}

/// Взламывающий двери инструмент или существо (`PryingComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prying {
    /// Открывает запитанные шлюзы.
    #[serde(default)]
    pub pry_powered: bool,
    /// Открывает и заболченные.
    #[serde(default)]
    pub force: bool,
    #[serde(default = "one")]
    pub speed_modifier: f32,
    #[serde(default = "crowbar_sound")]
    pub use_sound: SoundSpecifier,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn crowbar_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Items/crowbar.ogg")
}
fn tenth() -> f32 {
    0.1
}

/// Дверь можно открыть руками без инструмента (`PryUnpoweredComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PryUnpowered {
    #[serde(default = "tenth")]
    pub pry_modifier: f32,
}

fn pry_modifier() -> f32 {
    1.5
}
fn pressure_threshold() -> f32 {
    20.0
}
fn temperature_threshold() -> f32 {
    330.0
}
fn close_cooldown() -> f64 {
    2.0
}

/// Пожарный шлюз (`FirelockComponent`): закрытый открывает любой; держит давление и огонь,
/// закрывается по тревоге (поведение — `firelock.rs`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Firelock {
    #[serde(default = "pry_modifier")]
    pub locked_pry_time_modifier: f32,
    #[serde(default = "pressure_threshold")]
    pub pressure_threshold: f32,
    #[serde(default = "temperature_threshold")]
    pub temperature_threshold: f32,
    #[serde(default = "yes")]
    pub alarm_auto_close: bool,
    /// Секунды (`EmergencyCloseCooldownDuration`).
    #[serde(default = "close_cooldown")]
    pub emergency_close_cooldown_duration: f64,
    #[serde(skip)]
    pub emergency_close_cooldown: Option<f64>,
    #[serde(default)]
    pub pressure: bool,
    #[serde(default)]
    pub temperature: bool,
    #[serde(default)]
    pub powered: bool,
}

impl Default for Firelock {
    fn default() -> Self {
        astrocore_data::from_node(&astrocore_data::DataNode::map(
            astrocore_data::DataMap::new(),
        ))
        .expect("пустой пожарный шлюз разбирается")
    }
}

impl Firelock {
    /// Держит давление или огонь (`IsLocked`).
    pub fn is_locked(&self) -> bool {
        self.pressure || self.temperature
    }
}

/// Двери, у которых назначена смена состояния. Ресурс сервера.
#[derive(Resource, Debug, Default)]
pub struct ActiveDoors(pub HashSet<Entity>);

/// Состояние двери изменилось (`DoorStateChangedEvent`). Поднимается на двери.
#[derive(Debug, Clone, Copy)]
pub struct DoorStateChanged {
    pub state: DoorState,
}

/// Перед открытием (`BeforeDoorOpenedEvent`): обработчик может запретить.
#[derive(Debug, Clone, Copy)]
pub struct BeforeDoorOpened {
    pub user: Option<Entity>,
    pub cancelled: bool,
}

/// Перед закрытием (`BeforeDoorClosedEvent`).
#[derive(Debug, Clone, Copy)]
pub struct BeforeDoorClosed {
    pub perform_collision_check: bool,
    /// Дверь уже на середине закрытия.
    pub partial: bool,
    pub cancelled: bool,
}

/// Перед отказом (`BeforeDoorDeniedEvent`).
#[derive(Debug, Clone, Copy, Default)]
pub struct BeforeDoorDenied {
    pub cancelled: bool,
}

/// Перед автозакрытием (`BeforeDoorAutoCloseEvent`).
#[derive(Debug, Clone, Copy, Default)]
pub struct BeforeDoorAutoClose {
    pub cancelled: bool,
}

/// Болты подняты или опущены (`DoorBoltsChangedEvent`).
#[derive(Debug, Clone, Copy)]
pub struct DoorBoltsChanged {
    pub bolts_down: bool,
}

/// Можно ли взломать (`BeforePryEvent`).
#[derive(Debug, Clone)]
pub struct BeforePry {
    pub user: Entity,
    pub pry_powered: bool,
    pub force: bool,
    /// Инструментом или существом с `Prying` (не голыми руками).
    pub strong_pry: bool,
    pub message: Option<String>,
    pub cancelled: bool,
}

/// Время взлома (`GetPryTimeModifierEvent`).
#[derive(Debug, Clone, Copy)]
pub struct GetPryTimeModifier {
    pub user: Entity,
    pub pry_time_modifier: f32,
    pub base_time: f64,
}

/// Дверь взломана (`PriedEvent`).
#[derive(Debug, Clone, Copy)]
pub struct Pried {
    pub user: Entity,
}

/// Окончание взлома (`DoorPryDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
struct DoorPryDoAfter;

fn now(world: &World) -> f64 {
    world.resource::<GameTime>().cur_time().as_secs_f64()
}

fn activate(world: &mut World, door: Entity) {
    world.resource_mut::<ActiveDoors>().0.insert(door);
}

fn deactivate(world: &mut World, door: Entity) {
    world.resource_mut::<ActiveDoors>().0.remove(&door);
}

fn door(world: &World, entity: Entity) -> Option<Door> {
    world.get::<Door>(entity).cloned()
}

fn with_door(world: &mut World, entity: Entity, f: impl FnOnce(&mut Door)) {
    if let Some(mut door) = world.get_mut::<Door>(entity) {
        f(&mut door);
    }
}

fn play(world: &mut World, sound: Option<SoundSpecifier>, entity: Entity) {
    if let Some(sound) = sound {
        world.play_entity(&sound, entity, None);
    }
}

/// Заболчена ли дверь (`IsBolted`).
pub fn is_bolted(world: &World, entity: Entity) -> bool {
    world.get::<DoorBolt>(entity).is_some_and(|b| b.bolts_down)
}

/// Задать состояние (`SetState`). `false` — не изменилось.
pub fn set_state(world: &mut World, entity: Entity, state: DoorState) -> bool {
    let Some(current) = door(world, entity) else {
        return false;
    };
    let mut state = state;
    if state == current.state {
        return false;
    }
    let time = now(world);
    match state {
        DoorState::Opening if is_welded(world, entity) => {
            state = DoorState::Closed;
            with_door(world, entity, |d| d.partial = false);
        }
        DoorState::Opening => {
            activate(world, entity);
            with_door(world, entity, |d| {
                d.next_state_change = Some(time + d.open_time_one)
            });
        }
        DoorState::Closing => {
            activate(world, entity);
            with_door(world, entity, |d| {
                d.next_state_change = Some(time + d.close_time_one)
            });
        }
        DoorState::Denying => {
            activate(world, entity);
            with_door(world, entity, |d| {
                d.next_state_change = Some(time + d.deny_duration)
            });
        }
        DoorState::Emagging => {
            activate(world, entity);
            with_door(world, entity, |d| {
                d.next_state_change = Some(time + d.emag_duration)
            });
        }
        DoorState::Open => {
            if is_welded(world, entity) {
                return false;
            }
            with_door(world, entity, |d| d.partial = false);
            if current.next_state_change.is_none() {
                deactivate(world, entity);
            }
        }
        DoorState::Closed => {
            with_door(world, entity, |d| d.partial = false);
        }
    }
    if state == current.state {
        return false;
    }
    with_door(world, entity, |d| d.state = state);
    world.raise(entity, &mut DoorStateChanged { state });
    world.set_appearance(entity, DOOR_STATE, state.name());
    true
}

/// Отказ: мигание и звук (`Deny`).
pub fn deny(world: &mut World, entity: Entity) {
    let Some(current) = door(world, entity) else {
        return;
    };
    if current.state != DoorState::Closed {
        return;
    }
    let mut event = BeforeDoorDenied::default();
    world.raise(entity, &mut event);
    if event.cancelled || !set_state(world, entity, DoorState::Denying) {
        return;
    }
    play(world, current.deny_sound, entity);
}

/// Открыть или закрыть (`TryToggleDoor`).
pub fn try_toggle(world: &mut World, entity: Entity, user: Option<Entity>) -> bool {
    let Some(current) = door(world, entity) else {
        return false;
    };
    match current.state {
        DoorState::Closed | DoorState::Denying => {
            try_open(world, entity, user, current.state == DoorState::Denying)
        }
        DoorState::Open => try_close(world, entity, user),
        _ => false,
    }
}

/// Открыть с проверками (`TryOpen`).
pub fn try_open(world: &mut World, entity: Entity, user: Option<Entity>, quiet: bool) -> bool {
    if !can_open(world, entity, user, quiet) {
        return false;
    }
    start_opening(world, entity);
    true
}

/// Можно ли открыть (`CanOpen`): не на паузе, никто не против, есть доступ, не заварена.
pub fn can_open(world: &mut World, entity: Entity, user: Option<Entity>, quiet: bool) -> bool {
    if world.get::<Door>(entity).is_none() || world.get::<Paused>(entity).is_some() {
        return false;
    }
    let mut event = BeforeDoorOpened {
        user,
        cancelled: false,
    };
    world.raise(entity, &mut event);
    if event.cancelled {
        return false;
    }
    if !has_access(world, entity, user) {
        if !quiet {
            deny(world, entity);
        }
        return false;
    }
    !is_welded(world, entity)
}

/// Начать открытие без проверок (`StartOpening`).
pub fn start_opening(world: &mut World, entity: Entity) {
    let Some(current) = door(world, entity) else {
        return;
    };
    if !set_state(world, entity, DoorState::Opening) {
        return;
    }
    if world
        .get::<Door>(entity)
        .is_some_and(|d| d.state == DoorState::Opening)
    {
        play(world, current.open_sound, entity);
    }
    if current.state == DoorState::Emagging && world.get::<DoorBolt>(entity).is_some() {
        try_set_bolts_down(world, entity, true);
    }
}

/// Середина открытия: дверь проходима (`OnPartialOpen`).
fn on_partial_open(world: &mut World, entity: Entity) {
    set_collidable(world, entity, false);
    let time = now(world);
    with_door(world, entity, |d| {
        d.partial = true;
        d.next_state_change = Some(time + d.close_time_two);
    });
    activate(world, entity);
}

/// Закрыть с проверками (`TryClose`).
pub fn try_close(world: &mut World, entity: Entity, user: Option<Entity>) -> bool {
    if !can_close(world, entity, user, false) {
        return false;
    }
    start_closing(world, entity);
    true
}

/// Можно ли закрыть (`CanClose`): в проёме никого нет.
pub fn can_close(world: &mut World, entity: Entity, user: Option<Entity>, partial: bool) -> bool {
    let Some(current) = door(world, entity) else {
        return false;
    };
    if current.state == DoorState::Closed
        || is_welded(world, entity)
        || world.get::<Paused>(entity).is_some()
    {
        return false;
    }
    let mut event = BeforeDoorClosed {
        perform_collision_check: current.perform_collision_check,
        partial,
        cancelled: false,
    };
    world.raise(entity, &mut event);
    if event.cancelled || !has_access(world, entity, user) {
        return false;
    }
    !event.perform_collision_check
        || colliding(world, entity, current.allow_machine_layer).is_empty()
}

/// Начать закрытие без проверок (`StartClosing`).
pub fn start_closing(world: &mut World, entity: Entity) {
    let Some(current) = door(world, entity) else {
        return;
    };
    if set_state(world, entity, DoorState::Closing) {
        play(world, current.close_sound, entity);
    }
}

/// Середина закрытия: дверь снова твёрдая и давит оставшихся (`OnPartialClose`).
pub(crate) fn on_partial_close(world: &mut World, entity: Entity) -> bool {
    let time = now(world);
    if !can_close(world, entity, None, true) {
        with_door(world, entity, |d| {
            d.next_state_change = Some(time + d.open_time_two);
            d.state = DoorState::Open;
        });
        world.set_appearance(entity, DOOR_STATE, DoorState::Open.name());
        return false;
    }
    with_door(world, entity, |d| {
        d.partial = true;
        d.next_state_change = Some(time + d.close_time_two);
    });
    set_collidable(world, entity, true);
    activate(world, entity);
    crush(world, entity);
    true
}

/// Твёрдость, непрозрачность и непроницаемость двери (`SetCollidable`).
pub fn set_collidable(world: &mut World, entity: Entity, collidable: bool) {
    let Some(current) = door(world, entity) else {
        return;
    };
    if current.change_airtight {
        set_airblocked(world, entity, collidable);
    }
    if world.get::<Physics>(entity).is_some() {
        world.set_can_collide(entity, collidable);
    }
    if !collidable {
        with_door(world, entity, |d| d.currently_crushing.clear());
    }
    if current.occludes
        && let Some(mut occluder) = world.get_mut::<Occluder>(entity)
        && occluder.enabled != collidable
    {
        occluder.enabled = collidable;
    }
}

/// Раздавить стоящих в проёме (`Crush`): урон и отложенное открытие.
fn crush(world: &mut World, entity: Entity) {
    let Some(current) = door(world, entity) else {
        return;
    };
    if !current.can_crush {
        return;
    }
    let victims = colliding(world, entity, current.allow_machine_layer);
    for victim in &victims {
        with_door(world, entity, |d| {
            d.currently_crushing.insert(*victim);
        });
        if let Some(damage) = &current.crush_damage {
            world.change_damage(*victim, damage, false, true, Some(entity));
        }
    }
    if victims.is_empty() {
        return;
    }
    // Открыться снова, когда оглушение пройдёт.
    let time = now(world);
    with_door(world, entity, |d| {
        d.next_state_change = Some(time + d.door_stun_time);
        d.partial = false;
    });
}

/// Слой и маска всех фикстур тела.
fn layer_mask(world: &World, entity: Entity) -> (i32, i32) {
    world.get::<Fixtures>(entity).map_or((0, 0), |fixtures| {
        fixtures.fixtures.values().fold((0, 0), |(layer, mask), f| {
            (layer | f.layer.0, mask | f.mask.0)
        })
    })
}

/// Кто стоит в проёме двери (`GetColliding`).
pub fn colliding(world: &World, entity: Entity, allow_machine_layer: bool) -> Vec<Entity> {
    let Some(transform) = world.get::<Transform>(entity) else {
        return Vec::new();
    };
    let Some(grid) = transform.grid() else {
        return Vec::new();
    };
    if world.get::<MapGrid>(grid).is_none() {
        return Vec::new();
    }
    let cell = TileCoord::containing(transform.local_position(), transform.level());
    let center = cell.center();
    let bounds = Box2::centered(center, Vec2::splat(0.98));
    let (own_layer, own_mask) = layer_mask(world, entity);
    let named = |name: &str| astrocore_physics::group_bits(name).unwrap_or_default();
    let glass = named("GlassLayer");
    let glass_airlock = named("GlassAirlockLayer");
    let table = named("TableLayer");
    let conveyor = named("ConveyorMask");
    let machine = named("MachineLayer");
    world
        .entities_intersecting(grid, transform.level(), bounds)
        .into_iter()
        .filter(|other| *other != entity)
        .filter(|other| {
            let Some(body) = world.get::<Physics>(*other) else {
                return false;
            };
            if !body.can_collide {
                return false;
            }
            if body.body_type == BodyType::Static
                && let Some(other_transform) = world.get::<Transform>(*other)
                && other_transform.anchored()
                && TileCoord::containing(other_transform.local_position(), other_transform.level())
                    != cell
            {
                return false;
            }
            let (layer, mask) = layer_mask(world, *other);
            if layer == glass || layer == glass_airlock || layer == table {
                return false;
            }
            if mask & groups::LOW == 0 {
                return false;
            }
            if layer == conveyor || (allow_machine_layer && layer == machine) {
                return false;
            }
            CollisionGroups(own_mask).intersects(CollisionGroups(layer))
                || CollisionGroups(mask).intersects(CollisionGroups(own_layer))
        })
        .collect()
}

/// Есть ли у действующего доступ (`HasAccess`). Без действующего — есть.
pub fn has_access(world: &World, entity: Entity, user: Option<Entity>) -> bool {
    let Some(user) = user else {
        return true;
    };
    if world
        .get::<Airlock>(entity)
        .is_some_and(|a| a.emergency_access)
    {
        return true;
    }
    if world
        .get::<Door>(entity)
        .is_some_and(|d| d.state == DoorState::Closed)
        && world.get::<Firelock>(entity).is_some()
    {
        return true;
    }
    if world.get::<AccessReader>(entity).is_none() {
        return true;
    }
    is_allowed(world, user, entity)
}

/// Назначить или отменить смену состояния (`SetNextStateChange`).
pub fn set_next_state_change(world: &mut World, entity: Entity, delay: Option<f64>) {
    let Some(current) = door(world, entity) else {
        return;
    };
    if current.state != DoorState::Open && current.state != DoorState::Closed {
        return;
    }
    match delay {
        Some(delay) if delay > 0.0 => {
            let time = now(world);
            with_door(world, entity, |d| d.next_state_change = Some(time + delay));
            activate(world, entity);
        }
        _ => {
            with_door(world, entity, |d| d.next_state_change = None);
            deactivate(world, entity);
        }
    }
}

/// Открыть толчком того, кто стоит вплотную (`CheckDoorBump`).
fn check_door_bump(world: &mut World, entity: Entity) {
    if !world.get::<Door>(entity).is_some_and(|d| d.bump_open) {
        return;
    }
    let Some(transform) = world.get::<Transform>(entity) else {
        return;
    };
    let Some(grid) = transform.grid() else {
        return;
    };
    let cell = TileCoord::containing(transform.local_position(), transform.level());
    let bounds = Box2::centered(cell.center(), Vec2::splat(1.05));
    let candidates: Vec<Entity> = world
        .entities_intersecting(grid, transform.level(), bounds)
        .into_iter()
        .filter(|other| *other != entity && world.has_tag(*other, DOOR_BUMP_TAG))
        .collect();
    for other in candidates {
        if try_open(world, entity, Some(other), true) {
            break;
        }
    }
}

/// Следующая фаза (`NextState`).
fn next_state(world: &mut World, entity: Entity, time: f64) {
    with_door(world, entity, |d| d.next_state_change = None);
    let Some(current) = door(world, entity) else {
        return;
    };
    if !current.currently_crushing.is_empty() && current.state != DoorState::Opening {
        // Давит — открывается без проверок, чтобы никто не застрял в двери.
        start_opening(world, entity);
        return;
    }
    match current.state {
        DoorState::Opening if current.partial => {
            set_state(world, entity, DoorState::Open);
        }
        DoorState::Opening => on_partial_open(world, entity),
        DoorState::Closing if current.partial => {
            set_state(world, entity, DoorState::Closed);
        }
        DoorState::Closing => {
            on_partial_close(world, entity);
        }
        DoorState::Denying => {
            set_state(world, entity, DoorState::Closed);
        }
        DoorState::Emagging => start_opening(world, entity),
        DoorState::Open => {
            if !try_close(world, entity, None) {
                with_door(world, entity, |d| d.next_state_change = Some(time + 1.0));
            }
        }
        DoorState::Closed => {}
    }
}

/// Смена состояний назначенных дверей (`Update`).
fn update(world: &mut World) {
    let time = now(world);
    let doors: Vec<Entity> = world.resource::<ActiveDoors>().0.iter().copied().collect();
    for entity in doors {
        let Some(next) = world.get::<Door>(entity).map(|d| d.next_state_change) else {
            deactivate(world, entity);
            continue;
        };
        let Some(next) = next else {
            deactivate(world, entity);
            continue;
        };
        if world.get::<Paused>(entity).is_some() {
            continue;
        }
        if next < time {
            next_state(world, entity, time);
        }
        if world
            .get::<Door>(entity)
            .is_some_and(|d| d.state == DoorState::Closed)
        {
            deactivate(world, entity);
            check_door_bump(world, entity);
        }
    }
}

// Болты.

fn bolt_lights_visible(bolt: &DoorBolt) -> bool {
    bolt.bolt_lights_enabled && bolt.bolts_down && bolt.powered
}

fn update_bolt_lights(world: &mut World, entity: Entity) {
    if let Some(visible) = world.get::<DoorBolt>(entity).map(bolt_lights_visible) {
        world.set_appearance(entity, BOLT_LIGHTS, visible);
    }
}

/// Огни болтов (`SetBoltLightsEnabled`).
pub fn set_bolt_lights_enabled(world: &mut World, entity: Entity, enabled: bool) {
    let Some(mut bolt) = world.get_mut::<DoorBolt>(entity) else {
        return;
    };
    if bolt.bolt_lights_enabled == enabled {
        return;
    }
    bolt.bolt_lights_enabled = enabled;
    update_bolt_lights(world, entity);
}

/// Опустить или поднять болты без проверок (`SetBoltsDown`).
pub fn set_bolts_down(world: &mut World, entity: Entity, down: bool) {
    let Some(sound) = world.get_mut::<DoorBolt>(entity).map(|mut bolt| {
        bolt.bolts_down = down;
        if down {
            bolt.bolt_down_sound.clone()
        } else {
            bolt.bolt_up_sound.clone()
        }
    }) else {
        return;
    };
    update_bolt_lights(world, entity);
    world.raise(entity, &mut DoorBoltsChanged { bolts_down: down });
    world.play_entity(&sound, entity, None);
}

/// Болты с питанием (`TrySetBoltDown`).
pub fn try_set_bolts_down(world: &mut World, entity: Entity, down: bool) -> bool {
    if !is_powered(world, entity) {
        return false;
    }
    if world
        .get::<DoorBolt>(entity)
        .is_none_or(|b| b.bolts_down == down)
    {
        return false;
    }
    set_bolts_down(world, entity, down);
    true
}

/// Провод болтов перерезан (`SetBoltWireCut`).
pub fn set_bolt_wire_cut(world: &mut World, entity: Entity, cut: bool) {
    if let Some(mut bolt) = world.get_mut::<DoorBolt>(entity) {
        bolt.bolt_wire_cut = cut;
    }
}

// Шлюз.

/// Можно ли шлюзу двигаться: есть питание, болты подняты (`CanChangeState`).
pub fn airlock_can_change_state(world: &World, entity: Entity) -> bool {
    world.get::<Airlock>(entity).is_some_and(|a| a.powered) && !is_bolted(world, entity)
}

/// Назначить автозакрытие (`UpdateAutoClose`).
pub fn update_auto_close(world: &mut World, entity: Entity) {
    let (Some(airlock), Some(current)) =
        (world.get::<Airlock>(entity).cloned(), door(world, entity))
    else {
        return;
    };
    if current.state != DoorState::Open
        || !airlock.auto_close
        || !airlock_can_change_state(world, entity)
    {
        return;
    }
    let mut event = BeforeDoorAutoClose::default();
    world.raise(entity, &mut event);
    if event.cancelled {
        return;
    }
    set_next_state_change(
        world,
        entity,
        Some(airlock.auto_close_delay * f64::from(airlock.auto_close_delay_modifier)),
    );
}

/// Аварийный доступ: пускает всех (`SetEmergencyAccess`).
pub fn set_emergency_access(world: &mut World, entity: Entity, value: bool) {
    let Some(airlock) = world.get::<Airlock>(entity).cloned() else {
        return;
    };
    if !airlock.powered || airlock.emergency_access == value {
        return;
    }
    if let Some(mut component) = world.get_mut::<Airlock>(entity) {
        component.emergency_access = value;
    }
    world.set_appearance(entity, EMERGENCY_LIGHTS, value);
    let sound = if value {
        airlock.emergency_on_sound
    } else {
        airlock.emergency_off_sound
    };
    world.play_entity(&sound, entity, None);
}

/// Безопасность: не закрываться на стоящих (`SetSafety`).
pub fn set_safety(world: &mut World, entity: Entity, value: bool) {
    if let Some(mut airlock) = world.get_mut::<Airlock>(entity) {
        airlock.safety = value;
    }
}

/// Множитель задержки автозакрытия (`SetAutoCloseDelayModifier`).
pub fn set_auto_close_delay_modifier(world: &mut World, entity: Entity, value: f32) {
    if let Some(mut airlock) = world.get_mut::<Airlock>(entity) {
        airlock.auto_close_delay_modifier = value;
    }
}

// Взлом.

/// Можно ли взломать (`CanPry`).
fn can_pry(
    world: &mut World,
    target: Entity,
    user: Entity,
    tool: Option<Entity>,
) -> (bool, Option<String>) {
    let prying = tool
        .and_then(|t| world.get::<Prying>(t).cloned())
        .or_else(|| world.get::<Prying>(user).cloned());
    let mut event = match prying {
        Some(prying) => BeforePry {
            user,
            pry_powered: prying.pry_powered,
            force: prying.force,
            strong_pry: true,
            message: None,
            cancelled: false,
        },
        None => {
            if world.get::<PryUnpowered>(target).is_none() {
                return (false, None);
            }
            BeforePry {
                user,
                pry_powered: false,
                force: false,
                strong_pry: false,
                message: None,
                cancelled: false,
            }
        }
    };
    world.raise(target, &mut event);
    (!event.cancelled, event.message)
}

fn start_pry(
    world: &mut World,
    target: Entity,
    user: Entity,
    tool: Option<Entity>,
    modifier: f32,
) -> bool {
    let mut event = GetPryTimeModifier {
        user,
        pry_time_modifier: 1.0,
        base_time: 5.0,
    };
    world.raise(target, &mut event);
    let delay = event.base_time * f64::from(event.pry_time_modifier) / f64::from(modifier);
    let mut args = DoAfterArgs::new(user, delay, Some(target), tool, Some(target));
    args.break_on_damage = true;
    args.break_on_move = true;
    args.need_hand = tool != Some(user);
    !matches!(
        world.try_start_do_after(args, DoorPryDoAfter),
        DoAfterStart::Refused
    )
}

/// Взломать инструментом (`TryPry` с инструментом). `true` — событие обработано.
pub fn try_pry_with(world: &mut World, target: Entity, user: Entity, tool: Entity) -> bool {
    let Some(prying) = world.get::<Prying>(tool).cloned() else {
        return false;
    };
    if !prying.enabled {
        return false;
    }
    let (allowed, message) = can_pry(world, target, user, Some(tool));
    if !allowed {
        if let Some(message) = message.filter(|m| !m.is_empty()) {
            let text = loc(world, &message, &[]);
            world.popup_entity_for(&text, target, user, PopupType::Small);
        }
        return true;
    }
    start_pry(world, target, user, Some(tool), prying.speed_modifier);
    true
}

/// Взломать руками (`TryPry` без инструмента).
pub fn try_pry_by_hand(world: &mut World, target: Entity, user: Entity) -> bool {
    let Some(unpowered) = world.get::<PryUnpowered>(target).cloned() else {
        return true;
    };
    if !can_pry(world, target, user, None).0 {
        return true;
    }
    let modifier = world
        .get::<Prying>(user)
        .map_or(unpowered.pry_modifier, |p| p.speed_modifier);
    start_pry(world, target, user, None, modifier)
}

fn on_init(world: &mut World, entity: Entity) {
    let Some(current) = door(world, entity) else {
        return;
    };
    let mut state = current.state;
    if current.next_state_change.is_some() {
        activate(world, entity);
    } else if matches!(state, DoorState::Opening | DoorState::Closing) {
        state = if state == DoorState::Opening {
            DoorState::Open
        } else {
            DoorState::Closed
        };
        with_door(world, entity, |d| {
            d.state = state;
            d.partial = false;
        });
    }
    let partial = world.get::<Door>(entity).is_some_and(|d| d.partial);
    let collidable = state == DoorState::Closed
        || (state == DoorState::Closing && partial)
        || (state == DoorState::Opening && !partial);
    set_collidable(world, entity, collidable);
    world.set_appearance(entity, DOOR_STATE, state.name());
}

fn install_door(world: &mut World) {
    world.subscribe::<Door, ComponentInit>(|world, entity, _| on_init(world, entity));
    world.subscribe::<Door, EntityTerminating>(|world, entity, _| deactivate(world, entity));
    world.subscribe::<Door, ActivateInWorld>(|world, entity, event| {
        if event.handled || !event.complex {
            return;
        }
        // Шлюз раньше двери: щелчок отключает автозакрытие (`SharedAirlockSystem.OnActivate`).
        if let Some(mut airlock) = world.get_mut::<Airlock>(entity)
            && airlock.keep_open_if_clicked
            && airlock.auto_close
        {
            airlock.auto_close = false;
        }
        if !world.get::<Door>(entity).is_some_and(|d| d.click_open) {
            return;
        }
        if !try_toggle(world, entity, Some(event.user)) {
            try_pry_by_hand(world, entity, event.user);
        }
        event.handled = true;
    });
    world.subscribe::<Door, StartCollide>(|world, entity, event| {
        let Some(current) = door(world, entity) else {
            return;
        };
        if !current.bump_open || !matches!(current.state, DoorState::Closed | DoorState::Denying) {
            return;
        }
        let other = event.other_entity;
        if world.has_tag(other, DOOR_BUMP_TAG) {
            try_open(
                world,
                entity,
                Some(other),
                current.state == DoorState::Denying,
            );
        }
    });
    world.subscribe::<Door, PreventCollide>(|world, entity, event| {
        if world
            .get::<Door>(entity)
            .is_some_and(|d| d.currently_crushing.contains(&event.other_entity))
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<Door, BeforePry>(|world, entity, event| {
        if !world.get::<Door>(entity).is_some_and(|d| d.can_pry) || is_welded(world, entity) {
            event.cancelled = true;
        }
    });
    world.subscribe::<Door, Pried>(|world, entity, event| {
        match world.get::<Door>(entity).map(|d| d.state) {
            Some(DoorState::Closed) => start_opening(world, entity),
            Some(DoorState::Open) => start_closing(world, entity),
            _ => {}
        }
        let _ = event.user;
    });
    world.subscribe::<Door, WeldableAttempt>(|world, entity, event| {
        let Some(current) = door(world, entity) else {
            return;
        };
        if !current.currently_crushing.is_empty()
            || matches!(
                current.state,
                DoorState::Open | DoorState::Opening | DoorState::Closing
            )
        {
            event.cancelled = true;
        }
    });
    world.subscribe::<Door, GetPryTimeModifier>(|world, entity, event| {
        if let Some(current) = world.get::<Door>(entity) {
            event.base_time = current.pry_time;
        }
    });
    // Взлом (`PryingSystem`).
    world.subscribe::<Door, InteractUsing>(|world, entity, event| {
        if event.handled {
            return;
        }
        event.handled = try_pry_with(world, entity, event.user, event.used);
    });
    world.subscribe::<Door, GetVerbs>(|world, entity, event| {
        if event.kind != VerbKind::Alternative
            || !event.can_interact
            || !event.can_access
            || world.get::<Prying>(event.user).is_none()
        {
            return;
        }
        let user = event.user;
        let text = loc(world, "door-pry", &[]);
        event
            .verbs
            .push(Verb::new(VerbKind::Alternative, text, move |world| {
                try_pry_with(world, entity, user, user);
            }));
    });
    world.subscribe::<Door, DoAfterDone<DoorPryDoAfter>>(|world, entity, event| {
        if event.cancelled || event.target.is_none() {
            return;
        }
        let (allowed, message) = can_pry(world, entity, event.user, event.used);
        if !allowed {
            if let Some(message) = message.filter(|m| !m.is_empty()) {
                let text = loc(world, &message, &[]);
                world.popup_entity_for(&text, entity, event.user, PopupType::Small);
            }
            return;
        }
        if let Some(sound) = event.used.and_then(|tool| {
            world
                .get::<Prying>(tool)
                .map(|p| (tool, p.use_sound.clone()))
        }) {
            world.play_entity(&sound.1, sound.0, None);
        }
        world.raise(entity, &mut Pried { user: event.user });
    });
}

fn install_bolts(world: &mut World) {
    world.subscribe::<DoorBolt, BeforeDoorOpened>(|world, entity, event| {
        if is_bolted(world, entity) {
            event.cancelled = true;
        }
    });
    world.subscribe::<DoorBolt, BeforeDoorClosed>(|world, entity, event| {
        if is_bolted(world, entity) {
            event.cancelled = true;
        }
    });
    world.subscribe::<DoorBolt, BeforeDoorDenied>(|world, entity, event| {
        if is_bolted(world, entity) {
            event.cancelled = true;
        }
    });
    world.subscribe::<DoorBolt, BeforePry>(|world, entity, event| {
        if event.cancelled || !is_bolted(world, entity) || event.force {
            return;
        }
        event.message = Some("airlock-component-cannot-pry-is-bolted-message".into());
        event.cancelled = true;
    });
    world.subscribe::<DoorBolt, DoorStateChanged>(|world, entity, _| {
        update_bolt_lights(world, entity);
    });
    // Серверный `DoorSystem.OnBoltPowerChanged`.
    world.subscribe::<DoorBolt, PowerChanged>(|world, entity, event| {
        let powered = event.powered;
        if powered
            && world
                .get::<DoorBolt>(entity)
                .is_some_and(|b| b.bolt_wire_cut)
        {
            try_set_bolts_down(world, entity, true);
        }
        if let Some(mut bolt) = world.get_mut::<DoorBolt>(entity) {
            bolt.powered = powered;
        }
        update_bolt_lights(world, entity);
    });
    world.subscribe::<DoorBolt, MapInit>(|world, entity, _| {
        let powered = is_powered(world, entity);
        if let Some(mut bolt) = world.get_mut::<DoorBolt>(entity) {
            bolt.powered = powered;
        }
        update_bolt_lights(world, entity);
    });
}

fn install_airlock(world: &mut World) {
    world.subscribe::<Airlock, BeforeDoorClosed>(|world, entity, event| {
        if event.cancelled {
            return;
        }
        if world.get::<Airlock>(entity).is_some_and(|a| !a.safety) {
            event.perform_collision_check = false;
        }
        // Питание и болты важны только в начале закрытия: взломанная ломом дверь
        // закрывается до конца.
        if !event.partial && !airlock_can_change_state(world, entity) {
            event.cancelled = true;
        }
    });
    world.subscribe::<Airlock, DoorStateChanged>(|world, entity, event| {
        // Панель видна только у закрытого шлюза.
        if world.get::<crate::wires::WiresPanel>(entity).is_some() {
            let visible = world
                .get::<Airlock>(entity)
                .is_some_and(|a| a.open_panel_visible)
                || event.state != DoorState::Open;
            crate::wires::change_panel_visibility(world, entity, visible);
        }
        update_auto_close(world, entity);
        if event.state == DoorState::Closed
            && let Some(mut airlock) = world.get_mut::<Airlock>(entity)
        {
            airlock.auto_close = true;
        }
    });
    world.subscribe::<Airlock, DoorBoltsChanged>(|world, entity, event| {
        if !event.bolts_down {
            update_auto_close(world, entity);
        }
    });
    world.subscribe::<Airlock, BeforeDoorOpened>(|world, entity, event| {
        if !airlock_can_change_state(world, entity) {
            event.cancelled = true;
        }
    });
    world.subscribe::<Airlock, BeforeDoorDenied>(|world, entity, event| {
        if !airlock_can_change_state(world, entity) {
            event.cancelled = true;
        }
    });
    world.subscribe::<Airlock, GetPryTimeModifier>(|world, entity, event| {
        let Some(airlock) = world.get::<Airlock>(entity).cloned() else {
            return;
        };
        if airlock.powered {
            event.pry_time_modifier *= airlock.powered_pry_modifier;
        }
        if is_bolted(world, entity) {
            event.pry_time_modifier *= airlock.bolted_pry_modifier;
        }
    });
    world.subscribe::<Airlock, BeforePry>(|world, entity, event| {
        if event.cancelled || event.pry_powered {
            return;
        }
        let Some(airlock) = world.get::<Airlock>(entity) else {
            return;
        };
        if !airlock.powered {
            return;
        }
        event.message = Some(airlock.pry_failed_popup.clone());
        event.cancelled = true;
    });
    world.subscribe::<Airlock, PowerChanged>(|world, entity, event| {
        let powered = event.powered;
        if let Some(mut airlock) = world.get_mut::<Airlock>(entity) {
            airlock.powered = powered;
        }
        if world.get::<Door>(entity).is_none() {
            return;
        }
        if !powered {
            if world
                .get::<Door>(entity)
                .is_some_and(|d| d.state == DoorState::Open)
            {
                set_next_state_change(world, entity, None);
            }
        } else {
            update_auto_close(world, entity);
        }
    });
    world.subscribe::<Airlock, MapInit>(|world, entity, _| {
        let powered = is_powered(world, entity);
        if let Some(mut airlock) = world.get_mut::<Airlock>(entity) {
            airlock.powered = powered;
        }
        let emergency = world
            .get::<Airlock>(entity)
            .is_some_and(|a| a.emergency_access);
        world.set_appearance(entity, EMERGENCY_LIGHTS, emergency);
    });
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<Door>();
    world.register_data_component::<DoorBolt>();
    world.register_data_component::<Airlock>();
    world.register_data_component::<Prying>();
    world.register_data_component::<PryUnpowered>();
    world.register_data_component::<Firelock>();
    if !side.is_server() {
        return;
    }
    world.init_resource::<ActiveDoors>();
    install_door(world);
    install_bolts(world);
    install_airlock(world);
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
