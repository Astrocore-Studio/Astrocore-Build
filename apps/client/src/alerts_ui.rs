//! AtmosAlertsComputerWindow / AtmosAlarmEntryContainer adapted to egui.
use crate::hud::Commands;
use astrocore_core::ObjectId;
use astrocore_world::{Placement, StructureKind, World};
use bevy_egui::egui::{self, Color32, Rect, Stroke, Vec2};
use gameplay::alerts::{self as alerts, Entry, Group, Level, View};
use std::collections::{BTreeMap, BTreeSet};

pub struct UiState {
    console: Option<ObjectId>,
    tab: usize,
    show: [bool; 4],
    zoom: f32,
    pan: Vec2,
    pending: BTreeMap<ObjectId, f64>,
    previous_focus: Option<ObjectId>,
    scroll: bool,
    geometry: Geometry,
}
impl Default for UiState {
    fn default() -> Self {
        Self {
            console: None,
            tab: 0,
            show: [false, false, true, true],
            zoom: 1.0,
            pan: Vec2::ZERO,
            pending: BTreeMap::new(),
            previous_focus: None,
            scroll: false,
            geometry: Geometry::default(),
        }
    }
}
#[derive(Default)]
struct Geometry {
    console: [f32; 2],
    revision: Option<(u64, i32)>,
    floor: BTreeSet<(i32, i32)>,
    blocked: BTreeSet<((i32, i32), (i32, i32))>,
    walls: Vec<([f32; 2], [f32; 2])>,
    regions: BTreeMap<ObjectId, Region>,
}
type Region = (Vec<[i32; 2]>, Vec<(i32, i32)>);
fn edge(a: (i32, i32), b: (i32, i32)) -> ((i32, i32), (i32, i32)) {
    if a < b { (a, b) } else { (b, a) }
}
impl Geometry {
    fn update(&mut self, world: &World, view: &View) {
        let floor = world.authored_object(view.console).map_or(0, |o| o.floor.0);
        let key = (world.revision().0, floor);
        if self.revision != Some(key) {
            *self = Self {
                revision: Some(key),
                ..Default::default()
            };
            if let Some(console) = world.authored_object(view.console) {
                let at = (console.min() + console.max()) * 0.5;
                self.console = [at.x, at.z];
            }
            for e in world.elements().filter(|e| e.floor.0 == floor) {
                match (e.kind, e.placement) {
                    (StructureKind::Floor, Placement::Cell(c)) => {
                        self.floor.insert((c.x, c.z));
                    }
                    (StructureKind::Wall | StructureKind::Opening(_), Placement::Edge(g)) => {
                        let (a, b) = g.endpoints(1.0, 0.0);
                        self.walls.push(([a.x, a.z], [b.x, b.z]));
                        let (a, b) = match g.dir {
                            astrocore_math::EdgeDir::AlongX => ((g.x, g.z - 1), (g.x, g.z)),
                            astrocore_math::EdgeDir::AlongZ => ((g.x - 1, g.z), (g.x, g.z)),
                        };
                        self.blocked.insert(edge(a, b));
                    }
                    _ => {}
                }
            }
        }
        self.regions
            .retain(|id, _| view.entries.iter().any(|e| e.id == *id));
        for entry in &view.entries {
            if self
                .regions
                .get(&entry.id)
                .is_some_and(|r| r.0 == entry.seeds)
            {
                continue;
            }
            let mut visited = BTreeSet::new();
            let mut overflow = false;
            // ClientNavMapSystem.Regions: 625 tiles, radius 25; airlocks block propagation.
            for &[sx, sz] in &entry.seeds {
                let mut stack = vec![(sx, sz)];
                while let Some(cell) = stack.pop() {
                    if visited.len() > 625 {
                        visited.clear();
                        overflow = true;
                        break;
                    }
                    if (cell.0 - sx).pow(2) + (cell.1 - sz).pow(2) > 25 * 25
                        || !self.floor.contains(&cell)
                        || !visited.insert(cell)
                    {
                        continue;
                    }
                    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let next = (cell.0 + dx, cell.1 + dz);
                        if !self.blocked.contains(&edge(cell, next)) {
                            stack.push(next);
                        }
                    }
                }
                if overflow {
                    break;
                }
            }
            self.regions.insert(
                entry.id,
                (entry.seeds.clone(), visited.into_iter().collect()),
            );
        }
    }
}
fn color(level: Level) -> Color32 {
    match level {
        Level::Invalid => Color32::from_gray(90),
        Level::Normal => Color32::from_rgb(50, 205, 50),
        Level::Warning => Color32::from_rgb(255, 182, 72),
        Level::Danger => Color32::from_rgb(255, 67, 67),
    }
}
fn label(level: Level) -> &'static str {
    match level {
        Level::Invalid => "Нет питания",
        Level::Normal => "Норма",
        Level::Warning => "Предупреждение",
        Level::Danger => "Опасность",
    }
}
fn select(out: &mut Commands, current: Option<ObjectId>, id: ObjectId) {
    out.push((
        alerts::CMD_FOCUS,
        alerts::focus_data((current != Some(id)).then_some(id)),
    ));
}
pub fn draw(
    ctx: &egui::Context,
    state: &mut UiState,
    view: Option<&View>,
    world: Option<&World>,
) -> Commands {
    let mut out = Vec::new();
    let Some(view) = view else {
        state.console = None;
        return out;
    };
    if state.console != Some(view.console) {
        *state = UiState {
            console: Some(view.console),
            ..Default::default()
        };
    }
    if state.previous_focus != view.focus {
        state.previous_focus = view.focus;
        state.scroll = true;
        if let Some(entry) = view.entries.iter().find(|e| Some(e.id) == view.focus)
            && (state.tab != 0 || entry.level <= Level::Normal || entry.silenced)
        {
            state.tab = if entry.group == Group::AirAlarm { 1 } else { 2 };
        }
    }
    let now = ctx.input(|i| i.time);
    let expired: Vec<_> = state
        .pending
        .iter()
        .filter(|(_, until)| **until <= now)
        .map(|(id, _)| *id)
        .collect();
    for id in expired {
        state.pending.remove(&id);
        if view.focus == Some(id) {
            out.push((alerts::CMD_FOCUS, alerts::focus_data(None)));
        }
    }
    if !state.pending.is_empty() {
        ctx.request_repaint();
    }
    let active = |entry: &&Entry| {
        entry.level > Level::Normal && (!entry.silenced || state.pending.contains_key(&entry.id))
    };
    let count = view.entries.iter().filter(active).count();
    let mut open = true;
    egui::Window::new("Атмосферные тревоги")
        .id(egui::Id::new("atmos_alerts"))
        .open(&mut open)
        .default_size([1120.0, 640.0])
        .show(ctx, |ui| {
            let height = (ctx.content_rect().height() - 130.0).clamp(320.0, 610.0);
            ui.horizontal(|ui| {
                let map_width = (ui.available_width() * 0.58).max(240.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(map_width, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.horizontal(|ui| {
                            if ui.small_button("−").clicked() {
                                state.zoom = (state.zoom / 1.25).max(0.5);
                            }
                            if ui.small_button("+").clicked() {
                                state.zoom = (state.zoom * 1.25).min(12.0);
                            }
                            if ui.small_button("Сбросить").clicked() {
                                state.zoom = 1.0;
                                state.pan = Vec2::ZERO;
                            }
                        });
                        if let Some(world) = world {
                            state.geometry.update(world, view);
                        }
                        nav_map(ui, state, view, &mut out);
                        ui.horizontal_wrapped(|ui| {
                            for level in
                                [Level::Invalid, Level::Normal, Level::Warning, Level::Danger]
                            {
                                ui.colored_label(color(level), label(level));
                            }
                        });
                    },
                );
                ui.vertical(|ui| {
                    ui.set_min_width(300.0);
                    ui.heading(&view.station);
                    ui.horizontal_wrapped(|ui| {
                        for (index, name) in [
                            format!("Тревоги ({count})"),
                            "Воздушные".into(),
                            "Пожарные".into(),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            if ui.selectable_label(state.tab == index, name).clicked() {
                                state.tab = index;
                                state.scroll = true;
                            }
                        }
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("atmos_alarm_entries")
                        .max_height(height - 100.0)
                        .show(ui, |ui| {
                            let mut shown = 0;
                            for entry in &view.entries {
                                let show = match state.tab {
                                    0 => {
                                        entry.level > Level::Normal
                                            && (!entry.silenced
                                                || state.pending.contains_key(&entry.id))
                                    }
                                    1 => entry.group == Group::AirAlarm,
                                    _ => entry.group == Group::FireAlarm,
                                };
                                if !show {
                                    continue;
                                }
                                shown += 1;
                                let row = ui.scope(|ui| {
                                    ui.colored_label(color(entry.level), label(entry.level));
                                    if ui
                                        .selectable_label(
                                            view.focus == Some(entry.id),
                                            format!("{} [{}]", entry.name, entry.address),
                                        )
                                        .clicked()
                                    {
                                        select(&mut out, view.focus, entry.id);
                                    }
                                    if view.focus == Some(entry.id)
                                        && entry.group == Group::AirAlarm
                                    {
                                        if let Some(reading) = &view.reading {
                                            egui::Grid::new(entry.id).num_columns(2).show(
                                                ui,
                                                |ui| {
                                                    for (name, value, level) in [
                                                        (
                                                            "Температура",
                                                            format!(
                                                                "{:.2} °C / {:.2} K",
                                                                reading.temperature.0 - 273.15,
                                                                reading.temperature.0
                                                            ),
                                                            reading.temperature.1,
                                                        ),
                                                        (
                                                            "Давление",
                                                            format!(
                                                                "{:.2} кПа",
                                                                reading.pressure.0
                                                            ),
                                                            reading.pressure.1,
                                                        ),
                                                    ] {
                                                        ui.label(name);
                                                        ui.colored_label(color(level), value);
                                                        ui.end_row();
                                                    }
                                                    for &(gas, _, fraction, level) in &reading.gases
                                                    {
                                                        let gas = [
                                                            "O₂",
                                                            "N₂",
                                                            "CO₂",
                                                            "Плазма",
                                                            "T₂",
                                                            "H₂O",
                                                            "NH₃",
                                                            "N₂O",
                                                            "Фрезон",
                                                        ]
                                                        .get(gas as usize)
                                                        .copied()
                                                        .unwrap_or("?");
                                                        ui.label(gas);
                                                        ui.colored_label(
                                                            color(level),
                                                            format!("{:.2}%", fraction * 100.0),
                                                        );
                                                        ui.end_row();
                                                    }
                                                },
                                            );
                                        } else {
                                            ui.label("Нет данных");
                                        }
                                    }
                                    let mut silenced = entry.silenced;
                                    if ui.checkbox(&mut silenced, "Приглушить тревогу").changed()
                                    {
                                        out.push((
                                            alerts::CMD_SILENCE,
                                            alerts::silence_data(entry.id, silenced),
                                        ));
                                        if silenced {
                                            state.pending.insert(entry.id, now + 2.5);
                                        } else {
                                            state.pending.remove(&entry.id);
                                        }
                                    }
                                    if state.pending.contains_key(&entry.id) {
                                        ui.label("Приглушение тревоги…");
                                    }
                                    ui.separator();
                                });
                                if state.scroll && view.focus == Some(entry.id) {
                                    row.response.scroll_to_me(Some(egui::Align::Center));
                                    state.scroll = false;
                                }
                            }
                            if shown == 0 && state.tab == 0 {
                                ui.colored_label(color(Level::Normal), "Активных тревог нет");
                            }
                        });
                    ui.label("Отображать на карте");
                    ui.horizontal_wrapped(|ui| {
                        for (index, level) in
                            [Level::Invalid, Level::Normal, Level::Warning, Level::Danger]
                                .into_iter()
                                .enumerate()
                        {
                            ui.checkbox(&mut state.show[index], label(level));
                        }
                    });
                });
            });
            ui.separator();
            ui.small("Обратитесь за помощью к атмосферному технику · v1.8");
        });
    if !open {
        out.push((alerts::CMD_CLOSE, Vec::new()));
    }
    out
}
fn nav_map(ui: &mut egui::Ui, state: &mut UiState, view: &View, out: &mut Commands) {
    let size = egui::vec2(
        ui.available_width(),
        (ui.available_height() - 40.0).max(240.0),
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_gray(12));
    if response.dragged() {
        state.pan += response.drag_delta();
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        state.zoom = (state.zoom * (scroll * 0.005).exp()).clamp(0.5, 12.0);
    }
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for &(x, z) in &state.geometry.floor {
        min = min.min(egui::vec2(x as f32, z as f32));
        max = max.max(egui::vec2(x as f32 + 1.0, z as f32 + 1.0));
    }
    if !min.is_finite() {
        return;
    }
    let extent = (max - min).max(Vec2::splat(1.0));
    let scale =
        ((rect.width() - 20.0) / extent.x).min((rect.height() - 20.0) / extent.y) * state.zoom;
    let center = (min + max) * 0.5;
    let screen = |at: Vec2| rect.center() + (at - center) * scale + state.pan;
    for &(x, z) in &state.geometry.floor {
        painter.rect_filled(
            Rect::from_min_max(
                screen(egui::vec2(x as f32, z as f32)),
                screen(egui::vec2(x as f32 + 1.0, z as f32 + 1.0)),
            ),
            0.0,
            Color32::from_gray(28),
        );
    }
    let mut entries: Vec<_> = view.entries.iter().collect();
    entries.sort_by_key(|e| {
        if view.focus == Some(e.id) {
            999
        } else {
            e.level as i32
        }
    });
    for entry in &entries {
        if let Some((_, cells)) = state.geometry.regions.get(&entry.id) {
            let shade = if view.focus.is_some() && view.focus != Some(entry.id) {
                154.0 / 255.0 * 105.0 / 255.0
            } else {
                154.0 / 255.0
            };
            for &(x, z) in cells {
                painter.rect_filled(
                    Rect::from_min_max(
                        screen(egui::vec2(x as f32, z as f32)),
                        screen(egui::vec2(x as f32 + 1.0, z as f32 + 1.0)),
                    ),
                    0.0,
                    color(entry.level).gamma_multiply(shade),
                );
            }
        }
    }
    for &(a, b) in &state.geometry.walls {
        painter.line_segment(
            [screen(a.into()), screen(b.into())],
            Stroke::new(1.0, Color32::from_gray(64)),
        );
    }
    painter.circle_filled(screen(state.geometry.console.into()), 4.0, Color32::CYAN);
    let mut closest = None;
    for entry in entries {
        if !state.show[entry.level as usize] && view.focus != Some(entry.id) {
            continue;
        }
        let at = screen(entry.at.into());
        let fill = color(entry.level);
        match entry.level {
            Level::Warning => {
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        at + egui::vec2(0.0, -5.0),
                        at + egui::vec2(5.0, 4.0),
                        at + egui::vec2(-5.0, 4.0),
                    ],
                    fill,
                    Stroke::NONE,
                ));
            }
            Level::Danger => {
                painter.rect_filled(Rect::from_center_size(at, egui::vec2(8.0, 8.0)), 0.0, fill);
            }
            _ => {
                painter.circle_filled(at, 4.0, fill);
            }
        }
        if view.focus == Some(entry.id) {
            painter.circle_stroke(at, 7.0, Stroke::new(1.0, Color32::WHITE));
        }
        if let Some(pointer) = response.hover_pos() {
            let distance = at.distance(pointer);
            if distance < 10.0 && closest.is_none_or(|(_, d)| distance < d) {
                closest = Some((entry, distance));
            }
        }
    }
    if let Some((entry, _)) = closest {
        response
            .clone()
            .on_hover_text(format!("{}: {}", entry.name, label(entry.level)));
        if response.clicked() {
            out.push((alerts::CMD_FOCUS, alerts::focus_data(Some(entry.id))));
            if state.tab != 0 || entry.level <= Level::Normal || entry.silenced {
                state.tab = if entry.group == Group::AirAlarm { 1 } else { 2 };
            }
            state.scroll = true;
        }
    }
}
