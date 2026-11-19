//! Окно маппинга (`EntitySpawnWindow` SS14), клавиша F5 у тех, у кого есть флаг `Spawn`:
//! каталог прототипов с поиском и значками; выбранный ставится щелчком по миру (по центру
//! клетки, с Shift — в точку курсора), R поворачивает на четверть, режим удаления убирает объект
//! под курсором. Пока окно открыто, камера смотрит сверху, а обычные щелчки (удар, постройка)
//! не работают. Права проверяет сервер (`systems/mapping.rs`), окно — только подсказка.

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, Key};
use gameplay::admin::AdminView;
use gameplay::systems::mapping::{
    CMD_ERASE, CMD_MARKER, CMD_SPAWN, CMD_STRUCTURE, CMD_TILE, EraseRequest, FLAG, LATEJOIN,
    MarkerRequest, SpawnRequest, StructureEntry, StructureRequest, TILE_FLAG, TileRequest,
    erase_data, line_cells, marker_data, spawn_data, structure_catalog, structure_data, tile_data,
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
    Structures,
    Markers,
}

pub struct MappingState {
    pub open: bool,
    pub tab: Tab,
    search: String,
    /// Выбранный прототип (идентификатор движка).
    pub selected: Option<String>,
    pub rot: u8,
    /// Ластик: рамка протяжкой убирает всё, кроме пола (`mapping.erase`).
    pub eraser: bool,
    /// Показывать потолки в режиме маппинга (по умолчанию скрыты, чтобы видеть пол сверху).
    pub ceilings: bool,
    /// Ортографическая камера сверху (без перспективы, как в SS14); иначе — перспектива.
    pub ortho: bool,
    /// Номер карты и имя файла для команд карт (`savemap <карта> <имя>` и др.).
    map_id: String,
    save_name: String,
    /// Выбранная плитка: `Some(None)` — пустая (убрать пол).
    pub tile: Option<Option<String>>,
    /// Класть и потолок из той же плитки.
    pub ceiling: bool,
    /// Уровень, на котором работает маппинг.
    pub level: i32,
    /// Клетка, где нажата кнопка мыши (протяжка прямоугольником).
    pub drag_from: Option<[i32; 2]>,
    /// Выбранная конструкция (прототип SS14).
    pub structure: Option<String>,
    /// Выбранный маркер: должность точки появления или `Latejoin`.
    pub marker: Option<String>,
    /// Каталог конструкций (стены, окна, решётки, шлюзы), по имени.
    structures: Vec<StructureEntry>,
}

impl Default for MappingState {
    fn default() -> Self {
        Self {
            open: false,
            tab: Tab::Entities,
            search: String::new(),
            selected: None,
            rot: 0,
            eraser: false,
            ceilings: false,
            ortho: true,
            map_id: "1".into(),
            save_name: String::new(),
            tile: None,
            ceiling: true,
            level: 0,
            drag_from: None,
            structure: None,
            marker: None,
            structures: structure_catalog(),
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
    jobs: &[(String, String)],
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
                    ui.selectable_value(&mut st.tab, Tab::Structures, "Стены и двери");
                    ui.selectable_value(&mut st.tab, Tab::Markers, "Маркеры");
                }
            });
            // Общие для всех вкладок: ластик и вид.
            ui.horizontal(|ui| {
                ui.checkbox(&mut st.eraser, "Ластик")
                    .on_hover_text("Рамка протяжкой убирает сущности, стены, шлюзы; пол остаётся");
                ui.checkbox(&mut st.ceilings, "Потолки")
                    .on_hover_text("Показывать потолки (сверху они закрывают пол)");
                ui.checkbox(&mut st.ortho, "Без перспективы")
                    .on_hover_text("Ортографический вид сверху, как в SS14");
            });
            ui.label(format!("Уровень: {} (PgUp / PgDn)", st.level));
            if st.eraser {
                ui.label(
                    egui::RichText::new("Ластик: ЛКМ — клетка, протяжка — рамка (до 32 × 32)")
                        .color(Color32::from_rgb(240, 140, 120)),
                );
            }
            ui.horizontal(|ui| {
                ui.label("Поиск:");
                ui.add(egui::TextEdit::singleline(&mut st.search).desired_width(f32::INFINITY));
            });
            match st.tab {
                Tab::Entities => entities_head(ui, st, icons),
                Tab::Tiles => tiles_head(ui, st, icons),
                Tab::Structures => structures_head(ui, st),
                Tab::Markers => hint(
                    ui,
                    "Точки появления: ЛКМ по клетке. В игре их не видно, в маппинге — кружки. Убрать — ластиком",
                ),
            }
            maps_row(ui, st, &can, &mut out);
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| match st.tab {
                    Tab::Entities => entity_list(ctx, ui, st, icons),
                    Tab::Tiles => tile_list(ctx, ui, st, icons),
                    Tab::Structures => structure_list(ctx, ui, st, icons),
                    Tab::Markers => marker_list(ui, st, jobs),
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
    ui.label(format!("Поворот: {}°", u16::from(st.rot) * 90));
    if let Some(sel) = &st.selected {
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

/// Карты: номер и имя файла; кнопки — те же консольные команды SS14 (`mapping <карта> [имя]`,
/// `loadmap <карта> <имя>`, `savemap <карта> <имя>`). Права (флаг Mapping), проверку имени и
/// номера делает сервер, ответ приходит в консоль.
fn maps_row(
    ui: &mut egui::Ui,
    st: &mut MappingState,
    can: &dyn Fn(&str) -> bool,
    out: &mut Commands,
) {
    if !can("savemap") {
        return;
    }
    ui.horizontal(|ui| {
        ui.label("Карта №");
        ui.add(egui::TextEdit::singleline(&mut st.map_id).desired_width(32.0));
        ui.add(
            egui::TextEdit::singleline(&mut st.save_name)
                .hint_text("имя файла")
                .desired_width(110.0),
        );
    });
    let (id, name) = (
        st.map_id.trim().to_string(),
        st.save_name.trim().to_string(),
    );
    ui.horizontal(|ui| {
        let mut send = |ui: &mut egui::Ui, enabled: bool, label: &str, tip: &str, line: String| {
            if ui
                .add_enabled(enabled, egui::Button::new(label))
                .on_hover_text(tip)
                .clicked()
            {
                out.push((gameplay::admin::CMD_CONSOLE, line.into_bytes()));
            }
        };
        let both = !id.is_empty() && !name.is_empty();
        send(ui, both, "Сохранить", "savemap <карта> <имя>: инициализированную — только с force", format!("savemap {id} {name}"));
        if can("loadmap") {
            send(ui, both, "Загрузить", "loadmap <карта> <имя>: карта идёт, как в игре", format!("loadmap {id} {name}"));
        }
        if can("mapping") {
            let line = if name.is_empty() { format!("mapping {id}") } else { format!("mapping {id} {name}") };
            send(ui, !id.is_empty(), "Маппинг", "mapping <карта> [имя]: новая карта на паузе (пустая или из файла), вы на ней призраком", line);
        }
    });
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
                st.eraser = false;
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

fn structures_head(ui: &mut egui::Ui, st: &mut MappingState) {
    ui.label(format!("Поворот шлюза: {}°", u16::from(st.rot) * 90));
    if let Some(sel) = &st.structure {
        ui.label(
            egui::RichText::new(format!("Строим: {}", camel_words(sel)))
                .color(Color32::from_rgb(140, 200, 255)),
        );
    }
    hint(
        ui,
        "Стены, окна, решётки: протяжка — линия (до 64 клеток). Шлюз: ЛКМ по клетке, R — поворот проёма. Esc — снять выбор",
    );
}

fn structure_list(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    st: &mut MappingState,
    icons: &mut Icons,
) {
    let q = st.search.trim().to_lowercase();
    let list: Vec<StructureEntry> = st
        .structures
        .iter()
        .filter(|e| q.is_empty() || e.proto.to_lowercase().contains(&q))
        .cloned()
        .collect();
    for e in list {
        ui.horizontal(|ui| {
            match icons.material_icon(ctx, &e.material) {
                Some((tex, uv)) => {
                    ui.add(egui::Image::new((tex, egui::vec2(24.0, 24.0))).uv(uv));
                }
                None => {
                    ui.add_space(28.0);
                }
            }
            let chosen = st.structure.as_deref() == Some(e.proto.as_str());
            let label = if e.airlock {
                format!("{} (шлюз)", camel_words(&e.proto))
            } else {
                camel_words(&e.proto)
            };
            if ui
                .selectable_label(chosen, label)
                .on_hover_text(&e.proto)
                .clicked()
            {
                st.structure = Some(e.proto.clone());
            }
        });
    }
}

fn marker_list(ui: &mut egui::Ui, st: &mut MappingState, jobs: &[(String, String)]) {
    let q = st.search.trim().to_lowercase();
    let all = std::iter::once((LATEJOIN.to_string(), "Поздний вход (Latejoin)".to_string()))
        .chain(jobs.iter().cloned());
    for (id, title) in all.filter(|(id, t)| {
        q.is_empty() || id.to_lowercase().contains(&q) || t.to_lowercase().contains(&q)
    }) {
        let chosen = st.marker.as_deref() == Some(id.as_str());
        if ui
            .selectable_label(chosen, format!("Появление: {title}"))
            .on_hover_text(&id)
            .clicked()
        {
            st.marker = Some(id);
        }
    }
}

/// `AirlockEngineeringLocked` → `Airlock Engineering Locked`.
pub fn camel_words(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
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
        // Уровни: выше и ниже (ADR-0003).
        if keys.just_pressed(KeyCode::PageUp) {
            st.level = (st.level + 1).min(49);
        }
        if keys.just_pressed(KeyCode::PageDown) {
            st.level = (st.level - 1).max(-50);
        }
        if keys.just_pressed(KeyCode::Escape) {
            st.selected = None;
            st.eraser = false;
            st.tile = None;
            st.structure = None;
            st.marker = None;
            st.drag_from = None;
        }
    }
    if st.eraser {
        if buttons.just_pressed(MouseButton::Left) && !wants.wants_any_pointer_input() {
            st.drag_from = ground.0.map(cell_of);
        }
        if buttons.just_released(MouseButton::Left)
            && let (Some(from), Some(point)) = (st.drag_from.take(), ground.0)
        {
            let r = EraseRequest {
                from,
                to: cell_of(point),
                level: st.level,
            };
            net.send_command(CMD_ERASE, erase_data(&r));
        }
        return;
    }
    if st.tab == Tab::Markers {
        if let (Some(job), Some(point)) = (st.marker.clone(), ground.0)
            && buttons.just_pressed(MouseButton::Left)
            && !wants.wants_any_pointer_input()
        {
            let r = MarkerRequest {
                job,
                at: cell_of(point),
                level: st.level,
            };
            net.send_command(CMD_MARKER, marker_data(&r));
        }
        return;
    }
    if st.tab == Tab::Structures {
        // Протяжка: клетка нажатия — начало линии, клетка отпускания — её направление и длина.
        let Some(proto) = st.structure.clone() else {
            st.drag_from = None;
            return;
        };
        if buttons.just_pressed(MouseButton::Left) && !wants.wants_any_pointer_input() {
            st.drag_from = ground.0.map(cell_of);
        }
        if buttons.just_released(MouseButton::Left)
            && let (Some(from), Some(point)) = (st.drag_from.take(), ground.0)
        {
            let r = StructureRequest {
                proto,
                from,
                to: cell_of(point),
                rot: st.rot,
                level: st.level,
            };
            net.send_command(CMD_STRUCTURE, structure_data(&r));
        }
        return;
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

/// Цвет сетки клеток и подсветки цели.
const GRID_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.18);
const HOVER_COLOR: Color = Color::srgb(1.0, 0.85, 0.2);
const ERASE_COLOR: Color = Color::srgb(1.0, 0.35, 0.3);
/// Радиус сетки вокруг клетки под курсором, клеток.
const GRID_RADIUS: i32 = 12;

/// Вид маппинга: потолки по флажку (фильтр отрисовки движка), сетка клеток вокруг курсора,
/// подсветка клетки под курсором и предпросмотр протяжки (рамка плиток и ластика, линия стен).
pub fn view(
    mut gizmos: Gizmos,
    ground: Res<crate::Ground>,
    buttons: Res<ButtonInput<MouseButton>>,
    hud: Res<crate::Hud>,
    rendered: Option<ResMut<astrocore_render::RenderedWorld>>,
    net: Option<NonSend<astrocore_client::Net>>,
    mut shown: Local<Option<(bool, i32)>>,
) {
    let st = &hud.state.mapping;
    // Пока открыт маппинг: уровни выше выбранного скрыты, потолки — по флажку.
    let want = st.open.then_some((st.ceilings, st.level));
    if let Some(mut r) = rendered
        && (want != *shown || (want.is_some() && r.filter.is_none()))
    {
        r.filter = want.map(|(ceilings, level)| {
            std::sync::Arc::new(move |e: &astrocore_world::StructureElement| {
                e.floor.0 <= level
                    && (ceilings || !matches!(e.kind, astrocore_world::StructureKind::Ceiling))
            }) as astrocore_render::ElementFilter
        });
        r.rebuild_all = true;
        *shown = want;
    }
    // Маркеры точек появления выбранного уровня: кружок с крестом (поздний вход — голубой).
    if st.open
        && let Some(world) = net.as_ref().and_then(|n| n.core.world())
    {
        for o in world.authored() {
            let Some(job) = o.metadata.get("spawn_job") else {
                continue;
            };
            if o.floor.0 != st.level {
                continue;
            }
            let c = (o.min() + o.max()) * 0.5;
            let at = Vec3::new(c.x, o.min().y + 0.03, c.z);
            let color = if job == LATEJOIN {
                Color::srgb(0.3, 0.8, 1.0)
            } else {
                Color::srgb(0.3, 1.0, 0.4)
            };
            gizmos.circle(
                Isometry3d::new(at, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                0.35,
                color,
            );
            gizmos.line(at - Vec3::X * 0.25, at + Vec3::X * 0.25, color);
            gizmos.line(at - Vec3::Z * 0.25, at + Vec3::Z * 0.25, color);
        }
    }
    if !st.open {
        return;
    }
    let Some([px, py, pz]) = ground.0 else {
        return;
    };
    let y = py + 0.01;
    let here = cell_of([px, py, pz]);
    // Сетка.
    let (x0, x1) = (here[0] - GRID_RADIUS, here[0] + GRID_RADIUS + 1);
    let (z0, z1) = (here[1] - GRID_RADIUS, here[1] + GRID_RADIUS + 1);
    for x in x0..=x1 {
        gizmos.line(
            Vec3::new(x as f32, y, z0 as f32),
            Vec3::new(x as f32, y, z1 as f32),
            GRID_COLOR,
        );
    }
    for z in z0..=z1 {
        gizmos.line(
            Vec3::new(x0 as f32, y, z as f32),
            Vec3::new(x1 as f32, y, z as f32),
            GRID_COLOR,
        );
    }
    let rect = |gizmos: &mut Gizmos, a: [i32; 2], b: [i32; 2], color: Color| {
        let (ax, bx) = (a[0].min(b[0]) as f32, a[0].max(b[0]) as f32 + 1.0);
        let (az, bz) = (a[1].min(b[1]) as f32, a[1].max(b[1]) as f32 + 1.0);
        let y = y + 0.01;
        let c = [
            Vec3::new(ax, y, az),
            Vec3::new(bx, y, az),
            Vec3::new(bx, y, bz),
            Vec3::new(ax, y, bz),
        ];
        for i in 0..4 {
            gizmos.line(c[i], c[(i + 1) % 4], color);
        }
    };
    let color = if st.eraser { ERASE_COLOR } else { HOVER_COLOR };
    match st.drag_from.filter(|_| buttons.pressed(MouseButton::Left)) {
        // Протяжка: линия стен — клетками, остальное — рамкой.
        Some(from) if !st.eraser && st.tab == Tab::Structures => {
            for c in line_cells(from, here) {
                rect(&mut gizmos, c, c, color);
            }
        }
        Some(from) => rect(&mut gizmos, from, here, color),
        None => rect(&mut gizmos, here, here, color),
    }
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
        assert_eq!(camel_words("AirlockEngineering"), "Airlock Engineering");
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
