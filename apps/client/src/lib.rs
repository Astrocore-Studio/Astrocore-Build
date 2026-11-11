use astrocore_client::Net;
use astrocore_core::ObjectId;
use astrocore_math::glam::Vec3 as MVec3;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};
mod body;
mod hud;
use gameplay::{
    CMD_DROP, CMD_EQUIP, CMD_SELECT_HAND, CMD_SWAP_HANDS, HANDS_COMPONENT, Hands,
    INVENTORY_COMPONENT, InventoryView, decode_hands, decode_inventory, drop_data, equip_data,
};

/// Что игрок сейчас держит и носит, как это видит клиент (копия серверных реплик).
#[derive(Resource, Default)]
pub struct Loadout {
    pub hands: Option<Hands>,
    pub inventory: Option<InventoryView>,
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
            app.insert_resource(BodyRoot(root.to_path_buf()));
        }
        app.add_plugins(EguiPlugin::default())
            .init_resource::<Loadout>()
            .insert_resource(Hud {
                icons: hud::Icons::from_manifest(self.manifest.as_deref().unwrap_or("".as_ref())),
                state: hud::HudState::default(),
            })
            .insert_resource(Autoplay {
                enabled: self.autoplay,
                frame: 0,
            })
            .add_systems(Startup, install_body)
            .add_systems(Update, (refresh_loadout, hand_keys, autoplay).chain())
            .add_systems(EguiPrimaryContextPass, loadout_window);
    }
}

#[derive(Resource)]
struct BodyRoot(std::path::PathBuf);

fn install_body(
    root: Option<Res<BodyRoot>>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) {
    if let Some(root) = root {
        body::install(&root.0, &mut images, &mut commands);
    }
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
fn refresh_loadout(net: Option<NonSend<Net>>, mut loadout: ResMut<Loadout>) {
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
}

fn hand_keys(keys: Res<ButtonInput<KeyCode>>, loadout: Res<Loadout>, net: Option<NonSendMut<Net>>) {
    let Some(mut net) = net else { return };
    let active = loadout.hands.as_ref().map_or(0, |h| h.active() as u8);
    if keys.just_pressed(KeyCode::KeyG) {
        net.send_command(CMD_DROP, drop_data(active));
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
fn loadout_window(
    mut contexts: EguiContexts,
    loadout: Res<Loadout>,
    mut hud: ResMut<Hud>,
    look: Option<Res<astrocore_client::Look>>,
    net: Option<NonSendMut<Net>>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    let Some(mut net) = net else {
        return Ok(());
    };
    let Hud { icons, state } = &mut *hud;
    let commands = hud::draw(
        &ctx,
        icons,
        state,
        loadout.hands.as_ref(),
        loadout.inventory.as_ref(),
        look.is_some_and(|l| l.first_person),
    );
    for (kind, data) in commands {
        net.send_command(kind, data);
    }
    Ok(())
}
