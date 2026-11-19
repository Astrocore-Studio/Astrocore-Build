//! Окно маппинга (`EntitySpawnWindow` SS14), клавиша F5 у тех, у кого есть флаг `Spawn`:
//! каталог прототипов с поиском и значками; выбранный ставится щелчком по миру (по центру
//! клетки, с Shift — в точку курсора), R поворачивает на четверть, режим удаления убирает объект
//! под курсором. Пока окно открыто, камера смотрит сверху, а обычные щелчки (удар, постройка)
//! не работают. Права проверяет сервер (`systems/mapping.rs`), окно — только подсказка.

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, Key};
use gameplay::admin::AdminView;
use gameplay::systems::mapping::{
    CMD_DELETE, CMD_SPAWN, CMD_TILE, FLAG, SpawnRequest, TILE_FLAG, TileRequest, delete_data,
    spawn_data, tile_data,
};

use crate::hud::{Commands, Icons, TileEntry};

/// Сколько строк каталога показывать за раз (поиск сужает).
const LIST_LIMIT: usize = 300;

/// Вкладка окна маппинга.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Entities,
    Tiles,
}

pub struct MappingState {
    pub open: bool,
    pub tab: Tab,
    search: String,
    /// Выбранный прототип (идентификатор движка).
    pub selected: Option<String>,
    pub rot: u8,
    pub delete: bool,
    /// Имя файла для «Сохранить карту» (`savemap <имя>` на сервере).
    save_name: String,
    /// Выбранная плитка: `Some(None)` — пустая (убрать пол).
    pub tile: Option<Option<String>>,
    /// Класть и потолок из той же плитки.
    pub ceiling: bool,
    /// Уровень, на котором работает маппинг.
    pub level: i32,
    /// Клетка, где нажата кнопка мыши (протяжка прямоугольником).
    pub drag_from: Option<[i32; 2]>,
}

impl Default for MappingState {
    fn default() -> Self {
        Self {
            open: false,
            tab: Tab::Entities,
            search: String::new(),
            selected: None,
            rot: 0,
            delete: false,
            save_name: String::new(),
            tile: None,
            ceiling: true,
            level: 0,
            drag_from: None,
        }
    }
}

/// Может ли игрок пользоваться маппингом (по правам, присланным сервером).
pub fn allowed(admin: Option<&AdminView>) -> bool {
    admin.is_some_and(|a| a.flags.iter().any(|f| f == FLAG))
}

/// Строки каталога под поиском: подстрока в названии или идентификаторе, без учёта регистра.
pub fn filter<'a>(catalog: &'a [(String, String)], search: &str) -> Vec<&'a (String, String)> {
    let q = search.trim().to_lowercase();
    catalog
        .iter()
        .filter(|(id, name)| q.is_empty() || name.to_lowercase().contains(&q) || id.contains(&q))
        .take(LIST_LIMIT)
        .collect()
}

/// Окно маппинга: вкладки «Сущности» и «Плитки», управление картами. Возвращает команды для
/// сервера (карты); ставит, кладёт и удаляет щелчок по миру, см. [`input`].
pub fn panel(
    ctx: &egui::Context,
    st: &mut MappingState,
    icons: &mut Icons,
    admin: Option<&AdminView>,
) -> Commands {
    if !allowed(admin) {
        st.open = false;
        return Vec::new();
    }
    if ctx.input(|i| i.key_pressed(Key::F5)) {
        st.open = !st.open;
    }
    if !st.open {
        return Vec::new();
    }
    let can = |cmd: &str| admin.is_some_and(|a| a.commands.iter().any(|c| c.name == cmd));
    let can_tiles = admin.is_some_and(|a| a.flags.iter().any(|f| f == TILE_FLAG));
    if !can_tiles {
        st.tab = Tab::Entities;
    }
    let mut open = true;
    let mut out: Commands = Vec::new();
    egui::Window::new("Маппинг")
        .id(egui::Id::new("mapping"))
        .default_pos(egui::pos2(ctx.content_rect().max.x - 380.0, 60.0))
        .default_width(340.0)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut st.tab, Tab::Entities, "Сущности");
                if can_tiles {
                    ui.selectable_value(&mut st.tab, Tab::Tiles, "Плитки");
                }
            });
            ui.horizontal(|ui| {
                ui.label("Поиск:");
                ui.add(egui::TextEdit::singleline(&mut st.search).desired_width(f32::INFINITY));
            });
            match st.tab {
                Tab::Entities => entities_head(ui, st, icons),
                Tab::Tiles => tiles_head(ui, st, icons),
            }
            maps_row(ui, st, &can, &mut out);
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| match st.tab {
                    Tab::Entities => entity_list(ctx, ui, st, icons),
                    Tab::Tiles => tile_list(ctx, ui, st, icons),
                });
        });
    if !open {
        st.open = false;
    }
    out
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(11.0)
            .color(Color32::from_gray(150)),
    );
}

fn entities_head(ui: &mut egui::Ui, st: &mut MappingState, icons: &Icons) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut st.delete, "Удаление");
        ui.label(format!("Поворот: {}°", u16::from(st.rot) * 90));
    });
    if st.delete {
        ui.label(
            egui::RichText::new("ЛКМ по объекту — удалить").color(Color32::from_rgb(240, 140, 120)),
        );
    } else if let Some(sel) = &st.selected {
        let name = icons
            .aim_info(sel)
            .map_or(sel.as_str(), |i| i.name.as_str());
        ui.label(
            egui::RichText::new(format!("Ставим: {name}")).color(Color32::from_rgb(140, 200, 255)),
        );
    }
    hint(
        ui,
        "ЛКМ — поставить (Shift — без привязки к клетке), R — повернуть, Esc — снять выбор, F5 — закрыть",
    );
}

fn tiles_head(ui: &mut egui::Ui, st: &mut MappingState, icons: &Icons) {
    ui.checkbox(&mut st.ceiling, "С потолком из той же плитки");
    let chosen = match &st.tile {
        Some(Some(id)) => icons
            .tiles()
            .iter()
            .find(|t| &t.id == id)
            .map(|t| tile_label(&t.name)),
        Some(None) => Some("пустая плитка (убрать пол и потолок)".to_string()),
        None => None,
    };
    if let Some(name) = chosen {
        ui.label(
            egui::RichText::new(format!("Кладём: {name}")).color(Color32::from_rgb(140, 200, 255)),
        );
    }
    hint(
        ui,
        "ЛКМ — клетка, протяжка — прямоугольник (до 32 × 32), Esc — снять выбор, F5 — закрыть",
    );
}

/// Сохранить, загрузить, новая карта: те же консольные команды `savemap`, `loadmap`, `mapping`;
/// права (флаг Mapping) и проверку имени делает сервер, ответ приходит в консоль.
fn maps_row(
    ui: &mut egui::Ui,
    st: &mut MappingState,
    can: &dyn Fn(&str) -> bool,
    out: &mut Commands,
) {
    if can("savemap") {
        ui.horizontal(|ui| {
            ui.label("Карта:");
            ui.add(
                egui::TextEdit::singleline(&mut st.save_name)
                    .hint_text("имя файла")
                    .desired_width(120.0),
            );
            let name = st.save_name.trim();
            let mut button = |ui: &mut egui::Ui, cmd: &str, label: &str, tip: &str| {
                if ui
                    .add_enabled(!name.is_empty(), egui::Button::new(label))
                    .on_hover_text(tip)
                    .clicked()
                {
                    out.push((
                        gameplay::admin::CMD_CONSOLE,
                        format!("{cmd} {name}").into_bytes(),
                    ));
                }
            };
            button(
                ui,
                "savemap",
                "Сохранить",
                "savemap <имя>: стены, полы, предметы на земле и зоны",
            );
            if can("loadmap") {
                button(
                    ui,
                    "loadmap",
                    "Загрузить",
                    "loadmap <имя>: новый раунд на этой карте",
                );
            }
        });
    }
    if can("mapping")
        && ui
            .button("Новая пустая карта")
            .on_hover_text("mapping: новый раунд на пустой карте (несохранённое пропадёт)")
            .clicked()
    {
        out.push((gameplay::admin::CMD_CONSOLE, b"mapping".to_vec()));
    }
}

fn entity_list(ctx: &egui::Context, ui: &mut egui::Ui, st: &mut MappingState, icons: &mut Icons) {
    let catalog = icons.catalog();
    for (id, name) in filter(&catalog, &st.search) {
        ui.horizontal(|ui| {
            if let Some(tex) = icons.icon(ctx, id) {
                ui.add(egui::Image::new((tex, egui::vec2(24.0, 24.0))));
            } else {
                ui.add_space(28.0);
            }
            let chosen = st.selected.as_deref() == Some(id.as_str());
            if ui
                .selectable_label(chosen, name)
                .on_hover_text(id)
                .clicked()
            {
                st.selected = Some(id.clone());
                st.delete = false;
            }
        });
    }
}

fn tile_list(ctx: &egui::Context, ui: &mut egui::Ui, st: &mut MappingState, icons: &mut Icons) {
    ui.horizontal(|ui| {
        ui.add_space(28.0);
        if ui
            .selectable_label(st.tile == Some(None), "Пустая плитка (убрать пол)")
            .clicked()
        {
            st.tile = Some(None);
        }
    });
    let q = st.search.trim().to_lowercase();
    let tiles: Vec<TileEntry> = icons
        .tiles()
        .iter()
        .filter(|t| q.is_empty() || t.name.to_lowercase().contains(&q))
        .cloned()
        .collect();
    for t in tiles {
        ui.horizontal(|ui| {
            if let Some(tex) = icons.load(ctx, &t.image) {
                ui.add(egui::Image::new((tex, egui::vec2(24.0, 24.0))));
            } else {
                ui.add_space(28.0);
            }
            let chosen = st.tile.as_ref().and_then(|t| t.as_deref()) == Some(t.id.as_str());
            if ui
                .selectable_label(chosen, tile_label(&t.name))
                .on_hover_text(&t.id)
                .clicked()
            {
                st.tile = Some(Some(t.id.clone()));
            }
        });
    }
}

/// Название плитки для списка: `FloorSteelMini` → `Steel Mini`.
pub fn tile_label(name: &str) -> String {
    let name = name
        .strip_prefix("Floor")
        .filter(|n| !n.is_empty())
        .unwrap_or(name);
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Прямоугольник клеток протяжки: от клетки нажатия до клетки отпускания.
pub fn cell_of(p: [f32; 3]) -> [i32; 2] {
    [p[0].floor() as i32, p[2].floor() as i32]
}

/// Точка установки: центр клетки или, с Shift, сама точка курсора.
pub fn snap(p: [f32; 3], free: bool) -> [f32; 3] {
    if free {
        p
    } else {
        [p[0].floor() + 0.5, p[1], p[2].floor() + 0.5]
    }
}

/// Щелчки и клавиши маппинга по миру.
#[allow(clippy::too_many_arguments)]
pub fn input(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    wants: Res<bevy_egui::input::EguiWantsInput>,
    ground: Res<crate::Ground>,
    mut hud: ResMut<crate::Hud>,
    net: Option<NonSendMut<astrocore_client::Net>>,
) {
    let Some(mut net) = net else { return };
    let st = &mut hud.state.mapping;
    if !st.open {
        return;
    }
    if !wants.wants_any_keyboard_input() {
        if keys.just_pressed(KeyCode::KeyR) {
            st.rot = (st.rot + 1) % 4;
        }
        if keys.just_pressed(KeyCode::Escape) {
            st.selected = None;
            st.delete = false;
            st.tile = None;
            st.drag_from = None;
        }
    }
    if st.tab == Tab::Tiles {
        // Протяжка: клетка нажатия — один угол, клетка отпускания — другой.
        let Some(tile) = st.tile.clone() else {
            st.drag_from = None;
            return;
        };
        if buttons.just_pressed(MouseButton::Left) && !wants.wants_any_pointer_input() {
            st.drag_from = ground.0.map(cell_of);
        }
        if buttons.just_released(MouseButton::Left)
            && let (Some(from), Some(point)) = (st.drag_from.take(), ground.0)
        {
            let r = TileRequest {
                tile,
                from,
                to: cell_of(point),
                level: st.level,
                ceiling: st.ceiling,
            };
            net.send_command(CMD_TILE, tile_data(&r));
        }
        return;
    }
    if !buttons.just_pressed(MouseButton::Left) || wants.wants_any_pointer_input() {
        return;
    }
    if st.delete {
        if let Some(id) = hud.state.aim_id {
            net.send_command(CMD_DELETE, delete_data(id));
        }
        return;
    }
    let (Some(proto), Some(point)) = (st.selected.clone(), ground.0) else {
        return;
    };
    let free = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let r = SpawnRequest {
        proto,
        at: snap(point, free),
        rot: st.rot,
    };
    net.send_command(CMD_SPAWN, spawn_data(&r));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_filters_by_name_or_id_and_points_snap_to_cells() {
        let c = vec![
            ("ss14:crowbar".to_string(), "crowbar".to_string()),
            (
                "ss14:vending_machine_cola".to_string(),
                "Space Cola".to_string(),
            ),
        ];
        assert_eq!(filter(&c, "").len(), 2);
        assert_eq!(filter(&c, "COLA")[0].0, "ss14:vending_machine_cola");
        assert_eq!(filter(&c, "crow").len(), 1);
        assert!(filter(&c, "nothing").is_empty());
        assert_eq!(snap([2.2, 0.3, -1.7], false), [2.5, 0.3, -1.5]);
        assert_eq!(snap([2.2, 0.3, -1.7], true), [2.2, 0.3, -1.7]);
        let admin = |flags: &[&str]| AdminView {
            rank: None,
            flags: flags.iter().map(|f| f.to_string()).collect(),
            commands: Vec::new(),
        };
        assert!(allowed(Some(&admin(&["Spawn"]))));
        assert!(!allowed(Some(&admin(&["Admin"]))));
        assert!(!allowed(None));
        assert_eq!(tile_label("FloorSteelMini"), "Steel Mini");
        assert_eq!(tile_label("Plating"), "Plating");
        assert_eq!(cell_of([-0.2, 0.3, 3.9]), [-1, 3]);
        // Каталог плиток — материалы манифеста с полем `tile`, по имени.
        let manifest = serde_json::json!({ "materials": {
            "ss14:wall_solid": { "image": "Textures/w.png" },
            "ss14:tile_floorsteel": { "image": "Textures/s.png", "tile": "FloorSteel" },
            "ss14:tile_floordark": { "image": "Textures/d.png", "tile": "FloorDark" },
        }});
        let tiles = crate::hud::tiles_of(&manifest);
        let ids: Vec<&str> = tiles.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["ss14:tile_floordark", "ss14:tile_floorsteel"]);
    }
}
