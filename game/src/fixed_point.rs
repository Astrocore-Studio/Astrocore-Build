//! Число с двумя знаками после запятой, как в SS14: урон и пороги считаются в сотых без
//! накопления ошибки округления. Образец — `Content.Shared/FixedPoint/FixedPoint2.cs`
//! SS14 @ 2f4a2aad.

use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const SHIFT: i32 = 100;
const FLOAT_EPSILON: f64 = 0.00001;

/// Число в сотых (`FixedPoint2`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FixedPoint2(pub i32);

/// Сдвиг от нуля перед отбрасыванием дробной части (`ApplyFloatEpsilon`).
fn with_epsilon(value: f64) -> f64 {
    if value == 0.0 {
        0.0
    } else {
        value + FLOAT_EPSILON * value.signum()
    }
}

impl FixedPoint2 {
    pub const ZERO: FixedPoint2 = FixedPoint2(0);
    /// Наименьший шаг (`Epsilon`).
    pub const EPSILON: FixedPoint2 = FixedPoint2(1);
    pub const MAX: FixedPoint2 = FixedPoint2(i32::MAX);

    /// Целое число (`New(int)`).
    pub const fn new(value: i32) -> Self {
        Self(value * SHIFT)
    }

    /// Из дробного с отбрасыванием лишних знаков (`New(float)`).
    pub fn from_f32(value: f32) -> Self {
        Self::from_f64(f64::from(value))
    }

    pub fn from_f64(value: f64) -> Self {
        Self(with_epsilon(value * f64::from(SHIFT)) as i32)
    }

    pub const fn from_cents(value: i32) -> Self {
        Self(value)
    }

    pub fn float(self) -> f32 {
        self.0 as f32 / SHIFT as f32
    }

    pub fn double(self) -> f64 {
        f64::from(self.0) / f64::from(SHIFT)
    }

    pub fn int(self) -> i32 {
        self.0 / SHIFT
    }

    pub fn max(self, other: Self) -> Self {
        Ord::max(self, other)
    }

    pub fn min(self, other: Self) -> Self {
        Ord::min(self, other)
    }

    pub fn clamp(self, low: Self, high: Self) -> Self {
        Ord::clamp(self, low, high)
    }

    pub fn abs(self) -> Self {
        Self(self.0.abs())
    }

    /// Умножение на дробное (`operator *(FixedPoint2, float)`).
    pub fn scale(self, factor: f32) -> Self {
        Self(with_epsilon(f64::from(self.0) * f64::from(factor)) as i32)
    }
}

impl Add for FixedPoint2 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self(self.0 + other.0)
    }
}

impl AddAssign for FixedPoint2 {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl Sub for FixedPoint2 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self(self.0 - other.0)
    }
}

impl SubAssign for FixedPoint2 {
    fn sub_assign(&mut self, other: Self) {
        self.0 -= other.0;
    }
}

impl Neg for FixedPoint2 {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl Mul for FixedPoint2 {
    type Output = Self;
    fn mul(self, other: Self) -> Self {
        Self((i64::from(self.0) * i64::from(other.0) / i64::from(SHIFT)) as i32)
    }
}

impl Mul<i32> for FixedPoint2 {
    type Output = Self;
    fn mul(self, other: i32) -> Self {
        Self(self.0 * other)
    }
}

impl Div for FixedPoint2 {
    type Output = Self;
    fn div(self, other: Self) -> Self {
        if other.0 == 0 {
            return Self::ZERO;
        }
        Self((i64::from(SHIFT) * i64::from(self.0) / i64::from(other.0)) as i32)
    }
}

impl fmt::Display for FixedPoint2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.double();
        if self.0 % SHIFT == 0 {
            write!(f, "{}", self.0 / SHIFT)
        } else {
            write!(f, "{value}")
        }
    }
}

impl Serialize for FixedPoint2 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.serialize_f64(self.double())
        } else {
            serializer.serialize_i32(self.0)
        }
    }
}

impl<'de> Deserialize<'de> for FixedPoint2 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NumberVisitor;

        impl Visitor<'_> for NumberVisitor {
            type Value = FixedPoint2;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("число")
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<FixedPoint2, E> {
                Ok(FixedPoint2::new(value as i32))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<FixedPoint2, E> {
                Ok(FixedPoint2::new(value as i32))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<FixedPoint2, E> {
                Ok(FixedPoint2::from_f32(value as f32))
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<FixedPoint2, E> {
                text.trim()
                    .parse::<f32>()
                    .map(FixedPoint2::from_f32)
                    .map_err(|_| E::custom(format!("не число: `{text}`")))
            }
        }

        if deserializer.is_human_readable() {
            deserializer.deserialize_any(NumberVisitor)
        } else {
            i32::deserialize(deserializer).map(FixedPoint2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_matches_ss14() {
        assert_eq!(FixedPoint2::from_f32(0.1), FixedPoint2(10));
        assert_eq!(FixedPoint2::from_f32(-1.5), FixedPoint2(-150));
        assert_eq!(FixedPoint2::from_f32(1.0 / 3.0), FixedPoint2(33));
        assert_eq!(FixedPoint2::new(10) / FixedPoint2::new(3), FixedPoint2(333));
        assert_eq!(
            FixedPoint2::new(2) * FixedPoint2::from_f32(0.5),
            FixedPoint2::new(1)
        );
        assert_eq!(FixedPoint2::new(5).scale(0.7), FixedPoint2(350));
        assert_eq!(FixedPoint2::from_f32(2.5).to_string(), "2.5");
        let parsed: FixedPoint2 =
            astrocore_data::from_node(&astrocore_data::parse_yaml("1.25").unwrap()).unwrap();
        assert_eq!(parsed, FixedPoint2(125));
    }
}
