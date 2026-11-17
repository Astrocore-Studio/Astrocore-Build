use astrocore_core::ObjectId;
use astrocore_render::{DoorPanel, DoorSprites, Phase};
use bevy::prelude::*;
use std::collections::BTreeMap;

pub(crate) struct ObservedMotion {
    elapsed: f32,
    received_at: f32,
    opening: bool,
}

/// The visual transition starts with the SS14 action, before its physical cutoff.
pub(crate) fn motions(
    loadout: Res<crate::Loadout>,
    time: Res<Time>,
    sprites: Res<DoorSprites>,
    mut panels: Query<(&mut DoorPanel, &MeshMaterial3d<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut observed: Local<BTreeMap<ObjectId, ObservedMotion>>,
) {
    let Some(motions) = &loadout.airlock_motions else {
        return;
    };
    observed.retain(|id, _| motions.iter().any(|motion| motion.element == *id));
    for (mut panel, material) in &mut panels {
        let Some(motion) = motions.iter().find(|motion| motion.element == panel.id) else {
            continue;
        };
        let previous = observed.entry(panel.id).or_insert_with(|| ObservedMotion {
            elapsed: motion.elapsed,
            received_at: time.elapsed_secs(),
            opening: motion.opening,
        });
        if previous.elapsed != motion.elapsed || previous.opening != motion.opening {
            previous.elapsed = motion.elapsed;
            previous.opening = motion.opening;
            previous.received_at = time.elapsed_secs();
        }
        let elapsed = previous.elapsed + (time.elapsed_secs() - previous.received_at);
        let Some(set) = sprites.0.get(&panel.set) else {
            continue;
        };
        let sheet = if motion.opening {
            &set.opening
        } else {
            &set.closing
        };
        let progress = if motion.duration > 0. {
            (elapsed / motion.duration).clamp(0., 1.)
        } else {
            1.
        };
        let seconds = progress * sheet.cycle_seconds();
        panel.anim.phase = if motion.opening {
            Phase::Opening
        } else {
            Phase::Closing
        };
        panel.anim.t = seconds;
        let Some(mut material) = materials.get_mut(&material.0) else {
            continue;
        };
        let mut frame = sheet.delays.len().saturating_sub(1);
        let mut end = 0.;
        for (i, delay) in sheet.delays.iter().enumerate() {
            end += *delay;
            if seconds < end {
                frame = i;
                break;
            }
        }
        let cols = sheet.cols.max(1);
        let rows = sheet.rows.max(1);
        let index = (frame as u32).min(cols * rows - 1);
        material.base_color_texture = Some(sheet.image.clone());
        material.uv_transform = bevy::math::Affine2::from_scale_angle_translation(
            Vec2::new(1. / cols as f32, 1. / rows as f32),
            0.,
            Vec2::new(
                (index % cols) as f32 / cols as f32,
                (index / cols) as f32 / rows as f32,
            ),
        );
    }
}
