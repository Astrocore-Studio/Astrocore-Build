//! Метки внешности: волосы, борода, узоры, хвосты, бельё — прототипы `marking` по слоям тела
//! (`HumanoidVisualLayers`), группы меток вида (`markingsGroup`: пределы и обязательные метки по
//! слоям, белые списки, совпадение с цветом кожи), органы с метками (`VisualOrganMarkings`).
//! Проверка внешности приводит цвет кожи к стратегии окраски вида (`skinColoration`), а метки — к
//! органам, слоям, группе, полу и пределам вида; случайная внешность берёт палитру из случайного
//! цвета и случайные метки по весам. На теле метки ложатся на органы с принудительными цветами
//! (кожа, глаза, цвет другой метки).
//! Образец — `MarkingPrototype`, `MarkingsGroupPrototype`, `MarkingManager`, `MarkingColoring`
//! и `ColoringTypes`, `SkinColorationPrototype` (`HumanTonedSkinColoration`,
//! `ClampedHsvColoration`, `ClampedHslColoration`, `HueNodeClampedHsvColoration`),
//! `HumanoidCharacterAppearance` (`EnsureValid`, `Random`, `RandomizeMarkings`),
//! `VisualOrganMarkingsComponent` и `SharedVisualBodySystem` (`ResolveMarkings`,
//! `OnMarkingsOrganApplyMarkings`), `ColorExtensions` Robust SS14 @ 2f4a2aad.
//!
//! Нет пока: скрытия слоёв одеждой (`HideableLayers` — вместе с видом одежды на теле),
//! смещения слоёв (`Displacement`), окна смены меток администратором.

use std::collections::{BTreeMap, BTreeSet};

use astrocore_data::DataNode;
use astrocore_math::Color;
use astrocore_prototypes::Prototypes;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::body::Sex;
use crate::profiles::{HumanoidCharacterAppearance, Marking, OrganMarkings, clamp_color};
use crate::random::GameRandom;

// Цвета.

/// Оттенок, насыщенность, яркость от 0 до 1 (`Color.ToHsv`).
pub fn to_hsv(color: Color) -> [f32; 3] {
    let (r, g, b) = (color.r, color.g, color.b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let c = max - min;
    let mut h = 0.0;
    if c != 0.0 {
        if max == r {
            h = ((g - b) / c) % 6.0;
            if h < 0.0 {
                h += 6.0;
            }
        } else if max == g {
            h = (b - r) / c + 2.0;
        } else {
            h = (r - g) / c + 4.0;
        }
    }
    let saturation = if max != 0.0 { c / max } else { 0.0 };
    [h * 60.0 / 360.0, saturation, max]
}

fn from_chroma(hue: f32, c: f32, m: f32, alpha: f32) -> Color {
    let hue = (hue - hue.trunc()) * 360.0;
    let h = hue / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h {
        h if (0.0..1.0).contains(&h) => (c, x, 0.0),
        h if (1.0..2.0).contains(&h) => (x, c, 0.0),
        h if (2.0..3.0).contains(&h) => (0.0, c, x),
        h if (3.0..4.0).contains(&h) => (0.0, x, c),
        h if (4.0..5.0).contains(&h) => (x, 0.0, c),
        h if (5.0..6.0).contains(&h) => (c, 0.0, x),
        _ => (0.0, 0.0, 0.0),
    };
    Color::rgba(r + m, g + m, b + m, alpha)
}

/// Цвет из HSV (`Color.FromHsv`).
pub fn from_hsv([h, s, v]: [f32; 3], alpha: f32) -> Color {
    let c = v * s;
    from_chroma(h, c, v - c, alpha)
}

/// Оттенок, насыщенность, светлота (`Color.ToHsl`).
pub fn to_hsl(color: Color) -> [f32; 3] {
    let (r, g, b) = (color.r, color.g, color.b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let c = max - min;
    let mut h = 0.0;
    if c != 0.0 {
        if max == r {
            h = (g - b) / c;
        } else if max == g {
            h = (b - r) / c + 2.0;
        } else {
            h = (r - g) / c + 4.0;
        }
    }
    let mut hue = h / 6.0;
    if hue < 0.0 {
        hue += 1.0;
    }
    let lightness = (max + min) / 2.0;
    let saturation = if lightness != 0.0 && lightness != 1.0 {
        c / (1.0 - (2.0 * lightness - 1.0).abs())
    } else {
        0.0
    };
    [hue, saturation, lightness]
}

/// Цвет из HSL (`Color.FromHsl`).
pub fn from_hsl([h, s, l]: [f32; 3], alpha: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    from_chroma(h, c, l - c / 2.0, alpha)
}

/// Цвет и два дополнительных по кругу оттенков (`GetComplementaryColors`).
pub fn complementary(color: Color, delta: f32) -> [Color; 3] {
    let [h, s, l] = to_hsl(color);
    let positive = h + delta;
    let positive = positive - positive.floor();
    let mut negative = h - delta;
    if negative < 0.0 {
        negative += 1.0;
    }
    [
        color,
        from_hsl([positive, s, l], color.a),
        from_hsl([negative, s, l], color.a),
    ]
}

// Окраска кожи.

const EPSILON_HUE: f32 = 0.00277;
const EPSILON: f32 = 0.003_921_568_6;

fn hue_in_range(hue: f32, min: f32, max: f32) -> bool {
    if min > max {
        hue >= min - EPSILON_HUE || hue <= max + EPSILON_HUE
    } else {
        hue >= min - EPSILON_HUE && hue <= max + EPSILON_HUE
    }
}

fn clamp_hue(hue: f32, min: f32, max: f32) -> f32 {
    if min > max {
        if hue >= min || hue <= max {
            return hue;
        }
        if hue > (max + min) / 2.0 { min } else { max }
    } else {
        hue.clamp(min, max)
    }
}

fn in_range(value: f32, (min, max): (f32, f32)) -> bool {
    value >= min - EPSILON && value <= max + EPSILON
}

/// Узел оттенка (`HueNodeClampedHsvColorationNode`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HueNode {
    pub hue: f32,
    pub saturation: (f32, f32),
    pub value: (f32, f32),
}

/// Стратегия окраски кожи (`ISkinColorationStrategy`).
#[derive(Debug, Clone, PartialEq)]
pub enum SkinStrategy {
    /// Человеческий тон: одно число от 0 до 100 (`HumanTonedSkinColoration`).
    HumanToned,
    ClampedHsv {
        hue: Option<(f32, f32)>,
        saturation: Option<(f32, f32)>,
        value: Option<(f32, f32)>,
    },
    ClampedHsl {
        hue: Option<(f32, f32)>,
        saturation: Option<(f32, f32)>,
        lightness: Option<(f32, f32)>,
    },
    HueNodes(Vec<HueNode>),
}

/// Тон человеческой кожи (`HumanTonedSkinColoration.FromUnary`).
pub fn human_tone(tone: f32) -> Color {
    crate::body::human_skin_tone(tone)
}

fn human_to_unary(color: Color) -> f32 {
    let [h, s, v] = to_hsv(color);
    if h.clamp(25.0 / 360.0, 1.0) > 25.0 / 360.0 && v == 1.0 {
        (45.0 - h * 360.0).abs()
    } else {
        s * 100.0
    }
}

const HSV_TOLERANCE: f32 = 0.019;

impl SkinStrategy {
    /// Ввод одним числом, а не цветом (`InputType == Unary`): только человеческий тон.
    pub fn is_unary(&self) -> bool {
        matches!(self, Self::HumanToned)
    }

    /// Число ползунка по цвету (`ToUnary`); у цветовых стратегий — 0.
    pub fn to_unary(&self, color: Color) -> f32 {
        match self {
            Self::HumanToned => human_to_unary(color),
            _ => 0.0,
        }
    }

    /// Цвет по числу ползунка (`FromUnary`); у цветовых стратегий — белый.
    pub fn from_unary(&self, value: f32) -> Color {
        match self {
            Self::HumanToned => human_tone(value),
            _ => Color::WHITE,
        }
    }

    /// Цвет кожи допустим (`VerifySkinColor`).
    pub fn verify(&self, color: Color) -> bool {
        match self {
            Self::HumanToned => {
                let [h, s, v] = to_hsv(color);
                let (h, s, v) = (
                    (h * 360.0).round(),
                    (s * 100.0).round(),
                    (v * 100.0).round(),
                );
                (25.0..=45.0).contains(&h) && s >= 20.0 && v >= 20.0
            }
            Self::ClampedHsv {
                hue,
                saturation,
                value,
            } => {
                let [h, s, v] = to_hsv(color);
                hue.is_none_or(|(a, b)| hue_in_range(h, a, b))
                    && saturation.is_none_or(|r| in_range(s, r))
                    && value.is_none_or(|r| in_range(v, r))
            }
            Self::ClampedHsl {
                hue,
                saturation,
                lightness,
            } => {
                let [h, s, l] = to_hsl(color);
                hue.is_none_or(|(a, b)| hue_in_range(h, a, b))
                    && saturation.is_none_or(|r| in_range(s, r))
                    && lightness.is_none_or(|r| in_range(l, r))
            }
            Self::HueNodes(nodes) => {
                let (Some(first), Some(last)) = (nodes.first(), nodes.last()) else {
                    return false;
                };
                let [h, s, v] = to_hsv(color);
                let hue = clamp_hue(h, first.hue, last.hue);
                let Some(range) = node_for_hue(nodes, hue) else {
                    return false;
                };
                s >= range.saturation.0 - HSV_TOLERANCE
                    && s <= range.saturation.1 + HSV_TOLERANCE
                    && v >= range.value.0 - HSV_TOLERANCE
                    && v <= range.value.1 + HSV_TOLERANCE
            }
        }
    }

    /// Ближайший допустимый цвет (`ClosestSkinColor`).
    pub fn closest(&self, color: Color) -> Color {
        match self {
            Self::HumanToned => {
                if self.verify(color) {
                    color
                } else {
                    human_tone(human_to_unary(color))
                }
            }
            Self::ClampedHsv {
                hue,
                saturation,
                value,
            } => {
                let old = to_hsv(color);
                let mut hsv = old;
                if let Some((a, b)) = hue {
                    hsv[0] = clamp_hue(hsv[0], *a, *b);
                }
                if let Some((a, b)) = saturation {
                    hsv[1] = hsv[1].clamp(*a, *b);
                }
                if let Some((a, b)) = value {
                    hsv[2] = hsv[2].clamp(*a, *b);
                }
                if hsv == old {
                    color
                } else {
                    from_hsv(hsv, color.a)
                }
            }
            Self::ClampedHsl {
                hue,
                saturation,
                lightness,
            } => {
                let old = to_hsl(color);
                let mut hsl = old;
                if let Some((a, b)) = hue {
                    hsl[0] = clamp_hue(hsl[0], *a, *b);
                }
                if let Some((a, b)) = saturation {
                    hsl[1] = hsl[1].clamp(*a, *b);
                }
                if let Some((a, b)) = lightness {
                    hsl[2] = hsl[2].clamp(*a, *b);
                }
                if hsl == old {
                    color
                } else {
                    from_hsl(hsl, color.a)
                }
            }
            Self::HueNodes(nodes) => {
                let (Some(first), Some(last)) = (nodes.first(), nodes.last()) else {
                    return color;
                };
                let old = to_hsv(color);
                let mut hsv = old;
                hsv[0] = clamp_hue(hsv[0], first.hue, last.hue);
                let Some(range) = node_for_hue(nodes, hsv[0]) else {
                    return color;
                };
                hsv[1] = hsv[1].clamp(range.saturation.0, range.saturation.1);
                hsv[2] = hsv[2].clamp(range.value.0, range.value.1);
                if hsv == old {
                    color
                } else {
                    from_hsv(hsv, color.a)
                }
            }
        }
    }

    /// Допустимый цвет: проверка с поправкой на округление до байтов (`EnsureVerified`).
    pub fn ensure_verified(&self, color: Color) -> Color {
        for i in 0..8 {
            let mut test = color;
            if i & 1 != 0 {
                test.r = (color.r + EPSILON).min(1.0);
            }
            if i & 2 != 0 {
                test.g = (color.g + EPSILON).min(1.0);
            }
            if i & 4 != 0 {
                test.b = (color.b + EPSILON).min(1.0);
            }
            if self.verify(test) {
                return color;
            }
        }
        self.closest(color)
    }
}

/// Узел для оттенка между соседними узлами (`GetNodeValuesForHue`).
fn node_for_hue(nodes: &[HueNode], hue: f32) -> Option<HueNode> {
    let (prev, next) = match nodes.len() {
        0 => return None,
        1 => (nodes[0], nodes[0]),
        _ => (0..nodes.len()).find_map(|i| {
            let current = nodes[i];
            let next = nodes.get(i + 1).copied().unwrap_or(nodes[0]);
            hue_in_range(hue, current.hue, next.hue).then_some((current, next))
        })?,
    };
    let weight = if (prev.hue - next.hue).abs() < 1e-6 {
        0.0
    } else {
        (hue - prev.hue) / (next.hue - prev.hue)
    };
    let lerp = |a: f32, b: f32| a + (b - a) * weight;
    Some(HueNode {
        hue,
        saturation: (
            lerp(prev.saturation.0, next.saturation.0),
            lerp(prev.saturation.1, next.saturation.1),
        ),
        value: (
            lerp(prev.value.0, next.value.0),
            lerp(prev.value.1, next.value.1),
        ),
    })
}

/// Окраска кожи вида (`SkinColorationPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct SkinColoration {
    pub strategy: SkinStrategy,
    pub realistic_colors: bool,
    pub squash_eye_hair_colors: bool,
}

fn number(node: &DataNode, key: &str) -> Option<f32> {
    node.get(key)?.as_str()?.trim().parse().ok()
}

fn flag(node: &DataNode, key: &str) -> bool {
    node.get(key)
        .and_then(DataNode::as_str)
        .is_some_and(|t| t.trim() == "true")
}

fn pair(node: &DataNode, key: &str) -> Option<(f32, f32)> {
    let seq = node.get(key)?.as_seq()?;
    Some((
        seq.first()?.as_str()?.trim().parse().ok()?,
        seq.get(1)?.as_str()?.trim().parse().ok()?,
    ))
}

fn parse_strategy(node: &DataNode) -> Option<SkinStrategy> {
    let tag = node.tag.as_deref()?;
    let name = tag.rsplit(':').next().unwrap_or(tag);
    Some(match name {
        "HumanTonedSkinColoration" => SkinStrategy::HumanToned,
        "ClampedHsvColoration" => SkinStrategy::ClampedHsv {
            hue: pair(node, "hue"),
            saturation: pair(node, "saturation"),
            value: pair(node, "value"),
        },
        "ClampedHslColoration" => SkinStrategy::ClampedHsl {
            hue: pair(node, "hue"),
            saturation: pair(node, "saturation"),
            lightness: pair(node, "lightness"),
        },
        "HueNodeClampedHsvColoration" => SkinStrategy::HueNodes(
            node.get("nodes")?
                .as_seq()?
                .iter()
                .map(|n| HueNode {
                    hue: number(n, "hue").unwrap_or(0.0),
                    saturation: pair(n, "saturation").unwrap_or((0.0, 1.0)),
                    value: pair(n, "value").unwrap_or((0.0, 1.0)),
                })
                .collect(),
        ),
        _ => return None,
    })
}

/// Окраска кожи по прототипу `skinColoration`.
pub fn skin_coloration(prototypes: &Prototypes, id: &str) -> Option<SkinColoration> {
    let node = prototypes.resolved_node("skinColoration", id)?;
    Some(SkinColoration {
        strategy: parse_strategy(node.get("strategy")?)?,
        realistic_colors: flag(node, "realisticColors"),
        squash_eye_hair_colors: flag(node, "squashEyeHairColors"),
    })
}

// Прототипы меток.

/// Слой без префикса `enum.HumanoidVisualLayers.`.
pub fn layer_name(text: &str) -> String {
    text.rsplit('.').next().unwrap_or(text).to_owned()
}

/// Ключ слоя спрайта тела (`HumanoidVisualLayers`).
pub fn layer_key(layer: &str) -> String {
    format!("enum.HumanoidVisualLayers.{layer}")
}

/// Источник цвета слоя метки (`LayerColoringType`).
#[derive(Debug, Clone, PartialEq)]
pub enum ColoringKind {
    Simple(Color),
    Skin,
    Eye,
    /// Цвет первой метки того же слоя (`CategoryColoring`).
    Category,
    /// Кожа темнее (`TattooColoring`).
    Tattoo,
}

/// Тип окраски с отрицанием.
#[derive(Debug, Clone, PartialEq)]
pub struct ColoringType {
    pub kind: ColoringKind,
    pub negative: bool,
}

impl ColoringType {
    fn color(&self, skin: Option<Color>, eyes: Option<Color>, others: &[Marking]) -> Option<Color> {
        let color = match &self.kind {
            ColoringKind::Simple(color) => Some(*color),
            ColoringKind::Skin => skin,
            ColoringKind::Eye => eyes,
            ColoringKind::Category => others
                .first()
                .and_then(|m| m.marking_colors.first().copied()),
            ColoringKind::Tattoo => skin.map(|s| {
                let [h, sat, _] = to_hsv(s);
                from_hsv([h, sat, 0.40], s.a)
            }),
        }?;
        Some(if self.negative {
            Color::rgba(1.0 - color.r, 1.0 - color.g, 1.0 - color.b, color.a)
        } else {
            color
        })
    }
}

/// Определение окраски слоя (`LayerColoringDefinition`).
#[derive(Debug, Clone, PartialEq)]
pub struct LayerColoring {
    pub kind: Option<ColoringType>,
    pub fallback: Vec<ColoringType>,
    pub fallback_color: Color,
}

impl Default for LayerColoring {
    fn default() -> Self {
        Self {
            kind: None,
            fallback: vec![ColoringType {
                kind: ColoringKind::Skin,
                negative: false,
            }],
            fallback_color: Color::WHITE,
        }
    }
}

impl LayerColoring {
    fn color(&self, skin: Option<Color>, eyes: Option<Color>, others: &[Marking]) -> Color {
        self.kind
            .as_ref()
            .and_then(|k| k.color(skin, eyes, others))
            .or_else(|| {
                self.fallback
                    .iter()
                    .find_map(|f| f.color(skin, eyes, others))
            })
            .unwrap_or(self.fallback_color)
    }
}

fn parse_coloring_type(node: &DataNode) -> Option<ColoringType> {
    let tag = node.tag.as_deref()?;
    let name = tag.rsplit(':').next().unwrap_or(tag);
    let kind = match name {
        "SimpleColoring" => ColoringKind::Simple(
            node.get("color")
                .and_then(DataNode::as_str)
                .and_then(Color::parse)
                .unwrap_or(Color::WHITE),
        ),
        "SkinColoring" => ColoringKind::Skin,
        "EyeColoring" => ColoringKind::Eye,
        "CategoryColoring" => ColoringKind::Category,
        "TattooColoring" => ColoringKind::Tattoo,
        _ => return None,
    };
    Some(ColoringType {
        kind,
        negative: flag(node, "negative"),
    })
}

fn parse_layer_coloring(node: &DataNode) -> LayerColoring {
    let mut coloring = LayerColoring {
        kind: node.get("type").and_then(parse_coloring_type),
        ..Default::default()
    };
    if let Some(fallbacks) = node.get("fallbackTypes").and_then(DataNode::as_seq) {
        coloring.fallback = fallbacks.iter().filter_map(parse_coloring_type).collect();
    }
    if let Some(color) = node
        .get("fallbackColor")
        .and_then(DataNode::as_str)
        .and_then(Color::parse)
    {
        coloring.fallback_color = color;
    }
    coloring
}

/// Слой метки (`MarkingLayerData`).
#[derive(Debug, Clone, PartialEq)]
pub struct MarkingLayer {
    pub sprite: String,
    pub state: String,
    pub coloring: Option<LayerColoring>,
    pub forced_coloring: bool,
    pub shader: Option<String>,
}

/// Прототип метки (`MarkingPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct MarkingProto {
    pub id: String,
    pub body_part: String,
    pub group_whitelist: Option<Vec<String>>,
    pub sex_restriction: Option<Sex>,
    pub strategy: Option<SkinStrategy>,
    pub forced_coloring: bool,
    pub default_coloring: LayerColoring,
    /// Устаревшая окраска по состояниям (`Coloring.Layers`).
    pub layer_colorings: BTreeMap<String, LayerColoring>,
    pub sprites: Vec<MarkingLayer>,
    pub random_weight: f32,
}

impl MarkingProto {
    fn has_forced_color_layer(&self) -> bool {
        self.sprites.iter().any(|s| s.forced_coloring)
    }

    /// Метка без цветов, белая по числу слоёв (`AsMarking`).
    pub fn as_marking(&self) -> Marking {
        Marking {
            marking_id: self.id.clone(),
            marking_colors: vec![Color::WHITE; self.sprites.len()],
        }
    }

    /// Цвета слоёв по окраске (`GetMarkingLayerColors`).
    pub fn layer_colors(
        &self,
        skin: Option<Color>,
        eyes: Option<Color>,
        others: &[Marking],
    ) -> Vec<Color> {
        let default = self.default_coloring.color(skin, eyes, others);
        self.sprites
            .iter()
            .map(|layer| {
                if let Some(coloring) = &layer.coloring {
                    coloring.color(skin, eyes, others)
                } else if let Some(coloring) = self.layer_colorings.get(&layer.state) {
                    coloring.color(skin, eyes, others)
                } else {
                    default
                }
            })
            .collect()
    }
}

/// Метка по прототипу `marking`.
pub fn marking_proto(prototypes: &Prototypes, id: &str) -> Option<MarkingProto> {
    let node = prototypes.resolved_node("marking", id)?;
    if flag(node, "abstract") {
        return None;
    }
    let coloring = node.get("coloring");
    let sprites = node
        .get("sprites")?
        .as_seq()?
        .iter()
        .filter_map(|s| {
            Some(MarkingLayer {
                sprite: s.get("sprite")?.as_str()?.to_owned(),
                state: s.get("state")?.as_str()?.to_owned(),
                coloring: s.get("coloring").map(parse_layer_coloring),
                forced_coloring: flag(s, "forcedColoring"),
                shader: s
                    .get("shader")
                    .and_then(DataNode::as_str)
                    .map(str::to_owned),
            })
        })
        .collect();
    Some(MarkingProto {
        id: id.to_owned(),
        body_part: layer_name(node.get("bodyPart")?.as_str()?),
        group_whitelist: node
            .get("groupWhitelist")
            .and_then(DataNode::as_seq)
            .map(|s| {
                s.iter()
                    .filter_map(|n| n.as_str().map(str::to_owned))
                    .collect()
            }),
        sex_restriction: node
            .get("sexRestriction")
            .and_then(DataNode::as_str)
            .and_then(crate::profiles::parse_sex),
        strategy: node.get("strategy").and_then(parse_strategy),
        forced_coloring: flag(node, "forcedColoring"),
        default_coloring: coloring
            .and_then(|c| c.get("default"))
            .map(parse_layer_coloring)
            .unwrap_or_default(),
        layer_colorings: coloring
            .and_then(|c| c.get("layers"))
            .and_then(DataNode::as_map)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.to_owned(), parse_layer_coloring(v)))
                    .collect()
            })
            .unwrap_or_default(),
        sprites,
        random_weight: number(node, "randomWeight").unwrap_or(1.0),
    })
}

/// Предел меток слоя (`MarkingsLimits`).
#[derive(Debug, Clone, PartialEq)]
pub struct MarkingsLimits {
    pub limit: i32,
    pub required: bool,
    pub only_group_whitelisted: Option<bool>,
    pub default: Vec<String>,
    pub nudity_default: Vec<String>,
    pub weight: f32,
}

/// Вид меток слоя (`MarkingsAppearance`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarkingsAppearance {
    pub layer_alpha: f32,
    pub match_skin: bool,
}

/// Группа меток (`MarkingsGroupPrototype`).
#[derive(Debug, Clone, PartialEq)]
pub struct MarkingsGroup {
    pub id: String,
    pub only_group_whitelisted: bool,
    pub limits: BTreeMap<String, MarkingsLimits>,
    pub appearances: BTreeMap<String, MarkingsAppearance>,
}

fn names(node: Option<&DataNode>) -> Vec<String> {
    node.and_then(DataNode::as_seq)
        .map(|s| {
            s.iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Группа по прототипу `markingsGroup`; пределы и вид слоёв складываются с родительскими
/// (`[AlwaysPushInheritance]`).
pub fn markings_group(prototypes: &Prototypes, id: &str) -> Option<MarkingsGroup> {
    let node = prototypes.resolved_node("markingsGroup", id)?;
    let mut group = MarkingsGroup {
        id: id.to_owned(),
        only_group_whitelisted: false,
        limits: BTreeMap::new(),
        appearances: BTreeMap::new(),
    };
    // Родители по порядку, затем своё поверх (поля ребёнка заменяют родительские целиком при
    // разрешении прототипа, поэтому складываются здесь).
    for parent in prototypes.parents("markingsGroup", id) {
        if let Some(parent) = markings_group(prototypes, parent) {
            group.only_group_whitelisted = parent.only_group_whitelisted;
            group.limits.extend(parent.limits);
            group.appearances.extend(parent.appearances);
        }
    }
    if node.get("onlyGroupWhitelisted").is_some() {
        group.only_group_whitelisted = flag(node, "onlyGroupWhitelisted");
    }
    if let Some(limits) = node.get("limits").and_then(DataNode::as_map) {
        for (layer, limit) in limits.iter() {
            group.limits.insert(
                layer_name(layer),
                MarkingsLimits {
                    limit: number(limit, "limit").unwrap_or(0.0) as i32,
                    required: flag(limit, "required"),
                    only_group_whitelisted: limit
                        .get("onlyGroupWhitelisted")
                        .map(|_| flag(limit, "onlyGroupWhitelisted")),
                    default: names(limit.get("default")),
                    nudity_default: names(limit.get("nudityDefault")),
                    weight: number(limit, "weight").unwrap_or(0.6),
                },
            );
        }
    }
    if let Some(appearances) = node.get("appearances").and_then(DataNode::as_map) {
        for (layer, appearance) in appearances.iter() {
            group.appearances.insert(
                layer_name(layer),
                MarkingsAppearance {
                    layer_alpha: number(appearance, "layerAlpha").unwrap_or(1.0),
                    match_skin: flag(appearance, "matchSkin"),
                },
            );
        }
    }
    Some(group)
}

/// Подходит ли метка группе и полу (`CanBeApplied`).
pub fn can_be_applied(group: &MarkingsGroup, sex: Sex, marking: &MarkingProto) -> bool {
    let whitelisted = group
        .limits
        .get(&marking.body_part)
        .and_then(|l| l.only_group_whitelisted)
        .unwrap_or(group.only_group_whitelisted);
    match &marking.group_whitelist {
        None if whitelisted => return false,
        Some(list) if !list.contains(&group.id) => return false,
        _ => {}
    }
    marking.sex_restriction.is_none_or(|s| s == sex)
}

/// Все метки слоя, годные группе и полу (`MarkingsByLayerAndGroupAndSex`), по порядку.
pub fn markings_for(
    prototypes: &Prototypes,
    layer: &str,
    group: &MarkingsGroup,
    sex: Sex,
) -> Vec<MarkingProto> {
    let mut ids: Vec<&str> = prototypes.ids("marking").collect();
    ids.sort_unstable();
    ids.into_iter()
        .filter_map(|id| marking_proto(prototypes, id))
        .filter(|m| m.body_part == layer && can_be_applied(group, sex, m))
        .collect()
}

// Органы с метками.

/// Слои и группа органа (`OrganMarkingData`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OrganMarkingData {
    #[serde(default)]
    pub layers: BTreeSet<String>,
    #[serde(default)]
    pub group: String,
}

/// Орган с метками (`VisualOrganMarkingsComponent`). В данных — `VisualOrganMarkings`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualOrganMarkings {
    pub marking_data: OrganMarkingData,
    /// Слой → метки на органе.
    #[serde(default)]
    pub markings: BTreeMap<String, Vec<Marking>>,
    #[serde(default)]
    pub hideable_layers: BTreeSet<String>,
}

/// Органы вида: категория → прототип органа (`MarkingManager.GetOrgans`).
pub fn species_organs(prototypes: &Prototypes, species: &str) -> BTreeMap<String, String> {
    let Some(doll) = prototypes
        .resolved_node("species", species)
        .and_then(|n| n.get("dollPrototype"))
        .and_then(DataNode::as_str)
    else {
        return BTreeMap::new();
    };
    prototypes
        .entity(doll)
        .and_then(|e| e.component("InitialBody").map(|c| c.node.clone()))
        .and_then(|node| {
            node.get("organs").and_then(DataNode::as_map).map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.to_owned(), v.as_str()?.to_owned())))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Метки органа по его прототипу (`TryGetMarkingData`).
pub fn organ_marking_data(prototypes: &Prototypes, organ: &str) -> Option<OrganMarkingData> {
    let component = prototypes.entity(organ)?.component("VisualOrganMarkings")?;
    let data = component.node.get("markingData")?;
    Some(OrganMarkingData {
        layers: names(data.get("layers"))
            .into_iter()
            .map(|l| layer_name(&l))
            .collect(),
        group: data.get("group")?.as_str()?.to_owned(),
    })
}

/// Метки органов вида (`GetMarkingData`).
pub fn species_marking_data(
    prototypes: &Prototypes,
    species: &str,
) -> BTreeMap<String, OrganMarkingData> {
    species_organs(prototypes, species)
        .into_iter()
        .filter_map(|(category, organ)| Some((category, organ_marking_data(prototypes, &organ)?)))
        .collect()
}

// Проверка внешности.

/// Цвета метки по числу слоёв и стратегии метки (`EnsureValidColors`).
fn ensure_valid_colors(marking: &mut Marking, proto: &MarkingProto) {
    let count = proto.sprites.len();
    if marking.marking_colors.len() < count {
        let last = marking
            .marking_colors
            .last()
            .copied()
            .unwrap_or(Color::WHITE);
        marking.marking_colors.resize(count, last);
    } else {
        marking.marking_colors.truncate(count);
    }
    if let Some(strategy) = &proto.strategy {
        for color in &mut marking.marking_colors {
            *color = strategy.ensure_verified(*color);
        }
    }
}

/// Привести внешность к виду и полу (`HumanoidCharacterAppearance.EnsureValid`).
pub fn ensure_valid_appearance(
    prototypes: &Prototypes,
    appearance: &HumanoidCharacterAppearance,
    species: &str,
    sex: Sex,
) -> HumanoidCharacterAppearance {
    let mut skin = appearance.skin_color;
    let eyes = clamp_color(appearance.eye_color);
    let mut validated = appearance.markings.clone();
    if let Some(info) = crate::profiles::species_info(prototypes, species) {
        if let Some(coloration) = skin_coloration(prototypes, &info.skin_coloration) {
            skin = coloration.strategy.ensure_verified(skin);
        }
        let organs = species_organs(prototypes, species);
        validated.retain(|organ, _| organs.contains_key(organ));
        for (category, organ) in &organs {
            let Some(data) = organ_marking_data(prototypes, organ) else {
                validated.remove(category);
                continue;
            };
            let Some(group) = markings_group(prototypes, &data.group) else {
                validated.remove(category);
                continue;
            };
            let mut sets = appearance
                .markings
                .get(category)
                .cloned()
                .unwrap_or_default();
            ensure_valid_layers(prototypes, &mut sets, &group, sex, &data);
            ensure_valid_limits(
                prototypes,
                &mut sets,
                &group,
                &data.layers,
                Some(skin),
                Some(eyes),
            );
            validated.insert(category.clone(), sets);
        }
    }
    HumanoidCharacterAppearance {
        eye_color: eyes,
        skin_color: skin,
        markings: validated,
    }
}

/// Неизвестные метки, чужие группе или полу, не на своих слоях — прочь; цвета по числу слоёв
/// (`EnsureValidColors`, `EnsureValidGroupAndSex`, `EnsureValidLayers`).
fn ensure_valid_layers(
    prototypes: &Prototypes,
    sets: &mut BTreeMap<String, Vec<Marking>>,
    group: &MarkingsGroup,
    sex: Sex,
    data: &OrganMarkingData,
) {
    for markings in sets.values_mut() {
        markings.retain_mut(|marking| {
            let Some(proto) = marking_proto(prototypes, &marking.marking_id) else {
                return false;
            };
            ensure_valid_colors(marking, &proto);
            can_be_applied(group, sex, &proto) && data.layers.contains(&proto.body_part)
        });
    }
    sets.retain(|_, markings| !markings.is_empty());
}

/// Не больше предела на слой; обязательные слои получают метки по умолчанию
/// (`EnsureValidLimits`).
fn ensure_valid_limits(
    prototypes: &Prototypes,
    sets: &mut BTreeMap<String, Vec<Marking>>,
    group: &MarkingsGroup,
    layers: &BTreeSet<String>,
    skin: Option<Color>,
    eyes: Option<Color>,
) {
    let mut counts: BTreeMap<String, i32> = BTreeMap::new();
    for markings in sets.values_mut() {
        // С конца, как в SS14: при переполнении остаются последние.
        let mut keep = vec![true; markings.len()];
        let mut parts = Vec::with_capacity(markings.len());
        for marking in markings.iter() {
            parts.push(marking_proto(prototypes, &marking.marking_id).map(|p| p.body_part));
        }
        for i in (0..markings.len()).rev() {
            let Some(part) = &parts[i] else {
                keep[i] = false;
                continue;
            };
            let Some(limit) = group.limits.get(part) else {
                continue;
            };
            let count = counts.get(part).copied().unwrap_or(0);
            if count >= limit.limit {
                keep[i] = false;
                continue;
            }
            counts.insert(part.clone(), count + 1);
        }
        let mut index = 0;
        markings.retain(|_| {
            let kept = keep[index];
            index += 1;
            kept
        });
    }
    for layer in layers {
        let Some(limit) = group.limits.get(layer) else {
            continue;
        };
        if counts.get(layer).copied().unwrap_or(0) > 0 || !limit.required {
            continue;
        }
        for id in &limit.default {
            let Some(proto) = marking_proto(prototypes, id) else {
                continue;
            };
            let set = sets.entry(layer.clone()).or_default();
            let colors = proto.layer_colors(skin, eyes, set);
            set.push(Marking {
                marking_id: id.clone(),
                marking_colors: colors,
            });
        }
    }
}

// Случайная внешность.

const REALISTIC_EYE_COLORS: [&str; 5] = ["#A52A2A", "#808080", "#F0FFFF", "#4682B4", "#000000"];
const REALISTIC_HAIR_COLORS: [&str; 6] = [
    "#FFFF00", "#000000", "#F4A460", "#A52A2A", "#F5DEB3", "#808080",
];

/// Палитра персонажа (`CharacterPalette`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterPalette {
    pub skin: Color,
    pub hair: Color,
    pub eyes: Color,
}

fn pick_color(random: &mut GameRandom, colors: &[&str]) -> Color {
    let index = random.range(0, colors.len() as i32) as usize;
    Color::parse(colors[index]).unwrap_or(Color::BLACK)
}

/// Случайная палитра под окраску вида (`GetRandomClampedPalette`).
pub fn random_palette(random: &mut GameRandom, coloration: &SkinColoration) -> CharacterPalette {
    let base = Color::rgba(random.next_f32(), random.next_f32(), random.next_f32(), 1.0);
    let delta = match random.range(0, 4) {
        0 => 150.0 / 360.0,
        1 => 120.0 / 360.0,
        2 => 30.0 / 360.0,
        _ => 180.0 / 360.0,
    };
    let [skin, mut hair, mut eyes] = complementary(base, delta);
    if coloration.realistic_colors {
        let picked = pick_color(random, &REALISTIC_HAIR_COLORS);
        let jitter = |c: f32, r: &mut GameRandom| (c + r.next_f32() * 0.5 - 0.25).clamp(0.0, 1.0);
        hair = Color::rgba(
            jitter(picked.r, random),
            jitter(picked.g, random),
            jitter(picked.b, random),
            picked.a,
        );
        let realistic: Vec<Color> = REALISTIC_EYE_COLORS
            .iter()
            .filter_map(|c| Color::parse(c))
            .collect();
        if !realistic.contains(&eyes) {
            eyes = pick_color(random, &REALISTIC_EYE_COLORS);
        }
    }
    if coloration.squash_eye_hair_colors {
        hair = coloration.strategy.closest(hair);
        eyes = coloration.strategy.ensure_verified(eyes);
    }
    CharacterPalette {
        skin: coloration.strategy.ensure_verified(skin),
        hair,
        eyes,
    }
}

/// Случайная метка по весам (`PickWeightedMarkingId`).
fn pick_weighted<'a>(
    random: &mut GameRandom,
    pool: &'a [MarkingProto],
) -> Option<&'a MarkingProto> {
    let weights: Vec<f32> = pool.iter().map(|m| m.random_weight).collect();
    random.pick_weighted(&weights).map(|i| &pool[i])
}

/// Случайные метки слоя (`PickLayerRandomMarkings`, `PickHairsRandomMarking`).
fn random_layer_markings(
    random: &mut GameRandom,
    layer: &str,
    limits: &MarkingsLimits,
    all: &[MarkingProto],
    palette: &CharacterPalette,
) -> Vec<Marking> {
    if layer == "Hair" || layer == "FacialHair" {
        if all.is_empty() || !random.prob(limits.weight) {
            return Vec::new();
        }
        return pick_weighted(random, all)
            .map(|hair| Marking {
                marking_id: hair.id.clone(),
                marking_colors: vec![palette.hair; hair.sprites.len()],
            })
            .into_iter()
            .collect();
    }
    let mut pool: Vec<MarkingProto> = all.to_vec();
    let mut out: Vec<Marking> = Vec::new();
    for _ in 0..limits.limit {
        if pool.is_empty() {
            break;
        }
        if !random.prob(limits.weight) {
            continue;
        }
        let Some(picked) = pick_weighted(random, &pool).cloned() else {
            continue;
        };
        pool.retain(|m| m.id != picked.id);
        let mut colors = Vec::new();
        for layer in &picked.sprites {
            let coloring = picked
                .layer_colorings
                .get(&layer.state)
                .unwrap_or(&picked.default_coloring);
            let color = if coloring.kind.is_some() {
                coloring.color(Some(palette.skin), Some(palette.eyes), &out)
            } else if random.prob(0.5) {
                palette.hair
            } else {
                palette.eyes
            };
            colors.push(color);
        }
        out.push(Marking {
            marking_id: picked.id.clone(),
            marking_colors: colors,
        });
    }
    out
}

/// Случайная внешность вида (`HumanoidCharacterAppearance.Random`).
pub fn random_appearance(
    prototypes: &Prototypes,
    random: &mut GameRandom,
    species: &str,
    sex: Sex,
) -> HumanoidCharacterAppearance {
    let coloration = crate::profiles::species_info(prototypes, species)
        .and_then(|s| skin_coloration(prototypes, &s.skin_coloration))
        .unwrap_or(SkinColoration {
            strategy: SkinStrategy::HumanToned,
            realistic_colors: true,
            squash_eye_hair_colors: false,
        });
    let palette = random_palette(random, &coloration);
    let mut markings = OrganMarkings::new();
    for (category, data) in species_marking_data(prototypes, species) {
        let Some(group) = markings_group(prototypes, &data.group) else {
            continue;
        };
        let mut layers = BTreeMap::new();
        for layer in &data.layers {
            let all = markings_for(prototypes, layer, &group, sex);
            if all.is_empty() {
                continue;
            }
            let Some(limits) = group.limits.get(layer).filter(|l| l.limit > 0) else {
                continue;
            };
            layers.insert(
                layer.clone(),
                random_layer_markings(random, layer, limits, &all, &palette),
            );
        }
        markings.insert(category, layers);
    }
    let appearance = HumanoidCharacterAppearance {
        eye_color: palette.eyes,
        skin_color: palette.skin,
        markings,
    };
    ensure_valid_appearance(prototypes, &appearance, species, sex)
}

// Метки на теле.

/// Принудительные цвета меток (`ResolveMarkings`).
fn resolve_markings(
    prototypes: &Prototypes,
    markings: &[Marking],
    skin: Option<Color>,
    eyes: Option<Color>,
    appearances: &BTreeMap<String, MarkingsAppearance>,
) -> Vec<Marking> {
    let mut resolved = Vec::new();
    let mut forced_colors = Vec::new();
    let mut forced_layers = Vec::new();
    for marking in markings {
        let Some(proto) = marking_proto(prototypes, &marking.marking_id) else {
            continue;
        };
        let match_skin = appearances
            .get(&proto.body_part)
            .is_some_and(|a| a.match_skin);
        if proto.forced_coloring || match_skin {
            forced_colors.push((marking.clone(), proto));
        } else if proto.has_forced_color_layer() {
            forced_layers.push((marking.clone(), proto));
        } else {
            resolved.push(marking.clone());
        }
    }
    for (marking, proto) in forced_colors {
        let mut colors = proto.layer_colors(skin, eyes, &resolved);
        if let Some(appearance) = appearances.get(&proto.body_part).filter(|a| a.match_skin)
            && let Some(skin) = skin
        {
            colors = vec![skin.with_alpha(appearance.layer_alpha); colors.len()];
        }
        resolved.push(Marking {
            marking_id: marking.marking_id,
            marking_colors: colors,
        });
    }
    for (marking, proto) in forced_layers {
        let colors = proto.layer_colors(skin, eyes, &resolved);
        let mut new = marking.marking_colors.clone();
        new.resize(proto.sprites.len(), Color::WHITE);
        for (i, layer) in proto.sprites.iter().enumerate() {
            if layer.forced_coloring {
                new[i] = colors[i];
            }
        }
        resolved.push(Marking {
            marking_id: marking.marking_id,
            marking_colors: new,
        });
    }
    resolved
}

/// Положить метки на органы тела (`ApplyOrganMarkingsEvent`, `OnMarkingsOrganApplyMarkings`).
pub fn apply_markings(world: &mut World, body: Entity, markings: &OrganMarkings) {
    use crate::body::{BodyWorldExt, Organ, VisualOrgan};
    for organ in world.organs(body) {
        let Some(category) = world.get::<Organ>(organ).and_then(|o| o.category.clone()) else {
            continue;
        };
        let Some(set) = markings.get(&category) else {
            continue;
        };
        let Some(component) = world.get::<VisualOrganMarkings>(organ).cloned() else {
            continue;
        };
        let (skin, eyes) = world.get::<VisualOrgan>(organ).map_or((None, None), |v| {
            (Some(v.profile.skin_color), Some(v.profile.eye_color))
        });
        let Some(prototypes) = world.get_resource::<Prototypes>() else {
            return;
        };
        let appearances = markings_group(prototypes, &component.marking_data.group)
            .map(|g| g.appearances)
            .unwrap_or_default();
        let mut organ_markings = component.markings.clone();
        for layer in &component.marking_data.layers {
            let Some(list) = set.get(layer) else {
                continue;
            };
            let known: Vec<Marking> = list
                .iter()
                .filter(|m| marking_proto(prototypes, &m.marking_id).is_some())
                .cloned()
                .collect();
            organ_markings.insert(layer.clone(), known);
        }
        let resolved: BTreeMap<String, Vec<Marking>> = organ_markings
            .into_iter()
            .map(|(layer, list)| {
                let resolved = resolve_markings(prototypes, &list, skin, eyes, &appearances);
                (layer, resolved)
            })
            .collect();
        if let Some(mut c) = world.get_mut::<VisualOrganMarkings>(organ) {
            c.markings = resolved;
        }
    }
}

pub fn install(world: &mut World) {
    use astrocore_ecs::RegistryWorldExt;
    use astrocore_net::NetRegistryWorldExt;
    world.register_data_component::<VisualOrganMarkings>();
    // Слои органа складываются с родительскими (`[AlwaysPushInheritance]` в `MarkingData`).
    world
        .resource_mut::<astrocore_ecs::ComponentRegistry>()
        .set_inheritance(
            "VisualOrganMarkings",
            &["markingData", "hideableLayers"],
            &[],
        );
    world.register_net_serde::<VisualOrganMarkings>();
}

#[cfg(test)]
mod tests;
