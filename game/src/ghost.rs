//! Призраки на сервере: появление призрака для разума, попытка стать призраком (с правом
//! вернуться в тело, если тело мертво; в критическом состоянии тело добивается), команда
//! `ghost`, возвращение в тело, телепорт к точкам и живым игрокам, действия «Бу!» и слух,
//! мёртвое тело при попытке идти отпускает призрака, призрак не трогает мир и не лезет в
//! шкафы, осмотр — сколько прошло со смерти; удаление призрака, когда игрок ушёл или
//! вернулся в тело; призраки видны только призракам и видят друг друга, а в конце раунда
//! их видят все. Образец — `GhostSystem` (с `Boo`), `SharedGhostSystem`, `GhostCommand`,
//! `GhostComponent`, `GhostHearingComponent`, `GhostOnMoveComponent`,
//! `SpookySpeakerComponent`, `SpookyExtinguishableComponent`, `WarpPointComponent`,
//! `CCVars.GhostKillCrit`, название точки по маяку навигационной карты
//! (`ServerNavMapSystem.OnNavMapBeaconMapInit`, `OnConfigurableMapInit`) SS14 @ 2f4a2aad.
//!
//! Нет пока: следования за целью (`FollowerSystem`, «призрачный торнадо»), мигания ламп от
//! «Бу!» (`SpookyPoweredLight`), спрайта призрака по урону, показа призраков по кнопке
//! администратора.

use std::time::Duration;

use astrocore_console::{
    CVarDef, CVarFlags, ConfigWorldExt, ConsoleCommand, ConsoleShell, ConsoleWorldExt,
};
use astrocore_ecs::{
    ComponentStartup, EventWorldExt, GameTime, GameWorldExt, LifecycleWorldExt, MapInit, MetaData,
    RegistryWorldExt, Update,
};
use astrocore_loc::{EntityNameWorldExt, LocArg};
use astrocore_map::{EntityCoordinates, SpawnWorldExt, TransformWorldExt};
use astrocore_movement::InputMover;
use astrocore_net::{
    Eye, NetEntities, NetEntity, NetEvent, NetEventWorldExt, NetRegistryWorldExt, NetTarget,
    NetUserId, PlayerDetached, Session, VisibilityWorldExt,
};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::action_blocker::{DropAttempt, InteractionAttempt, PickupAttempt, UseAttempt};
use crate::actions::{ActionEvent, ActionsWorldExt};
use crate::admin_logs::{LogImpact, LogType, add_admin_log};
use crate::damage::{DamageSpecifier, DamageableWorldExt};
use crate::examine::{Examined, loc};
use crate::mind::{self, Mind, MindRemoved, MindUnvisited};
use crate::mobs::{MobState, MobStateWorldExt};
use crate::popups::{PopupType, PopupWorldExt};

/// Слои видимости SS14 (`VisibilityFlags`).
pub const VISIBILITY_NORMAL: u32 = 1 << 0;
pub const VISIBILITY_GHOST: u32 = 1 << 1;

pub const GHOST_KILL_CRIT: CVarDef<bool> = CVarDef::new(
    "ghost.kill_crit",
    true,
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Призрак из критического состояния добивает тело и может вернуться в него.",
);

/// Призрак (`GhostComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Ghost {
    /// Может взаимодействовать с миром (`CanGhostInteract`, в данных `canInteract`).
    #[serde(rename = "canInteract")]
    pub can_ghost_interact: bool,
    pub can_return_to_body: bool,
    /// Время смерти по часам сервера.
    pub time_of_death: Duration,
    pub boo_radius: f32,
    pub boo_intensity: i32,
    pub boo_action: String,
    pub toggle_ghost_hearing_action: String,
    pub toggle_lighting_action: String,
    #[serde(rename = "toggleFoVAction")]
    pub toggle_fov_action: String,
    pub toggle_ghosts_action: String,
}

impl Default for Ghost {
    fn default() -> Self {
        Self {
            can_ghost_interact: false,
            can_return_to_body: false,
            time_of_death: Duration::ZERO,
            boo_radius: 3.0,
            boo_intensity: 6,
            boo_action: "ActionGhostBoo".into(),
            toggle_ghost_hearing_action: "ActionToggleGhostHearing".into(),
            toggle_lighting_action: "ActionToggleLighting".into(),
            toggle_fov_action: "ActionToggleFov".into(),
            toggle_ghosts_action: "ActionToggleGhosts".into(),
        }
    }
}

/// Слышит всю речь на карте (`GhostHearingComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GhostHearing {}

/// Мёртвое (или любое) тело при попытке идти отпускает призрака (`GhostOnMoveComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GhostOnMove {
    pub can_return: bool,
    pub must_be_dead: bool,
}

impl Default for GhostOnMove {
    fn default() -> Self {
        Self {
            can_return: true,
            must_be_dead: false,
        }
    }
}

/// Точка телепорта призраков (`WarpPointComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WarpPoint {
    /// Ключ перевода названия; без него — имя сущности.
    pub location: Option<String>,
    pub follow: bool,
}

/// Маяк навигационной карты (`NavMapBeaconComponent`): пока только текст — им называется
/// точка телепорта. Сама карта станции — фаза 7.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NavMapBeacon {
    pub text: Option<String>,
    pub default_text: Option<String>,
    pub color: String,
    pub enabled: bool,
}

impl Default for NavMapBeacon {
    fn default() -> Self {
        Self {
            text: None,
            default_text: None,
            color: "orange".into(),
            enabled: true,
        }
    }
}

/// Маяк, текст которого настраивают (`ConfigurableNavMapBeaconComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigurableNavMapBeacon {}

/// Ближе этого к маяку — «у маяка» (`CloseDistance`).
const BEACON_CLOSE_DISTANCE: f32 = 15.0;
/// Дальше этого — «далеко» (`FarDistance`).
const BEACON_FAR_DISTANCE: f32 = 30.0;

/// Где точка относительно ближайшего включённого настраиваемого маяка
/// (`NavMapSystem.GetNearestBeaconString`): «у мостика», «к северу от бара».
pub fn nearest_beacon_string(
    world: &mut World,
    coordinates: astrocore_map::MapCoordinates,
) -> String {
    use astrocore_map::{MapWorldExt, Transform};
    let beacons: Vec<(String, String, astrocore_math::Vec2)> = world
        .query_filtered::<(Entity, &NavMapBeacon), bevy_ecs::query::With<ConfigurableNavMapBeacon>>(
        )
        .iter(world)
        .filter(|(_, beacon)| beacon.enabled)
        .filter_map(|(entity, beacon)| {
            let text = beacon.text.clone()?;
            let position = world.map_coordinates(entity)?;
            (position.map == coordinates.map)
                .then(|| (text, beacon.color.clone(), position.position))
        })
        .collect();
    let nearest = beacons.into_iter().min_by(|a, b| {
        a.2.distance_squared(coordinates.position)
            .total_cmp(&b.2.distance_squared(coordinates.position))
    });
    let Some((text, color, position)) = nearest else {
        return loc(world, "nav-beacon-pos-no-beacons", &[]);
    };
    // Направление — с поправкой на поворот грида под маяком.
    let grid_rotation = world
        .find_grid_at(astrocore_map::MapCoordinates::new(
            coordinates.map,
            position,
        ))
        .and_then(|grid| world.get::<Transform>(grid))
        .map_or(astrocore_math::Angle::ZERO, Transform::local_rotation);
    let offset = coordinates.position - position;
    let direction = (astrocore_math::Angle::from_world_vec(offset) - grid_rotation).direction();
    let length = offset.length();
    if length < BEACON_CLOSE_DISTANCE {
        return loc(
            world,
            "nav-beacon-pos-format",
            &[
                ("color", LocArg::from(color)),
                ("marker", LocArg::from(text)),
            ],
        );
    }
    let modifier = if length > BEACON_FAR_DISTANCE {
        loc(world, "nav-beacon-pos-format-direction-mod-far", &[])
    } else {
        String::new()
    };
    let direction = crate::examine::format_direction(world, direction).to_lowercase();
    loc(
        world,
        "nav-beacon-pos-format-direction",
        &[
            ("modifier", LocArg::from(modifier)),
            ("direction", LocArg::from(direction)),
            ("color", LocArg::from(color)),
            ("marker", LocArg::from(text)),
        ],
    )
}

/// Сила «Бу!» (`GhostBooIntensity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum GhostBooIntensity {
    None = 0,
    Subtle = 1,
    #[default]
    Normal = 2,
    Extreme = 6,
}

impl GhostBooIntensity {
    /// По оставшейся силе (`GetIntensity`).
    pub fn from_remaining(value: i32) -> Self {
        if value >= Self::Extreme as i32 {
            Self::Extreme
        } else if value >= Self::Normal as i32 {
            Self::Normal
        } else if value >= Self::Subtle as i32 {
            Self::Subtle
        } else {
            Self::None
        }
    }
}

/// Призрак пугает сущность (`GhostBooEvent`).
#[derive(Debug, Clone, Copy)]
pub struct GhostBoo {
    pub allowed: GhostBooIntensity,
    pub response: GhostBooIntensity,
    pub handled: bool,
}

/// Говорит жуткие фразы от «Бу!» (`SpookySpeakerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SpookySpeaker {
    pub message_set: String,
    pub speak_chance: f32,
    /// Секунды.
    pub cooldown: f64,
    #[serde(skip)]
    pub next_speak_time: Duration,
    pub intensity: GhostBooIntensity,
}

impl Default for SpookySpeaker {
    fn default() -> Self {
        Self {
            message_set: String::new(),
            speak_chance: 0.5,
            cooldown: 30.0,
            next_speak_time: Duration::ZERO,
            intensity: GhostBooIntensity::Normal,
        }
    }
}

/// Гаснет от «Бу!» (`SpookyExtinguishableComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SpookyExtinguishable {
    pub extinguish_chance: f32,
    pub intensity: GhostBooIntensity,
    pub extinguish_sound: Option<astrocore_audio::SoundSpecifier>,
}

impl Default for SpookyExtinguishable {
    fn default() -> Self {
        Self {
            extinguish_chance: 0.8,
            intensity: GhostBooIntensity::Subtle,
            extinguish_sound: Some(astrocore_audio::SoundSpecifier::path(
                "/Audio/Effects/quick_exhale.ogg",
            )),
        }
    }
}

/// Куда можно телепортироваться (`GhostWarp`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GhostWarp {
    pub entity: NetEntity,
    pub display_name: String,
    pub is_warp_point: bool,
}

/// Сообщения призрака клиент ↔ сервер (`GhostWarpsRequestEvent`, `GhostWarpsResponseEvent`,
/// `GhostWarpToTargetRequestEvent`, `WarpToRandomRequestEvent`, `GhostReturnToBodyRequest`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GhostMessage {
    WarpsRequest,
    Warps(Vec<GhostWarp>),
    WarpToTarget(NetEntity),
    WarpToRandom,
    ReturnToBody,
    /// Переключить поле зрения (`RequestToggleFovEvent` `ContentEyeSystem`).
    ToggleFov,
}

/// Освещение призрака на клиенте (`OnToggleLighting`): обычное, ночное зрение
/// (`NightVision`), полный свет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GhostLighting {
    #[default]
    Normal,
    HalfBright,
    Fullbright,
}

/// Кого из призраков видно (`GhostVisibilityMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GhostVisibilityMode {
    #[default]
    ShowAllGhosts,
    HideOtherGhosts,
    HideOtherGhostsAndSelf,
}

/// Состояние призрака на клиенте. Ресурс мира клиента.
#[derive(bevy_ecs::resource::Resource, Debug, Clone, Default)]
pub struct GhostView {
    pub lighting: GhostLighting,
    pub visibility: GhostVisibilityMode,
    pub warps: Vec<GhostWarp>,
    pub warp_window: bool,
    pub version: u64,
}

/// Клиентские действия призрака (`OnToggleLighting`, `OnToggleFoV`, `OnToggleGhosts`).
pub fn client_action(world: &mut World, performer: Entity, kind: &str) {
    if world.get::<Ghost>(performer).is_none() {
        return;
    }
    let key = {
        let mut view = world.get_resource_or_insert_with(GhostView::default);
        let key = match kind {
            "ToggleLightingActionEvent" => {
                let (next, key) = match view.lighting {
                    GhostLighting::Normal => (
                        GhostLighting::HalfBright,
                        "ghost-gui-toggle-lighting-manager-popup-half-bright",
                    ),
                    GhostLighting::HalfBright => (
                        GhostLighting::Fullbright,
                        "ghost-gui-toggle-lighting-manager-popup-fullbright",
                    ),
                    GhostLighting::Fullbright => (
                        GhostLighting::Normal,
                        "ghost-gui-toggle-lighting-manager-popup-normal",
                    ),
                };
                view.lighting = next;
                key
            }
            "ToggleGhostsActionEvent" => {
                let (next, key) = match view.visibility {
                    GhostVisibilityMode::ShowAllGhosts => (
                        GhostVisibilityMode::HideOtherGhosts,
                        "ghost-gui-toggle-ghost-visibility-popup-off",
                    ),
                    GhostVisibilityMode::HideOtherGhosts => (
                        GhostVisibilityMode::HideOtherGhostsAndSelf,
                        "ghost-gui-toggle-all-ghosts-visibility-popup-off",
                    ),
                    GhostVisibilityMode::HideOtherGhostsAndSelf => (
                        GhostVisibilityMode::ShowAllGhosts,
                        "ghost-gui-toggle-ghost-visibility-popup-on",
                    ),
                };
                view.visibility = next;
                key
            }
            "ToggleFoVActionEvent" => "ghost-gui-toggle-fov-popup",
            _ => return,
        };
        view.version += 1;
        key
    };
    if kind == "ToggleFoVActionEvent" {
        world.send_net_event_to_server(&GhostMessage::ToggleFov);
    }
    let text = loc(world, key, &[]);
    world
        .get_resource_or_insert_with(crate::popups::PopupQueue::default)
        .0
        .push(crate::popups::Popup {
            message: text,
            kind: PopupType::Small,
            place: crate::popups::PopupPlace::Entity(performer),
        });
}

/// Команды призрака, доступные всем игрокам (`[AnyCommand]`).
pub const GHOST_COMMANDS: [&str; 1] = ["ghost"];

/// Можно ли стать призраком (`GhostAttemptEvent`): на теле, отменяемое.
#[derive(Debug, Clone, Copy)]
pub struct GhostAttempt {
    pub mind: Entity,
    pub cancelled: bool,
}

/// Попытку обработал кто-то другой (`GhostAttemptHandleEvent`). Широковещательно.
#[derive(Debug, Clone, Copy)]
pub struct GhostAttemptHandle {
    pub mind: Entity,
    pub can_return_global: bool,
    pub handled: bool,
    pub result: bool,
}

/// Может ли призрак вернуться в тело.
pub fn can_return_to_body(world: &World, entity: Entity) -> bool {
    world
        .get::<Ghost>(entity)
        .is_some_and(|g| g.can_return_to_body)
}

/// `SetCanReturnToBody`.
pub fn set_can_return_to_body(world: &mut World, entity: Entity, value: bool) {
    if let Some(mut ghost) = world.get_mut::<Ghost>(entity) {
        ghost.can_return_to_body = value;
    }
}

/// `SetTimeOfDeath`.
pub fn set_time_of_death(world: &mut World, entity: Entity, value: Duration) {
    if let Some(mut ghost) = world.get_mut::<Ghost>(entity) {
        ghost.time_of_death = value;
    }
}

fn valid_position(world: &World, at: &EntityCoordinates) -> bool {
    world.is_alive(at.parent)
        && world
            .get::<MetaData>(at.parent)
            .is_some_and(|m| m.stage < astrocore_ecs::LifeStage::Terminating)
}

/// Призрак для разума (`SpawnGhost`): на месте `at` или у точки наблюдателей. С `can_return`
/// разум посещает призрака, иначе переносится в него.
pub fn spawn_ghost(
    world: &mut World,
    mind_id: Entity,
    at: Option<EntityCoordinates>,
    can_return: bool,
) -> Option<Entity> {
    let state = world.get::<Mind>(mind_id)?.clone();
    let at = match at.filter(|at| valid_position(world, at)) {
        Some(at) => at,
        None => match crate::game_ticker::observer_spawn_point(world)
            .filter(|at| valid_position(world, at))
        {
            Some(at) => at,
            None => {
                eprintln!("нет места для призрака {:?}", state.character_name);
                mind::transfer_to(world, mind_id, None, false, false);
                return None;
            }
        },
    };
    let ghost = world
        .spawn_attached(crate::game_ticker::OBSERVER_PROTOTYPE, at)
        .ok()?;
    // Имя призрака — имя персонажа, иначе имя игрока.
    let name = state
        .character_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| {
            let user = state.user_id?;
            let session = mind::session_of_user(world, user)?;
            world.get::<Session>(session).map(|s| s.name.clone())
        });
    if let Some(name) = name
        && let Some(mut meta) = world.get_mut::<MetaData>(ghost)
    {
        meta.name = Some(name);
    }
    if let Some(time) = state.time_of_death {
        set_time_of_death(world, ghost, time);
    }
    set_can_return_to_body(world, ghost, can_return);
    if can_return {
        mind::visit(world, mind_id, ghost);
    } else {
        mind::transfer_to(world, mind_id, Some(ghost), false, true);
    }
    Some(ghost)
}

/// Стать призраком (`OnGhostAttempt`). `can_return_global` — вернуться можно, если тело
/// мертво; `forced` — без проверок тела и запрета.
pub fn on_ghost_attempt(
    world: &mut World,
    mind_id: Entity,
    can_return_global: bool,
    via_command: bool,
    forced: bool,
) -> bool {
    let Some(state) = world.get::<Mind>(mind_id).cloned() else {
        return false;
    };
    let player_entity = state.current_entity();
    let users: Vec<NetUserId> = state.user_id.into_iter().collect();
    if let Some(entity) = player_entity
        && via_command
    {
        let name = world.entity_name(entity);
        let text = if forced {
            format!("{name} was forced to ghost via command")
        } else {
            format!("{name} is attempting to ghost via command")
        };
        add_admin_log(world, LogType::Mind, LogImpact::Medium, &text, &[], &users);
    }
    if let Some(entity) = player_entity
        && !forced
    {
        let mut attempt = GhostAttempt {
            mind: mind_id,
            cancelled: false,
        };
        world.raise(entity, &mut attempt);
        if attempt.cancelled {
            return false;
        }
    }
    let mut handle = GhostAttemptHandle {
        mind: mind_id,
        can_return_global,
        handled: false,
        result: false,
    };
    world.raise_broadcast(&mut handle);
    if handle.handled {
        return handle.result;
    }
    if state.prevent_ghosting && !forced {
        if let Some(user) = state.user_id
            && let Some(session) = mind::session_of_user(world, user)
        {
            let text = crate::examine::loc(world, "comp-mind-ghosting-prevented", &[]);
            crate::chat::dispatch_server_message(world, session, &text);
        }
        return false;
    }
    if player_entity.is_some_and(|e| world.get::<Ghost>(e).is_some_and(|g| !g.can_ghost_interact)) {
        return false;
    }
    if state.visiting_entity.is_some() {
        mind::unvisit(world, mind_id);
    }
    let position = match player_entity.filter(|e| world.is_alive(*e)) {
        Some(entity) => mind::grid_or_map_coordinates(world, entity),
        None => crate::game_ticker::observer_spawn_point(world),
    };
    let Some(position) = position else {
        return false;
    };
    // Вернуться можно, если тело мертво; из крита — тело добивается (`GhostKillCrit`).
    let mut can_return = can_return_global && mind::is_character_dead_physically(world, mind_id);
    if world.cvar(&GHOST_KILL_CRIT)
        && can_return_global
        && let Some(entity) = player_entity
        && world.is_mob_critical(entity)
    {
        can_return = true;
        let dead = world.threshold_for_state(entity, MobState::Dead);
        let dealt = match dead {
            Some(threshold) => threshold - world.total_damage(entity),
            None => crate::fixed_point::FixedPoint2::from_f32(200.0),
        };
        let damage = DamageSpecifier::new([("Asphyxiation", dealt)]);
        world.change_damage(entity, &damage, true, true, None);
    }
    if let Some(entity) = player_entity {
        let name = world.entity_name(entity);
        let suffix = if can_return { "" } else { " (non-returnable)" };
        add_admin_log(
            world,
            LogType::Mind,
            LogImpact::Medium,
            &format!("{name} ghosted{suffix}"),
            &[],
            &users,
        );
    }
    spawn_ghost(world, mind_id, Some(position), can_return).is_some()
}

fn delete_ghost(world: &mut World, entity: Entity) {
    if world.is_alive(entity) {
        world.queue_delete(entity);
    }
}

// Телепорт.

/// Сущность призрака сессии (`CanGhostWarp`).
fn session_ghost(world: &World, session: Entity) -> Option<Entity> {
    let entity = world.get::<Session>(session)?.attached?;
    world.get::<Ghost>(entity).map(|_| entity)
}

/// Точки телепорта (`GetLocationWarps`).
pub fn location_warps(world: &mut World) -> Vec<GhostWarp> {
    let points: Vec<(Entity, WarpPoint)> = world
        .query::<(Entity, &WarpPoint)>()
        .iter(world)
        .map(|(e, w)| (e, w.clone()))
        .collect();
    points
        .into_iter()
        .filter_map(|(entity, point)| {
            let net = *world.get::<NetEntity>(entity)?;
            let display_name = match &point.location {
                Some(key) => loc(world, key, &[]),
                None => world.entity_name(entity),
            };
            Some(GhostWarp {
                entity: net,
                display_name,
                is_warp_point: true,
            })
        })
        .collect()
}

/// Живые игроки (`GetPlayerWarps`): «имя (должность)».
pub fn player_warps(world: &mut World, except: Option<Entity>) -> Vec<GhostWarp> {
    let players: Vec<(Entity, NetUserId)> = world
        .query::<&Session>()
        .iter(world)
        .filter_map(|s| Some((s.attached?, s.user_id)))
        .collect();
    let unknown = loc(world, "generic-unknown-title", &[]);
    let mut warps = Vec::new();
    for (entity, user) in players {
        if Some(entity) == except || !(world.is_mob_alive(entity) || world.is_mob_critical(entity))
        {
            continue;
        }
        let Some(net) = world.get::<NetEntity>(entity).copied() else {
            continue;
        };
        let job = crate::game_ticker::job_title(world, user).unwrap_or_else(|| unknown.clone());
        warps.push(GhostWarp {
            entity: net,
            display_name: format!("{} ({job})", world.entity_name(entity)),
            is_warp_point: false,
        });
    }
    warps
}

/// Переместить призрака к цели (`WarpTo`). Следования пока нет: призрак встаёт рядом.
pub fn warp_to(world: &mut World, ghost: Entity, target: Entity) {
    let from = world.entity_name(ghost);
    let to = world.entity_name(target);
    add_admin_log(
        world,
        LogType::GhostWarp,
        LogImpact::Low,
        &format!("{from} ghost warped to {to}"),
        &[],
        &[],
    );
    let Some(at) = mind::grid_or_map_coordinates(world, target) else {
        return;
    };
    if world.set_coordinates(ghost, at, None).is_ok() {
        astrocore_physics::PhysicsWorldExt::set_linear_velocity(
            world,
            ghost,
            astrocore_math::Vec2::ZERO,
        );
    }
}

fn receive(world: &mut World, event: &mut NetEvent<GhostMessage>) {
    let Some(session) = event.session else {
        return;
    };
    let Some(ghost) = session_ghost(world, session) else {
        return;
    };
    match event.event.clone() {
        GhostMessage::WarpsRequest => {
            let mut warps = player_warps(world, Some(ghost));
            warps.extend(location_warps(world));
            world.send_net_event(NetTarget::Session(session), &GhostMessage::Warps(warps));
        }
        GhostMessage::WarpToTarget(target) => {
            let target = world
                .get_resource::<NetEntities>()
                .and_then(|n| n.entity(target));
            if let Some(target) = target.filter(|t| world.is_alive(*t)) {
                warp_to(world, ghost, target);
            }
        }
        GhostMessage::WarpToRandom => {
            let warps = player_warps(world, Some(ghost));
            if warps.is_empty() {
                return;
            }
            let index = crate::random::random(world).range(0, warps.len() as i32) as usize;
            let target = world
                .get_resource::<NetEntities>()
                .and_then(|n| n.entity(warps[index].entity));
            if let Some(target) = target {
                warp_to(world, ghost, target);
            }
        }
        GhostMessage::ReturnToBody => {
            if !can_return_to_body(world, ghost) {
                return;
            }
            if let Some(mind_id) = mind::mind_of(world, ghost) {
                mind::unvisit(world, mind_id);
            }
        }
        GhostMessage::ToggleFov => {
            if let Some(mut eye) = world.get_mut::<Eye>(ghost) {
                eye.draw_fov = !eye.draw_fov;
            }
        }
        GhostMessage::Warps(_) => {}
    }
}

// Действия.

/// «Бу!» (`OnGhostBooAction`): пугает сущности рядом, пока хватает силы.
pub fn boo(world: &mut World, ghost: Entity) -> bool {
    let Some(state) = world.get::<Ghost>(ghost).cloned() else {
        return false;
    };
    let mut entities = crate::singularity::entities_in_range(world, ghost, state.boo_radius);
    // Цели в случайном порядке, чтобы не пугать одни и те же (`Shuffle`).
    for i in (1..entities.len()).rev() {
        let j = crate::random::random(world).range(0, i as i32 + 1) as usize;
        entities.swap(i, j);
    }
    let mut remaining = state.boo_intensity;
    let mut affected = false;
    for target in entities {
        let allowed = GhostBooIntensity::from_remaining(remaining);
        if allowed == GhostBooIntensity::None {
            break;
        }
        let mut event = GhostBoo {
            allowed,
            response: GhostBooIntensity::None,
            handled: false,
        };
        world.raise(target, &mut event);
        if !event.handled {
            continue;
        }
        affected = true;
        remaining -= event.response as i32;
        if remaining <= 0 {
            break;
        }
    }
    if !affected {
        let text = loc(world, "ghost-component-boo-action-failed", &[]);
        world.popup_entity_for(&text, ghost, ghost, PopupType::Small);
    }
    true
}

fn on_speaker_boo(world: &mut World, entity: Entity, event: &mut GhostBoo) {
    let Some(speaker) = world.get::<SpookySpeaker>(entity).cloned() else {
        return;
    };
    if event.handled || event.allowed < speaker.intensity {
        return;
    }
    if !crate::random::random(world).prob(speaker.speak_chance) {
        return;
    }
    let now = world.resource::<GameTime>().cur_time();
    if now < speaker.next_speak_time {
        return;
    }
    let message = crate::profiles::random_dataset_value(world, &speaker.message_set);
    if message.is_empty() {
        return;
    }
    // Фразы вида «...Оооо» иначе приняли бы за радио: префикс «рядом».
    let message = format!("{}{message}", crate::chat::LOCAL_PREFIX);
    crate::chat::try_send_in_game_ic_message(
        world,
        entity,
        &message,
        crate::chat::InGameIcChatType::Speak,
        crate::chat::ChatTransmitRange::HideChat,
        false,
        None,
        None,
        true,
        false,
    );
    if let Some(mut speaker) = world.get_mut::<SpookySpeaker>(entity) {
        speaker.next_speak_time = now + Duration::from_secs_f64(speaker.cooldown);
    }
    event.handled = true;
    event.response = speaker.intensity;
}

fn on_extinguish_boo(world: &mut World, entity: Entity, event: &mut GhostBoo) {
    let Some(spooky) = world.get::<SpookyExtinguishable>(entity).cloned() else {
        return;
    };
    if event.handled || event.allowed < spooky.intensity {
        return;
    }
    if !crate::random::random(world).prob(spooky.extinguish_chance) {
        return;
    }
    if !crate::flammable::try_extinguish(world, entity) {
        return;
    }
    if let Some(sound) = &spooky.extinguish_sound {
        astrocore_audio::AudioWorldExt::play_entity(world, sound, entity, None);
    }
    event.handled = true;
    event.response = spooky.intensity;
}

/// Слух призрака (`OnGhostHearingAction`): вся речь карты или только рядом.
fn toggle_hearing(world: &mut World, ghost: Entity, action: Entity) {
    let hearing = world.get::<GhostHearing>(ghost).is_some();
    if hearing {
        world.entity_mut(ghost).remove::<GhostHearing>();
    } else {
        world.entity_mut(ghost).insert(GhostHearing::default());
    }
    world.set_toggled(action, hearing);
    let key = if hearing {
        "ghost-gui-toggle-hearing-popup-off"
    } else {
        "ghost-gui-toggle-hearing-popup-on"
    };
    let text = loc(world, key, &[]);
    world.popup_entity_for(&text, ghost, ghost, PopupType::Small);
}

// Команда.

fn ghost_command(world: &mut World, shell: &mut ConsoleShell, _args: &[String]) {
    let Some(session) = shell.player else {
        shell.write_line(loc(world, "ghost-command-no-session", &[]));
        return;
    };
    let Some(user) = world.get::<Session>(session).map(|s| s.user_id) else {
        return;
    };
    if !crate::game_ticker::user_has_joined_game(world, user) {
        shell.write_line(loc(world, "ghost-command-error-lobby", &[]));
        return;
    }
    let mind_id = match mind::mind_of_user(world, user) {
        Some(mind_id) => mind_id,
        None => mind::create_mind(world, Some(user), None),
    };
    if !on_ghost_attempt(world, mind_id, true, true, false) {
        shell.write_line(loc(world, "ghost-command-denied", &[]));
    }
}

/// Мёртвые тела, которые игрок пытается сдвинуть (`OnRelayMoveInput`).
fn ghost_on_move(world: &mut World) {
    const DIRECTIONS: u8 = 1 | 2 | 4 | 8;
    let moving: Vec<(Entity, GhostOnMove)> = world
        .query::<(Entity, &GhostOnMove, &InputMover)>()
        .iter(world)
        .filter(|(_, _, mover)| mover.held.0 & DIRECTIONS != 0)
        .map(|(e, g, _)| (e, g.clone()))
        .collect();
    for (entity, component) in moving {
        if world.get::<mind::VisitingMind>(entity).is_some() {
            continue;
        }
        let Some(mind_id) = mind::mind_of(world, entity) else {
            continue;
        };
        if world
            .get::<Mind>(mind_id)
            .is_none_or(|m| m.visiting_entity.is_some())
        {
            continue;
        }
        if component.must_be_dead && (world.is_mob_alive(entity) || world.is_mob_critical(entity)) {
            continue;
        }
        on_ghost_attempt(world, mind_id, component.can_return, false, false);
    }
}

/// Призраков видят все (`MakeVisible`): в конце раунда, у кого есть метка.
pub fn make_visible(world: &mut World, visible: bool) {
    let ghosts: Vec<Entity> = world
        .query_filtered::<Entity, bevy_ecs::query::With<Ghost>>()
        .iter(world)
        .collect();
    for ghost in ghosts {
        if !crate::tag::TagWorldExt::has_tag(world, ghost, "AllowGhostShownByEvent") {
            continue;
        }
        let layer = if visible {
            VISIBILITY_NORMAL
        } else {
            VISIBILITY_GHOST
        };
        world.set_visibility_layer(ghost, layer);
    }
}

fn cancel_if_cannot_interact(world: &World, ghost: Entity, cancelled: &mut bool) {
    if world
        .get::<Ghost>(ghost)
        .is_some_and(|g| !g.can_ghost_interact)
    {
        *cancelled = true;
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_cvar(&GHOST_KILL_CRIT);
    world.register_data_component::<Ghost>();
    world.register_net_serde::<Ghost>();
    world.register_data_component::<GhostHearing>();
    world.register_data_component::<GhostOnMove>();
    world.register_data_component::<WarpPoint>();
    world.register_data_component::<NavMapBeacon>();
    world.register_data_component::<ConfigurableNavMapBeacon>();
    world.register_data_component::<SpookySpeaker>();
    world.register_data_component::<SpookyExtinguishable>();
    world.register_net_event::<GhostMessage>();
    if !side.is_server() {
        world.init_resource::<GhostView>();
        world.subscribe_net_event::<GhostMessage>(|world, event| {
            if let GhostMessage::Warps(warps) = &event.event {
                let mut warps = warps.clone();
                // Сначала игроки, потом точки; по имени (`UpdateWarps`).
                warps.sort_by(|a, b| {
                    (a.is_warp_point, &a.display_name).cmp(&(b.is_warp_point, &b.display_name))
                });
                let mut view = world.resource_mut::<GhostView>();
                view.warps = warps;
                view.version += 1;
            }
        });
        world.subscribe_broadcast::<crate::actions::ClientAction>(|world, event| {
            let (performer, kind) = (event.performer, event.kind.clone());
            client_action(world, performer, &kind);
        });
        return;
    }
    world.register_command(ConsoleCommand {
        name: "ghost",
        description: "Стать призраком.",
        help: "ghost",
        execute: ghost_command,
        complete: None,
    });
    world.subscribe_net_event::<GhostMessage>(receive);
    // Текст маяка по умолчанию — и название точки телепорта.
    world.subscribe::<NavMapBeacon, MapInit>(|world, entity, _| {
        let Some(beacon) = world.get::<NavMapBeacon>(entity).cloned() else {
            return;
        };
        let text = match (beacon.text, beacon.default_text) {
            (Some(text), _) => Some(text),
            (None, Some(key)) => Some(loc(world, &key, &[])),
            (None, None) => None,
        };
        if let Some(mut stored) = world.get_mut::<NavMapBeacon>(entity) {
            stored.text.clone_from(&text);
        }
        if world.get::<ConfigurableNavMapBeacon>(entity).is_some()
            && let Some(mut warp) = world.get_mut::<WarpPoint>(entity)
        {
            warp.location = text;
        }
    });
    // Действия призрака (`OnMapInit`): освещение, поле зрения и показ призраков — клиентские.
    world.subscribe::<Ghost, MapInit>(|world, entity, _| {
        let Some(ghost) = world.get::<Ghost>(entity).cloned() else {
            return;
        };
        for action in [
            &ghost.boo_action,
            &ghost.toggle_ghost_hearing_action,
            &ghost.toggle_lighting_action,
            &ghost.toggle_fov_action,
            &ghost.toggle_ghosts_action,
        ] {
            world.add_action(entity, action, None);
        }
    });
    world.subscribe::<Ghost, ActionEvent>(|world, entity, event| {
        if event.handled {
            return;
        }
        match event.kind.as_str() {
            "BooActionEvent" => event.handled = boo(world, entity),
            "ToggleGhostHearingActionEvent" => {
                toggle_hearing(world, entity, event.action);
                event.handled = true;
            }
            _ => {}
        }
    });
    world.subscribe::<SpookySpeaker, GhostBoo>(on_speaker_boo);
    world.subscribe::<SpookyExtinguishable, GhostBoo>(on_extinguish_boo);
    // Призрак не трогает мир (`OnAttempt`, `OnAttemptInteract`).
    world.subscribe::<Ghost, UseAttempt>(|world, entity, event| {
        cancel_if_cannot_interact(world, entity, &mut event.cancelled);
    });
    world.subscribe::<Ghost, InteractionAttempt>(|world, entity, event| {
        cancel_if_cannot_interact(world, entity, &mut event.cancelled);
    });
    world.subscribe::<Ghost, crate::chat::EmoteAttempt>(|world, entity, event| {
        cancel_if_cannot_interact(world, entity, &mut event.cancelled);
    });
    world.subscribe::<Ghost, DropAttempt>(|world, entity, event| {
        cancel_if_cannot_interact(world, entity, &mut event.cancelled);
    });
    world.subscribe::<Ghost, PickupAttempt>(|world, entity, event| {
        cancel_if_cannot_interact(world, entity, &mut event.cancelled);
    });
    world.subscribe::<Ghost, crate::entity_storage::InsertIntoEntityStorageAttempt>(
        |_, _, event| event.cancelled = true,
    );
    // Сколько прошло со смерти (`OnGhostExamine`).
    world.subscribe::<Ghost, Examined>(|world, entity, event| {
        let Some(death) = world.get::<Ghost>(entity).map(|g| g.time_of_death) else {
            return;
        };
        let since = world
            .resource::<GameTime>()
            .cur_time()
            .saturating_sub(death)
            .as_secs();
        let line = if since >= 60 {
            loc(
                world,
                "comp-ghost-examine-time-minutes",
                &[("minutes", LocArg::from(since / 60 % 60))],
            )
        } else {
            loc(
                world,
                "comp-ghost-examine-time-seconds",
                &[("seconds", LocArg::from(since % 60))],
            )
        };
        event.push_markup(line, 0);
    });
    // В конце раунда призраков видят все (`RoundEndMessageEvent` → `MakeVisible`).
    world.subscribe_broadcast::<crate::game_ticker::GameRunLevelChanged>(|world, event| {
        if event.new == crate::game_ticker::GameRunLevel::PostRound {
            make_visible(world, true);
        }
    });
    world.add_game_systems(Update, ghost_on_move);
    // Призрака видят только призраки, а он видит их (`OnGhostStartup`, `OnGhostVis`).
    world.subscribe::<Ghost, ComponentStartup>(|world, entity, _| {
        let post_round =
            crate::game_ticker::run_level(world) == crate::game_ticker::GameRunLevel::PostRound;
        if !post_round {
            world.set_visibility_layer(entity, VISIBILITY_GHOST);
        }
        if let Some(mut eye) = world.get_mut::<Eye>(entity) {
            eye.vis_mask.0 |= VISIBILITY_GHOST;
        }
        let now = world.resource::<GameTime>().cur_time();
        set_time_of_death(world, entity, now);
    });
    world.subscribe::<Ghost, MindRemoved>(|world, entity, _| delete_ghost(world, entity));
    world.subscribe::<Ghost, MindUnvisited>(|world, entity, _| delete_ghost(world, entity));
    world.subscribe::<Ghost, PlayerDetached>(|world, entity, _| delete_ghost(world, entity));
}

#[cfg(test)]
mod tests;
