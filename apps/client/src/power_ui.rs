//! Power monitor window, adapted from SS14 PowerMonitoringWindow and NavMapControl.
use astrocore_core::ObjectId;
use astrocore_world::{Placement, StructureKind, World};
use bevy_egui::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use gameplay::power_monitor::{self as power, CableChunk, CableView, Entry, Group, MapView, View};

use crate::hud::{Commands, Icons};

pub struct UiState {
    cables: [bool; 3],
    zoom: f32,
    pan: Vec2,
    console: Option<ObjectId>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            cables: [true; 3],
            zoom: 1.0,
            pan: Vec2::ZERO,
            console: None,
        }
    }
}

fn title(group: Group) -> &'static str {
    match group {
        Group::Generator => "Источники",
        Group::Smes => "СМЭС",
        Group::Substation => "Подстанции",
        Group::Apc => "АПЦ",
        Group::Consumer => "Потребители",
    }
}

fn color(group: Group) -> Color32 {
    match group {
        Group::Generator => Color32::from_rgb(128, 0, 128),
        Group::Smes => Color32::from_rgb(255, 69, 0),
        Group::Substation => Color32::YELLOW,
        Group::Apc => Color32::from_rgb(50, 205, 50),
        Group::Consumer => Color32::LIGHT_GRAY,
    }
}

fn name(map: &MapView, id: ObjectId) -> String {
    map.nodes
        .iter()
        .find(|n| n.0 == id)
        .and_then(|node| {
            let kind = map.kinds.get(node.2 as usize)?;
            Some(if node.4 > 1 {
                format!("{} array [{}]", kind.name, node.4)
            } else {
                kind.name.clone()
            })
        })
        .unwrap_or_default()
}

fn watts(value: f32) -> String {
    if value.abs() >= 1.0e6 {
        format!("{:.2} МВт", value / 1.0e6)
    } else if value.abs() >= 1.0e3 {
        format!("{:.2} кВт", value / 1.0e3)
    } else {
        format!("{value:.0} Вт")
    }
}

fn request(out: &mut Commands, group: Group, device: Option<ObjectId>) {
    out.push((power::CMD_FOCUS, power::focus_data(group, device)));
}

fn entry(
    ui: &mut egui::Ui,
    icons: &mut Icons,
    map: &MapView,
    data: &Entry,
    current: Option<ObjectId>,
    out: &mut Commands,
) {
    let node = map.nodes.iter().find(|n| n.0 == data.id);
    let kind = node.and_then(|n| map.kinds.get(n.2 as usize));
    ui.horizontal(|ui| {
        if let Some(kind) = kind
            && !kind.icon.is_empty()
            && let Some((texture, uv)) = icons.first_frame(ui.ctx(), &kind.icon)
        {
            ui.add(egui::Image::new((texture, egui::vec2(24.0, 24.0))).uv(uv));
        }
        let button = egui::Button::new(name(map, data.id)).selected(current == Some(data.id));
        if ui.add(button).clicked() {
            request(
                out,
                kind.map_or(Group::Generator, |k| k.group),
                (current != Some(data.id)).then_some(data.id),
            );
        }
        ui.label(format!("{:.0} Вт", data.power));
    });
    if let Some(level) = data.battery {
        let fill = Color32::from(egui::ecolor::Hsva::new(
            level.clamp(0.0, 1.0) * 0.33,
            1.0,
            1.0,
            1.0,
        ));
        ui.add(
            egui::ProgressBar::new(level)
                .text(format!("{:.0}%", level * 100.0))
                .fill(fill),
        );
    }
}

pub fn draw(
    ctx: &egui::Context,
    state: &mut UiState,
    icons: &mut Icons,
    view: Option<&View>,
    map: Option<&MapView>,
    cables: Option<&CableView>,
    world: Option<&World>,
) -> Commands {
    let mut out = Vec::new();
    let Some(view) = view else {
        state.console = None;
        return out;
    };
    let (Some(map), Some(cables)) = (
        map.filter(|m| m.console == view.console),
        cables.filter(|m| m.console == view.console),
    ) else {
        return out;
    };
    if state.console != Some(view.console) {
        *state = UiState {
            console: Some(view.console),
            ..Default::default()
        };
    }
    let mut open = true;
    egui::Window::new("Мониторинг питания")
        .id(egui::Id::new("power_monitor"))
        .open(&mut open)
        .default_size([1120.0, 640.0])
        .min_size([760.0, 540.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let map_width = (ui.available_width() * 0.58).max(220.0);
                let height = (ctx.content_rect().height() - 130.0).clamp(320.0, 610.0);
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
                        nav_map(ui, state, view, map, cables, world, &mut out);
                        ui.horizontal_wrapped(|ui| {
                            for group in
                                [Group::Generator, Group::Smes, Group::Substation, Group::Apc]
                            {
                                ui.colored_label(color(group), title(group));
                            }
                        });
                    },
                );
                ui.vertical(|ui| {
                    ui.set_min_width(300.0);
                    ui.heading(&map.station);
                    egui::Grid::new("power_totals").show(ui, |ui| {
                        ui.label("Выработка генераторов");
                        ui.label(watts(view.total_sources));
                        ui.end_row();
                        ui.label("Расход батарей");
                        let warning = view.total_battery_usage > view.total_sources * 0.1111;
                        ui.colored_label(
                            if warning {
                                Color32::RED
                            } else {
                                Color32::WHITE
                            },
                            watts(view.total_battery_usage),
                        );
                        ui.end_row();
                        ui.label("Нагрузка сетей");
                        let supply = view.total_sources + view.total_battery_usage;
                        let warning = supply < view.total_loads
                            && (supply - view.total_loads).abs() > view.total_loads.abs() * 0.1;
                        ui.colored_label(
                            if warning {
                                Color32::RED
                            } else {
                                Color32::WHITE
                            },
                            watts(view.total_loads),
                        );
                        ui.end_row();
                    });
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        for group in [Group::Generator, Group::Smes, Group::Substation, Group::Apc]
                        {
                            if ui
                                .selectable_label(view.group == group, title(group))
                                .clicked()
                            {
                                request(&mut out, group, view.focus);
                            }
                        }
                    });
                    egui::ScrollArea::vertical()
                        .id_salt("power_device_list")
                        .max_height(height - 220.0)
                        .show(ui, |ui| {
                            let mut entries: Vec<_> = view.entries.iter().collect();
                            entries
                                .sort_by(|a, b| b.power.total_cmp(&a.power).then(a.id.cmp(&b.id)));
                            for data in entries {
                                entry(ui, icons, map, data, view.focus, &mut out);
                                if view.focus == Some(data.id) {
                                    ui.indent(data.id.to_string(), |ui| {
                                        for (label, entries) in [
                                            ("← Источники", &view.sources),
                                            ("→ Нагрузки", &view.loads),
                                        ] {
                                            if !entries.is_empty() {
                                                ui.strong(label);
                                                for linked in entries {
                                                    entry(
                                                        ui, icons, map, linked, view.focus,
                                                        &mut out,
                                                    );
                                                }
                                            }
                                        }
                                    });
                                }
                                ui.separator();
                            }
                        });
                    ui.label("Показывать кабельные сети");
                    ui.horizontal_wrapped(|ui| {
                        for (i, label) in ["Высокое", "Среднее", "Низкое"].into_iter().enumerate()
                        {
                            ui.checkbox(&mut state.cables[i], label);
                        }
                    });
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.small("[user@nanotrasen] $run power_net_query");
                ui.small("v1.3");
            });
        });
    if !open {
        out.push((power::CMD_CLOSE, Vec::new()));
    }
    out
}

fn nav_map(
    ui: &mut egui::Ui,
    state: &mut UiState,
    view: &View,
    map: &MapView,
    cables: &CableView,
    world: Option<&World>,
    out: &mut Commands,
) {
    let size = egui::vec2(
        ui.available_width(),
        (ui.available_height() - 40.0).max(240.0),
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(12, 17, 23));
    if response.dragged() {
        state.pan += response.drag_delta();
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        state.zoom = (state.zoom * (scroll * 0.005).exp()).clamp(0.5, 12.0);
    }
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    let floor = world
        .and_then(|w| w.authored_object(view.console))
        .map(|o| o.floor);
    let mut floor_cells = Vec::new();
    let mut walls = Vec::new();
    if let Some(world) = world {
        let step = world.config().grid_step_m;
        for element in world.elements().filter(|e| Some(e.floor) == floor) {
            match (element.kind, element.placement) {
                (StructureKind::Floor, Placement::Cell(cell)) => {
                    let at = Vec2::new(cell.x as f32 * step, cell.z as f32 * step);
                    min = min.min(at);
                    max = max.max(at + Vec2::splat(step));
                    floor_cells.push((at, step));
                }
                (StructureKind::Wall | StructureKind::Opening(_), Placement::Edge(edge)) => {
                    let (a, b) = edge.endpoints(step, 0.0);
                    walls.push((Vec2::new(a.x, a.z), Vec2::new(b.x, b.z)));
                }
                _ => {}
            }
        }
    }
    for node in &map.nodes {
        let at = Vec2::from(node.1);
        min = min.min(at);
        max = max.max(at);
    }
    if !min.is_finite() || !max.is_finite() {
        return;
    }
    let extent = (max - min).max(Vec2::splat(1.0));
    let scale =
        ((rect.width() - 20.0) / extent.x).min((rect.height() - 20.0) / extent.y) * state.zoom;
    let center = (min + max) * 0.5;
    let to_screen = |at: Vec2| rect.center() + (at - center) * scale + state.pan;
    for (at, step) in floor_cells {
        painter.rect_filled(
            Rect::from_min_max(to_screen(at), to_screen(at + Vec2::splat(step))),
            0.0,
            Color32::from_rgb(30, 57, 67),
        );
    }
    for (a, b) in walls {
        painter.line_segment(
            [to_screen(a), to_screen(b)],
            Stroke::new(1.0, Color32::from_rgb(102, 164, 217)),
        );
    }
    draw_cables(
        &painter,
        &cables.all,
        state.cables,
        if cables.focus.is_empty() { 1.0 } else { 0.25 },
        scale,
        &to_screen,
    );
    draw_cables(
        &painter,
        &cables.focus,
        state.cables,
        1.0,
        scale,
        &to_screen,
    );
    let mut nearest = None;
    let pointer = response.hover_pos();
    for node in &map.nodes {
        let Some(kind) = map.kinds.get(node.2 as usize) else {
            continue;
        };
        let master = node.3.unwrap_or(node.0);
        let selected = view.focus == Some(master)
            || view
                .sources
                .iter()
                .chain(&view.loads)
                .any(|e| e.id == master);
        let color = if view.focus.is_none() || selected {
            color(kind.group)
        } else {
            color(kind.group).gamma_multiply(0.35)
        };
        let at = to_screen(Vec2::from(node.1));
        if !rect.contains(at) {
            continue;
        }
        match kind.group {
            Group::Apc => {
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        at + egui::vec2(0.0, -4.0),
                        at + egui::vec2(4.0, 4.0),
                        at + egui::vec2(-4.0, 4.0),
                    ],
                    color,
                    Stroke::NONE,
                ));
            }
            Group::Substation => {
                painter.rect_filled(Rect::from_center_size(at, egui::vec2(7.0, 7.0)), 0.0, color);
            }
            Group::Smes => {
                painter.add(egui::Shape::convex_polygon(
                    (0..6)
                        .map(|i| {
                            let angle = i as f32 * std::f32::consts::TAU / 6.0;
                            at + egui::vec2(angle.cos(), angle.sin()) * 4.0
                        })
                        .collect(),
                    color,
                    Stroke::NONE,
                ));
            }
            _ => {
                painter.circle_filled(at, 3.5, color);
            }
        }
        if selected {
            painter.circle_stroke(at, 6.0, Stroke::new(1.0, Color32::WHITE));
        }
        if let Some(pointer) = pointer {
            let distance = at.distance(pointer);
            if distance < 9.0 && nearest.is_none_or(|(_, _, best)| distance < best) {
                nearest = Some((master, kind.group, distance));
            }
        }
    }
    if let Some((id, group, _)) = nearest {
        response.clone().on_hover_text(name(map, id));
        if response.clicked() {
            request(out, group, (view.focus != Some(id)).then_some(id));
        }
    }
}

fn draw_cables(
    painter: &egui::Painter,
    chunks: &[CableChunk],
    enabled: [bool; 3],
    brightness: f32,
    scale: f32,
    to_screen: &impl Fn(Vec2) -> Pos2,
) {
    let colors = [
        Color32::from_rgb(255, 69, 0),
        Color32::YELLOW,
        Color32::from_rgb(50, 205, 50),
    ];
    for group in 0..3 {
        if !enabled[group] {
            continue;
        }
        let mut cells = std::collections::BTreeSet::new();
        for chunk in chunks {
            for bit in 0..25 {
                if chunk[2 + group] & (1 << bit) != 0 {
                    cells.insert((chunk[0] + bit / 5, chunk[1] + bit % 5));
                }
            }
        }
        let offset = (group as f32 - 1.0) * 0.2;
        let position =
            |x: i32, z: i32| to_screen(Vec2::new(x as f32 + 0.5 + offset, z as f32 + 0.5 + offset));
        let color = colors[group].gamma_multiply(brightness);
        for &(x, z) in &cells {
            for (dx, dz) in [(1, 0), (0, 1)] {
                if cells.contains(&(x + dx, z + dz)) {
                    painter.line_segment(
                        [position(x, z), position(x + dx, z + dz)],
                        Stroke::new((scale * 0.08).clamp(0.8, 2.0), color),
                    );
                }
            }
            if ![(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|(dx, dz)| cells.contains(&(x + dx, z + dz)))
            {
                painter.circle_filled(position(x, z), 1.2, color);
            }
        }
    }
}
