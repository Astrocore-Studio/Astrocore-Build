use astrocore_client::Net;
use astrocore_core::ObjectId;
use astrocore_math::glam::Vec3 as MVec3;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
mod body;
mod combat;
mod hud;
mod outfit;
mod players;
mod sounds;
use gameplay::{
    CMD_DROP, CMD_EQUIP, CMD_SELECT_HAND, CMD_SWAP_HANDS, HANDS_COMPONENT, Hands,
    INVENTORY_COMPONENT, InventoryView, decode_hands, decode_inventory, drop_data, equip_data,
};

/// Что игрок сейчас держит и носит, как это видит клиент (копия серверных реплик).
#[derive(Resource, Default)]
pub struct Loadout {
    pub hands: Option<Hands>,
    pub inventory: Option<InventoryView>,
    /// Идущее действие с таймером и время (с игры), когда клиент о нём узнал.
    pub doafter: Option<(gameplay::DoAfterView, f32)>,
    pub health: Option<gameplay::HealthView>,
    /// Когда (по часам клиента) пришло последнее состояние здоровья.
    pub health_at: f32,
}

/// Клиент билда: вид от первого лица, клавиши рук и худ по образцу SS14.
///
/// E — взаимодействие с ближайшим предметом или дверью (обрабатывает клиент движка),
/// G — выбросить предмет из активной руки, X — сменить активную руку, 1 и 2 — выбрать руку.
pub struct BuildClientPlugin {
    /// Манифест рендера: из него худ берёт значки предметов, рядом с ним лежат текстуры интерфейса.
    pub manifest: Option<std::path::PathBuf>,
    /// Отладка: сам выполняет короткий сценарий (поднять ближайшее, надеть на спину).
    pub autoplay: bool,
}

impl Plugin for BuildClientPlugin {
    fn build(&self, app: &mut App) {
        if let Some(root) = self.manifest.as_ref().and_then(|m| m.parent()) {
            app.insert_resource(BodyRoot(root.to_path_buf(), self.manifest.clone()));
        }
        app.add_plugins(EguiPlugin::default())
            .init_resource::<Loadout>()
            .init_resource::<Ground>()
            .insert_resource(Hud {
                icons: hud::Icons::from_manifest(self.manifest.as_deref().unwrap_or("".as_ref())),
                state: hud::HudState::default(),
            })
            .insert_resource(Autoplay {
                enabled: self.autoplay,
                frame: 0,
            })
            .add_systems(Startup, (install_body, install_sounds))
            .add_systems(
                Update,
                (
                    refresh_loadout,
                    cursor_ground,
                    hand_keys,
                    autoplay,
                    outfit::sync_outfits,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    ghost_camera,
                    players::walk_bob,
                    players::tint_ghosts,
                    sounds::door_sounds,
                    sounds::item_sounds,
                    combat::attack_and_heal,
                ),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (loadout_window, players::other_bars),
            );
    }
}

#[derive(Resource)]
struct BodyRoot(std::path::PathBuf, Option<std::path::PathBuf>);

fn install_sounds(
    root: Option<Res<BodyRoot>>,
    mut audio: ResMut<Assets<bevy::audio::AudioSource>>,
    mut commands: Commands,
) {
    if let Some(root) = root {
        commands.insert_resource(sounds::Sounds::load(&root.0, &mut audio));
    }
}

fn install_body(
    root: Option<Res<BodyRoot>>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) {
    if let Some(root) = root {
        body::install(&root.0, root.1.as_deref(), &mut images, &mut commands);
    }
}

/// Точка пола под курсором мыши (в метрах), если курсор над миром.
#[derive(Resource, Default)]
struct Ground(Option<[f32; 3]>);

/// Пересечение луча с горизонтальной плоскостью на высоте `y`.
fn ground_point(origin: Vec3, dir: Vec3, y: f32) -> Option<[f32; 3]> {
    if dir.y.abs() < 1e-4 {
        return None;
    }
    let t = (y - origin.y) / dir.y;
    (t > 0.0).then(|| (origin + dir * t).to_array())
}

fn cursor_ground(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    net: Option<NonSend<Net>>,
    mut ground: ResMut<Ground>,
) {
    ground.0 = None;
    let (Some(net), Ok(window), Ok((camera, transform))) =
        (net, windows.single(), cameras.single())
    else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(transform, cursor) else {
        return;
    };
    let feet = net.core.position();
    ground.0 = ground_point(ray.origin, *ray.direction, feet.y + hud::GROUND_LIFT_M);
}

#[derive(Resource)]
struct Hud {
    icons: hud::Icons,
    state: hud::HudState,
}

#[derive(Resource)]
struct Autoplay {
    enabled: bool,
    frame: u32,
}

/// Обновить копию состояния из реплик сервера.
fn refresh_loadout(net: Option<NonSend<Net>>, time: Res<Time>, mut loadout: ResMut<Loadout>) {
    let Some(net) = net else { return };
    let Some(me) = net.core.net_id() else { return };
    loadout.hands = net
        .core
        .component(me, HANDS_COMPONENT)
        .and_then(decode_hands);
    loadout.inventory = net
        .core
        .component(me, INVENTORY_COMPONENT)
        .and_then(decode_inventory);
    let health = net
        .core
        .component(me, gameplay::HEALTH_COMPONENT)
        .and_then(gameplay::decode_health);
    if health != loadout.health {
        loadout.health_at = time.elapsed_secs();
        loadout.health = health;
    }
    let view = net
        .core
        .component(me, gameplay::DOAFTER_COMPONENT)
        .and_then(gameplay::decode_doafter);
    loadout.doafter = match (view, loadout.doafter.take()) {
        (Some(v), Some((old, since))) if old == v => Some((old, since)),
        (Some(v), _) => Some((v, time.elapsed_secs())),
        (None, _) => None,
    };
}

fn hand_keys(keys: Res<ButtonInput<KeyCode>>, loadout: Res<Loadout>, net: Option<NonSendMut<Net>>) {
    let Some(mut net) = net else { return };
    let active = loadout.hands.as_ref().map_or(0, |h| h.active() as u8);
    if keys.just_pressed(KeyCode::KeyG) {
        net.send_command(CMD_DROP, drop_data(active));
    }
    if keys.just_pressed(KeyCode::Escape) && loadout.doafter.is_some() {
        net.send_command(gameplay::CMD_CANCEL, Vec::new());
    }
    if keys.just_pressed(KeyCode::KeyX) {
        net.send_command(CMD_SWAP_HANDS, vec![0]);
    }
    for (key, hand) in [(KeyCode::Digit1, 0u8), (KeyCode::Digit2, 1u8)] {
        if keys.just_pressed(key) {
            net.send_command(CMD_SELECT_HAND, vec![hand]);
        }
    }
}

/// Ближайший предмет на земле в радиусе `range` от груди игрока.
fn nearest_item(net: &Net, range: f32) -> Option<ObjectId> {
    let world = net.core.world()?;
    let me = net.core.position();
    let chest = MVec3::new(me.x, me.y + 1.0, me.z);
    world
        .authored()
        .filter(|o| matches!(o.kind, astrocore_world::AuthoredKind::Placement { .. }))
        .map(|o| (((o.min() + o.max()) * 0.5).distance(chest), o.id))
        .filter(|(d, _)| *d <= range)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

/// Короткий сценарий для проверки интерфейса: поднять ближайшее и надеть на спину.
fn autoplay(
    mut state: ResMut<Autoplay>,
    mut hud: ResMut<Hud>,
    loadout: Res<Loadout>,
    remote: Res<astrocore_client::RemoteSprites>,
    net: Option<NonSendMut<Net>>,
) {
    if !state.enabled {
        return;
    }
    let Some(mut net) = net else { return };
    if net.core.net_id().is_none() {
        return;
    }
    state.frame += 1;
    match state.frame {
        40 => {
            if let Some(id) = nearest_item(&net, 1.5) {
                net.send_command(gameplay::CMD_INTERACT, gameplay::interact_data(id));
            }
        }
        100 => {
            net.send_command(CMD_EQUIP, equip_data(0, "back"));
        }
        120 => hud.state.clothes_open = true,
        140 | 180 => {
            if let Some(id) = nearest_item(&net, 1.5) {
                net.send_command(gameplay::CMD_INTERACT, gameplay::interact_data(id));
            }
        }
        f if f > 220 && f % 40 == 0 => {
            // Драка для проверки боя: бьём первого увиденного игрока.
            if let Some(id) = remote.0.keys().next() {
                net.send_command(gameplay::CMD_ATTACK, gameplay::attack_data(*id));
            }
        }
        160 | 200 => {
            let bag = loadout.inventory.as_ref().and_then(|v| v.bags.first());
            if let Some(b) = bag {
                net.send_command(gameplay::CMD_STORE, gameplay::store_data(0, b.id));
            }
        }
        _ => {}
    }
}

/// Худ: руки и быстрые слоты внизу, окно одежды, сетки сумок.
/// Вид призрака — сверху: камера высоко над персонажем и смотрит вниз. Включается один раз при
/// смерти (приближение и поворот потом остаются за игроком), с возрождением возвращается вид от
/// первого лица.
fn ghost_camera(
    loadout: Res<Loadout>,
    look: Option<ResMut<astrocore_client::Look>>,
    mut rigs: Query<&mut astrocore_render::CameraRig>,
    mut was_ghost: Local<bool>,
) {
    let Some(mut look) = look else { return };
    let ghost = loadout.health.as_ref().is_some_and(|h| h.ghost);
    if ghost {
        look.first_person = false;
        if !*was_ghost {
            look.pitch = 1.25;
            for mut rig in &mut rigs {
                rig.distance = 14.0;
            }
        }
    } else if *was_ghost {
        look.first_person = true;
        look.pitch = 0.0;
    }
    *was_ghost = ghost;
}

fn loadout_window(
    mut contexts: EguiContexts,
    loadout: Res<Loadout>,
    mut hud: ResMut<Hud>,
    look: Option<Res<astrocore_client::Look>>,
    ground: Res<Ground>,
    time: Res<Time>,
    net: Option<NonSendMut<Net>>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    let Some(mut net) = net else {
        return Ok(());
    };
    // Призрак летает сквозь стены: клиент предсказывает движение так же, как сервер.
    let ghost = loadout.health.as_ref().is_some_and(|h| h.ghost);
    net.core.set_noclip(ghost);
    if let Some(h) = loadout.health.as_ref().filter(|h| h.ghost) {
        let left = (h.respawn_in - (time.elapsed_secs() - loadout.health_at)).max(0.0);
        egui::Area::new(egui::Id::new("ghost_panel"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 24.0))
            .show(&ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("Вы призрак")
                            .size(32.0)
                            .color(egui::Color32::from_rgb(170, 200, 255)),
                    );
                    if left > 0.0 {
                        ui.label(format!("Возродиться можно через {left:.0} с"));
                    } else if ui.button("Возродиться").clicked() {
                        net.send_command(gameplay::CMD_RESPAWN, Vec::new());
                    }
                });
            });
    }
    let progress = loadout.doafter.as_ref().map(|(v, since)| {
        let label = match v.kind.as_str() {
            "equip" => "Надеваем…",
            "unequip" => "Снимаем…",
            "pry" => "Вскрываем…",
            _ => "Действие…",
        };
        (
            label,
            ((time.elapsed_secs() - since) / v.delay_s.max(0.01)).min(1.0),
        )
    });
    let Hud { icons, state } = &mut *hud;
    state.health = loadout.health.clone();
    let commands = hud::draw(
        &ctx,
        icons,
        state,
        loadout.hands.as_ref(),
        loadout.inventory.as_ref(),
        look.is_some_and(|l| l.first_person),
        ground.0,
        progress.as_ref().map(|(label, f)| hud::Progress {
            label,
            fraction: *f,
        }),
    );
    for (kind, data) in commands {
        net.send_command(kind, data);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ray_hits_the_floor_plane_only_in_front_of_the_camera() {
        let p = ground_point(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, -1.0, -1.0), 0.0).unwrap();
        assert!((p[0]).abs() < 1e-5 && (p[1]).abs() < 1e-5 && (p[2] + 2.0).abs() < 1e-5);
        assert!(ground_point(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 1.0, -1.0), 0.0).is_none());
        assert!(ground_point(Vec3::new(0.0, 2.0, 0.0), Vec3::X, 0.0).is_none());
    }
}
