//! Тело персонажа: человек собирается из частей `parts.rsi` SS14 (ноги, торс, голова, руки).

use std::path::Path;

use astrocore_client::PlayerSprite;
use astrocore_render::SpriteSheet;
use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

const PARTS: &str = "Textures/Mobs/Species/Human/parts.rsi";
/// Части снизу вверх: ноги и ступни под торсом, голова, затем руки и кисти.
const LAYERS: &[&str] = &[
    "l_foot", "r_foot", "l_leg", "r_leg", "torso_m", "head_m", "l_arm", "r_arm", "l_hand", "r_hand",
];
/// Цвет кожи: части в RSI серые, SS14 красит их оттенком кожи.
const SKIN: [u8; 3] = [238, 190, 158];
const CELL: u32 = 32;

/// Собрать лист 4×1 (S, N, E, W) из частей. Каждый PNG частей — сетка 2×2 кадров направлений.
pub fn compose(root: &Path) -> Option<image::RgbaImage> {
    let mut sheet = image::RgbaImage::new(CELL * 4, CELL);
    for name in LAYERS {
        let part = image::open(root.join(PARTS).join(format!("{name}.png")))
            .ok()?
            .to_rgba8();
        for dir in 0..4u32 {
            let (sx, sy) = ((dir % 2) * CELL, (dir / 2) * CELL);
            for y in 0..CELL {
                for x in 0..CELL {
                    let mut px = *part.get_pixel(sx + x, sy + y);
                    if px[3] == 0 {
                        continue;
                    }
                    if !name.ends_with("_foot") {
                        for (c, s) in px.0.iter_mut().zip(SKIN) {
                            *c = (u16::from(*c) * u16::from(s) / 255) as u8;
                        }
                    }
                    let dst = sheet.get_pixel_mut(dir * CELL + x, y);
                    *dst = px;
                }
            }
        }
    }
    Some(sheet)
}

/// Подставить клиенту спрайт игрока; без файлов частей остаётся демонстрационный человечек.
pub fn install(root: &Path, images: &mut Assets<Image>, commands: &mut Commands) {
    let Some(sheet) = compose(root) else {
        eprintln!("части тела не найдены в {}", root.display());
        return;
    };
    let mut image = Image::new(
        Extent3d {
            width: sheet.width(),
            height: sheet.height(),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        sheet.into_raw(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();
    commands.insert_resource(PlayerSprite {
        sheet: SpriteSheet {
            image: images.add(image),
            columns: 4,
        },
        size: Vec2::new(1.0, 1.0),
    });
}
