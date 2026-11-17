//! Одежда и предметы в руках на спрайте персонажа: слои из RSI предметов (`equipped-*`,
//! `inhand-left/right`) накладываются на тело для каждого из четырёх направлений.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use astrocore_client::{LocalBody, Net, PlayerSprite, RemoteSprites};
use astrocore_math::glam::Vec3 as MathVec3;
use astrocore_render::{BillboardQuad, DirectionalSprite, voxel_mesh};
use astrocore_sprites::select_direction;
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
    /// Толщина надетого из манифеста (`worn_volume`).
    worn: WornVolume,
    /// Объёмные слои по описанию: сетки и материалы четырёх направлений.
    volumes: HashMap<Layer, Option<LayerAssets>>,
}

/// Толщина надетого: на сколько пикселей слой выступает за тело с каждой стороны. По слоту и по
/// предмету (предмет перекрывает слот); данные — компонент `WornVolume` в прототипах.
#[derive(Default, Clone)]
pub struct WornVolume {
    slots: HashMap<String, f32>,
    items: HashMap<String, f32>,
}

impl WornVolume {
    /// Прочитать из манифеста рендера (поле `worn_volume`); нет поля — всё тонкое, один пиксель.
    pub fn from_manifest(path: Option<&Path>) -> Self {
        let value = path
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        let table = |key: &str| -> HashMap<String, f32> {
            value
                .as_ref()
                .and_then(|v| v.get("worn_volume")?.get(key)?.as_object())
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32)))
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            slots: table("slots"),
            items: table("items"),
        }
    }

    fn shell(&self, slot: &str, proto: &str) -> u8 {
        self.items
            .get(proto)
            .or_else(|| self.slots.get(slot))
            .map_or(1, |px| px.round().clamp(0.0, 16.0) as u8)
    }
}

/// Чем нарисован слой.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Source {
    /// Тело с внешностью: плита в один пиксель.
    Body(Option<Look>),
    /// Картинка надетого или предмета в руке (2×2 направления).
    Worn(PathBuf),
}

/// Слой персонажа: что рисовать, насколько он выступает за тело и в каком порядке лежит.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Layer {
    source: Source,
    shell: u8,
    order: u8,
}

/// Сетки и материалы слоя по четырём направлениям; `None` — в этом направлении нечего рисовать.
#[derive(Clone)]
struct LayerAssets {
    meshes: [Option<Handle<Mesh>>; 4],
    materials: [Handle<StandardMaterial>; 4],
}

/// Объёмный слой на персонаже: смена направления меняет сетку и материал.
#[derive(Component)]
pub struct WornLayer {
    assets_meshes: [Option<Handle<Mesh>>; 4],
    assets_materials: [Handle<StandardMaterial>; 4],
}

/// Что определяет вид листа: внешность и надетые слои.
type Key = (Option<Look>, Vec<Layer>);

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
            worn: WornVolume::default(),
            volumes: HashMap::new(),
        }
    }

    /// Задать толщину надетого (из манифеста).
    #[must_use]
    pub fn with_worn(mut self, worn: WornVolume) -> Self {
        self.worn = worn;
        self
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

    fn layers(&self, worn: &[(String, String)], hands: &gameplay::Hands) -> Vec<Layer> {
        let mut out = Vec::new();
        let push = |out: &mut Vec<Layer>, path: PathBuf, shell: u8| {
            let order = u8::try_from(out.len() + 1).unwrap_or(u8::MAX);
            out.push(Layer {
                source: Source::Worn(path),
                shell,
                order,
            });
        };
        for (slot, suffix) in SLOT_ORDER {
            if let Some((_, proto)) = worn.iter().find(|(s, _)| s == slot)
                && let Some(p) = self.sibling(proto, |_| vec![format!("equipped-{suffix}.png")])
            {
                push(&mut out, p, self.worn.shell(slot, proto));
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
                push(&mut out, p, self.worn.shell("hand", proto));
            }
        }
        out
    }

    /// Сетки и материалы слоя: по одной на направление, плита глубиной `2 * shell + 1` пикселей
    /// (тело — один). Слои с равной толщиной разнесены на доли миллиметра, чтобы не мерцали.
    fn layer_assets(
        &mut self,
        layer: &Layer,
        size: Vec2,
        meshes: &mut Assets<Mesh>,
        images: &mut Assets<Image>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Option<LayerAssets> {
        if let Some(c) = self.volumes.get(layer) {
            return c.clone();
        }
        let cells: Option<[image::RgbaImage; 4]> = match &layer.source {
            Source::Body(look) => {
                let sheet = self.compose_look(look.as_ref(), &[]);
                Some(std::array::from_fn(|d| {
                    image::imageops::crop_imm(&sheet, d as u32 * CELL, 0, CELL, CELL).to_image()
                }))
            }
            Source::Worn(rel) => self
                .load(rel)
                .filter(|part| part.width() >= CELL * 2 && part.height() >= CELL * 2)
                .map(|part| {
                    std::array::from_fn(|d| {
                        let d = d as u32;
                        image::imageops::crop_imm(part, (d % 2) * CELL, (d / 2) * CELL, CELL, CELL)
                            .to_image()
                    })
                }),
        };
        let built = cells.map(|cells| {
            let depth =
                f32::from(layer.shell * 2 + 1) / CELL as f32 + f32::from(layer.order) * 0.0006;
            let mut mesh_of = |c: &image::RgbaImage| {
                voxel_mesh(c.as_raw(), CELL as usize, CELL as usize, size, depth)
                    .map(|m| meshes.add(m))
            };
            let meshes_by_dir: [Option<Handle<Mesh>>; 4] =
                std::array::from_fn(|d| mesh_of(&cells[d]));
            let materials_by_dir: [Handle<StandardMaterial>; 4] = std::array::from_fn(|d| {
                materials.add(StandardMaterial {
                    base_color_texture: Some(images.add(to_image(cells[d].clone()))),
                    reflectance: 0.05,
                    alpha_mode: AlphaMode::Opaque,
                    cull_mode: Some(bevy::render::render_resource::Face::Back),
                    ..default()
                })
            });
            LayerAssets {
                meshes: meshes_by_dir,
                materials: materials_by_dir,
            }
        });
        self.volumes.insert(layer.clone(), built.clone());
        built
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

/// Привести спрайты игроков в соответствие с тем, что на них надето и что у них в руках: плоский
/// лист остаётся основой (его видят призраки), а тело и каждый слой одежды становятся объёмными
/// плитами, толще тела на `WornVolume`.
#[allow(clippy::too_many_arguments)]
pub fn sync_outfits(
    net: Option<NonSend<Net>>,
    remote: Res<RemoteSprites>,
    local: Res<LocalBody>,
    player: Option<Res<PlayerSprite>>,
    outfits: Option<ResMut<Outfits>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    children: Query<&Children>,
    quads: Query<&MeshMaterial3d<StandardMaterial>, With<BillboardQuad>>,
    old_layers: Query<(), With<WornLayer>>,
) {
    let (Some(net), Some(mut outfits)) = (net, outfits) else {
        return;
    };
    let size = player.map_or(Vec2::ONE, |p| p.size);
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
        let ghost = net
            .core
            .component(id, gameplay::GHOST_COMPONENT)
            .is_some_and(|b| b.first() == Some(&1));
        let mut layers = Vec::new();
        if !ghost {
            layers.push(Layer {
                source: Source::Body(look.clone()),
                shell: 0,
                order: 0,
            });
            if let Some(h) = &hands {
                layers.extend(outfits.layers(&worn, h));
            }
        }
        let key: Key = (look, layers);
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
            } else {
                continue;
            }
            {
                // Объёмные слои живут на плоской основе: те же поворот и покачивание.
                if let Ok(old) = children.get(child) {
                    for &l in old {
                        if old_layers.contains(l) {
                            commands.entity(l).despawn();
                        }
                    }
                }
                for layer in &key.1 {
                    let Some(a) =
                        outfits.layer_assets(layer, size, &mut meshes, &mut images, &mut materials)
                    else {
                        continue;
                    };
                    let first = a.meshes[0].clone();
                    commands.entity(child).with_children(|p| {
                        let mut e = p.spawn((
                            WornLayer {
                                assets_meshes: a.meshes.clone(),
                                assets_materials: a.materials.clone(),
                            },
                            MeshMaterial3d(a.materials[0].clone()),
                            Transform::IDENTITY,
                        ));
                        match first {
                            Some(m) => {
                                e.insert((Mesh3d(m), Visibility::Inherited));
                            }
                            None => {
                                e.insert(Visibility::Hidden);
                            }
                        }
                    });
                }
            }
        }
        if applied {
            outfits.applied.insert(entity, key);
        }
    }
}

/// Выбрать для каждого слоя сетку по направлению, в котором персонаж виден камере (как у плоской
/// основы: столбец листа — индекс направления).
pub fn update_worn_layers(
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut layers: Query<(
        &WornLayer,
        &ChildOf,
        &mut Mesh3d,
        &mut MeshMaterial3d<StandardMaterial>,
        &mut Visibility,
    )>,
    quads: Query<&ChildOf, With<BillboardQuad>>,
    roots: Query<(&GlobalTransform, &DirectionalSprite)>,
) {
    let Ok(cam) = cameras.single() else { return };
    let cam = cam.translation();
    for (layer, parent, mut mesh, mut material, mut visibility) in &mut layers {
        let Ok(root) = quads.get(parent.parent()) else {
            continue;
        };
        let Ok((global, sprite)) = roots.get(root.parent()) else {
            continue;
        };
        let pos = global.translation();
        let dir = select_direction(
            MathVec3::new(sprite.facing.x, sprite.facing.y, sprite.facing.z),
            MathVec3::new(pos.x, pos.y, pos.z),
            MathVec3::new(cam.x, cam.y, cam.z),
            sprite.set,
        )
        .index()
        .min(3);
        match &layer.assets_meshes[dir] {
            Some(m) => {
                if mesh.0 != *m {
                    mesh.0 = m.clone();
                }
                if material.0 != layer.assets_materials[dir] {
                    material.0 = layer.assets_materials[dir].clone();
                }
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
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

    /// Глубина плиты слоя — `2 * shell + 1` пикселей (по 1/32 м), у тела — один пиксель, и более
    /// толстый слой выступает дальше тонкого.
    #[test]
    fn a_worn_layer_is_a_slab_thicker_than_the_body_by_its_shell() {
        let mut o = outfits();
        let manifest =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ss14/render-manifest.json");
        let worn = WornVolume::from_manifest(Some(&manifest));
        assert_eq!(
            worn.shell("outerClothing", "Nothing"),
            3,
            "верхняя одежда из данных"
        );
        assert_eq!(worn.shell("belt", "Nothing"), 2, "пояс из данных");
        assert!(worn.shell("outerClothing", "Nothing") > worn.shell("belt", "Nothing"));
        let (mut meshes, mut images, mut materials) = (
            Assets::<Mesh>::default(),
            Assets::<Image>::default(),
            Assets::<StandardMaterial>::default(),
        );
        let mut depth = |layer: &Layer, o: &mut Outfits| -> f32 {
            let a = o
                .layer_assets(layer, Vec2::ONE, &mut meshes, &mut images, &mut materials)
                .expect("слой строится");
            let mesh = meshes
                .get(a.meshes.iter().flatten().next().expect("есть сетка"))
                .expect("сетка");
            let z: Vec<f32> = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(|v| v.as_float3())
                .expect("вершины")
                .iter()
                .map(|v| v[2])
                .collect();
            z.iter().cloned().fold(f32::MIN, f32::max) - z.iter().cloned().fold(f32::MAX, f32::min)
        };
        let body = depth(
            &Layer {
                source: Source::Body(None),
                shell: 0,
                order: 0,
            },
            &mut o,
        );
        let suit = Layer {
            source: Source::Worn(
                "Textures/Clothing/Uniforms/Jumpskirts/Security/security.rsi/equipped-INNERCLOTHING.png".into(),
            ),
            shell: 1,
            order: 1,
        };
        let jumpsuit = depth(&suit, &mut o);
        let outer = depth(
            &Layer {
                shell: 3,
                order: 2,
                ..suit.clone()
            },
            &mut o,
        );
        assert!((body - 1.0 / 32.0).abs() < 0.002, "тело {body}");
        assert!(
            (jumpsuit - 3.0 / 32.0).abs() < 0.002,
            "комбинезон {jumpsuit}"
        );
        assert!((outer - 7.0 / 32.0).abs() < 0.002, "верхняя одежда {outer}");
    }
}
