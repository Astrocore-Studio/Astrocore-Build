//! Окно маппинга (`EntitySpawnWindow` SS14), клавиша F5 у тех, у кого есть флаг `Spawn`:
//! каталог прототипов с поиском и значками; выбранный ставится щелчком по миру (по центру
//! клетки, с Shift — в точку курсора), R поворачивает на четверть, режим удаления убирает объект
//! под курсором. Пока окно открыто, камера смотрит сверху, а обычные щелчки (удар, постройка)
//! не работают. Права проверяет сервер (`systems/mapping.rs`), окно — только подсказка.

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, Key};
use gameplay::admin::AdminView;
use gameplay::systems::mapping::{
    CMD_DELETE, CMD_SPAWN, FLAG, SpawnRequest, delete_data, spawn_data,
};

use crate::hud::{Commands, Icons};

/// Сколько строк каталога показывать за раз (поиск сужает).
const LIST_LIMIT: usize = 300;

#[derive(Default)]
pub struct MappingState {
    pub open: bool,
    search: String,
    /// Выбранный прототип (идентификатор движка).
    pub selected: Option<String>,
    pub rot: u8,
    pub delete: bool,
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

/// Окно каталога. Возвращает команды для сервера (здесь их нет: ставит щелчок по миру).
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
    let catalog = icons.catalog();
    let mut open = true;
    egui::Window::new("Маппинг: сущности")
        .id(egui::Id::new("mapping"))
        .default_pos(egui::pos2(ctx.content_rect().max.x - 380.0, 60.0))
        .default_width(340.0)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Поиск:");
                ui.add(egui::TextEdit::singleline(&mut st.search).desired_width(f32::INFINITY));
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut st.delete, "Удаление");
                ui.label(format!("Поворот: {}°", u16::from(st.rot) * 90));
            });
            if st.delete {
                ui.label(
                    egui::RichText::new("ЛКМ по объекту — удалить")
                        .color(Color32::from_rgb(240, 140, 120)),
                );
            } else if let Some(sel) = &st.selected {
                let name = catalog
                    .iter()
                    .find(|(id, _)| id == sel)
                    .map_or(sel.as_str(), |(_, n)| n.as_str());
                ui.label(
                    egui::RichText::new(format!("Ставим: {name}"))
                        .color(Color32::from_rgb(140, 200, 255)),
                );
            }
            ui.label(
                egui::RichText::new(
                    "ЛКМ — поставить (Shift — без привязки к клетке), R — повернуть, Esc — снять выбор, F5 — закрыть",
                )
                .size(11.0)
                .color(Color32::from_gray(150)),
            );
            ui.separator();
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for (id, name) in filter(&catalog, &st.search) {
                    ui.horizontal(|ui| {
                        if let Some(tex) = icons.icon(ctx, id) {
                            ui.add(egui::Image::new((tex, egui::vec2(24.0, 24.0))));
                        } else {
                            ui.add_space(28.0);
                        }
                        let chosen = st.selected.as_deref() == Some(id.as_str());
                        if ui.selectable_label(chosen, name).on_hover_text(id).clicked() {
                            st.selected = Some(id.clone());
                            st.delete = false;
                        }
                    });
                }
            });
        });
    if !open {
        st.open = false;
    }
    Vec::new()
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
        }
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
    }
}
