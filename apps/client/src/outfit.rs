//! Персонаж: тело с внешностью, одежда и вещи в руках (слои RSI `equipped-*`,
//! `inhand-left/right`) складываются в лист четырёх направлений, а из листа вырезается одна
//! объёмная воксельная модель ростом `gameplay::BODY_HEIGHT_M` (ADR-0002). Модель повёрнута туда,
//! куда смотрит персонаж; плоский лист остаётся у призраков.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use astrocore_client::{LocalBody, Net, RemoteSprites};
use astrocore_render::{BillboardQuad, DirectionalSprite};
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
    composed: HashMap<Option<Look>, Handle<Image>>,
    applied: HashMap<Entity, Key>,
    /// Тело, покрашенное цветом кожи (по цвету).
    bodies: HashMap<[u8; 3], Option<image::RgbaImage>>,
    /// Сетки моделей персонажа по виду (внешность и надетое) и сжатие по горизонтали.
    models: HashMap<Key, Option<(Handle<Mesh>, f32)>>,
    /// Модель каждого персонажа: персонаж → сущность модели.
    spawned: HashMap<Entity, Entity>,
    /// Общий материал моделей (цвет — из вершин).
    material: Option<Handle<StandardMaterial>>,
}

/// Модель персонажа: следует за своим персонажем (`owner`) и смотрит туда же, куда он.
#[derive(Component)]
pub struct BodyModel {
    owner: Entity,
    /// Во сколько раз модель сжата по горизонтали, чтобы ширина тела была `BODY_WIDTH_M`.
    squeeze: f32,
}

/// Что определяет вид персонажа: внешность, надетые слои по порядку и призрак ли он.
type Key = (Option<Look>, Vec<PathBuf>, bool);

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
            models: HashMap::new(),
            spawned: HashMap::new(),
            material: None,
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

    /// Модель персонажа по виду: лист четырёх направлений (тело, внешность, надетое) →
    /// пересечение силуэтов → воксели. Масштаб — рост тела без одежды равен `BODY_HEIGHT_M`,
    /// опора — под ступнями; второе число — сжатие по горизонтали, при котором ширина тела равна
    /// `BODY_WIDTH_M` (спрайты SS14 коренастые: в масштабе роста плечи вышли бы под метр).
    /// `None`, если лист пуст.
    fn body_model(&mut self, key: &Key) -> Option<(astrocore_voxels::VoxelModel, f32)> {
        let (look, layers, _) = key;
        let sheet = self.compose_look(look.as_ref(), layers);
        let body = self.compose_look(look.as_ref(), &[]);
        let cell = |img: &image::RgbaImage, d: u32| {
            image::imageops::crop_imm(img, d * CELL, 0, CELL, CELL).to_image()
        };
        let views: [image::RgbaImage; 4] = std::array::from_fn(|d| cell(&sheet, d as u32));
        let front = cell(&body, 0);
        // Строки силуэта тела спереди: от макушки до ступней.
        let rows: Vec<u32> = (0..CELL)
            .filter(|&y| (0..CELL).any(|x| front.get_pixel(x, y)[3] > 127))
            .collect();
        let (top, bottom) = (*rows.first()?, *rows.last()?);
        let cols = (0..CELL)
            .filter(|&x| (0..CELL).any(|y| front.get_pixel(x, y)[3] > 127))
            .count() as f32;
        let tall = (bottom - top + 1) as f32;
        let vpm = (tall / gameplay::BODY_HEIGHT_M).round().max(1.0) as u16;
        let grid = voxelize::carve([&views[0], &views[1], &views[2], &views[3]]);
        let mut model = voxelize::to_model(&grid, vpm)?;
        let [sx, _, sz] = model.size();
        // Ступни — нижняя строка тела: в осях модели это `CELL - 1 - bottom`.
        model.set_pivot([
            f32::from(sx) / 2.0,
            (CELL - 1 - bottom) as f32,
            f32::from(sz) / 2.0,
        ]);
        let width_m = cols / f32::from(vpm);
        let squeeze = (gameplay::BODY_WIDTH_M / width_m).clamp(0.2, 1.0);
        Some((model, squeeze))
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

/// Привести персонажей в соответствие с тем, что на них надето и что у них в руках: плоский лист
/// (его видят у призраков) и объёмная модель (у остальных; плоский лист тогда скрыт).
#[allow(clippy::too_many_arguments)]
pub fn sync_outfits(
    net: Option<NonSend<Net>>,
    remote: Res<RemoteSprites>,
    local: Res<LocalBody>,
    outfits: Option<ResMut<Outfits>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    children: Query<&Children>,
    mut quads: Query<(&MeshMaterial3d<StandardMaterial>, &mut Visibility), With<BillboardQuad>>,
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
    let gone: Vec<Entity> = outfits
        .spawned
        .keys()
        .filter(|e| !targets.iter().any(|(_, t)| t == *e))
        .copied()
        .collect();
    for owner in gone {
        if let Some(model) = outfits.spawned.remove(&owner) {
            commands.entity(model).despawn();
        }
    }
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
        let ghost = net
            .core
            .component(id, gameplay::GHOST_COMPONENT)
            .is_some_and(|b| b.first() == Some(&1));
        let layers = match (&hands, ghost) {
            (Some(h), false) => outfits.layers(&worn, h),
            _ => Vec::new(),
        };
        let key: Key = (look, layers, ghost);
        if outfits.applied.get(&entity) == Some(&key) {
            continue;
        }
        let handle = match outfits.composed.get(&key.0) {
            Some(h) => h.clone(),
            None => {
                let sheet = outfits.compose_look(key.0.as_ref(), &[]);
                let h = images.add(to_image(sheet));
                outfits.composed.insert(key.0.clone(), h.clone());
                h
            }
        };
        let mesh = if ghost {
            None
        } else {
            match outfits.models.get(&key) {
                Some(m) => m.clone(),
                None => {
                    let m = outfits.body_model(&key).map(|(model, squeeze)| {
                        (
                            meshes.add(astrocore_render::voxels::model_mesh(&model)),
                            squeeze,
                        )
                    });
                    outfits.models.insert(key.clone(), m.clone());
                    m
                }
            }
        };
        let Ok(kids) = children.get(entity) else {
            continue;
        };
        let mut applied = false;
        for &child in kids {
            if let Ok((m, mut visibility)) = quads.get_mut(child) {
                if let Some(mut mat) = materials.get_mut(&m.0) {
                    mat.base_color_texture = Some(handle.clone());
                }
                // Есть модель — плоский лист не нужен.
                visibility.set_if_neq(if mesh.is_some() {
                    Visibility::Hidden
                } else {
                    Visibility::Inherited
                });
                applied = true;
            }
        }
        if let Some(old) = outfits.spawned.remove(&entity) {
            commands.entity(old).despawn();
        }
        if let Some((mesh, squeeze)) = mesh {
            let material = outfits
                .material
                .get_or_insert_with(|| {
                    astrocore_render::voxels::voxel_model_material(&mut materials)
                })
                .clone();
            let model = commands
                .spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    Transform::default(),
                    Visibility::Hidden,
                    BodyModel {
                        owner: entity,
                        squeeze,
                    },
                ))
                .id();
            outfits.spawned.insert(entity, model);
        }
        if applied {
            outfits.applied.insert(entity, key);
        }
    }
}

/// Модели персонажей идут за ними: та же точка (с покачиванием шага), поворот — куда смотрит
/// персонаж, видимость — как у него (своё тело от первого лица скрыто).
pub fn follow_body_models(
    mut models: Query<(&BodyModel, &mut Transform, &mut Visibility)>,
    owners: Query<
        (
            &Transform,
            &DirectionalSprite,
            &Visibility,
            Option<&Children>,
        ),
        Without<BodyModel>,
    >,
    quads: Query<&Transform, (With<BillboardQuad>, Without<BodyModel>)>,
) {
    for (model, mut transform, mut visibility) in &mut models {
        let Ok((owner, sprite, owner_visibility, kids)) = owners.get(model.owner) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        // Покачивание шага — у плоского листа (`walk_bob`), берём его высоту.
        let bob = kids
            .into_iter()
            .flatten()
            .find_map(|k| quads.get(*k).ok())
            .map_or(0.0, |q| q.translation.y);
        let f = sprite.facing;
        let yaw = if f.x.abs() + f.z.abs() > 1e-4 {
            f.x.atan2(f.z)
        } else {
            0.0
        };
        transform.translation = owner.translation + Vec3::Y * bob;
        transform.rotation = Quat::from_rotation_y(yaw);
        transform.scale = Vec3::new(model.squeeze, 1.0, model.squeeze);
        visibility.set_if_neq(*owner_visibility);
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

    /// Модель персонажа — настоящий объём ростом в тайл (1 м), стоит ступнями на полу, а одежда
    /// делает её другой.
    #[test]
    fn a_character_becomes_a_voxel_model_of_full_height() {
        let mut o = outfits();
        let plain: Key = (Some(look([255, 225, 204], None)), Vec::new(), false);
        let (model, squeeze) = o.body_model(&plain).expect("модель тела");
        assert_eq!(squeeze, 1.0, "в масштабе тайла спрайт не сжимается");
        let mesh = astrocore_voxels::build_mesh(&model);
        let ys: Vec<f32> = mesh.positions.iter().map(|p| p[1]).collect();
        let low = ys.iter().cloned().fold(f32::MAX, f32::min);
        let high = ys.iter().cloned().fold(f32::MIN, f32::max);
        assert!(low.abs() < 0.01, "ступни на полу: {low}");
        assert!((high - gameplay::BODY_HEIGHT_M).abs() < 0.15, "рост {high}");
        let zs: Vec<f32> = mesh.positions.iter().map(|p| p[2]).collect();
        let depth = zs.iter().cloned().fold(f32::MIN, f32::max)
            - zs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(depth > 0.1, "не плоский: глубина {depth}");
        let dressed: Key = (
            plain.0.clone(),
            vec![PathBuf::from(
                "Textures/Clothing/Uniforms/Jumpsuits/Security/security.rsi/equipped-INNERCLOTHING.png",
            )],
            false,
        );
        let (suit, _) = o.body_model(&dressed).expect("модель в форме");
        assert_ne!(suit, model, "форма меняет модель");
    }

    /// Сохранить модели персонажей в `.avox` для осмотра в демо движка:
    /// `ASTRO_DUMP_BODY_MODELS=/папка cargo test -p client dump_body_models`.
    #[test]
    fn dump_body_models() {
        let Some(out) = std::env::var_os("ASTRO_DUMP_BODY_MODELS") else {
            return;
        };
        let out = PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let mut o = outfits();
        let t = |p: &str| PathBuf::from(format!("Textures/Clothing/{p}"));
        let hair = Some("Mobs/Customization/human_hair.rsi/afro.png");
        let outfits_list: Vec<(&str, Look, Vec<PathBuf>)> = vec![
            ("1_body", look([255, 225, 204], None), vec![]),
            (
                "2_security",
                look([200, 150, 110], hair),
                vec![
                    t("Shoes/Boots/jackboots.rsi/equipped-FEET.png"),
                    t("Uniforms/Jumpsuits/Security/security.rsi/equipped-INNERCLOTHING.png"),
                ],
            ),
            (
                "3_captain",
                look([120, 80, 60], hair),
                vec![
                    t("Shoes/Boots/laceups.rsi/equipped-FEET.png"),
                    t("Uniforms/Jumpsuits/Command/captain.rsi/equipped-INNERCLOTHING.png"),
                    t("Hands/Gloves/captain.rsi/equipped-HAND.png"),
                    t("OuterClothing/Armor/captain_carapace.rsi/equipped-OUTERCLOTHING.png"),
                    t("Back/Backpacks/captain.rsi/equipped-BACKPACK.png"),
                ],
            ),
        ];
        for (name, l, layers) in outfits_list {
            let key: Key = (Some(l), layers, false);
            let (model, squeeze) = o.body_model(&key).expect("модель");
            // В файле сжатие не хранится: для осмотра сжимаем сетку по X и Z (ближайший воксель).
            let [sx, sy, sz] = model.size();
            let n = |s: u16| ((f32::from(s) * squeeze).round() as u16).max(1);
            let mut baked =
                astrocore_voxels::VoxelModel::new([n(sx), sy, n(sz)], model.palette().to_vec())
                    .unwrap();
            baked
                .set_voxels_per_meter(model.voxels_per_meter())
                .unwrap();
            let p = model.pivot();
            baked.set_pivot([p[0] * squeeze, p[1], p[2] * squeeze]);
            for z in 0..n(sz) {
                for y in 0..sy {
                    for x in 0..n(sx) {
                        let (ox, oz) = (
                            (f32::from(x) / squeeze) as i32,
                            (f32::from(z) / squeeze) as i32,
                        );
                        let c = model.get(ox, i32::from(y), oz);
                        baked.set(x, y, z, c).unwrap();
                    }
                }
            }
            std::fs::write(
                out.join(format!("{name}.avox")),
                astrocore_voxels::encode(&baked),
            )
            .unwrap();
        }
    }
}
