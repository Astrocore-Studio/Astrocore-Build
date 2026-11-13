//! Стрелковое оружие (`GunSystem`, `BallisticAmmoProvider`, `ChamberMagazineAmmoProvider` SS14).
//!
//! Оружие стреляет патронами: либо из магазина (предмет со счётчиком патронов) с патронником,
//! либо из встроенного хранилища (дробовики, револьверы). Патрон несёт снаряд (`CartridgeAmmo`),
//! у снаряда — урон по видам (`Projectile.damage`); дробь разбрасывает несколько снарядов
//! (`ProjectileSpread`). Выстрел мгновенный: луч от груди стрелка до первой преграды, попадание
//! в человека по росту: голова, торс или ноги.
//!
//! Не перенесено: полёт снаряда (скорость), рикошет и пробитие, рост разброса от стрельбы,
//! режимы огня, сошки и зарядка энергооружия (батарейное оружие в билде не стреляет), стреляные
//! гильзы, звук осечки.

use astrocore_math::glam::Vec3;
use serde::{Deserialize, Serialize};

pub const AMMO_COMPONENT: &str = "build.ammo";
pub const AMMO_MAX_BYTES: usize = 128;
/// Дальность выстрела, м.
pub const RANGE_M: f32 = 40.0;
/// Радиус цели для попадания: чуть шире капсулы (0,17 м), чтобы целиться было можно.
pub const HIT_RADIUS_M: f32 = 0.3;
/// Рост цели, м.
pub const BODY_HEIGHT_M: f32 = 1.0;

/// Что делает оружие (`GunComponent` + поставщик патронов).
#[derive(Debug, Clone, PartialEq)]
pub struct GunSpec {
    pub fire_rate: f32,
    pub min_angle: f32,
    pub max_angle: f32,
    /// Звук выстрела (путь внутри `Audio`).
    pub sound: String,
    /// Стартовый магазин (id SS14) для оружия с магазином; пусто — встроенное хранилище.
    pub magazine: String,
    /// Патрон встроенного хранилища и его вместимость.
    pub cartridge: String,
    pub capacity: u32,
    /// Автоматический огонь (`selectedMode: FullAuto`).
    pub auto: bool,
}

/// Магазин: патрон и вместимость (`BallisticAmmoProvider` предмета-магазина).
#[derive(Debug, Clone, PartialEq)]
pub struct MagSpec {
    pub cartridge: String,
    pub capacity: u32,
}

/// Заряд оружия на конкретном экземпляре.
#[derive(Debug, Clone, PartialEq)]
pub struct GunState {
    /// Вставленный магазин: прототип предмета, патрон, число патронов.
    pub mag: Option<(astrocore_core::PrototypeId, String, u32)>,
    /// Патрон в патроннике (магазинное оружие).
    pub chamber: Option<String>,
    /// Встроенное хранилище: патрон и число.
    pub internal: (String, u32),
    /// Когда можно стрелять снова, с (по часам модуля).
    pub next_shot: f32,
}

impl GunState {
    /// Всего патронов, готовых к выстрелу: патронник, магазин и встроенное хранилище.
    pub fn ammo(&self) -> u32 {
        u32::from(self.chamber.is_some()) + self.mag.as_ref().map_or(0, |m| m.2) + self.internal.1
    }

    /// Взять патрон для выстрела: из патронника (потом новый из магазина) или из хранилища.
    pub fn take_round(&mut self) -> Option<String> {
        if let Some(c) = self.chamber.take() {
            self.chamber = self.feed();
            return Some(c);
        }
        if self.internal.1 > 0 {
            self.internal.1 -= 1;
            return Some(self.internal.0.clone());
        }
        None
    }

    /// Дослать патрон из магазина в патронник.
    pub fn feed(&mut self) -> Option<String> {
        let m = self.mag.as_mut()?;
        (m.2 > 0).then(|| {
            m.2 -= 1;
            m.1.clone()
        })
    }
}

/// Для клиента: заряд оружия в активной руке.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AmmoView {
    /// В активной руке оружие.
    pub gun: bool,
    /// Патронов сейчас и вместимость магазина или хранилища.
    pub ammo: u32,
    pub capacity: u32,
    /// Автоматический огонь.
    pub auto: bool,
}

pub fn encode_ammo(v: &AmmoView) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

pub fn decode_ammo(bytes: &[u8]) -> Option<AmmoView> {
    if bytes.is_empty() || bytes.len() > AMMO_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// Пересечение луча с вертикальным цилиндром (человек): расстояние до входа или `None`.
pub fn ray_cylinder(origin: Vec3, dir: Vec3, base: Vec3, radius: f32, height: f32) -> Option<f32> {
    // В плоскости XZ: |o + t·d − c|² = r².
    let (ox, oz) = (origin.x - base.x, origin.z - base.z);
    let (dx, dz) = (dir.x, dir.z);
    let a = dx * dx + dz * dz;
    let (t0, t1) = if a < 1e-9 {
        // Луч идёт вертикально: попадёт, если он внутри круга.
        if ox * ox + oz * oz > radius * radius {
            return None;
        }
        (0.0, f32::INFINITY)
    } else {
        let b = 2.0 * (ox * dx + oz * dz);
        let c = ox * ox + oz * oz - radius * radius;
        let disc = b * b - 4.0 * a * c;
        if disc < 0.0 {
            return None;
        }
        let s = disc.sqrt();
        ((-b - s) / (2.0 * a), (-b + s) / (2.0 * a))
    };
    // Участок луча внутри цилиндра по высоте.
    let (mut lo, mut hi) = (t0.max(0.0), t1);
    if dir.y.abs() > 1e-9 {
        let (y0, y1) = (
            (base.y - origin.y) / dir.y,
            (base.y + height - origin.y) / dir.y,
        );
        lo = lo.max(y0.min(y1));
        hi = hi.min(y0.max(y1));
    } else if origin.y < base.y || origin.y > base.y + height {
        return None;
    }
    (lo <= hi).then_some(lo)
}

/// Куда в тело пришёлся выстрел по высоте точки попадания над ступнями (доля роста).
pub fn hit_part(height_fraction: f32, left_side: bool) -> &'static str {
    if height_fraction > 0.8 {
        "Head"
    } else if height_fraction > 0.45 {
        "Torso"
    } else if left_side {
        "LeftLeg"
    } else {
        "RightLeg"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrocore_core::PrototypeId;

    fn state(mag: u32) -> GunState {
        GunState {
            mag: Some((
                PrototypeId::new("ss14:mag").unwrap(),
                "CartridgePistol".into(),
                mag,
            )),
            chamber: Some("CartridgePistol".into()),
            internal: (String::new(), 0),
            next_shot: 0.0,
        }
    }

    #[test]
    fn a_magazine_gun_fires_the_chamber_then_refeeds_until_empty() {
        let mut g = state(2);
        assert_eq!(g.ammo(), 3);
        assert!(g.take_round().is_some());
        assert_eq!(g.ammo(), 2, "патронник пополнен из магазина");
        assert!(g.take_round().is_some());
        assert!(g.take_round().is_some());
        assert_eq!(g.take_round(), None);
        assert_eq!(g.ammo(), 0);
    }

    #[test]
    fn an_internal_store_is_spent_round_by_round() {
        let mut g = GunState {
            mag: None,
            chamber: None,
            internal: ("ShellShotgun".into(), 2),
            next_shot: 0.0,
        };
        assert_eq!(g.take_round().as_deref(), Some("ShellShotgun"));
        assert_eq!(g.ammo(), 1);
    }

    #[test]
    fn the_ray_hits_a_person_in_front_and_misses_to_the_side() {
        let (o, d) = (Vec3::new(0.0, 0.55, 0.0), Vec3::new(0.0, 0.0, 1.0));
        let t = ray_cylinder(o, d, Vec3::new(0.0, 0.2, 5.0), 0.3, 1.0).unwrap();
        assert!((t - 4.7).abs() < 1e-3, "{t}");
        assert!(ray_cylinder(o, d, Vec3::new(1.0, 0.2, 5.0), 0.3, 1.0).is_none());
        assert!(ray_cylinder(o, -d, Vec3::new(0.0, 0.2, 5.0), 0.3, 1.0).is_none());
        // Выше головы тоже мимо.
        assert!(
            ray_cylinder(
                Vec3::new(0.0, 3.0, 0.0),
                d,
                Vec3::new(0.0, 0.2, 5.0),
                0.3,
                1.0
            )
            .is_none()
        );
    }

    #[test]
    fn hits_land_by_height() {
        assert_eq!(hit_part(0.9, true), "Head");
        assert_eq!(hit_part(0.6, true), "Torso");
        assert_eq!(hit_part(0.2, true), "LeftLeg");
        assert_eq!(hit_part(0.2, false), "RightLeg");
    }

    #[test]
    fn ammo_views_survive_the_wire() {
        let v = AmmoView {
            gun: true,
            ammo: 7,
            capacity: 11,
            auto: false,
        };
        assert_eq!(decode_ammo(&encode_ammo(&v)), Some(v));
    }
}
