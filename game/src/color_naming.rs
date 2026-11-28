//! Название цвета словами («тёмно-красный», «бледно-голубой») по цвету в Oklch: оттенок по
//! углу, насыщенность и светлота по порогам. Образец — `ColorNaming` Robust (SS14 @ 2f4a2aad).

use astrocore_loc::LocArg;
use astrocore_math::Color;
use bevy_ecs::world::World;

use crate::examine::loc;

const HUE_NAMES: [(f32, &str); 9] = [
    (0.0, "color-pink"),
    (15.0, "color-red"),
    (45.0, "color-orange"),
    (90.0, "color-yellow"),
    (135.0, "color-green"),
    (180.0, "color-cyan"),
    (240.0, "color-blue"),
    (285.0, "color-purple"),
    (330.0, "color-pink"),
];
const HUE_FALLBACK: (f32, &str) = (361.0, "color-pink");

const BROWN_LIGHTNESS: f32 = 0.675;
const VERY_DARK: f32 = 0.25;
const DARK: f32 = 0.5;
const NEUTRAL: f32 = 0.7;
const LIGHT: f32 = 0.85;
const LOW_CHROMA: f32 = 0.07;
const HIGH_CHROMA: f32 = 0.16;
const LIGHT_LOW_CHROMA: f32 = 0.6;
const WHITE_LIGHTNESS: f32 = 0.99;
const BLACK_LIGHTNESS: f32 = 0.01;
const GRAY_CHROMA: f32 = 0.01;

/// sRGB → линейный (`Color.FromSrgb`).
fn linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Светлота, насыщенность и оттенок в радианах (`ToLch(ToLab(FromSrgb))`).
pub fn oklch(color: Color) -> (f32, f32, f32) {
    let (r, g, b) = (linear(color.r), linear(color.g), linear(color.b));
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    let lightness = 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s;
    let a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
    let bb = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;
    let chroma = (a * a + bb * bb).sqrt();
    let mut hue = bb.atan2(a);
    if hue < 0.0 {
        hue += 2.0 * std::f32::consts::PI;
    }
    (lightness, chroma, hue)
}

fn has(world: &World, id: &str) -> bool {
    world
        .get_resource::<astrocore_loc::Loc>()
        .is_some_and(|l| l.has(id))
}

/// Оттенок словами и поправленная светлота (`DescribeHue`).
fn describe_hue(world: &World, lightness: f32, hue: f32) -> (String, f32) {
    for (i, (degrees, original)) in HUE_NAMES.iter().enumerate() {
        let (next_degrees, next_name) = HUE_NAMES.get(i + 1).copied().unwrap_or(HUE_FALLBACK);
        let (start, end) = (degrees.to_radians(), next_degrees.to_radians());
        if start > hue || hue >= end {
            continue;
        }
        let mut name = *original;
        let mut adjusted = lightness;
        if name == "color-orange" && lightness <= BROWN_LIGHTNESS {
            name = "color-brown";
        } else if name == "color-orange" {
            adjusted = lightness - BROWN_LIGHTNESS + DARK;
        }
        if hue >= (start + end) / 2.0 && *original != next_name {
            let combined = format!("{name}-{next_name}");
            if has(world, &combined) {
                return (loc(world, &combined, &[]), adjusted);
            }
            let a = loc(world, name, &[]);
            let b = loc(world, next_name, &[]);
            return (
                loc(
                    world,
                    "color-mixed-hue",
                    &[("a", LocArg::from(a)), ("b", LocArg::from(b))],
                ),
                adjusted,
            );
        }
        return (loc(world, name, &[]), adjusted);
    }
    (loc(world, "color-unknown", &[]), lightness)
}

/// Цвет словами (`Describe`).
pub fn describe(world: &World, color: Color) -> String {
    let (lightness, chroma, hue) = oklch(color);
    if lightness >= WHITE_LIGHTNESS {
        return loc(world, "color-white", &[]);
    }
    if lightness <= BLACK_LIGHTNESS {
        return loc(world, "color-black", &[]);
    }
    let (mut hue_text, lightness) = describe_hue(world, lightness, hue);
    let mut chroma_text = if chroma <= LOW_CHROMA {
        Some(if lightness >= LIGHT_LOW_CHROMA {
            "color-pale"
        } else {
            "color-gray-adjective"
        })
    } else if chroma >= HIGH_CHROMA {
        Some("color-strong")
    } else {
        None
    };
    let lightness_text = if lightness < VERY_DARK {
        Some("color-very-dark")
    } else if lightness < DARK {
        Some("color-dark")
    } else if lightness < NEUTRAL {
        None
    } else if lightness < LIGHT {
        Some("color-light")
    } else {
        Some("color-very-light")
    };
    if chroma <= GRAY_CHROMA {
        hue_text = loc(world, "color-gray", &[]);
        chroma_text = None;
    }
    let text = |id: &str| loc(world, id, &[]);
    match (chroma_text, lightness_text) {
        (Some(c), Some(l)) => loc(
            world,
            "color-hue-chroma-lightness",
            &[
                ("hue", hue_text.into()),
                ("chroma", text(c).into()),
                ("lightness", text(l).into()),
            ],
        ),
        (Some(c), None) => loc(
            world,
            "color-hue-chroma",
            &[("hue", hue_text.into()), ("chroma", text(c).into())],
        ),
        (None, Some(l)) => loc(
            world,
            "color-hue-lightness",
            &[("hue", hue_text.into()), ("lightness", text(l).into())],
        ),
        (None, None) => hue_text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklch_matches_known_colors() {
        let (l, c, _) = oklch(Color::WHITE);
        assert!((l - 1.0).abs() < 1e-3 && c < 1e-3);
        // Чистый красный: L ≈ 0,628, C ≈ 0,258, h ≈ 29°.
        let (l, c, h) = oklch(Color::rgba(1.0, 0.0, 0.0, 1.0));
        assert!((l - 0.628).abs() < 1e-2);
        assert!((c - 0.258).abs() < 1e-2);
        assert!((h.to_degrees() - 29.2).abs() < 1.0);
    }
}
