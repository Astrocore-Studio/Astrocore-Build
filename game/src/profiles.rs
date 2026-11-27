//! Профиль персонажа: имя, описание, вид, возраст, пол, род, голос, внешность (цвет глаз и
//! кожи, метки по органам и слоям), приоритеты должностей, антагонисты, черты, снаряжение по
//! ролям, что делать при недоступной должности, где появляться. Настройки игрока — профили по
//! слотам и выбранный слот. Проверка профиля приводит его к допустимому (неизвестные вид,
//! должности, черты отбрасываются, имя чистится), случайный профиль — как у нового игрока.
//! Образец — `HumanoidCharacterProfile`, `HumanoidCharacterAppearance`, `PlayerPreferences`,
//! `JobPriority`, `PreferenceUnavailableMode`, `SpawnPriorityPreference`, `Marking`,
//! `RoleLoadout`, `NamingSystem`, `SpeciesPrototype`, `LocalizedDatasetPrototype` SS14
//! @ 2f4a2aad.
//!
//! Метки и окраска кожи — в `markings.rs`.
//!
//! Нет пока: проверки снаряжения по прототипам групп (`RoleLoadout.EnsureValid` — вместе со
//! снаряжением), голосов (`EmoteSoundsPrototype`) — берётся голос вида по полу.

use std::collections::{BTreeMap, BTreeSet};

use astrocore_console::{CVarDef, CVarFlags, Config, ConfigWorldExt};
use astrocore_data::DataNode;
use astrocore_loc::LocArg;
use astrocore_math::Color;
use astrocore_prototypes::Prototypes;
use bevy_ecs::world::World;
use serde::{Deserialize, Serialize};

use crate::body::{Sex, human_skin_tone};
use crate::random::GameRandom;

/// Длина имени (`ic.name_length`).
pub const IC_NAME_LENGTH: CVarDef<i32> = CVarDef::new(
    "ic.name_length",
    32,
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Наибольшая длина имени персонажа.",
);
/// Имя только из латиницы, цифр, пробела, апострофа и дефиса (`ic.restricted_names`).
pub const IC_RESTRICTED_NAMES: CVarDef<bool> = CVarDef::new(
    "ic.restricted_names",
    true,
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Имя персонажа — только латиница, цифры, пробел, апостроф и дефис.",
);
/// Имя и фамилия с заглавной буквы (`ic.name_case`).
pub const IC_NAME_CASE: CVarDef<bool> = CVarDef::new(
    "ic.name_case",
    true,
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Первое и последнее слово имени — с заглавной буквы.",
);
/// Длина описания (`ic.flavor_text_length`).
pub const IC_FLAVOR_TEXT_LENGTH: CVarDef<i32> = CVarDef::new(
    "ic.flavor_text_length",
    512,
    CVarFlags::SERVER.union(CVarFlags::REPLICATED),
    "Наибольшая длина описания персонажа.",
);
/// Слотов персонажей (`game.maxcharacterslots`).
pub const GAME_MAX_CHARACTER_SLOTS: CVarDef<i32> = CVarDef::new(
    "game.maxcharacterslots",
    30,
    CVarFlags::SERVER
        .union(CVarFlags::REPLICATED)
        .union(CVarFlags::ARCHIVE),
    "Сколько персонажей может быть у игрока.",
);
/// Должности нового персонажа через запятую (`game.new_character_jobs`).
pub const GAME_NEW_CHARACTER_JOBS: CVarDef<&str> = CVarDef::new(
    "game.new_character_jobs",
    "Passenger",
    CVarFlags::REPLICATED,
    "Должности нового персонажа через запятую; первая — высокий приоритет.",
);
/// Виды, которых не дают новым игрокам (`ic.blacklist_species_new_account`).
pub const IC_NEW_ACCOUNT_SPECIES_BLACKLIST: CVarDef<&str> = CVarDef::new(
    "ic.blacklist_species_new_account",
    "Diona,Vulpkanin,Vox,SlimePerson",
    CVarFlags::SERVER,
    "Виды, которых не получает случайный персонаж нового игрока.",
);

/// Вид по умолчанию (`DefaultSpecies`).
pub const DEFAULT_SPECIES: &str = "Human";
/// Должность для переполнения (`GameTicker.FallbackOverflowJob`).
pub const FALLBACK_OVERFLOW_JOB: &str = "Passenger";

/// Грамматический род (`Gender` Robust).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum Gender {
    #[default]
    Epicene,
    Female,
    Male,
    Neuter,
}

impl Gender {
    pub fn name(self) -> &'static str {
        match self {
            Self::Epicene => "Epicene",
            Self::Female => "Female",
            Self::Male => "Male",
            Self::Neuter => "Neuter",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Epicene, Self::Female, Self::Male, Self::Neuter]
            .into_iter()
            .find(|g| g.name().eq_ignore_ascii_case(text))
    }
}

/// Имя пола, как в базе SS14.
pub fn sex_name(sex: Sex) -> &'static str {
    match sex {
        Sex::Male => "Male",
        Sex::Female => "Female",
        Sex::Unsexed => "Unsexed",
    }
}

pub fn parse_sex(text: &str) -> Option<Sex> {
    [Sex::Male, Sex::Female, Sex::Unsexed]
        .into_iter()
        .find(|s| sex_name(*s).eq_ignore_ascii_case(text))
}

/// Приоритет должности (`JobPriority`): номера как в базе SS14.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum JobPriority {
    #[default]
    Never = 0,
    Low = 1,
    Medium = 2,
    High = 3,
}

impl JobPriority {
    pub fn from_i64(value: i64) -> Self {
        match value {
            1 => Self::Low,
            2 => Self::Medium,
            3 => Self::High,
            _ => Self::Never,
        }
    }
}

/// Если должность недоступна (`PreferenceUnavailableMode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PreferenceUnavailableMode {
    StayInLobby = 0,
    #[default]
    SpawnAsOverflow = 1,
}

impl PreferenceUnavailableMode {
    pub fn from_i64(value: i64) -> Self {
        if value == 1 {
            Self::SpawnAsOverflow
        } else {
            Self::StayInLobby
        }
    }
}

/// Где появляться при позднем входе (`SpawnPriorityPreference`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpawnPriorityPreference {
    #[default]
    None = 0,
    Arrivals = 1,
    Cryosleep = 2,
}

impl SpawnPriorityPreference {
    pub fn from_i64(value: i64) -> Self {
        match value {
            1 => Self::Arrivals,
            2 => Self::Cryosleep,
            _ => Self::None,
        }
    }
}

/// Метка внешности (`Marking`): прототип и цвета.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marking {
    pub marking_id: String,
    #[serde(default, rename = "markingColor")]
    pub marking_colors: Vec<Color>,
}

/// Метки: категория органа → слой → метки.
pub type OrganMarkings = BTreeMap<String, BTreeMap<String, Vec<Marking>>>;

/// Внешность (`HumanoidCharacterAppearance`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HumanoidCharacterAppearance {
    pub eye_color: Color,
    pub skin_color: Color,
    #[serde(default)]
    pub markings: OrganMarkings,
}

impl Default for HumanoidCharacterAppearance {
    fn default() -> Self {
        Self {
            eye_color: Color::BLACK,
            skin_color: clamp_color(crate::body::color_from_hsv(0.07, 0.2, 1.0)),
            markings: OrganMarkings::new(),
        }
    }
}

/// Цвет без прозрачности, по байтам (`ClampColor`).
pub fn clamp_color(color: Color) -> Color {
    let [r, g, b, _] = color.to_bytes();
    Color::from_bytes([r, g, b, 255])
}

/// Снаряжение роли (`RoleLoadout`): выбранное по группам и имя сущности (у ИИ, боргов).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RoleLoadout {
    pub role: String,
    pub entity_name: Option<String>,
    /// Группа → выбранные прототипы снаряжения.
    pub selected: BTreeMap<String, Vec<String>>,
}

/// Профиль персонажа (`HumanoidCharacterProfile`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanoidCharacterProfile {
    pub name: String,
    pub flavor_text: String,
    pub species: String,
    pub age: i32,
    pub sex: Sex,
    pub voice: String,
    pub gender: Gender,
    pub appearance: HumanoidCharacterAppearance,
    pub spawn_priority: SpawnPriorityPreference,
    pub job_priorities: BTreeMap<String, JobPriority>,
    pub preference_unavailable: PreferenceUnavailableMode,
    pub antag_preferences: BTreeSet<String>,
    pub trait_preferences: BTreeSet<String>,
    pub loadouts: BTreeMap<String, RoleLoadout>,
}

impl Default for HumanoidCharacterProfile {
    fn default() -> Self {
        Self {
            name: "John Doe".into(),
            flavor_text: String::new(),
            species: DEFAULT_SPECIES.into(),
            age: 18,
            sex: Sex::Male,
            voice: "MaleHuman".into(),
            gender: Gender::Male,
            appearance: HumanoidCharacterAppearance::default(),
            spawn_priority: SpawnPriorityPreference::None,
            job_priorities: BTreeMap::from([(FALLBACK_OVERFLOW_JOB.into(), JobPriority::High)]),
            preference_unavailable: PreferenceUnavailableMode::SpawnAsOverflow,
            antag_preferences: BTreeSet::new(),
            trait_preferences: BTreeSet::new(),
            loadouts: BTreeMap::new(),
        }
    }
}

/// Высокий приоритет — у одной должности, «никогда» не хранится.
fn normalize_priorities(priorities: &mut BTreeMap<String, JobPriority>) {
    priorities.retain(|_, p| *p != JobPriority::Never);
    let mut high = false;
    for priority in priorities.values_mut() {
        if *priority != JobPriority::High {
            continue;
        }
        if high {
            *priority = JobPriority::Medium;
        }
        high = true;
    }
}

impl HumanoidCharacterProfile {
    /// Приоритет должности (`WithJobPriority`): новый высокий понижает прежний до среднего.
    pub fn set_job_priority(&mut self, job: &str, priority: JobPriority) {
        match priority {
            JobPriority::Never => {
                self.job_priorities.remove(job);
            }
            JobPriority::High => {
                for value in self.job_priorities.values_mut() {
                    if *value == JobPriority::High {
                        *value = JobPriority::Medium;
                    }
                }
                self.job_priorities.insert(job.into(), priority);
            }
            _ => {
                self.job_priorities.insert(job.into(), priority);
            }
        }
    }

    /// Приоритеты целиком (`WithJobPriorities`).
    pub fn set_job_priorities(&mut self, priorities: BTreeMap<String, JobPriority>) {
        self.job_priorities = priorities;
        normalize_priorities(&mut self.job_priorities);
    }

    /// Должности из настройки (`WithJobFromCvar`): первая — высокий, остальные — средний.
    pub fn set_jobs_from_config(&mut self, config: &Config) {
        let mut priorities = BTreeMap::new();
        let mut priority = JobPriority::High;
        for job in config.get_text(&GAME_NEW_CHARACTER_JOBS).split(',') {
            let job = job.trim();
            if job.is_empty() {
                continue;
            }
            priorities.insert(job.to_owned(), priority);
            priority = JobPriority::Medium;
        }
        self.job_priorities = priorities;
    }
}

/// Настройки игрока (`PlayerPreferences`): персонажи по слотам и выбранный.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerPreferences {
    pub characters: BTreeMap<i32, HumanoidCharacterProfile>,
    pub selected_character_index: i32,
    pub admin_ooc_color: Color,
}

impl PlayerPreferences {
    /// Выбранный персонаж (`SelectedCharacter`).
    pub fn selected_character(&self) -> Option<&HumanoidCharacterProfile> {
        self.characters.get(&self.selected_character_index)
    }
}

// Вид.

/// Сведения о виде (`SpeciesPrototype`) для профиля.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeciesInfo {
    pub id: String,
    pub round_start: bool,
    pub sexes: Vec<Sex>,
    pub min_age: i32,
    pub old_age: i32,
    pub max_age: i32,
    pub voices: BTreeSet<String>,
    pub default_sounds_by_sex: [String; 3],
    pub naming: String,
    pub male_first_names: String,
    pub female_first_names: String,
    pub last_names: String,
    pub skin_coloration: String,
    pub default_human_skin_tone: f32,
    pub default_skin_tone: Color,
}

fn text(node: &DataNode, key: &str) -> Option<String> {
    node.get(key)?.as_str().map(str::to_owned)
}

fn number<T: std::str::FromStr>(node: &DataNode, key: &str) -> Option<T> {
    node.get(key)?.as_str()?.parse().ok()
}

fn strings(node: &DataNode, key: &str) -> Option<Vec<String>> {
    Some(
        node.get(key)?
            .as_seq()?
            .iter()
            .filter_map(|n| n.as_str().map(str::to_owned))
            .collect(),
    )
}

/// Вид по прототипу `species`.
pub fn species_info(prototypes: &Prototypes, id: &str) -> Option<SpeciesInfo> {
    let node = prototypes.resolved_node("species", id)?;
    let sounds = strings(node, "defaultSoundsBySex").unwrap_or_default();
    let sound = |i: usize, default: &str| sounds.get(i).cloned().unwrap_or_else(|| default.into());
    Some(SpeciesInfo {
        id: id.to_owned(),
        round_start: number(node, "roundStart").unwrap_or(false),
        sexes: strings(node, "sexes")
            .map(|s| s.iter().filter_map(|t| parse_sex(t)).collect())
            .unwrap_or_else(|| vec![Sex::Male, Sex::Female]),
        min_age: number(node, "minAge").unwrap_or(18),
        old_age: number(node, "oldAge").unwrap_or(60),
        max_age: number(node, "maxAge").unwrap_or(120),
        voices: strings(node, "voices")
            .map(|v| v.into_iter().collect())
            .unwrap_or_else(|| ["MaleHuman".into(), "FemaleHuman".into()].into()),
        default_sounds_by_sex: [
            sound(0, "MaleHuman"),
            sound(1, "FemaleHuman"),
            sound(2, "MaleHuman"),
        ],
        naming: text(node, "naming").unwrap_or_else(|| "FirstLast".into()),
        male_first_names: text(node, "maleFirstNames").unwrap_or_else(|| "NamesFirstMale".into()),
        female_first_names: text(node, "femaleFirstNames")
            .unwrap_or_else(|| "NamesFirstFemale".into()),
        last_names: text(node, "lastNames").unwrap_or_else(|| "NamesLast".into()),
        skin_coloration: text(node, "skinColoration").unwrap_or_default(),
        default_human_skin_tone: number(node, "defaultHumanSkinTone").unwrap_or(20.0),
        default_skin_tone: text(node, "defaultSkinTone")
            .and_then(|t| Color::parse(&t))
            .unwrap_or(Color::WHITE),
    })
}

/// Виды, доступные в начале раунда, по порядку идентификаторов.
pub fn round_start_species(prototypes: &Prototypes) -> Vec<String> {
    let mut ids: Vec<String> = prototypes
        .ids("species")
        .filter(|id| species_info(prototypes, id).is_some_and(|s| s.round_start))
        .map(str::to_owned)
        .collect();
    ids.sort();
    ids
}

/// Индекс голоса по полу (`DefaultSoundsBySex[(int)sex]`).
fn sex_index(sex: Sex) -> usize {
    match sex {
        Sex::Male => 0,
        Sex::Female => 1,
        Sex::Unsexed => 2,
    }
}

fn prototypes(world: &World) -> Option<&Prototypes> {
    world.get_resource::<Prototypes>()
}

fn with_random<T>(world: &mut World, f: impl FnOnce(&mut GameRandom) -> T) -> T {
    f(&mut crate::random::random(world))
}

// Имена.

/// Случайная строка набора `localizedDataset`: ключ перевода `префикс` + номер с единицы.
fn random_dataset_value(world: &mut World, dataset: &str) -> String {
    let (prefix, count) = prototypes(world)
        .and_then(|p| p.resolved_node("localizedDataset", dataset))
        .and_then(|n| n.get("values"))
        .map(|v| {
            (
                text(v, "prefix").unwrap_or_default(),
                number::<i32>(v, "count").unwrap_or(0),
            )
        })
        .unwrap_or_default();
    if count <= 0 {
        return String::new();
    }
    let index = with_random(world, |r| r.range(1, count + 1));
    crate::examine::loc(world, &format!("{prefix}{index}"), &[])
}

fn first_name(world: &mut World, species: &SpeciesInfo, gender: Option<Gender>) -> String {
    let male = match gender {
        Some(Gender::Male) => true,
        Some(Gender::Female) => false,
        _ => with_random(world, |r| r.prob(0.5)),
    };
    let dataset = if male {
        &species.male_first_names
    } else {
        &species.female_first_names
    };
    random_dataset_value(world, dataset)
}

/// Случайное имя вида (`NamingSystem.GetName`).
pub fn random_name(world: &mut World, species: &str, gender: Option<Gender>) -> String {
    let info = prototypes(world)
        .and_then(|p| species_info(p, species).or_else(|| species_info(p, DEFAULT_SPECIES)));
    let Some(info) = info else {
        return "John Doe".into();
    };
    let arg = |s: String| LocArg::from(s.as_str());
    match info.naming.as_str() {
        "First" => {
            let first = first_name(world, &info, gender);
            crate::examine::loc(world, "namepreset-first", &[("first", arg(first))])
        }
        "TheFirstofLast" => {
            let first = first_name(world, &info, gender);
            let last = random_dataset_value(world, &info.last_names);
            crate::examine::loc(
                world,
                "namepreset-thefirstoflast",
                &[("first", arg(first)), ("last", arg(last))],
            )
        }
        "FirstDashFirst" => {
            let first1 = first_name(world, &info, gender);
            let first2 = first_name(world, &info, gender);
            crate::examine::loc(
                world,
                "namepreset-firstdashfirst",
                &[("first1", arg(first1)), ("first2", arg(first2))],
            )
        }
        _ => {
            let first = first_name(world, &info, gender);
            let last = random_dataset_value(world, &info.last_names);
            crate::examine::loc(
                world,
                "namepreset-firstlast",
                &[("first", arg(first)), ("last", arg(last))],
            )
        }
    }
}

// Случайный профиль.

/// Род по полу (`RandomGender`).
pub fn gender_for_sex(sex: Sex) -> Gender {
    match sex {
        Sex::Male => Gender::Male,
        Sex::Female => Gender::Female,
        Sex::Unsexed => Gender::Epicene,
    }
}

const REALISTIC_EYE_COLORS: [&str; 5] = ["#A52A2A", "#808080", "#F0FFFF", "#4682B4", "#000000"];

/// Случайная внешность вида (`HumanoidCharacterAppearance.Random`): кожа по тону вида, глаза
/// из естественных цветов. Метки — с 6.2б.
pub fn random_appearance(world: &mut World, species: &SpeciesInfo) -> HumanoidCharacterAppearance {
    let eye = with_random(world, |r| r.range(0, REALISTIC_EYE_COLORS.len() as i32)) as usize;
    let skin = if species.skin_coloration == "HumanToned" {
        let tone = with_random(world, |r| r.next_f32() * 100.0);
        human_skin_tone(tone)
    } else {
        species.default_skin_tone
    };
    HumanoidCharacterAppearance {
        eye_color: Color::parse(REALISTIC_EYE_COLORS[eye]).unwrap_or(Color::BLACK),
        skin_color: clamp_color(skin),
        markings: OrganMarkings::new(),
    }
}

/// Случайный профиль (`HumanoidCharacterProfile.Random`); виды из `ignored` не выпадают.
pub fn random_profile(world: &mut World, ignored: &BTreeSet<String>) -> HumanoidCharacterProfile {
    let pool: Vec<String> = prototypes(world)
        .map(round_start_species)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !ignored.contains(s))
        .collect();
    let species_id = if pool.is_empty() {
        DEFAULT_SPECIES.to_owned()
    } else {
        let index = with_random(world, |r| r.range(0, pool.len() as i32)) as usize;
        pool[index].clone()
    };
    let Some(species) = prototypes(world).and_then(|p| species_info(p, &species_id)) else {
        return HumanoidCharacterProfile::default();
    };
    let sex = if species.sexes.is_empty() {
        Sex::Male
    } else {
        let index = with_random(world, |r| r.range(0, species.sexes.len() as i32)) as usize;
        species.sexes[index]
    };
    let gender = gender_for_sex(sex);
    let name = random_name(world, &species.id, Some(gender));
    let age = with_random(world, |r| r.range(species.min_age, species.old_age));
    let appearance = if world.contains_resource::<Prototypes>() {
        world.resource_scope(|world, prototypes: bevy_ecs::world::Mut<Prototypes>| {
            with_random(world, |r| {
                crate::markings::random_appearance(&prototypes, r, &species.id, sex)
            })
        })
    } else {
        random_appearance(world, &species)
    };
    HumanoidCharacterProfile {
        name,
        species: species.id.clone(),
        age,
        sex,
        voice: species.default_sounds_by_sex[sex_index(sex)].clone(),
        gender,
        appearance,
        ..HumanoidCharacterProfile::default()
    }
}

// Проверка.

/// Имя без запрещённых символов (`RestrictedNameRegex`: `[^A-Za-z0-9 '\-]`).
fn restrict_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '\'' | '-'))
        .collect()
}

/// Первое и последнее слово с заглавной буквы (`ICNameCaseRegex`).
fn name_case(name: &str) -> String {
    let words: Vec<&str> = name.split(' ').collect();
    let last = words.iter().rposition(|w| !w.is_empty());
    let first = words.iter().position(|w| !w.is_empty());
    words
        .iter()
        .enumerate()
        .map(|(i, word)| {
            if Some(i) == first || Some(i) == last {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            } else {
                (*word).to_owned()
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Без разметки `[тег]` (`FormattedMessage.RemoveMarkup`).
fn remove_markup(text: &str) -> String {
    crate::examine::strip_markup(text)
}

fn sets_preference(prototypes: &Prototypes, kind: &str, id: &str) -> bool {
    prototypes
        .resolved_node(kind, id)
        .is_some_and(|n| number::<bool>(n, "setPreference").unwrap_or(true))
}

/// Черты в пределах очков категорий (`GetValidTraits`).
fn valid_traits(prototypes: &Prototypes, traits: &BTreeSet<String>) -> BTreeSet<String> {
    let mut spent: BTreeMap<String, i32> = BTreeMap::new();
    let mut result = BTreeSet::new();
    for id in traits {
        let Some(node) = prototypes.resolved_node("trait", id) else {
            continue;
        };
        let Some(category) = text(node, "category") else {
            result.insert(id.clone());
            continue;
        };
        let Some(category_node) = prototypes.resolved_node("traitCategory", &category) else {
            continue;
        };
        let max = number::<i32>(category_node, "maxTraitPoints").unwrap_or(-1);
        let cost = number::<i32>(node, "cost").unwrap_or(0);
        let total = spent.get(&category).copied().unwrap_or(0) + cost;
        if max >= 0 && total > max {
            continue;
        }
        spent.insert(category, total);
        result.insert(id.clone());
    }
    result
}

/// Привести профиль к допустимому (`EnsureValid`).
pub fn ensure_valid(world: &mut World, profile: &mut HumanoidCharacterProfile) {
    let species = prototypes(world)
        .and_then(|p| species_info(p, &profile.species))
        .filter(|s| s.round_start)
        .or_else(|| prototypes(world).and_then(|p| species_info(p, DEFAULT_SPECIES)));
    let Some(species) = species else {
        return;
    };
    profile.species = species.id.clone();
    let mut sex = profile.sex;
    if !species.sexes.contains(&sex) {
        sex = species.sexes.first().copied().unwrap_or(Sex::Male);
    }
    if !species.voices.contains(&profile.voice) {
        profile.voice = species.default_sounds_by_sex[sex_index(profile.sex)].clone();
    }
    profile.sex = sex;
    profile.age = profile
        .age
        .clamp(species.min_age, species.max_age.max(species.min_age));

    let config = world.resource::<Config>();
    let max_name = config.get(&IC_NAME_LENGTH).max(1) as usize;
    let restricted = config.get(&IC_RESTRICTED_NAMES);
    let case = config.get(&IC_NAME_CASE);
    let max_flavor = config.get(&IC_FLAVOR_TEXT_LENGTH).max(0) as usize;
    let mut name: String = profile.name.chars().take(max_name).collect();
    name = name.trim().to_owned();
    if restricted {
        name = restrict_name(&name);
    }
    if case {
        name = name_case(&name);
    }
    if name.trim().is_empty() {
        name = random_name(world, &species.id, Some(profile.gender));
    }
    profile.name = name;
    profile.flavor_text = remove_markup(&profile.flavor_text)
        .chars()
        .take(max_flavor)
        .collect();

    // Внешность: цвета без прозрачности, кожа по окраске вида, метки по органам вида.
    profile.appearance.eye_color = clamp_color(profile.appearance.eye_color);
    profile.appearance.skin_color = clamp_color(profile.appearance.skin_color);
    if let Some(prototypes) = prototypes(world) {
        profile.appearance = crate::markings::ensure_valid_appearance(
            prototypes,
            &profile.appearance,
            &profile.species,
            profile.sex,
        );
    }

    let Some(prototypes) = prototypes(world) else {
        return;
    };
    profile
        .job_priorities
        .retain(|job, _| sets_preference(prototypes, "job", job));
    normalize_priorities(&mut profile.job_priorities);
    profile
        .antag_preferences
        .retain(|antag| sets_preference(prototypes, "antag", antag));
    profile.trait_preferences = valid_traits(prototypes, &profile.trait_preferences);
    profile
        .loadouts
        .retain(|role, _| prototypes.resolved_node("roleLoadout", role).is_some());
    for (role, loadout) in &mut profile.loadouts {
        loadout.role = role.clone();
    }
}

/// Проверенная копия (`Validated`).
pub fn validated(
    world: &mut World,
    profile: &HumanoidCharacterProfile,
) -> HumanoidCharacterProfile {
    let mut copy = profile.clone();
    ensure_valid(world, &mut copy);
    copy
}

/// Описание персонажа (`Summary`).
pub fn summary(world: &World, profile: &HumanoidCharacterProfile) -> String {
    crate::examine::loc(
        world,
        "humanoid-character-profile-summary",
        &[
            ("name", LocArg::from(profile.name.as_str())),
            (
                "gender",
                LocArg::from(profile.gender.name().to_lowercase().as_str()),
            ),
            ("age", LocArg::from(profile.age.to_string().as_str())),
        ],
    )
}

pub fn install(world: &mut World) {
    world.register_cvar(&IC_NAME_LENGTH);
    world.register_cvar(&IC_RESTRICTED_NAMES);
    world.register_cvar(&IC_NAME_CASE);
    world.register_cvar(&IC_FLAVOR_TEXT_LENGTH);
    world.register_cvar(&GAME_MAX_CHARACTER_SLOTS);
    world.register_cvar(&GAME_NEW_CHARACTER_JOBS);
    world.register_cvar(&IC_NEW_ACCOUNT_SPECIES_BLACKLIST);
}

#[cfg(test)]
pub(crate) mod tests;
