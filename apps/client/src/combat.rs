//! Бой на клиенте: удар по клику и лечение.

use astrocore_client::{Net, RemoteSprites};
use astrocore_core::NetworkEntityId;
use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;
use gameplay::{CMD_ATTACK, CMD_HEAL};

/// Насколько близко по горизонтали луч должен пройти к оси тела, чтобы попасть, м.
const PICK_RADIUS_M: f32 = 0.45;

/// Высота спрайта персонажа, м.
const BODY_M: f32 = gameplay::BODY_HEIGHT_M;

/// Куда пришёлся выстрел по спрайту: часть тела по высоте и по смещению в сторону (смещение от
/// камеры вправо — левая часть тела, ведь персонаж обычно смотрит на нас).
fn zone_of(height: f32, lateral: f32) -> &'static str {
    let h = (height / BODY_M).clamp(0.0, 1.0);
    let side_left = lateral > 0.0;
    if h >= 0.75 {
        "Head"
    } else if h >= 0.35 {
        if lateral.abs() > 0.18 {
            if side_left { "LeftArm" } else { "RightArm" }
        } else {
            "Torso"
        }
    } else if side_left {
        "LeftLeg"
    } else {
        "RightLeg"
    }
}

/// Ближайший вдоль луча персонаж, в которого можно попасть, и часть тела под курсором.
pub fn pick_target(
    origin: Vec3,
    dir: Vec3,
    candidates: &[(NetworkEntityId, Vec3)],
) -> Option<(NetworkEntityId, &'static str)> {
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    candidates
        .iter()
        .filter_map(|(id, feet)| {
            // Тело — вертикальный отрезок от ступней до макушки: берём точку луча, ближайшую к
            // его оси, и смотрим, в пределах ли она роста и радиуса попадания.
            let flat = Vec3::new(dir.x, 0.0, dir.z);
            let t = if flat.length_squared() > 1e-6 {
                ((*feet - origin).dot(flat)) / flat.length_squared()
            } else {
                (*feet + Vec3::Y * (BODY_M * 0.5) - origin).dot(dir)
            };
            if t <= 0.0 {
                return None;
            }
            let p = origin + dir * t;
            let height = p.y - feet.y;
            let miss = (p.x - feet.x).hypot(p.z - feet.z);
            if miss > PICK_RADIUS_M || !(0.0..=BODY_M).contains(&height) {
                return None;
            }
            Some((*id, t, zone_of(height, (p - *feet).dot(right))))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _, part)| (id, part))
}

/// Правая кнопка по миру — меню глаголов цели; Shift + левая — сразу осмотр (SS14: Shift+клик).
/// Цель: игрок под курсором, иначе объект под прицелом.
#[allow(clippy::too_many_arguments)]
pub fn verb_input(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    wants: Res<EguiWantsInput>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    remote: Res<RemoteSprites>,
    transforms: Query<&Transform>,
    loadout: Res<crate::Loadout>,
    mut hud: ResMut<crate::Hud>,
    net: Option<NonSendMut<Net>>,
) {
    let Some(mut net) = net else { return };
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let free = !wants.wants_any_pointer_input();
    let menu = buttons.just_pressed(MouseButton::Right) && free;
    let examine = buttons.just_pressed(MouseButton::Left) && shift && free;
    if !menu && !examine {
        return;
    }
    let (Ok(window), Ok((camera, cam_t))) = (windows.single(), cameras.single()) else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_t, cursor) else {
        return;
    };
    let candidates: Vec<(NetworkEntityId, Vec3)> = remote
        .0
        .iter()
        .filter_map(|(id, e)| Some((*id, transforms.get(*e).ok()?.translation)))
        .collect();
    let target = pick_target(ray.origin, *ray.direction, &candidates)
        .map(|(id, _)| gameplay::VerbTarget::Player(id))
        .or(hud.state.aim_id.map(gameplay::VerbTarget::Object));
    let Some(target) = target else {
        hud.state.menu = None;
        return;
    };
    // В боевом режиме правая кнопка по игроку — толчок (`DisarmAttackEvent`), а не меню.
    if menu
        && hud.state.combat
        && let gameplay::VerbTarget::Player(p) = target
    {
        net.send_command(gameplay::CMD_DISARM, gameplay::attack_data(p));
        return;
    }
    if examine {
        net.send_command(
            gameplay::CMD_VERB_DO,
            gameplay::target_data(target, Some("examine")),
        );
    } else {
        net.send_command(gameplay::CMD_VERB_LIST, gameplay::target_data(target, None));
        hud.state.menu = Some(crate::hud::VerbMenu {
            at: bevy_egui::egui::pos2(cursor.x, cursor.y),
            target,
            after_seq: loadout.verbs.as_ref().map_or(0, |v| v.seq),
        });
    }
}

/// Куда целится бросок, если курсор смотрит выше пола, м.
const THROW_AIM_M: f32 = 8.0;

/// Левая кнопка по миру — удар по игроку под курсором; `T` — применить предмет из руки на него.
#[allow(clippy::too_many_arguments)]
pub fn attack_and_heal(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    wants: Res<EguiWantsInput>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    remote: Res<RemoteSprites>,
    transforms: Query<&Transform>,
    loadout: Res<crate::Loadout>,
    ground: Res<crate::Ground>,
    look: Option<Res<astrocore_client::Look>>,
    mut hud: ResMut<crate::Hud>,
    net: Option<NonSendMut<Net>>,
) {
    let Some(mut net) = net else { return };
    let active = loadout.hands.as_ref().map_or(0, |h| h.active() as u8);
    if keys.just_pressed(KeyCode::KeyR)
        && let Some(me) = net.core.net_id()
    {
        net.send_command(CMD_HEAL, gameplay::heal_data(active, me));
    }
    // Оружие в руке и боевой режим: левая кнопка — выстрел в сторону прицела (у автоматического
    // огня — пока зажата), `U` — перезарядка, `Y` — достать магазин.
    if loadout.ammo.as_ref().is_some_and(|a| a.gun) && hud.state.combat {
        if keys.just_pressed(KeyCode::KeyU) {
            net.send_command(gameplay::CMD_RELOAD, Vec::new());
        }
        if keys.just_pressed(KeyCode::KeyY) {
            net.send_command(gameplay::CMD_EJECT_MAG, Vec::new());
        }
        let auto = loadout.ammo.as_ref().is_some_and(|a| a.auto);
        let fire = if auto {
            buttons.pressed(MouseButton::Left)
        } else {
            buttons.just_pressed(MouseButton::Left)
        };
        if fire
            && !wants.wants_any_pointer_input()
            && let (Ok(window), Ok((camera, cam_t))) = (windows.single(), cameras.single())
            && let Some(ray) = window
                .cursor_position()
                .and_then(|c| camera.viewport_to_world(cam_t, c).ok())
        {
            let target = ray.origin + *ray.direction * THROW_AIM_M * 4.0;
            net.send_command(gameplay::CMD_SHOOT, gameplay::throw_data(target.to_array()));
            return;
        }
    }
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let attack =
        buttons.just_pressed(MouseButton::Left) && !shift && !wants.wants_any_pointer_input();
    if attack && let Some((recipe, rotation)) = &hud.state.placement {
        if let Some(point) = ground.0 {
            net.send_command(
                gameplay::crafting::CMD_PLACE,
                gameplay::crafting::placement_data(
                    gameplay::crafting::placement_variant(recipe, ground.1),
                    *rotation,
                    point,
                ),
            );
        }
        return;
    }
    // Режим броска: щелчок бросает предмет из руки в точку под курсором и выключает режим.
    if attack && hud.state.throw_mode {
        hud.state.throw_mode = false;
        let (Ok(window), Ok((camera, cam_t))) = (windows.single(), cameras.single()) else {
            return;
        };
        let ray = window
            .cursor_position()
            .and_then(|c| camera.viewport_to_world(cam_t, c).ok());
        let feet = net.core.position();
        // Точка на полу под курсором; если смотрят выше пола — вперёд на предельную дальность.
        let target = ground.0.map(|p| [p[0], p[1], p[2]]).or_else(|| {
            let r = ray?;
            let d = *r.direction;
            Some([
                feet.x + d.x * THROW_AIM_M,
                feet.y,
                feet.z + d.z * THROW_AIM_M,
            ])
        });
        if let Some(t) = target {
            net.send_command(gameplay::CMD_THROW, gameplay::throw_data(t));
        }
        return;
    }
    let heal_other = keys.just_pressed(KeyCode::KeyT);
    let feed_other = keys.just_pressed(KeyCode::KeyC);
    if !attack && !heal_other && !feed_other {
        return;
    }
    let (Ok(window), Ok((camera, cam_t))) = (windows.single(), cameras.single()) else {
        return;
    };
    let ray = if look.is_some_and(|l| l.first_person) {
        Ray3d::new(cam_t.translation(), cam_t.forward())
    } else {
        let Some(ray) = window
            .cursor_position()
            .and_then(|cursor| camera.viewport_to_world(cam_t, cursor).ok())
        else {
            return;
        };
        ray
    };
    let candidates: Vec<(NetworkEntityId, Vec3)> = remote
        .0
        .iter()
        .filter_map(|(id, e)| Some((*id, transforms.get(*e).ok()?.translation)))
        .collect();
    let Some((target, part)) = pick_target(ray.origin, *ray.direction, &candidates) else {
        if attack
            && let Some(id) = hud.state.aim_id
            && !loadout
                .hands
                .as_ref()
                .and_then(|hands| hands.get(hands.active()))
                .is_some_and(|item| {
                    gameplay::crafting::rcd_modes(item.prototype.as_str())
                        .next()
                        .is_some()
                })
        {
            net.send_command(
                if hud.state.combat {
                    gameplay::CMD_ATTACK_OBJECT
                } else {
                    gameplay::CMD_INTERACT
                },
                gameplay::pickup_data(id),
            );
            return;
        }
        // Клик по полу: предмет в руке применяется к точке (швабра по луже и ведру).
        if attack {
            let feet = net.core.position();
            let floor_point = crate::ground_point(
                ray.origin,
                *ray.direction,
                feet.y + crate::hud::GROUND_LIFT_M,
            );
            let wall = net.core.world().and_then(|world| {
                let step = world.config().grid_step_m;
                world
                    .elements()
                    .filter_map(|e| {
                        let astrocore_world::Placement::Free(p) = e.placement else {
                            return None;
                        };
                        if !e.material.as_str().starts_with("ss14:wall_")
                            && !e.material.as_str().contains("window")
                            && e.material.as_str() != "ss14:grille"
                        {
                            return None;
                        }
                        let (min, max) = p.aabb(step);
                        let t = crate::ray_box(
                            ray.origin.to_array(),
                            ray.direction.as_vec3().to_array(),
                            min,
                            max,
                        )?;
                        if floor_point
                            .is_some_and(|pt| t > ray.origin.distance(Vec3::from_array(pt)))
                        {
                            return None;
                        }
                        Some((t, [p.cell.x as f32 + 0.5, feet.y, p.cell.z as f32 + 0.5]))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, point)| point)
            });
            let Some(point) = wall.or(floor_point) else {
                return;
            };
            if loadout
                .hands
                .as_ref()
                .and_then(|h| h.get(h.active()))
                .is_some_and(|i| {
                    gameplay::crafting::rcd_modes(i.prototype.as_str())
                        .next()
                        .is_some()
                })
            {
                net.send_command(
                    gameplay::crafting::CMD_RCD_USE,
                    gameplay::drop_at_data(hud.state.rcd_rotation, point),
                );
                return;
            }
            let mut data = gameplay::drop_at_data(active, point);
            data.push(ground.1);
            net.send_command(gameplay::CMD_USE_GROUND, data);
        }
        return;
    };
    if attack {
        let index = gameplay::PARTS.iter().position(|p| *p == part).unwrap_or(1) as u8;
        net.send_command(CMD_ATTACK, gameplay::attack_part_data(target, index));
    } else if feed_other {
        net.send_command(gameplay::CMD_FEED, gameplay::feed_data(active, target));
    } else {
        net.send_command(CMD_HEAL, gameplay::heal_data(active, target));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ray_picks_the_nearest_player_and_the_part_it_hits() {
        let (a, b, c) = (NetworkEntityId(1), NetworkEntityId(2), NetworkEntityId(3));
        let dir = Vec3::NEG_Z;
        let near = (a, Vec3::new(0.0, 0.0, -2.0));
        let far = (b, Vec3::new(0.0, 0.0, -4.0));
        let aside = (c, Vec3::new(3.0, 0.0, -1.0));
        // Высоты — в долях роста: луч на середине роста идёт в торс ближайшего.
        let at = |frac: f32| frac * BODY_M;
        let origin = Vec3::new(0.0, at(0.5), 0.0);
        assert_eq!(
            pick_target(origin, dir, &[far, near, aside]),
            Some((a, "Torso"))
        );
        assert_eq!(pick_target(origin, dir, &[aside]), None);
        assert_eq!(
            pick_target(origin, Vec3::Z, &[near]),
            None,
            "позади — не цель"
        );
        // Выше — голова, ниже — нога, сбоку — рука.
        assert_eq!(
            pick_target(Vec3::new(0.0, at(0.9), 0.0), dir, &[near])
                .unwrap()
                .1,
            "Head"
        );
        assert_eq!(
            pick_target(Vec3::new(0.0, at(0.1), 0.0), dir, &[near])
                .unwrap()
                .1,
            "RightLeg"
        );
        assert_eq!(
            pick_target(Vec3::new(0.3, at(0.5), 0.0), dir, &[near])
                .unwrap()
                .1,
            "LeftArm"
        );
        assert_eq!(
            pick_target(Vec3::new(-0.3, at(0.5), 0.0), dir, &[near])
                .unwrap()
                .1,
            "RightArm"
        );
    }
}
