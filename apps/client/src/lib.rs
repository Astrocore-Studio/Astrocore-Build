use astrocore_client::Net;
use astrocore_core::ObjectId;
use astrocore_math::glam::Vec3 as MVec3;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
mod alarm_visuals;
mod alerts_ui;
mod body;
mod combat;
mod fire;
mod hud;
mod lights;
mod outfit;
mod players;
mod power_ui;
mod sounds;
mod statusfx;
use gameplay::{
    CMD_DROP, CMD_EQUIP, CMD_SELECT_HAND, CMD_SWAP_HANDS, HANDS_COMPONENT, Hands,
    INVENTORY_COMPONENT, InventoryView, decode_hands, decode_inventory, drop_data, equip_data,
};

/// Что игрок сейчас держит и носит, как это видит клиент (копия серверных реплик).
#[derive(Resource, Default)]
pub struct Loadout {
    pub atmos_alerts: Option<gameplay::alerts::View>,
    pub alarm_visuals: Option<gameplay::alerts::Visuals>,
    pub power_monitor: Option<gameplay::power_monitor::View>,
    pub power_map: Option<gameplay::power_monitor::MapView>,
    pub power_cables: Option<gameplay::power_monitor::CableView>,
    pub hands: Option<Hands>,
    pub inventory: Option<InventoryView>,
    /// Идущее действие с таймером и время (с игры), когда клиент о нём узнал.
    pub doafter: Option<(gameplay::DoAfterView, f32)>,
    pub health: Option<gameplay::HealthView>,
    pub vitals: Option<gameplay::Vitals>,
    /// Открытый торговый автомат.
    pub vending: Option<gameplay::VendingView>,
    /// Список должностей и текущая.
    pub jobs: Option<gameplay::JobsView>,
    /// Лобби перед раундом.
    pub lobby: Option<gameplay::LobbyView>,
    /// Чат: то, что слышал игрок.
    pub chat: Option<gameplay::ChatView>,
    /// Сводка конца раунда.
    pub roundend: Option<gameplay::RoundEndView>,
    /// Открытая консоль выдачи ID.
    pub idconsole: Option<gameplay::IdConsoleView>,
    /// Раздатчик реагентов и анализатор здоровья.
    pub dispenser: Option<gameplay::DispenserView>,
    pub scan: Option<gameplay::ScanView>,
    pub chemhand: Option<gameplay::ChemHandView>,
    pub lab: Option<gameplay::LabView>,
    pub cryo: Option<gameplay::CryoView>,
    /// Подсказки на экране: текст и время появления; номер последней обработанной пачки.
    pub toasts: Vec<(String, f32)>,
    pub fx_seq: u32,
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
        // Пока поле ввода в фокусе, клавиши не доходят до игры (иначе W/A/S/D двигали бы персонажа).
        app.insert_resource(bevy_egui::EguiGlobalSettings {
            enable_absorb_bevy_input_system: true,
            ..Default::default()
        })
        .add_plugins(EguiPlugin::default())
        .init_resource::<Loadout>()
        .init_resource::<Ground>()
        .insert_resource(Hud {
            icons: hud::Icons::from_manifest(self.manifest.as_deref().unwrap_or("".as_ref())),
            state: hud::HudState::default(),
            power: power_ui::UiState::default(),
            alerts: alerts_ui::UiState::default(),
        })
        .insert_resource(Autoplay {
            enabled: self.autoplay,
            frame: 0,
        })
        .init_resource::<fire::FxAssets>()
        .init_resource::<lights::FlashlightOn>()
        .add_systems(Startup, (install_body, install_sounds))
        .add_systems(Last, alarm_visuals::sync)
        .add_systems(PreUpdate, smoke_inputs.after(bevy::input::InputSystems))
        .add_systems(
            Update,
            (
                refresh_loadout,
                cursor_ground,
                aim_target,
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
                sounds::trace_alarm_playback,
                fx_events,
                combat::attack_and_heal,
                fire::sync_fx,
                fire::animate_flames,
                lights::sync_lights,
                lights::flashlight,
            ),
        )
        .add_systems(
            PostUpdate,
            statusfx::camera_shake.before(bevy::transform::TransformSystems::Propagate),
        )
        .add_systems(
            EguiPrimaryContextPass,
            (
                loadout_window,
                players::other_bars,
                statusfx::overlay,
                lights::vignette,
            ),
        );
    }
}

#[derive(Resource)]
struct BodyRoot(std::path::PathBuf, Option<std::path::PathBuf>);

/// Подсказки и звуки от сервера (`build.fx`): новая пачка — показать подсказки, сыграть звуки.
fn fx_events(
    net: Option<NonSend<Net>>,
    time: Res<Time>,
    mut loadout: ResMut<Loadout>,
    sounds: Option<ResMut<sounds::Sounds>>,
    mut audio: ResMut<Assets<bevy::audio::AudioSource>>,
    mut commands: Commands,
) {
    let Some(net) = net else { return };
    let Some(me) = net.core.net_id() else { return };
    let Some(batch) = net
        .core
        .component(me, gameplay::FX_COMPONENT)
        .and_then(gameplay::decode_fx)
    else {
        return;
    };
    if batch.seq == loadout.fx_seq {
        return;
    }
    loadout.fx_seq = batch.seq;
    let now = time.elapsed_secs();
    for p in batch.popups {
        loadout.toasts.push((p, now));
    }
    if let Some(mut sounds) = sounds {
        let here = net.core.position();
        for s in batch.sounds {
            let d = here.distance(Vec3::from_array(s.at));
            sounds.play_path(&mut commands, &mut audio, &s.path, d, s.volume_db);
        }
    }
    loadout.toasts.retain(|(_, t)| now - t < 4.0);
}

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
        alarm_visuals::install(&root.0, &mut images, &mut commands);
    }
}

/// Точка пола под курсором мыши (в метрах), если курсор над миром.
#[derive(Resource, Default)]
pub(crate) struct Ground(pub(crate) Option<[f32; 3]>);

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

/// Луч против ящика: расстояние до входа в ящик `[min, max]`.
fn ray_box(origin: [f32; 3], dir: [f32; 3], min: [f32; 3], max: [f32; 3]) -> Option<f32> {
    let (mut t0, mut t1) = (0.0_f32, f32::INFINITY);
    for i in 0..3 {
        if dir[i].abs() < 1e-6 {
            if origin[i] < min[i] || origin[i] > max[i] {
                return None;
            }
            continue;
        }
        let (a, b) = ((min[i] - origin[i]) / dir[i], (max[i] - origin[i]) / dir[i]);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// Дальность подсказки «что под прицелом», м.
const AIM_RANGE_M: f32 = 3.5;

/// Что под прицелом (в первом лице — центр экрана, в третьем — курсор): название объекта для
/// подсказки (`ContextMenu`/подпись SS14 3D).
fn aim_target(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    look: Option<Res<astrocore_client::Look>>,
    net: Option<NonSend<Net>>,
    mut hud: ResMut<Hud>,
) {
    hud.state.aim = None;
    let (Some(net), Ok(window), Ok((camera, transform))) =
        (net, windows.single(), cameras.single())
    else {
        return;
    };
    let first_person = look.is_some_and(|l| l.first_person);
    let ray = if first_person {
        Ray3d::new(transform.translation(), transform.forward())
    } else {
        let Some(cursor) = window.cursor_position() else {
            return;
        };
        let Ok(ray) = camera.viewport_to_world(transform, cursor) else {
            return;
        };
        ray
    };
    let Some(world) = net.core.world() else {
        return;
    };
    let (origin, dir) = (ray.origin.to_array(), ray.direction.as_vec3().to_array());
    let mut best: Option<(f32, String)> = None;
    for o in world.authored() {
        let astrocore_world::AuthoredKind::Placement { prototype } = &o.kind else {
            continue;
        };
        let Some(info) = hud.icons.aim_info(prototype.as_str()) else {
            continue;
        };
        // Границы на карте — след предмета на полу; цель — видимый объём спрайта: ширина и
        // высота из манифеста (плоские предметы — тонкая плитка у пола).
        let centre = (o.min() + o.max()) * 0.5;
        let half = (info.size[0] * 0.5).max((o.max().x - o.min().x) * 0.5) + 0.05;
        let height = if info.flat {
            0.15
        } else {
            info.size[1].max(0.3)
        };
        let min = [centre.x - half, o.min().y, centre.z - half];
        let max = [centre.x + half, o.min().y + height, centre.z + half];
        let Some(t) = ray_box(origin, dir, min, max) else {
            continue;
        };
        if t <= AIM_RANGE_M && best.as_ref().is_none_or(|(b, _)| t < *b) {
            best = Some((t, info.name.clone()));
        }
    }
    hud.state.aim = best.map(|(_, n)| n);
}

#[derive(Resource)]
pub(crate) struct Hud {
    pub(crate) icons: hud::Icons,
    state: hud::HudState,
    power: power_ui::UiState,
    alerts: alerts_ui::UiState,
}

#[derive(Resource)]
struct Autoplay {
    enabled: bool,
    frame: u32,
}

fn smoke_inputs(
    autoplay: Res<Autoplay>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut motion: ResMut<bevy::input::mouse::AccumulatedMouseMotion>,
) {
    if autoplay.enabled && std::env::var_os("ASTRO_SMOKE_EXIT").is_some() {
        keys.reset_all();
        mouse.reset_all();
        motion.delta = Vec2::ZERO;
    }
}

/// Обновить копию состояния из реплик сервера.
fn refresh_loadout(net: Option<NonSend<Net>>, time: Res<Time>, mut loadout: ResMut<Loadout>) {
    let Some(net) = net else { return };
    let Some(me) = net.core.net_id() else { return };
    loadout.atmos_alerts = net
        .core
        .component(me, gameplay::alerts::COMPONENT)
        .and_then(gameplay::alerts::decode);
    loadout.alarm_visuals = net
        .core
        .component(me, gameplay::alerts::VISUAL_COMPONENT)
        .and_then(gameplay::alerts::decode);
    loadout.power_monitor = net
        .core
        .component(me, gameplay::power_monitor::COMPONENT)
        .and_then(gameplay::power_monitor::decode);
    loadout.power_map = net
        .core
        .component(me, gameplay::power_monitor::MAP_COMPONENT)
        .and_then(gameplay::power_monitor::decode);
    loadout.power_cables = net
        .core
        .component(me, gameplay::power_monitor::CABLE_COMPONENT)
        .and_then(gameplay::power_monitor::decode);
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
    loadout.vitals = net
        .core
        .component(me, gameplay::VITALS_COMPONENT)
        .and_then(gameplay::decode_vitals);
    if health != loadout.health {
        loadout.health_at = time.elapsed_secs();
        loadout.health = health;
    }
    let view = net
        .core
        .component(me, gameplay::DOAFTER_COMPONENT)
        .and_then(gameplay::decode_doafter);
    loadout.dispenser = net
        .core
        .component(me, gameplay::DISPENSER_COMPONENT)
        .and_then(gameplay::decode_dispenser);
    loadout.cryo = net
        .core
        .component(me, gameplay::CRYO_COMPONENT)
        .and_then(gameplay::decode_cryo);
    loadout.lab = net
        .core
        .component(me, gameplay::LAB_COMPONENT)
        .and_then(gameplay::decode_lab);
    loadout.chemhand = net
        .core
        .component(me, gameplay::CHEMHAND_COMPONENT)
        .and_then(gameplay::decode_chemhand);
    loadout.scan = net
        .core
        .component(me, gameplay::SCAN_COMPONENT)
        .and_then(gameplay::decode_scan);
    loadout.idconsole = net
        .core
        .component(me, gameplay::IDCONSOLE_COMPONENT)
        .and_then(gameplay::decode_view);
    loadout.chat = net
        .core
        .component(me, gameplay::CHAT_COMPONENT)
        .and_then(gameplay::decode_chat);
    loadout.roundend = net
        .core
        .component(me, gameplay::ROUNDEND_COMPONENT)
        .and_then(gameplay::decode_end);
    loadout.lobby = net
        .core
        .component(me, gameplay::LOBBY_COMPONENT)
        .and_then(gameplay::decode_lobby);
    loadout.jobs = net
        .core
        .component(me, gameplay::JOBS_COMPONENT)
        .and_then(gameplay::decode_jobs);
    loadout.vending = net
        .core
        .component(me, gameplay::VENDING_COMPONENT)
        .and_then(gameplay::decode_vending);
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
    if keys.just_pressed(KeyCode::KeyF) {
        net.send_command(gameplay::CMD_USE, gameplay::use_data(active));
    }
    if keys.just_pressed(KeyCode::KeyV) {
        net.send_command(gameplay::CMD_SPILL, vec![active]);
    }
    if keys.just_pressed(KeyCode::KeyH) {
        net.send_command(gameplay::CMD_SHAKE, Vec::new());
    }
    if keys.just_pressed(KeyCode::KeyL) {
        net.send_command(gameplay::CMD_LOCK, Vec::new());
    }
    if keys.just_pressed(KeyCode::KeyZ) {
        net.send_command(gameplay::CMD_RESIST, Vec::new());
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
    mut commands: Commands,
    mut state: ResMut<Autoplay>,
    mut hud: ResMut<Hud>,
    loadout: Res<Loadout>,
    remote: Res<astrocore_client::RemoteSprites>,
    net: Option<NonSendMut<Net>>,
    mut look: Option<ResMut<astrocore_client::Look>>,
) {
    if !state.enabled {
        return;
    }
    let Some(mut net) = net else { return };
    if net.core.net_id().is_none() {
        return;
    }
    // Welcome can arrive before the map snapshot; debug interactions need the world.
    if net.core.world().is_none() {
        return;
    }
    if loadout.hands.is_none() || loadout.health.is_none() {
        return;
    }
    state.frame += 1;
    let exit_frame = std::env::var("ASTRO_SMOKE_EXIT")
        .ok()
        .map(|value| value.parse::<u32>().unwrap_or(380).max(380));
    if exit_frame.is_some_and(|frame| state.frame >= frame) {
        commands.queue(|world: &mut World| {
            world.write_message(bevy::app::AppExit::Success);
        });
        return;
    }
    // Отладка: снимки окна в файлы (ASTRO_SHOT=каталог, кадры 260 и 340).
    if let Some(dir) = std::env::var_os("ASTRO_SHOT")
        && matches!(state.frame, 260 | 340)
    {
        let path = std::path::Path::new(&dir).join(format!("frame{}.png", state.frame));
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(path));
    }
    // Отладка: ASTRO_LOOK=yaw,pitch держит взгляд (yaw 0 — на север, к −Z); ASTRO_LOOK_DOWN — вниз.
    if let Some(l) = look.as_mut() {
        if let Ok(v) = std::env::var("ASTRO_LOOK") {
            let p: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if let [yaw, pitch] = p[..] {
                l.yaw = yaw;
                l.pitch = pitch;
            }
        } else if std::env::var_os("ASTRO_LOOK_DOWN").is_some() {
            l.pitch = 0.9;
        }
    }
    // Отладка: ASTRO_LOOK_AT_ID=<hex id> наводит взгляд на объект карты (для снимков вблизи).
    if let Ok(h) = std::env::var("ASTRO_LOOK_AT_ID")
        && let Some(id) = u128::from_str_radix(&h, 16).ok().map(ObjectId::from_u128)
        && let Some(world) = net.core.world()
        && let Some(object) = world.authored_object(id)
        && let Some(look) = look.as_mut()
    {
        let at = (object.min() + object.max()) * 0.5 + MVec3::Y * 0.3;
        let delta = at - net.core.position() - MVec3::Y * 0.9;
        look.yaw = (-delta.x).atan2(-delta.z);
        look.pitch = (-delta.y).atan2(delta.x.hypot(delta.z));
    }
    // Отладка: сказать фразу в чат (ASTRO_AUTOPLAY_SAY=текст).
    if let Ok(text) = std::env::var("ASTRO_AUTOPLAY_SAY")
        && matches!(state.frame, 80 | 100)
    {
        net.send_command(
            gameplay::CMD_CHAT,
            gameplay::chat_data(gameplay::Channel::Say, &text),
        );
    }
    // Отладка: сразу взять должность (для снимков).
    if let Ok(job) = std::env::var("ASTRO_AUTOPLAY_JOB")
        && state.frame == 60
    {
        net.send_command(gameplay::CMD_LOBBY_READY, Vec::new());
        net.send_command(gameplay::CMD_JOB, job.into_bytes());
    }
    // Отладка: открыть окно ближайшей машины (ASTRO_AUTOPLAY_OPEN=ChemMaster|CryoPod); для капсулы
    // затем лечь в неё самому.
    if let Ok(want) = std::env::var("ASTRO_AUTOPLAY_OPEN") {
        if state.frame == 150
            && let Some(world) = net.core.world()
        {
            let _ = world;
            let me = net.core.position();
            let id = std::env::var("ASTRO_OPEN_ID")
                .ok()
                .and_then(|h| u128::from_str_radix(&h, 16).ok())
                .map(ObjectId::from_u128)
                .or_else(|| nearest_item(&net, 2.5));
            eprintln!("ASTRO_OPEN: {want} {id:?} me={me:?}");
            if let Some(id) = id {
                net.send_command(gameplay::CMD_INTERACT, gameplay::interact_data(id));
            }
        }
        if state.frame == 200
            && want == "CryoPod"
            && let Some(me) = net.core.net_id()
        {
            net.send_command(gameplay::CMD_CRYO_PUT, me.0.to_le_bytes().to_vec());
        }
        if want == "ComputerPowerMonitoring" {
            use gameplay::power_monitor::{self as power, Group};
            if state.frame == 200 {
                net.send_command(power::CMD_FOCUS, power::focus_data(Group::Apc, None));
            }
            // Wait for the replicated tab before selecting; packet arrival is not frame-bound.
            if (230..320).contains(&state.frame)
                && state.frame.is_multiple_of(15)
                && let Some(view) = loadout
                    .power_monitor
                    .as_ref()
                    .filter(|v| v.group == Group::Apc && v.focus.is_none())
                && let Some(entry) = view
                    .entries
                    .iter()
                    .max_by(|a, b| a.power.total_cmp(&b.power))
            {
                net.send_command(
                    power::CMD_FOCUS,
                    power::focus_data(Group::Apc, Some(entry.id)),
                );
            }
            if state.frame == 340 {
                if let Some(view) = &loadout.power_monitor {
                    eprintln!(
                        "ASTRO_POWER: group={:?} focus={:?} entries={} sources={} loads={}",
                        view.group,
                        view.focus,
                        view.entries.len(),
                        view.sources.len(),
                        view.loads.len()
                    );
                } else {
                    eprintln!("ASTRO_POWER: console did not open");
                }
            }
        }
        if want == "ComputerAlert" {
            use gameplay::alerts::{self as alerts, Group};
            if (200..320).contains(&state.frame)
                && state.frame.is_multiple_of(15)
                && let Some(view) = loadout.atmos_alerts.as_ref().filter(|v| v.focus.is_none())
                && let Some(entry) = view
                    .entries
                    .iter()
                    .filter(|e| e.group == Group::AirAlarm && !e.seeds.is_empty())
                    .max_by_key(|e| e.level)
            {
                net.send_command(alerts::CMD_FOCUS, alerts::focus_data(Some(entry.id)));
            }
            if state.frame == 340 {
                if let Some(view) = &loadout.atmos_alerts {
                    eprintln!(
                        "ASTRO_ALERTS: air={} fire={} focus={:?} reading={:?}",
                        view.entries
                            .iter()
                            .filter(|e| e.group == Group::AirAlarm)
                            .count(),
                        view.entries
                            .iter()
                            .filter(|e| e.group == Group::FireAlarm)
                            .count(),
                        view.focus,
                        view.reading
                    );
                } else {
                    eprintln!("ASTRO_ALERTS: console did not open");
                }
            }
        }
        if want == "FireAlarm" {
            let alarm = std::env::var("ASTRO_OPEN_ID")
                .ok()
                .and_then(|h| u128::from_str_radix(&h, 16).ok())
                .map(ObjectId::from_u128);
            if let Some(id) = alarm
                && let Some(world) = net.core.world()
                && let Some(object) = world.authored_object(id)
                && let Some(look) = look.as_mut()
            {
                let at = (object.min() + object.max()) * 0.5 + MVec3::Y * 0.45;
                let delta = at - net.core.position() - MVec3::Y * 0.9;
                look.yaw = (-delta.x).atan2(-delta.z);
                look.pitch = (-delta.y).atan2(delta.x.hypot(delta.z));
            }
            if state.frame == 300
                && let Some(id) = alarm
            {
                net.send_command(gameplay::CMD_INTERACT, gameplay::interact_data(id));
            }
            if matches!(state.frame, 260 | 340) {
                let level = loadout
                    .alarm_visuals
                    .as_ref()
                    .and_then(|v| v.states.iter().find(|(id, _)| Some(*id) == alarm))
                    .map(|(_, level)| level);
                eprintln!(
                    "ASTRO_ALARMS: frame={} id={alarm:?} level={level:?}",
                    state.frame
                );
            }
        }
        return;
    }
    // Сценарий огня для снимков: поднять сварку и зажечь.
    if std::env::var_os("ASTRO_AUTOPLAY_FIRE").is_some() {
        match state.frame {
            30 => {
                if let Some(id) = nearest_item(&net, 2.5) {
                    net.send_command(gameplay::CMD_INTERACT, gameplay::interact_data(id));
                }
            }
            60 => {
                let hand = loadout.hands.as_ref().map_or(0, |h| h.active() as u8);
                if std::env::var_os("ASTRO_AUTOPLAY_SPILL").is_some() {
                    net.send_command(gameplay::CMD_SPILL, vec![hand]);
                } else {
                    net.send_command(gameplay::CMD_USE, gameplay::use_data(hand));
                }
            }
            _ => {}
        }
        return;
    }
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
    mut loadout: ResMut<Loadout>,
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
    net.core.set_speed(if ghost {
        1.0
    } else {
        loadout.vitals.as_ref().map_or(1.0, |v| v.speed)
    });
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
    // Лобби: экран на весь кадр, пока игрок не вышел на станцию.
    if let Some(lobby) = loadout.lobby.as_ref().filter(|l| l.in_lobby) {
        let at = egui::pos2(16.0, (ctx.content_rect().max.y - 230.0).max(16.0));
        let mut cmds = hud::lobby_screen(&ctx, lobby);
        cmds.extend(hud::chat_panel(
            &ctx,
            &mut hud.state,
            loadout.chat.as_ref(),
            at,
        ));
        for (kind, data) in cmds {
            net.send_command(kind, data);
        }
        return Ok(());
    }
    let progress = loadout.doafter.as_ref().map(|(v, since)| {
        let label = match v.kind.as_str() {
            "equip" => "Надеваем…",
            "unequip" => "Снимаем…",
            "pry" => "Вскрываем…",
            "heal" => "Лечим…",
            "eat" => "Едим…",
            _ => "Действие…",
        };
        (
            label,
            ((time.elapsed_secs() - since) / v.delay_s.max(0.01)).min(1.0),
        )
    });
    let Hud {
        icons,
        state,
        power,
        alerts,
    } = &mut *hud;
    state.health = loadout.health.clone();
    state.vitals = loadout.vitals.clone();
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
    let mut commands = commands;
    commands.extend(power_ui::draw(
        &ctx,
        power,
        icons,
        loadout.power_monitor.as_ref(),
        loadout.power_map.as_ref(),
        loadout.power_cables.as_ref(),
        net.core.world(),
    ));
    commands.extend(alerts_ui::draw(
        &ctx,
        alerts,
        loadout.atmos_alerts.as_ref(),
        net.core.world(),
    ));
    commands.extend(hud::vending_window(&ctx, icons, loadout.vending.as_ref()));
    let hand = loadout.hands.as_ref().map_or(0, |h| h.active() as u8);
    commands.extend(hud::idconsole_window(
        &ctx,
        state,
        loadout.idconsole.as_ref(),
        hand,
    ));
    if let Some(v) = loadout.roundend.as_ref() {
        hud::roundend_panel(&ctx, v);
    }
    let chat_at = egui::pos2(16.0, (ctx.content_rect().max.y - 290.0).max(16.0));
    commands.extend(hud::chat_panel(&ctx, state, loadout.chat.as_ref(), chat_at));
    hud::toasts(&ctx, &mut loadout.toasts, time.elapsed_secs());
    commands.extend(hud::dispenser_window(
        &ctx,
        loadout.dispenser.as_ref(),
        hand,
    ));
    commands.extend(hud::scan_window(&ctx, loadout.scan.as_ref()));
    commands.extend(hud::chemhand_bar(&ctx, loadout.chemhand.as_ref()));
    commands.extend(hud::cryo_window(&ctx, loadout.cryo.as_ref(), hand));
    commands.extend(hud::lab_window(&ctx, loadout.lab.as_ref(), hand));
    commands.extend(hud::jobs_window(&ctx, state, loadout.jobs.as_ref()));
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
