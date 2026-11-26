//! Тело из органов: голова, руки, кисти, ноги — отдельные сущности в контейнере тела.
//! Кисть даёт телу руку, видимые органы рисуют части тела цветом кожи из профиля вида.
//! Образец — `Content.Shared/Body` SS14 @ 2f4a2aad (`BodyComponent`, `OrganComponent`,
//! `InitialBodySystem`, `HandOrganSystem`, `VisualOrganComponent`, `SharedVisualBodySystem`)
//! и `HumanoidCharacterAppearance.DefaultWithSpecies`, `HumanTonedSkinColoration`.
//! Метки внешности (волосы, бельё — `VisualOrganMarkings`) и связи органов пока не
//! переносятся; профиль из лобби — фаза 6.

use std::collections::BTreeMap;

use astrocore_ecs::{ComponentInit, EventWorldExt, LifecycleWorldExt, MapInit, RegistryWorldExt};
use astrocore_map::{
    ContainerKind, ContainerWorldExt, EntGotInsertedIntoContainer, EntGotRemovedFromContainer,
    SpawnWorldExt, TransformWorldExt,
};
use astrocore_math::Color;
use astrocore_net::NetRegistryWorldExt;
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::hands::{Hand, HandsWorldExt};
use crate::ordered::OrderedMap;

/// Контейнер органов (`BodyComponent.ContainerID`).
pub const BODY_ORGANS: &str = "body_organs";
/// Слой глаз (`HumanoidVisualLayers.Eyes`): красится цветом глаз.
pub const EYES_LAYER: &str = "enum.HumanoidVisualLayers.Eyes";

/// Тело. В данных — компонент `Body`.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Body {}

/// Орган. В данных — компонент `Organ`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Organ {
    #[serde(skip)]
    pub body: Option<Entity>,
    #[serde(default)]
    pub category: Option<String>,
}

/// Органы, с которыми тело появляется. В данных — компонент `InitialBody`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InitialBody {
    pub organs: OrderedMap<String>,
    #[serde(default)]
    pub relationships: Option<BTreeMap<String, Vec<String>>>,
}

/// Кисть: даёт телу руку. В данных — компонент `HandOrgan`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandOrgan {
    #[serde(rename = "handID")]
    pub hand_id: String,
    pub data: Hand,
}

/// Пол (`Sex`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum Sex {
    #[default]
    Male,
    Female,
    Unsexed,
}

/// Цвета органа из профиля (`OrganProfileData`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganProfileData {
    #[serde(default)]
    pub sex: Sex,
    #[serde(default = "white")]
    pub eye_color: Color,
    #[serde(default = "white")]
    pub skin_color: Color,
}

fn white() -> Color {
    Color::WHITE
}

impl Default for OrganProfileData {
    fn default() -> Self {
        Self {
            sex: Sex::Male,
            eye_color: Color::WHITE,
            skin_color: Color::WHITE,
        }
    }
}

/// Слой спрайта органа (`PrototypeLayerData`): поля, которые орган задаёт слою тела.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OrganLayerData {
    pub sprite: Option<String>,
    pub state: Option<String>,
    pub color: Option<Color>,
    pub visible: Option<bool>,
    pub shader: Option<String>,
}

/// Видимый орган: рисует слой тела. В данных — компонент `VisualOrgan`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualOrgan {
    pub layer: String,
    pub data: OrganLayerData,
    #[serde(default)]
    pub sex_state_overrides: Option<BTreeMap<Sex, String>>,
    #[serde(default)]
    pub profile: OrganProfileData,
    #[serde(default)]
    pub displacement: Option<String>,
}

/// Тело рисуется органами. В данных — компонент `VisualBody`.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct VisualBody {}

fn human() -> String {
    "Human".into()
}

/// Профиль гуманоида: вид и пол. В данных — компонент `HumanoidProfile`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HumanoidProfile {
    #[serde(default = "human")]
    pub species: String,
    #[serde(default)]
    pub sex: Sex,
    #[serde(default)]
    pub age: Option<i32>,
}

impl Default for HumanoidProfile {
    fn default() -> Self {
        Self {
            species: human(),
            sex: Sex::Male,
            age: None,
        }
    }
}

/// Орган вставлен в тело (`OrganGotInsertedEvent`). На органе.
#[derive(Debug, Clone, Copy)]
pub struct OrganGotInserted {
    pub body: Entity,
}

/// Орган вынут из тела (`OrganGotRemovedEvent`). На органе.
#[derive(Debug, Clone, Copy)]
pub struct OrganGotRemoved {
    pub body: Entity,
}

/// Цвет из HSV (`Color.FromHsv` Robust): оттенок, насыщенность, яркость от 0 до 1.
pub fn color_from_hsv(hue: f32, saturation: f32, value: f32) -> Color {
    let h = (hue.rem_euclid(1.0)) * 6.0;
    let c = value * saturation;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = value - c;
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Color::rgba(r + m, g + m, b + m, 1.0)
}

/// Тон человеческой кожи от 0 (светлый) до 100 (тёмный) (`HumanTonedSkinColoration.FromUnary`).
pub fn human_skin_tone(tone: f32) -> Color {
    let tone = tone.clamp(0.0, 100.0);
    let offset = tone - 20.0;
    let (mut hue, mut saturation, mut value) = (25.0, 20.0, 100.0);
    if offset <= 0.0 {
        hue += offset.abs();
    } else {
        saturation += offset;
        value -= offset;
    }
    color_from_hsv(hue / 360.0, saturation / 100.0, value / 100.0)
}

/// Цвет кожи вида по умолчанию (`DefaultWithSpecies`).
pub fn default_skin_color(world: &World, species: &str) -> Color {
    let Some(prototypes) = world.get_resource::<Prototypes>() else {
        return Color::WHITE;
    };
    let Some(node) = prototypes.resolved_node("species", species) else {
        return Color::WHITE;
    };
    let coloration = node
        .get("skinColoration")
        .and_then(|n| n.as_str())
        .unwrap_or("");
    let unary = coloration == "HumanToned"
        || prototypes
            .resolved_node("skinColoration", coloration)
            .and_then(|n| n.get("strategy"))
            .and_then(|s| s.tag.clone())
            .is_some_and(|tag| tag.ends_with("HumanTonedSkinColoration"));
    if unary {
        let tone = node
            .get("defaultHumanSkinTone")
            .and_then(|n| n.as_str())
            .and_then(|t| t.parse().ok())
            .unwrap_or(20.0);
        return human_skin_tone(tone);
    }
    node.get("defaultSkinTone")
        .and_then(|n| n.as_str())
        .and_then(Color::parse)
        .unwrap_or(Color::WHITE)
}

/// Тело на `World`.
pub trait BodyWorldExt {
    /// Органы тела.
    fn organs(&self, body: Entity) -> Vec<Entity>;
}

impl BodyWorldExt for World {
    fn organs(&self, body: Entity) -> Vec<Entity> {
        self.container(body, BODY_ORGANS)
            .map(|c| c.entities().to_vec())
            .unwrap_or_default()
    }
}

/// Цвета профиля органам (`ApplyOrganProfileDataEvent`): кожа, глаза, состояние по полу.
fn apply_profile(world: &mut World, body: Entity) {
    let Some(profile) = world.get::<HumanoidProfile>(body).cloned() else {
        return;
    };
    let skin = default_skin_color(world, &profile.species);
    let data = OrganProfileData {
        sex: profile.sex,
        eye_color: Color::BLACK,
        skin_color: skin,
    };
    for organ in world.organs(body) {
        if world
            .get::<Organ>(organ)
            .and_then(|o| o.category.clone())
            .is_none()
        {
            continue;
        }
        let Some(mut visual) = world.get_mut::<VisualOrgan>(organ) else {
            continue;
        };
        visual.profile = data;
        visual.data.color = Some(if visual.layer == EYES_LAYER {
            data.eye_color
        } else {
            data.skin_color
        });
        if let Some(state) = visual
            .sex_state_overrides
            .as_ref()
            .and_then(|o| o.get(&data.sex))
            .cloned()
        {
            visual.data.state = Some(state);
        }
    }
}

pub(crate) fn install(world: &mut World, side: crate::Side) {
    world.register_data_component::<Body>();
    world.register_data_component::<Organ>();
    world.register_data_component::<InitialBody>();
    world.register_data_component::<HandOrgan>();
    world.register_data_component::<VisualOrgan>();
    // `data` складывается с родительским (`[AlwaysPushInheritance]` в `VisualOrganComponent`).
    world
        .resource_mut::<astrocore_ecs::ComponentRegistry>()
        .set_inheritance("VisualOrgan", &["data"], &[]);
    world.register_data_component::<VisualBody>();
    world.register_data_component::<HumanoidProfile>();
    world.register_net_serde::<VisualOrgan>();
    world.register_net_serde::<HumanoidProfile>();
    world.subscribe::<Body, ComponentInit>(|world, body, _| {
        world.ensure_container(body, BODY_ORGANS, ContainerKind::List);
    });
    // Орган в контейнере тела знает своё тело (`OnBodyEntInserted`).
    world.subscribe::<Organ, EntGotInsertedIntoContainer>(|world, organ, event| {
        if &*event.container != BODY_ORGANS || world.get::<Body>(event.owner).is_none() {
            return;
        }
        if let Some(mut component) = world.get_mut::<Organ>(organ) {
            component.body = Some(event.owner);
        }
        world.raise(organ, &mut OrganGotInserted { body: event.owner });
    });
    world.subscribe::<Organ, EntGotRemovedFromContainer>(|world, organ, event| {
        if &*event.container != BODY_ORGANS {
            return;
        }
        if let Some(mut component) = world.get_mut::<Organ>(organ) {
            component.body = None;
        }
        world.raise(organ, &mut OrganGotRemoved { body: event.owner });
    });
    if !side.is_server() {
        return;
    }
    // Органы появляются в теле при появлении тела (`InitialBodySystem.OnMapInit`).
    world.subscribe::<InitialBody, MapInit>(|world, body, _| {
        let Some(initial) = world.get::<InitialBody>(body).cloned() else {
            return;
        };
        world.ensure_container(body, BODY_ORGANS, ContainerKind::List);
        let Some(at) = world.coordinates(body) else {
            return;
        };
        for (_, prototype) in initial.organs.iter() {
            let Ok(organ) = world.spawn_attached(prototype, at) else {
                continue;
            };
            if world
                .insert_into_container(organ, body, BODY_ORGANS)
                .is_err()
            {
                world.delete_entity(organ);
            }
        }
    });
    // Профиль красит органы после их появления (`SharedVisualBodySystem.OnVisualMapInit`).
    world.subscribe::<VisualBody, MapInit>(|world, body, _| apply_profile(world, body));
    // Кисть даёт руку, без кисти руки нет (`HandOrganSystem`).
    world.subscribe::<HandOrgan, OrganGotInserted>(|world, organ, event| {
        if let Some(hand) = world.get::<HandOrgan>(organ).cloned() {
            world.add_hand(event.body, &hand.hand_id, hand.data);
        }
    });
    world.subscribe::<HandOrgan, OrganGotRemoved>(|world, organ, event| {
        if world.is_alive(event.body)
            && let Some(hand) = world.get::<HandOrgan>(organ).cloned()
        {
            world.remove_hand(event.body, &hand.hand_id);
        }
    });
}

#[cfg(test)]
mod tests;
