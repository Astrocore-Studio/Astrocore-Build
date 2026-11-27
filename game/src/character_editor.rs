//! Правка профиля в редакторе персонажа: смена вида и пола с поправкой голоса и меток, кожа
//! ползунком или цветом по окраске вида, выбор и снятие меток в пределах группы, цвета меток,
//! приоритеты должностей по отделам, антагонисты. Без окна — его рисует `character_ui`.
//! Образец — `HumanoidProfileEditor` (`Appearance`, `Roles`), `MarkingsViewModel`
//! (`TrySelectMarking`, `TryDeselectMarking`, `TrySetMarkingColor`, `ValidateMarkings`),
//! `DepartmentUIComparer`, `JobUIComparer` SS14 @ 2f4a2aad.

use std::collections::BTreeMap;

use astrocore_data::DataNode;
use astrocore_math::Color;
use astrocore_prototypes::Prototypes;

use crate::body::Sex;
use crate::markings::{
    MarkingProto, OrganMarkingData, SkinStrategy, ensure_valid_appearance, marking_proto,
    markings_for, markings_group, skin_coloration, species_marking_data,
};
use crate::profiles::{
    HumanoidCharacterProfile, JobPriority, clamp_color, gender_for_sex, sets_preference,
    species_info,
};

fn text(node: &DataNode, key: &str) -> Option<String> {
    node.get(key)?.as_str().map(str::to_owned)
}

fn flag(node: &DataNode, key: &str) -> bool {
    node.get(key)
        .and_then(DataNode::as_str)
        .is_some_and(|t| t.trim() == "true")
}

/// Стратегия окраски кожи вида.
pub fn skin_strategy(prototypes: &Prototypes, species: &str) -> Option<SkinStrategy> {
    let info = species_info(prototypes, species)?;
    Some(skin_coloration(prototypes, &info.skin_coloration)?.strategy)
}

/// Метки заново по виду и полу (`ValidateMarkings`).
fn revalidate(prototypes: &Prototypes, profile: &mut HumanoidCharacterProfile) {
    profile.appearance = ensure_valid_appearance(
        prototypes,
        &profile.appearance,
        &profile.species,
        profile.sex,
    );
}

/// Голос по полу, если нынешний виду не подходит (`UpdateVoiceControls`), или всегда.
fn default_voice(prototypes: &Prototypes, profile: &mut HumanoidCharacterProfile, force: bool) {
    let Some(info) = species_info(prototypes, &profile.species) else {
        return;
    };
    if force || !info.voices.contains(&profile.voice) {
        let index = match profile.sex {
            Sex::Male => 0,
            Sex::Female => 1,
            Sex::Unsexed => 2,
        };
        profile.voice = info.default_sounds_by_sex[index].clone();
    }
}

/// Сменить вид (`SetSpecies`): кожа — к окраске вида, пол — из доступных виду, голос и
/// метки — заново.
pub fn set_species(prototypes: &Prototypes, profile: &mut HumanoidCharacterProfile, species: &str) {
    let Some(info) = species_info(prototypes, species) else {
        return;
    };
    profile.species = species.to_owned();
    if let Some(strategy) = skin_strategy(prototypes, species) {
        let skin = profile.appearance.skin_color;
        profile.appearance.skin_color = clamp_color(if strategy.is_unary() {
            strategy.from_unary(strategy.to_unary(skin))
        } else {
            strategy.closest(skin)
        });
    }
    if !info.sexes.contains(&profile.sex) {
        profile.sex = info.sexes.first().copied().unwrap_or(Sex::Unsexed);
        profile.gender = gender_for_sex(profile.sex);
    }
    default_voice(prototypes, profile, false);
    revalidate(prototypes, profile);
}

/// Сменить пол (`SetSex`): род и голос — по полу, метки — по ограничениям пола.
pub fn set_sex(prototypes: &Prototypes, profile: &mut HumanoidCharacterProfile, sex: Sex) {
    profile.sex = sex;
    profile.gender = gender_for_sex(sex);
    default_voice(prototypes, profile, true);
    revalidate(prototypes, profile);
}

/// Положение ползунка кожи, если окраска вида задаётся числом.
pub fn skin_unary(prototypes: &Prototypes, profile: &HumanoidCharacterProfile) -> Option<f32> {
    let strategy = skin_strategy(prototypes, &profile.species)?;
    strategy
        .is_unary()
        .then(|| strategy.to_unary(profile.appearance.skin_color))
}

/// Кожа по ползунку (`FromUnary`) у числовой окраски, иначе ближайший допустимый цвет
/// (`ClosestSkinColor`).
pub fn set_skin(
    prototypes: &Prototypes,
    profile: &mut HumanoidCharacterProfile,
    unary: Option<f32>,
    color: Color,
) {
    let Some(strategy) = skin_strategy(prototypes, &profile.species) else {
        profile.appearance.skin_color = clamp_color(color);
        return;
    };
    let skin = match unary {
        Some(value) if strategy.is_unary() => strategy.from_unary(value),
        _ => strategy.closest(color),
    };
    profile.appearance.skin_color = clamp_color(skin);
}

/// Цвет глаз без прозрачности.
pub fn set_eye_color(profile: &mut HumanoidCharacterProfile, color: Color) {
    profile.appearance.eye_color = clamp_color(color);
}

// Метки.

/// Слой меток органа в редакторе: предел и выбор.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerChoice {
    pub organ: String,
    pub layer: String,
    pub group: String,
    /// Предел меток; `None` — без предела.
    pub limit: Option<i32>,
    pub required: bool,
    pub available: Vec<MarkingProto>,
}

/// Органы вида с метками (`GetMarkingData`).
pub fn organs(prototypes: &Prototypes, species: &str) -> BTreeMap<String, OrganMarkingData> {
    species_marking_data(prototypes, species)
}

/// Слои органа с доступными метками (`MarkingsByLayerAndGroupAndSex`) и пределами.
pub fn layer_choices(
    prototypes: &Prototypes,
    profile: &HumanoidCharacterProfile,
    organ: &str,
) -> Vec<LayerChoice> {
    let organs = organs(prototypes, &profile.species);
    let Some(data) = organs.get(organ) else {
        return Vec::new();
    };
    let Some(group) = markings_group(prototypes, &data.group) else {
        return Vec::new();
    };
    data.layers
        .iter()
        .map(|layer| {
            let limits = group.limits.get(layer);
            LayerChoice {
                organ: organ.to_owned(),
                layer: layer.clone(),
                group: group.id.clone(),
                limit: limits.map(|l| l.limit),
                required: limits.is_some_and(|l| l.required),
                available: markings_for(prototypes, layer, &group, profile.sex),
            }
        })
        .collect()
}

/// Метки слоя в профиле.
pub fn selected<'a>(
    profile: &'a HumanoidCharacterProfile,
    organ: &str,
    layer: &str,
) -> &'a [crate::profiles::Marking] {
    profile
        .appearance
        .markings
        .get(organ)
        .and_then(|layers| layers.get(layer))
        .map_or(&[], Vec::as_slice)
}

/// Прежние цвета снятых меток (`_previousColors`): вернутся при новом выборе.
pub type PreviousColors = BTreeMap<String, Vec<Color>>;

/// Выбрать метку (`TrySelectMarking`): цвета — прежние или по окраске метки; при пределе
/// одна — заменить, иначе добавить, пока есть место.
pub fn select_marking(
    prototypes: &Prototypes,
    profile: &mut HumanoidCharacterProfile,
    organ: &str,
    layer: &str,
    marking: &str,
    previous: &PreviousColors,
) -> bool {
    let Some(proto) = marking_proto(prototypes, marking) else {
        return false;
    };
    let Some(choice) = layer_choices(prototypes, profile, organ)
        .into_iter()
        .find(|c| c.layer == layer)
    else {
        return false;
    };
    if !choice.available.iter().any(|m| m.id == marking) {
        return false;
    }
    let skin = profile.appearance.skin_color;
    let eyes = profile.appearance.eye_color;
    let markings = profile
        .appearance
        .markings
        .entry(organ.to_owned())
        .or_default()
        .entry(layer.to_owned())
        .or_default();
    if markings.iter().any(|m| m.marking_id == marking) {
        return false;
    }
    let colors = previous
        .get(marking)
        .cloned()
        .unwrap_or_else(|| proto.layer_colors(Some(skin), Some(eyes), markings));
    let new = crate::profiles::Marking {
        marking_id: marking.to_owned(),
        marking_colors: colors,
    };
    match choice.limit {
        None => markings.push(new),
        Some(1) if markings.len() == 1 => {
            markings.clear();
            markings.push(new);
        }
        Some(limit) if (markings.len() as i32) < limit => markings.push(new),
        Some(_) => return false,
    }
    revalidate(prototypes, profile);
    true
}

/// Снять метку (`TryDeselectMarking`): обязательный слой пустым не остаётся; цвета
/// запоминаются.
pub fn deselect_marking(
    prototypes: &Prototypes,
    profile: &mut HumanoidCharacterProfile,
    organ: &str,
    layer: &str,
    marking: &str,
    previous: &mut PreviousColors,
) -> bool {
    let required = layer_choices(prototypes, profile, organ)
        .into_iter()
        .find(|c| c.layer == layer)
        .is_some_and(|c| c.required);
    let Some(markings) = profile
        .appearance
        .markings
        .get_mut(organ)
        .and_then(|layers| layers.get_mut(layer))
    else {
        return false;
    };
    let count = markings.iter().filter(|m| m.marking_id == marking).count();
    if count == 0 || (required && markings.len() - count == 0) {
        return false;
    }
    if let Some(removed) = markings.iter().find(|m| m.marking_id == marking) {
        previous.insert(marking.to_owned(), removed.marking_colors.clone());
    }
    markings.retain(|m| m.marking_id != marking);
    true
}

/// Можно ли менять цвета метки (`IsMarkingColorCustomizable`): не принудительная окраска и
/// слой не под цвет кожи.
pub fn color_customizable(
    prototypes: &Prototypes,
    profile: &HumanoidCharacterProfile,
    organ: &str,
    layer: &str,
    marking: &str,
) -> bool {
    let Some(proto) = marking_proto(prototypes, marking) else {
        return false;
    };
    if proto.forced_coloring {
        return false;
    }
    let organs = organs(prototypes, &profile.species);
    let Some(group) = organs
        .get(organ)
        .and_then(|data| markings_group(prototypes, &data.group))
    else {
        return false;
    };
    group.appearances.get(layer).is_none_or(|a| !a.match_skin)
}

/// Цвет слоя метки (`TrySetMarkingColor`): со стратегией метки — ближайший допустимый.
pub fn set_marking_color(
    prototypes: &Prototypes,
    profile: &mut HumanoidCharacterProfile,
    organ: &str,
    layer: &str,
    marking: &str,
    index: usize,
    color: Color,
) {
    let Some(proto) = marking_proto(prototypes, marking) else {
        return;
    };
    let color = match &proto.strategy {
        Some(strategy) => strategy.closest(color),
        None => color,
    };
    let Some(found) = profile
        .appearance
        .markings
        .get_mut(organ)
        .and_then(|layers| layers.get_mut(layer))
        .and_then(|markings| markings.iter_mut().find(|m| m.marking_id == marking))
    else {
        return;
    };
    if let Some(slot) = found.marking_colors.get_mut(index) {
        *slot = color;
    }
}

// Должности и антагонисты.

/// Должность в списке редактора.
#[derive(Debug, Clone, PartialEq)]
pub struct JobEntry {
    pub id: String,
    /// Ключ перевода названия.
    pub name: String,
}

/// Отдел со своими должностями.
#[derive(Debug, Clone, PartialEq)]
pub struct DepartmentEntry {
    pub id: String,
    pub name: String,
    pub color: Option<Color>,
    pub jobs: Vec<JobEntry>,
}

fn job_weights(prototypes: &Prototypes) -> BTreeMap<String, i32> {
    prototypes
        .resolved_node("jobWeight", "Default")
        .and_then(|n| n.get("weights"))
        .and_then(DataNode::as_map)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.to_owned(), v.as_str()?.trim().parse().ok()?)))
                .collect()
        })
        .unwrap_or_default()
}

/// Отделы по весу (`DepartmentUIComparer`) без скрытых; в каждом — должности, выбираемые в
/// настройках, по весу `jobWeight Default` (`JobUIComparer`; без веса — как ноль).
pub fn departments(prototypes: &Prototypes) -> Vec<DepartmentEntry> {
    let weights = job_weights(prototypes);
    let mut list: Vec<(i32, DepartmentEntry)> = prototypes
        .ids("department")
        .filter_map(|id| {
            let node = prototypes.resolved_node("department", id)?;
            if flag(node, "editorHidden") {
                return None;
            }
            let weight = text(node, "weight")
                .and_then(|w| w.trim().parse().ok())
                .unwrap_or(0);
            let mut jobs: Vec<JobEntry> = node
                .get("roles")
                .and_then(DataNode::as_seq)
                .into_iter()
                .flatten()
                .filter_map(|n| n.as_str())
                .filter(|job| sets_preference(prototypes, "job", job))
                .map(|job| JobEntry {
                    id: job.to_owned(),
                    name: prototypes
                        .resolved_node("job", job)
                        .and_then(|n| text(n, "name"))
                        .unwrap_or_else(|| job.to_owned()),
                })
                .collect();
            jobs.sort_by(|a, b| {
                let weight = |j: &JobEntry| weights.get(&j.id).copied().unwrap_or(0);
                weight(b).cmp(&weight(a)).then_with(|| a.id.cmp(&b.id))
            });
            Some((
                weight,
                DepartmentEntry {
                    id: id.to_owned(),
                    name: text(node, "name").unwrap_or_else(|| id.to_owned()),
                    color: text(node, "color").and_then(|c| Color::parse(&c)),
                    jobs,
                },
            ))
        })
        .collect();
    list.sort_by(|(wa, a), (wb, b)| wb.cmp(wa).then_with(|| a.id.cmp(&b.id)));
    list.into_iter().map(|(_, d)| d).collect()
}

/// Приоритет должности в профиле.
pub fn job_priority(profile: &HumanoidCharacterProfile, job: &str) -> JobPriority {
    profile.job_priorities.get(job).copied().unwrap_or_default()
}

/// Антагонисты, выбираемые в настройках: идентификатор и ключ названия.
pub fn antags(prototypes: &Prototypes) -> Vec<JobEntry> {
    let mut list: Vec<JobEntry> = prototypes
        .ids("antag")
        .filter(|id| sets_preference(prototypes, "antag", id))
        .map(|id| JobEntry {
            id: id.to_owned(),
            name: prototypes
                .resolved_node("antag", id)
                .and_then(|n| text(n, "name"))
                .unwrap_or_else(|| id.to_owned()),
        })
        .collect();
    list.sort_by(|a, b| a.id.cmp(&b.id));
    list
}

/// Включить или выключить антагониста.
pub fn set_antag(profile: &mut HumanoidCharacterProfile, antag: &str, wanted: bool) {
    if wanted {
        profile.antag_preferences.insert(antag.to_owned());
    } else {
        profile.antag_preferences.remove(antag);
    }
}

/// Ключ названия голоса (`EmoteSoundsPrototype.VoiceSelectorName`).
pub fn voice_name(prototypes: &Prototypes, voice: &str) -> String {
    prototypes
        .resolved_node("emoteSounds", voice)
        .and_then(|n| text(n, "voiceSelectorName"))
        .unwrap_or_else(|| "humanoid-profile-editor-voice-none".into())
}

/// Цвет из текста `#RRGGBB` или `RRGGBB`.
pub fn parse_hex(text: &str) -> Option<Color> {
    let text = text.trim();
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Color::parse(&format!("#{hex}"))
}

/// Цвет текстом `#RRGGBB`.
pub fn hex(color: Color) -> String {
    let [r, g, b, _] = color.to_bytes();
    format!("#{r:02X}{g:02X}{b:02X}")
}

#[cfg(test)]
mod tests;
