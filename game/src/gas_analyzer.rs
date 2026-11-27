//! Газоанализатор: показывает воздух вокруг и газ цели (баллона, канистры, участка трубы)
//! и обновляет окно раз в секунду, пока оно открыто; цель вне досягаемости отпускается.
//! Образец — `GasAnalyzerComponent`, `ActiveGasAnalyzerComponent`, `GasAnalyzerSystem` и
//! `GasAnalyzerScanEvent` SS14 @ 2f4a2aad.
//!
//! Нет пока: поворота окна для перевёрнутых устройств (`DeviceFlipped`) — у трёхходовых
//! смесителей и фильтров своего ответа ещё нет, они показываются по трубам.

use astrocore_ecs::{EventWorldExt, GameTime, GameWorldExt, RegistryWorldExt, Update};
use astrocore_loc::EntityNameWorldExt;
use astrocore_net::AppearanceWorldExt;
use astrocore_ui::{BoundUiClosed, BoundUiOpened, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::atmos::GasMixture;
use crate::atmos::gas::{GAS_COUNT, GAS_NAMES};
use crate::interaction::{AfterInteract, INTERACTION_RANGE, InteractionWorldExt};
use crate::popups::{PopupType, PopupWorldExt};

/// Окно анализатора (`GasAnalyzerUiKey.Key`).
pub const GAS_ANALYZER_UI_KEY: &str = "enum.GasAnalyzerUiKey.Key";
/// Вид включённого (`GasAnalyzerVisuals.Enabled`).
pub const ANALYZER_ENABLED: &str = "enum.GasAnalyzerVisuals.Enabled";
/// Меньше — газ в окне не показывается (`UIMinMoles`).
const UI_MIN_MOLES: f32 = 0.01;
/// Интервал обновления (`UpdateInterval`), с.
const UPDATE_INTERVAL: f32 = 1.0;

/// Газоанализатор (`GasAnalyzerComponent`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasAnalyzer {
    #[serde(skip)]
    pub target: Option<Entity>,
    #[serde(skip)]
    pub user: Option<Entity>,
    #[serde(default)]
    pub enabled: bool,
    /// Включён и обновляется (`ActiveGasAnalyzerComponent.AccumulatedFrametime`).
    #[serde(skip)]
    pub active: Option<f32>,
}

/// Газ в окне (`GasEntry`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasEntry {
    pub gas: String,
    pub amount: f32,
}

/// Смесь в окне (`GasMixEntry`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasMixEntry {
    pub name: String,
    pub volume: f32,
    pub pressure: f32,
    pub temperature: f32,
    pub gases: Vec<GasEntry>,
}

/// Данные окна (`GasAnalyzerUserMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GasAnalyzerState {
    pub mixes: Vec<GasMixEntry>,
    pub device_name: String,
}

/// Чем ответить на сканирование (`GasAnalyzerScanEvent`). На цели.
#[derive(Debug, Clone, Default)]
pub struct GasAnalyzerScan {
    pub gas_mixtures: Option<Vec<(String, Option<GasMixture>)>>,
}

impl GasAnalyzerScan {
    pub fn add(&mut self, name: String, mixture: Option<GasMixture>) {
        self.gas_mixtures
            .get_or_insert_with(Vec::new)
            .push((name, mixture));
    }
}

/// Строка смеси (`GenerateGasMixEntry`): газы по убыванию количества.
pub fn gas_mix_entry(name: String, mixture: Option<&GasMixture>) -> GasMixEntry {
    let Some(mixture) = mixture else {
        return GasMixEntry {
            name,
            volume: 0.0,
            pressure: 0.0,
            temperature: 0.0,
            gases: Vec::new(),
        };
    };
    let mut gases: Vec<GasEntry> = (0..GAS_COUNT)
        .filter(|i| mixture.moles[*i] > UI_MIN_MOLES)
        .map(|i| GasEntry {
            gas: GAS_NAMES[i].to_owned(),
            amount: mixture.moles[i],
        })
        .collect();
    gases.sort_by(|a, b| b.amount.total_cmp(&a.amount));
    GasMixEntry {
        name,
        volume: mixture.volume,
        pressure: mixture.pressure(),
        temperature: mixture.temperature(),
        gases,
    }
}

/// Газ узлов-труб цели: доля сети по объёму узла (без ответа на сканирование).
fn pipe_mixes(world: &World, target: Entity) -> Vec<(String, GasMixture)> {
    let Some(container) = world.get::<crate::node_container::NodeContainer>(target) else {
        return Vec::new();
    };
    container
        .nodes
        .iter()
        .filter(|(_, node)| {
            crate::node_container::node_info(node).is_some_and(|info| info.pipe.is_some())
        })
        .filter_map(|(name, _)| {
            let air = crate::pipes::pipe_air(world, target, name)?;
            if air.volume == 0.0 {
                return None;
            }
            let volume = crate::pipes::node_volume(world, target, name);
            let mut part = air.clone();
            part.multiply(volume / air.volume);
            part.volume = volume;
            Some((name.clone(), part))
        })
        .collect()
}

/// Включить и открыть окно (`ActivateAnalyzer`).
pub fn activate(world: &mut World, analyzer: Entity, user: Entity, target: Option<Entity>) {
    if !world.open_ui(analyzer, GAS_ANALYZER_UI_KEY, user)
        && !world.ui_open_for(analyzer, GAS_ANALYZER_UI_KEY, user)
    {
        return;
    }
    let Some(mut component) = world.get_mut::<GasAnalyzer>(analyzer) else {
        return;
    };
    component.target = target;
    component.user = Some(user);
    component.enabled = true;
    if component.active.is_none() {
        component.active = Some(0.0);
    }
    world.set_appearance(analyzer, ANALYZER_ENABLED, true);
    update_analyzer(world, analyzer);
}

/// Выключить (`DisableAnalyzer`).
fn disable(world: &mut World, analyzer: Entity, user: Option<Entity>) {
    if let Some(user) = user {
        world.close_ui(analyzer, GAS_ANALYZER_UI_KEY, user);
    }
    let Some(mut component) = world.get_mut::<GasAnalyzer>(analyzer) else {
        return;
    };
    let was_enabled = component.enabled;
    component.enabled = false;
    component.user = None;
    component.active = None;
    if let Some(user) = user
        && was_enabled
    {
        let text = crate::examine::loc(world, "gas-analyzer-shutoff", &[]);
        world.popup_entity_for(&text, user, user, PopupType::Small);
    }
    world.set_appearance(analyzer, ANALYZER_ENABLED, false);
}

/// Обновить окно (`UpdateAnalyzer`). `false` — показывать нечего.
pub fn update_analyzer(world: &mut World, analyzer: Entity) -> bool {
    let Some(state) = analyze(world, analyzer) else {
        return false;
    };
    world.set_ui_state(analyzer, GAS_ANALYZER_UI_KEY, &state);
    true
}

/// Данные окна: воздух вокруг и газ цели; цель вне досягаемости или без газа отпускается.
/// `None` — анализатор выключен, цель пропала.
pub fn analyze(world: &mut World, analyzer: Entity) -> Option<GasAnalyzerState> {
    let component = world.get::<GasAnalyzer>(analyzer).cloned()?;
    let mut target = component.target;
    // Ушёл от цели — цель отпускается.
    if let (Some(t), Some(user)) = (target, component.user)
        && world.get_entity(t).is_ok()
        && !world.in_range_unobstructed(user, t, INTERACTION_RANGE)
    {
        if component.enabled {
            let text = crate::examine::loc(world, "gas-analyzer-object-out-of-range", &[]);
            world.popup_entity_for(&text, user, user, PopupType::Small);
        }
        target = None;
    }
    let mut mixes = Vec::new();
    let environment = crate::barotrauma::containing_mixture(world, analyzer);
    let label = crate::examine::loc(world, "gas-analyzer-window-environment-tab-label", &[]);
    mixes.push(gas_mix_entry(label, environment.as_ref()));
    if let Some(t) = target {
        if world.get_entity(t).is_err() {
            if let Some(mut c) = world.get_mut::<GasAnalyzer>(analyzer) {
                c.target = None;
            }
            disable(world, analyzer, component.user);
            return None;
        }
        let mut valid = false;
        let mut scan = GasAnalyzerScan::default();
        world.raise(t, &mut scan);
        match scan.gas_mixtures {
            Some(list) => {
                for (name, mixture) in list {
                    if let Some(mixture) = mixture {
                        mixes.push(gas_mix_entry(name, Some(&mixture)));
                        valid = true;
                    }
                }
            }
            None => {
                for (name, mixture) in pipe_mixes(world, t) {
                    mixes.push(gas_mix_entry(name, Some(&mixture)));
                    valid = true;
                }
            }
        }
        if !valid {
            target = None;
        }
    }
    if let Some(mut c) = world.get_mut::<GasAnalyzer>(analyzer) {
        c.target = target;
    }
    let device_name = target.map(|t| world.entity_name(t)).unwrap_or_default();
    Some(GasAnalyzerState { mixes, device_name })
}

fn update(world: &mut World) {
    let dt = world.resource::<GameTime>().frame_time();
    let active: Vec<Entity> = world
        .query::<(Entity, &GasAnalyzer)>()
        .iter(world)
        .filter(|(_, a)| a.active.is_some())
        .map(|(e, _)| e)
        .collect();
    for analyzer in active {
        let Some(mut component) = world.get_mut::<GasAnalyzer>(analyzer) else {
            continue;
        };
        let accumulated = component.active.unwrap_or(0.0) + dt;
        if accumulated < UPDATE_INTERVAL {
            component.active = Some(accumulated);
            continue;
        }
        component.active = Some(accumulated - UPDATE_INTERVAL);
        if !update_analyzer(world, analyzer)
            && let Some(mut component) = world.get_mut::<GasAnalyzer>(analyzer)
        {
            component.active = None;
        }
    }
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<GasAnalyzer>();
    if !side.is_server() {
        return;
    }
    // Щелчок по цели или по полу (`OnAfterInteract`): воздух вокруг показывается всегда.
    world.subscribe::<GasAnalyzer, AfterInteract>(|world, analyzer, event| {
        let mut target = event.target;
        if let Some(t) = target
            && !world.in_range_unobstructed(event.user, t, INTERACTION_RANGE)
        {
            target = None;
        }
        activate(world, analyzer, event.user, target);
        event.handled = true;
    });
    world.subscribe::<GasAnalyzer, BoundUiOpened>(|world, analyzer, event| {
        if event.key != GAS_ANALYZER_UI_KEY {
            return;
        }
        let active = world
            .get::<GasAnalyzer>(analyzer)
            .is_some_and(|a| a.active.is_some());
        if !active {
            activate(world, analyzer, event.actor, None);
        }
    });
    world.subscribe::<GasAnalyzer, BoundUiClosed>(|world, analyzer, event| {
        if event.key != GAS_ANALYZER_UI_KEY {
            return;
        }
        let active = world
            .get::<GasAnalyzer>(analyzer)
            .is_some_and(|a| a.active.is_some());
        if active && !world.ui_open_for(analyzer, GAS_ANALYZER_UI_KEY, event.actor) {
            disable(world, analyzer, Some(event.actor));
        }
    });
    // Баллон и канистра отвечают своим газом (`OnAnalyzed`).
    world.subscribe::<crate::gas_tank::GasTank, GasAnalyzerScan>(|world, tank, event| {
        let name = world.entity_name(tank);
        let air = crate::gas_tank::tank_air(world, tank).cloned();
        event.add(name, air);
    });
    world.subscribe::<crate::atmos_portables::GasCanister, GasAnalyzerScan>(
        |world, canister, event| {
            let name = world.entity_name(canister);
            let air = crate::atmos_portables::canister_mixture(world, canister).cloned();
            event.add(name, air);
            if let Some(tank) = crate::atmos_portables::inserted_tank(world, canister) {
                let name = world.entity_name(tank);
                let air = crate::gas_tank::tank_air(world, tank).cloned();
                event.add(name, air);
            }
        },
    );
    world.add_game_systems(Update, update);
}

#[cfg(test)]
mod tests;
