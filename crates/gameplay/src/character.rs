//! Профиль персонажа (`HumanoidCharacterProfile`, `HumanoidCharacterAppearance` SS14).
//!
//! Игрок хранит до `Limits::max_slots` профилей. Клиент шлёт профиль серверу, сервер приводит
//! его к допустимому виду (`Profile::validated`, аналог `EnsureValid`) и только такой принимает.
//!
//! Все правила — данные, как в SS14, а не код: пределы (`CharacterRules`), виды (`Species`:
//! возраст, пол, палитры цветов глаз и волос, тон кожи по умолчанию) и стили причёски и бороды
//! (`Marking`) лежат в прототипах `content/prototypes/character.ron` и `customization.ron`;
//! `Rules::from_registry` собирает их, а редактор персонажа на клиенте рисуется по тем же
//! `Rules`, которые сервер присылает (`RULES_COMPONENT`). Новый вид или другой предел — правка
//! контента без перекомпиляции. Цвет кожи человека — «тоном» 0…100 (`HumanToned`).
//!
//! Не перенесено: другие стратегии цвета кожи, маркировки, черты, предпочтения антагонистов,
//! голос, приоритеты должностей (этап 5 плана), сохранение (этап 4).

use serde::{Deserialize, Serialize};

/// Имя, если игрок оставил поле пустым (в SS14 — случайное по списку имён вида).
pub const DEFAULT_NAME: &str = "John Doe";

pub const LOOK_COMPONENT: &str = "build.look";
pub const LOOK_MAX_BYTES: usize = 1024;
/// Правила и каталог стилей для редактора (только владельцу, один раз). Компонент ограничен
/// 16 КиБ, поэтому каталог идёт отдельно и в сжатом виде (картинки сгруппированы по RSI).
pub const RULES_COMPONENT: &str = "build.charrules";
pub const RULES_MAX_BYTES: usize = 16_384;
pub const CATALOG_COMPONENT: &str = "build.charstyles";
pub const CATALOG_MAX_BYTES: usize = 16_384;
/// Текущий профиль игрока (только владельцу).
pub const PROFILE_COMPONENT: &str = "build.profile";
pub const PROFILE_MAX_BYTES: usize = 4096;
/// Команды редактора: сохранить профиль (JSON) и бросить случайный.
pub const CMD_CHAR_SAVE: &str = "char.save";
pub const CMD_CHAR_RANDOM: &str = "char.random";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sex {
    Male,
    Female,
    Unsexed,
}

/// Гендер — местоимения (`Gender` SS14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Gender {
    Male,
    Female,
    Epicene,
    Neuter,
}

impl Sex {
    /// Гендер, который ставит редактор вместе с полом (`WithSex`).
    pub fn default_gender(self) -> Gender {
        match self {
            Sex::Male => Gender::Male,
            Sex::Female => Gender::Female,
            Sex::Unsexed => Gender::Epicene,
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "Male" => Some(Sex::Male),
            "Female" => Some(Sex::Female),
            "Unsexed" => Some(Sex::Unsexed),
            _ => None,
        }
    }
}

/// Одежда по предпочтению (`ClothingPreference`): комбинезон или юбка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Clothing {
    Jumpsuit,
    Jumpskirt,
}

/// Рюкзак по предпочтению (`BackpackPreference`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backpack {
    Backpack,
    Satchel,
    Duffel,
}

pub type Rgb = [u8; 3];

/// Внешность: цвет кожи тоном, глаза, причёска и борода (стиль и цвет).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Appearance {
    /// 0 — светлая, 100 — тёмная (`HumanToned`).
    pub skin_tone: f32,
    pub eyes: Rgb,
    /// Идентификатор маркировки `Hair` из каталога; пусто — без причёски.
    pub hair: String,
    pub hair_color: Rgb,
    /// Идентификатор маркировки `FacialHair`; пусто — без бороды.
    pub facial_hair: String,
    pub facial_hair_color: Rgb,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            skin_tone: 20.0,
            eyes: [0, 0, 0],
            hair: String::new(),
            hair_color: [0x32, 0x28, 0x1e],
            facial_hair: String::new(),
            facial_hair_color: [0x32, 0x28, 0x1e],
        }
    }
}

/// Один профиль персонажа.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub flavor_text: String,
    pub species: String,
    pub age: u8,
    pub sex: Sex,
    pub gender: Gender,
    pub appearance: Appearance,
    pub clothing: Clothing,
    pub backpack: Backpack,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: DEFAULT_NAME.into(),
            flavor_text: String::new(),
            species: "Human".into(),
            age: 18,
            sex: Sex::Male,
            gender: Gender::Male,
            appearance: Appearance::default(),
            clothing: Clothing::Jumpsuit,
            backpack: Backpack::Backpack,
        }
    }
}

/// Пределы профиля (`CharacterRules`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_slots: usize,
    pub max_name_len: usize,
    pub max_flavor_len: usize,
}

/// Вид (`Species` SS14): что можно выбрать и значения по умолчанию.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Species {
    pub id: String,
    pub name: String,
    /// Доступен ли игроку в редакторе (`roundStart`).
    pub round_start: bool,
    pub min_age: u8,
    pub max_age: u8,
    pub sexes: Vec<Sex>,
    pub default_skin_tone: f32,
    /// Палитры, из которых случайный профиль берёт цвет; в редакторе цвет любой.
    pub eye_colors: Vec<Rgb>,
    pub hair_colors: Vec<Rgb>,
}

/// Стиль причёски или бороды: идентификатор, имя (состояние RSI) и картинка (путь внутри
/// `Textures`, лист 2×2 направлений S, N, E, W).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub id: String,
    pub name: String,
    pub png: String,
}

/// Допустимые стили причёски и бороды (прототипы с компонентом `Marking`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub hair: Vec<Style>,
    pub facial_hair: Vec<Style>,
}

impl Catalog {
    pub fn hair_style(&self, id: &str) -> Option<&Style> {
        self.hair.iter().find(|s| s.id == id)
    }

    pub fn facial_style(&self, id: &str) -> Option<&Style> {
        self.facial_hair.iter().find(|s| s.id == id)
    }
}

/// Все правила персонажа: пределы, виды, стили. Собираются из прототипов; без них действуют
/// значения SS14 по умолчанию (`Rules::default`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    pub limits: Limits,
    pub species: Vec<Species>,
    pub catalog: Catalog,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            limits: Limits {
                max_slots: 5,
                max_name_len: 32,
                max_flavor_len: 512,
            },
            species: vec![Species {
                id: "Human".into(),
                name: "Human".into(),
                round_start: true,
                min_age: 18,
                max_age: 120,
                sexes: vec![Sex::Male, Sex::Female, Sex::Unsexed],
                default_skin_tone: 20.0,
                eye_colors: vec![[44, 30, 20], [74, 47, 26], [59, 110, 58], [58, 95, 158]],
                hair_colors: vec![[26, 20, 16], [90, 58, 34], [194, 154, 90]],
            }],
            catalog: Catalog::default(),
        }
    }
}

fn color_list(d: Option<&prototypes::Data>) -> Vec<Rgb> {
    d.and_then(prototypes::Data::as_list)
        .map(|l| {
            l.iter()
                .filter_map(|c| {
                    let c = c.as_list()?;
                    let ch = |i: usize| c.get(i)?.as_int().map(|v| v.clamp(0, 255) as u8);
                    Some([ch(0)?, ch(1)?, ch(2)?])
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Rules {
    /// Правила из прототипов: `CharacterRules` (пределы), `Species` (виды) и `Marking` (стили).
    /// Чего нет в контенте, остаётся по умолчанию.
    pub fn from_registry(
        registry: &prototypes::PrototypeRegistry,
        namespace: &str,
    ) -> Result<Self, prototypes::ProtoError> {
        let mut rules = Self::default();
        let mut species = Vec::new();
        for (_, name) in registry.engine_index(namespace)? {
            let Ok(resolved) = registry.spawnable(&name) else {
                continue;
            };
            let text = |c: &prototypes::Data, k: &str| {
                c.get(k)
                    .and_then(prototypes::Data::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let int = |c: &prototypes::Data, k: &str, d: i64| {
                c.get(k).and_then(prototypes::Data::as_int).unwrap_or(d)
            };
            if let Some(c) = resolved.component("CharacterRules") {
                rules.limits = Limits {
                    max_slots: int(c, "maxSlots", 5).clamp(1, 32) as usize,
                    max_name_len: int(c, "maxNameLength", 32).clamp(1, 128) as usize,
                    max_flavor_len: int(c, "maxFlavorLength", 512).clamp(0, 4096) as usize,
                };
            }
            if let Some(c) = resolved.component("Species") {
                let sexes: Vec<Sex> = c
                    .get("sexes")
                    .and_then(prototypes::Data::as_list)
                    .map(|l| l.iter().filter_map(|v| Sex::parse(v.as_str()?)).collect())
                    .unwrap_or_default();
                let (min_age, max_age) = (
                    int(c, "minAge", 18).clamp(1, 255) as u8,
                    int(c, "maxAge", 120).clamp(1, 255) as u8,
                );
                species.push(Species {
                    id: name.clone(),
                    name: text(c, "name"),
                    round_start: c
                        .get("roundStart")
                        .and_then(prototypes::Data::as_bool)
                        .unwrap_or(false),
                    min_age: min_age.min(max_age),
                    max_age: max_age.max(min_age),
                    sexes: if sexes.is_empty() {
                        vec![Sex::Male]
                    } else {
                        sexes
                    },
                    default_skin_tone: c
                        .get("defaultSkinTone")
                        .and_then(prototypes::Data::as_f64)
                        .map_or(20.0, |v| v as f32)
                        .clamp(0.0, 100.0),
                    eye_colors: color_list(c.get("eyeColors")),
                    hair_colors: color_list(c.get("hairColors")),
                });
            }
            if let Some(m) = resolved.component("Marking") {
                let style = Style {
                    id: name.clone(),
                    name: text(m, "state"),
                    png: format!("{}/{}.png", text(m, "sprite"), text(m, "state")),
                };
                match text(m, "part").as_str() {
                    "Hair" => rules.catalog.hair.push(style),
                    "FacialHair" => rules.catalog.facial_hair.push(style),
                    _ => {}
                }
            }
        }
        if !species.is_empty() {
            species.sort_by(|a, b| a.id.cmp(&b.id));
            rules.species = species;
        }
        rules.catalog.hair.sort_by(|a, b| a.name.cmp(&b.name));
        rules
            .catalog
            .facial_hair
            .sort_by(|a, b| a.name.cmp(&b.name));
        Ok(rules)
    }

    /// Вид по идентификатору, а неизвестный или недоступный — первый доступный в раунде.
    pub fn species_of(&self, id: &str) -> &Species {
        self.species
            .iter()
            .find(|s| s.id == id && s.round_start)
            .or_else(|| self.species.iter().find(|s| s.round_start))
            .unwrap_or(&self.species[0])
    }
}

/// Цвет кожи по тону 0…100 (`HumanTonedSkinColoration.FromUnary`): первые 20 единиц сдвигают
/// оттенок от 45° к 25°, остальные 80 поднимают насыщенность с 20 до 100 и опускают яркость со
/// 100 до 20.
pub fn skin_color(tone: f32) -> Rgb {
    let tone = if tone.is_finite() {
        tone.clamp(0.0, 100.0)
    } else {
        20.0
    };
    let offset = tone - 20.0;
    let (mut hue, mut sat, mut val) = (25.0_f32, 20.0_f32, 100.0_f32);
    if offset <= 0.0 {
        hue += offset.abs();
    } else {
        sat += offset;
        val -= offset;
    }
    hsv_to_rgb(hue / 360.0, sat / 100.0, val / 100.0)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match (i as i32).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r, g, b].map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// Имя по правилам SS14: недопустимые знаки убираются (в SS14 `[^A-Za-z0-9 '\-]`; здесь ещё
/// буквы любого алфавита), длина обрезается до `max_len`, края подрезаются, первая буква
/// первого и последнего слова становится заглавной (`ic.name_case`).
pub fn clean_name(raw: &str, max_len: usize) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '\'' | '-'))
        .take(max_len)
        .collect();
    let kept = kept.trim();
    let words: Vec<&str> = kept.split(' ').collect();
    let last = words.len().saturating_sub(1);
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            if i == 0 || i == last {
                let mut cs = w.chars();
                cs.next().map_or_else(String::new, |f| {
                    f.to_uppercase().collect::<String>() + cs.as_str()
                })
            } else {
                (*w).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Описание без разметки (`[…]`) и управляющих знаков, не длиннее `max_len`.
pub fn clean_flavor(raw: &str, max_len: usize) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    for c in raw.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            c if depth == 0 && (!c.is_control() || c == '\n') => out.push(c),
            _ => {}
        }
        if out.chars().count() >= max_len {
            break;
        }
    }
    out.chars().take(max_len).collect()
}

impl Profile {
    /// Профиль по умолчанию для вида: пустое имя, минимальный возраст, тон кожи вида.
    pub fn default_for(rules: &Rules) -> Profile {
        let sp = rules.species_of("");
        Profile {
            species: sp.id.clone(),
            age: sp.min_age,
            sex: sp.sexes[0],
            gender: sp.sexes[0].default_gender(),
            appearance: Appearance {
                skin_tone: sp.default_skin_tone,
                ..Appearance::default()
            },
            ..Profile::default()
        }
    }

    /// Профиль, приведённый к допустимому виду (`EnsureValid`): вид из доступных, возраст и пол в
    /// пределах вида, чистое имя (пустое — `DEFAULT_NAME`), тон кожи 0…100, неизвестные стили убраны.
    pub fn validated(&self, rules: &Rules) -> Profile {
        let sp = rules.species_of(&self.species);
        let mut name = clean_name(&self.name, rules.limits.max_name_len);
        if name.is_empty() {
            name = DEFAULT_NAME.into();
        }
        let a = &self.appearance;
        let style = |id: &str, found: bool| if found { id.to_string() } else { String::new() };
        let sex = if sp.sexes.contains(&self.sex) {
            self.sex
        } else {
            sp.sexes[0]
        };
        Profile {
            name,
            flavor_text: clean_flavor(&self.flavor_text, rules.limits.max_flavor_len),
            species: sp.id.clone(),
            age: self.age.clamp(sp.min_age, sp.max_age),
            sex,
            gender: self.gender,
            appearance: Appearance {
                skin_tone: if a.skin_tone.is_finite() {
                    a.skin_tone.clamp(0.0, 100.0)
                } else {
                    sp.default_skin_tone
                },
                eyes: a.eyes,
                hair: style(&a.hair, rules.catalog.hair_style(&a.hair).is_some()),
                hair_color: a.hair_color,
                facial_hair: style(
                    &a.facial_hair,
                    rules.catalog.facial_style(&a.facial_hair).is_some(),
                ),
                facial_hair_color: a.facial_hair_color,
            },
            clothing: self.clothing,
            backpack: self.backpack,
        }
    }
}

/// Слой внешности для клиента: картинка (путь внутри `Textures`) и цвет.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LookLayer {
    pub png: String,
    pub color: Rgb,
}

/// Как выглядит персонаж: рассылается всем (`build.look`). Клиент красит тело цветом кожи и кладёт
/// слои глаз, бороды и причёски (порядок слоёв гуманоида SS14: под одеждой).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Look {
    pub skin: Rgb,
    pub eyes: Rgb,
    pub facial_hair: Option<LookLayer>,
    pub hair: Option<LookLayer>,
}

pub fn encode_look(look: &Look) -> Vec<u8> {
    serde_json::to_vec(look).unwrap_or_default()
}

/// Безопасный ли путь картинки: клиент читает его как есть.
pub fn safe_png(png: &str) -> bool {
    !png.is_empty()
        && png.len() <= 128
        && png.ends_with(".png")
        && png.split('/').all(|part| {
            !part.is_empty()
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        })
}

/// Разобрать внешность; пути картинок проверяются.
pub fn decode_look(bytes: &[u8]) -> Option<Look> {
    if bytes.is_empty() || bytes.len() > LOOK_MAX_BYTES {
        return None;
    }
    let look: Look = serde_json::from_slice(bytes).ok()?;
    let safe = |l: &Option<LookLayer>| l.as_ref().is_none_or(|l| safe_png(&l.png));
    (safe(&look.hair) && safe(&look.facial_hair)).then_some(look)
}

/// Правила без каталога: пределы и виды.
#[derive(Serialize, Deserialize)]
struct RulesWire {
    limits: Limits,
    species: Vec<Species>,
}

/// Каталог стилей: картинки сгруппированы по набору RSI, у стиля — идентификатор и состояние.
#[derive(Serialize, Deserialize, Default)]
struct CatalogWire {
    hair: Vec<(String, Vec<(String, String)>)>,
    facial_hair: Vec<(String, Vec<(String, String)>)>,
}

fn group(styles: &[Style]) -> Vec<(String, Vec<(String, String)>)> {
    let mut groups: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for s in styles {
        let rsi = s.png.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        match groups.iter_mut().find(|(r, _)| *r == rsi) {
            Some((_, list)) => list.push((s.id.clone(), s.name.clone())),
            None => groups.push((rsi, vec![(s.id.clone(), s.name.clone())])),
        }
    }
    groups
}

fn ungroup(groups: Vec<(String, Vec<(String, String)>)>) -> Vec<Style> {
    groups
        .into_iter()
        .flat_map(|(rsi, list)| {
            list.into_iter().map(move |(id, name)| Style {
                png: format!("{rsi}/{name}.png"),
                id,
                name,
            })
        })
        .collect()
}

pub fn encode_rules(rules: &Rules) -> Vec<u8> {
    serde_json::to_vec(&RulesWire {
        limits: rules.limits.clone(),
        species: rules.species.clone(),
    })
    .unwrap_or_default()
}

pub fn encode_catalog(catalog: &Catalog) -> Vec<u8> {
    serde_json::to_vec(&CatalogWire {
        hair: group(&catalog.hair),
        facial_hair: group(&catalog.facial_hair),
    })
    .unwrap_or_default()
}

/// Разобрать правила и каталог на клиенте: размер ограничен, пути картинок проверяются.
pub fn decode_rules(rules: &[u8], catalog: &[u8]) -> Option<Rules> {
    if rules.is_empty()
        || rules.len() > RULES_MAX_BYTES
        || catalog.is_empty()
        || catalog.len() > CATALOG_MAX_BYTES
    {
        return None;
    }
    let wire: RulesWire = serde_json::from_slice(rules).ok()?;
    let cat: CatalogWire = serde_json::from_slice(catalog).ok()?;
    let out = Rules {
        limits: wire.limits,
        species: wire.species,
        catalog: Catalog {
            hair: ungroup(cat.hair),
            facial_hair: ungroup(cat.facial_hair),
        },
    };
    let ok = !out.species.is_empty()
        && out.species.iter().all(|s| !s.sexes.is_empty())
        && out
            .catalog
            .hair
            .iter()
            .chain(&out.catalog.facial_hair)
            .all(|s| safe_png(&s.png));
    ok.then_some(out)
}

pub fn encode_profile(profile: &Profile) -> Vec<u8> {
    serde_json::to_vec(profile).unwrap_or_default()
}

pub fn decode_profile(bytes: &[u8]) -> Option<Profile> {
    if bytes.is_empty() || bytes.len() > PROFILE_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

impl Profile {
    /// Случайный профиль (`HumanoidCharacterProfile.Random`): пол, возраст, кожа, глаза, причёска,
    /// у мужчин часто борода; цвета из палитр вида. Один и тот же `seed` — один профиль.
    pub fn random(seed: u64, rules: &Rules) -> Profile {
        let sp = rules.species_of("");
        let mut n = mix(seed);
        let mut next = |m: u64| {
            n = mix(n);
            n % m.max(1)
        };
        let sex = sp.sexes[next(sp.sexes.len() as u64) as usize];
        let pick_style = |list: &[Style], i: u64| {
            if list.is_empty() {
                String::new()
            } else {
                list[i as usize % list.len()].id.clone()
            }
        };
        let pick_color = |list: &[Rgb], i: u64, fallback: Rgb| {
            if list.is_empty() {
                fallback
            } else {
                list[i as usize % list.len()]
            }
        };
        let hair = pick_style(&rules.catalog.hair, next(1000));
        let facial = if sex == Sex::Male && next(100) < 40 {
            pick_style(&rules.catalog.facial_hair, next(1000))
        } else {
            String::new()
        };
        let hair_color = pick_color(&sp.hair_colors, next(1000), [0x32, 0x28, 0x1e]);
        let age_span = u64::from(sp.max_age.min(sp.min_age.saturating_add(42)) - sp.min_age) + 1;
        Profile {
            species: sp.id.clone(),
            sex,
            gender: sex.default_gender(),
            age: sp.min_age + next(age_span) as u8,
            appearance: Appearance {
                skin_tone: next(101) as f32,
                eyes: pick_color(&sp.eye_colors, next(1000), [0, 0, 0]),
                hair,
                hair_color,
                facial_hair: facial,
                facial_hair_color: hair_color,
            },
            ..Profile::default()
        }
        .validated(rules)
    }

    /// Внешность для рассылки клиентам.
    pub fn look(&self, rules: &Rules) -> Look {
        let a = &self.appearance;
        let layer = |style: Option<&Style>, color: Rgb| {
            style.map(|s| LookLayer {
                png: s.png.clone(),
                color,
            })
        };
        Look {
            skin: skin_color(a.skin_tone),
            eyes: a.eyes,
            facial_hair: layer(
                rules.catalog.facial_style(&a.facial_hair),
                a.facial_hair_color,
            ),
            hair: layer(rules.catalog.hair_style(&a.hair), a.hair_color),
        }
    }
}

/// Профили игрока: не больше `Limits::max_slots` и выбранный слот в их пределах.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slots {
    pub profiles: Vec<Profile>,
    pub selected: usize,
}

impl Slots {
    pub fn new(rules: &Rules) -> Self {
        Self {
            profiles: vec![Profile::default_for(rules)],
            selected: 0,
        }
    }

    pub fn current(&self) -> &Profile {
        &self.profiles[self.selected.min(self.profiles.len() - 1)]
    }

    /// Заменить профиль слота `slot` (или добавить новый, если слот следующий по порядку и есть
    /// место). Профиль проверяется. `false` — слот вне пределов.
    pub fn set(&mut self, slot: usize, profile: &Profile, rules: &Rules) -> bool {
        let valid = profile.validated(rules);
        if slot < self.profiles.len() {
            self.profiles[slot] = valid;
            true
        } else if slot == self.profiles.len() && slot < rules.limits.max_slots {
            self.profiles.push(valid);
            true
        } else {
            false
        }
    }

    /// Удалить слот; последний профиль не удаляется (в SS14 всегда есть хотя бы один).
    pub fn delete(&mut self, slot: usize) -> bool {
        if self.profiles.len() <= 1 || slot >= self.profiles.len() {
            return false;
        }
        self.profiles.remove(slot);
        self.selected = self.selected.min(self.profiles.len() - 1);
        true
    }

    pub fn select(&mut self, slot: usize) -> bool {
        if slot < self.profiles.len() {
            self.selected = slot;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        let mut r = Rules::default();
        r.catalog.hair = vec![Style {
            id: "HumanHairAfro".into(),
            name: "afro".into(),
            png: "Mobs/Customization/human_hair.rsi/afro.png".into(),
        }];
        r.catalog.facial_hair = vec![Style {
            id: "HumanFacialHairBeard".into(),
            name: "beard".into(),
            png: "Mobs/Customization/human_facial_hair.rsi/beard.png".into(),
        }];
        r
    }

    /// Правила из настоящих файлов контента.
    fn real_rules() -> Rules {
        let mut registry = prototypes::PrototypeRegistry::new();
        for (text, name) in [
            (
                include_str!("../../../content/prototypes/customization.ron"),
                "customization",
            ),
            (
                include_str!("../../../content/prototypes/character.ron"),
                "character",
            ),
        ] {
            registry.load_ron_str(text, name).unwrap();
        }
        registry.validate().unwrap();
        Rules::from_registry(&registry, "ss14").unwrap()
    }

    #[test]
    fn the_rules_come_from_the_content_prototypes() {
        let r = real_rules();
        assert_eq!(
            r.limits,
            Limits {
                max_slots: 5,
                max_name_len: 32,
                max_flavor_len: 512
            }
        );
        assert_eq!(r.species.len(), 1);
        let human = &r.species[0];
        assert_eq!(
            (human.id.as_str(), human.min_age, human.max_age),
            ("Human", 18, 120)
        );
        assert_eq!(human.sexes, vec![Sex::Male, Sex::Female, Sex::Unsexed]);
        assert_eq!(human.eye_colors.len(), 6);
        assert_eq!(human.hair_colors.len(), 7);
        assert_eq!(r.catalog.hair.len(), 203);
        assert_eq!(r.catalog.facial_hair.len(), 35);
        assert!(r.catalog.hair_style("HumanHairAfro").is_some());
        assert!(r.catalog.facial_style("HumanFacialHairChin").is_some());
    }

    #[test]
    fn a_new_species_in_content_needs_no_code() {
        let mut registry = prototypes::PrototypeRegistry::new();
        registry
            .load_ron_str(
                r#"[
                    (id: "CharacterRules", components: {"CharacterRules": (maxSlots: 3, maxNameLength: 10, maxFlavorLength: 20)}),
                    (id: "Dwarf", components: {"Species": (name: "Dwarf", roundStart: true, minAge: 30, maxAge: 200, sexes: ["Female"], defaultSkinTone: 50.0, eyeColors: [[1, 2, 3]], hairColors: [])}),
                    (id: "Ghost", components: {"Species": (name: "Ghost", roundStart: false, minAge: 1, maxAge: 9, sexes: ["Male"])}),
                ]"#,
                "t",
            )
            .unwrap();
        registry.validate().unwrap();
        let r = Rules::from_registry(&registry, "ss14").unwrap();
        assert_eq!(r.limits.max_slots, 3);
        assert_eq!(r.species.len(), 2);
        let p = Profile {
            species: "Ghost".into(),
            age: 5,
            sex: Sex::Male,
            name: "x".repeat(50),
            ..Profile::default()
        }
        .validated(&r);
        assert_eq!(p.species, "Dwarf", "недоступный вид заменён доступным");
        assert_eq!(p.age, 30, "возраст в пределах вида");
        assert_eq!(p.sex, Sex::Female, "пол из списка вида");
        assert_eq!(p.name.chars().count(), 10, "предел имени из правил");
        assert_eq!(Profile::default_for(&r).appearance.skin_tone, 50.0);
    }

    #[test]
    fn a_name_is_cleaned_cut_and_capitalized_like_ss14() {
        assert_eq!(clean_name("  john   doe ", 32), "John   Doe");
        assert_eq!(clean_name("j0hn <b>doe</b>!", 32), "J0hn Bdoeb");
        assert_eq!(clean_name("анна-мария иванова", 32), "Анна-мария Иванова");
        assert_eq!(clean_name("o'neil", 32), "O'neil");
        assert_eq!(clean_name(&"a".repeat(100), 32).chars().count(), 32);
        assert_eq!(clean_name("!!!", 32), "");
    }

    #[test]
    fn flavor_text_loses_markup_and_is_cut() {
        assert_eq!(
            clean_flavor("Tall [color=red]and[/color] calm", 512),
            "Tall and calm"
        );
        assert_eq!(clean_flavor(&"x".repeat(2000), 512).chars().count(), 512);
        assert_eq!(
            clean_flavor("a\u{7}b", 512),
            "ab",
            "управляющие знаки убраны"
        );
    }

    #[test]
    fn skin_tones_follow_the_human_toned_strategy() {
        assert_eq!(skin_color(20.0), [255, 225, 204]);
        let (light, dark) = (skin_color(0.0), skin_color(100.0));
        assert!(light[0] > 250 && light[1] > light[2], "{light:?}");
        assert!(dark[0] < 60, "тёмный: яркость 20%: {dark:?}");
        assert_eq!(skin_color(-5.0), skin_color(0.0));
        assert_eq!(skin_color(f32::NAN), skin_color(20.0));
    }

    #[test]
    fn a_profile_is_brought_to_a_valid_shape() {
        let r = rules();
        let bad = Profile {
            name: "  ".into(),
            age: 5,
            species: "Vox".into(),
            appearance: Appearance {
                skin_tone: 500.0,
                hair: "NoSuchHair".into(),
                facial_hair: "HumanFacialHairBeard".into(),
                ..Appearance::default()
            },
            ..Profile::default()
        };
        let ok = bad.validated(&r);
        assert_eq!(ok.name, DEFAULT_NAME);
        assert_eq!(ok.age, 18);
        assert_eq!(ok.species, "Human");
        assert_eq!(ok.appearance.skin_tone, 100.0);
        assert_eq!(ok.appearance.hair, "");
        assert_eq!(ok.appearance.facial_hair, "HumanFacialHairBeard");
        assert_eq!(ok.validated(&r), ok, "повторная проверка ничего не меняет");
        let old = Profile {
            age: 200,
            ..Profile::default()
        };
        assert_eq!(old.validated(&r).age, 120);
    }

    #[test]
    fn a_random_profile_is_valid_stable_and_varied() {
        let r = rules();
        let a = Profile::random(7, &r);
        assert_eq!(a, Profile::random(7, &r));
        assert_eq!(a.validated(&r), a);
        let many: std::collections::BTreeSet<String> = (0..64)
            .map(|s| {
                let p = Profile::random(s, &r);
                format!(
                    "{:?}{}{:?}",
                    p.sex, p.appearance.skin_tone, p.appearance.eyes
                )
            })
            .collect();
        assert!(many.len() > 20, "внешности разные: {}", many.len());
        let real = real_rules();
        let eyes: std::collections::BTreeSet<Rgb> = (0..64)
            .map(|s| Profile::random(s, &real).appearance.eyes)
            .collect();
        assert!(
            eyes.iter().all(|c| real.species[0].eye_colors.contains(c)),
            "из палитры вида"
        );
    }

    #[test]
    fn the_look_resolves_styles_and_rejects_unsafe_paths() {
        let r = rules();
        let mut p = Profile::default();
        p.appearance.hair = "HumanHairAfro".into();
        let look = p.look(&r);
        assert_eq!(
            look.hair.as_ref().map(|l| l.png.as_str()),
            Some("Mobs/Customization/human_hair.rsi/afro.png")
        );
        assert!(look.facial_hair.is_none());
        assert_eq!(decode_look(&encode_look(&look)), Some(look.clone()));
        let mut bad = look;
        bad.hair.as_mut().unwrap().png = "../../etc/passwd.png".into();
        assert!(decode_look(&encode_look(&bad)).is_none());
        assert!(decode_look(&[]).is_none());
        assert!(decode_look(&vec![b' '; LOOK_MAX_BYTES + 1]).is_none());
    }

    #[test]
    fn rules_travel_to_the_client_and_unsafe_ones_are_refused() {
        let real = real_rules();
        let (rules, catalog) = (encode_rules(&real), encode_catalog(&real.catalog));
        assert!(
            rules.len() < RULES_MAX_BYTES,
            "правила {} байт",
            rules.len()
        );
        assert!(
            catalog.len() < CATALOG_MAX_BYTES,
            "каталог {} байт",
            catalog.len()
        );
        assert_eq!(decode_rules(&rules, &catalog), Some(real.clone()));
        let mut bad = real;
        bad.catalog.hair[0].png = "../x.png".into();
        assert!(decode_rules(&rules, &encode_catalog(&bad.catalog)).is_none());
    }

    #[test]
    fn at_most_the_limit_of_slots_and_the_last_profile_stays() {
        let r = rules();
        let mut s = Slots::new(&r);
        let max = r.limits.max_slots;
        for i in 1..max {
            assert!(s.set(i, &Profile::default(), &r), "слот {i}");
        }
        assert_eq!(s.profiles.len(), max);
        assert!(!s.set(max, &Profile::default(), &r));
        assert!(!s.set(max + 3, &Profile::default(), &r));
        assert!(s.select(4));
        assert!(s.delete(4));
        assert_eq!(s.selected, 3);
        while s.profiles.len() > 1 {
            assert!(s.delete(0));
        }
        assert!(!s.delete(0));
        assert!(!s.select(1));
    }

    #[test]
    fn a_profile_round_trips_through_json() {
        let p = Profile {
            name: "Анна Иванова".into(),
            sex: Sex::Female,
            gender: Gender::Female,
            clothing: Clothing::Jumpskirt,
            ..Profile::default()
        };
        assert_eq!(decode_profile(&encode_profile(&p)), Some(p));
    }
}
