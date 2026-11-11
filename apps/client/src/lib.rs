use astrocore_client::Net;
use astrocore_core::{NetworkEntityId, ObjectId};
use astrocore_math::glam::Vec3 as MVec3;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use gameplay::{
    CMD_DROP, CMD_EQUIP, CMD_SELECT_HAND, CMD_STORE, CMD_SWAP_HANDS, CMD_TAKE, CMD_UNEQUIP,
    HANDS_COMPONENT, Hands, INVENTORY_COMPONENT, InventoryView, decode_hands, decode_inventory,
    drop_data, equip_data, store_data, take_data, unequip_data,
};

/// Что игрок сейчас держит и носит, как это видит клиент (копия серверных реплик).
#[derive(Resource, Default)]
pub struct Loadout {
    pub hands: Option<Hands>,
    pub inventory: Option<InventoryView>,
}

/// Клиент билда: клавиши рук и окно «Руки и инвентарь».
///
/// E — взаимодействие с ближайшим предметом или дверью (обрабатывает клиент движка),
/// G — выбросить предмет из активной руки, X — сменить активную руку, 1 и 2 — выбрать руку.
pub struct BuildClientPlugin {
    /// Отладка: сам выполняет короткий сценарий (поднять ближайшее, надеть на спину).
    pub autoplay: bool,
}

impl Plugin for BuildClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<Loadout>()
            .insert_resource(Autoplay {
                enabled: self.autoplay,
                frame: 0,
            })
            .add_systems(Update, (refresh_loadout, hand_keys, autoplay).chain())
            .add_systems(EguiPrimaryContextPass, loadout_window);
    }
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
fn autoplay(mut state: ResMut<Autoplay>, net: Option<NonSendMut<Net>>) {
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
        _ => {}
    }
}

fn item_label(proto: &str) -> String {
    proto.split_once(':').map_or(proto, |(_, n)| n).to_string()
}

/// Окно «Руки и инвентарь»: руки, надетое и содержимое сумок с кнопками действий.
fn loadout_window(
    mut contexts: EguiContexts,
    loadout: Res<Loadout>,
    net: Option<NonSendMut<Net>>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    let Some(mut net) = net else {
        return Ok(());
    };
    let Some(hands) = loadout.hands.as_ref() else {
        egui::Window::new("Руки и инвентарь").show(&ctx, |ui| {
            ui.label("Руки пусты. E — взять предмет или открыть дверь.");
        });
        return Ok(());
    };
    let view = loadout.inventory.as_ref();
    let slots: Vec<String> = view
        .map(|v| v.inventory.iter().map(|(d, _)| d.name.clone()).collect())
        .unwrap_or_default();
    let mut commands: Vec<(&'static str, Vec<u8>)> = Vec::new();

    egui::Window::new("Руки и инвентарь")
        .default_width(300.0)
        .show(&ctx, |ui| {
            ui.heading("Руки");
            for (i, name, item) in hands.iter() {
                let active = i == hands.active();
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(
                            active,
                            format!("{} {name}", if active { "▶" } else { " " }),
                        )
                        .clicked()
                    {
                        commands.push((CMD_SELECT_HAND, vec![i as u8]));
                    }
                    match item {
                        None => {
                            ui.weak("пусто");
                        }
                        Some(it) => {
                            ui.label(item_label(it.prototype.as_str()));
                            if ui.small_button("выбросить").clicked() {
                                commands.push((CMD_DROP, drop_data(i as u8)));
                            }
                            ui.menu_button("надеть", |ui| {
                                for s in &slots {
                                    if ui.button(s).clicked() {
                                        commands.push((CMD_EQUIP, equip_data(i as u8, s)));
                                        ui.close();
                                    }
                                }
                            });
                            if let Some(v) = view {
                                for bag in &v.bags {
                                    if ui
                                        .small_button(format!("в сумку {}", short(bag.id)))
                                        .clicked()
                                    {
                                        commands.push((CMD_STORE, store_data(i as u8, bag.id)));
                                    }
                                }
                            }
                        }
                    }
                });
            }
            if let Some(v) = view {
                ui.separator();
                ui.heading("Надето");
                let mut any = false;
                for (def, item) in v.inventory.iter() {
                    if let Some(it) = item {
                        any = true;
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "{}: {}",
                                def.name,
                                item_label(it.prototype.as_str())
                            ));
                            if ui.small_button("снять").clicked() {
                                commands.push((CMD_UNEQUIP, unequip_data(&def.name)));
                            }
                        });
                    }
                }
                if !any {
                    ui.weak("ничего не надето");
                }
                for bag in &v.bags {
                    ui.separator();
                    ui.heading(format!("Сумка {}", short(bag.id)));
                    if bag.items.is_empty() {
                        ui.weak("пусто");
                    }
                    for st in &bag.items {
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "{} ({},{}{})",
                                item_label(st.item.prototype.as_str()),
                                st.location.x,
                                st.location.y,
                                if st.location.rotated { ", ↻" } else { "" }
                            ));
                            if ui.small_button("достать").clicked() {
                                commands.push((CMD_TAKE, take_data(bag.id, st.item.id)));
                            }
                        });
                    }
                }
            }
            ui.separator();
            ui.small("E — взаимодействие · G — выбросить · X — сменить руку · 1/2 — выбрать руку");
        });
    for (kind, data) in commands {
        net.send_command(kind, data);
    }
    let _: Option<NetworkEntityId> = None;
    Ok(())
}

fn short(id: ObjectId) -> String {
    let v = id.as_uuid().as_u128();
    format!("#{:04x}", v & 0xFFFF)
}
