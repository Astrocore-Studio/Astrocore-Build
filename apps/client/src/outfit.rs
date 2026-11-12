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
    composed: HashMap<Vec<PathBuf>, Handle<Image>>,
    applied: HashMap<Entity, Vec<PathBuf>>,
}

impl Outfits {
    pub fn new(root: PathBuf, base: image::RgbaImage, icons: HashMap<String, String>) -> Self {
        Self {
            root,
            base,
            icons,
            png: HashMap::new(),
            composed: HashMap::new(),
            applied: HashMap::new(),
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
        let key = hands
            .as_ref()
            .map_or_else(Vec::new, |h| outfits.layers(&worn, h));
        if outfits.applied.get(&entity) == Some(&key) {
            continue;
        }
        let handle = match outfits.composed.get(&key) {
            Some(h) => h.clone(),
            None => {
                let sheet = outfits.compose(&key);
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
