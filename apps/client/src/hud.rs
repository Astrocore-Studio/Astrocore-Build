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
    first_frames: HashMap<String, Rect>,
    /// Названия прототипов для подсказки под прицелом.
    names: HashMap<String, AimInfo>,
}

/// Название и размер спрайта прототипа: по ним выбирается объект под прицелом.
#[derive(Clone, Debug)]
pub struct AimInfo {
    pub name: String,
    /// Ширина и высота спрайта, м.
    pub size: [f32; 2],
    /// Лежит на полу плашмя.
    pub flat: bool,
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
            first_frames: HashMap::new(),
            names: HashMap::new(),
        }
    }

    /// Название прототипа (`EntityPrototype.name`).
    pub fn aim_info(&self, proto: &str) -> Option<&AimInfo> {
        self.names.get(proto)
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
        let names = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| {
                v.get("sprites")?.as_object().map(|m| {
                    m.iter()
                        .filter_map(|(k, e)| {
                            let size = e.get("size")?.as_array()?;
                            Some((
                                k.clone(),
                                AimInfo {
                                    name: e.get("name")?.as_str()?.to_string(),
                                    size: [
                                        size.first()?.as_f64()? as f32,
                                        size.get(1)?.as_f64()? as f32,
                                    ],
                                    flat: e.get("layout").and_then(|l| l.as_str()) == Some("flat"),
                                },
                            ))
                        })
                        .collect()
                })
            })
            .unwrap_or_default();
        let mut icons = Self::new(root, items);
        icons.names = names;
        icons
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

    pub(crate) fn first_frame(
        &mut self,
        ctx: &egui::Context,
        rel: &str,
    ) -> Option<(egui::TextureId, Rect)> {
        let texture = self.load(ctx, rel)?;
        let uv = *self.first_frames.entry(rel.into()).or_insert_with(|| {
            let path = self.root.join(rel);
            let (width, height) = image::image_dimensions(&path).unwrap_or((32, 32));
            let meta = path
                .parent()
                .and_then(|p| std::fs::read(p.join("meta.json")).ok())
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
            let frame = |key| {
                meta.as_ref()
                    .and_then(|m| m.get("size")?.get(key)?.as_f64())
                    .unwrap_or(32.0) as f32
            };
            Rect::from_min_max(
                Pos2::ZERO,
                Pos2::new(frame("x") / width as f32, frame("y") / height as f32),
            )
        });
        Some((texture, uv))
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
    /// Окно выбора должности закрыто вручную.
    pub jobs_hidden: bool,
    /// Чат: введённый текст, канал и просьба вернуть фокус полю ввода.
    pub chat_input: String,
    pub chat_channel: u8,
    pub chat_focus: bool,
    /// Название объекта под прицелом.
    pub aim: Option<String>,
    /// Режимы кнопок справа внизу (боевой режим, шаг вместо бега).
    pub combat: bool,
    pub walk: bool,
    /// Режим броска: следующий щелчок по миру бросает предмет из руки.
    pub throw_mode: bool,
    /// Объект под прицелом (для правой кнопки и осмотра).
    pub aim_id: Option<ObjectId>,
    /// Открытое меню глаголов: куда, для какой цели и с какого ответа сервера ждать список.
    pub menu: Option<VerbMenu>,
    /// Окно осмотра закрыто на ответе с этим номером.
    pub examine_closed: u32,
    /// Лежит ли игрок (реплика сервера).
    pub lying: bool,
    /// Выносливость: процент порога и крит (реплика сервера).
    pub stamina: (u8, bool),
    /// Поля окна консоли ID: правятся локально, уходят по кнопке «Записать».
    pub idc_name: String,
    pub idc_title: String,
    pub idc_access: std::collections::BTreeSet<String>,
    /// Какая целевая карта сейчас показана (консоль и имя прототипа): при смене поля сбрасываются.
    pub idc_for: Option<(ObjectId, String)>,
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
    edge_panel(ctx, state, &mut out);
    if first_person {
        aim_tooltip(ctx, state);
    }
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

/// Подсказка под прицелом: название объекта, на который смотрит игрок (SS14 3D).
fn aim_tooltip(ctx: &egui::Context, state: &HudState) {
    let Some(name) = state.aim.as_deref() else {
        return;
    };
    let c = ctx.content_rect().center();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("aim_tooltip"),
    ));
    let font = egui::FontId::proportional(15.0);
    let galley = painter.layout_no_wrap(name.to_string(), font, Color32::from_gray(235));
    let rect = Rect::from_min_size(
        Pos2::new(c.x - galley.size().x * 0.5, c.y + 22.0),
        galley.size(),
    )
    .expand2(Vec2::new(8.0, 3.0));
    painter.rect_filled(rect, 3.0, Color32::from_rgba_unmultiplied(8, 10, 16, 190));
    painter.galley(rect.min + Vec2::new(8.0, 3.0), galley, Color32::WHITE);
}

/// Цвет части тела куклы по доле урона до порога отказа: от зелёного к красному.
fn doll_color(damage: f32, limit: f32) -> Color32 {
    let t = (damage / limit).clamp(0.0, 1.0);
    let (r, g) = (60.0 + 190.0 * t, 200.0 - 150.0 * t);
    Color32::from_rgb(r as u8, g as u8, 70)
}

/// Панель справа внизу (`HandsGui`/`HealthDoll` SS14 3D): кнопки COMBAT, RUN/WALK, RESIST, REST,
/// THROW и кукла здоровья с процентом. REST и THROW пока не перенесены: кнопки неактивны.
fn edge_panel(ctx: &egui::Context, state: &mut HudState, out: &mut Commands) {
    let health = state.health.clone();
    let state_stamina = state.stamina;
    egui::Area::new(egui::Id::new("edge_buttons"))
        .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-84.0, -10.0))
        .show(ctx, |ui| {
            let mode = |on: bool| {
                if on {
                    Color32::from_rgb(150, 50, 50)
                } else {
                    Color32::from_rgb(40, 46, 60)
                }
            };
            let size = Vec2::new(86.0, 24.0);
            let btn = |label: &str, fill: Color32| {
                egui::Button::new(egui::RichText::new(label).size(12.0).strong())
                    .fill(fill)
                    .min_size(size)
            };
            if ui
                .add(btn("COMBAT", mode(state.combat)))
                .on_hover_text("Боевой режим: бить можно только в нём")
                .clicked()
            {
                state.combat = !state.combat;
                out.push((gameplay::CMD_COMBAT, vec![u8::from(state.combat)]));
            }
            let run = if state.walk { "WALK" } else { "RUN" };
            if ui
                .add(btn(run, mode(state.walk)))
                .on_hover_text("Бег / шаг")
                .clicked()
            {
                state.walk = !state.walk;
                out.push((gameplay::CMD_WALK, vec![u8::from(state.walk)]));
            }
            if ui
                .add(btn("RESIST", mode(false)))
                .on_hover_text("Сопротивляться: потушить огонь, выбраться из капсулы")
                .clicked()
            {
                out.push((gameplay::CMD_RESIST, Vec::new()));
            }
            let lying = state.lying;
            if ui
                .add(btn(if lying { "STAND" } else { "REST" }, mode(lying)))
                .on_hover_text("Лечь / встать (вставать 2 с, ползком медленнее)")
                .clicked()
            {
                out.push((gameplay::CMD_REST, Vec::new()));
            }
            if ui
                .add(btn("THROW", mode(state.throw_mode)))
                .on_hover_text("Бросок: щёлкните по миру, чтобы бросить предмет из руки")
                .clicked()
            {
                state.throw_mode = !state.throw_mode;
            }
        });
    egui::Area::new(egui::Id::new("edge_doll"))
        .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-10.0, -10.0))
        .show(ctx, |ui| {
            // Кукла здоровья и процент (`total / CRITICAL_AT`).
            let (area, _) = ui.allocate_exact_size(Vec2::new(64.0, 130.0), Sense::hover());
            let total: f32 = health.as_ref().map_or(0.0, |h| h.damage.values().sum());
            let pct = (100.0 - total / gameplay::CRITICAL_AT * 100.0).clamp(0.0, 100.0);
            let dmg = |part: &str| {
                health
                    .as_ref()
                    .and_then(|h| h.parts.get(part))
                    .copied()
                    .unwrap_or(0.0)
            };
            let p = ui.painter();
            let o = area.min;
            let part = |p: &egui::Painter, name: &str, limit: f32, r: Rect| {
                p.rect_filled(r, 3.0, doll_color(dmg(name), limit));
                p.rect_stroke(
                    r,
                    3.0,
                    egui::Stroke::new(1.0, Color32::from_black_alpha(160)),
                    egui::StrokeKind::Inside,
                );
            };
            let at = |x: f32, y: f32, w: f32, h: f32| {
                Rect::from_min_size(o + Vec2::new(x, y), Vec2::new(w, h))
            };
            part(
                p,
                "Head",
                gameplay::HEAD_FATAL_AT,
                at(23.0, 0.0, 18.0, 18.0),
            );
            part(
                p,
                "Torso",
                gameplay::TORSO_FATAL_AT,
                at(17.0, 20.0, 30.0, 38.0),
            );
            part(
                p,
                "LeftArm",
                gameplay::LIMB_DISABLED_AT,
                at(5.0, 20.0, 10.0, 36.0),
            );
            part(
                p,
                "RightArm",
                gameplay::LIMB_DISABLED_AT,
                at(49.0, 20.0, 10.0, 36.0),
            );
            part(
                p,
                "LeftLeg",
                gameplay::LIMB_DISABLED_AT,
                at(17.0, 60.0, 14.0, 40.0),
            );
            part(
                p,
                "RightLeg",
                gameplay::LIMB_DISABLED_AT,
                at(33.0, 60.0, 14.0, 40.0),
            );
            // Полоса выносливости под куклой: заполняется по мере усталости, красная в крите.
            if state_stamina.0 > 0 {
                let bar = Rect::from_min_size(o + Vec2::new(0.0, 122.0), Vec2::new(64.0, 6.0));
                p.rect_filled(bar, 2.0, Color32::from_black_alpha(160));
                let fill = Rect::from_min_size(
                    bar.min,
                    Vec2::new(
                        bar.width() * f32::from(state_stamina.0) / 100.0,
                        bar.height(),
                    ),
                );
                let c = if state_stamina.1 {
                    Color32::from_rgb(220, 60, 60)
                } else {
                    Color32::from_rgb(230, 190, 60)
                };
                p.rect_filled(fill, 2.0, c);
            }
            p.text(
                o + Vec2::new(32.0, 109.0),
                egui::Align2::CENTER_CENTER,
                format!("{pct:.0}%"),
                egui::FontId::proportional(15.0),
                Color32::WHITE,
            );
        });
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
/// Окно чата (`ChatUIController`): журнал, выбор канала и строка ввода. Клавиша T или клик по
/// строке ввода даёт ей фокус; пока она в фокусе, клавиши не двигают персонажа.
pub fn chat_panel(
    ctx: &egui::Context,
    state: &mut HudState,
    view: Option<&gameplay::ChatView>,
    pos: Pos2,
) -> Commands {
    use gameplay::Channel;
    let mut out: Commands = Vec::new();
    let channels = [
        (Channel::Say, "Сказать"),
        (Channel::Whisper, "Шепнуть"),
        (Channel::Emote, "Действие"),
        (Channel::Ooc, "OOC"),
    ];
    if !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
        state.chat_focus = true;
    }
    egui::Area::new(egui::Id::new("chat"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(8, 10, 16, 190))
                .corner_radius(4.0)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.set_width(380.0);
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for l in view.map_or(&[][..], |v| &v.lines[..]) {
                                let (text, color) = match l.channel {
                                    Channel::Say => (
                                        format!("{} говорит: «{}»", l.who, l.text),
                                        Color32::from_gray(230),
                                    ),
                                    Channel::Whisper => (
                                        format!("{} шепчет: «{}»", l.who, l.text),
                                        Color32::from_gray(160),
                                    ),
                                    Channel::Emote => (
                                        format!("* {} {}", l.who, l.text),
                                        Color32::from_rgb(230, 200, 120),
                                    ),
                                    Channel::Ooc => (
                                        format!("OOC {}: {}", l.who, l.text),
                                        Color32::from_rgb(120, 170, 255),
                                    ),
                                    Channel::Dead => (
                                        format!("МЕРТВЫЕ {}: {}", l.who, l.text),
                                        Color32::from_rgb(190, 140, 255),
                                    ),
                                };
                                ui.label(egui::RichText::new(text).color(color).size(13.0));
                            }
                        });
                    ui.horizontal(|ui| {
                        let name = |c: u8| {
                            channels
                                .iter()
                                .find(|(ch, _)| ch.to_byte() == c)
                                .map_or("Сказать", |(_, n)| *n)
                        };
                        egui::ComboBox::from_id_salt("chat_channel")
                            .width(84.0)
                            .selected_text(name(state.chat_channel))
                            .show_ui(ui, |ui| {
                                for (ch, label) in channels {
                                    ui.selectable_value(
                                        &mut state.chat_channel,
                                        ch.to_byte(),
                                        label,
                                    );
                                }
                            });
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut state.chat_input)
                                .hint_text("Enter — написать")
                                .char_limit(gameplay::chat::MAX_LEN)
                                .desired_width(f32::INFINITY),
                        );
                        if state.chat_focus {
                            edit.request_focus();
                            state.chat_focus = false;
                        }
                        let enter =
                            edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if enter {
                            let text = std::mem::take(&mut state.chat_input);
                            if let Some(t) = gameplay::chat::clean(&text) {
                                // «/me …» — действие, «/ooc …» — OOC, «/w …» — шёпот.
                                let (channel, body) = match t.split_once(' ') {
                                    Some(("/me", b)) => (Channel::Emote.to_byte(), b.to_string()),
                                    Some(("/ooc", b)) => (Channel::Ooc.to_byte(), b.to_string()),
                                    Some(("/w", b)) => (Channel::Whisper.to_byte(), b.to_string()),
                                    _ => (state.chat_channel, t),
                                };
                                if let Some(ch) = Channel::from_byte(channel) {
                                    out.push((gameplay::CMD_CHAT, gameplay::chat_data(ch, &body)));
                                }
                            }
                        }
                    });
                });
        });
    out
}

/// Экран лобби (`LobbyGui`): заголовок, раунд, таймер и кнопка «Готов» / «Присоединиться».
pub fn lobby_screen(ctx: &egui::Context, v: &gameplay::LobbyView) -> Commands {
    let mut out: Commands = Vec::new();
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("lobby"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, 0.0, Color32::from_rgb(10, 12, 18));
            ui.set_min_size(screen.size());
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.2).max(24.0));
                ui.label(
                    egui::RichText::new("ASTROCORE BUILD")
                        .size(48.0)
                        .strong()
                        .color(Color32::from_rgb(170, 200, 255)),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("Станция {} · раунд {}", v.station, v.round))
                        .size(20.0)
                        .color(Color32::from_gray(190)),
                );
                ui.add_space(28.0);
                let timer = if v.started {
                    "Раунд идёт".to_string()
                } else {
                    format!("До начала раунда: {}:{:02}", v.left / 60, v.left % 60)
                };
                ui.label(egui::RichText::new(timer).size(28.0));
                ui.add_space(8.0);
                ui.label(format!("Игроков: {} · готовы: {}", v.total, v.ready_count));
                ui.add_space(24.0);
                let (text, fill) = if v.started {
                    ("Присоединиться", Color32::from_rgb(40, 110, 70))
                } else if v.ready {
                    ("Готов ✓", Color32::from_rgb(40, 110, 70))
                } else {
                    ("Не готов", Color32::from_rgb(90, 50, 50))
                };
                let button = egui::Button::new(egui::RichText::new(text).size(24.0))
                    .fill(fill)
                    .min_size(egui::vec2(240.0, 52.0));
                if ui.add(button).clicked() {
                    out.push((gameplay::CMD_LOBBY_READY, Vec::new()));
                }
                if !v.started {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(
                            "Готовые игроки выйдут на станцию, когда таймер дойдёт до нуля",
                        )
                        .small()
                        .color(Color32::from_gray(140)),
                    );
                }
            });
        });
    out
}

/// Сводка конца раунда (`RoundEndSummaryWindow`): причина, длительность, экипаж и таймер до
/// нового раунда.
pub fn roundend_panel(ctx: &egui::Context, v: &gameplay::RoundEndView) {
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("roundend"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.center().x - 200.0, screen.min.y + 60.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(Color32::from_rgba_unmultiplied(12, 14, 20, 235))
                .show(ui, |ui| {
                    ui.set_width(400.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("РАУНД ОКОНЧЕН")
                                .size(30.0)
                                .strong()
                                .color(Color32::from_rgb(255, 170, 120)),
                        );
                        ui.label(egui::RichText::new(&v.reason).size(18.0));
                        ui.label(format!(
                            "Раунд {} · длился {}:{:02}",
                            v.round,
                            v.duration / 60,
                            v.duration % 60
                        ));
                    });
                    ui.separator();
                    let alive = v.crew.iter().filter(|c| c.alive).count();
                    ui.label(format!("Экипаж: {} · выжили: {alive}", v.crew.len()));
                    for c in &v.crew {
                        let (mark, color) = if c.alive {
                            ("жив", Color32::from_rgb(120, 220, 140))
                        } else {
                            ("погиб", Color32::from_rgb(230, 100, 100))
                        };
                        ui.horizontal(|ui| {
                            ui.label(&c.job);
                            ui.colored_label(color, mark);
                        });
                    }
                    ui.separator();
                    ui.vertical_centered(|ui| {
                        ui.label(format!("Новый раунд через {} с", v.restart_in));
                    });
                });
        });
}

/// Открытое меню глаголов (`VerbMenu` SS14): точка на экране, цель и номер ответа, после которого
/// список считается пришедшим по этой цели.
#[derive(Clone, Debug)]
pub struct VerbMenu {
    pub at: Pos2,
    pub target: gameplay::VerbTarget,
    pub after_seq: u32,
}

/// Меню глаголов у курсора: список приходит от сервера (`CMD_VERB_LIST`); выбор глагола шлёт
/// `CMD_VERB_DO`. Закрывается щелчком мимо или Escape.
pub fn verb_menu(
    ctx: &egui::Context,
    state: &mut HudState,
    view: Option<&gameplay::VerbsView>,
) -> Commands {
    let mut out: Commands = Vec::new();
    let Some(menu) = state.menu.clone() else {
        return out;
    };
    let want = gameplay::target_data(menu.target, None);
    let Some(v) = view.filter(|v| v.seq > menu.after_seq && v.target == want) else {
        return out;
    };
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let resp = egui::Area::new(egui::Id::new("verb_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(menu.at)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(130.0);
                ui.label(egui::RichText::new("Действия").small().weak());
                for verb in &v.verbs {
                    let b = ui.add_enabled(verb.disabled.is_none(), egui::Button::new(&verb.label));
                    let b = match &verb.disabled {
                        Some(why) => b.on_disabled_hover_text(why),
                        None => b,
                    };
                    if b.clicked() {
                        out.push((
                            gameplay::CMD_VERB_DO,
                            gameplay::target_data(menu.target, Some(&verb.id)),
                        ));
                        close = true;
                    }
                }
            });
        });
    // Щелчок вне меню закрывает его.
    if ctx.input(|i| i.pointer.any_pressed())
        && !ctx
            .pointer_interact_pos()
            .is_some_and(|p| resp.response.rect.contains(p))
    {
        close = true;
    }
    if close {
        state.menu = None;
    }
    out
}

/// Окно осмотра (`ExamineSystem`): имя и строки описания.
pub fn examine_window(
    ctx: &egui::Context,
    state: &mut HudState,
    view: Option<&gameplay::ExamineView>,
) {
    let Some(v) = view.filter(|v| v.seq > state.examine_closed) else {
        return;
    };
    let mut open = true;
    egui::Window::new(&v.name)
        .id(egui::Id::new("examine"))
        .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 80.0))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_max_width(340.0);
            if v.lines.is_empty() {
                ui.label("Ничего особенного.");
            }
            for l in &v.lines {
                ui.label(l);
            }
        });
    if !open {
        state.examine_closed = v.seq;
    }
}

/// Окно выбора должности (`LateJoinGui`): список со свободными местами; пока должности нет.
pub fn jobs_window(
    ctx: &egui::Context,
    state: &mut HudState,
    view: Option<&gameplay::JobsView>,
) -> Commands {
    let mut out: Commands = Vec::new();
    let Some(v) = view else {
        return out;
    };
    if v.current.is_some() {
        // Взял должность — окно снова откроется после возрождения.
        state.jobs_hidden = false;
        return out;
    }
    if state.jobs_hidden {
        return out;
    }
    let mut open = true;
    egui::Window::new("Должность")
        .id(egui::Id::new("jobs"))
        .default_pos(Pos2::new(60.0, 80.0))
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(480.0)
                .show(ui, |ui| {
                    for j in &v.jobs {
                        let free = j.max < 0 || j.taken < j.max as u32;
                        let places = if j.max < 0 {
                            "∞".to_string()
                        } else {
                            format!("{}/{}", j.taken, j.max)
                        };
                        let b = egui::Button::new(format!("{} [{places}]", j.title));
                        if ui.add_enabled(free, b).clicked() {
                            out.push((gameplay::CMD_JOB, j.id.clone().into_bytes()));
                        }
                    }
                });
        });
    if !open {
        state.jobs_hidden = true;
    }
    out
}

/// Окно консоли выдачи ID (`IdCardConsoleWindow`): слоты, имя, должность и уровни доступа.
pub fn idconsole_window(
    ctx: &egui::Context,
    state: &mut HudState,
    view: Option<&gameplay::IdConsoleView>,
    active_hand: u8,
) -> Commands {
    let mut out: Commands = Vec::new();
    let Some(v) = view else {
        state.idc_for = None;
        return out;
    };
    // Новая целевая карта — поля берутся с неё.
    let key = v.target.as_ref().map(|t| (v.console, t.name.clone()));
    if state.idc_for != key {
        state.idc_for = key;
        if let Some(t) = &v.target {
            state.idc_name = t.full_name.clone();
            state.idc_title = t.job_title.clone();
            state.idc_access = t.access.iter().cloned().collect();
        }
    }
    let mut open = true;
    egui::Window::new("Консоль выдачи ID")
        .id(egui::Id::new("idconsole"))
        .default_pos(Pos2::new(140.0, 100.0))
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("ID с правами:");
                match &v.privileged {
                    Some(n) => {
                        ui.label(n);
                        if ui.button("Вынуть").clicked() {
                            out.push((gameplay::CMD_ID_EJECT, vec![0]));
                        }
                    }
                    None => {
                        if ui.button("Вставить из руки").clicked() {
                            out.push((gameplay::CMD_ID_INSERT, vec![0, active_hand]));
                        }
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Целевая карта:");
                match &v.target {
                    Some(t) => {
                        ui.label(&t.name);
                        if ui.button("Вынуть").clicked() {
                            out.push((gameplay::CMD_ID_EJECT, vec![1]));
                        }
                    }
                    None => {
                        if ui.button("Вставить из руки").clicked() {
                            out.push((gameplay::CMD_ID_INSERT, vec![1, active_hand]));
                        }
                    }
                }
            });
            let Some(_) = &v.target else {
                return;
            };
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Имя:");
                ui.text_edit_singleline(&mut state.idc_name);
            });
            ui.horizontal(|ui| {
                ui.label("Должность:");
                ui.text_edit_singleline(&mut state.idc_title);
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for level in &v.levels {
                        // Менять можно только уровни ID с правами (остальные видны, но недоступны).
                        let can = v.authorized && v.allowed.contains(level);
                        let mut on = state.idc_access.contains(level);
                        if ui
                            .add_enabled(can, egui::Checkbox::new(&mut on, level))
                            .changed()
                        {
                            if on {
                                state.idc_access.insert(level.clone());
                            } else {
                                state.idc_access.remove(level);
                            }
                        }
                    }
                });
            if ui
                .add_enabled(v.authorized, egui::Button::new("Записать"))
                .clicked()
            {
                let req = gameplay::WriteRequest {
                    name: state.idc_name.clone(),
                    title: state.idc_title.clone(),
                    access: state.idc_access.iter().cloned().collect(),
                };
                out.push((gameplay::CMD_ID_WRITE, gameplay::write_data(&req)));
            }
        });
    if !open {
        state.idc_for = None;
        out.push((gameplay::CMD_ID_CLOSE, Vec::new()));
    }
    out
}

/// Подсказки у нижнего края экрана: каждая держится 3 с и гаснет.
pub fn toasts(ctx: &egui::Context, list: &mut Vec<(String, f32)>, now: f32) {
    list.retain(|(_, t)| now - t < 3.5);
    if list.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -150.0])
        .interactable(false)
        .show(ctx, |ui| {
            for (text, t) in list.iter() {
                let alpha = (3.5 - (now - t)).clamp(0.0, 1.0);
                ui.label(
                    egui::RichText::new(text)
                        .color(Color32::from_white_alpha((alpha * 255.0) as u8))
                        .size(16.0)
                        .strong(),
                );
            }
        });
}

fn hex_color(s: &str) -> Color32 {
    let t = s.trim_start_matches('#');
    u32::from_str_radix(t, 16).map_or(Color32::WHITE, |v| {
        Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    })
}

/// Окно раздатчика реагентов (`ReagentDispenserWindow`): количество, банки, мензурка.
pub fn dispenser_window(
    ctx: &egui::Context,
    view: Option<&gameplay::DispenserView>,
    active_hand: u8,
) -> Commands {
    let mut out: Commands = Vec::new();
    let Some(v) = view else {
        return out;
    };
    let mut open = true;
    egui::Window::new("Раздатчик реагентов")
        .id(egui::Id::new("dispenser"))
        .default_pos(Pos2::new(160.0, 90.0))
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Количество:");
                for (i, a) in v.amounts.iter().enumerate() {
                    if ui
                        .selectable_label((v.amount - a).abs() < 1e-3, format!("{a}"))
                        .clicked()
                    {
                        out.push((gameplay::CMD_DISP_AMOUNT, vec![i as u8]));
                    }
                }
            });
            ui.separator();
            match &v.beaker {
                Some(b) => {
                    ui.horizontal(|ui| {
                        ui.label(format!("Мензурка {:.0}/{:.0}", b.volume, b.max));
                        if ui.button("Очистить").clicked() {
                            out.push((gameplay::CMD_DISP_CLEAR, Vec::new()));
                        }
                        if ui.button("Вынуть").clicked() {
                            out.push((gameplay::CMD_DISP_EJECT, Vec::new()));
                        }
                    });
                    ui.label(format!("  температура {:.1} °C", b.temperature - 273.15));
                    for (r, q) in &b.reagents {
                        ui.label(format!("  {r}: {q:.1}"));
                    }
                }
                None => {
                    if ui.button("Вставить мензурку из руки").clicked() {
                        out.push((gameplay::CMD_DISP_INSERT, vec![active_hand]));
                    }
                }
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    for j in &v.jugs {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                            ui.painter().rect_filled(r, 2.0, hex_color(&j.color));
                            let b = egui::Button::new(format!("{} [{:.0}]", j.label, j.quantity));
                            if ui
                                .add_enabled(v.beaker.is_some() && j.quantity > 0.0, b)
                                .clicked()
                            {
                                out.push((gameplay::CMD_DISP_DISPENSE, vec![j.index]));
                            }
                        });
                    }
                });
        });
    if !open {
        out.push((gameplay::CMD_DISP_CLOSE, Vec::new()));
    }
    out
}

/// Окно криокапсулы (`CryoPodWindow`): пациент, газ, мензурка и буфер впрыска.
pub fn cryo_window(
    ctx: &egui::Context,
    view: Option<&gameplay::CryoView>,
    active_hand: u8,
) -> Commands {
    let mut out: Commands = Vec::new();
    let Some(v) = view else {
        return out;
    };
    let mut open = true;
    egui::Window::new("Криокапсула")
        .id(egui::Id::new("cryo"))
        .default_pos(Pos2::new(200.0, 110.0))
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            if !v.powered {
                ui.colored_label(Color32::LIGHT_RED, "Нет питания");
            }
            match &v.patient {
                Some(p) => {
                    ui.label(format!(
                        "Пациент: {}, тело {:.1} °C, кровь {:.0}%, урон {:.0}",
                        p.state,
                        p.temperature_k - 273.15,
                        p.blood * 100.0,
                        p.total_damage
                    ));
                    if ui.button("Вынуть пациента").clicked() {
                        out.push((gameplay::CMD_CRYO_EJECT, Vec::new()));
                    }
                }
                None => {
                    ui.label("Капсула пуста");
                    for (id, label) in &v.nearby {
                        if ui.button(format!("Положить: {label}")).clicked() {
                            out.push((gameplay::CMD_CRYO_PUT, id.to_le_bytes().to_vec()));
                        }
                    }
                }
            }
            ui.separator();
            if v.gases.is_empty() {
                ui.label("Воздуха нет (трубы не подключены)");
            } else {
                ui.label(format!(
                    "Воздух: {:.1} °C, {:.1} кПа",
                    v.air_temperature_k - 273.15,
                    v.air_pressure_kpa
                ));
            }
            for (g, pct) in &v.gases {
                ui.label(format!("   {g}: {pct:.1}%"));
            }
            ui.separator();
            match &v.beaker {
                Some(b) => {
                    ui.horizontal(|ui| {
                        ui.label(format!("Мензурка {:.0}/{:.0}", b.volume, b.max));
                        if ui.button("Вынуть").clicked() {
                            out.push((gameplay::CMD_CRYO_BEAKER_EJECT, Vec::new()));
                        }
                    });
                    for (_, name, q) in &b.reagents {
                        ui.label(format!("   {name}: {q:.1}"));
                    }
                    ui.horizontal(|ui| {
                        ui.label("Налить в буфер:");
                        for (i, a) in v.amounts.iter().enumerate() {
                            if ui
                                .add_enabled(
                                    v.patient.is_some(),
                                    egui::Button::new(format!("{a:.0}")),
                                )
                                .clicked()
                            {
                                out.push((gameplay::CMD_CRYO_INJECT, vec![i as u8]));
                            }
                        }
                    });
                }
                None => {
                    if ui.button("Поставить мензурку из руки").clicked() {
                        out.push((gameplay::CMD_CRYO_INSERT, vec![active_hand]));
                    }
                }
            }
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Буфер впрыска: {:.0} ед.",
                    v.buffer_volume.max(0.0) + 0.0
                ));
                if ui.button("Очистить").clicked() {
                    out.push((gameplay::CMD_CRYO_CLEAR, Vec::new()));
                }
            });
            for (_, name, q) in &v.buffer {
                ui.label(format!("   {name}: {q:.1}"));
            }
        });
    if !open {
        out.push((gameplay::CMD_CRYO_CLOSE, Vec::new()));
    }
    out
}

/// Окно химической машины: центрифуга и электролизёр (слот и пуск) или ХимМастер (буфер, таблетки, бутылки).
pub fn lab_window(
    ctx: &egui::Context,
    view: Option<&gameplay::LabView>,
    active_hand: u8,
) -> Commands {
    use egui::{Id, Window};
    let mut out: Commands = Vec::new();
    let Some(v) = view else {
        return out;
    };
    let mut open = true;
    let temp = |t: f32| format!("{:.1} °C ({:.0} K)", t - 273.15, t);
    let container = |ui: &mut egui::Ui,
                     title: &str,
                     c: &Option<gameplay::chemlab::ContainerInfo>,
                     slot: u8,
                     out: &mut Commands,
                     insert: bool| {
        ui.horizontal(|ui| match c {
            Some(c) => {
                ui.label(format!("{title}: {} {:.0}/{:.0}", c.name, c.volume, c.max));
                if ui.button("Вынуть").clicked() {
                    out.push((gameplay::CMD_LAB_EJECT, vec![slot]));
                }
            }
            None => {
                ui.label(format!("{title}: пусто"));
                if insert && ui.button("Поставить из руки").clicked() {
                    out.push((gameplay::CMD_LAB_INSERT, vec![active_hand]));
                }
            }
        });
        if let Some(c) = c {
            if c.max > 0.0 {
                ui.label(format!("   температура {}", temp(c.temperature)));
            }
            for (_, name, q) in &c.reagents {
                ui.label(format!("   {name}: {q:.1}"));
            }
            for (name, dose) in &c.items {
                ui.label(format!("   · {name} ({dose:.0} ед.)"));
            }
        }
    };
    Window::new(v.title.clone())
        .id(Id::new("lab"))
        .default_pos(Pos2::new(180.0, 100.0))
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            if v.kind == "mixer" {
                container(ui, "Слот", &v.input, 0, &mut out, true);
                ui.separator();
                match v.mixing {
                    Some(left) => {
                        ui.add(
                            egui::ProgressBar::new(
                                (1.0 - left / v.duration.max(0.1)).clamp(0.0, 1.0),
                            )
                            .text(format!("Работает, осталось {left:.0} с")),
                        );
                    }
                    None => {
                        if ui
                            .add_enabled(v.input.is_some(), egui::Button::new("Запустить"))
                            .clicked()
                        {
                            out.push((gameplay::CMD_LAB_START, Vec::new()));
                        }
                    }
                }
                return;
            }
            ui.horizontal(|ui| {
                if ui.selectable_label(!v.discard, "Перенос").clicked() {
                    out.push((gameplay::CMD_LAB_MODE, vec![0]));
                }
                if ui.selectable_label(v.discard, "Удаление").clicked() {
                    out.push((gameplay::CMD_LAB_MODE, vec![1]));
                }
            });
            container(ui, "Мензурка", &v.input, 0, &mut out, true);
            let amount_buttons =
                |ui: &mut egui::Ui, out: &mut Commands, from_buffer: bool, id: &str| {
                    ui.horizontal_wrapped(|ui| {
                        for (i, a) in gameplay::chemlab::MASTER_AMOUNTS.iter().enumerate() {
                            let text = if *a == f32::MAX {
                                "всё".to_string()
                            } else {
                                format!("{a:.0}")
                            };
                            if ui.small_button(text).clicked() {
                                let mut d = vec![u8::from(from_buffer), i as u8];
                                d.extend_from_slice(id.as_bytes());
                                out.push((gameplay::CMD_LAB_REAGENT, d));
                            }
                        }
                    });
                };
            if let Some(c) = &v.input {
                for (id, name, q) in &c.reagents {
                    ui.label(format!("{name}: {q:.1}"));
                    amount_buttons(ui, &mut out, false, id);
                }
            }
            ui.separator();
            ui.label(format!("Буфер: {:.0} ед.", v.buffer_volume.max(0.0) + 0.0));
            for (id, name, q) in &v.buffer {
                ui.label(format!("{name}: {q:.1}"));
                amount_buttons(ui, &mut out, true, id);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Брать из:");
                if ui.selectable_label(!v.external, "буфера").clicked() {
                    out.push((gameplay::CMD_LAB_SOURCE, vec![0]));
                }
                if ui.selectable_label(v.external, "мензурки").clicked() {
                    out.push((gameplay::CMD_LAB_SOURCE, vec![1]));
                }
            });
            container(ui, "Выход", &v.output, 1, &mut out, true);
            let state = ui.data_mut(|d| {
                d.get_temp_mut_or_insert_with(Id::new("lab-form"), || (5u8, 1u8, String::new()))
                    .clone()
            });
            let (mut dose, mut number, mut label) = state;
            ui.horizontal(|ui| {
                ui.label("Доза");
                ui.add(egui::DragValue::new(&mut dose).range(1..=v.dosage_limit.min(255) as u8));
                ui.label("Штук");
                ui.add(egui::DragValue::new(&mut number).range(1..=20));
                ui.label("Ярлык");
                ui.text_edit_singleline(&mut label);
            });
            ui.data_mut(|d| d.insert_temp(Id::new("lab-form"), (dose, number, label.clone())));
            ui.horizontal(|ui| {
                if ui.button("Таблетки").clicked() {
                    let mut d = vec![dose, number];
                    d.extend_from_slice(label.as_bytes());
                    out.push((gameplay::CMD_LAB_PILLS, d));
                }
                if ui.button("В бутылку").clicked() {
                    let mut d = vec![dose, 1];
                    d.extend_from_slice(label.as_bytes());
                    out.push((gameplay::CMD_LAB_BOTTLE, d));
                }
            });
        });
    if !open {
        out.push((gameplay::CMD_LAB_CLOSE, Vec::new()));
    }
    out
}

/// Окно анализатора здоровья (`HealthAnalyzerWindow`).
pub fn scan_window(ctx: &egui::Context, view: Option<&gameplay::ScanView>) -> Commands {
    let mut out: Commands = Vec::new();
    let Some(v) = view else {
        return out;
    };
    let mut open = true;
    egui::Window::new("Анализатор здоровья")
        .id(egui::Id::new("scan"))
        .default_pos(Pos2::new(900.0, 120.0))
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            if v.active {
                ui.colored_label(Color32::LIGHT_GREEN, "Сканирование идёт");
            } else {
                ui.colored_label(Color32::LIGHT_RED, "Цель вне зоны: данные устарели");
            }
            ui.label(format!("Цель: {}", v.target));
            ui.label(format!("Состояние: {}", v.state));
            ui.label(format!(
                "Температура: {:.1} °C ({:.1} K)",
                v.temperature_k - 273.15,
                v.temperature_k
            ));
            ui.label(format!("Кровь: {:.0}%", v.blood * 100.0));
            if v.bleeding {
                ui.colored_label(Color32::LIGHT_RED, "Кровотечение");
            }
            ui.label(format!("Суммарный урон: {:.1}", v.total_damage));
            for (group, total, kinds) in &v.groups {
                ui.label(format!("{group}: {total:.1}"));
                for (k, a) in kinds {
                    ui.label(format!("   · {k}: {a:.1}"));
                }
            }
        });
    if !open {
        out.push((gameplay::CMD_SCAN_CLOSE, Vec::new()));
    }
    out
}

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

/// Выбор количества у ёмкости или шприца в активной руке: кнопки из прототипа.
pub fn chemhand_bar(ctx: &egui::Context, view: Option<&gameplay::ChemHandView>) -> Commands {
    let mut out: Commands = Vec::new();
    let Some(v) = view.filter(|v| !v.amounts.is_empty()) else {
        return out;
    };
    egui::Area::new(egui::Id::new("chemhand"))
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -150.0])
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let what = match v.mode.as_str() {
                        "" => "Перелив",
                        "Inject" => "Шприц: ввод",
                        "Draw" => "Шприц: набор",
                        _ => "Шприц",
                    };
                    ui.label(what);
                    for (i, a) in v.amounts.iter().enumerate() {
                        let on = (a - v.amount).abs() < 1e-3;
                        if ui.selectable_label(on, format!("{a:.0}")).clicked() {
                            out.push((gameplay::CMD_CHEM_AMOUNT, vec![v.hand, i as u8]));
                        }
                    }
                });
            });
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, f: impl FnOnce(&egui::Context)) -> usize {
        let mut f = Some(f);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(f) = f.take() {
                f(ui.ctx());
            }
        });
        out.textures_delta.clear();
        out.shapes.len()
    }

    #[test]
    fn the_examine_window_draws_until_it_is_closed() {
        let ctx = egui::Context::default();
        let mut state = HudState::default();
        let view = gameplay::ExamineView {
            seq: 3,
            name: "crowbar".into(),
            lines: vec!["A multipurpose tool.".into()],
        };
        // Первый кадр раскладывает окно, второй рисует его.
        frame(&ctx, |c| examine_window(c, &mut state, Some(&view)));
        let drawn = frame(&ctx, |c| examine_window(c, &mut state, Some(&view)));
        assert!(drawn > 0, "окно осмотра нарисовано");
        // Без ответа или закрытое на этом ответе окна нет.
        assert_eq!(frame(&ctx, |c| examine_window(c, &mut state, None)), 0);
        state.examine_closed = 3;
        assert_eq!(
            frame(&ctx, |c| examine_window(c, &mut state, Some(&view))),
            0
        );
        // Новый ответ открывает его снова.
        let next = gameplay::ExamineView { seq: 4, ..view };
        frame(&ctx, |c| examine_window(c, &mut state, Some(&next)));
        assert!(frame(&ctx, |c| examine_window(c, &mut state, Some(&next))) > 0);
    }

    #[test]
    fn the_verb_menu_waits_for_the_answer_to_its_own_request() {
        let ctx = egui::Context::default();
        let target = gameplay::VerbTarget::Player(astrocore_core::NetworkEntityId(5));
        let mut state = HudState {
            menu: Some(VerbMenu {
                at: Pos2::new(10.0, 10.0),
                target,
                after_seq: 7,
            }),
            ..HudState::default()
        };
        let mut answer = gameplay::VerbsView {
            seq: 7,
            target: gameplay::target_data(target, None),
            verbs: vec![gameplay::Verb {
                id: "examine".into(),
                label: "Examine".into(),
                disabled: None,
            }],
        };
        // Старый ответ (тот же номер) меню не открывает.
        frame(&ctx, |c| {
            verb_menu(c, &mut state, Some(&answer));
        });
        assert_eq!(
            frame(&ctx, |c| {
                verb_menu(c, &mut state, Some(&answer));
            }),
            0
        );
        // Новый ответ по этой цели — рисует.
        answer.seq = 8;
        frame(&ctx, |c| {
            verb_menu(c, &mut state, Some(&answer));
        });
        assert!(
            frame(&ctx, |c| {
                verb_menu(c, &mut state, Some(&answer));
            }) > 0
        );
        // Ответ по другой цели — нет.
        answer.target = gameplay::target_data(
            gameplay::VerbTarget::Player(astrocore_core::NetworkEntityId(6)),
            None,
        );
        assert_eq!(
            frame(&ctx, |c| {
                verb_menu(c, &mut state, Some(&answer));
            }),
            0
        );
    }
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
