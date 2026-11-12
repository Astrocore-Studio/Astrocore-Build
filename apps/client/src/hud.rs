//! Худ по образцу интерфейса SS14: руки и быстрые слоты внизу, окно одежды и сетка сумки.

use std::collections::HashMap;
use std::path::PathBuf;

use astrocore_core::ObjectId;
use bevy_egui::egui::{self, Color32, Pos2, Rect, Sense, Vec2};
use gameplay::{
    BagView, CMD_DROP, CMD_EQUIP, CMD_MOVE_HAND, CMD_MOVE_SLOT, CMD_SELECT_HAND, CMD_STORE,
    CMD_TAKE, CMD_TAKE_TO, CMD_UNEQUIP, CMD_UNEQUIP_TO, Hands, InventoryView, drop_at_data,
    drop_data, equip_data, move_hand_data, move_slot_data, store_data, take_data, take_to_data,
    unequip_data, unequip_to_data,
};

/// Насколько выше ступней лежит плоскость, куда падает предмет под курсором, м (верх пола).
pub const GROUND_LIFT_M: f32 = 0.0;

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
    /// Задержки кадров анимированных состояний (из `meta.json` набора RSI), с.
    delays: HashMap<String, Vec<f32>>,
    /// Размер текстуры в пикселях, нужен для раскладки кадров.
    sizes: HashMap<String, (u32, u32)>,
}

impl Icons {
    /// `root` — каталог рядом с манифестом рендера (`assets/ss14`), `items` — идентификатор
    /// предмета → путь картинки относительно `root`.
    pub fn new(root: PathBuf, items: HashMap<String, String>) -> Self {
        Self {
            root,
            items,
            cache: HashMap::new(),
            delays: HashMap::new(),
            sizes: HashMap::new(),
        }
    }

    /// Каталог с текстурами (`assets/ss14`).
    pub(crate) fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Значки предметов из манифеста: идентификатор → путь картинки относительно его каталога.
    pub fn icon_paths(manifest: Option<&std::path::Path>) -> HashMap<String, String> {
        manifest.map_or_else(HashMap::new, |p| Self::from_manifest(p).items)
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

    /// Кадр анимированного состояния RSI в момент `time`: текстура и область в долях (кадры лежат
    /// рядами слева направо, сверху вниз; размер кадра — 32 px, как в алертах SS14).
    fn animated(
        &mut self,
        ctx: &egui::Context,
        rsi_state: &str,
        time: f64,
    ) -> Option<(egui::TextureId, Rect)> {
        self.animated_in(ctx, "Interface", rsi_state, time)
    }

    /// То же для набора из любого каталога `Textures/<base>/` (огонь в мире лежит в `Effects`).
    pub(crate) fn animated_in(
        &mut self,
        ctx: &egui::Context,
        base: &str,
        rsi_state: &str,
        time: f64,
    ) -> Option<(egui::TextureId, Rect)> {
        let rel = format!("Textures/{base}/{rsi_state}.png");
        let tex = self.load(ctx, &rel)?;
        if !self.sizes.contains_key(&rel) {
            let size = image::image_dimensions(self.root.join(&rel)).ok()?;
            self.sizes.insert(rel.clone(), size);
            let (dir, stem) = rsi_state.rsplit_once('/')?;
            let meta = std::fs::read(self.root.join(format!("Textures/{base}/{dir}/meta.json")))
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
            let delays = meta
                .and_then(|m| {
                    m.get("states")?
                        .as_array()?
                        .iter()
                        .find(|s| s.get("name").and_then(|n| n.as_str()) == Some(stem))?
                        .get("delays")?
                        .as_array()?
                        .first()?
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_f64().map(|f| f as f32))
                                .collect()
                        })
                })
                .unwrap_or_default();
            self.delays.insert(rel.clone(), delays);
        }
        let (w, h) = *self.sizes.get(&rel)?;
        let delays = self.delays.get(&rel)?;
        let (cols, rows) = ((w / 32).max(1), (h / 32).max(1));
        let frame = if delays.is_empty() {
            0
        } else {
            let total: f32 = delays.iter().sum();
            let mut t = (time as f32) % total.max(0.001);
            let mut f = 0;
            for (i, d) in delays.iter().enumerate() {
                if t < *d {
                    f = i;
                    break;
                }
                t -= d;
                f = i;
            }
            f as u32
        };
        let (cx, cy) = (frame % cols, frame / cols);
        let uv = Rect::from_min_max(
            Pos2::new(cx as f32 / cols as f32, cy as f32 / rows as f32),
            Pos2::new((cx + 1) as f32 / cols as f32, (cy + 1) as f32 / rows as f32),
        );
        Some((tex, uv))
    }

    fn ui(&mut self, ctx: &egui::Context, name: &str) -> Option<egui::TextureId> {
        self.load(ctx, &format!("Textures/Interface/{name}.png"))
    }

    fn item(&mut self, ctx: &egui::Context, proto: &str) -> Option<egui::TextureId> {
        let rel = self.items.get(proto)?.clone();
        self.load(ctx, &rel)
    }
}

/// Идущее действие с таймером: подпись и доля выполненного (0…1).
pub struct Progress<'a> {
    pub label: &'a str,
    pub fraction: f32,
}

/// Откуда тянут предмет.
#[derive(Clone, Debug)]
enum Src {
    Hand(u8),
    Slot(String),
    Bag { bag: ObjectId, item: ObjectId },
}

/// Содержимое перетаскивания: источник и прототип (для значка у курсора).
#[derive(Clone, Debug)]
struct Drag {
    src: Src,
    proto: String,
}

/// Действие, когда предмет отпустили на цель.
fn drop_on_hand(d: &Drag, to: u8, out: &mut Commands) {
    match &d.src {
        Src::Hand(from) if *from != to => out.push((CMD_MOVE_HAND, move_hand_data(*from, to))),
        Src::Slot(slot) => out.push((CMD_UNEQUIP_TO, unequip_to_data(to, slot))),
        Src::Bag { bag, item } => out.push((CMD_TAKE_TO, take_to_data(to, *bag, *item))),
        Src::Hand(_) => {}
    }
}

/// Действие, когда предмет отпустили в пустое место экрана (на пол).
fn drop_to_world(d: &Drag, ground: Option<[f32; 3]>, out: &mut Commands) {
    match &d.src {
        Src::Hand(h) => match ground {
            Some(point) => out.push((CMD_DROP, drop_at_data(*h, point))),
            None => out.push((CMD_DROP, drop_data(*h))),
        },
        Src::Slot(slot) => out.push((CMD_UNEQUIP, unequip_data(slot))),
        Src::Bag { bag, item } => out.push((CMD_TAKE, take_data(*bag, *item))),
    }
}

/// Действия игрока в худе; собираются за кадр и отправляются серверу.
pub type Commands = Vec<(&'static str, Vec<u8>)>;

/// Состояние окон худа.
#[derive(Default)]
pub struct HudState {
    /// Здоровье игрока для значка состояния в панели алертов.
    pub health: Option<gameplay::HealthView>,
    /// Состояние тела: сытость, давление, кислород.
    pub vitals: Option<gameplay::Vitals>,
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
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ctx: &egui::Context,
    icons: &mut Icons,
    state: &mut HudState,
    hands: Option<&Hands>,
    view: Option<&InventoryView>,
    first_person: bool,
    ground: Option<[f32; 3]>,
    progress: Option<Progress>,
) -> Commands {
    let mut out: Commands = Vec::new();
    let Some(hands) = hands else {
        return out;
    };
    if first_person {
        viewmodel(ctx, icons, hands);
    }
    if let Some(p) = progress {
        egui::Area::new(egui::Id::new("hud_progress"))
            .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -92.0))
            .show(ctx, |ui| {
                ui.add(
                    egui::ProgressBar::new(p.fraction.clamp(0.0, 1.0))
                        .desired_width(220.0)
                        .text(p.label),
                );
            });
    }
    let toggle = icons.ui(ctx, "Default/Slots/toggle");
    // Рюкзак-переключатель в левом нижнем углу экрана (`InventoryGui` SS14): показывает и прячет
    // окно одежды над собой.
    egui::Area::new(egui::Id::new("hud_toggle"))
        .anchor(egui::Align2::LEFT_BOTTOM, Vec2::new(8.0, -8.0))
        .show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(SLOT), Sense::click());
            paint_image(ui, toggle, r, Color32::WHITE);
            if resp.on_hover_text("Одежда").clicked() {
                state.clothes_open = !state.clothes_open;
            }
        });
    alerts(
        ctx,
        icons,
        state.health.as_ref(),
        state.vitals.as_ref(),
        &mut out,
    );
    egui::Area::new(egui::Id::new("hud_bar"))
        .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -8.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(GAP);
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
            .anchor(
                egui::Align2::LEFT_BOTTOM,
                Vec2::new(8.0, -(8.0 + SLOT + GAP)),
            )
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
    drag_ghost_and_world_drop(ctx, icons, ground, &mut out);
    out
}

/// Панель алертов справа вверху (`AlertsUI` SS14), столбцом: здоровье (`health0…4` по доле урона до
/// критического порога, критическое состояние и смерть — свои значки), кровотечение (`bleed0…10`) и
/// «нечем дышать» (`not_enough_oxy`).
fn alerts(
    ctx: &egui::Context,
    icons: &mut Icons,
    health: Option<&gameplay::HealthView>,
    vitals: Option<&gameplay::Vitals>,
    out: &mut Commands,
) {
    let Some(h) = health else { return };
    let total: f32 = h.damage.values().sum();
    let mut shown: Vec<(String, String)> = Vec::new();
    let mut fire_at: Option<usize> = None;
    let mut parts: Vec<String> = h
        .parts
        .iter()
        .filter(|(_, v)| **v > 0.0)
        .map(|(p, v)| format!("{}: {v:.0}", part_ru(p)))
        .collect();
    parts.sort();
    let tip = if parts.is_empty() {
        format!("Урон: {total:.0}")
    } else {
        format!("Урон: {total:.0}\n{}", parts.join("\n"))
    };
    let health_icon = match h.state {
        gameplay::MobState::Dead => "Alerts/human_dead.rsi/dead".to_string(),
        gameplay::MobState::Critical => "Alerts/human_critical.rsi/critical".to_string(),
        gameplay::MobState::Alive => {
            let sev = ((total / gameplay::CRITICAL_AT) * 5.0)
                .floor()
                .clamp(0.0, 4.0) as u8;
            format!("Alerts/human_alive.rsi/health{sev}")
        }
    };
    shown.push((health_icon, tip));
    if let Some(v) = vitals {
        if v.bleed > 0.0 {
            // Тяжесть значка — целая часть силы кровотечения, 0…10 (`TryModifyBleedAmount`).
            let sev = v.bleed.floor().clamp(0.0, 10.0) as u8;
            shown.push((
                format!("Alerts/bleed.rsi/bleed{sev}"),
                format!(
                    "Кровотечение: {:.1}; крови {:.0}%",
                    v.bleed,
                    v.blood * 100.0
                ),
            ));
        } else if v.blood < 0.9 {
            shown.push((
                "Alerts/bleed.rsi/bleed0".to_string(),
                format!("Малокровие: {:.0}% крови", v.blood * 100.0),
            ));
        }
        if v.temperature_alert != 0 {
            let kind = if v.temperature_alert > 0 {
                "hot"
            } else {
                "cold"
            };
            let tip = if v.temperature_alert > 0 {
                format!("Жарко: {:.0} °C", v.temperature - 273.15)
            } else {
                format!("Холодно: {:.0} °C", v.temperature - 273.15)
            };
            shown.push((
                format!("Alerts/temperature.rsi/{kind}{}", v.temperature_alert.abs()),
                tip,
            ));
        }
        if v.on_fire {
            fire_at = Some(shown.len());
            shown.push((
                "Alerts/Fire/fire".to_string(),
                "Вы горите! Нажмите, чтобы упасть и сбить огонь (Z)".to_string(),
            ));
        }
        if v.low_oxygen {
            shown.push((
                "Alerts/breathing.rsi/not_enough_oxy".to_string(),
                "Нечем дышать".to_string(),
            ));
        }
        match v.pressure_alert {
            gameplay::PressureAlert::None => {}
            gameplay::PressureAlert::Low(n) => shown.push((
                format!("Alerts/pressure.rsi/lowpressure{n}"),
                format!("Низкое давление: {:.0} кПа", v.pressure_kpa),
            )),
            gameplay::PressureAlert::High(n) => shown.push((
                format!("Alerts/pressure.rsi/highpressure{n}"),
                format!("Высокое давление: {:.0} кПа", v.pressure_kpa),
            )),
        }
        match v.hunger_alert {
            1 => shown.push((
                "Alerts/hunger.rsi/peckish".to_string(),
                format!("Хочется есть ({:.0})", v.hunger),
            )),
            2 => shown.push((
                "Alerts/hunger.rsi/starving".to_string(),
                format!("Голод ({:.0})", v.hunger),
            )),
            _ => {}
        }
        match v.thirst_alert {
            1 => shown.push((
                "Alerts/thirst.rsi/thirsty".to_string(),
                format!("Хочется пить ({:.0})", v.thirst),
            )),
            2 => shown.push((
                "Alerts/thirst.rsi/parched".to_string(),
                format!("Мучает жажда ({:.0})", v.thirst),
            )),
            _ => {}
        }
    }
    let time = ctx.input(|i| i.time);
    let frames: Vec<_> = shown
        .iter()
        .map(|(n, _)| icons.animated(ctx, n, time))
        .collect();
    egui::Area::new(egui::Id::new("hud_alerts"))
        .anchor(egui::Align2::RIGHT_TOP, Vec2::new(-8.0, 8.0))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(GAP);
            for (i, ((_, tip), frame)) in shown.iter().zip(frames).enumerate() {
                let sense = if fire_at == Some(i) {
                    Sense::click()
                } else {
                    Sense::hover()
                };
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(SLOT), sense);
                if let Some((tex, uv)) = frame {
                    ui.painter().image(tex, r, uv, Color32::WHITE);
                }
                if resp.on_hover_text(tip).clicked() {
                    out.push((gameplay::CMD_RESIST, Vec::new()));
                }
            }
        });
}

fn part_ru(p: &str) -> &str {
    match p {
        "Head" => "Голова",
        "Torso" => "Торс",
        "LeftArm" => "Левая рука",
        "RightArm" => "Правая рука",
        "LeftLeg" => "Левая нога",
        "RightLeg" => "Правая нога",
        other => other,
    }
}

/// Значок перетаскиваемого предмета у курсора; отпущенный вне окон худа предмет падает на пол.
fn drag_ghost_and_world_drop(
    ctx: &egui::Context,
    icons: &mut Icons,
    ground: Option<[f32; 3]>,
    out: &mut Commands,
) {
    let Some(payload) = egui::DragAndDrop::payload::<Drag>(ctx) else {
        return;
    };
    if let Some(pos) = ctx.pointer_interact_pos()
        && let Some(tex) = icons.item(ctx, &payload.proto)
    {
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("drag_ghost"));
        ctx.layer_painter(layer).image(
            tex,
            Rect::from_center_size(pos, Vec2::splat(SLOT * 0.8)),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::from_white_alpha(220),
        );
    }
    let over_window = ctx
        .pointer_latest_pos()
        .and_then(|p| ctx.layer_id_at(p))
        .is_some_and(|l| l.order != egui::Order::Background);
    if ctx.input(|i| i.pointer.any_released()) && !over_window {
        drop_to_world(&payload, ground, out);
        egui::DragAndDrop::clear_payload(ctx);
    }
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
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(SLOT), Sense::click_and_drag());
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
        if hands.is_disabled(i) {
            let b = icons.ui(ctx, "Default/blocked");
            paint_image(ui, b, rect, Color32::WHITE);
        }
        if let Some(it) = item
            && let Some(n) = view.and_then(|v| v.charges.iter().find(|(id, _)| *id == it.id))
        {
            ui.painter().text(
                rect.right_bottom() - Vec2::new(4.0, 2.0),
                egui::Align2::RIGHT_BOTTOM,
                n.1.to_string(),
                egui::FontId::proportional(16.0),
                Color32::WHITE,
            );
        }
        slot_overlays(ctx, ui, icons, rect, active, resp.hovered());
        if let Some(it) = item
            && resp.drag_started()
        {
            resp.dnd_set_drag_payload(Drag {
                src: Src::Hand(i as u8),
                proto: it.prototype.to_string(),
            });
        }
        if let Some(d) = resp.dnd_release_payload::<Drag>() {
            drop_on_hand(&d, i as u8, out);
        }
        if resp.clicked() {
            out.push((CMD_SELECT_HAND, vec![i as u8]));
        }
        if let Some(it) = item {
            let mut label = it
                .prototype
                .as_str()
                .split_once(':')
                .map_or("", |p| p.1)
                .to_string();
            if let Some(f) = view.and_then(|v| v.fills.iter().find(|f| f.0 == it.id)) {
                label.push_str(&format!(" · {:.0}/{:.0}", f.1, f.2));
                if f.3 {
                    label.push_str(" · закрыто (F — открыть)");
                }
            }
            let resp = resp.on_hover_text(label);
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
    let resp = ui.interact(rect, ui.id().with(slot), Sense::click_and_drag());
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
    if let Some(it) = worn
        && resp.drag_started()
    {
        resp.dnd_set_drag_payload(Drag {
            src: Src::Slot(slot.to_string()),
            proto: it.prototype.to_string(),
        });
    }
    if let Some(d) = resp.dnd_release_payload::<Drag>() {
        match &d.src {
            Src::Hand(h) => out.push((CMD_EQUIP, equip_data(*h, slot))),
            Src::Slot(from) if from != slot => {
                out.push((CMD_MOVE_SLOT, move_slot_data(from, slot)));
            }
            _ => {}
        }
    }
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

/// Окно торгового автомата (`VendingMachineMenu`): список товаров со значками и остатком; нажатие
/// заказывает товар, закрытие окна закрывает автомат.
pub fn vending_window(
    ctx: &egui::Context,
    icons: &mut Icons,
    view: Option<&gameplay::VendingView>,
) -> Commands {
    let mut out: Commands = Vec::new();
    let Some(v) = view else {
        return out;
    };
    let mut open = true;
    egui::Window::new(&v.name)
        .id(egui::Id::new("vending"))
        .default_pos(Pos2::new(120.0, 120.0))
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    for item in &v.items {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
                            let tex = icons.item(ctx, &item.proto);
                            paint_image(ui, tex, r, Color32::WHITE);
                            let enabled = item.amount > 0 && !v.ejecting;
                            let button =
                                egui::Button::new(format!("{} [{}]", item.name, item.amount));
                            if ui.add_enabled(enabled, button).clicked() {
                                out.push((gameplay::CMD_VEND, item.name.clone().into_bytes()));
                            }
                        });
                    }
                });
            if v.denying {
                ui.colored_label(Color32::LIGHT_RED, "Отказано");
            } else if v.ejecting {
                ui.label("Выдаю…");
            }
        });
    if !open {
        out.push((gameplay::CMD_VEND_CLOSE, Vec::new()));
    }
    out
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
            let (outer, bag_resp) = ui.allocate_exact_size(size, Sense::hover());
            if let Some(d) = bag_resp.dnd_release_payload::<Drag>()
                && let Src::Hand(h) = d.src
            {
                out.push((CMD_STORE, store_data(h, bag.id)));
            }
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
                    Sense::click_and_drag(),
                );
                if resp.drag_started() {
                    resp.dnd_set_drag_payload(Drag {
                        src: Src::Bag {
                            bag: bag.id,
                            item: st.item.id,
                        },
                        proto: st.item.prototype.to_string(),
                    });
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use gameplay::HeldItem;

    fn item(n: u128) -> HeldItem {
        HeldItem {
            id: ObjectId::from_u128(n),
            prototype: astrocore_core::PrototypeId::new("ss14:pen").unwrap(),
        }
    }

    /// Прогнать худ несколько кадров с событиями указателя; вернуть все команды.
    fn run(hands: &Hands, view: Option<&InventoryView>, path: &[(f32, f32)]) -> Commands {
        let ctx = egui::Context::default();
        let mut icons = Icons::new(PathBuf::from("/nonexistent"), HashMap::new());
        let mut state = HudState::default();
        let mut all = Commands::new();
        let mut frame = |events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0))),
                events,
                ..Default::default()
            };
            ctx.begin_pass(raw);
            all.extend(draw(
                &ctx,
                &mut icons,
                &mut state,
                Some(hands),
                view,
                false,
                GROUND,
                None,
            ));
            let mut out = ctx.end_pass();
            out.textures_delta.clear();
        };
        for _ in 0..3 {
            frame(vec![]);
        }
        let at = |p: (f32, f32)| Pos2::new(p.0, p.1);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(at(path[0]))]);
        frame(vec![button(at(path[0]), true)]);
        for p in &path[1..] {
            frame(vec![egui::Event::PointerMoved(at(*p))]);
            frame(vec![]);
        }
        let end = at(path[path.len() - 1]);
        frame(vec![button(end, false)]);
        frame(vec![]);
        all
    }

    // Раскладка нижней полосы: кнопка одежды 0,7 слота, затем три слота, две руки и три слота;
    // слот 64 px, промежуток 4 px, полоса по центру экрана 1280 px и в 8 px от нижнего края.
    const GROUND: Option<[f32; 3]> = Some([1.0, 0.2, 2.0]);
    const LEFT_HAND: (f32, f32) = (630.4, 680.0);
    const RIGHT_HAND: (f32, f32) = (698.4, 680.0);

    #[test]
    fn dragging_an_item_to_the_other_hand_moves_it() {
        let mut hands = Hands::humanoid();
        hands.pickup_to(0, item(1)).unwrap();
        let cmds = run(&hands, None, &[LEFT_HAND, (660.0, 680.0), RIGHT_HAND]);
        assert!(
            cmds.contains(&(CMD_MOVE_HAND, move_hand_data(0, 1))),
            "{cmds:?}"
        );
    }

    #[test]
    fn releasing_an_item_over_empty_screen_drops_it() {
        let mut hands = Hands::humanoid();
        hands.pickup_to(0, item(1)).unwrap();
        let cmds = run(&hands, None, &[LEFT_HAND, (600.0, 500.0), (600.0, 300.0)]);
        assert!(
            cmds.contains(&(CMD_DROP, drop_at_data(0, [1.0, 0.2, 2.0]))),
            "{cmds:?}"
        );
    }

    #[test]
    fn dragging_an_empty_hand_does_nothing() {
        let hands = Hands::humanoid();
        let cmds = run(&hands, None, &[LEFT_HAND, RIGHT_HAND]);
        assert!(
            cmds.iter()
                .all(|(k, _)| *k != CMD_MOVE_HAND && *k != CMD_DROP)
        );
    }

    #[test]
    fn dragging_an_item_onto_a_clothing_slot_equips_it() {
        let mut hands = Hands::humanoid();
        hands.pickup_to(0, item(1)).unwrap();
        let view = InventoryView {
            inventory: gameplay::Inventory::human(),
            bags: Vec::new(),
            charges: Vec::new(),
            fills: Vec::new(),
        };
        // Слоты левой полосы: id, belt, back; «back» — третий.
        let cmds = run(
            &hands,
            Some(&view),
            &[LEFT_HAND, (600.0, 680.0), (562.4, 680.0)],
        );
        assert!(
            cmds.contains(&(CMD_EQUIP, equip_data(0, "back"))),
            "{cmds:?}"
        );
    }

    fn belt_view() -> InventoryView {
        let mut inventory = gameplay::Inventory::human();
        let info = gameplay::ItemInfo {
            size: gameplay::ItemSize::Small,
            slots: ["belt".to_string()].into(),
            storage: None,
            qualities: Default::default(),
            tags: Default::default(),
            components: Default::default(),
        };
        inventory.equip("belt", item(7), &info).unwrap();
        InventoryView {
            inventory,
            bags: Vec::new(),
            charges: Vec::new(),
            fills: Vec::new(),
        }
    }

    const BELT: (f32, f32) = (494.4, 680.0);

    #[test]
    fn dragging_worn_items_to_a_hand_or_another_slot() {
        let hands = Hands::humanoid();
        let view = belt_view();
        let to_hand = run(&hands, Some(&view), &[BELT, (600.0, 680.0), RIGHT_HAND]);
        assert!(
            to_hand.contains(&(CMD_UNEQUIP_TO, unequip_to_data(1, "belt"))),
            "{to_hand:?}"
        );
        let to_slot = run(&hands, Some(&view), &[BELT, (530.0, 680.0), (562.4, 680.0)]);
        assert!(
            to_slot.contains(&(CMD_MOVE_SLOT, move_slot_data("belt", "back"))),
            "{to_slot:?}"
        );
    }
}
