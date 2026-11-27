//! Медсканер: щелчок по живому существу — сканирование с задержкой 0,8 с, затем окно с
//! состоянием пациента, которое обновляется раз в секунду, пока пациент в 2,5 тайлах (дальше
//! — «неактивно» до возвращения). Выпустил из рук или убрал в сумку — слежение кончается.
//! Состояние окна: пациент, температура, уровень крови, кровотечение; урон и состояние моба
//! окно читает с самой сущности пациента. Образец — `HealthAnalyzerComponent`,
//! `HealthAnalyzerSystem`, `ServerHealthAnalyzerSystem`, `HealthAnalyzerUiState` SS14 @
//! 2f4a2aad.
//!
//! Невоскрешаемых (`UnrevivableComponent`) пока нет: пометка «нельзя реанимировать» не
//! выставляется.

use std::time::Duration;

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, RegistryWorldExt, Update};
use astrocore_loc::EntityNameWorldExt;
use astrocore_map::{EntGotInsertedIntoContainer, TransformWorldExt};
use astrocore_net::NetEntity;
use astrocore_ui::UiWorldExt;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::do_after::{DoAfterArgs, DoAfterDone, DoAfterStart, DoAfterWorldExt};
use crate::examine::loc;
use crate::hands::Dropped;
use crate::interaction::AfterInteract;
use crate::mobs::MobStateComponent;
use crate::popups::{PopupType, PopupWorldExt};

/// Ключ окна (`HealthAnalyzerUiKey.Key`).
pub const HEALTH_ANALYZER_UI_KEY: &str = "enum.HealthAnalyzerUiKey.Key";

fn one() -> f32 {
    1.0
}
fn scan_delay() -> f32 {
    0.8
}
fn max_scan_range() -> Option<f32> {
    Some(2.5)
}
fn end_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Items/Medical/healthscanner.ogg")
}

/// Медсканер (`HealthAnalyzerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthAnalyzer {
    #[serde(skip)]
    pub next_update: Duration,
    /// Между обновлениями окна, с.
    #[serde(default = "one")]
    pub update_interval: f32,
    /// Пациент в пределах — окно обновляется.
    #[serde(default)]
    pub is_analyzer_active: bool,
    /// Задержка сканирования, с.
    #[serde(default = "scan_delay")]
    pub scan_delay: f32,
    #[serde(skip)]
    pub scanned_entity: Option<Entity>,
    /// Дальность слежения в тайлах; `None` — без предела.
    #[serde(default = "max_scan_range")]
    pub max_scan_range: Option<f32>,
    #[serde(default)]
    pub scanning_begin_sound: Option<SoundSpecifier>,
    #[serde(default = "end_sound")]
    pub scanning_end_sound: SoundSpecifier,
    /// Без сообщения пациенту и звука конца.
    #[serde(default)]
    pub silent: bool,
}

/// Сканирование закончено (`HealthAnalyzerDoAfterEvent`).
#[derive(Debug, Clone, Copy)]
pub struct HealthAnalyzerDoAfter;

/// Состояние окна (`HealthAnalyzerUiState`). Нет значения — `None` (у SS14 — `NaN`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HealthAnalyzerUiState {
    pub target_entity: Option<NetEntity>,
    /// Температура тела, К.
    pub temperature: Option<f32>,
    /// Доля крови.
    pub blood_level: Option<f32>,
    pub scan_mode: bool,
    pub bleeding: bool,
    pub unrevivable: bool,
}

fn now(world: &World) -> Duration {
    world.resource::<GameTime>().cur_time()
}

fn interval(analyzer: &HealthAnalyzer) -> Duration {
    Duration::from_secs_f32(analyzer.update_interval)
}

/// Состояние окна по пациенту (`GetHealthAnalyzerUiState`).
pub fn ui_state(world: &World, target: Option<Entity>, scan_mode: bool) -> HealthAnalyzerUiState {
    let Some(entity) = target.filter(|e| world.get_entity(*e).is_ok()) else {
        return HealthAnalyzerUiState::default();
    };
    let blood = world.get::<crate::bloodstream::Bloodstream>(entity);
    HealthAnalyzerUiState {
        target_entity: world.get::<NetEntity>(entity).copied(),
        temperature: crate::temperature::temperature_of(world, entity),
        blood_level: blood.map(crate::bloodstream::Bloodstream::blood_level),
        scan_mode,
        bleeding: blood.is_some_and(|b| b.bleed_amount > 0.0),
        unrevivable: false,
    }
}

fn update_ui(world: &mut World, analyzer: Entity) {
    let Some(component) = world.get::<HealthAnalyzer>(analyzer) else {
        return;
    };
    let state = ui_state(
        world,
        component.scanned_entity,
        component.is_analyzer_active,
    );
    world.set_ui_state(analyzer, HEALTH_ANALYZER_UI_KEY, &state);
}

/// Связать сканер с пациентом (`BeginAnalyzingEntity`).
pub fn begin_analyzing(world: &mut World, analyzer: Entity, target: Entity) {
    let time = now(world);
    let Some(mut component) = world.get_mut::<HealthAnalyzer>(analyzer) else {
        return;
    };
    component.scanned_entity = Some(target);
    component.is_analyzer_active = true;
    component.next_update = time + interval(&component);
    update_ui(world, analyzer);
}

/// Кончить слежение (`StopAnalyzingEntity`).
pub fn stop_analyzing(world: &mut World, analyzer: Entity) {
    let Some(mut component) = world.get_mut::<HealthAnalyzer>(analyzer) else {
        return;
    };
    if component.scanned_entity.is_none() && !component.is_analyzer_active {
        return;
    }
    component.scanned_entity = None;
    component.is_analyzer_active = false;
    component.next_update = Duration::ZERO;
    update_ui(world, analyzer);
}

/// Пациент далеко: последнее обновление и «неактивно» (`PauseAnalyzingEntity`).
fn pause_analyzing(world: &mut World, analyzer: Entity) {
    let Some(mut component) = world.get_mut::<HealthAnalyzer>(analyzer) else {
        return;
    };
    if !component.is_analyzer_active {
        return;
    }
    component.is_analyzer_active = false;
    update_ui(world, analyzer);
}

fn in_scan_range(world: &World, analyzer: Entity, patient: Entity, range: f32) -> bool {
    match (
        world.map_coordinates(analyzer),
        world.map_coordinates(patient),
    ) {
        (Some(a), Some(b)) => a.in_range(&b, range),
        _ => false,
    }
}

fn update(world: &mut World) {
    let time = now(world);
    let due: Vec<(Entity, Entity)> = world
        .query::<(Entity, &HealthAnalyzer)>()
        .iter(world)
        .filter(|(_, a)| a.next_update <= time)
        .filter_map(|(e, a)| Some((e, a.scanned_entity?)))
        .collect();
    for (analyzer, patient) in due {
        let Some(mut component) = world.get_mut::<HealthAnalyzer>(analyzer) else {
            continue;
        };
        component.next_update = time + interval(&component);
        let range = component.max_scan_range;
        if world.get_entity(patient).is_err() {
            stop_analyzing(world, analyzer);
            continue;
        }
        if let Some(range) = range
            && !in_scan_range(world, analyzer, patient, range)
        {
            pause_analyzing(world, analyzer);
            continue;
        }
        if let Some(mut component) = world.get_mut::<HealthAnalyzer>(analyzer) {
            component.is_analyzer_active = true;
        }
        update_ui(world, analyzer);
    }
}

/// Начать сканирование (`OnAfterInteract`).
fn on_after_interact(world: &mut World, analyzer: Entity, event: &mut AfterInteract) {
    let Some(target) = event.target else {
        return;
    };
    if !event.can_reach || world.get::<MobStateComponent>(target).is_none() {
        return;
    }
    let Some(component) = world.get::<HealthAnalyzer>(analyzer).cloned() else {
        return;
    };
    if let Some(sound) = &component.scanning_begin_sound {
        world.play_entity(sound, analyzer, None);
    }
    let mut args = DoAfterArgs::new(
        event.user,
        f64::from(component.scan_delay),
        Some(target),
        Some(analyzer),
        Some(analyzer),
    );
    args.need_hand = true;
    args.break_on_move = true;
    let refused = world.try_start_do_after(args, HealthAnalyzerDoAfter) == DoAfterStart::Refused;
    if target == event.user || refused || component.silent {
        return;
    }
    let user = world.entity_arg(event.user);
    let text = loc(
        world,
        "health-analyzer-popup-scan-target",
        &[("user", user.into())],
    );
    world.popup_entity_for(&text, target, target, PopupType::Medium);
}

/// Сканирование закончено: слежение и окно (`OnDoAfter`).
fn on_do_after(
    world: &mut World,
    analyzer: Entity,
    event: &mut DoAfterDone<HealthAnalyzerDoAfter>,
) {
    if event.handled || event.cancelled {
        return;
    }
    let Some(target) = event.target.filter(|t| world.get_entity(*t).is_ok()) else {
        return;
    };
    let Some(component) = world.get::<HealthAnalyzer>(analyzer).cloned() else {
        return;
    };
    if !component.silent {
        world.play_entity(&component.scanning_end_sound, analyzer, None);
    }
    begin_analyzing(world, analyzer, target);
    world.open_ui(analyzer, HEALTH_ANALYZER_UI_KEY, event.user);
    event.handled = true;
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<HealthAnalyzer>();
    if !side.is_server() {
        return;
    }
    world.subscribe::<HealthAnalyzer, AfterInteract>(on_after_interact);
    world.subscribe::<HealthAnalyzer, DoAfterDone<HealthAnalyzerDoAfter>>(on_do_after);
    world.subscribe::<HealthAnalyzer, Dropped>(|world, analyzer, _| {
        stop_analyzing(world, analyzer);
    });
    world.subscribe::<HealthAnalyzer, EntGotInsertedIntoContainer>(|world, analyzer, _| {
        stop_analyzing(world, analyzer);
    });
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
