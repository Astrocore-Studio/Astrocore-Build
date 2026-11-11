//! Худ по образцу интерфейса SS14: руки и быстрые слоты внизу, окно одежды и сетка сумки.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy_egui::egui::{self, Color32, Pos2, Rect, Sense, Vec2};
use gameplay::{
    BagView, CMD_DROP, CMD_EQUIP, CMD_SELECT_HAND, CMD_STORE, CMD_TAKE, CMD_UNEQUIP, Hands,
    InventoryView, drop_data, equip_data, store_data, take_data, unequip_data,
};

const SLOT: f32 = 64.0;
const GAP: f32 = 4.0;
const TILE: f32 = 32.0;

/// Слоты по группам худа SS14: нижняя полоса слева и справа от рук, остальное — в окне одежды.
const LEFT_BAR: &[&str] = &["id", "belt", "back"];
const RIGHT_BAR: &[&str] = &["pocket1", "pocket2", "suitstorage"];
/// Окно одежды: слот и его клетка `uiWindowPos` из шаблона SS14.
const CLOTHES: &[(&str, i32, i32)] = &[
    ("shoes", 1, 0),
    ("jumpsuit", 0, 1),
    ("outerClothing", 1, 1),
    ("gloves", 2, 1),
    ("neck", 0, 2),
    ("mask", 1, 2),
    ("ears", 2, 2),
    ("eyes", 0, 3),
    ("head", 1, 3),
];

/// Картинка пустого слота (`slotTexture` шаблона SS14).
fn slot_texture(slot: &str) -> &'static str {
    match slot {
        "shoes" => "shoes",
        "jumpsuit" => "uniform",
        "outerClothing" => "suit",
        "gloves" => "gloves",
        "neck" => "neck",
        "mask" => "mask",
        "eyes" => "glasses",
        "ears" => "ears",
        "head" => "head",
        "pocket1" | "pocket2" => "pocket",
        "suitstorage" => "suit_storage",
        "id" => "id",
        "belt" => "belt",
        _ => "back",
    }
}

/// Загрузчик картинок худа и значков предметов с кэшем текстур egui.
pub struct Icons {
    root: PathBuf,
    items: HashMap<String, String>,
    cache: HashMap<String, Option<egui::TextureHandle>>,
}

impl Icons {
    /// `root` — каталог рядом с манифестом рендера (`assets/ss14`), `items` — идентификатор
    /// предмета → путь картинки относительно `root`.
    pub fn new(root: PathBuf, items: HashMap<String, String>) -> Self {
        Self {
            root,
            items,
            cache: HashMap::new(),
        }
    }

    /// Прочитать манифест рендера: значки предметов.
    pub fn from_manifest(path: &std::path::Path) -> Self {
        let root = path.parent().map(PathBuf::from).unwrap_or_default();
        let items = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| {
                v.get("sprites")?.as_object().map(|m| {
                    m.iter()
                        .filter_map(|(k, e)| {
                            Some((k.clone(), e.get("image")?.as_str()?.to_string()))
                        })
                        .collect()
                })
            })
            .unwrap_or_default();
        Self::new(root, items)
    }

    fn load(&mut self, ctx: &egui::Context, rel: &str) -> Option<egui::TextureId> {
        if !self.cache.contains_key(rel) {
            let tex = image::open(self.root.join(rel)).ok().map(|img| {
                let img = img.to_rgba8();
                let size = [img.width() as usize, img.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                ctx.load_texture(rel, color, egui::TextureOptions::NEAREST)
            });
            self.cache.insert(rel.to_string(), tex);
        }
        self.cache.get(rel)?.as_ref().map(|t| t.id())
    }

    fn ui(&mut self, ctx: &egui::Context, name: &str) -> Option<egui::TextureId> {
        self.load(ctx, &format!("Textures/Interface/{name}.png"))
    }

    fn item(&mut self, ctx: &egui::Context, proto: &str) -> Option<egui::TextureId> {
        let rel = self.items.get(proto)?.clone();
        self.load(ctx, &rel)
    }
}

/// Действия игрока в худе; собираются за кадр и отправляются серверу.
pub type Commands = Vec<(&'static str, Vec<u8>)>;

/// Состояние окон худа.
#[derive(Default)]
pub struct HudState {
    pub clothes_open: bool,
    /// Сумки, окна которых закрыты вручную.
    pub bags_closed: std::collections::HashSet<u128>,
}

fn paint_image(ui: &egui::Ui, tex: Option<egui::TextureId>, rect: Rect, tint: Color32) {
    if let Some(t) = tex {
        ui.painter().image(
            t,
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            tint,
        );
    }
}

/// Подсветка активной руки (`hand_active`) и наведения (`slot_highlight`) из спрайтов SS14.
fn slot_overlays(
    ctx: &egui::Context,
    ui: &egui::Ui,
    icons: &mut Icons,
    rect: Rect,
    active: bool,
    hover: bool,
) {
    if active {
        let t = icons.ui(ctx, "Inventory/hand_active");
        paint_image(ui, t, rect, Color32::WHITE);
    } else if hover {
        let t = icons.ui(ctx, "Default/slot_highlight");
        paint_image(ui, t, rect, Color32::WHITE);
    }
}

fn short(id: u128) -> String {
    format!("#{:04x}", id & 0xFFFF)
}

fn bag_id(b: &BagView) -> u128 {
    b.id.as_uuid().as_u128()
}

/// Нарисовать весь худ. Возвращает команды для сервера.
pub fn draw(
    ctx: &egui::Context,
    icons: &mut Icons,
    state: &mut HudState,
    hands: Option<&Hands>,
    view: Option<&InventoryView>,
    first_person: bool,
) -> Commands {
    let mut out: Commands = Vec::new();
    let Some(hands) = hands else {
        return out;
    };
    if first_person {
        viewmodel(ctx, icons, hands);
    }
    let toggle = icons.ui(ctx, "Default/Slots/toggle");
    egui::Area::new(egui::Id::new("hud_bar"))
        .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -8.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(GAP);
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(SLOT * 0.7), Sense::click());
                paint_image(ui, toggle, r, Color32::WHITE);
                if resp.on_hover_text("Одежда").clicked() {
                    state.clothes_open = !state.clothes_open;
                }
                for s in LEFT_BAR {
                    equip_slot(ctx, ui, icons, hands, view, s, &mut out);
                }
                hand_slots(ctx, ui, icons, hands, view, &mut out);
                for s in RIGHT_BAR {
                    equip_slot(ctx, ui, icons, hands, view, s, &mut out);
                }
            });
        });
    if state.clothes_open {
        egui::Area::new(egui::Id::new("hud_clothes"))
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::new(8.0, -8.0))
            .show(ctx, |ui| {
                let (w, h) = (3.0 * (SLOT + GAP), 4.0 * (SLOT + GAP));
                let (area, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
                for (name, cx, cy) in CLOTHES {
                    let min = area.min + Vec2::new(*cx as f32, *cy as f32) * (SLOT + GAP);
                    let rect = Rect::from_min_size(min, Vec2::splat(SLOT));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                    equip_slot_at(ctx, &mut child, rect, icons, hands, view, name, &mut out);
                }
            });
    }
    if let Some(v) = view {
        for (n, bag) in v.bags.iter().enumerate() {
            if state.bags_closed.contains(&bag_id(bag)) {
                continue;
            }
            bag_window(ctx, icons, state, hands, bag, n, &mut out);
        }
    }
    out
}

/// Вид от первого лица: прицел и предметы в руках крупно по нижним углам экрана.
fn viewmodel(ctx: &egui::Context, icons: &mut Icons, hands: &Hands) {
    let screen = ctx.content_rect();
    let c = screen.center();
    let p = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("crosshair"),
    ));
    let stroke = egui::Stroke::new(2.0, Color32::from_white_alpha(180));
    p.line_segment([c - Vec2::new(6.0, 0.0), c - Vec2::new(2.0, 0.0)], stroke);
    p.line_segment([c + Vec2::new(2.0, 0.0), c + Vec2::new(6.0, 0.0)], stroke);
    p.line_segment([c - Vec2::new(0.0, 6.0), c - Vec2::new(0.0, 2.0)], stroke);
    p.line_segment([c + Vec2::new(0.0, 2.0), c + Vec2::new(0.0, 6.0)], stroke);
    const BIG: f32 = 160.0;
    for (i, _, item) in hands.iter() {
        let Some(it) = item else { continue };
        let Some(tex) = icons.item(ctx, it.prototype.as_str()) else {
            continue;
        };
        let left = i == 0;
        let x = if left {
            screen.left() + 90.0
        } else {
            screen.right() - 90.0 - BIG
        };
        let rect = Rect::from_min_size(
            Pos2::new(x, screen.bottom() - 100.0 - BIG),
            Vec2::splat(BIG),
        );
        let tint = if i == hands.active() {
            Color32::WHITE
        } else {
            Color32::from_gray(150)
        };
        let layer = egui::LayerId::new(egui::Order::Background, egui::Id::new(("viewmodel", i)));
        ctx.layer_painter(layer).image(
            tex,
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            tint,
        );
    }
}

fn hand_slots(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    icons: &mut Icons,
    hands: &Hands,
    view: Option<&InventoryView>,
    out: &mut Commands,
) {
    for (i, name, item) in hands.iter() {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(SLOT), Sense::click());
        let active = i == hands.active();
        let letter = if name.starts_with('l') {
            "Default/Slots/hand_l"
        } else {
            "Default/Slots/hand_r"
        };
        match item {
            None => {
                let t = icons.ui(ctx, letter);
                paint_image(ui, t, rect, Color32::WHITE);
            }
            Some(it) => {
                let bg = icons.ui(ctx, "Default/SlotBackground");
                paint_image(ui, bg, rect, Color32::WHITE);
                let t = icons.item(ctx, it.prototype.as_str());
                paint_image(ui, t, rect.shrink(8.0), Color32::WHITE);
            }
        }
        slot_overlays(ctx, ui, icons, rect, active, resp.hovered());
        if resp.clicked() {
            out.push((CMD_SELECT_HAND, vec![i as u8]));
        }
        if let Some(it) = item {
            let resp =
                resp.on_hover_text(it.prototype.as_str().split_once(':').map_or("", |p| p.1));
            resp.context_menu(|ui| {
                if ui.button("Выбросить").clicked() {
                    out.push((CMD_DROP, drop_data(i as u8)));
                    ui.close();
                }
                if let Some(v) = view {
                    for (d, _) in v.inventory.iter() {
                        if ui.button(format!("Надеть: {}", d.name)).clicked() {
                            out.push((CMD_EQUIP, equip_data(i as u8, &d.name)));
                            ui.close();
                        }
                    }
                    for bag in &v.bags {
                        if ui
                            .button(format!("В сумку {}", short(bag_id(bag))))
                            .clicked()
                        {
                            out.push((CMD_STORE, store_data(i as u8, bag.id)));
                            ui.close();
                        }
                    }
                }
            });
        }
    }
}

fn equip_slot(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    icons: &mut Icons,
    hands: &Hands,
    view: Option<&InventoryView>,
    slot: &str,
    out: &mut Commands,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(SLOT), Sense::hover());
    equip_slot_at(ctx, ui, rect, icons, hands, view, slot, out);
}

/// Слот одежды: щелчок кладёт в него предмет из активной руки или снимает надетое в пустую руку.
#[allow(clippy::too_many_arguments)]
fn equip_slot_at(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    rect: Rect,
    icons: &mut Icons,
    hands: &Hands,
    view: Option<&InventoryView>,
    slot: &str,
    out: &mut Commands,
) {
    let resp = ui.interact(rect, ui.id().with(slot), Sense::click());
    let worn = view.and_then(|v| v.inventory.get(slot));
    match worn {
        Some(it) => {
            let bg = icons.ui(ctx, "Default/SlotBackground");
            paint_image(ui, bg, rect, Color32::WHITE);
            let t = icons.item(ctx, it.prototype.as_str());
            paint_image(ui, t, rect.shrink(8.0), Color32::WHITE);
        }
        None => {
            let t = icons.ui(ctx, &format!("Default/Slots/{}", slot_texture(slot)));
            paint_image(ui, t, rect, Color32::WHITE);
            let blocked = view.is_some_and(|v| {
                v.inventory
                    .iter()
                    .find(|(d, _)| d.name == slot)
                    .and_then(|(d, _)| d.depends_on.as_deref())
                    .is_some_and(|dep| v.inventory.get(dep).is_none())
            });
            if blocked {
                let b = icons.ui(ctx, "Default/blocked");
                paint_image(ui, b, rect, Color32::WHITE);
            }
        }
    }
    slot_overlays(ctx, ui, icons, rect, false, resp.hovered());
    if view.is_none() {
        return;
    }
    if resp.on_hover_text(slot).clicked() {
        let active = hands.active();
        match (worn, hands.get(active)) {
            (None, Some(_)) => out.push((CMD_EQUIP, equip_data(active as u8, slot))),
            (Some(_), None) => out.push((CMD_UNEQUIP, unequip_data(slot))),
            _ => {}
        }
    }
}

/// Окно сумки: сетка клеток SS14 (`tile_empty`, `tile_blocked`), предметы на своих клетках.
/// Рамка хранилища из частей `piece_*` SS14 (по 8 px, масштаб ×2).
fn draw_frame(ctx: &egui::Context, ui: &egui::Ui, icons: &mut Icons, outer: Rect, f: f32) {
    let (l, t, r, b) = (outer.left(), outer.top(), outer.right(), outer.bottom());
    let parts: [(&str, Rect); 8] = [
        (
            "topLeft",
            Rect::from_min_max(Pos2::new(l, t), Pos2::new(l + f, t + f)),
        ),
        (
            "top",
            Rect::from_min_max(Pos2::new(l + f, t), Pos2::new(r - f, t + f)),
        ),
        (
            "topRight",
            Rect::from_min_max(Pos2::new(r - f, t), Pos2::new(r, t + f)),
        ),
        (
            "left",
            Rect::from_min_max(Pos2::new(l, t + f), Pos2::new(l + f, b - f)),
        ),
        (
            "right",
            Rect::from_min_max(Pos2::new(r - f, t + f), Pos2::new(r, b - f)),
        ),
        (
            "bottomLeft",
            Rect::from_min_max(Pos2::new(l, b - f), Pos2::new(l + f, b)),
        ),
        (
            "bottom",
            Rect::from_min_max(Pos2::new(l + f, b - f), Pos2::new(r - f, b)),
        ),
        (
            "bottomRight",
            Rect::from_min_max(Pos2::new(r - f, b - f), Pos2::new(r, b)),
        ),
    ];
    let c = icons.ui(ctx, "Default/Storage/piece_center");
    paint_image(ui, c, outer.shrink(f), Color32::WHITE);
    for (name, rect) in parts {
        let tex = icons.ui(ctx, &format!("Default/Storage/piece_{name}"));
        paint_image(ui, tex, rect, Color32::WHITE);
    }
}

fn bag_window(
    ctx: &egui::Context,
    icons: &mut Icons,
    state: &mut HudState,
    hands: &Hands,
    bag: &BagView,
    n: usize,
    out: &mut Commands,
) {
    if bag.grid.is_empty() {
        return;
    }
    let max_x = bag.grid.iter().map(|c| c.0).max().unwrap_or(0);
    let max_y = bag.grid.iter().map(|c| c.1).max().unwrap_or(0);
    let mut open = true;
    egui::Window::new(format!("Сумка {}", short(bag_id(bag))))
        .id(egui::Id::new(("bag", bag_id(bag))))
        .default_pos(Pos2::new(900.0 - n as f32 * 20.0, 120.0 + n as f32 * 20.0))
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            const FRAME: f32 = 16.0;
            let size =
                Vec2::new((max_x + 1) as f32, (max_y + 1) as f32) * TILE + Vec2::splat(FRAME * 2.0);
            let (outer, _) = ui.allocate_exact_size(size, Sense::hover());
            draw_frame(ctx, ui, icons, outer, FRAME);
            let area = Rect::from_min_size(
                outer.min + Vec2::splat(FRAME),
                size - Vec2::splat(FRAME * 2.0),
            );
            let empty = icons.ui(ctx, "Default/Storage/tile_empty");
            let blocked = icons.ui(ctx, "Default/Storage/tile_blocked");
            for x in 0..=max_x {
                for y in 0..=max_y {
                    let r = Rect::from_min_size(
                        area.min + Vec2::new(x as f32, y as f32) * TILE,
                        Vec2::splat(TILE),
                    );
                    let tex = if bag.grid.contains(&(x, y)) {
                        empty
                    } else {
                        blocked
                    };
                    paint_image(ui, tex, r, Color32::WHITE);
                }
            }
            for st in &bag.items {
                let (Some(x0), Some(y0), Some(x1), Some(y1)) = (
                    st.cells().iter().map(|c| c.0).min(),
                    st.cells().iter().map(|c| c.1).min(),
                    st.cells().iter().map(|c| c.0).max(),
                    st.cells().iter().map(|c| c.1).max(),
                ) else {
                    continue;
                };
                let r = Rect::from_min_max(
                    area.min + Vec2::new(x0 as f32, y0 as f32) * TILE,
                    area.min + Vec2::new((x1 + 1) as f32, (y1 + 1) as f32) * TILE,
                );
                let resp = ui.interact(
                    r,
                    ui.id().with(st.item.id.as_uuid().as_u128()),
                    Sense::click(),
                );
                let t = icons.item(ctx, st.item.prototype.as_str());
                paint_image(ui, t, r.shrink(3.0), Color32::WHITE);
                if resp
                    .on_hover_text(
                        st.item
                            .prototype
                            .as_str()
                            .split_once(':')
                            .map_or("", |p| p.1),
                    )
                    .clicked()
                {
                    out.push((CMD_TAKE, take_data(bag.id, st.item.id)));
                }
            }
            if let Some(it) = hands.get(hands.active()) {
                let _ = it;
                if ui.button("Положить из активной руки").clicked() {
                    out.push((CMD_STORE, store_data(hands.active() as u8, bag.id)));
                }
            }
        });
    if !open {
        state.bags_closed.insert(bag_id(bag));
    }
}
