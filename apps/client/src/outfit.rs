//! Одежда и предметы в руках на спрайте персонажа: слои из RSI предметов (`equipped-*`,
//! `inhand-left/right`) накладываются на тело для каждого из четырёх направлений.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use astrocore_client::{LocalBody, Net, RemoteSprites};
use astrocore_render::BillboardQuad;
use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use gameplay::character::{LOOK_COMPONENT, Look, LookLayer, decode_look};
use gameplay::{HANDS_COMPONENT, WORN_COMPONENT, decode_hands, decode_worn};

const CELL: u32 = 32;

/// Порядок слоёв надетого снизу вверх (порядок слоёв гуманоида в SS14; имена слоёв — флаги `equipped-*`).
const SLOT_ORDER: &[(&str, &str)] = &[
    ("shoes", "FEET"),
    ("jumpsuit", "INNERCLOTHING"),
    ("gloves", "HAND"),
    ("id", "IDCARD"),
    ("outerClothing", "OUTERCLOTHING"),
    ("belt", "BELT"),
    ("back", "BACKPACK"),
    ("neck", "NECK"),
    ("mask", "MASK"),
    ("eyes", "EYES"),
    ("ears", "EARS"),
    ("head", "HELMET"),
];

#[derive(Resource)]
pub struct Outfits {
    root: PathBuf,
    base: image::RgbaImage,
    /// Прототип предмета → картинка значка (путь относительно `root`), из манифеста рендера.
    icons: HashMap<String, String>,
    png: HashMap<PathBuf, Option<image::RgbaImage>>,
    composed: HashMap<Key, Handle<Image>>,
    applied: HashMap<Entity, Key>,
    /// Тело, покрашенное цветом кожи (по цвету).
    bodies: HashMap<[u8; 3], Option<image::RgbaImage>>,
}

/// Что определяет вид листа: внешность и надетые слои.
type Key = (Option<Look>, Vec<PathBuf>);

impl Outfits {
    pub fn new(root: PathBuf, base: image::RgbaImage, icons: HashMap<String, String>) -> Self {
        Self {
            root,
            base,
            icons,
            png: HashMap::new(),
            composed: HashMap::new(),
            applied: HashMap::new(),
            bodies: HashMap::new(),
        }
    }

    /// Путь картинки состояния рядом со значком предмета, если такой файл есть.
    fn sibling(&self, proto: &str, file: impl Fn(&str) -> Vec<String>) -> Option<PathBuf> {
        let icon = Path::new(self.icons.get(proto)?);
        let dir = icon.parent()?;
        let stem = icon.file_stem()?.to_str()?;
        file(stem)
            .into_iter()
            .map(|f| dir.join(f))
            .find(|p| self.root.join(p).is_file())
    }

    fn layers(&self, worn: &[(String, String)], hands: &gameplay::Hands) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for (slot, suffix) in SLOT_ORDER {
            if let Some((_, proto)) = worn.iter().find(|(s, _)| s == slot)
                && let Some(p) = self.sibling(proto, |_| vec![format!("equipped-{suffix}.png")])
            {
                out.push(p);
            }
        }
        for (_, name, item) in hands.iter() {
            let Some(it) = item else { continue };
            let side = if name.starts_with('l') {
                "left"
            } else {
                "right"
            };
            let proto = it.prototype.as_str();
            if let Some(p) = self.sibling(proto, |stem| {
                vec![
                    format!("{stem}-inhand-{side}.png"),
                    format!("inhand-{side}.png"),
                ]
            }) {
                out.push(p);
            }
        }
        out
    }

    fn load(&mut self, rel: &Path) -> Option<&image::RgbaImage> {
        let root = self.root.clone();
        self.png
            .entry(rel.to_path_buf())
            .or_insert_with(|| image::open(root.join(rel)).ok().map(|i| i.to_rgba8()))
            .as_ref()
    }

    /// Наложить картинку 2×2 направлений на лист, умножив цвет на `tint`.
    fn overlay(&mut self, sheet: &mut image::RgbaImage, rel: &Path, tint: Option<[u8; 3]>) {
        let Some(part) = self.load(rel) else { return };
        if part.width() < CELL * 2 || part.height() < CELL * 2 {
            return;
        }
        for dir in 0..4u32 {
            let mut cell =
                image::imageops::crop_imm(part, (dir % 2) * CELL, (dir / 2) * CELL, CELL, CELL)
                    .to_image();
            if let Some(t) = tint {
                for px in cell.pixels_mut() {
                    for (c, k) in px.0.iter_mut().take(3).zip(t) {
                        *c = (u16::from(*c) * u16::from(k) / 255) as u8;
                    }
                }
            }
            image::imageops::overlay(sheet, &cell, i64::from(dir * CELL), 0);
        }
    }

    /// Лист для превью в редакторе: внешность без одежды.
    pub(crate) fn preview_sheet(&mut self, look: &Look) -> image::RgbaImage {
        self.compose_look(Some(look), &[])
    }

    /// Лист персонажа: тело цвета кожи, глаза, борода и причёска (цвета из профиля), затем одежда.
    fn compose_look(&mut self, look: Option<&Look>, layers: &[PathBuf]) -> image::RgbaImage {
        let Some(look) = look else {
            return self.compose(layers);
        };
        let root = self.root.clone();
        let body = self
            .bodies
            .entry(look.skin)
            .or_insert_with(|| crate::body::compose(&root, look.skin))
            .clone()
            .unwrap_or_else(|| self.base.clone());
        let mut sheet = body;
        self.overlay(
            &mut sheet,
            Path::new("Textures/Mobs/Customization/eyes.rsi/eyes.png"),
            Some(look.eyes),
        );
        for layer in [&look.facial_hair, &look.hair].into_iter().flatten() {
            let LookLayer { png, color } = layer;
            let rel = Path::new("Textures").join(png);
            self.overlay(&mut sheet, &rel, Some(*color));
        }
        for rel in layers {
            self.overlay(&mut sheet, rel, None);
        }
        sheet
    }

    fn compose(&mut self, layers: &[PathBuf]) -> image::RgbaImage {
        let mut sheet = self.base.clone();
        for rel in layers {
            let Some(part) = self.load(rel) else { continue };
            if part.width() < CELL * 2 || part.height() < CELL * 2 {
                continue;
            }
            for dir in 0..4u32 {
                let cell =
                    image::imageops::crop_imm(part, (dir % 2) * CELL, (dir / 2) * CELL, CELL, CELL)
                        .to_image();
                image::imageops::overlay(&mut sheet, &cell, i64::from(dir * CELL), 0);
            }
        }
        sheet
    }
}

fn to_image(sheet: image::RgbaImage) -> Image {
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
    image
}

/// Привести спрайты игроков в соответствие с тем, что на них надето и что у них в руках.
#[allow(clippy::too_many_arguments)]
pub fn sync_outfits(
    net: Option<NonSend<Net>>,
    remote: Res<RemoteSprites>,
    local: Res<LocalBody>,
    outfits: Option<ResMut<Outfits>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    children: Query<&Children>,
    quads: Query<&MeshMaterial3d<StandardMaterial>, With<BillboardQuad>>,
) {
    let (Some(net), Some(mut outfits)) = (net, outfits) else {
        return;
    };
    let mut targets: Vec<(astrocore_core::NetworkEntityId, Entity)> =
        remote.0.iter().map(|(i, e)| (*i, *e)).collect();
    if let (Some(me), Some(e)) = (net.core.net_id(), local.0) {
        targets.push((me, e));
    }
    outfits
        .applied
        .retain(|e, _| targets.iter().any(|(_, t)| t == e));
    for (id, entity) in targets {
        let worn = net
            .core
            .component(id, WORN_COMPONENT)
            .and_then(decode_worn)
            .unwrap_or_default();
        let hands = net
            .core
            .component(id, HANDS_COMPONENT)
            .and_then(decode_hands);
        let look = net.core.component(id, LOOK_COMPONENT).and_then(decode_look);
        let key: Key = (
            look,
            hands
                .as_ref()
                .map_or_else(Vec::new, |h| outfits.layers(&worn, h)),
        );
        if outfits.applied.get(&entity) == Some(&key) {
            continue;
        }
        let handle = match outfits.composed.get(&key) {
            Some(h) => h.clone(),
            None => {
                let sheet = outfits.compose_look(key.0.as_ref(), &key.1);
                let h = images.add(to_image(sheet));
                outfits.composed.insert(key.clone(), h.clone());
                h
            }
        };
        let Ok(kids) = children.get(entity) else {
            continue;
        };
        let mut applied = false;
        for &child in kids {
            if let Ok(m) = quads.get(child)
                && let Some(mut mat) = materials.get_mut(&m.0)
            {
                mat.base_color_texture = Some(handle.clone());
                applied = true;
            }
        }
        if applied {
            outfits.applied.insert(entity, key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outfits() -> Outfits {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ss14");
        let base = crate::body::compose(&root, crate::body::DEFAULT_SKIN).expect("части тела");
        Outfits::new(root, base, HashMap::new())
    }

    fn look(skin: [u8; 3], hair: Option<&str>) -> Look {
        Look {
            skin,
            eyes: [0x3a, 0x5f, 0x9e],
            facial_hair: None,
            hair: hair.map(|png| LookLayer {
                png: png.into(),
                color: [0xc2, 0x9a, 0x5a],
            }),
        }
    }

    #[test]
    fn the_skin_colour_tints_the_body_and_hair_and_eyes_are_drawn_over_it() {
        let mut o = outfits();
        let light = o.compose_look(Some(&look([255, 225, 204], None)), &[]);
        let dark = o.compose_look(Some(&look([90, 60, 40], None)), &[]);
        assert_ne!(light, dark, "кожа другого цвета");
        let bald = light.clone();
        let styled = o.compose_look(
            Some(&look(
                [255, 225, 204],
                Some("Mobs/Customization/human_hair.rsi/afro.png"),
            )),
            &[],
        );
        assert_ne!(bald, styled, "причёска меняет лист");
        // Глаза окрашены: в листе есть пиксель цвета глаз с учётом множителя на серый спрайт.
        assert!(
            styled
                .pixels()
                .any(|p| p[3] > 0 && u16::from(p[2]) > u16::from(p[0]) + 20),
            "глаза синие"
        );
    }

    /// Сохранить картинку со случайными персонажами: `ASTRO_DUMP_LOOKS=/путь.png cargo test dump_looks`.
    #[test]
    fn dump_looks() {
        let Some(out) = std::env::var_os("ASTRO_DUMP_LOOKS") else {
            return;
        };
        let mut registry = prototypes::PrototypeRegistry::new();
        registry
            .load_ron_str(
                include_str!("../../../content/prototypes/customization.ron"),
                "customization",
            )
            .unwrap();
        registry.validate().unwrap();
        let rules = gameplay::character::Rules::from_registry(&registry, "ss14").unwrap();
        let mut o = outfits();
        let (cols, rows) = (8u32, 4u32);
        let scale = 3u32;
        let mut sheet = image::RgbaImage::from_pixel(
            cols * CELL * scale,
            rows * CELL * scale,
            image::Rgba([60, 60, 60, 255]),
        );
        for i in 0..cols * rows {
            let p = gameplay::character::Profile::random(u64::from(i) + 1, &rules);
            let look = p.look(&rules);
            let body = o.compose_look(Some(&look), &[]);
            // Лицом к зрителю: первое направление листа.
            let front = image::imageops::crop_imm(&body, 0, 0, CELL, CELL).to_image();
            let big = image::imageops::resize(
                &front,
                CELL * scale,
                CELL * scale,
                image::imageops::FilterType::Nearest,
            );
            image::imageops::overlay(
                &mut sheet,
                &big,
                i64::from((i % cols) * CELL * scale),
                i64::from((i / cols) * CELL * scale),
            );
        }
        sheet.save(out).unwrap();
    }
}
