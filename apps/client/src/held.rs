//! Предметы в руках от первого лица: объёмная модель из спрайта (два пикселя толщины), как у
//! предметов на полу, перед камерой справа и слева внизу; оружие смотрит дулом вперёд.
//!
//! Спрайты оружия SS14 рисуют дулом вправо, поэтому модель поворачивается вокруг вертикали так,
//! чтобы «право» спрайта ушло вперёд; небольшой доворот даёт увидеть боковую грань и объём.

use astrocore_client::Look;
use astrocore_render::PlacementSprites;
use astrocore_render::manifest::voxel_mesh;
use bevy::prelude::*;

use crate::Loadout;

/// Модели в руках: сущность и прототип, который она изображает, на каждую руку.
#[derive(Resource, Default)]
pub struct HeldModels(Vec<Option<(Entity, String)>>);

/// Длина модели вдоль взгляда не больше этого, м (крупные предметы уменьшаются).
const MAX_LENGTH_M: f32 = 0.5;
/// Где модель относительно камеры: правая рука справа внизу, левая слева.
const OFFSET: Vec3 = Vec3::new(0.2, -0.17, -0.42);
/// Доворот вокруг вертикали сверх «дулом вперёд», рад: видна боковая грань.
const TURN: f32 = 0.35;

#[allow(clippy::too_many_arguments)]
pub fn sync_held(
    mut commands: Commands,
    loadout: Res<Loadout>,
    look: Option<Res<Look>>,
    sprites: Option<Res<PlacementSprites>>,
    images: Res<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    cameras: Query<Entity, With<Camera3d>>,
    mut held: ResMut<HeldModels>,
    mut hud: ResMut<crate::Hud>,
) {
    let (Ok(cam), Some(sprites)) = (cameras.single(), sprites) else {
        return;
    };
    let first_person = look.is_some_and(|l| l.first_person);
    let want: Vec<Option<String>> = loadout
        .hands
        .as_ref()
        .map(|h| {
            h.iter()
                .map(|(_, _, item)| item.map(|i| i.prototype.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if held.0.len() < want.len() {
        held.0.resize_with(want.len(), || None);
    }
    hud.state.held_3d.clear();
    for (hand, proto) in want.iter().enumerate() {
        let current = held.0[hand].as_ref().map(|(_, p)| p.clone());
        let target = proto.clone().filter(|_| first_person);
        if current != target {
            if let Some((e, _)) = held.0[hand].take() {
                commands.entity(e).despawn();
            }
            if let Some(name) = &target
                && let Some(model) =
                    build_model(name, hand, &sprites, &images, &mut meshes, &mut materials)
            {
                let e = commands.spawn(model).id();
                commands.entity(cam).add_child(e);
                held.0[hand] = Some((e, name.clone()));
            }
        }
        // Для руки с моделью плоский значок не рисуется.
        hud.state.held_3d.push(held.0[hand].is_some());
    }
}

fn build_model(
    proto: &str,
    hand: usize,
    sprites: &PlacementSprites,
    images: &Assets<Image>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<(Mesh3d, MeshMaterial3d<StandardMaterial>, Transform)> {
    let id = astrocore_core::PrototypeId::new(proto).ok()?;
    let sprite = sprites.0.get(&id)?;
    let img = images.get(&sprite.image)?;
    let data = img.data.as_ref()?;
    let mesh = voxel_mesh(
        data,
        img.width() as usize,
        img.height() as usize,
        sprite.size,
        sprite.thickness.max(2.0 / 32.0),
    )?;
    let length = sprite.size.x.max(sprite.size.y);
    let scale = (MAX_LENGTH_M / length).min(1.5);
    // Рисунок лицом к +Z, дулом к +X. Поворот на +90° переводит +X вперёд (−Z); TURN доворачивает
    // так, чтобы камера видела грань, а не ребро. Левая рука зеркалит доворот.
    let side = if hand == 0 { -1.0 } else { 1.0 };
    let rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - side * TURN);
    // Рисунок стоит на основании в начале координат: центрируем по ширине и высоте.
    let centre = rotation * Vec3::new(-sprite.size.x * 0.5, -sprite.size.y * 0.5, 0.0) * scale;
    let at = Vec3::new(OFFSET.x * side, OFFSET.y, OFFSET.z) + centre;
    let material = materials.add(StandardMaterial {
        base_color_texture: Some(sprite.image.clone()),
        reflectance: 0.05,
        alpha_mode: AlphaMode::Opaque,
        ..default()
    });
    Some((
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(material),
        Transform::from_translation(at)
            .with_rotation(rotation)
            .with_scale(Vec3::splat(scale)),
    ))
}
