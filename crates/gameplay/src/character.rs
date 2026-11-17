//! Профиль персонажа (`HumanoidCharacterProfile`, `HumanoidCharacterAppearance` SS14).
//!
//! Игрок хранит до `MAX_SLOTS` профилей. Клиент шлёт профиль серверу, сервер приводит его к
//! допустимому виду (`Profile::validated`, аналог `EnsureValid`) и только такой принимает. Пока
//! только человек: пол из списка, цвет кожи — «тоном» 0…100 (`HumanToned`), цвета глаз и волос
//! произвольные (канал за каналом без прозрачности), стили причёски и бороды — из каталога
//! прототипов с компонентом `Marking` (`content/prototypes/customization.ron`).
//!
//! Не перенесено: другие виды, маркировки, черты, предпочтения антагонистов, голос, приоритеты
//! должностей (этап 5 плана), сохранение (этап 4).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Слотов персонажа на игрока.
pub const MAX_SLOTS: usize = 5;
/// Пределы SS14: `ic.name_length`, `ic.flavor_text_length`, возраст человека.
pub const MAX_NAME_LEN: usize = 32;
pub const MAX_FLAVOR_LEN: usize = 512;
pub const MIN_AGE: u8 = 18;
pub const MAX_AGE: u8 = 120;
/// Вид по умолчанию и единственный (`Species/human.yml`).
pub const SPECIES_HUMAN: &str = "Human";
/// Имя, если игрок оставил поле пустым (в SS14 — случайное по списку имён вида).
pub const DEFAULT_NAME: &str = "John Doe";
/// Тон кожи по умолчанию (`DefaultHumanSkinTone`).
pub const DEFAULT_SKIN_TONE: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sex {
    Male,
    Female,
    Unsexed,
}

/// Гендер — местоимения (`Gender` SS14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
            skin_tone: DEFAULT_SKIN_TONE,
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
            species: SPECIES_HUMAN.into(),
            age: MIN_AGE,
            sex: Sex::Male,
            gender: Gender::Male,
            appearance: Appearance::default(),
            clothing: Clothing::Jumpsuit,
            backpack: Backpack::Backpack,
        }
    }
}

/// Допустимые стили: идентификаторы маркировок причёски и бороды.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    pub hair: BTreeSet<String>,
    pub facial_hair: BTreeSet<String>,
}

impl Catalog {
    /// Каталог из прототипов с компонентом `Marking` (`part`: `Hair` или `FacialHair`).
    pub fn from_registry(
        registry: &prototypes::PrototypeRegistry,
        namespace: &str,
    ) -> Result<Self, prototypes::ProtoError> {
        let mut catalog = Self::default();
        for (_, name) in registry.engine_index(namespace)? {
            let Ok(resolved) = registry.spawnable(&name) else {
                continue;
            };
            let Some(marking) = resolved.component("Marking") else {
                continue;
            };
            match marking.get("part").and_then(prototypes::Data::as_str) {
                Some("Hair") => catalog.hair.insert(name),
                Some("FacialHair") => catalog.facial_hair.insert(name),
                _ => false,
            };
        }
        Ok(catalog)
    }
}

/// Цвет кожи по тону 0…100 (`HumanTonedSkinColoration.FromUnary`): первые 20 единиц сдвигают
/// оттенок от 45° к 25°, остальные 80 поднимают насыщенность с 20 до 100 и опускают яркость со
/// 100 до 20.
pub fn skin_color(tone: f32) -> Rgb {
    let tone = if tone.is_finite() {
        tone.clamp(0.0, 100.0)
    } else {
        DEFAULT_SKIN_TONE
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
/// буквы любого алфавита), длина обрезается до `MAX_NAME_LEN`, края подрезаются, первая буква
/// первого и последнего слова становится заглавной (`ic.name_case`).
pub fn clean_name(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '\'' | '-'))
        .take(MAX_NAME_LEN)
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

/// Описание без разметки (`[…]`) и управляющих знаков, не длиннее `MAX_FLAVOR_LEN`.
pub fn clean_flavor(raw: &str) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    for c in raw.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            c if depth == 0 && (!c.is_control() || c == '\n') => out.push(c),
            _ => {}
        }
        if out.chars().count() >= MAX_FLAVOR_LEN {
            break;
        }
    }
    out.chars().take(MAX_FLAVOR_LEN).collect()
}

impl Profile {
    /// Профиль, приведённый к допустимому виду (`EnsureValid`): вид — человек, возраст в пределах,
    /// чистое имя (пустое — `DEFAULT_NAME`), тон кожи 0…100, неизвестные стили убраны.
    pub fn validated(&self, catalog: &Catalog) -> Profile {
        let mut name = clean_name(&self.name);
        if name.is_empty() {
            name = DEFAULT_NAME.into();
        }
        let a = &self.appearance;
        let style = |id: &str, known: &BTreeSet<String>| {
            if known.contains(id) {
                id.to_string()
            } else {
                String::new()
            }
        };
        Profile {
            name,
            flavor_text: clean_flavor(&self.flavor_text),
            species: SPECIES_HUMAN.into(),
            age: self.age.clamp(MIN_AGE, MAX_AGE),
            sex: self.sex,
            gender: self.gender,
            appearance: Appearance {
                skin_tone: if a.skin_tone.is_finite() {
                    a.skin_tone.clamp(0.0, 100.0)
                } else {
                    DEFAULT_SKIN_TONE
                },
                eyes: a.eyes,
                hair: style(&a.hair, &catalog.hair),
                hair_color: a.hair_color,
                facial_hair: style(&a.facial_hair, &catalog.facial_hair),
                facial_hair_color: a.facial_hair_color,
            },
            clothing: self.clothing,
            backpack: self.backpack,
        }
    }
}

/// Не больше `MAX_SLOTS` профилей и выбранный слот в их пределах.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slots {
    pub profiles: Vec<Profile>,
    pub selected: usize,
}

impl Default for Slots {
    fn default() -> Self {
        Self {
            profiles: vec![Profile::default()],
            selected: 0,
        }
    }
}

impl Slots {
    pub fn current(&self) -> &Profile {
        &self.profiles[self.selected.min(self.profiles.len() - 1)]
    }

    /// Заменить профиль слота `slot` (или добавить новый, если слот следующий по порядку и есть
    /// место). Профиль проверяется. `false` — слот вне пределов.
    pub fn set(&mut self, slot: usize, profile: &Profile, catalog: &Catalog) -> bool {
        let valid = profile.validated(catalog);
        if slot < self.profiles.len() {
            self.profiles[slot] = valid;
            true
        } else if slot == self.profiles.len() && slot < MAX_SLOTS {
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

    fn catalog() -> Catalog {
        Catalog {
            hair: ["HumanHairAfro".to_string()].into(),
            facial_hair: ["HumanFacialHairBeard".to_string()].into(),
        }
    }

    #[test]
    fn the_real_catalog_has_the_hairstyles_and_beards_of_the_fork() {
        let mut registry = prototypes::PrototypeRegistry::new();
        registry
            .load_ron_str(
                include_str!("../../../content/prototypes/customization.ron"),
                "customization",
            )
            .unwrap();
        registry.validate().unwrap();
        let c = Catalog::from_registry(&registry, "ss14").unwrap();
        assert_eq!(c.hair.len(), 203);
        assert_eq!(c.facial_hair.len(), 35);
        assert!(c.hair.contains("HumanHairAfro") && c.facial_hair.contains("HumanFacialHairChin"));
    }

    #[test]
    fn a_name_is_cleaned_cut_and_capitalized_like_ss14() {
        assert_eq!(clean_name("  john   doe "), "John   Doe");
        assert_eq!(clean_name("j0hn <b>doe</b>!"), "J0hn Bdoeb");
        assert_eq!(clean_name("анна-мария иванова"), "Анна-мария Иванова");
        assert_eq!(clean_name("o'neil"), "O'neil");
        assert_eq!(clean_name(&"a".repeat(100)).chars().count(), MAX_NAME_LEN);
        assert_eq!(clean_name("!!!"), "");
    }

    #[test]
    fn flavor_text_loses_markup_and_is_cut() {
        assert_eq!(
            clean_flavor("Tall [color=red]and[/color] calm"),
            "Tall and calm"
        );
        assert_eq!(
            clean_flavor(&"x".repeat(2000)).chars().count(),
            MAX_FLAVOR_LEN
        );
        assert_eq!(clean_flavor("a\u{7}b"), "ab", "управляющие знаки убраны");
    }

    #[test]
    fn skin_tones_follow_the_human_toned_strategy() {
        assert_eq!(
            skin_color(20.0),
            [255, 225, 204],
            "светлый: оттенок 25°, насыщенность 20"
        );
        let light = skin_color(0.0);
        let dark = skin_color(100.0);
        assert!(light[0] > 250 && light[1] > light[2], "{light:?}");
        assert!(dark[0] < 60, "тёмный: яркость 20%: {dark:?}");
        assert_eq!(
            skin_color(-5.0),
            skin_color(0.0),
            "вне диапазона зажимается"
        );
        assert_eq!(skin_color(f32::NAN), skin_color(DEFAULT_SKIN_TONE));
    }

    #[test]
    fn a_profile_is_brought_to_a_valid_shape() {
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
        let ok = bad.validated(&catalog());
        assert_eq!(ok.name, DEFAULT_NAME, "пустое имя заменено");
        assert_eq!(ok.age, MIN_AGE);
        assert_eq!(ok.species, SPECIES_HUMAN);
        assert_eq!(ok.appearance.skin_tone, 100.0);
        assert_eq!(ok.appearance.hair, "", "неизвестная причёска убрана");
        assert_eq!(ok.appearance.facial_hair, "HumanFacialHairBeard");
        assert_eq!(
            ok.validated(&catalog()),
            ok,
            "повторная проверка ничего не меняет"
        );
        let old = Profile {
            age: 200,
            ..Profile::default()
        };
        assert_eq!(old.validated(&catalog()).age, MAX_AGE);
    }

    #[test]
    fn at_most_five_slots_and_the_last_profile_stays() {
        let mut s = Slots::default();
        let c = catalog();
        for i in 1..MAX_SLOTS {
            assert!(s.set(i, &Profile::default(), &c), "слот {i}");
        }
        assert_eq!(s.profiles.len(), MAX_SLOTS);
        assert!(!s.set(MAX_SLOTS, &Profile::default(), &c), "шестого нет");
        assert!(
            !s.set(MAX_SLOTS + 3, &Profile::default(), &c),
            "и через слот тоже"
        );
        assert!(s.select(4));
        assert!(s.delete(4));
        assert_eq!(s.selected, 3, "выбор не выходит за список");
        while s.profiles.len() > 1 {
            assert!(s.delete(0));
        }
        assert!(!s.delete(0), "последний не удаляется");
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
        let text = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Profile>(&text).unwrap(), p);
    }
}
