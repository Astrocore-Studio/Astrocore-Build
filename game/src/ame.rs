//! Антиматериальный двигатель (AME): части щита, собранные квадратом, образуют ядра;
//! контроллер раз в 10 секунд впрыскивает антиматерию из банки и отдаёт мощность в сеть
//! высокого напряжения. Образец — `AmeNodeGroup`, `AmeControllerComponent`,
//! `AmeControllerSystem`, `AmeShieldComponent`, `AmeShieldingSystem`,
//! `AmeFuelContainerComponent` SS14 @ 2f4a2aad.
//!
//! Двигатель — группа узлов `AMEngine` (`node_container`): части щита и контроллер, соседние по
//! сторонам. Ядро — часть, у которой все восемь соседних клеток заняты частями. Мощность —
//! `200 000 · log10(2 · топливо / √ядра)` Вт; впрыск больше двух единиц на ядро портит
//! целостность ядер. Взрыв при нулевой стабильности — вместе со взрывами (5.9).

use astrocore_audio::{AudioWorldExt, SoundSpecifier};
use astrocore_ecs::{
    ComponentInit, EventWorldExt, GameTime, GameWorldExt, Paused, RegistryWorldExt, Update,
};
use astrocore_light::PointLight;
use astrocore_map::{MapGrid, TileCoord, Transform};
use astrocore_net::{AppearanceWorldExt, NetRegistryWorldExt};
use astrocore_ui::{BoundUiMessage, UiWorldExt};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::Side;
use crate::item_slots::{ItemSlot, add_item_slot, set_lock, slot_item, try_eject_to_hands};
use crate::node_container::NodeGroups;
use crate::power::PowerSupplier;
use crate::random::random;

/// Слот банки топлива (`SharedAmeControllerComponent.FuelSlotId`).
pub const FUEL_SLOT: &str = "fuelSlot";
/// Ключ окна контроллера.
pub const AME_UI_KEY: &str = "enum.AmeControllerUiKey.Key";
/// Группа узлов двигателя.
pub const AME_GROUP: &str = "AMEngine";

fn two() -> i32 {
    2
}

fn hundred() -> i32 {
    100
}

fn ten() -> f64 {
    10.0
}

fn click_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/machine_switch.ogg")
}

fn inject_sound() -> SoundSpecifier {
    SoundSpecifier::path("/Audio/Machines/ame_fuelinjection.ogg")
}

/// Контроллер (`AmeControllerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmeController {
    #[serde(default)]
    pub fuel_slot: ItemSlot,
    #[serde(default)]
    pub injecting: bool,
    #[serde(default = "two")]
    pub injection_amount: i32,
    #[serde(default = "hundred")]
    pub stability: i32,
    /// Секунды между впрысками.
    #[serde(default = "ten")]
    pub update_period: f64,
    #[serde(default = "click_sound")]
    pub click_sound: SoundSpecifier,
    #[serde(default = "inject_sound")]
    pub inject_sound: SoundSpecifier,
    /// Время следующего впрыска, секунды игры.
    #[serde(skip)]
    pub next_update: f64,
}

/// Часть щита (`AmeShieldComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AmeShield {
    #[serde(default = "hundred")]
    pub integrity: i32,
    #[serde(skip)]
    pub is_core: bool,
}

fn five_hundred() -> i32 {
    500
}

/// Банка антиматерии (`AmeFuelContainerComponent`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmeFuelContainer {
    #[serde(default = "five_hundred")]
    pub fuel_amount: i32,
    #[serde(default = "five_hundred")]
    pub fuel_capacity: i32,
}

/// Состояние окна контроллера (`AmeControllerBoundUserInterfaceState`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AmeUiState {
    pub has_power: bool,
    pub is_master: bool,
    pub injecting: bool,
    pub has_fuel_jar: bool,
    pub fuel_amount: i32,
    pub injection_amount: i32,
    pub core_count: usize,
    /// Отдаёт сейчас, кВт.
    pub current_power_supply: f32,
    /// Отдаст при текущем впрыске, кВт.
    pub targeted_power_supply: f32,
}

/// Кнопки окна (`UiButton`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AmeUiButton {
    Eject,
    ToggleInjection,
    IncreaseFuel,
    DecreaseFuel,
}

/// Двигатель: части группы, ядра, главный контроллер.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Engine {
    pub cores: Vec<Entity>,
    pub master: Option<Entity>,
}

/// Мощность при впрыске (`CalculatePower`), Вт.
pub fn calculate_power(fuel: i32, cores: usize) -> f32 {
    if fuel <= 0 || cores == 0 {
        return 0.0;
    }
    (200_000.0 * (2.0 * fuel as f32 * (cores as f32).powf(-0.5)).log10()).max(0.0)
}

/// Двигатель контроллера (`TryGetAMENodeGroup` + `LoadNodes`).
pub fn engine_of(world: &World, controller: Entity) -> Option<Engine> {
    let groups = world.get_resource::<NodeGroups>()?;
    let group = groups.group_of(controller, "ame")?;
    let group = &groups.groups[group];
    if group.kind != AME_GROUP {
        return None;
    }
    let members: Vec<Entity> = group.nodes.iter().map(|(entity, _)| *entity).collect();
    let mut cores = Vec::new();
    for &member in &members {
        if world.get::<AmeShield>(member).is_none() {
            continue;
        }
        let Some(transform) = world.get::<Transform>(member) else {
            continue;
        };
        let (Some(grid), true) = (transform.grid(), transform.anchored()) else {
            continue;
        };
        let cell = TileCoord::containing(transform.local_position(), transform.level());
        let Some(map_grid) = world.get::<MapGrid>(grid) else {
            continue;
        };
        let mut neighbours = 0;
        for dx in -1..=1 {
            for dy in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let here = TileCoord::new(cell.x + dx, cell.y + dy, cell.z);
                neighbours += map_grid
                    .anchored_at(here)
                    .iter()
                    .filter(|e| **e != member && world.get::<AmeShield>(**e).is_some())
                    .count();
            }
        }
        if neighbours >= 8 {
            cores.push(member);
        }
    }
    let master = members
        .iter()
        .copied()
        .filter(|e| world.get::<AmeController>(*e).is_some())
        .min();
    Some(Engine { cores, master })
}

/// Вид ядер и щита (`SetCore`, `UpdateCoreVisuals`).
fn update_core_visuals(world: &mut World, engine: &Engine, controller: Option<&AmeController>) {
    let (amount, injecting) = controller.map_or((0, false), |c| (c.injection_amount, c.injecting));
    let strength = if engine.cores.is_empty() {
        0
    } else {
        amount / engine.cores.len() as i32
    };
    for &core in &engine.cores {
        world.set_appearance(core, "enum.AmeShieldVisuals.Core", true);
        let state = match (injecting, strength > 2) {
            (false, _) => "Off",
            (true, true) => "Strong",
            (true, false) => "Weak",
        };
        world.set_appearance(core, "enum.AmeShieldVisuals.CoreState", state.to_owned());
        if let Some(mut light) = world.get_mut::<PointLight>(core) {
            light.enabled = injecting;
            light.radius = strength.clamp(1, 12) as f32;
        }
    }
}

/// Показания контроллера (`UpdateDisplay`).
fn update_display(world: &mut World, controller: Entity) {
    let Some(state) = world.get::<AmeController>(controller).map(|c| {
        if !c.injecting {
            "Off"
        } else {
            match c.stability {
                ..10 => "Fuck",
                10..50 => "Critical",
                50..80 => "Warning",
                _ => "On",
            }
        }
    }) else {
        return;
    };
    world.set_appearance(
        controller,
        "enum.AmeControllerVisuals.DisplayState",
        state.to_owned(),
    );
}

/// Состояние окна (`GetUiState`).
pub fn ui_state(world: &World, controller: Entity) -> Option<AmeUiState> {
    let component = world.get::<AmeController>(controller)?;
    let engine = engine_of(world, controller);
    let cores = engine.as_ref().map_or(0, |e| e.cores.len());
    let jar = slot_item(world, controller, FUEL_SLOT);
    let fuel = jar.and_then(|jar| world.get::<AmeFuelContainer>(jar));
    let current = world
        .get::<PowerSupplier>(controller)
        .filter(|_| cores > 0)
        .map_or(0.0, |s| s.current_supply / 1000.0);
    Some(AmeUiState {
        has_power: crate::power::is_powered(world, controller),
        is_master: engine.as_ref().and_then(|e| e.master) == Some(controller),
        injecting: component.injecting && fuel.is_some(),
        has_fuel_jar: jar.is_some(),
        fuel_amount: fuel.map_or(0, |f| f.fuel_amount),
        injection_amount: component.injection_amount,
        core_count: cores,
        current_power_supply: current,
        targeted_power_supply: calculate_power(component.injection_amount, cores) / 1000.0,
    })
}

fn update_ui(world: &mut World, controller: Entity) {
    if let Some(state) = ui_state(world, controller) {
        world.set_ui_state(controller, AME_UI_KEY, &state);
    }
}

/// Включает или выключает впрыск (`SetInjecting`).
pub fn set_injecting(world: &mut World, controller: Entity, value: bool) {
    let Some(mut component) = world.get_mut::<AmeController>(controller) else {
        return;
    };
    if component.injecting == value {
        return;
    }
    component.injecting = value;
    if !value && let Some(mut supplier) = world.get_mut::<PowerSupplier>(controller) {
        supplier.supply_rate = 0.0;
    }
    set_lock(world, controller, FUEL_SLOT, value);
    update_display(world, controller);
    update_ui(world, controller);
}

/// Меняет количество впрыска с шагом (`AdjustInjectionAmount`): от 0 до 8 на ядро.
pub fn adjust_injection(world: &mut World, controller: Entity, delta: i32) {
    let max = engine_of(world, controller).map_or(0, |e| e.cores.len() as i32 * 8);
    if let Some(mut component) = world.get_mut::<AmeController>(controller) {
        component.injection_amount = (component.injection_amount + delta).clamp(0, max);
    }
    update_ui(world, controller);
}

/// Впрыск (`UpdateController` + `InjectFuel`).
fn inject(world: &mut World, controller: Entity) {
    let Some(component) = world.get::<AmeController>(controller).cloned() else {
        return;
    };
    let engine = engine_of(world, controller);
    if component.injecting
        && let Some(engine) = &engine
    {
        let jar = slot_item(world, controller, FUEL_SLOT);
        let fuel = jar.and_then(|jar| {
            world
                .get::<AmeFuelContainer>(jar)
                .map(|f| (jar, f.fuel_amount))
        });
        match fuel {
            Some((_, amount)) if amount <= 0 => set_injecting(world, controller, false),
            Some((jar, amount)) => {
                let injected = component.injection_amount.min(amount);
                let cores = engine.cores.len();
                let mut power = 0.0;
                let mut overloading = false;
                if injected > 0 && cores > 0 {
                    power = calculate_power(injected, cores);
                    // Сверх двух единиц на ядро ядра теряют целостность.
                    if injected > cores as i32 * 2 {
                        overloading = true;
                        let fuzz = random(world).range(-1, 2);
                        let instability = (injected - cores as i32) / cores as i32 + fuzz;
                        for &core in &engine.cores {
                            if let Some(mut shield) = world.get_mut::<AmeShield>(core) {
                                shield.integrity -= instability;
                            }
                        }
                    }
                }
                if let Some(mut supplier) = world.get_mut::<PowerSupplier>(controller) {
                    supplier.supply_rate = power;
                }
                if let Some(mut container) = world.get_mut::<AmeFuelContainer>(jar) {
                    container.fuel_amount -= injected;
                }
                if injected > 0 {
                    let mut sound = component.inject_sound.clone();
                    if overloading
                        && let SoundSpecifier::Path { params, .. }
                        | SoundSpecifier::Collection { params, .. } = &mut sound
                    {
                        params.volume += 10.0;
                    }
                    world.play_entity(&sound, controller, None);
                }
            }
            None => {}
        }
    }
    // Стабильность — средняя целостность ядер (`GetTotalStability`).
    let stability = engine.as_ref().map_or(100, |engine| {
        if engine.cores.is_empty() {
            100
        } else {
            let total: i32 = engine
                .cores
                .iter()
                .filter_map(|c| world.get::<AmeShield>(*c).map(|s| s.integrity))
                .sum();
            total / engine.cores.len() as i32
        }
    });
    if let Some(mut component) = world.get_mut::<AmeController>(controller) {
        component.stability = stability;
    }
    if let Some(engine) = &engine {
        let current = world.get::<AmeController>(controller).cloned();
        update_core_visuals(world, engine, current.as_ref());
    }
    update_display(world, controller);
    update_ui(world, controller);
}

fn update(world: &mut World) {
    let now = world.resource::<GameTime>().cur_time().as_secs_f64();
    let due: Vec<Entity> = world
        .query::<(Entity, &AmeController)>()
        .iter(world)
        .filter(|(entity, controller)| {
            controller.next_update <= now && world.get::<Paused>(*entity).is_none()
        })
        .map(|(entity, _)| entity)
        .collect();
    for controller in due {
        if let Some(mut component) = world.get_mut::<AmeController>(controller) {
            component.next_update = now + component.update_period;
        }
        inject(world, controller);
    }
}

fn on_button(world: &mut World, controller: Entity, event: &mut BoundUiMessage<AmeUiButton>) {
    let needs_power = event.message != AmeUiButton::Eject;
    if needs_power && !crate::power::is_powered(world, controller) {
        return;
    }
    if let Some(sound) = world
        .get::<AmeController>(controller)
        .map(|c| c.click_sound.clone())
    {
        world.play_entity(&sound, controller, None);
    }
    match event.message {
        AmeUiButton::Eject => {
            if !world
                .get::<AmeController>(controller)
                .is_some_and(|c| c.injecting)
            {
                try_eject_to_hands(world, controller, FUEL_SLOT, Some(event.actor));
            }
        }
        AmeUiButton::ToggleInjection => {
            let value = !world
                .get::<AmeController>(controller)
                .is_some_and(|c| c.injecting);
            set_injecting(world, controller, value);
        }
        AmeUiButton::IncreaseFuel => adjust_injection(world, controller, 2),
        AmeUiButton::DecreaseFuel => adjust_injection(world, controller, -2),
    }
    if let Some(engine) = engine_of(world, controller) {
        let current = world.get::<AmeController>(controller).cloned();
        update_core_visuals(world, &engine, current.as_ref());
    }
    update_ui(world, controller);
}

pub fn install(world: &mut World, side: Side) {
    world.register_data_component::<AmeController>();
    world.register_data_component::<AmeShield>();
    world.register_data_component::<AmeFuelContainer>();
    world.register_net_serde::<AmeFuelContainer>();
    world.register_ui_message::<AmeUiButton>();
    if !side.is_server() {
        return;
    }
    // Слот топлива (`OnInit`: `AddItemSlot`).
    world.subscribe::<AmeController, ComponentInit>(|world, controller, _| {
        if let Some(slot) = world
            .get::<AmeController>(controller)
            .map(|c| c.fuel_slot.clone())
        {
            add_item_slot(world, controller, FUEL_SLOT, slot);
        }
    });
    world.subscribe::<AmeController, BoundUiMessage<AmeUiButton>>(on_button);
    world.add_game_systems(
        Update,
        bevy_ecs::schedule::IntoScheduleConfigs::after(
            update,
            crate::node_container::update_system,
        ),
    );
}

#[cfg(test)]
mod tests;
